//! Shared, bounded, read-only evidence service for browser and MCP adapters.
//! Each call reads one SQLite snapshot. Cursors pin append watermarks and are
//! invalidated by retention; image leases keep admitted streams alive after expiry.
use crate::{
    evidence::{Evidence, FocusState, StateChange},
    history::{History, ImageRead, Screenshot, ScreenshotSample},
    recording::Window,
};
use anyhow::{Result, bail};
use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

mod display;
mod navigation;
mod updates;
pub use display::{DisplayFocus, DisplayImage};
pub use navigation::ActivityBounds;
pub use updates::{OverviewChange, OverviewUpdates};

const HOUR: i64 = 3_600_000;
const PAGE_BYTES: usize = 1024 * 1024;
const BREAKDOWN_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryError {
    InvalidRequest,
    LimitExceeded,
    Busy,
    CursorInvalidated,
    Unavailable,
}
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for QueryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start_ms: i64,
    pub end_ms: i64,
}
impl Range {
    fn validate(self, maximum: i64) -> Result<()> {
        if self
            .end_ms
            .checked_sub(self.start_ms)
            .is_none_or(|n| n <= 0 || n > maximum)
        {
            bail!(QueryError::InvalidRequest);
        }
        Ok(())
    }
    fn clip(self, start: i64, end: i64) -> Option<Self> {
        let result = Self {
            start_ms: start.max(self.start_ms),
            end_ms: end.min(self.end_ms),
        };
        (result.start_ms < result.end_ms).then_some(result)
    }
    fn contains(self, time: i64) -> bool {
        self.start_ms <= time && time < self.end_ms
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRange {
    pub range: Range,
    pub timezone: String,
}
#[derive(Debug, Clone)]
pub enum Selection {
    ActivityDate(NaiveDate),
    Range(Range),
}

#[derive(Debug, Clone, Copy)]
pub enum Client {
    Browser,
    Mcp,
}

#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    /// The byte budget, rather than the requested record count, ended this page.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cursor {
    query: String,
    observations: i64,
    screenshots: i64,
    epoch: i64,
    offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub unix_ms: i64,
    pub monotonic_ms: i64,
    pub segment: i64,
    pub evidence: Evidence,
}

// Private scan rows can share a checkpoint's parsed state without changing the
// owned public evidence contract or allowing cached data to outlive a query.
#[derive(Clone)]
struct ScanObservation {
    id: i64,
    unix_ms: i64,
    monotonic_ms: i64,
    segment: i64,
    evidence: Arc<Evidence>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupKey {
    pub application: String,
    pub title: String,
}
pub trait Grouping: Send + Sync {
    /// Change this version whenever the policy changes; cursors and block IDs use it.
    fn version(&self) -> &str;
    fn key(&self, window: &Window) -> GroupKey;
}
pub struct ExactTitle;
impl Grouping for ExactTitle {
    fn version(&self) -> &str {
        "exact-title-v1"
    }
    fn key(&self, window: &Window) -> GroupKey {
        GroupKey {
            application: window.application.clone(),
            title: window.title.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub id: String,
    pub recording_boundary: i64,
    pub first_observation: i64,
    pub last_observation: i64,
    pub segment: i64,
    pub range: Range,
    pub elapsed_ms: i64,
    pub idle_ms: i64,
    pub group: GroupKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FocusCoverage {
    Focus { window: Window, idle: bool },
    NoFocus { idle: bool },
    Gap { reason: String },
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coverage {
    pub range: Range,
    pub segment: Option<i64>,
    pub observation: Option<i64>,
    pub recording_boundary: i64,
    /// None for uncertain intervals; wall-clock gap widths are not focus time.
    pub elapsed_ms: Option<i64>,
    pub focus: FocusCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageCoverage {
    pub range: Range,
    pub segment: i64,
    pub screenshot: i64,
    pub monitor: String,
    pub unavailable: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Breakdown {
    Application {
        application: String,
        elapsed_ms: i64,
        percent_of_focus: f64,
    },
    ExactTitle {
        application: String,
        title: String,
        elapsed_ms: i64,
        percent_of_focus: f64,
    },
    FocusCoverage(Coverage),
    ImageCoverage(ImageCoverage),
    DisplayFocus(DisplayFocus),
    DisplayImage(DisplayImage),
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct Totals {
    pub focus_ms: i64,
    pub no_focus_ms: i64,
    pub idle_ms: i64,
}
#[derive(Debug, Serialize)]
pub struct Overview {
    pub revision: String,
    pub resolved: ResolvedRange,
    pub totals: Totals,
    pub breakdown: Page<Breakdown>,
}
#[derive(Debug, Serialize)]
pub struct CursorEvidence {
    pub selected_ms: i64,
    /// Multiple entries disclose repeated wall times instead of choosing a segment silently.
    pub focus: Vec<Coverage>,
    pub screenshots: Vec<SelectedScreenshot>,
}
#[derive(Debug, Serialize)]
pub struct SelectedScreenshot {
    pub screenshot: Screenshot,
    pub stale: bool,
}
pub struct RetrievedImage {
    pub screenshot: Screenshot,
    pub stream: ImageStream,
}
pub struct ImageStream {
    image: ImageRead,
    _mcp: Option<File>,
}
impl std::io::Read for ImageStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.image, buffer)
    }
}

// Original observations are immutable while retained. Keep one bounded parsed
// snapshot, extend it from append watermarks, and rebuild after retention. Pinned
// readers may use the prefix of a newer snapshot, but never its later records.
const OBSERVATION_CACHE_BYTES: usize = 24 * 1024 * 1024;
struct ObservationSnapshot {
    key: String,
    watermark: i64,
    rows: Vec<ScanObservation>,
    evidence_bytes: usize,
}
static OBSERVATION_SNAPSHOT: OnceLock<Mutex<Option<Arc<ObservationSnapshot>>>> = OnceLock::new();

// Immutable serialized rows keep pagination cache accounting independent of
// Rust enum/string allocation overhead. One result is capped at 8 MiB and the
// process cache at 16 MiB; two admitted readers can retain two evicted results.
const OVERVIEW_CACHE_BYTES: usize = 16 * 1024 * 1024;
struct CachedOverview {
    key: String,
    totals: Totals,
    rows: Vec<Box<[u8]>>,
    bytes: usize,
}
#[derive(Default)]
struct OverviewCache {
    entries: VecDeque<Arc<CachedOverview>>,
    bytes: usize,
}
static OVERVIEWS: OnceLock<Mutex<OverviewCache>> = OnceLock::new();
fn cached_overview(key: &str) -> Option<Arc<CachedOverview>> {
    let mut cache = OVERVIEWS.get_or_init(Default::default).lock().unwrap();
    let index = cache.entries.iter().position(|entry| entry.key == key)?;
    let entry = cache.entries.remove(index).unwrap();
    cache.entries.push_back(entry.clone());
    Some(entry)
}
fn remember_overview(entry: CachedOverview) {
    let mut cache = OVERVIEWS.get_or_init(Default::default).lock().unwrap();
    if cache.entries.iter().any(|old| old.key == entry.key) {
        return;
    }
    while cache.bytes + entry.bytes > OVERVIEW_CACHE_BYTES {
        let old = cache.entries.pop_front().unwrap();
        cache.bytes -= old.bytes;
    }
    cache.bytes += entry.bytes;
    cache.entries.push_back(Arc::new(entry));
}
struct OverviewBuilder {
    rows: Option<Vec<Box<[u8]>>>,
    bytes: usize,
}
impl OverviewBuilder {
    fn new() -> Self {
        Self {
            rows: Some(Vec::new()),
            bytes: 16 * 1024,
        }
    }
    fn push(&mut self, item: &Breakdown) -> Result<()> {
        if let Some(rows) = &mut self.rows {
            let encoded = serde_json::to_vec(item)?.into_boxed_slice();
            // Account for capacity growth in the vector and allocator overhead.
            self.bytes += encoded.len() + 64;
            if self.bytes > BREAKDOWN_BYTES {
                self.rows = None;
            } else {
                rows.push(encoded);
            }
        }
        Ok(())
    }
}

struct Permit {
    global: File,
    mcp: Option<File>,
}
fn try_lock(path: impl AsRef<Path>) -> Result<Option<File>> {
    let file = File::open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub struct Queries {
    history: History,
    timezone: Tz,
    grouping: Box<dyn Grouping>,
    cancelled: Option<Arc<AtomicBool>>,
}
impl Queries {
    pub fn open(directory: &Path) -> Result<Self> {
        Self::with_policy(
            directory,
            crate::config::ACTIVITY_TIMEZONE,
            Box::new(ExactTitle),
        )
    }
    pub fn with_policy(
        directory: &Path,
        timezone: &str,
        grouping: Box<dyn Grouping>,
    ) -> Result<Self> {
        let timezone = timezone.parse().map_err(|_| QueryError::InvalidRequest)?;
        Ok(Self {
            history: History::read(directory)?,
            timezone,
            grouping,
            cancelled: None,
        })
    }
    pub(crate) fn cancel_when(mut self, cancelled: Option<Arc<AtomicBool>>) -> Self {
        self.cancelled = cancelled;
        self
    }
    fn check_cancelled(&self) -> Result<()> {
        if self
            .cancelled
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            bail!(QueryError::Busy);
        }
        Ok(())
    }
    pub fn resolve(&self, selection: Selection) -> Result<ResolvedRange> {
        let range = match selection {
            Selection::Range(range) => range,
            Selection::ActivityDate(date) => {
                let next = date.succ_opt().ok_or(QueryError::InvalidRequest)?;
                let boundary = |date: NaiveDate| -> Result<i64> {
                    let local = date
                        .and_hms_opt(4, 0, 0)
                        .ok_or(QueryError::InvalidRequest)?;
                    // An exceptional timezone fold/gap at 04:00 needs an explicit
                    // range, not an arbitrary choice of one possible boundary.
                    Ok(self
                        .timezone
                        .from_local_datetime(&local)
                        .single()
                        .ok_or(QueryError::InvalidRequest)?
                        .timestamp_millis())
                };
                Range {
                    start_ms: boundary(date)?,
                    end_ms: boundary(next)?,
                }
            }
        };
        range.validate(31 * 24 * HOUR)?;
        Ok(ResolvedRange {
            range,
            timezone: self.timezone.to_string(),
        })
    }
    fn admit(&self, client: Client) -> Result<Permit> {
        self.check_cancelled()?;
        let directory = &self.history.directory;
        let mcp = if matches!(client, Client::Mcp) {
            // Older v3 histories already have the global slots. Create only this
            // coordination file when opening one offline, without rewriting data.
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(directory.join("mcp-read.lock"))?;
            Some(try_lock(directory.join("mcp-read.lock"))?.ok_or(QueryError::Busy)?)
        } else {
            None
        };
        for index in 0..2 {
            if let Some(global) = try_lock(directory.join(format!("image-read-{index}.lock")))? {
                return Ok(Permit { global, mcp });
            }
        }
        bail!(QueryError::Busy)
    }
    fn cursor(&self, query: String, page: &PageRequest) -> Result<Cursor> {
        if !(1..=500).contains(&page.limit.unwrap_or(100)) {
            bail!(QueryError::InvalidRequest);
        }
        let connection = &self.history.connection;
        let epoch = connection
            .query_row(
                "SELECT value FROM state WHERE key='retention_epoch'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if let Some(token) = &page.cursor {
            if token.len() > 4096 {
                bail!(QueryError::InvalidRequest);
            }
            let bytes = decode_cursor(token)?;
            let cursor: Cursor =
                serde_json::from_slice(&bytes).map_err(|_| QueryError::InvalidRequest)?;
            if cursor.query != query {
                bail!(QueryError::InvalidRequest);
            }
            if cursor.epoch != epoch {
                bail!(QueryError::CursorInvalidated);
            }
            return Ok(cursor);
        }
        Ok(Cursor {
            query,
            epoch,
            offset: 0,
            observations: connection.query_row(
                "SELECT coalesce(max(id),0) FROM observations",
                [],
                |r| r.get(0),
            )?,
            screenshots: connection.query_row(
                "SELECT coalesce(max(id),0) FROM screenshots",
                [],
                |r| r.get(0),
            )?,
        })
    }
    fn observation_snapshot(&self, cursor: &Cursor) -> Result<Option<Arc<ObservationSnapshot>>> {
        let path = self
            .history
            .directory
            .join("history.sqlite3")
            .canonicalize()?;
        let metadata = std::fs::metadata(&path)?;
        let key = format!(
            "{}:{}:{}:{}",
            path.display(),
            metadata.dev(),
            metadata.ino(),
            cursor.epoch
        );
        let previous = OBSERVATION_SNAPSHOT
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .clone()
            .filter(|snapshot| snapshot.key == key);
        if let Some(previous) = &previous
            && previous.watermark >= cursor.observations
        {
            return Ok(Some(previous.clone()));
        }
        let after = previous.as_ref().map_or(0, |snapshot| snapshot.watermark);
        let additional: i64 = self.history.connection.query_row(
            "SELECT count(*) FROM observations WHERE id>?1 AND id<=?2",
            params![after, cursor.observations],
            |row| row.get(0),
        )?;
        let total = previous.as_ref().map_or(0, |snapshot| snapshot.rows.len())
            + usize::try_from(additional)?;
        let mut evidence_bytes = previous
            .as_ref()
            .map_or(0, |snapshot| snapshot.evidence_bytes);
        let row_bytes = total.saturating_mul(std::mem::size_of::<ScanObservation>());
        if row_bytes
            .saturating_add(evidence_bytes)
            .saturating_add(16 * 1024)
            > OBSERVATION_CACHE_BYTES
        {
            return Ok(None);
        }
        let mut records = Vec::new();
        records.try_reserve_exact(total)?;
        if let Some(previous) = &previous {
            records.extend(previous.rows.iter().cloned());
        }
        let row_bytes = records.capacity() * std::mem::size_of::<ScanObservation>();
        let mut statement = self.history.connection.prepare(
            "SELECT id,unix_ms,elapsed_ms,segment,evidence FROM observations WHERE id>?1 AND id<=?2 ORDER BY id")?;
        let mut rows = statement.query(params![after, cursor.observations])?;
        let mut cached: Option<(String, Arc<Evidence>)> = None;
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            let text: String = row.get(4)?;
            if text.len() > PAGE_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            let evidence = match cached.as_ref().filter(|(old, _)| old == &text) {
                Some((_, evidence)) => evidence.clone(),
                None => {
                    let evidence: Arc<Evidence> = Arc::new(serde_json::from_str(&text)?);
                    // Typed state has only a fixed struct and owned strings;
                    // unknown JSON can have many tiny array/object allocations.
                    evidence_bytes += text.len()
                        * if matches!(*evidence, Evidence::Unknown(_)) {
                            64
                        } else {
                            2
                        }
                        + std::mem::size_of::<Evidence>()
                        + 128;
                    if row_bytes + evidence_bytes + 16 * 1024 > OBSERVATION_CACHE_BYTES {
                        return Ok(None);
                    }
                    cached =
                        if text.len() <= 16 * 1024 && matches!(*evidence, Evidence::State { .. }) {
                            Some((text, evidence.clone()))
                        } else {
                            None
                        };
                    evidence
                }
            };
            records.push(ScanObservation {
                id: row.get(0)?,
                unix_ms: row.get(1)?,
                monotonic_ms: row.get(2)?,
                segment: row.get(3)?,
                evidence,
            });
        }
        let snapshot = Arc::new(ObservationSnapshot {
            key,
            watermark: cursor.observations,
            rows: records,
            evidence_bytes,
        });
        let old = OBSERVATION_SNAPSHOT
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .replace(snapshot.clone());
        drop(old);
        Ok(Some(snapshot))
    }
    // Oversized histories fall back to a streaming scan with one cached state.
    // Snapshot watermarks preserve pagination even when later appends are cached.
    fn scan(
        &self,
        cursor: &Cursor,
        mut visit: impl FnMut(ScanObservation) -> Result<()>,
    ) -> Result<()> {
        if let Some(snapshot) = self.observation_snapshot(cursor)? {
            for record in snapshot
                .rows
                .iter()
                .take_while(|record| record.id <= cursor.observations)
            {
                self.check_cancelled()?;
                visit(record.clone())?;
            }
            return Ok(());
        }
        let mut statement = self.history.connection.prepare("SELECT id,unix_ms,elapsed_ms,segment,evidence FROM observations WHERE id<=?1 ORDER BY id")?;
        let mut rows = statement.query(params![cursor.observations])?;
        // Continued focus produces identical checkpoint payloads every four
        // seconds. Reuse only the last small typed state, never observations or
        // their identities/timestamps. This cache dies with the read snapshot.
        let mut cached: Option<(String, Arc<Evidence>)> = None;
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            let text: String = row.get(4)?;
            if text.len() > PAGE_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            let evidence = match cached.as_ref().filter(|(previous, _)| previous == &text) {
                Some((_, evidence)) => evidence.clone(),
                None => {
                    let evidence: Arc<Evidence> = Arc::new(serde_json::from_str(&text)?);
                    cached =
                        if text.len() <= 16 * 1024 && matches!(*evidence, Evidence::State { .. }) {
                            Some((text, evidence.clone()))
                        } else {
                            None
                        };
                    evidence
                }
            };
            visit(ScanObservation {
                id: row.get(0)?,
                unix_ms: row.get(1)?,
                monotonic_ms: row.get(2)?,
                segment: row.get(3)?,
                evidence,
            })?;
        }
        Ok(())
    }
    pub fn observations(
        &self,
        client: Client,
        range: Range,
        page: PageRequest,
    ) -> Result<Page<Observation>> {
        range.validate(48 * HOUR)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor(format!("observations:{range:?}"), &page)?;
        let mut output = Pager::new(cursor.clone(), &page);
        self.scan(&cursor, |observation| {
            if range.contains(observation.unix_ms) {
                output.push(Observation::from(observation))?;
            }
            Ok(())
        })?;
        output.finish()
    }
    pub fn observation(&self, client: Client, id: i64) -> Result<Observation> {
        if id <= 0 {
            bail!(QueryError::InvalidRequest);
        }
        let _permit = self.admit(client)?;
        let record = self
            .history
            .connection
            .query_row(
                "SELECT unix_ms,elapsed_ms,segment,evidence FROM observations WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or(QueryError::Unavailable)?;
        if record.3.len() > PAGE_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        Ok(Observation {
            id,
            unix_ms: record.0,
            monotonic_ms: record.1,
            segment: record.2,
            evidence: serde_json::from_str(&record.3)?,
        })
    }
    fn coverage(
        &self,
        cursor: &Cursor,
        range: Range,
        mut visit: impl FnMut(Coverage) -> Result<()>,
    ) -> Result<()> {
        let mut previous: Option<ScanObservation> = None;
        let mut furthest = range.start_ms;
        let mut recording_boundary = 0;
        self.scan(cursor, |next| {
            if let Some(old) = previous.take() {
                if is_boundary(&old.evidence) {
                    recording_boundary = old.id;
                }
                if let Some(clipped) = range.clip(old.unix_ms, next.unix_ms) {
                    if clipped.start_ms > furthest {
                        visit(unknown(Range {
                            start_ms: furthest,
                            end_ms: clipped.start_ms,
                        }))?;
                    }
                    furthest = furthest.max(clipped.end_ms);
                    let continuous =
                        old.segment == next.segment && next.monotonic_ms >= old.monotonic_ms;
                    let focus = if continuous {
                        classify(&old.evidence)
                    } else {
                        match next.evidence.as_ref() {
                            Evidence::Boundary { reason, .. } => FocusCoverage::Gap {
                                reason: reason.clone(),
                            },
                            _ => FocusCoverage::Unknown,
                        }
                    };
                    let elapsed_ms = if continuous
                        && matches!(
                            focus,
                            FocusCoverage::Focus { .. } | FocusCoverage::NoFocus { .. }
                        ) {
                        Some(clipped_elapsed(&old, &next, clipped))
                    } else {
                        None
                    };
                    visit(Coverage {
                        range: clipped,
                        segment: Some(old.segment),
                        observation: Some(old.id),
                        recording_boundary,
                        elapsed_ms,
                        focus,
                    })?;
                }
            }
            previous = Some(next);
            Ok(())
        })?;
        if furthest < range.end_ms {
            let tail = Range {
                start_ms: furthest,
                end_ms: range.end_ms,
            };
            // A known pause/lock remains a gap; last known focus never extends
            // past its durable endpoint into recorder downtime.
            let focus = previous
                .as_ref()
                .filter(|o| o.unix_ms <= tail.start_ms)
                .map(|o| classify(&o.evidence));
            match focus {
                Some(focus @ FocusCoverage::Gap { .. }) => visit(Coverage {
                    range: tail,
                    segment: previous.as_ref().map(|o| o.segment),
                    observation: previous.as_ref().map(|o| o.id),
                    recording_boundary,
                    elapsed_ms: None,
                    focus,
                })?,
                _ => visit(unknown(tail))?,
            }
        }
        Ok(())
    }
    pub fn blocks(&self, client: Client, range: Range, page: PageRequest) -> Result<Page<Block>> {
        range.validate(48 * HOUR)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor(
            format!("blocks:{range:?}:{}", self.grouping.version()),
            &page,
        )?;
        let mut output = Pager::new(cursor.clone(), &page);
        let mut block: Option<Block> = None;
        // Scan from the beginning so a clipped block retains its original ID.
        let full = Range {
            start_ms: i64::MIN,
            end_ms: i64::MAX,
        };
        self.coverage(&cursor, full, |coverage| {
            if let FocusCoverage::Focus { window, idle } = &coverage.focus {
                let key = self.grouping.key(window);
                let segment = coverage.segment.unwrap();
                let id = coverage.observation.unwrap();
                let compatible = block.as_ref().is_some_and(|b| {
                    b.segment == segment
                        && b.recording_boundary == coverage.recording_boundary
                        && b.group == key
                        && b.range.end_ms == coverage.range.start_ms
                });
                if !compatible {
                    if let Some(old) = block.take() {
                        emit_block(old, range, &mut output)?;
                    }
                    block = Some(Block {
                        id: format!("{}:{segment}:{id}", self.grouping.version()),
                        recording_boundary: coverage.recording_boundary,
                        first_observation: id,
                        last_observation: id,
                        segment,
                        range: coverage.range,
                        elapsed_ms: 0,
                        idle_ms: 0,
                        group: key,
                    });
                }
                let b = block.as_mut().unwrap();
                b.range.end_ms = coverage.range.end_ms;
                b.last_observation = id;
                if let Some(clip) = range.clip(coverage.range.start_ms, coverage.range.end_ms) {
                    let duration =
                        proportional(coverage.elapsed_ms.unwrap_or(0), coverage.range, clip);
                    b.elapsed_ms += duration;
                    if *idle {
                        b.idle_ms += duration;
                    }
                }
            } else if let Some(old) = block.take() {
                emit_block(old, range, &mut output)?;
            }
            Ok(())
        })?;
        if let Some(old) = block {
            emit_block(old, range, &mut output)?;
        }
        output.finish()
    }
    fn overview_key(&self, cursor: &Cursor) -> Result<String> {
        let path = self
            .history
            .directory
            .join("history.sqlite3")
            .canonicalize()?;
        let metadata = std::fs::metadata(&path)?;
        Ok(format!(
            "{}:{}:{}:{}:{}:{}:{}",
            path.display(),
            metadata.dev(),
            metadata.ino(),
            cursor.query,
            cursor.observations,
            cursor.screenshots,
            cursor.epoch
        ))
    }
    pub fn overview(
        &self,
        client: Client,
        selection: Selection,
        page: PageRequest,
    ) -> Result<Overview> {
        self.overview_display(client, selection, page, None)
    }
    /// Optional viewport budget affects display coverage only, never exact totals or evidence.
    pub fn overview_display(
        &self,
        client: Client,
        selection: Selection,
        page: PageRequest,
        points: Option<usize>,
    ) -> Result<Overview> {
        let resolved = self.resolve(selection)?;
        let mut display = points
            .map(|points| display::Display::new(resolved.range, points))
            .transpose()?
            .filter(display::Display::coarse);
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor(
            format!(
                "overview:{:?}:{}:{points:?}",
                resolved.range, resolved.timezone
            ),
            &page,
        )?;
        let key = self.overview_key(&cursor)?;
        let mut output = Pager::new(cursor.clone(), &page);
        if let Some(cached) = cached_overview(&key) {
            // Skip directly to the requested page; Pager still enforces payload
            // limits, continuation offsets and identical truncation semantics.
            output.seen = cursor.offset;
            for row in cached.rows.iter().skip(cursor.offset) {
                output.push(serde_json::from_slice(row)?)?;
                if output.more {
                    break;
                }
            }
            return Ok(Overview {
                revision: encode_cursor(&Cursor {
                    offset: 0,
                    ..cursor.clone()
                })?,
                resolved,
                totals: cached.totals.clone(),
                breakdown: output.finish()?,
            });
        }
        let mut cache = OverviewBuilder::new();
        let mut emit = |item: Breakdown| {
            cache.push(&item)?;
            output.push(item)
        };
        let mut totals = Totals::default();
        let mut titles = BTreeMap::<(String, String), i64>::new();
        let mut applications = BTreeMap::<String, i64>::new();
        let mut bytes = 0;
        self.coverage(&cursor, resolved.range, |coverage| {
            let elapsed = coverage.elapsed_ms.unwrap_or(0);
            match &coverage.focus {
                FocusCoverage::Focus { window, idle } => {
                    totals.focus_ms += elapsed;
                    if *idle {
                        totals.idle_ms += elapsed;
                    }
                    let key = (window.application.clone(), window.title.clone());
                    if !titles.contains_key(&key) {
                        bytes += key.0.len() * 2 + key.1.len() + 128;
                        if bytes > BREAKDOWN_BYTES {
                            bail!(QueryError::LimitExceeded);
                        }
                    }
                    *titles.entry(key).or_default() += elapsed;
                    *applications.entry(window.application.clone()).or_default() += elapsed;
                }
                FocusCoverage::NoFocus { idle } => {
                    totals.no_focus_ms += elapsed;
                    if *idle {
                        totals.idle_ms += elapsed;
                    }
                }
                _ => (),
            }
            Ok(())
        })?;
        let percent = |elapsed| {
            if totals.focus_ms == 0 {
                0.0
            } else {
                elapsed as f64 * 100.0 / totals.focus_ms as f64
            }
        };
        for (application, elapsed_ms) in applications {
            emit(Breakdown::Application {
                application,
                elapsed_ms,
                percent_of_focus: percent(elapsed_ms),
            })?;
        }
        for ((application, title), elapsed_ms) in titles {
            emit(Breakdown::ExactTitle {
                application,
                title,
                elapsed_ms,
                percent_of_focus: percent(elapsed_ms),
            })?;
        }
        if let Some(display) = &mut display {
            self.coverage(&cursor, resolved.range, |coverage| display.focus(coverage))?;
            self.image_coverage(&cursor, resolved.range, None, |coverage| {
                display.image(coverage)
            })?;
        } else {
            self.coverage(&cursor, resolved.range, |coverage| {
                emit(Breakdown::FocusCoverage(coverage))
            })?;
            self.image_coverage(&cursor, resolved.range, None, |coverage| {
                emit(Breakdown::ImageCoverage(coverage))
            })?;
        }
        if let Some(display) = display {
            for row in display.rows() {
                emit(row)?;
            }
        }
        if let Some(rows) = cache.rows {
            remember_overview(CachedOverview {
                key,
                totals: totals.clone(),
                rows,
                bytes: cache.bytes,
            });
        }
        Ok(Overview {
            revision: encode_cursor(&Cursor {
                offset: 0,
                ..cursor.clone()
            })?,
            resolved,
            totals,
            breakdown: output.finish()?,
        })
    }
    fn scan_images(
        &self,
        cursor: &Cursor,
        monitor: Option<&str>,
        mut visit: impl FnMut(Screenshot) -> Result<()>,
    ) -> Result<()> {
        if monitor.is_some_and(|m| m.is_empty() || m.len() > 256) {
            bail!(QueryError::InvalidRequest);
        }
        let mut statement = self.history.connection.prepare("SELECT id,unix_ms,monotonic_ms,segment,monitor,width,height,transform,image,unavailable FROM screenshots WHERE id<=?1 AND (?2 IS NULL OR monitor=?2) ORDER BY segment,monitor,monotonic_ms,id")?;
        let mut rows = statement.query(params![cursor.screenshots, monitor])?;
        while let Some(row) = rows.next()? {
            self.check_cancelled()?;
            visit(screenshot_row(row)?)?;
        }
        Ok(())
    }
    pub fn screenshots(
        &self,
        client: Client,
        range: Range,
        monitor: Option<&str>,
        page: PageRequest,
    ) -> Result<Page<Screenshot>> {
        range.validate(48 * HOUR)?;
        let _permit = self.admit(client)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let cursor = self.cursor(format!("screenshots:{range:?}:{monitor:?}"), &page)?;
        let mut output = Pager::new(cursor.clone(), &page);
        self.scan_images(&cursor, monitor, |image| {
            if range.contains(image.sample.unix_ms) {
                output.push(image)?;
            }
            Ok(())
        })?;
        output.finish()
    }
    fn image_coverage(
        &self,
        cursor: &Cursor,
        range: Range,
        monitor: Option<&str>,
        mut visit: impl FnMut(ImageCoverage) -> Result<()>,
    ) -> Result<()> {
        // Boundaries apply even to older histories without per-monitor gap rows.
        let mut boundaries = BTreeMap::<i64, Vec<i64>>::new();
        let mut endpoints = BTreeMap::<i64, i64>::new();
        let mut context_bytes = 0;
        self.scan(cursor, |observation| {
            if !endpoints.contains_key(&observation.segment) {
                context_bytes += 128;
            }
            if is_boundary(&observation.evidence) {
                context_bytes += 16;
            }
            if context_bytes > BREAKDOWN_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            endpoints
                .entry(observation.segment)
                .and_modify(|end| *end = (*end).max(observation.unix_ms))
                .or_insert(observation.unix_ms);
            if is_boundary(&observation.evidence) {
                boundaries
                    .entry(observation.segment)
                    .or_default()
                    .push(observation.unix_ms);
            }
            Ok(())
        })?;
        for times in boundaries.values_mut() {
            times.sort_unstable();
        }
        let emit = |image: Screenshot,
                    next: Option<&Screenshot>,
                    visit: &mut dyn FnMut(ImageCoverage) -> Result<()>|
         -> Result<()> {
            let sample = &image.sample;
            let mut end = endpoints
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
            if let Some(times) = boundaries.get(&sample.segment) {
                let index = times.partition_point(|time| *time <= sample.unix_ms);
                if let Some(boundary) = times.get(index) {
                    end = end.min(*boundary);
                }
            }
            if let Some(clipped) = range.clip(sample.unix_ms, end) {
                visit(ImageCoverage {
                    range: clipped,
                    segment: sample.segment,
                    screenshot: image.id,
                    monitor: sample.monitor.clone(),
                    unavailable: image
                        .unavailable
                        .clone()
                        .or_else(|| image.image.is_none().then(|| "unavailable".into())),
                })?;
            }
            Ok(())
        };
        let mut previous: Option<Screenshot> = None;
        self.scan_images(cursor, monitor, |next| {
            if let Some(old) = previous.take() {
                let same = old.sample.segment == next.sample.segment
                    && old.sample.monitor == next.sample.monitor;
                emit(old, same.then_some(&next), &mut visit)?;
            }
            previous = Some(next);
            Ok(())
        })?;
        if let Some(old) = previous {
            emit(old, None, &mut visit)?;
        }
        Ok(())
    }
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
        let mut focus = Vec::new();
        let mut response_bytes = 0;
        self.coverage(&cursor, range, |coverage| {
            response_bytes += serde_json::to_vec(&coverage)?.len();
            if focus.len() >= 500 || response_bytes > PAGE_BYTES {
                bail!(QueryError::LimitExceeded);
            }
            focus.push(coverage);
            Ok(())
        })?;
        let mut screenshots = Vec::new();
        self.image_coverage(&cursor, range, monitor, |coverage| {
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
    fn screenshot(&self, id: i64) -> Result<Option<Screenshot>> {
        Ok(self.history.connection.query_row("SELECT id,unix_ms,monotonic_ms,segment,monitor,width,height,transform,image,unavailable FROM screenshots WHERE id=?1", [id], screenshot_row).optional()?)
    }
    pub fn image(&self, client: Client, id: i64) -> Result<RetrievedImage> {
        self.image_format(client, id, crate::tiles::ImageFormat::Original)
    }
    pub fn tiles(&self, client: Client, id: i64) -> Result<RetrievedImage> {
        self.image_format(client, id, crate::tiles::ImageFormat::Bundle)
    }
    pub fn thumbnail(&self, client: Client, id: i64) -> Result<RetrievedImage> {
        self.image_format(client, id, crate::tiles::ImageFormat::Thumbnail)
    }
    fn image_format(
        &self,
        client: Client,
        id: i64,
        format: crate::tiles::ImageFormat,
    ) -> Result<RetrievedImage> {
        if id <= 0 {
            bail!(QueryError::InvalidRequest);
        }
        let permit = self.admit(client)?;
        let lease = self.history.try_image_lease().map_err(|error| {
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::fs::TryLockError>()
                    .is_some_and(|e| matches!(e, std::fs::TryLockError::WouldBlock))
            }) {
                anyhow::Error::from(QueryError::Busy)
            } else {
                error
            }
        })?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let screenshot = self.screenshot(id)?.ok_or(QueryError::Unavailable)?;
        let image = self
            .history
            .image_admitted(id, permit.global, lease, format)
            .map_err(|error| {
                if error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::fs::TryLockError>()
                        .is_some_and(|e| matches!(e, std::fs::TryLockError::WouldBlock))
                }) {
                    anyhow::Error::from(QueryError::Busy)
                } else {
                    error
                }
            })?
            .ok_or(QueryError::Unavailable)?;
        Ok(RetrievedImage {
            screenshot,
            stream: ImageStream {
                image,
                _mcp: permit.mcp,
            },
        })
    }
}

fn screenshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Screenshot> {
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
fn is_boundary(evidence: &Evidence) -> bool {
    !matches!(
        evidence,
        Evidence::State {
            kind: StateChange::Focus | StateChange::Idle | StateChange::Checkpoint,
            ..
        }
    )
}
fn classify(evidence: &Evidence) -> FocusCoverage {
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
            Some(window) => FocusCoverage::Focus {
                window: window.clone(),
                idle: *idle,
            },
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
fn proportional(elapsed: i64, full: Range, clipped: Range) -> i64 {
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
fn emit_block(mut block: Block, range: Range, output: &mut Pager<Block>) -> Result<()> {
    if let Some(clip) = range.clip(block.range.start_ms, block.range.end_ms) {
        block.range = clip;
        output.push(block)?;
    }
    Ok(())
}
struct Pager<T> {
    cursor: Cursor,
    limit: usize,
    seen: usize,
    bytes: usize,
    items: Vec<T>,
    more: bool,
    truncated: bool,
}
impl<T: Serialize> Pager<T> {
    fn new(cursor: Cursor, request: &PageRequest) -> Self {
        Self {
            cursor,
            limit: request.limit.unwrap_or(100),
            seen: 0,
            bytes: 0,
            items: vec![],
            more: false,
            truncated: false,
        }
    }
    fn push(&mut self, item: T) -> Result<()> {
        let index = self.seen;
        self.seen += 1;
        if index < self.cursor.offset {
            return Ok(());
        }
        if self.more || self.items.len() >= self.limit {
            self.more = true;
            return Ok(());
        }
        let size = serde_json::to_vec(&item)?.len();
        if size > PAGE_BYTES {
            bail!(QueryError::LimitExceeded);
        }
        if self.bytes + size > PAGE_BYTES {
            self.more = true;
            self.truncated = true;
            return Ok(());
        }
        self.bytes += size;
        self.items.push(item);
        Ok(())
    }
    fn finish(mut self) -> Result<Page<T>> {
        self.cursor.offset += self.items.len();
        let next_cursor = self.more.then(|| encode_cursor(&self.cursor)).transpose()?;
        Ok(Page {
            items: self.items,
            next_cursor,
            truncated: self.truncated,
        })
    }
}

fn encode_cursor(cursor: &Cursor) -> Result<String> {
    Ok(serde_json::to_vec(cursor)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
fn decode_cursor(token: &str) -> Result<Vec<u8>> {
    if !token.is_ascii() || !token.len().is_multiple_of(2) {
        bail!(QueryError::InvalidRequest);
    }
    token
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|_| QueryError::InvalidRequest)?;
            u8::from_str_radix(text, 16).map_err(|_| QueryError::InvalidRequest.into())
        })
        .collect()
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn superseded_scan_stops_and_releases_its_admission_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path()).unwrap();
        for time in 0..10 {
            history
                .append(time, time, 1, serde_json::json!({"legacy":"unknown"}))
                .unwrap();
        }
        let flag = Arc::new(AtomicBool::new(false));
        let queries = Queries::open(dir.path())
            .unwrap()
            .cancel_when(Some(flag.clone()));
        let cursor = queries
            .cursor("cancellation-test".into(), &PageRequest::default())
            .unwrap();
        let mut visited = 0;
        {
            let _permit = queries.admit(Client::Browser).unwrap();
            let error = queries
                .scan(&cursor, |_| {
                    visited += 1;
                    if visited == 2 {
                        flag.store(true, Ordering::Relaxed);
                    }
                    Ok(())
                })
                .unwrap_err();
            assert_eq!(error.downcast_ref::<QueryError>(), Some(&QueryError::Busy));
        }
        assert_eq!(visited, 2);
        let fresh = Queries::open(dir.path()).unwrap();
        let _browser = fresh.admit(Client::Browser).unwrap();
        let _mcp = fresh.admit(Client::Mcp).unwrap();
    }
}
