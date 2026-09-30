use super::*;

#[derive(Debug, Serialize)]
pub struct ActivityBounds {
    pub resolved: ResolvedRange,
    pub activity: Option<Range>,
}

impl Queries {
    pub fn activity_bounds(&self, client: Client, selection: Selection) -> Result<ActivityBounds> {
        let resolved = self.resolve(selection)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor("activity_bounds".into(), &PageRequest::default())?;
        let mut activity: Option<Range> = None;
        let mut include = |range: Range| {
            if let Some(bounds) = &mut activity {
                bounds.start_ms = bounds.start_ms.min(range.start_ms);
                bounds.end_ms = bounds.end_ms.max(range.end_ms);
            } else {
                activity = Some(range);
            }
        };
        self.coverage(&cursor, resolved.range, |row| {
            if matches!(row.focus, FocusCoverage::Focus { .. }) {
                include(row.range);
            }
            Ok(())
        })?;
        self.image_coverage(&cursor, resolved.range, None, |row| {
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
