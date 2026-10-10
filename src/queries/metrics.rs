//! Focus metrics derived from focus coverage at query time: context switches
//! and focus stretches. Nothing is recorded for them. They describe observed
//! focus — not task time, and not a judgement of the work.
//!
//! Both are measured within *active runs*: adjacent coverage intervals of one
//! timing segment and recording session whose evidence is known, non-idle
//! focus or a confirmed absence of focus. Idle periods, recording gaps,
//! pause/lock, unknown time and timing-segment boundaries end a run, so no
//! switch or stretch spans them. Elapsed time is monotonic within a segment;
//! uncertain intervals contribute none. Metrics are clipped to the requested
//! range: a transition exactly at its start has no observed predecessor.
use super::*;
use crate::categories::Categories;
use chrono::{DateTime, Offset, TimeZone};

/// An interruption at least this long ends a focus stretch. Shorter ones —
/// a glance at chat, passing through an empty workspace — are bridged when the
/// stretch then returns to its subject.
pub const BRIEF_INTERRUPTION_MS: i64 = 60_000;
/// Bridged interruptions may make up at most this share of a stretch's elapsed
/// time, so rapid alternation between two subjects is never one long stretch.
pub const MAX_INTERRUPTED_PERCENT: i64 = 10;
/// Stretch lengths of the fragmentation distribution: long stretches last at
/// least 25 minutes, medium ones at least 10.
pub const LONG_STRETCH_MS: i64 = 25 * 60_000;
pub const MEDIUM_STRETCH_MS: i64 = 10 * 60_000;
/// More subject changes than this within one interruption end the stretch,
/// bounding the replay buffer however rapidly focus changes.
const MAX_PENDING: usize = 64;

/// What a focus stretch stays on: the category of categorized focus, otherwise
/// the application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FocusSubject {
    Category { name: String },
    Application { name: String },
}
#[derive(Clone, Copy)]
enum Subject<'a> {
    Category(&'a str),
    Application(&'a str),
}
impl FocusSubject {
    fn is(&self, other: Subject) -> bool {
        match (self, other) {
            (Self::Category { name }, Subject::Category(other))
            | (Self::Application { name }, Subject::Application(other)) => name == other,
            _ => false,
        }
    }
    fn borrow(&self) -> Subject<'_> {
        match self {
            Self::Category { name } => Subject::Category(name),
            Self::Application { name } => Subject::Application(name),
        }
    }
}
impl Subject<'_> {
    fn owned(self) -> FocusSubject {
        match self {
            Self::Category(name) => FocusSubject::Category { name: name.into() },
            Self::Application(name) => FocusSubject::Application { name: name.into() },
        }
    }
}

/// A continuous run of non-idle focus on one subject, ending at idle, a gap,
/// a timing boundary, or a switch that is not a bridged brief interruption.
#[derive(Debug, Clone, Serialize)]
pub struct FocusStretch {
    /// From the first to the last focus on its subject.
    pub range: Range,
    /// Non-idle focus time, including focus during bridged interruptions.
    pub elapsed_ms: i64,
    pub subject: FocusSubject,
    /// Bridged brief interruptions.
    pub interruptions: i64,
}
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct StretchBucket {
    pub count: i64,
    pub elapsed_ms: i64,
}
/// Every stretch by length. The three elapsed totals add up to `active_ms`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct StretchDistribution {
    /// At least 25 minutes.
    pub long: StretchBucket,
    /// At least 10 and under 25 minutes.
    pub medium: StretchBucket,
    /// Under 10 minutes.
    pub short: StretchBucket,
}
/// Switches and active focus within one local clock hour, clipped to the range.
#[derive(Debug, Clone, Serialize)]
pub struct HourlyFocus {
    pub range: Range,
    pub active_ms: i64,
    pub application_switches: i64,
    pub category_switches: Option<i64>,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct FocusMetrics {
    /// Non-idle focus time: the rate denominator and the sum of all stretches.
    pub active_ms: i64,
    /// Changes of application between consecutive focus within active runs.
    pub application_switches: i64,
    /// Changes of category, with uncategorized as one more value; null
    /// without configured categories.
    pub category_switches: Option<i64>,
    /// Switches per hour of active focus; null without active focus.
    pub application_switches_per_hour: Option<f64>,
    pub category_switches_per_hour: Option<f64>,
    /// The earliest of the longest stretches.
    pub longest_stretch: Option<FocusStretch>,
    pub stretches: StretchDistribution,
    /// Local clock hours with active focus or a switch, in time order.
    pub hours: Vec<HourlyFocus>,
}

/// A stretch still being extended.
struct Open {
    subject: FocusSubject,
    range: Range,
    subject_ms: i64,
    /// Elapsed bridged interruptions, of which `bridged_focus_ms` had focus.
    bridged_ms: i64,
    bridged_focus_ms: i64,
    interruptions: i64,
}
/// Focus on other subjects, or no focus, since the open stretch's subject.
struct Piece {
    subject: Option<FocusSubject>,
    range: Range,
    elapsed_ms: i64,
}

#[derive(Default)]
struct Stretches {
    open: Option<Open>,
    pending: Vec<Piece>,
    pending_ms: i64,
    longest: Option<FocusStretch>,
    distribution: StretchDistribution,
}
impl Stretches {
    fn piece(&mut self, subject: Option<Subject>, range: Range, elapsed_ms: i64) {
        let Some(open) = &mut self.open else {
            // Stretches start with focus; no focus between them belongs to none.
            if let Some(subject) = subject {
                self.open = Some(Open {
                    subject: subject.owned(),
                    range,
                    subject_ms: elapsed_ms,
                    bridged_ms: 0,
                    bridged_focus_ms: 0,
                    interruptions: 0,
                });
            }
            return;
        };
        if subject.is_some_and(|subject| open.subject.is(subject)) {
            if !self.pending.is_empty() {
                let interrupted = open.bridged_ms + self.pending_ms;
                if interrupted * (100 - MAX_INTERRUPTED_PERCENT)
                    > MAX_INTERRUPTED_PERCENT * open.subject_ms
                {
                    self.close_and_replay();
                    return self.piece(subject, range, elapsed_ms);
                }
                open.bridged_ms = interrupted;
                open.bridged_focus_ms += self
                    .pending
                    .iter()
                    .filter(|piece| piece.subject.is_some())
                    .map(|piece| piece.elapsed_ms)
                    .sum::<i64>();
                open.interruptions += 1;
                self.pending.clear();
                self.pending_ms = 0;
            }
            open.range.end_ms = range.end_ms;
            open.subject_ms += elapsed_ms;
            return;
        }
        self.pending_ms += elapsed_ms;
        match self.pending.last_mut() {
            Some(last)
                if last.range.end_ms == range.start_ms
                    && match (&last.subject, subject) {
                        (Some(last), Some(subject)) => last.is(subject),
                        (None, None) => true,
                        _ => false,
                    } =>
            {
                last.range.end_ms = range.end_ms;
                last.elapsed_ms += elapsed_ms;
            }
            _ => self.pending.push(Piece {
                subject: subject.map(Subject::owned),
                range,
                elapsed_ms,
            }),
        }
        if self.pending_ms >= BRIEF_INTERRUPTION_MS || self.pending.len() > MAX_PENDING {
            self.close_and_replay();
        }
    }
    /// Ends the open stretch before its unbridged interruption, which then
    /// starts the next stretches.
    fn close_and_replay(&mut self) {
        if let Some(open) = self.open.take() {
            let stretch = FocusStretch {
                range: open.range,
                elapsed_ms: open.subject_ms + open.bridged_focus_ms,
                subject: open.subject,
                interruptions: open.interruptions,
            };
            let bucket = match stretch.elapsed_ms {
                ms if ms >= LONG_STRETCH_MS => &mut self.distribution.long,
                ms if ms >= MEDIUM_STRETCH_MS => &mut self.distribution.medium,
                _ => &mut self.distribution.short,
            };
            bucket.count += 1;
            bucket.elapsed_ms += stretch.elapsed_ms;
            if self
                .longest
                .as_ref()
                .is_none_or(|longest| stretch.elapsed_ms > longest.elapsed_ms)
            {
                self.longest = Some(stretch);
            }
        }
        self.pending_ms = 0;
        for piece in std::mem::take(&mut self.pending) {
            self.piece(
                piece.subject.as_ref().map(FocusSubject::borrow),
                piece.range,
                piece.elapsed_ms,
            );
        }
    }
    /// The active run ended: nothing after it can bridge an interruption.
    fn end_run(&mut self) {
        while self.open.is_some() || !self.pending.is_empty() {
            self.close_and_replay();
        }
    }
}

/// The end of the active run that the next interval may continue.
struct RunEnd {
    segment: Option<i64>,
    recording_boundary: i64,
    end_ms: i64,
}

/// Streams focus coverage of one range, in scan order, into its metrics.
pub(super) struct Metrics {
    range: Range,
    timezone: Tz,
    categorized: bool,
    result: FocusMetrics,
    run: Option<RunEnd>,
    /// Application and category of the run's latest focus.
    last: Option<(String, Option<String>)>,
    stretches: Stretches,
    hours: BTreeMap<i64, HourlyFocus>,
    /// Start of the latest hour bucket, which most intervals fall into.
    latest_hour: Option<i64>,
}
impl Metrics {
    pub fn new(range: Range, timezone: Tz, categorized: bool) -> Self {
        Self {
            range,
            timezone,
            categorized,
            result: FocusMetrics {
                category_switches: categorized.then_some(0),
                ..FocusMetrics::default()
            },
            run: None,
            last: None,
            stretches: Stretches::default(),
            hours: BTreeMap::new(),
            latest_hour: None,
        }
    }
    pub fn add(&mut self, coverage: &Coverage) {
        let (focus, elapsed) = match (&coverage.focus, coverage.elapsed_ms) {
            (
                FocusCoverage::Focus {
                    window,
                    idle: false,
                    category,
                },
                Some(elapsed),
            ) => (Some((window, category)), elapsed),
            (FocusCoverage::NoFocus { idle: false }, Some(elapsed)) => (None, elapsed),
            _ => return self.end_run(),
        };
        let continuous = self.run.as_ref().is_some_and(|run| {
            run.segment == coverage.segment
                && run.recording_boundary == coverage.recording_boundary
                && run.end_ms == coverage.range.start_ms
        });
        if !continuous {
            self.end_run();
        }
        self.run = Some(RunEnd {
            segment: coverage.segment,
            recording_boundary: coverage.recording_boundary,
            end_ms: coverage.range.end_ms,
        });
        let Some((window, category)) = focus else {
            self.stretches.piece(None, coverage.range, elapsed);
            return;
        };
        self.result.active_ms += elapsed;
        self.hourly(coverage.range, elapsed);
        let (application_switch, category_switch) = match &self.last {
            Some((application, last_category)) => (
                *application != window.application,
                self.categorized && last_category != category,
            ),
            None => (false, false),
        };
        if application_switch || category_switch || self.last.is_none() {
            self.last = Some((window.application.clone(), category.clone()));
        }
        if application_switch || category_switch {
            let (application, category) =
                (i64::from(application_switch), i64::from(category_switch));
            self.result.application_switches += application;
            if let Some(switches) = &mut self.result.category_switches {
                *switches += category;
            }
            let hour = self.hour(coverage.range.start_ms);
            hour.application_switches += application;
            if let Some(switches) = &mut hour.category_switches {
                *switches += category;
            }
        }
        let subject = match category {
            Some(name) => Subject::Category(name),
            None => Subject::Application(&window.application),
        };
        self.stretches.piece(Some(subject), coverage.range, elapsed);
    }
    fn end_run(&mut self) {
        self.run = None;
        self.last = None;
        self.stretches.end_run();
    }
    /// The bucket of the local clock hour containing `ms`. Offsets change on
    /// whole hours, so the latest bucket needs no timezone lookup.
    fn hour(&mut self, ms: i64) -> &mut HourlyFocus {
        let start = match self.latest_hour {
            Some(start) if (start..start + HOUR).contains(&ms) => start,
            _ => {
                let utc = DateTime::from_timestamp_millis(ms).unwrap_or_default();
                let offset = i64::from(
                    self.timezone
                        .offset_from_utc_datetime(&utc.naive_utc())
                        .fix()
                        .local_minus_utc(),
                ) * 1000;
                ms - (ms + offset).rem_euclid(HOUR)
            }
        };
        self.latest_hour = Some(start);
        let range = self.range;
        let categorized = self.categorized;
        self.hours.entry(start).or_insert_with(|| HourlyFocus {
            range: range.clip(start, start + HOUR).unwrap_or(Range {
                start_ms: ms,
                end_ms: ms + 1,
            }),
            active_ms: 0,
            application_switches: 0,
            category_switches: categorized.then_some(0),
        })
    }
    /// Spreads active focus over the clock hours it overlaps.
    fn hourly(&mut self, range: Range, elapsed: i64) {
        let mut start = range.start_ms;
        while start < range.end_ms {
            let hour = self.hour(start);
            let end = hour.range.end_ms.min(range.end_ms).max(start + 1);
            hour.active_ms += coverage::proportional(
                elapsed,
                range,
                Range {
                    start_ms: start,
                    end_ms: end,
                },
            );
            start = end;
        }
    }
    pub fn finish(mut self) -> FocusMetrics {
        self.end_run();
        let mut result = self.result;
        let active = result.active_ms;
        result.application_switches_per_hour = per_hour(result.application_switches, active);
        result.category_switches_per_hour =
            result.category_switches.and_then(|n| per_hour(n, active));
        result.longest_stretch = self.stretches.longest;
        result.stretches = self.stretches.distribution;
        result.hours = self.hours.into_values().collect();
        result
    }
}

/// Switches per hour of active focus, to one decimal: finer precision would
/// overstate what focus events show.
fn per_hour(switches: i64, active_ms: i64) -> Option<f64> {
    (active_ms > 0)
        .then(|| (switches as f64 * HOUR as f64 / active_ms as f64 * 10.0).round() / 10.0)
}

/// Focus on one category, or on one application, within an activity day.
#[derive(Debug, Clone, Serialize)]
pub struct CategoryFocus {
    /// Null for uncategorized focus.
    pub category: Option<String>,
    /// Recorded focus, including idle periods.
    pub focus_ms: i64,
    /// Focus outside idle periods.
    pub active_ms: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct ApplicationFocus {
    pub application: String,
    pub focus_ms: i64,
    pub active_ms: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// Focus evidence: a focused window, idle or not, or a confirmed absence of focus.
    Observed,
    /// A recording gap with its reason, such as a pause, lock or restart.
    Gap,
    /// No evidence.
    Unknown,
}
/// A maximal run of one kind of evidence, in wall-clock time.
#[derive(Debug, Clone, Serialize)]
pub struct EvidenceInterval {
    pub range: Range,
    pub kind: EvidenceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
/// Which part of a day has evidence, in wall-clock time. Recording gaps and
/// unknown time count only before the query time: the future is neither, so
/// the three totals add up to the elapsed part of the day.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DayEvidence {
    pub observed_ms: i64,
    pub gap_ms: i64,
    pub unknown_ms: i64,
    pub intervals: Vec<EvidenceInterval>,
}

/// Focus totals and metrics of one activity day, clipped to the requested range.
#[derive(Debug, Serialize)]
pub struct DayFocusMetrics {
    /// Activity date, YYYY-MM-DD.
    pub date: String,
    pub range: Range,
    pub totals: Totals,
    /// Focus by category: configured categories in order, then uncategorized,
    /// each only with focus. Empty without configured categories.
    pub categories: Vec<CategoryFocus>,
    /// Focus by application, most first.
    pub applications: Vec<ApplicationFocus>,
    pub evidence: DayEvidence,
    pub metrics: FocusMetrics,
}
#[derive(Debug, Serialize)]
pub struct FocusMetricsByDay {
    pub resolved: ResolvedRange,
    pub days: Vec<DayFocusMetrics>,
}

/// Accumulates one activity day. `bytes` accounts subject and evidence entries
/// across all days against the breakdown budget.
struct Day {
    date: NaiveDate,
    range: Range,
    totals: Totals,
    categories: HashMap<Option<String>, (i64, i64)>,
    applications: HashMap<String, (i64, i64)>,
    evidence: DayEvidence,
    metrics: Metrics,
}
impl Day {
    fn add(&mut self, coverage: &Coverage, now: i64, bytes: &mut usize) -> Result<()> {
        self.totals.add(coverage);
        self.metrics.add(coverage);
        let elapsed = coverage.elapsed_ms.unwrap_or(0);
        let (kind, reason) = match &coverage.focus {
            FocusCoverage::Focus {
                window,
                idle,
                category,
            } => {
                let active = if *idle { 0 } else { elapsed };
                let entry = match self.categories.get_mut(category) {
                    Some(entry) => entry,
                    None => {
                        *bytes += category.as_ref().map_or(0, String::len) + 64;
                        self.categories.entry(category.clone()).or_default()
                    }
                };
                entry.0 += elapsed;
                entry.1 += active;
                let entry = match self.applications.get_mut(&window.application) {
                    Some(entry) => entry,
                    None => {
                        *bytes += window.application.len() + 64;
                        self.applications
                            .entry(window.application.clone())
                            .or_default()
                    }
                };
                entry.0 += elapsed;
                entry.1 += active;
                (EvidenceKind::Observed, None)
            }
            FocusCoverage::NoFocus { .. } => (EvidenceKind::Observed, None),
            FocusCoverage::Gap { reason } => (EvidenceKind::Gap, Some(reason)),
            FocusCoverage::Unknown => (EvidenceKind::Unknown, None),
        };
        let mut range = coverage.range;
        if kind != EvidenceKind::Observed {
            match range.clip(range.start_ms, now) {
                Some(past) => range = past,
                None => return Ok(()),
            }
        }
        let width = range.end_ms - range.start_ms;
        let evidence = &mut self.evidence;
        *match kind {
            EvidenceKind::Observed => &mut evidence.observed_ms,
            EvidenceKind::Gap => &mut evidence.gap_ms,
            EvidenceKind::Unknown => &mut evidence.unknown_ms,
        } += width;
        match evidence.intervals.last_mut() {
            Some(last)
                if last.range.end_ms == range.start_ms
                    && last.kind == kind
                    && last.reason.as_ref() == reason =>
            {
                last.range.end_ms = range.end_ms;
            }
            _ => {
                *bytes += reason.map_or(0, String::len) + 64;
                evidence.intervals.push(EvidenceInterval {
                    range,
                    kind,
                    reason: reason.cloned(),
                });
            }
        }
        if *bytes > BREAKDOWN_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        Ok(())
    }
    fn finish(self, categories: &Categories) -> DayFocusMetrics {
        let mut by_category = self.categories;
        let configured = !categories.list().is_empty();
        let categories = categories
            .list()
            .iter()
            .map(|category| Some(category.name.clone()))
            .chain(configured.then_some(None))
            .filter_map(|category| {
                let (focus_ms, active_ms) = by_category.remove(&category)?;
                (focus_ms > 0 || active_ms > 0).then_some(CategoryFocus {
                    category,
                    focus_ms,
                    active_ms,
                })
            })
            .collect();
        let mut applications: Vec<_> = self
            .applications
            .into_iter()
            .map(|(application, (focus_ms, active_ms))| ApplicationFocus {
                application,
                focus_ms,
                active_ms,
            })
            .collect();
        applications.sort_by(|a, b| {
            b.focus_ms
                .cmp(&a.focus_ms)
                .then_with(|| a.application.cmp(&b.application))
        });
        DayFocusMetrics {
            date: self.date.to_string(),
            range: self.range,
            totals: self.totals,
            categories,
            applications,
            evidence: self.evidence,
            metrics: self.metrics.finish(),
        }
    }
}

impl Queries {
    pub(super) fn metrics(&self, range: Range) -> Metrics {
        Metrics::new(
            range,
            self.timezone,
            !self.settings.categories.list().is_empty(),
        )
    }
    /// Totals, focus by category and application, evidence and focus metrics per
    /// activity day of a selection up to 31 days, from one coverage pass without
    /// image coverage. Each day's metrics equal the overview metrics of that day
    /// clipped to the range.
    pub fn focus_metrics(&self, client: Client, selection: Selection) -> Result<FocusMetricsByDay> {
        let resolved = self.resolve(selection)?;
        let range = resolved.range;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let mut days = Vec::new();
        let start = self
            .timezone
            .timestamp_millis_opt(range.start_ms)
            .single()
            .ok_or(QueryError::InvalidRequest)?
            .naive_local()
            - chrono::Duration::hours(4);
        let mut date = start.date();
        loop {
            let day = self.resolve(Selection::ActivityDate(date))?.range;
            if day.start_ms >= range.end_ms {
                break;
            }
            if let Some(clipped) = range.clip(day.start_ms, day.end_ms) {
                days.push(Day {
                    date,
                    range: clipped,
                    totals: Totals::default(),
                    categories: HashMap::new(),
                    applications: HashMap::new(),
                    evidence: DayEvidence::default(),
                    metrics: self.metrics(clipped),
                });
            }
            date = date.succ_opt().ok_or(QueryError::InvalidRequest)?;
        }
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor("focus_metrics".into(), &PageRequest::default())?;
        let plan = self.plan(&cursor, range, Scope::Evidence)?;
        let mut bytes = 0;
        self.coverage(&cursor, range, &plan, None, |coverage| {
            let first = days.partition_point(|day| day.range.end_ms <= coverage.range.start_ms);
            for day in &mut days[first..] {
                if day.range.start_ms >= coverage.range.end_ms {
                    break;
                }
                if day.range.start_ms <= coverage.range.start_ms
                    && coverage.range.end_ms <= day.range.end_ms
                {
                    day.add(&coverage, now, &mut bytes)?;
                } else if let Some(clipped) = day
                    .range
                    .clip(coverage.range.start_ms, coverage.range.end_ms)
                {
                    let piece = Coverage {
                        range: clipped,
                        elapsed_ms: coverage.elapsed_ms.map(|elapsed| {
                            coverage::proportional(elapsed, coverage.range, clipped)
                        }),
                        ..coverage.clone()
                    };
                    day.add(&piece, now, &mut bytes)?;
                }
            }
            Ok(())
        })?;
        Ok(FocusMetricsByDay {
            resolved,
            days: days
                .into_iter()
                .map(|day| day.finish(&self.settings.categories))
                .collect(),
        })
    }
}
