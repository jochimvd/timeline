//! Selected-instant evidence and timeline navigation.
use super::*;

/// Tiles that may differ before a screenshot counts as changed: enough for a
/// clock or CPU readout, too few for an edited paragraph or another window.
const STEP_CHANGED_TILES: usize = 3;
/// How far one step looks, in time and in captures.
const STEP_SPAN_MS: i64 = 3 * HOUR;
const STEP_ROWS: usize = 5_000;
/// A longer pause between captures ends a step: time passed unseen.
const STEP_GAP_MS: i64 = 60_000;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ScreenshotStep {
    /// Capture time to select, or null when there is no screenshot that way.
    pub unix_ms: Option<i64>,
    /// Captures passed over on the monitor with the most of them.
    pub skipped: usize,
    /// From the selection to the last capture passed over; a pause after it is not included.
    pub skipped_ms: i64,
}

/// One retained capture's identity: its image name and decoded tile map.
struct Frame {
    unix_ms: i64,
    image: String,
    size: (u32, u32),
    map: Vec<u8>,
}
impl Frame {
    fn changed_from(&self, shown: &Frame) -> Result<bool> {
        if self.image == shown.image {
            return Ok(false);
        }
        if self.size != shown.size {
            return Ok(true);
        }
        let (a, b) = (
            crate::tile_map::decode(&self.map)?,
            crate::tile_map::decode(&shown.map)?,
        );
        Ok(a.len() != b.len()
            || a.iter().zip(&b).filter(|(a, b)| a != b).count() > STEP_CHANGED_TILES)
    }
}

/// What a step must not pass silently: the focused window, or its absence.
#[derive(PartialEq)]
enum FocusKey {
    Window(String, String),
    None,
    Gap,
}
impl From<&FocusCoverage> for FocusKey {
    fn from(focus: &FocusCoverage) -> Self {
        match focus {
            FocusCoverage::Focus { window, .. } => {
                Self::Window(window.application.clone(), window.title.clone())
            }
            FocusCoverage::NoFocus { .. } => Self::None,
            FocusCoverage::Gap { .. } | FocusCoverage::Unknown => Self::Gap,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ActivityBounds {
    pub resolved: ResolvedRange,
    pub activity: Option<Range>,
}

impl Queries {
    pub fn cursor_evidence(
        &self,
        client: Client,
        selected_ms: i64,
        monitor: Option<&str>,
    ) -> Result<CursorEvidence> {
        let range = Range {
            start_ms: selected_ms,
            end_ms: selected_ms
                .checked_add(1)
                .ok_or(QueryError::InvalidRequest)?,
        };
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor("selection".into(), &PageRequest::default())?;
        let plan = self.plan(&cursor, range, Scope::Evidence)?;
        let mut context = ImageContext::default();
        let mut focus = Vec::new();
        let mut response_bytes = 0;
        self.coverage(&cursor, range, &plan, Some(&mut context), |coverage| {
            response_bytes += serde_json::to_vec(&coverage)?.len();
            if focus.len() >= 500 || response_bytes > PAGE_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            focus.push(coverage);
            Ok(())
        })?;
        let mut screenshots = Vec::new();
        self.image_coverage(&cursor, range, monitor, &plan, &context, |coverage| {
            if screenshots.len() >= 500 {
                bail!(QueryError::LimitExceeded);
            }
            let screenshot = self
                .screenshot(coverage.screenshot)?
                .ok_or(QueryError::Unavailable)?;
            let stale = selected_ms.saturating_sub(screenshot.sample.unix_ms)
                >= crate::config::CAPTURE_INTERVAL_MS;
            let selected = SelectedScreenshot { screenshot, stale };
            response_bytes += serde_json::to_vec(&selected)?.len();
            if response_bytes > PAGE_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            screenshots.push(selected);
            Ok(())
        })?;
        // Once retention has removed any screenshot, time before the oldest
        // retained capture has lost its screenshots.
        let (expired, oldest): (bool, Option<i64>) = self.history.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM state WHERE key='screenshots_expired_through'),
                (SELECT min(unix_ms) FROM screenshots)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(CursorEvidence {
            selected_ms,
            focus,
            screenshots,
            screenshots_expired: expired && oldest.is_none_or(|oldest| selected_ms < oldest),
        })
    }

    pub fn activity_bounds(&self, client: Client, selection: Selection) -> Result<ActivityBounds> {
        let resolved = self.resolve(selection)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor("activity_bounds".into(), &PageRequest::default())?;
        let plan = self.plan(&cursor, resolved.range, Scope::Evidence)?;
        let mut context = ImageContext::default();
        let mut activity: Option<Range> = None;
        let mut include = |range: Range| {
            if let Some(bounds) = &mut activity {
                bounds.start_ms = bounds.start_ms.min(range.start_ms);
                bounds.end_ms = bounds.end_ms.max(range.end_ms);
            } else {
                activity = Some(range);
            }
        };
        self.coverage(&cursor, resolved.range, &plan, Some(&mut context), |row| {
            if matches!(row.focus, FocusCoverage::Focus { .. }) {
                include(row.range);
            }
            Ok(())
        })?;
        self.image_coverage(&cursor, resolved.range, None, &plan, &context, |row| {
            if row.unavailable.is_none() {
                include(row.range);
            }
            Ok(())
        })?;
        Ok(ActivityBounds { resolved, activity })
    }

    pub fn adjacent_screenshot(
        &self,
        client: Client,
        selected_ms: i64,
        direction: i32,
        monitor: Option<&str>,
    ) -> Result<Option<i64>> {
        let sql = match direction {
            -1 => {
                "SELECT max(unix_ms) FROM screenshots WHERE unix_ms < ?1 AND image IS NOT NULL AND (?2 IS NULL OR monitor=?2)"
            }
            1 => {
                "SELECT min(unix_ms) FROM screenshots WHERE unix_ms > ?1 AND image IS NOT NULL AND (?2 IS NULL OR monitor=?2)"
            }
            _ => bail!(QueryError::InvalidRequest),
        };
        let _permit = self.admit(client)?;
        self.check_cancelled()?;
        Ok(self
            .history
            .connection
            .query_row(sql, params![selected_ms, monitor], |row| row.get(0))?)
    }

    /// The next capture in `direction` that looks different from what is shown at
    /// `selected_ms`: more than a few tiles changed on some monitor, the focused
    /// window changed, or capture resumed after a pause. Without one within reach,
    /// the furthest capture looked at, so every step makes progress; with no
    /// capture within reach at all, the nearest one beyond it.
    pub fn screenshot_step(
        &self,
        client: Client,
        selected_ms: i64,
        direction: i32,
        monitor: Option<&str>,
    ) -> Result<ScreenshotStep> {
        let (start, end) = match direction {
            -1 => (selected_ms.saturating_sub(STEP_SPAN_MS), selected_ms),
            1 => (selected_ms, selected_ms.saturating_add(STEP_SPAN_MS)),
            _ => bail!(QueryError::InvalidRequest),
        };
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let frame = |r: &rusqlite::Row| -> rusqlite::Result<(String, Frame)> {
            Ok((
                r.get(0)?,
                Frame {
                    unix_ms: r.get(1)?,
                    image: r.get(2)?,
                    size: (r.get(3)?, r.get(4)?),
                    map: r.get(5)?,
                },
            ))
        };
        // What each monitor shows now: its latest capture at or before the selection.
        let shown: HashMap<String, Frame> = self
            .history
            .connection
            .prepare(
                "SELECT s.monitor, max(s.unix_ms), s.image, i.width, i.height, i.map
                 FROM screenshots s JOIN images i ON i.name=s.image
                 WHERE s.unix_ms<=?1 AND s.unix_ms>?1-?2 AND (?3 IS NULL OR s.monitor=?3)
                 GROUP BY s.monitor",
            )?
            .query_map(params![selected_ms, STEP_GAP_MS, monitor], frame)?
            .collect::<rusqlite::Result<_>>()?;
        let range = Range {
            start_ms: start,
            end_ms: end.saturating_add(1),
        };
        let cursor = self.cursor("screenshot_step".into(), &PageRequest::default())?;
        let plan = self.plan(&cursor, range, Scope::Evidence)?;
        let mut focus: Vec<(Range, FocusKey)> = Vec::new();
        self.coverage(&cursor, range, &plan, None, |row| {
            focus.push((row.range, FocusKey::from(&row.focus)));
            Ok(())
        })?;
        let focus_at = |ms: i64| {
            let index = focus.partition_point(|(range, _)| range.end_ms <= ms);
            focus
                .get(index)
                .filter(|(range, _)| range.start_ms <= ms)
                .map(|(_, key)| key)
        };
        let current = focus_at(selected_ms);
        let sql = if direction > 0 {
            "SELECT s.monitor, s.unix_ms, s.image, i.width, i.height, i.map
             FROM screenshots s JOIN images i ON i.name=s.image
             WHERE s.unix_ms>?1 AND s.unix_ms<=?2 AND (?3 IS NULL OR s.monitor=?3)
             ORDER BY s.unix_ms, s.id LIMIT ?4"
        } else {
            "SELECT s.monitor, s.unix_ms, s.image, i.width, i.height, i.map
             FROM screenshots s JOIN images i ON i.name=s.image
             WHERE s.unix_ms<?2 AND s.unix_ms>=?1 AND (?3 IS NULL OR s.monitor=?3)
             ORDER BY s.unix_ms DESC, s.id DESC LIMIT ?4"
        };
        let mut statement = self.history.connection.prepare(sql)?;
        let rows = statement.query_map(params![start, end, monitor, STEP_ROWS as i64], frame)?;
        let mut skipped: HashMap<String, usize> = HashMap::new();
        let mut last = selected_ms;
        let mut before_last = selected_ms;
        let mut furthest = None;
        for (index, row) in rows.enumerate() {
            if index % 256 == 0 {
                self.check_cancelled()?;
            }
            let (monitor, frame) = row?;
            let previous = shown.get(&monitor);
            // Going back, the frame on screen is itself in the way.
            if previous.is_some_and(|p| p.unix_ms == frame.unix_ms) {
                continue;
            }
            let paused = (frame.unix_ms - last).abs() > STEP_GAP_MS;
            let refocused = focus_at(frame.unix_ms) != current;
            let changed = match previous {
                Some(previous) => frame.changed_from(previous)?,
                None => true,
            };
            if paused || refocused || changed {
                return Ok(ScreenshotStep {
                    unix_ms: Some(frame.unix_ms),
                    skipped: skipped.into_values().max().unwrap_or(0),
                    skipped_ms: (last - selected_ms).abs(),
                });
            }
            before_last = last;
            last = frame.unix_ms;
            furthest = Some(frame.unix_ms);
            // Always against what is on screen, so small edits add up to a change.
            *skipped.entry(monitor).or_default() += 1;
        }
        drop(statement);
        if furthest.is_none() {
            // Nothing within reach: the next capture beyond, after a long pause.
            let beyond = if direction > 0 {
                "SELECT min(unix_ms) FROM screenshots WHERE unix_ms > ?1 AND image IS NOT NULL AND (?2 IS NULL OR monitor=?2)"
            } else {
                "SELECT max(unix_ms) FROM screenshots WHERE unix_ms < ?1 AND image IS NOT NULL AND (?2 IS NULL OR monitor=?2)"
            };
            let edge = if direction > 0 { end } else { start };
            return Ok(ScreenshotStep {
                unix_ms: self.history.connection.query_row(
                    beyond,
                    params![edge, monitor],
                    |row| row.get(0),
                )?,
                skipped: 0,
                skipped_ms: 0,
            });
        }
        Ok(ScreenshotStep {
            unix_ms: furthest,
            skipped: skipped
                .into_values()
                .max()
                .unwrap_or(0)
                .saturating_sub(usize::from(furthest.is_some())),
            skipped_ms: (before_last - selected_ms).abs(),
        })
    }
}
