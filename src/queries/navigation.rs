//! Selected-instant evidence and timeline navigation.
use super::*;

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
        Ok(CursorEvidence {
            selected_ms,
            focus,
            screenshots,
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
}
