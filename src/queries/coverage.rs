//! Focus coverage between consecutive observations, and activity blocks over it.
use super::*;

/// Streams coverage for one range. Unscanned history between chunks is never
/// paired; the recording boundary before a chunk is looked up only if used.
struct Focus<'a> {
    queries: &'a Queries,
    range: Range,
    previous: Option<ScanObservation>,
    furthest: i64,
    chunk: i64,
    boundary: Option<i64>,
    /// Checkpoints share parsed evidence; reuse its cleaned, categorized focus.
    classified: Option<(Arc<Evidence>, FocusCoverage)>,
}
impl<'a> Focus<'a> {
    fn new(queries: &'a Queries, range: Range) -> Self {
        Self {
            queries,
            range,
            previous: None,
            furthest: range.start_ms,
            chunk: 0,
            boundary: None,
            classified: None,
        }
    }
    fn classify(&mut self, evidence: &Arc<Evidence>) -> FocusCoverage {
        if let Some((last, focus)) = &self.classified
            && Arc::ptr_eq(last, evidence)
        {
            return focus.clone();
        }
        let focus = classify(evidence, &self.queries.settings);
        self.classified = Some((evidence.clone(), focus.clone()));
        focus
    }
    fn recording_boundary(&mut self) -> Result<i64> {
        if let Some(boundary) = self.boundary {
            return Ok(boundary);
        }
        let boundary = self.queries.boundary_before(self.chunk)?;
        self.boundary = Some(boundary);
        Ok(boundary)
    }
    fn push(
        &mut self,
        next: ScanObservation,
        consecutive: bool,
        visit: &mut impl FnMut(Coverage) -> Result<()>,
    ) -> Result<()> {
        if !consecutive {
            self.previous = None;
            self.chunk = next.id;
            self.boundary = None;
        }
        if let Some(old) = self.previous.take() {
            if is_boundary(&old.evidence) {
                self.boundary = Some(old.id);
            }
            if let Some(clipped) = self.range.clip(old.unix_ms, next.unix_ms) {
                if clipped.start_ms > self.furthest {
                    visit(unknown(Range {
                        start_ms: self.furthest,
                        end_ms: clipped.start_ms,
                    }))?;
                }
                self.furthest = self.furthest.max(clipped.end_ms);
                let recording_boundary = self.recording_boundary()?;
                let focus = if continuous(&old, &next) {
                    self.classify(&old.evidence)
                } else {
                    discontinuity(&next)
                };
                visit(interval(&old, &next, clipped, recording_boundary, focus))?;
            }
        }
        self.previous = Some(next);
        Ok(())
    }
    /// `latest` is the latest retained observation, if the plan scanned it;
    /// `screenshots` the latest screenshot row the cursor pins.
    fn finish(
        mut self,
        latest: Option<i64>,
        screenshots: i64,
        visit: &mut impl FnMut(Coverage) -> Result<()>,
    ) -> Result<()> {
        if self.furthest >= self.range.end_ms {
            return Ok(());
        }
        let tail = Range {
            start_ms: self.furthest,
            end_ms: self.range.end_ms,
        };
        // A known pause/lock remains a gap; last known focus never extends
        // past its durable endpoint into recorder downtime.
        let previous = self
            .previous
            .take()
            .filter(|o| Some(o.id) == latest && o.unix_ms <= tail.start_ms);
        let Some(o) = previous else {
            return visit(unknown(tail));
        };
        let focus = self.classify(&o.evidence);
        if let FocusCoverage::Gap { .. } = focus {
            return visit(Coverage {
                range: tail,
                segment: Some(o.segment),
                observation: Some(o.id),
                recording_boundary: self.recording_boundary()?,
                elapsed_ms: None,
                focus,
            });
        }
        // A capture is also a durable endpoint: the recorder commits focus
        // changes before it takes one, so focus held through the latest capture
        // of its timing segment. Only within the loss bound, as checkpoints
        // would otherwise have followed.
        let mut start = tail.start_ms;
        if matches!(
            focus,
            FocusCoverage::Focus { .. } | FocusCoverage::NoFocus { .. }
        ) && let Some((unix_ms, monotonic_ms)) = self.queries.capture_after(&o, screenshots)?
        {
            let through = ScanObservation {
                unix_ms: unix_ms.saturating_add(1),
                monotonic_ms: monotonic_ms.saturating_add(1),
                evidence: o.evidence.clone(),
                ..o
            };
            let end = through.unix_ms.min(tail.end_ms);
            if end > start {
                let clipped = Range {
                    start_ms: start,
                    end_ms: end,
                };
                let recording_boundary = self.recording_boundary()?;
                visit(interval(&o, &through, clipped, recording_boundary, focus))?;
                start = end;
            }
        }
        if start < tail.end_ms {
            visit(unknown(Range {
                start_ms: start,
                end_ms: tail.end_ms,
            }))?;
        }
        Ok(())
    }
}

fn continuous(old: &ScanObservation, next: &ScanObservation) -> bool {
    old.segment == next.segment && next.monotonic_ms >= old.monotonic_ms
}
/// Focus between discontinuous observations: the reason for the next one, if known.
fn discontinuity(next: &ScanObservation) -> FocusCoverage {
    match next.evidence.as_ref() {
        Evidence::Boundary { reason, .. } => FocusCoverage::Gap {
            reason: reason.clone(),
        },
        _ => FocusCoverage::Unknown,
    }
}
/// Evidence between two consecutive observations, clipped to the range.
fn interval(
    old: &ScanObservation,
    next: &ScanObservation,
    clipped: Range,
    recording_boundary: i64,
    focus: FocusCoverage,
) -> Coverage {
    let elapsed_ms = (continuous(old, next)
        && matches!(
            focus,
            FocusCoverage::Focus { .. } | FocusCoverage::NoFocus { .. }
        ))
    .then(|| clipped_elapsed(old, next, clipped));
    Coverage {
        range: clipped,
        segment: Some(old.segment),
        observation: Some(old.id),
        recording_boundary,
        elapsed_ms,
        focus,
    }
}

impl Queries {
    /// One pass over planned observations: focus coverage of `range` and, when
    /// requested, the per-segment context image coverage needs.
    pub(super) fn coverage(
        &self,
        cursor: &Cursor,
        range: Range,
        plan: &Plan,
        mut context: Option<&mut ImageContext>,
        mut visit: impl FnMut(Coverage) -> Result<()>,
    ) -> Result<()> {
        let mut focus = Focus::new(self, range);
        self.scan(cursor, plan, |observation, consecutive| {
            if let Some(context) = &mut context {
                context.observe(&observation)?;
            }
            focus.push(observation, consecutive, &mut visit)
        })?;
        focus.finish(plan.last, cursor.screenshots, &mut visit)
    }
    /// The latest successful capture after `observation` in its timing segment
    /// and within the loss bound, as wall and monotonic time.
    fn capture_after(
        &self,
        observation: &ScanObservation,
        screenshots: i64,
    ) -> Result<Option<(i64, i64)>> {
        Ok(self
            .history
            .connection
            .prepare_cached(
                "SELECT unix_ms,monotonic_ms FROM screenshots WHERE unix_ms>?1 AND id<=?2
                    AND segment=?3 AND monotonic_ms BETWEEN ?4 AND ?4+?5 AND image IS NOT NULL
                    ORDER BY unix_ms DESC LIMIT 1",
            )?
            .query_row(
                params![
                    observation.unix_ms,
                    screenshots,
                    observation.segment,
                    observation.monotonic_ms,
                    crate::config::LOSS_BOUND_MS
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn blocks(&self, client: Client, range: Range, page: PageRequest) -> Result<Page<Block>> {
        range.validate(48 * HOUR)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.analysis_cursor(&format!("blocks:{range:?}"), &page)?;
        let mut output = Pager::new(cursor.clone(), &page);
        // Blocks never cross segments. Coverage from each segment's start keeps
        // a clipped block's original ID.
        let plan = self.plan(&cursor, range, Scope::Blocks)?;
        self.planned_blocks(&cursor, range, &plan, |block| output.push(block))?;
        output.finish()
    }
    pub(super) fn planned_blocks(
        &self,
        cursor: &Cursor,
        range: Range,
        plan: &Plan,
        mut visit: impl FnMut(Block) -> Result<()>,
    ) -> Result<()> {
        let mut blocks = Blocks {
            grouping: self.grouping.as_ref(),
            version: self.block_version(),
            range,
            current: None,
        };
        let full = Range {
            start_ms: i64::MIN,
            end_ms: i64::MAX,
        };
        self.coverage(cursor, full, plan, None, |coverage| {
            blocks.push(coverage, &mut visit)
        })?;
        blocks.finish(&mut visit)
    }
}

struct Blocks<'a> {
    grouping: &'a dyn Grouping,
    version: String,
    range: Range,
    current: Option<Block>,
}
impl Blocks<'_> {
    fn push(
        &mut self,
        coverage: Coverage,
        output: &mut impl FnMut(Block) -> Result<()>,
    ) -> Result<()> {
        let FocusCoverage::Focus {
            window,
            idle,
            category,
        } = &coverage.focus
        else {
            return self.finish(output);
        };
        let key = self.grouping.key(window);
        let segment = coverage.segment.unwrap();
        let id = coverage.observation.unwrap();
        let compatible = self.current.as_ref().is_some_and(|b| {
            b.segment == segment
                && b.recording_boundary == coverage.recording_boundary
                && b.group == key
                && b.category == *category
                && b.range.end_ms == coverage.range.start_ms
        });
        if !compatible {
            self.finish(output)?;
            self.current = Some(Block {
                id: format!("{}:{segment}:{id}", self.version),
                recording_boundary: coverage.recording_boundary,
                first_observation: id,
                last_observation: id,
                segment,
                range: coverage.range,
                elapsed_ms: 0,
                idle_ms: 0,
                group: key,
                category: category.clone(),
            });
        }
        let block = self.current.as_mut().unwrap();
        block.range.end_ms = coverage.range.end_ms;
        block.last_observation = id;
        if let Some(clip) = self
            .range
            .clip(coverage.range.start_ms, coverage.range.end_ms)
        {
            let duration = proportional(coverage.elapsed_ms.unwrap_or(0), coverage.range, clip);
            block.elapsed_ms += duration;
            if *idle {
                block.idle_ms += duration;
            }
        }
        Ok(())
    }
    fn finish(&mut self, output: &mut impl FnMut(Block) -> Result<()>) -> Result<()> {
        if let Some(mut block) = self.current.take()
            && let Some(clip) = self.range.clip(block.range.start_ms, block.range.end_ms)
        {
            block.range = clip;
            output(block)?;
        }
        Ok(())
    }
}

pub(super) fn is_boundary(evidence: &Evidence) -> bool {
    !matches!(
        evidence,
        Evidence::State {
            kind: StateChange::Focus | StateChange::Idle | StateChange::Checkpoint,
            ..
        }
    )
}
fn classify(evidence: &Evidence, settings: &Settings) -> FocusCoverage {
    match evidence {
        Evidence::State {
            state:
                FocusState {
                    manual_paused: true,
                    ..
                },
            ..
        } => FocusCoverage::Gap {
            reason: "pause".into(),
        },
        Evidence::State {
            kind: StateChange::Disconnect,
            ..
        } => FocusCoverage::Gap {
            reason: "disconnect".into(),
        },
        Evidence::State {
            state: FocusState { locked: true, .. },
            ..
        } => FocusCoverage::Gap {
            reason: "lock".into(),
        },
        Evidence::State {
            state:
                FocusState {
                    connected: true,
                    focus_known: true,
                    focus,
                    idle,
                    ..
                },
            ..
        } => match focus {
            Some(window) => {
                let window = settings.titles.clean(window);
                FocusCoverage::Focus {
                    category: settings
                        .categories
                        .classify(&window)
                        .map(|category| category.name.clone()),
                    window,
                    idle: *idle,
                }
            }
            None => FocusCoverage::NoFocus { idle: *idle },
        },
        Evidence::Boundary { reason, .. } => FocusCoverage::Gap {
            reason: reason.clone(),
        },
        _ => FocusCoverage::Unknown,
    }
}
fn unknown(range: Range) -> Coverage {
    Coverage {
        range,
        segment: None,
        observation: None,
        recording_boundary: 0,
        elapsed_ms: None,
        focus: FocusCoverage::Unknown,
    }
}
pub(super) fn proportional(elapsed: i64, full: Range, clipped: Range) -> i64 {
    // Difference of cumulative elapsed fractions makes adjacent clips additive.
    let offset = |time: i64| {
        (i128::from(time) - i128::from(full.start_ms)) * i128::from(elapsed)
            / (i128::from(full.end_ms) - i128::from(full.start_ms))
    };
    (offset(clipped.end_ms) - offset(clipped.start_ms)) as i64
}
fn clipped_elapsed(old: &ScanObservation, next: &ScanObservation, clipped: Range) -> i64 {
    proportional(
        next.monotonic_ms.saturating_sub(old.monotonic_ms),
        Range {
            start_ms: old.unix_ms,
            end_ms: next.unix_ms,
        },
        clipped,
    )
}
