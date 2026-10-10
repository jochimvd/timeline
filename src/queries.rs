//! Shared, bounded, read-only evidence service for browser and MCP adapters.
//! Each call reads one SQLite snapshot. Cursors pin append watermarks and are
//! invalidated by retention; image leases keep admitted streams alive after expiry.
//! Scans read only the timing segments that can intersect the requested range.
use crate::{
    evidence::{Evidence, FocusState, StateChange},
    history::{History, ImageRead, Screenshot, ScreenshotSample},
    recording::Window,
    settings::Settings,
};
use anyhow::{Result, bail};
use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

mod coverage;
mod display;
mod images;
mod navigation;
mod overview;
mod paging;
mod scan;
mod segments;
mod updates;
use coverage::is_boundary;
pub use display::{DisplayFocus, DisplayImage};
use images::ImageContext;
pub use navigation::ActivityBounds;
use overview::cached_overview;
use paging::{Cursor, Pager, decode_cursor, encode_cursor};
use scan::ScanObservation;
pub(crate) use segments::initialize;
use segments::{Plan, Run, Scope};
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
pub struct Observation {
    pub id: i64,
    pub unix_ms: i64,
    pub monotonic_ms: i64,
    pub segment: i64,
    pub evidence: Evidence,
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
    /// Category of every focus interval in the block; null when uncategorized.
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FocusCoverage {
    Focus {
        window: Window,
        idle: bool,
        /// Configured category of the cleaned window; null when uncategorized.
        #[serde(default)]
        category: Option<String>,
    },
    NoFocus {
        idle: bool,
    },
    Gap {
        reason: String,
    },
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
    /// Every configured category in configuration order, then uncategorized
    /// focus (`category: null`) when there is any.
    Category {
        category: Option<String>,
        /// Explicit `#rrggbb` color, else a 1-based categorical palette slot.
        color: Option<String>,
        palette: Option<u8>,
        elapsed_ms: i64,
        percent_of_focus: f64,
    },
    Application {
        application: String,
        elapsed_ms: i64,
        percent_of_focus: f64,
    },
    ExactTitle {
        application: String,
        title: String,
        category: Option<String>,
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
pub struct CategoryList {
    pub categories: Vec<crate::categories::Category>,
    pub problem: Option<String>,
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
    settings: Arc<Settings>,
    cancelled: Option<Arc<AtomicBool>>,
}
impl Queries {
    pub fn open(directory: &Path) -> Result<Self> {
        Self::with_policy(
            directory,
            crate::config::activity_timezone(),
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
            settings: Default::default(),
            cancelled: None,
        })
    }
    /// Title rules clean focus titles and categories classify focus in
    /// coverage, blocks and overviews; raw observations stay as recorded.
    pub fn with_settings(mut self, settings: Arc<Settings>) -> Self {
        self.settings = settings;
        self
    }
    /// Versions block IDs by grouping and title rules, which decide block extents.
    fn block_version(&self) -> String {
        match self.settings.titles.version() {
            "" => self.grouping.version().into(),
            titles => format!("{}+titles-{titles}", self.grouping.version()),
        }
    }
    /// Versions analysis cursors and cached overviews by every analysis setting.
    fn analysis_version(&self) -> String {
        match self.settings.categories.version() {
            "" => self.block_version(),
            categories => format!("{}+categories-{categories}", self.block_version()),
        }
    }
    /// Configured categories and any problem that made the settings file ignored.
    pub fn categories(&self) -> CategoryList {
        CategoryList {
            categories: self.settings.categories.list().to_vec(),
            problem: self.settings.problem.clone(),
        }
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
        self.observations_in(&cursor, range, |observation| {
            output.push(Observation::from(observation))
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
    pub fn image(&self, client: Client, id: i64) -> Result<RetrievedImage> {
        self.image_format(client, id, crate::tiles::ImageFormat::Original)
    }
    pub fn tiles(&self, client: Client, id: i64) -> Result<RetrievedImage> {
        self.image_format(client, id, crate::tiles::ImageFormat::Bundle)
    }
    pub fn thumbnail(&self, client: Client, id: i64) -> Result<RetrievedImage> {
        self.image_format(client, id, crate::tiles::ImageFormat::Thumbnail)
    }
    /// A lossy rendition at most `max_edge` (64–4096) pixels on its longest edge,
    /// for clients that cannot use native resolution. The metadata stays original.
    pub fn preview(&self, client: Client, id: i64, max_edge: u32) -> Result<RetrievedImage> {
        if !(64..=4096).contains(&max_edge) {
            bail!(QueryError::InvalidRequest);
        }
        self.image_format(client, id, crate::tiles::ImageFormat::Preview(max_edge))
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
        let lease = self.history.try_image_lease().map_err(lock_busy)?;
        let _snapshot = self.history.connection.unchecked_transaction()?;
        let screenshot = self.screenshot(id)?.ok_or(QueryError::Unavailable)?;
        let image = self
            .history
            .image_admitted(id, permit.global, lease, format)
            .map_err(lock_busy)?
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

/// A contended image lease means busy, not failure.
fn lock_busy(error: anyhow::Error) -> anyhow::Error {
    let contended = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::fs::TryLockError>()
            .is_some_and(|e| matches!(e, std::fs::TryLockError::WouldBlock))
    });
    if contended {
        QueryError::Busy.into()
    } else {
        error
    }
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
        let range = Range {
            start_ms: 0,
            end_ms: 10,
        };
        let plan = queries.plan(&cursor, range, Scope::Evidence).unwrap();
        let mut visited = 0;
        {
            let _permit = queries.admit(Client::Browser).unwrap();
            let error = queries
                .scan(&cursor, &plan, |_, _| {
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
