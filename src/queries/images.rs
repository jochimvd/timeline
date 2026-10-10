//! Screenshot listings and per-monitor image coverage. Images never carry across
//! recording/monitor boundaries or past their segment's last observation.
use super::*;

const COLUMNS: &str =
    "id,unix_ms,monotonic_ms,segment,monitor,width,height,transform,image,unavailable";

/// Per-segment observation context, collected during the focus pass. Boundaries
/// apply even to older histories without per-monitor gap rows.
#[derive(Default)]
pub(super) struct ImageContext {
    endpoints: BTreeMap<i64, i64>,
    boundaries: BTreeMap<i64, Vec<i64>>,
    bytes: usize,
}
impl ImageContext {
    pub fn observe(&mut self, observation: &ScanObservation) -> Result<()> {
        let boundary = is_boundary(&observation.evidence);
        if !self.endpoints.contains_key(&observation.segment) {
            self.bytes += 128;
        }
        if boundary {
            self.bytes += 16;
        }
        if self.bytes > BREAKDOWN_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        self.endpoints
            .entry(observation.segment)
            .and_modify(|end| *end = (*end).max(observation.unix_ms))
            .or_insert(observation.unix_ms);
        if boundary {
            // Ids ascend, but wall times may step back slightly within a segment.
            let times = self.boundaries.entry(observation.segment).or_default();
            let index = times.partition_point(|time| *time <= observation.unix_ms);
            times.insert(index, observation.unix_ms);
        }
        Ok(())
    }
    /// End of the image interval starting at `sample`, before clipping.
    fn end(&self, image: &Screenshot, next: Option<&Screenshot>) -> i64 {
        let sample = &image.sample;
        let mut end = self
            .endpoints
            .get(&sample.segment)
            .copied()
            .unwrap_or(sample.unix_ms)
            .max(sample.unix_ms.saturating_add(1));
        if let Some(next) = next {
            // Invalidation is recorded in the outgoing segment, but its wall
            // timestamp may already be on the other side of a clock jump.
            // Only the previous durable endpoint certifies that interval.
            end = if matches!(
                next.unavailable.as_deref(),
                Some(
                    "clock_discontinuity"
                        | "monotonic_reset"
                        | "suspend"
                        | "observation_stall"
                        | "restart"
                )
            ) {
                end.min(next.sample.unix_ms)
            } else {
                next.sample.unix_ms
            };
        }
        if let Some(times) = self.boundaries.get(&sample.segment) {
            let index = times.partition_point(|time| *time <= sample.unix_ms);
            if let Some(boundary) = times.get(index) {
                end = end.min(*boundary);
            }
        }
        end
    }
    fn coverage(
        &self,
        image: Screenshot,
        next: Option<&Screenshot>,
        range: Range,
    ) -> Option<ImageCoverage> {
        let clipped = range.clip(image.sample.unix_ms, self.end(&image, next))?;
        Some(ImageCoverage {
            range: clipped,
            segment: image.sample.segment,
            screenshot: image.id,
            monitor: image.sample.monitor,
            unavailable: image
                .unavailable
                .or_else(|| image.image.is_none().then(|| "unavailable".into())),
        })
    }
}

fn validate_monitor(monitor: Option<&str>) -> Result<()> {
    if monitor.is_some_and(|m| m.is_empty() || m.len() > 256) {
        bail!(QueryError::InvalidRequest);
    }
    Ok(())
}

impl Queries {
    pub fn screenshots(
        &self,
        client: Client,
        range: Range,
        monitor: Option<&str>,
        page: PageRequest,
    ) -> Result<Page<Screenshot>> {
        range.validate(48 * HOUR)?;
        validate_monitor(monitor)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor(format!("screenshots:{range:?}:{monitor:?}"), &page)?;
        let mut output = Pager::new(cursor.clone(), &page);
        let mut statement = self.history.connection.prepare_cached(&format!("SELECT {COLUMNS} FROM screenshots WHERE unix_ms>=?1 AND unix_ms<?2 AND id<=?3 AND (?4 IS NULL OR monitor=?4) ORDER BY segment,monitor,monotonic_ms,id"))?;
        let mut rows = statement.query(params![
            range.start_ms,
            range.end_ms,
            cursor.screenshots,
            monitor
        ])?;
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            output.push(screenshot_row(row)?)?;
        }
        output.finish()
    }

    pub(super) fn screenshot(&self, id: i64) -> Result<Option<Screenshot>> {
        Ok(self
            .history
            .connection
            .prepare_cached(&format!("SELECT {COLUMNS} FROM screenshots WHERE id=?1"))?
            .query_row([id], screenshot_row)
            .optional()?)
    }

    /// Samples of one segment, or of all, ordered by segment, monitor and time.
    fn scan_images(
        &self,
        cursor: &Cursor,
        segment: Option<i64>,
        monitor: Option<&str>,
        visit: &mut impl FnMut(Screenshot) -> Result<()>,
    ) -> Result<()> {
        let order = "ORDER BY segment,monitor,monotonic_ms,id";
        let mut statement = self.history.connection.prepare_cached(&match segment {
            Some(_) => format!("SELECT {COLUMNS} FROM screenshots WHERE segment=?3 AND id<=?1 AND (?2 IS NULL OR monitor=?2) {order}"),
            None => format!("SELECT {COLUMNS} FROM screenshots WHERE id<=?1 AND (?2 IS NULL OR monitor=?2) AND ?3 IS NULL {order}"),
        })?;
        let mut rows = statement.query(params![cursor.screenshots, monitor, segment])?;
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            visit(screenshot_row(row)?)?;
        }
        Ok(())
    }

    pub(super) fn image_coverage(
        &self,
        cursor: &Cursor,
        range: Range,
        monitor: Option<&str>,
        plan: &Plan,
        context: &ImageContext,
        mut visit: impl FnMut(ImageCoverage) -> Result<()>,
    ) -> Result<()> {
        validate_monitor(monitor)?;
        let mut previous: Option<Screenshot> = None;
        let mut pair = |next: Screenshot| {
            if let Some(old) = previous.take() {
                let same = old.sample.segment == next.sample.segment
                    && old.sample.monitor == next.sample.monitor;
                if let Some(coverage) = context.coverage(old, same.then_some(&next), range) {
                    visit(coverage)?;
                }
            }
            previous = Some(next);
            Ok(())
        };
        match &plan.images {
            Some(segments) => {
                for &segment in segments {
                    self.scan_images(cursor, Some(segment), monitor, &mut pair)?;
                }
            }
            None => self.scan_images(cursor, None, monitor, &mut pair)?,
        }
        if let Some(coverage) = previous.and_then(|old| context.coverage(old, None, range)) {
            visit(coverage)?;
        }
        Ok(())
    }
}

pub(super) fn screenshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Screenshot> {
    Ok(Screenshot {
        id: row.get(0)?,
        sample: ScreenshotSample {
            unix_ms: row.get(1)?,
            monotonic_ms: row.get(2)?,
            segment: row.get(3)?,
            monitor: row.get(4)?,
            width: row.get(5)?,
            height: row.get(6)?,
            transform: row.get(7)?,
        },
        image: row.get(8)?,
        unavailable: row.get(9)?,
    })
}
