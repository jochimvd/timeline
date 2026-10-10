//! Range planning over timing segments. Clock jumps, suspend, stalls and
//! monotonic resets all start a new segment, so wall time is near-monotonic
//! within one. Per-segment wall bounds restrict reads to the segments, and the
//! transitions between them, that can intersect a range. Plans cover whole
//! segments, so block identities and per-segment image endpoints stay exact.
use super::*;

// Writer-side read indexes: per-segment id and wall bounds become a few seeks
// per segment. Readers of histories without them scan once per process instead.
const INDEXES: [(&str, &str); 3] = [
    ("observations_by_segment", "observations(segment)"),
    (
        "observations_by_segment_time",
        "observations(segment,unix_ms)",
    ),
    (
        "screenshots_by_segment_time",
        "screenshots(segment,unix_ms)",
    ),
];
pub(crate) fn initialize(db: &rusqlite::Connection) -> Result<()> {
    for (name, columns) in INDEXES {
        db.execute_batch(&format!("CREATE INDEX IF NOT EXISTS {name} ON {columns};"))?;
    }
    Ok(())
}
fn indexed(db: &rusqlite::Connection) -> Result<bool> {
    let count: i64 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN (?1,?2,?3)",
        params![INDEXES[0].0, INDEXES[1].0, INDEXES[2].0],
        |row| row.get(0),
    )?;
    Ok(count == INDEXES.len() as i64)
}
/// Each distinct segment by seeking past the previous one.
fn distinct(table: &str) -> String {
    format!(
        "WITH RECURSIVE s(segment) AS (SELECT min(segment) FROM {table} UNION ALL
        SELECT (SELECT min(segment) FROM {table} WHERE segment>s.segment) FROM s
        WHERE s.segment IS NOT NULL)"
    )
}

/// Observation ids and wall bounds of one timing segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Run {
    pub first: i64,
    pub last: i64,
    earliest: i64,
    latest: i64,
}
impl Run {
    fn overlaps(&self, range: Range) -> bool {
        self.earliest < range.end_ms && self.latest > range.start_ms
    }
    fn merge(&mut self, other: Run) {
        self.first = self.first.min(other.first);
        self.last = self.last.max(other.last);
        self.earliest = self.earliest.min(other.earliest);
        self.latest = self.latest.max(other.latest);
    }
}

// Per-segment bounds for one database and retention epoch, extended at append
// watermarks. Appends only widen bounds, so a pinned reader may plan with a
// newer summary: it selects a superset and its scans stop at its watermark.
const MAX_SEGMENTS: usize = 65536;
#[derive(Clone)]
pub(super) struct Segments {
    pub key: String,
    observations: i64,
    screenshots: i64,
    /// None past `MAX_SEGMENTS`; every plan then covers the whole history.
    by_segment: Option<BTreeMap<i64, Run>>,
    /// Id order. None when segments interleave by id (not written by recorders).
    runs: Option<Vec<Run>>,
    images: Option<BTreeMap<i64, (i64, i64)>>,
}
static SEGMENTS: OnceLock<Mutex<Option<Arc<Segments>>>> = OnceLock::new();

impl Segments {
    fn new(key: String) -> Self {
        Self {
            key,
            observations: 0,
            screenshots: 0,
            by_segment: Some(BTreeMap::new()),
            runs: Some(Vec::new()),
            images: Some(BTreeMap::new()),
        }
    }
    /// Includes rows through the cursor; seeks may also include later appends.
    fn extend(&mut self, db: &rusqlite::Connection, cursor: &Cursor) -> Result<()> {
        let seek = self.observations == 0 && self.screenshots == 0 && indexed(db)?;
        if cursor.observations > self.observations {
            let (sql, bounds) = if seek {
                (
                    format!(
                        "{} SELECT segment,(SELECT min(id) FROM observations WHERE segment=s.segment),(SELECT max(id) FROM observations WHERE segment=s.segment),(SELECT min(unix_ms) FROM observations WHERE segment=s.segment),(SELECT max(unix_ms) FROM observations WHERE segment=s.segment) FROM s WHERE segment IS NOT NULL",
                        distinct("observations")
                    ),
                    vec![],
                )
            } else {
                ("SELECT segment,min(id),max(id),min(unix_ms),max(unix_ms) FROM observations WHERE id>?1 AND id<=?2 GROUP BY segment".into(), vec![self.observations, cursor.observations])
            };
            let mut statement = db.prepare_cached(&sql)?;
            let mut rows = statement.query(rusqlite::params_from_iter(bounds))?;
            while let Some(row) = rows.next()? {
                let run = Run {
                    first: row.get(1)?,
                    last: row.get(2)?,
                    earliest: row.get(3)?,
                    latest: row.get(4)?,
                };
                self.observations = self.observations.max(run.last);
                let Some(segments) = &mut self.by_segment else {
                    break;
                };
                segments
                    .entry(row.get(0)?)
                    .and_modify(|old| old.merge(run))
                    .or_insert(run);
                if segments.len() > MAX_SEGMENTS {
                    self.by_segment = None;
                }
            }
            self.observations = self.observations.max(cursor.observations);
            self.runs = self.by_segment.as_ref().and_then(|segments| {
                let mut runs: Vec<Run> = segments.values().copied().collect();
                runs.sort_unstable_by_key(|run| run.first);
                runs.windows(2)
                    .all(|pair| pair[0].last < pair[1].first)
                    .then_some(runs)
            });
        }
        if cursor.screenshots > self.screenshots {
            let (sql, bounds) = if seek {
                (
                    format!(
                        "{} SELECT segment,(SELECT min(unix_ms) FROM screenshots WHERE segment=s.segment),(SELECT max(unix_ms) FROM screenshots WHERE segment=s.segment) FROM s WHERE segment IS NOT NULL",
                        distinct("screenshots")
                    ),
                    vec![],
                )
            } else {
                ("SELECT segment,min(unix_ms),max(unix_ms) FROM screenshots WHERE id>?1 AND id<=?2 GROUP BY segment".into(), vec![self.screenshots, cursor.screenshots])
            };
            let mut statement = db.prepare_cached(&sql)?;
            let mut rows = statement.query(rusqlite::params_from_iter(bounds))?;
            while let Some(row) = rows.next()? {
                let bounds: (i64, i64) = (row.get(1)?, row.get(2)?);
                let Some(images) = &mut self.images else {
                    break;
                };
                images
                    .entry(row.get(0)?)
                    .and_modify(|old| *old = (old.0.min(bounds.0), old.1.max(bounds.1)))
                    .or_insert(bounds);
                if images.len() > MAX_SEGMENTS {
                    self.images = None;
                }
            }
            // Seeks may include later appends; extensions then re-merge them.
            self.screenshots = cursor.screenshots;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Whole segments overlapping the range; blocks never cross segments.
    Blocks,
    /// Focus coverage, including transitions and the latest-evidence tail, and
    /// the segments whose image coverage can intersect the range.
    Evidence,
}

pub(super) struct Plan {
    /// Inclusive id intervals, each a run of consecutive observations.
    chunks: Vec<(i64, i64)>,
    pub segments: Option<Arc<Segments>>,
    /// The latest observation when a coverage tail may depend on it.
    pub last: Option<i64>,
    /// Ascending screenshot segments to read; None reads every segment.
    pub images: Option<Vec<i64>>,
}
impl Plan {
    fn whole(last: Option<i64>) -> Self {
        Self {
            chunks: vec![(i64::MIN, i64::MAX)],
            segments: None,
            last,
            images: None,
        }
    }
    pub fn chunks(&self) -> &[(i64, i64)] {
        &self.chunks
    }
    /// Splits a chunk by segment, with the run when a piece covers all of it.
    pub fn pieces(&self, from: i64, to: i64) -> Vec<(i64, i64, Option<Run>)> {
        let Some(runs) = self.segments.as_ref().and_then(|s| s.runs.as_ref()) else {
            return vec![(from, to, None)];
        };
        let start = runs.partition_point(|run| run.last < from);
        runs[start..]
            .iter()
            .take_while(|run| run.first <= to)
            .map(|run| {
                let piece = (from.max(run.first), to.min(run.last));
                (
                    piece.0,
                    piece.1,
                    (piece == (run.first, run.last)).then_some(*run),
                )
            })
            .collect()
    }
}

impl Queries {
    fn segments(&self, cursor: &Cursor) -> Result<Arc<Segments>> {
        let key = format!("{}:{}", self.database_key()?, cursor.epoch);
        let cached = SEGMENTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .clone()
            .filter(|segments| segments.key == key);
        if let Some(cached) = &cached
            && cached.observations >= cursor.observations
            && cached.screenshots >= cursor.screenshots
        {
            return Ok(cached.clone());
        }
        let mut next = cached.map_or_else(|| Segments::new(key), |cached| (*cached).clone());
        next.extend(&self.history.connection, cursor)?;
        let next = Arc::new(next);
        let old = SEGMENTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .replace(next.clone());
        drop(old);
        Ok(next)
    }

    pub(super) fn plan(&self, cursor: &Cursor, range: Range, scope: Scope) -> Result<Plan> {
        let segments = self.segments(cursor)?;
        let last = match scope {
            Scope::Blocks => None,
            Scope::Evidence => self.latest_before(cursor, range)?,
        };
        let (Some(runs), Some(by_segment)) = (&segments.runs, &segments.by_segment) else {
            return Ok(Plan::whole(last));
        };
        let mut pieces = Vec::new();
        for (index, run) in runs.iter().enumerate() {
            if run.overlaps(range) {
                pieces.push((run.first, run.last));
            }
            // The interval between segments belongs to neither segment's bounds.
            if scope == Scope::Evidence
                && let Some(next) = runs.get(index + 1)
                && run.earliest < range.end_ms
                && next.latest > range.start_ms
            {
                pieces.push((run.last, run.last));
                pieces.push((next.first, next.first));
            }
        }
        pieces.extend(last.map(|id| (id, id)));
        let images = match (scope, &segments.images) {
            (Scope::Blocks, _) => Some(Vec::new()),
            (Scope::Evidence, None) => {
                pieces.extend(runs.iter().map(|run| (run.first, run.last)));
                None
            }
            (Scope::Evidence, Some(images)) => {
                let mut selected = Vec::new();
                for (&segment, &(earliest, latest)) in images {
                    let run = by_segment.get(&segment);
                    // Intervals end at the next sample or the last observation.
                    let end = run.map_or(i64::MIN, |run| run.latest);
                    if earliest < range.end_ms && latest.saturating_add(1).max(end) > range.start_ms
                    {
                        selected.push(segment);
                        pieces.extend(run.map(|run| (run.first, run.last)));
                    }
                }
                Some(selected)
            }
        };
        Ok(Plan {
            chunks: merge(pieces, runs),
            segments: Some(segments.clone()),
            last,
            images,
        })
    }

    /// The latest observation when it precedes the range end, so a gap tail
    /// can use it; known focus never extends past it into recorder downtime.
    fn latest_before(&self, cursor: &Cursor, range: Range) -> Result<Option<i64>> {
        let latest: Option<(i64, i64)> = self
            .history
            .connection
            .prepare_cached(
                "SELECT id,unix_ms FROM observations WHERE id<=?1 ORDER BY id DESC LIMIT 1",
            )?
            .query_row([cursor.observations], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        Ok(latest
            .filter(|(_, unix_ms)| *unix_ms < range.end_ms)
            .map(|(id, _)| id))
    }
}

/// Sorted, merged intervals. Adjacent segments join so the transition between
/// them is scanned as one consecutive pair.
fn merge(mut pieces: Vec<(i64, i64)>, runs: &[Run]) -> Vec<(i64, i64)> {
    pieces.sort_unstable();
    let successor = |id: i64| {
        let index = runs.binary_search_by_key(&id, |run| run.last).ok()?;
        runs.get(index + 1).map(|run| run.first)
    };
    let mut chunks: Vec<(i64, i64)> = Vec::new();
    for (from, to) in pieces {
        match chunks.last_mut() {
            Some(last) if from <= last.1 || successor(last.1) == Some(from) => {
                last.1 = last.1.max(to);
            }
            _ => chunks.push((from, to)),
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::{BoundaryKind, FocusState},
        history::ScreenshotSample,
    };
    fn run(first: i64, last: i64) -> Run {
        Run {
            first,
            last,
            earliest: 0,
            latest: 0,
        }
    }
    #[test]
    fn transitions_join_adjacent_segments_but_never_skip_observations() {
        let runs = [run(1, 10), run(11, 20), run(25, 30)];
        // A transition (10, 11) joins two whole segments; a lone first row does not
        // join a later last row of the same segment.
        assert_eq!(
            merge(vec![(11, 20), (1, 10), (10, 10), (11, 11)], &runs),
            [(1, 20)]
        );
        assert_eq!(
            merge(vec![(25, 25), (20, 20), (11, 11)], &runs),
            [(11, 11), (20, 25)]
        );
        assert_eq!(merge(vec![(30, 30), (25, 30)], &runs), [(25, 30)]);
    }

    /// Focus and image coverage rows plus blocks, as JSON for comparison.
    fn evidence(queries: &Queries, cursor: &Cursor, range: Range, plans: [&Plan; 2]) -> String {
        let mut context = ImageContext::default();
        let mut rows = Vec::new();
        queries
            .coverage(cursor, range, plans[0], Some(&mut context), |row| {
                rows.push(serde_json::to_value(row)?);
                Ok(())
            })
            .unwrap();
        queries
            .image_coverage(cursor, range, None, plans[0], &context, |row| {
                rows.push(serde_json::to_value(row)?);
                Ok(())
            })
            .unwrap();
        queries
            .planned_blocks(cursor, range, plans[1], |block| {
                rows.push(serde_json::to_value(block)?);
                Ok(())
            })
            .unwrap();
        serde_json::to_string(&rows).unwrap()
    }
    fn planned(queries: &Queries, cursor: &Cursor, range: Range) -> String {
        let evidence_plan = queries.plan(cursor, range, Scope::Evidence).unwrap();
        let blocks_plan = queries.plan(cursor, range, Scope::Blocks).unwrap();
        evidence(queries, cursor, range, [&evidence_plan, &blocks_plan])
    }
    fn whole(queries: &Queries, cursor: &Cursor, range: Range) -> String {
        let latest: i64 = queries
            .history
            .connection
            .query_row(
                "SELECT max(id) FROM observations WHERE id<=?1",
                [cursor.observations],
                |row| row.get(0),
            )
            .unwrap();
        let blocks = Plan::whole(None);
        evidence(
            queries,
            cursor,
            range,
            [&Plan::whole(Some(latest)), &blocks],
        )
    }

    #[test]
    fn segment_plans_match_whole_history_scans_across_clock_jumps() {
        equivalence(true);
        // Histories not yet reopened by a recorder lack the segment indexes.
        equivalence(false);
    }
    fn equivalence(indexed: bool) {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path()).unwrap();
        if !indexed {
            for (name, _) in INDEXES {
                history
                    .connection
                    .execute_batch(&format!("DROP INDEX {name}"))
                    .unwrap();
            }
        }
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as i64
        };
        let queries = Queries::open(dir.path()).unwrap();
        let mut wall = 1_000_000;
        let mut pinned = None;
        for segment in 1..=14 {
            // Repeated wall times, forward jumps and downtime between segments.
            wall += [-40_000, -6_000, 2_000, 90_000][random(4) as usize];
            let mut monotonic = segment * 10_000_000;
            // Recorders begin segments with a boundary; legacy fixtures may not.
            if random(4) > 0 {
                let reason = ["restart", "clock_discontinuity", "suspend"][random(3) as usize];
                let evidence = Evidence::Boundary {
                    kind: BoundaryKind::Boundary,
                    reason: reason.into(),
                    previous_durable_observation: None,
                    previous_durable_unix_ms: None,
                };
                history
                    .commit_evidence(wall, monotonic, segment, &evidence, None)
                    .unwrap();
            }
            for step in 0..30 {
                // Recording continues within the segment a pinned cursor ends in.
                if (segment, step) == (9, 15) {
                    pinned = Some(
                        queries
                            .cursor("pinned".into(), &PageRequest::default())
                            .unwrap(),
                    );
                }
                let kind = [
                    StateChange::Focus,
                    StateChange::Checkpoint,
                    StateChange::Checkpoint,
                    StateChange::Idle,
                    StateChange::Pause,
                    StateChange::Lock,
                ][random(6) as usize];
                let state = FocusState {
                    manual_paused: kind == StateChange::Pause,
                    connected: true,
                    locked: kind == StateChange::Lock,
                    idle: kind == StateChange::Idle,
                    focus: (random(5) > 0).then(|| Window {
                        address: "w".into(),
                        application: ["editor", "browser"][random(2) as usize].into(),
                        title: ["A", "B", "C"][random(3) as usize].into(),
                    }),
                    focus_known: true,
                    screenshot_recording: true,
                    screenshot_pressure: false,
                    screenshot_cleanup: false,
                };
                let evidence = Evidence::State { kind, state };
                history
                    .commit_evidence(wall, monotonic, segment, &evidence, None)
                    .unwrap();
                if random(3) == 0 {
                    // Samples may lie outside their segment's observation bounds.
                    let sample = ScreenshotSample {
                        unix_ms: wall + random(6_000) - 3_000,
                        monotonic_ms: monotonic,
                        segment: segment + i64::from(random(8) == 0),
                        monitor: ["A", "B"][random(2) as usize].into(),
                        width: 2,
                        height: 1,
                        transform: 0,
                    };
                    let reason =
                        ["capture_pending", "clock_discontinuity", "missing"][random(3) as usize];
                    history.screenshot_gap(&sample, reason).unwrap();
                }
                let step = 100 + random(3_000);
                // Small backward wall steps stay within one segment.
                wall += step - if random(8) == 0 { 400 } else { 0 };
                monotonic += step;
            }
        }
        let latest = queries
            .cursor("latest".into(), &PageRequest::default())
            .unwrap();
        let (start, end) = (900_000, wall + 20_000);
        let mut ranges = vec![Range {
            start_ms: start,
            end_ms: end,
        }];
        for _ in 0..400 {
            let start_ms = start + random((end - start) as u64);
            let length = [1, 50, 2_000, 30_000, 200_000][random(5) as usize];
            ranges.push(Range {
                start_ms,
                end_ms: start_ms + 1 + random(length),
            });
        }
        let pinned = pinned.unwrap();
        // Pinned first: the summary and parsed segments then extend at appends.
        let expected = whole(&queries, &pinned, ranges[0]);
        assert_eq!(planned(&queries, &pinned, ranges[0]), expected);
        for range in ranges {
            let expected = whole(&queries, &latest, range);
            assert_eq!(planned(&queries, &latest, range), expected, "{range:?}");
            // The pinned cursor plans with the newer, wider segment summary.
            let expected = whole(&queries, &pinned, range);
            assert_eq!(
                planned(&queries, &pinned, range),
                expected,
                "pinned {range:?}"
            );
        }
    }
}
