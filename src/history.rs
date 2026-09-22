use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    time::Duration,
};

const MAX_EVIDENCE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: i64,
    pub unix_ms: i64,
    pub elapsed_ms: i64,
    pub segment: i64,
    pub evidence: serde_json::Value,
}

pub struct History {
    connection: Connection,
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

impl History {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let metadata = fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() },
            "history must be an owned directory, not a symlink"
        );
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let writer_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join("recorder.lock"))?;
        writer_lock
            .try_lock()
            .context("another recorder owns this history directory")?;
        let connection = Connection::open(directory.join("history.sqlite3"))?;
        connection.busy_timeout(Duration::from_millis(500))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(version <= 2, "history schema is newer than this recorder");
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
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
            CREATE TABLE IF NOT EXISTS retention_clock (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);",
        )?;
        if version == 1 {
            connection.execute_batch(
                "BEGIN IMMEDIATE;
                ALTER TABLE observations ADD COLUMN retention_ms INTEGER NOT NULL DEFAULT 0;
                UPDATE observations SET retention_ms=unix_ms;
                PRAGMA user_version=2;
                COMMIT;",
            )?;
        } else {
            connection.execute_batch("PRAGMA user_version=2;")?;
        }
        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS observations_by_retention ON observations(retention_ms);",
        )?;
        let clock: Option<String> = connection
            .query_row("SELECT value FROM retention_clock WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()?;
        let retention_clock = clock
            .map(|clock| serde_json::from_str(&clock))
            .transpose()?;
        fs::set_permissions(
            directory.join("history.sqlite3"),
            fs::Permissions::from_mode(0o600),
        )?;
        Ok(Self {
            connection,
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
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        ensure!(version == 2, "unsupported history schema version {version}");
        Ok(Self {
            connection,
            _writer_lock: None,
            retention_clock: None,
        })
    }

    pub fn cleanup_before(&mut self, cutoff_retention_ms: i64) -> Result<usize> {
        Ok(self.connection.execute(
            "DELETE FROM observations WHERE retention_ms < ?1",
            [cutoff_retention_ms],
        )?)
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
        elapsed_ms: i64,
        segment: i64,
        evidence: serde_json::Value,
    ) -> Result<i64> {
        self.commit(unix_ms, elapsed_ms, segment, evidence, None)
    }

    /// Publish evidence and an optional manual-pause change as one durable commit.
    pub fn commit(
        &mut self,
        unix_ms: i64,
        elapsed_ms: i64,
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
        transaction.execute(
            "INSERT INTO observations(unix_ms, elapsed_ms, segment, evidence, retention_ms) VALUES (?1,?2,?3,?4,?5)",
            params![unix_ms, elapsed_ms, segment, evidence, self.retention_clock.as_ref().map(|clock| clock.age_ms).unwrap_or(unix_ms)],
        )?;
        let id = transaction.last_insert_rowid();
        if let Some(paused) = paused {
            transaction.execute(
                "UPDATE state SET value=?1 WHERE key='manual_paused'",
                [paused],
            )?;
        }
        if let Some(clock) = &self.retention_clock {
            transaction.execute("INSERT INTO retention_clock VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value", [serde_json::to_string(clock)?])?;
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
        let id: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM observations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => Ok(self.observations(id - 1, 1)?.pop()),
            None => Ok(None),
        }
    }

    pub fn observations(&self, after: i64, limit: usize) -> Result<Vec<Observation>> {
        ensure!(
            (1..=500).contains(&limit),
            "page size must be between 1 and 500"
        );
        ensure!(after >= 0, "observation cursor must be nonnegative");
        let mut statement = self.connection.prepare(
            "SELECT id, unix_ms, elapsed_ms, segment, evidence FROM observations WHERE id > ?1 ORDER BY id LIMIT ?2"
        )?;
        let rows = statement.query_map(params![after, limit as i64], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut observations = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let (id, unix_ms, elapsed_ms, segment, evidence) = row?;
            ensure!(
                evidence.len() <= MAX_EVIDENCE_BYTES,
                "stored observation exceeds evidence byte limit"
            );
            if bytes + evidence.len() > MAX_EVIDENCE_BYTES {
                break;
            }
            bytes += evidence.len();
            observations.push(Observation {
                id,
                unix_ms,
                elapsed_ms,
                segment,
                evidence: serde_json::from_str(&evidence)?,
            });
        }
        Ok(observations)
    }
}
