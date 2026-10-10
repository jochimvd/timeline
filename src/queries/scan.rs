//! Planned observation reads: one shared parser for streamed rows and a
//! bounded cache of parsed whole segments.
use super::*;

// Private scan rows can share a checkpoint's parsed state without changing the
// owned public evidence contract or allowing cached data to outlive a query.
#[derive(Clone)]
pub(super) struct ScanObservation {
    pub id: i64,
    pub unix_ms: i64,
    pub monotonic_ms: i64,
    pub segment: i64,
    pub evidence: Arc<Evidence>,
}
impl From<ScanObservation> for Observation {
    fn from(row: ScanObservation) -> Self {
        Self {
            id: row.id,
            unix_ms: row.unix_ms,
            monotonic_ms: row.monotonic_ms,
            segment: row.segment,
            evidence: (*row.evidence).clone(),
        }
    }
}

const COLUMNS: &str = "id,unix_ms,elapsed_ms,segment,evidence";

/// Continued focus produces identical checkpoint payloads every four seconds.
/// Reuse only the last small typed state, never identities or timestamps.
#[derive(Default)]
struct Parser {
    last: Option<(String, Arc<Evidence>)>,
    /// Estimated heap retained by newly parsed evidence.
    bytes: usize,
}
impl Parser {
    fn parse(&mut self, text: &str) -> Result<Arc<Evidence>> {
        if text.len() > PAGE_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        if let Some((last, evidence)) = &self.last
            && last == text
        {
            return Ok(evidence.clone());
        }
        let evidence: Arc<Evidence> = Arc::new(serde_json::from_str(text)?);
        // Typed state has only a fixed struct and owned strings; unknown JSON
        // can have many tiny array/object allocations.
        let expansion = if matches!(*evidence, Evidence::Unknown(_)) {
            64
        } else {
            2
        };
        self.bytes += text.len() * expansion + std::mem::size_of::<Evidence>() + 128;
        self.last = (text.len() <= 16 * 1024 && matches!(*evidence, Evidence::State { .. }))
            .then(|| (text.to_owned(), evidence.clone()));
        Ok(evidence)
    }
    /// Reads a row selected with `COLUMNS`, borrowing its text until parsed.
    fn row(&mut self, row: &rusqlite::Row<'_>) -> Result<ScanObservation> {
        Ok(ScanObservation {
            id: row.get(0)?,
            unix_ms: row.get(1)?,
            monotonic_ms: row.get(2)?,
            segment: row.get(3)?,
            evidence: self.parse(row.get_ref(4)?.as_str()?)?,
        })
    }
}

// Parsed whole segments for one database and retention epoch, within 24 MiB.
// Closed segments are immutable and the open one extends at append watermarks;
// pinned readers use only rows through their own watermark. Larger segments
// stream with the parser's last-state reuse instead.
const PARSED_BYTES: usize = 24 * 1024 * 1024;
#[derive(Clone, Default)]
struct Parsed {
    rows: Vec<ScanObservation>,
    /// Every row of the segment through this observation id is present.
    through: i64,
    evidence_bytes: usize,
}
impl Parsed {
    fn bytes(&self) -> usize {
        self.rows.capacity() * std::mem::size_of::<ScanObservation>() + self.evidence_bytes
    }
}
#[derive(Default)]
struct ParsedCache {
    key: String,
    /// By first observation id, with a last-use tick.
    segments: HashMap<i64, (Arc<Parsed>, u64)>,
    oversized: HashSet<i64>,
    bytes: usize,
    tick: u64,
}
impl ParsedCache {
    fn of(key: &str) -> MutexGuard<'static, Self> {
        static CACHE: OnceLock<Mutex<ParsedCache>> = OnceLock::new();
        let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
        if cache.key != key {
            *cache = Self {
                key: key.into(),
                ..Default::default()
            };
        }
        cache
    }
    fn get(&mut self, first: i64) -> Option<Arc<Parsed>> {
        self.tick += 1;
        let (parsed, used) = self.segments.get_mut(&first)?;
        *used = self.tick;
        Some(parsed.clone())
    }
    fn insert(&mut self, first: i64, parsed: Arc<Parsed>) {
        if let Some((old, _)) = self.segments.remove(&first) {
            self.bytes -= old.bytes();
        }
        while self.bytes + parsed.bytes() > PARSED_BYTES {
            let oldest = self
                .segments
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(first, _)| *first)
                .unwrap();
            let (old, _) = self.segments.remove(&oldest).unwrap();
            self.bytes -= old.bytes();
        }
        self.tick += 1;
        self.bytes += parsed.bytes();
        self.segments.insert(first, (parsed, self.tick));
    }
}

impl Queries {
    /// Visits planned observations in id order, flagging whether each directly
    /// follows the previous visited one. Rows past the pinned watermark are skipped.
    pub(super) fn scan(
        &self,
        cursor: &Cursor,
        plan: &Plan,
        mut visit: impl FnMut(ScanObservation, bool) -> Result<()>,
    ) -> Result<()> {
        let mut parser = Parser::default();
        for &(from, to) in plan.chunks() {
            let mut consecutive = false;
            let mut emit = |observation| {
                self.check_cancelled()?;
                visit(observation, consecutive)?;
                consecutive = true;
                Ok(true)
            };
            for (from, to, run) in plan.pieces(from, to) {
                let parsed = match (run, &plan.segments) {
                    (Some(run), Some(segments)) => self.parsed(cursor, &segments.key, run)?,
                    _ => None,
                };
                match parsed {
                    Some(parsed) => {
                        for observation in &parsed.rows {
                            if observation.id > cursor.observations {
                                break;
                            }
                            emit(observation.clone())?;
                        }
                    }
                    None => {
                        let to = to.min(cursor.observations);
                        self.read(from, to, &mut parser, |observation, _| emit(observation))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Streams observations with ids in `from..=to` while `visit` returns true.
    /// Returns whether every row was visited.
    fn read(
        &self,
        from: i64,
        to: i64,
        parser: &mut Parser,
        mut visit: impl FnMut(ScanObservation, &Parser) -> Result<bool>,
    ) -> Result<bool> {
        let mut statement = self.history.connection.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM observations WHERE id>=?1 AND id<=?2 ORDER BY id"
        ))?;
        let mut rows = statement.query(params![from, to])?;
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            let observation = parser.row(row)?;
            if !visit(observation, parser)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// A whole segment's parsed rows through the pinned watermark, cached and
    /// extended as needed. None if the segment exceeds the cache budget.
    fn parsed(&self, cursor: &Cursor, key: &str, run: Run) -> Result<Option<Arc<Parsed>>> {
        let previous = {
            let mut cache = ParsedCache::of(key);
            if cache.oversized.contains(&run.first) {
                return Ok(None);
            }
            cache.get(run.first)
        };
        let through = previous.as_ref().map_or(run.first - 1, |p| p.through);
        if through >= cursor.observations.min(run.last) {
            return Ok(previous);
        }
        // Only the open segment grows, so a copy is bounded by one segment.
        let mut parsed = previous.map_or_else(Parsed::default, |p| (*p).clone());
        let mut parser = Parser {
            bytes: parsed.evidence_bytes,
            ..Default::default()
        };
        let to = run.last.min(cursor.observations);
        let complete = self.read(through + 1, to, &mut parser, |observation, parser| {
            parsed.rows.push(observation);
            parsed.evidence_bytes = parser.bytes;
            Ok(parsed.bytes() <= PARSED_BYTES)
        })?;
        let mut cache = ParsedCache::of(key);
        if !complete {
            cache.oversized.insert(run.first);
            return Ok(None);
        }
        parsed.through = cursor.observations;
        parsed.rows.shrink_to_fit();
        let parsed = Arc::new(parsed);
        cache.insert(run.first, parsed.clone());
        Ok(Some(parsed))
    }

    /// Identity of the latest recording boundary before `id`, or 0. Segments
    /// normally begin with one, so this reads at most one segment's tail.
    pub(super) fn boundary_before(&self, id: i64) -> Result<i64> {
        let mut statement = self.history.connection.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM observations WHERE id<?1 ORDER BY id DESC"
        ))?;
        let mut rows = statement.query([id])?;
        let mut parser = Parser::default();
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            let observation = parser.row(row)?;
            if is_boundary(&observation.evidence) {
                return Ok(observation.id);
            }
        }
        Ok(0)
    }

    /// Observations with a wall time inside the range, in id order.
    pub(super) fn observations_in(
        &self,
        cursor: &Cursor,
        range: Range,
        mut visit: impl FnMut(ScanObservation) -> Result<()>,
    ) -> Result<()> {
        let mut statement = self.history.connection.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM observations WHERE unix_ms>=?1 AND unix_ms<?2 AND id<=?3 ORDER BY id"
        ))?;
        let mut rows =
            statement.query(params![range.start_ms, range.end_ms, cursor.observations])?;
        let mut parser = Parser::default();
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            visit(parser.row(row)?)?;
        }
        Ok(())
    }
}
