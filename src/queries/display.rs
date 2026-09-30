//! Lossy display summaries only. Original coverage and point selections stay exact.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayFocus {
    pub range: Range,
    pub focus: FocusCoverage,
    pub mixed: bool,
    pub has_gap: bool,
    pub idle: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayImage {
    pub range: Range,
    pub monitor: String,
    pub unavailable: Option<String>,
}

pub(super) struct Display {
    range: Range,
    step: i64,
    focus: BTreeMap<i64, (DisplayFocus, i64, Option<i64>, i64)>,
    images: BTreeMap<(String, i64), DisplayImage>,
}
impl Display {
    pub fn new(range: Range, points: usize) -> Result<Self> {
        if !(1..=2048).contains(&points) {
            bail!(QueryError::InvalidRequest);
        }
        Ok(Self {
            range,
            step: (range.end_ms - range.start_ms + points as i64 - 1) / points as i64,
            focus: BTreeMap::new(),
            images: BTreeMap::new(),
        })
    }
    pub fn coarse(&self) -> bool {
        self.step > 5000
    }
    fn pieces(&self, range: Range) -> impl Iterator<Item = (i64, Range)> + use<> {
        let start = self.range.start_ms;
        let end = self.range.end_ms;
        let step = self.step;
        let first = (range.start_ms - start) / step;
        let last = (range.end_ms - 1 - start) / step;
        (first..=last).map(move |index| {
            (
                index,
                Range {
                    start_ms: range.start_ms.max(start + index * step),
                    end_ms: range
                        .end_ms
                        .min(end.min(start.saturating_add((index + 1) * step))),
                },
            )
        })
    }
    pub fn focus(&mut self, coverage: Coverage) -> Result<()> {
        let gap = matches!(
            coverage.focus,
            FocusCoverage::Gap { .. } | FocusCoverage::Unknown
        );
        let idle = matches!(
            coverage.focus,
            FocusCoverage::Focus { idle: true, .. } | FocusCoverage::NoFocus { idle: true }
        );
        for (index, range) in self.pieces(coverage.range) {
            let duration = range.end_ms - range.start_ms;
            let (bucket, longest, segment, boundary) =
                self.focus.entry(index).or_insert_with(|| {
                    (
                        DisplayFocus {
                            range,
                            focus: coverage.focus.clone(),
                            mixed: false,
                            has_gap: gap,
                            idle,
                        },
                        duration,
                        coverage.segment,
                        coverage.recording_boundary,
                    )
                });
            bucket.mixed |= bucket.focus != coverage.focus
                || *segment != coverage.segment
                || *boundary != coverage.recording_boundary;
            bucket.has_gap |= gap;
            bucket.idle |= idle;
            bucket.range.start_ms = bucket.range.start_ms.min(range.start_ms);
            bucket.range.end_ms = bucket.range.end_ms.max(range.end_ms);
            // Representative is the longest individual interval, not an exact state.
            let focused = matches!(coverage.focus, FocusCoverage::Focus { .. });
            let was_focused = matches!(bucket.focus, FocusCoverage::Focus { .. });
            if (focused && !was_focused) || (focused == was_focused && duration > *longest) {
                bucket.focus = coverage.focus.clone();
                *longest = duration;
            }
        }
        Ok(())
    }
    pub fn image(&mut self, coverage: ImageCoverage) -> Result<()> {
        for (index, range) in self.pieces(coverage.range) {
            let key = (coverage.monitor.clone(), index);
            if !self.images.contains_key(&key) && self.images.len() >= 32768 {
                bail!(QueryError::LimitExceeded);
            }
            let bucket = self.images.entry(key).or_insert_with(|| DisplayImage {
                range,
                monitor: coverage.monitor.clone(),
                unavailable: coverage.unavailable.clone(),
            });
            if bucket.unavailable != coverage.unavailable
                || range.start_ms > bucket.range.end_ms
                || range.end_ms < bucket.range.start_ms
            {
                bucket.unavailable = Some("Mixed screenshot coverage · zoom in".into());
            }
            bucket.range.start_ms = bucket.range.start_ms.min(range.start_ms);
            bucket.range.end_ms = bucket.range.end_ms.max(range.end_ms);
        }
        Ok(())
    }
    pub fn rows(self) -> impl Iterator<Item = Breakdown> {
        let mut focus: Vec<DisplayFocus> = Vec::new();
        for (row, ..) in self.focus.into_values() {
            if let Some(previous) = focus.last_mut().filter(|previous| {
                previous.range.end_ms == row.range.start_ms
                    && previous.focus == row.focus
                    && previous.mixed == row.mixed
                    && previous.has_gap == row.has_gap
                    && previous.idle == row.idle
            }) {
                previous.range.end_ms = row.range.end_ms;
            } else {
                focus.push(row);
            }
        }
        let mut images: Vec<DisplayImage> = Vec::new();
        for row in self.images.into_values() {
            if let Some(previous) = images.last_mut().filter(|previous| {
                previous.range.end_ms == row.range.start_ms
                    && previous.monitor == row.monitor
                    && previous.unavailable == row.unavailable
            }) {
                previous.range.end_ms = row.range.end_ms;
            } else {
                images.push(row);
            }
        }
        focus
            .into_iter()
            .map(Breakdown::DisplayFocus)
            .chain(images.into_iter().map(Breakdown::DisplayImage))
    }
}
