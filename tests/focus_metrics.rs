//! Context switches and focus stretches over real temporary SQLite histories.
use chrono::TimeZone;
use timeline::{
    evidence::{BoundaryKind, Evidence, FocusState, StateChange},
    history::History,
    queries::{Client::*, *},
    recording::Window,
    settings::Settings,
};

const S: i64 = 1000;
const MIN: i64 = 60 * S;
const CATEGORIES: &str = r#"
[[categories]]
name = "Code"
[[categories.rules]]
application = "editor"
[[categories.rules]]
application = "terminal"

[[categories]]
name = "Chat"
[[categories.rules]]
application = "chat"
"#;

/// Appends evidence with wall time equal to monotonic time unless a test
/// introduces a boundary.
struct Recording {
    history: History,
    wall: i64,
    segment: i64,
    last: FocusState,
}
fn focus_state(application: Option<&str>, idle: bool) -> FocusState {
    FocusState {
        manual_paused: false,
        connected: true,
        locked: false,
        idle,
        focus: application.map(|application| Window {
            address: application.into(),
            application: application.into(),
            title: format!("{application} window"),
        }),
        focus_known: true,
        screenshot_recording: true,
        screenshot_pressure: false,
        screenshot_cleanup: false,
    }
}
impl Recording {
    fn new(dir: &std::path::Path, start: i64) -> Self {
        Self {
            history: History::open(dir).unwrap(),
            wall: start,
            segment: 1,
            last: focus_state(None, false),
        }
    }
    fn commit(&mut self, kind: StateChange, state: FocusState) {
        self.history
            .commit_evidence(
                self.wall,
                self.wall,
                self.segment,
                &Evidence::State {
                    kind,
                    state: state.clone(),
                },
                None,
            )
            .unwrap();
        self.last = state;
    }
    /// Focus on `application` (None: no focused window) for `ms`.
    fn focus(&mut self, application: Option<&str>, ms: i64) -> &mut Self {
        self.commit(StateChange::Focus, focus_state(application, false));
        self.wall += ms;
        self
    }
    /// The current focus turns idle for `ms`, then input resumes.
    fn idle(&mut self, ms: i64) -> &mut Self {
        let mut state = self.last.clone();
        state.idle = true;
        self.commit(StateChange::Idle, state);
        self.wall += ms;
        let mut state = self.last.clone();
        state.idle = false;
        self.commit(StateChange::Idle, state);
        self
    }
    fn pause(&mut self, ms: i64) -> &mut Self {
        let mut paused = focus_state(None, false);
        paused.manual_paused = true;
        paused.focus_known = false;
        self.commit(StateChange::Pause, paused);
        self.wall += ms;
        self.commit(StateChange::Resume, focus_state(None, false));
        self
    }
    /// A clock discontinuity starts a new timing segment.
    fn discontinuity(&mut self) -> &mut Self {
        self.end();
        self.segment += 1;
        self.history
            .commit_evidence(
                self.wall,
                self.wall,
                self.segment,
                &Evidence::Boundary {
                    kind: BoundaryKind::Boundary,
                    reason: "clock_discontinuity".into(),
                    previous_durable_observation: None,
                    previous_durable_unix_ms: None,
                },
                None,
            )
            .unwrap();
        self
    }
    /// Checkpoints the current focus so the last interval has an end.
    fn end(&mut self) -> i64 {
        let state = self.last.clone();
        self.commit(StateChange::Checkpoint, state);
        self.wall
    }
}

fn queries(dir: &std::path::Path, categories: bool) -> Queries {
    let settings = Settings::parse(if categories { CATEGORIES } else { "" }).unwrap();
    Queries::with_policy(dir, "Europe/Oslo", Box::new(ExactTitle))
        .unwrap()
        .with_settings(settings.into())
}
fn metrics(q: &Queries, start_ms: i64, end_ms: i64) -> FocusMetrics {
    q.overview(
        Mcp,
        Selection::Range(Range { start_ms, end_ms }),
        PageRequest::default(),
    )
    .unwrap()
    .metrics
}
fn stretches(m: &FocusMetrics) -> [(i64, i64); 3] {
    let s = &m.stretches;
    [
        (s.long.count, s.long.elapsed_ms),
        (s.medium.count, s.medium.elapsed_ms),
        (s.short.count, s.short.elapsed_ms),
    ]
}
fn category(name: &str) -> FocusSubject {
    FocusSubject::Category { name: name.into() }
}

#[test]
fn switches_count_application_and_category_changes_and_stretches_bridge_glances() {
    let dir = tempfile::tempdir().unwrap();
    let end = Recording::new(dir.path(), 0)
        .focus(Some("editor"), 10 * MIN)
        .focus(Some("terminal"), 10 * MIN)
        .focus(Some("chat"), 30 * S)
        .focus(Some("editor"), 9 * MIN + 30 * S)
        .focus(Some("chat"), MIN)
        .focus(Some("editor"), 14 * MIN)
        .end();
    assert_eq!(end, 45 * MIN);

    let m = metrics(&queries(dir.path(), true), 0, end);
    assert_eq!(m.active_ms, 45 * MIN);
    assert_eq!(m.application_switches, 5);
    assert_eq!(m.category_switches, Some(4), "editor → terminal stays Code");
    assert_eq!(m.application_switches_per_hour, Some(6.7));
    assert_eq!(m.category_switches_per_hour, Some(5.3));
    let longest = m.longest_stretch.as_ref().unwrap();
    assert_eq!(
        (longest.range.start_ms, longest.range.end_ms),
        (0, 30 * MIN),
        "a 30 s glance at chat is bridged; a full minute is not"
    );
    assert_eq!(longest.elapsed_ms, 30 * MIN);
    assert_eq!(longest.subject, category("Code"));
    assert_eq!(longest.interruptions, 1);
    assert_eq!(
        stretches(&m),
        [(1, 30 * MIN), (1, 14 * MIN), (1, MIN)],
        "stretches partition active focus"
    );

    // Without categories, stretches follow the application.
    let m = metrics(&queries(dir.path(), false), 0, end);
    assert_eq!(m.application_switches, 5);
    assert_eq!(m.category_switches, None);
    assert_eq!(m.category_switches_per_hour, None);
    let longest = m.longest_stretch.as_ref().unwrap();
    assert_eq!(
        longest.subject,
        FocusSubject::Application {
            name: "editor".into()
        }
    );
    assert_eq!(longest.elapsed_ms, 14 * MIN);
    assert_eq!(
        stretches(&m),
        [(0, 0), (3, 34 * MIN), (3, 11 * MIN)],
        "editor 10, terminal 10, chat 0.5, editor 9.5, chat 1, editor 14"
    );
}

/// Editor for `before`, chat for `glance`, then editor for 20 minutes.
fn glance(before: i64, glance: i64) -> FocusMetrics {
    let dir = tempfile::tempdir().unwrap();
    let end = Recording::new(dir.path(), 0)
        .focus(Some("editor"), before)
        .focus(Some("chat"), glance)
        .focus(Some("editor"), 20 * MIN)
        .end();
    metrics(&queries(dir.path(), true), 0, end)
}

#[test]
fn brief_interruption_tolerance_edges() {
    assert_eq!(BRIEF_INTERRUPTION_MS, 60 * S);
    let bridged = glance(20 * MIN, BRIEF_INTERRUPTION_MS - 1);
    let longest = bridged.longest_stretch.as_ref().unwrap();
    assert_eq!(longest.elapsed_ms, 41 * MIN - 1);
    assert_eq!(longest.interruptions, 1);
    assert_eq!(bridged.stretches.long.count, 1);
    assert_eq!(
        bridged.application_switches, 2,
        "a bridged glance still switches"
    );

    let broken = glance(20 * MIN, BRIEF_INTERRUPTION_MS);
    assert_eq!(
        broken.longest_stretch.as_ref().unwrap().elapsed_ms,
        20 * MIN
    );
    assert_eq!(stretches(&broken), [(0, 0), (2, 40 * MIN), (1, MIN)]);

    // Bridged interruptions stay within a tenth of the stretch: 30 s needs
    // 4.5 minutes on the subject first.
    assert_eq!(MAX_INTERRUPTED_PERCENT, 10);
    let settled = glance(4 * MIN + 30 * S, 30 * S);
    assert_eq!(
        settled.longest_stretch.as_ref().unwrap().elapsed_ms,
        25 * MIN
    );
    let unsettled = glance(4 * MIN + 29 * S, 30 * S);
    assert_eq!(
        unsettled.longest_stretch.as_ref().unwrap().elapsed_ms,
        20 * MIN
    );
    assert_eq!(unsettled.stretches.short.count, 2);
}

#[test]
fn rapid_alternation_is_switching_not_one_long_stretch() {
    let dir = tempfile::tempdir().unwrap();
    let mut recording = Recording::new(dir.path(), 0);
    for _ in 0..30 {
        recording
            .focus(Some("editor"), 30 * S)
            .focus(Some("chat"), 30 * S);
    }
    let end = recording.end();
    let m = metrics(&queries(dir.path(), true), 0, end);
    assert_eq!(m.application_switches, 59);
    assert_eq!(m.application_switches_per_hour, Some(118.0));
    assert_eq!(m.longest_stretch.as_ref().unwrap().elapsed_ms, 30 * S);
    assert_eq!(stretches(&m), [(0, 0), (0, 0), (60, 30 * MIN)]);

    // Very many subject changes within one interruption end the stretch rather
    // than growing the replay buffer; all time still lands in a stretch.
    let dir = tempfile::tempdir().unwrap();
    let mut recording = Recording::new(dir.path(), 0);
    recording.focus(Some("editor"), 10 * MIN);
    for _ in 0..40 {
        recording
            .focus(Some("chat"), S / 2)
            .focus(Some("terminal"), S / 2);
    }
    let end = recording.focus(Some("editor"), 10 * MIN).end();
    let m = metrics(&queries(dir.path(), false), 0, end);
    assert_eq!(m.longest_stretch.as_ref().unwrap().elapsed_ms, 10 * MIN);
    let s = &m.stretches;
    assert_eq!(
        s.long.elapsed_ms + s.medium.elapsed_ms + s.short.elapsed_ms,
        m.active_ms
    );
}

#[test]
fn idle_gaps_pauses_and_timing_boundaries_end_runs_but_brief_no_focus_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut recording = Recording::new(dir.path(), 0);
    recording
        .focus(Some("editor"), 10 * MIN)
        .idle(10 * MIN)
        .focus(Some("chat"), 5 * MIN)
        .pause(5 * MIN)
        .focus(Some("editor"), 10 * MIN)
        .discontinuity()
        .focus(Some("terminal"), 10 * MIN)
        .focus(None, 20 * S)
        .focus(Some("terminal"), 5 * MIN)
        .focus(None, 20 * S)
        .focus(Some("editor"), 5 * MIN);
    let end = recording.end();
    let q = queries(dir.path(), true);
    let m = metrics(&q, 0, end);
    assert_eq!(
        m.active_ms,
        45 * MIN,
        "idle, pause and no focus are not active focus"
    );
    assert_eq!(
        m.application_switches, 1,
        "only terminal → (no focus) → editor is a switch"
    );
    assert_eq!(m.category_switches, Some(0));
    let longest = m.longest_stretch.as_ref().unwrap();
    assert_eq!(
        (longest.range.start_ms, longest.range.end_ms),
        (40 * MIN, end),
        "brief absence of focus is bridged but not counted"
    );
    assert_eq!(longest.elapsed_ms, 20 * MIN);
    assert_eq!(longest.interruptions, 2);
    assert_eq!(
        stretches(&m),
        [(0, 0), (3, 40 * MIN), (1, 5 * MIN)],
        "editor 10 | idle | chat 5 | pause | editor 10 | boundary | code 20"
    );

    // Hourly buckets follow local clock hours; switches land where they occur.
    assert_eq!(m.hours.len(), 2);
    assert_eq!(
        m.hours[0].range,
        Range {
            start_ms: 0,
            end_ms: 60 * MIN
        }
    );
    assert_eq!(m.hours[0].active_ms, 45 * MIN - (end - 60 * MIN));
    assert_eq!(m.hours[0].application_switches, 1);
    assert_eq!(m.hours[1].application_switches, 0);
    assert_eq!(m.hours[1].range.end_ms, end, "clipped to the range");
    assert_eq!(
        m.hours.iter().map(|hour| hour.active_ms).sum::<i64>(),
        m.active_ms
    );
}

#[test]
fn activity_days_split_metrics_like_separate_overviews() {
    let zone: chrono_tz::Tz = "Europe/Oslo".parse().unwrap();
    let at = |day: u32, hour: u32, minute: u32| {
        zone.with_ymd_and_hms(2026, 10, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    };
    let dir = tempfile::tempdir().unwrap();
    let end = Recording::new(dir.path(), at(5, 3, 30))
        .focus(Some("editor"), 40 * MIN)
        .focus(Some("chat"), 10 * MIN)
        .end();
    assert_eq!(end, at(5, 4, 20));
    let q = queries(dir.path(), true);
    let days = q
        .focus_metrics(
            Browser,
            Selection::Range(Range {
                start_ms: at(4, 4, 0),
                end_ms: at(6, 4, 0),
            }),
        )
        .unwrap()
        .days;
    assert_eq!(
        days.iter().map(|day| day.date.as_str()).collect::<Vec<_>>(),
        ["2026-10-04", "2026-10-05"]
    );
    for day in &days {
        let overview = metrics(&q, day.range.start_ms, day.range.end_ms);
        assert_eq!(
            serde_json::to_value(&day.metrics).unwrap(),
            serde_json::to_value(&overview).unwrap(),
            "{}",
            day.date
        );
    }
    let (first, second) = (&days[0].metrics, &days[1].metrics);
    assert_eq!(days[0].totals.focus_ms, 30 * MIN);
    assert_eq!(
        first.longest_stretch.as_ref().unwrap().range.end_ms,
        at(5, 4, 0)
    );
    assert_eq!(first.application_switches, 0);
    assert_eq!(second.application_switches, 1);
    assert_eq!(
        second.longest_stretch.as_ref().unwrap().elapsed_ms,
        10 * MIN
    );
    assert_eq!(second.hours[0].range.start_ms, at(5, 4, 0));

    // A single date resolves to its activity day; the whole range does not
    // split the stretch at 04:00.
    let single = q
        .focus_metrics(Mcp, Selection::ActivityDate("2026-10-05".parse().unwrap()))
        .unwrap();
    assert_eq!(single.days.len(), 1);
    let whole = metrics(&q, at(4, 4, 0), at(6, 4, 0));
    assert_eq!(whole.longest_stretch.as_ref().unwrap().elapsed_ms, 40 * MIN);
    assert_eq!(whole.application_switches, 1);
}

#[test]
fn live_updates_carry_current_metrics() {
    let dir = tempfile::tempdir().unwrap();
    let mut recording = Recording::new(dir.path(), 0);
    recording.focus(Some("editor"), 10 * MIN);
    recording.end();
    let q = queries(dir.path(), true);
    let range = Range {
        start_ms: 0,
        end_ms: HOUR_MS,
    };
    let first = q
        .overview(Browser, Selection::Range(range), PageRequest::default())
        .unwrap();
    assert_eq!(first.metrics.application_switches, 0);
    recording.focus(Some("chat"), 5 * MIN);
    recording.end();
    let update = q
        .overview_updates(
            Browser,
            Selection::Range(range),
            PageRequest::default(),
            None,
            &first.revision,
        )
        .unwrap();
    let metrics = update.metrics.unwrap();
    assert_eq!(metrics.application_switches, 1);
    assert_eq!(metrics.active_ms, 15 * MIN);
}
const HOUR_MS: i64 = 60 * MIN;

#[test]
fn days_carry_focus_by_category_and_application_and_their_evidence() {
    let zone: chrono_tz::Tz = "Europe/Oslo".parse().unwrap();
    let at = |day: u32, hour: u32, minute: u32| {
        zone.with_ymd_and_hms(2026, 10, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    };
    let dir = tempfile::tempdir().unwrap();
    let mut recording = Recording::new(dir.path(), at(5, 9, 0));
    recording
        .focus(Some("editor"), 30 * MIN)
        .idle(10 * MIN)
        .focus(Some("browser"), 20 * MIN)
        .pause(15 * MIN)
        .focus(Some("chat"), 5 * MIN)
        .focus(None, 5 * MIN);
    let end = recording.end();
    assert_eq!(end, at(5, 10, 25));
    let q = queries(dir.path(), true);
    let days = q
        .focus_metrics(
            Mcp,
            Selection::ActivityDates {
                first: "2026-10-04".parse().unwrap(),
                last: "2026-10-05".parse().unwrap(),
            },
        )
        .unwrap()
        .days;
    assert_eq!(days.len(), 2);
    let (empty, day) = (&days[0], &days[1]);

    // A day before any evidence has none, rather than zero activity.
    assert!(empty.categories.is_empty() && empty.applications.is_empty());
    assert_eq!(empty.evidence.observed_ms, 0);
    assert_eq!(empty.evidence.unknown_ms, 24 * 60 * MIN);
    assert_eq!(empty.evidence.intervals.len(), 1);

    let categories: Vec<_> = day
        .categories
        .iter()
        .map(|row| (row.category.as_deref(), row.focus_ms, row.active_ms))
        .collect();
    assert_eq!(
        categories,
        [
            (Some("Code"), 40 * MIN, 30 * MIN),
            (Some("Chat"), 5 * MIN, 5 * MIN),
            (None, 20 * MIN, 20 * MIN),
        ],
        "configured order, then uncategorized; idle focus is not active"
    );
    let applications: Vec<_> = day
        .applications
        .iter()
        .map(|row| (row.application.as_str(), row.focus_ms))
        .collect();
    assert_eq!(
        applications,
        [
            ("editor", 40 * MIN),
            ("browser", 20 * MIN),
            ("chat", 5 * MIN)
        ]
    );
    assert_eq!(
        day.categories.iter().map(|row| row.active_ms).sum::<i64>(),
        day.metrics.active_ms
    );
    assert_eq!(
        day.categories.iter().map(|row| row.focus_ms).sum::<i64>(),
        day.totals.focus_ms
    );

    let evidence = &day.evidence;
    assert_eq!(evidence.observed_ms, 70 * MIN, "focus, idle and no focus");
    assert_eq!(evidence.gap_ms, 15 * MIN);
    assert_eq!(
        evidence.observed_ms + evidence.gap_ms + evidence.unknown_ms,
        day.range.end_ms - day.range.start_ms,
        "a past day is fully accounted for"
    );
    let intervals: Vec<_> = evidence
        .intervals
        .iter()
        .map(|interval| {
            (
                interval.kind,
                interval.reason.as_deref(),
                interval.range.start_ms,
                interval.range.end_ms,
            )
        })
        .collect();
    assert_eq!(
        intervals,
        [
            (EvidenceKind::Unknown, None, at(5, 4, 0), at(5, 9, 0)),
            (EvidenceKind::Observed, None, at(5, 9, 0), at(5, 10, 0)),
            (
                EvidenceKind::Gap,
                Some("pause"),
                at(5, 10, 0),
                at(5, 10, 15)
            ),
            (EvidenceKind::Observed, None, at(5, 10, 15), end),
            (EvidenceKind::Unknown, None, end, at(6, 4, 0)),
        ],
        "adjacent coverage of one kind merges"
    );

    // Without categories, days still carry applications.
    let plain = queries(dir.path(), false)
        .focus_metrics(Mcp, Selection::ActivityDate("2026-10-05".parse().unwrap()))
        .unwrap();
    assert!(plain.days[0].categories.is_empty());
    assert_eq!(plain.days[0].applications.len(), 3);
}

#[test]
fn activity_date_spans_cover_up_to_31_days_including_daylight_saving() {
    let dir = tempfile::tempdir().unwrap();
    Recording::new(dir.path(), 0)
        .focus(Some("editor"), MIN)
        .end();
    let q = queries(dir.path(), true);
    let dates = |first: &str, last: &str| Selection::ActivityDates {
        first: first.parse().unwrap(),
        last: last.parse().unwrap(),
    };
    // October 2026 in Oslo has 31 days and a 25-hour day.
    let october = q.resolve(dates("2026-10-01", "2026-10-31")).unwrap().range;
    assert_eq!(october.end_ms - october.start_ms, (31 * 24 + 1) * 60 * MIN);
    let days = q
        .focus_metrics(Browser, dates("2026-10-01", "2026-10-31"))
        .unwrap()
        .days;
    assert_eq!(days.len(), 31);
    assert_eq!(days[30].date, "2026-10-31");
    let week = q.resolve(dates("2026-10-05", "2026-10-11")).unwrap().range;
    assert_eq!(
        week.start_ms,
        q.resolve(Selection::ActivityDate("2026-10-05".parse().unwrap()))
            .unwrap()
            .range
            .start_ms
    );
    for (first, last) in [("2026-10-01", "2026-11-01"), ("2026-10-05", "2026-10-04")] {
        let error = q.resolve(dates(first, last)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<QueryError>(),
            Some(&QueryError::InvalidRequest),
            "{first}..{last}"
        );
    }
}

#[test]
fn evidence_does_not_count_the_future_as_missing() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let dir = tempfile::tempdir().unwrap();
    Recording::new(dir.path(), now - 2 * HOUR_MS)
        .focus(Some("editor"), HOUR_MS)
        .end();
    let q = queries(dir.path(), true);
    let range = Range {
        start_ms: now - 3 * HOUR_MS,
        end_ms: now + 3 * HOUR_MS,
    };
    let days = q.focus_metrics(Mcp, Selection::Range(range)).unwrap().days;
    let evidence =
        |field: fn(&DayEvidence) -> i64| days.iter().map(|day| field(&day.evidence)).sum::<i64>();
    assert_eq!(evidence(|e| e.observed_ms), HOUR_MS);
    let unknown = evidence(|e| e.unknown_ms);
    let checked = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        (2 * HOUR_MS..=2 * HOUR_MS + checked - now).contains(&unknown),
        "unknown before and after the hour, up to the query time: {unknown}"
    );
    assert!(
        days.iter()
            .flat_map(|day| &day.evidence.intervals)
            .all(|interval| interval.range.end_ms <= checked)
    );
}

#[test]
fn sessions_list_every_stretch_in_time_order_with_its_own_windows() {
    let dir = tempfile::tempdir().unwrap();
    let end = Recording::new(dir.path(), 0)
        .focus(Some("editor"), 10 * MIN)
        .focus(Some("terminal"), 10 * MIN)
        .focus(Some("chat"), 30 * S)
        .focus(Some("editor"), 9 * MIN + 30 * S)
        .idle(10 * MIN)
        .focus(Some("chat"), 2 * MIN)
        .end();
    let q = queries(dir.path(), true);
    let range = Range {
        start_ms: 0,
        end_ms: end,
    };
    let list = q.sessions(Browser, Selection::Range(range)).unwrap();
    let sessions: Vec<_> = list
        .sessions
        .iter()
        .map(|s| {
            (
                s.stretch.subject.clone(),
                s.stretch.range.start_ms,
                s.stretch.elapsed_ms,
            )
        })
        .collect();
    assert_eq!(
        sessions,
        [
            (category("Code"), 0, 30 * MIN),
            (category("Chat"), 40 * MIN, 2 * MIN)
        ],
        "the glance is bridged and idle ends the first stretch"
    );
    // The bridged chat glance is not one of the stretch's own windows.
    let windows: Vec<_> = list.sessions[0]
        .windows
        .iter()
        .map(|w| (w.application.as_str(), w.active_ms))
        .collect();
    assert_eq!(
        windows,
        [("editor", 19 * MIN + 30 * S), ("terminal", 10 * MIN)]
    );
    assert_eq!(list.sessions[0].windows[0].title, "editor window");
    // Stretches match those the overview counts.
    let m = metrics(&q, 0, end);
    assert_eq!(
        m.stretches.long.count + m.stretches.medium.count + m.stretches.short.count,
        list.sessions.len() as i64
    );
    let week = Range {
        start_ms: 0,
        end_ms: 8 * 24 * 60 * MIN + 2 * 60 * MIN,
    };
    assert!(q.sessions(Browser, Selection::Range(week)).is_err());
}
