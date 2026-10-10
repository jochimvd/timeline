//! Revision-pinned row deltas over bounded overview snapshots. Unchanged polls
//! read only append watermarks; changed snapshots reuse the overview cache.
use super::*;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
pub struct OverviewChange {
    pub remove: bool,
    pub row: Breakdown,
}
#[derive(Debug, Serialize)]
pub struct OverviewUpdates {
    pub revision: String,
    pub reset: bool,
    pub totals: Option<Totals>,
    pub metrics: Option<FocusMetrics>,
    pub changes: Page<OverviewChange>,
}
impl Queries {
    pub fn overview_updates(
        &self,
        client: Client,
        selection: Selection,
        page: PageRequest,
        points: Option<usize>,
        since: &str,
    ) -> Result<OverviewUpdates> {
        let resolved = self.resolve(selection)?;
        if since.len() > 4096 {
            bail!(QueryError::InvalidRequest);
        }
        let old: Cursor = serde_json::from_slice(&decode_cursor(since)?)
            .map_err(|_| QueryError::InvalidRequest)?;
        let scope = overview::overview_scope(&resolved, points);
        let query = format!("{scope}:{}", self.analysis_version());
        // Changed grouping or title rules redo the overview instead of diffing.
        let policy_changed = old.query != query
            && old
                .query
                .strip_prefix(&scope)
                .is_some_and(|version| version.starts_with(':'));
        if (old.query != query && !policy_changed) || old.offset != 0 {
            bail!(QueryError::InvalidRequest);
        }
        let cursor = {
            let _permit = self.admit(client)?;
            let _snapshot = self.history.connection.unchecked_transaction()?;
            self.cursor(format!("updates:{query}:{since}"), &page)?
        };
        let current = Cursor {
            query,
            offset: 0,
            ..cursor.clone()
        };
        let revision = encode_cursor(&current)?;
        let empty = |reset| OverviewUpdates {
            revision: revision.clone(),
            reset,
            totals: None,
            metrics: None,
            changes: Page {
                items: vec![],
                next_cursor: None,
                truncated: false,
            },
        };
        if policy_changed || old.epoch != current.epoch {
            return Ok(empty(true));
        }
        if old.observations > current.observations || old.screenshots > current.screenshots {
            bail!(QueryError::InvalidRequest);
        }
        if old.observations == current.observations && old.screenshots == current.screenshots {
            return Ok(empty(false));
        }
        let Some(previous) = cached_overview(&self.overview_key(&old)?) else {
            return Ok(empty(true));
        };
        let new_key = self.overview_key(&current)?;
        if cached_overview(&new_key).is_none() {
            // Pin the exact append watermarks for every page of this update.
            self.overview_display(
                client,
                Selection::Range(resolved.range),
                PageRequest {
                    cursor: Some(revision.clone()),
                    limit: Some(1),
                },
                points,
            )?;
        }
        let Some(next) = cached_overview(&new_key) else {
            return Ok(empty(true));
        };
        let previous_rows: BTreeSet<&[u8]> = previous.rows.iter().map(|row| row.as_ref()).collect();
        let next_rows: BTreeSet<&[u8]> = next.rows.iter().map(|row| row.as_ref()).collect();
        let mut output = Pager::new(cursor, &page);
        for row in &previous.rows {
            self.check_cancelled()?;
            if !next_rows.contains(row.as_ref()) {
                output.push(OverviewChange {
                    remove: true,
                    row: serde_json::from_slice(row)?,
                })?;
            }
        }
        for row in &next.rows {
            self.check_cancelled()?;
            if !previous_rows.contains(row.as_ref()) {
                output.push(OverviewChange {
                    remove: false,
                    row: serde_json::from_slice(row)?,
                })?;
            }
        }
        Ok(OverviewUpdates {
            revision,
            reset: false,
            totals: Some(next.totals.clone()),
            metrics: Some(next.metrics.clone()),
            changes: output.finish()?,
        })
    }
}
