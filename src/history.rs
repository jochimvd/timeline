use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_EVIDENCE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub unix_ms: i64,
    /// Absolute CLOCK_MONOTONIC reading, not an observation duration.
    pub monotonic_ms: i64,
    pub segment: i64,
    pub evidence: serde_json::Value,
}

pub struct History {
    pub(crate) connection: Connection,
    pub(crate) directory: PathBuf,
    cleanup: Option<crate::image_cleanup::ImageCleanup>,
    _writer_lock: Option<File>,
    retention_clock: Option<RetentionClock>,
}

#[derive(Serialize, Deserialize)]
struct RetentionClock {
    boot_id: String,
    boot_ms: i64,
    wall_ms: i64,
    age_ms: i64,
}

const SCHEMA: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA synchronous=FULL;
    CREATE TABLE IF NOT EXISTS observations (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        unix_ms INTEGER NOT NULL,
        elapsed_ms INTEGER NOT NULL,
        segment INTEGER NOT NULL,
        evidence TEXT NOT NULL,
        retention_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS state (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
    INSERT OR IGNORE INTO state VALUES ('manual_paused', 0), ('segment', 0);
    CREATE INDEX IF NOT EXISTS observations_by_time ON observations(unix_ms);
    CREATE TABLE IF NOT EXISTS retention_clock (
        id INTEGER PRIMARY KEY CHECK(id=1),
        value TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS observations_by_retention ON observations(retention_ms);
";

/// Create or reopen an owner-only lock file without truncating it.
pub(crate) fn lock_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

/// Exclusive writer ownership of a history directory, held while `File` lives.
pub(crate) fn lock_writer(directory: &Path, owned: &'static str) -> Result<File> {
    let lock = lock_file(&directory.join("recorder.lock"))?;
    lock.try_lock().context(owned)?;
    Ok(lock)
}

fn prepare_directory(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() },
        "history must be an owned directory, not a symlink"
    );
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn load_retention_clock(connection: &Connection) -> Result<Option<RetentionClock>> {
    let clock: Option<String> = connection
        .query_row("SELECT value FROM retention_clock WHERE id=1", [], |row| {
            row.get(0)
        })
        .optional()?;
    Ok(clock
        .map(|clock| serde_json::from_str(&clock))
        .transpose()?)
}

/// Persisted in the caller's transaction, alongside the evidence it dated.
fn save_retention_clock(connection: &Connection, clock: &RetentionClock) -> Result<()> {
    connection.execute(
        "INSERT INTO retention_clock VALUES (1, ?1)
        ON CONFLICT(id) DO UPDATE SET value=excluded.value",
        [serde_json::to_string(clock)?],
    )?;
    Ok(())
}

/// Retention age for new evidence; wall time until a clock is established.
fn retention_ms(clock: Option<&RetentionClock>, unix_ms: i64) -> i64 {
    clock.map_or(unix_ms, |clock| clock.age_ms)
}

/// Columns read by `StoredObservation::read`, in order.
const OBSERVATION_COLUMNS: &str = "id, unix_ms, elapsed_ms, segment, evidence";

struct StoredObservation {
    id: i64,
    unix_ms: i64,
    monotonic_ms: i64,
    segment: i64,
    evidence: String,
}
impl StoredObservation {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            unix_ms: row.get(1)?,
            monotonic_ms: row.get(2)?,
            segment: row.get(3)?,
            evidence: row.get(4)?,
        })
    }
    fn parse(self) -> Result<Observation> {
        ensure!(
            self.evidence.len() <= MAX_EVIDENCE_BYTES,
            "stored observation exceeds evidence byte limit"
        );
        Ok(Observation {
            id: self.id,
            unix_ms: self.unix_ms,
            monotonic_ms: self.monotonic_ms,
            segment: self.segment,
            evidence: serde_json::from_str(&self.evidence)?,
        })
    }
}

impl History {
    pub fn open(directory: &Path) -> Result<Self> {
        prepare_directory(directory)?;
        let writer_lock = lock_writer(directory, "another recorder owns this history directory")?;
        let connection = Connection::open(directory.join("history.sqlite3"))?;
        connection.busy_timeout(Duration::from_millis(500))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(
            matches!(version, 0 | 4 | 5),
            "unsupported history schema {version}; run timeline migrate-images while recorder is stopped"
        );
        connection.execute_batch(SCHEMA)?;
        let retention_clock = load_retention_clock(&connection)?;
        fs::set_permissions(
            directory.join("history.sqlite3"),
            fs::Permissions::from_mode(0o600),
        )?;
        crate::tile_map::upgrade(&connection)?;
        crate::tiles::initialize(directory, &connection)?;
        Ok(Self {
            connection,
            directory: directory.to_owned(),
            cleanup: None,
            _writer_lock: Some(writer_lock),
            retention_clock,
        })
    }

    pub fn read(directory: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            directory.join("history.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        connection.busy_timeout(Duration::from_millis(500))?;
        connection.execute_batch("PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(version == 5, "unsupported history schema version {version}");
        Ok(Self {
            connection,
            directory: directory.to_owned(),
            cleanup: None,
            _writer_lock: None,
            retention_clock: None,
        })
    }

    pub fn cleanup_before(&mut self, cutoff_retention_ms: i64) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        let count = transaction.execute(
            "DELETE FROM observations WHERE retention_ms < ?1",
            [cutoff_retention_ms],
        )?;
        // Only images touched by this expiry can lose their final reference.
        // Avoid a full retained-image table scan on the metadata event thread.
        crate::tiles::expire_images(&transaction, cutoff_retention_ms)?;
        let screenshot_count = transaction.execute(
            "DELETE FROM screenshots WHERE retention_ms < ?1",
            [cutoff_retention_ms],
        )?;
        if count > 0 || screenshot_count > 0 {
            transaction.execute(
                "INSERT INTO state VALUES ('retention_epoch', 1)
                ON CONFLICT(key) DO UPDATE SET value=value+1",
                [],
            )?;
        }
        transaction.commit()?;
        if let Some(cleanup) = &mut self.cleanup {
            cleanup.request()?;
        } else {
            self.reclaim_images(&std::sync::atomic::AtomicBool::new(false))?;
        }
        Ok(count)
    }

    /// Track elapsed age using BOOTTIME (including suspend) across recorder
    /// restarts. Wall time is only a fallback across an actual machine reboot.
    /// Clock state is committed atomically with the next evidence record.
    pub fn advance_retention_clock(&mut self, boot_id: &str, boot_ms: i64, wall_ms: i64) -> i64 {
        let age_ms = match &self.retention_clock {
            Some(previous) => {
                let elapsed = if previous.boot_id == boot_id {
                    boot_ms.saturating_sub(previous.boot_ms)
                } else {
                    wall_ms.saturating_sub(previous.wall_ms)
                };
                previous.age_ms.saturating_add(elapsed.max(0))
            }
            None => wall_ms,
        };
        self.retention_clock = Some(RetentionClock {
            boot_id: boot_id.into(),
            boot_ms,
            wall_ms,
            age_ms,
        });
        age_ms
    }

    pub fn append(
        &mut self,
        unix_ms: i64,
        monotonic_ms: i64,
        segment: i64,
        evidence: serde_json::Value,
    ) -> Result<i64> {
        self.commit(unix_ms, monotonic_ms, segment, evidence, None)
    }

    /// Publish evidence and an optional manual-pause change as one durable commit.
    pub fn commit(
        &mut self,
        unix_ms: i64,
        monotonic_ms: i64,
        segment: i64,
        evidence: serde_json::Value,
        paused: Option<bool>,
    ) -> Result<i64> {
        let evidence = evidence.to_string();
        ensure!(
            evidence.len() <= MAX_EVIDENCE_BYTES,
            "observation exceeds evidence byte limit"
        );
        let transaction = self.connection.transaction()?;
        let retention_ms = retention_ms(self.retention_clock.as_ref(), unix_ms);
        transaction.execute(
            "INSERT INTO observations(unix_ms, elapsed_ms, segment, evidence, retention_ms)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            params![unix_ms, monotonic_ms, segment, evidence, retention_ms],
        )?;
        let id = transaction.last_insert_rowid();
        if let Some(paused) = paused {
            transaction.execute(
                "UPDATE state SET value=?1 WHERE key='manual_paused'",
                [paused],
            )?;
        }
        if let Some(clock) = &self.retention_clock {
            save_retention_clock(&transaction, clock)?;
        }
        transaction.commit()?;
        Ok(id)
    }

    pub fn manual_paused(&self) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT value FROM state WHERE key='manual_paused'",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn start_segment(&mut self) -> Result<i64> {
        Ok(self.connection.query_row(
            "UPDATE state SET value=value+1 WHERE key='segment' RETURNING value",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn latest(&self) -> Result<Option<Observation>> {
        self.connection
            .query_row(
                &format!("SELECT {OBSERVATION_COLUMNS} FROM observations ORDER BY id DESC LIMIT 1"),
                [],
                StoredObservation::read,
            )
            .optional()?
            .map(StoredObservation::parse)
            .transpose()
    }

    pub fn observations(&self, after: i64, limit: usize) -> Result<Vec<Observation>> {
        ensure!(
            (1..=500).contains(&limit),
            "page size must be between 1 and 500"
        );
        ensure!(after >= 0, "observation cursor must be nonnegative");
        let mut statement = self.connection.prepare(&format!(
            "SELECT {OBSERVATION_COLUMNS} FROM observations WHERE id > ?1 ORDER BY id LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![after, limit as i64], StoredObservation::read)?;
        let mut observations = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let observation = row?;
            let length = observation.evidence.len();
            // An oversized row fails in `parse` rather than ending the page.
            if length <= MAX_EVIDENCE_BYTES && bytes + length > MAX_EVIDENCE_BYTES {
                break;
            }
            observations.push(observation.parse()?);
            bytes += length;
        }
        Ok(observations)
    }
}

pub use crate::tiles::{ImageRead, PreparedImage};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotSample {
    pub unix_ms: i64,
    pub monotonic_ms: i64,
    pub segment: i64,
    pub monitor: String,
    pub width: u32,
    pub height: u32,
    /// Wayland output transform; pixels retain the native buffer orientation.
    pub transform: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    pub id: i64,
    #[serde(flatten)]
    pub sample: ScreenshotSample,
    pub image: Option<String>,
    pub unavailable: Option<String>,
}

impl History {
    pub fn commit_screenshot(
        &mut self,
        sample: &ScreenshotSample,
        fingerprint: &str,
        image: &PreparedImage,
    ) -> Result<i64> {
        ensure!(
            self.directory == image.directory,
            "image belongs to another history"
        );
        let tx = self.connection.transaction()?;
        let name = image.publish(&tx, sample, fingerprint)?;
        let id = Self::insert_sample(
            &tx,
            sample,
            Some(&name),
            None,
            self.retention_clock.as_ref(),
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn find_image(&self, fingerprint: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT name FROM images WHERE fingerprint=?1 LIMIT 1",
                [fingerprint],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn reuse_screenshot(
        &mut self,
        sample: &ScreenshotSample,
        image: &str,
    ) -> Result<Option<i64>> {
        let _lease = Self::image_lease(&self.directory)?;
        let dimensions: Option<(u32, u32)> = self
            .connection
            .query_row(
                "SELECT width, height FROM images WHERE name=?1",
                [image],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some(dimensions) = dimensions else {
            return Ok(None);
        };
        ensure!(
            dimensions == (sample.width, sample.height),
            "reused image dimensions mismatch"
        );
        Ok(Some(self.insert_screenshot(sample, Some(image), None)?))
    }

    pub fn screenshot_gap(&mut self, sample: &ScreenshotSample, reason: &str) -> Result<i64> {
        self.insert_screenshot(sample, None, Some(reason))
    }

    fn insert_screenshot(
        &mut self,
        sample: &ScreenshotSample,
        image: Option<&str>,
        reason: Option<&str>,
    ) -> Result<i64> {
        let tx = self.connection.transaction()?;
        let id = Self::insert_sample(&tx, sample, image, reason, self.retention_clock.as_ref())?;
        tx.commit()?;
        Ok(id)
    }

    fn insert_sample(
        tx: &rusqlite::Transaction<'_>,
        sample: &ScreenshotSample,
        image: Option<&str>,
        reason: Option<&str>,
        clock: Option<&RetentionClock>,
    ) -> Result<i64> {
        ensure!(
            sample.monitor.len() <= 256 && reason.is_none_or(|v| v.len() <= 1024),
            "screenshot evidence exceeds limits"
        );
        tx.execute(
            "INSERT INTO screenshots(
                unix_ms, monotonic_ms, segment, monitor, width, height, transform,
                image, unavailable, retention_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                sample.unix_ms,
                sample.monotonic_ms,
                sample.segment,
                sample.monitor,
                sample.width,
                sample.height,
                sample.transform,
                image,
                reason,
                retention_ms(clock, sample.unix_ms),
            ],
        )?;
        let id = tx.last_insert_rowid();
        if let Some(clock) = clock {
            save_retention_clock(tx, clock)?;
        }
        Ok(id)
    }

    pub fn screenshots(&self, after: i64, limit: usize) -> Result<Vec<Screenshot>> {
        ensure!(
            after >= 0 && (1..=500).contains(&limit),
            "invalid screenshot cursor or page size (1–500)"
        );
        let mut stmt = self.connection.prepare(
            "SELECT id, unix_ms, monotonic_ms, segment, monitor, width, height, transform,
                image, unavailable
            FROM screenshots WHERE id > ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after, limit as i64], |row| {
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
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn enable_background_cleanup(&mut self) -> Result<()> {
        if self.cleanup.is_none() {
            let mut cleanup = crate::image_cleanup::ImageCleanup::start(self.directory.clone())?;
            // Startup cleanup may have deferred files for another process's
            // active read. Finish them promptly when its lease is released.
            cleanup.request()?;
            self.cleanup = Some(cleanup);
        }
        Ok(())
    }

    pub fn cleanup_pending(&mut self) -> Result<bool> {
        self.cleanup
            .as_mut()
            .map(|cleanup| cleanup.pending())
            .unwrap_or(Ok(false))
    }

    pub fn screenshot_pressure(&self) -> Result<bool> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM state WHERE key='screenshot_pressure'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_screenshot_pressure(&mut self, pressure: bool) -> Result<()> {
        self.connection.execute(
            "INSERT INTO state VALUES ('screenshot_pressure', ?1)
            ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [pressure],
        )?;
        Ok(())
    }

    pub fn available_bytes(directory: &Path) -> Result<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(directory.as_os_str().as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stat = unsafe { stat.assume_init() };
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn latest_is_the_newest_observation() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = History::open(directory.path()).unwrap();
        assert!(history.latest().unwrap().is_none());
        history.append(1_000, 10, 1, json!({"n": 1})).unwrap();
        let id = history.append(2_000, 20, 2, json!({"n": 2})).unwrap();
        let latest = history.latest().unwrap().unwrap();
        assert_eq!(
            (
                latest.id,
                latest.unix_ms,
                latest.monotonic_ms,
                latest.segment
            ),
            (id, 2_000, 20, 2)
        );
        assert_eq!(latest.evidence, json!({"n": 2}));
    }

    #[test]
    fn reusing_an_unknown_image_records_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let mut history = History::open(directory.path()).unwrap();
        let sample = ScreenshotSample {
            unix_ms: 1_000,
            monotonic_ms: 10,
            segment: 1,
            monitor: "DP-1".into(),
            width: 2,
            height: 1,
            transform: 0,
        };
        assert!(
            history
                .reuse_screenshot(&sample, "missing")
                .unwrap()
                .is_none()
        );
        assert!(history.screenshots(0, 10).unwrap().is_empty());
    }
}
