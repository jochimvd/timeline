use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

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
}

impl History {
    pub fn open(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        let connection = Connection::open(directory.join("history.sqlite3"))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                unix_ms INTEGER NOT NULL,
                elapsed_ms INTEGER NOT NULL,
                segment INTEGER NOT NULL,
                evidence TEXT NOT NULL
            );")?;
        Ok(Self { connection })
    }

    pub fn append(&mut self, unix_ms: i64, elapsed_ms: i64, segment: i64, evidence: serde_json::Value) -> Result<i64> {
        self.connection.execute(
            "INSERT INTO observations(unix_ms, elapsed_ms, segment, evidence) VALUES (?1,?2,?3,?4)",
            params![unix_ms, elapsed_ms, segment, evidence.to_string()],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn observations(&self, after: i64, limit: usize) -> Result<Vec<Observation>> {
        ensure!((1..=500).contains(&limit), "page size must be between 1 and 500");
        ensure!(after >= 0, "observation cursor must be nonnegative");
        let mut statement = self.connection.prepare(
            "SELECT id, unix_ms, elapsed_ms, segment, evidence FROM observations WHERE id > ?1 ORDER BY id LIMIT ?2"
        )?;
        let rows = statement.query_map(params![after, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get::<_, String>(4)?))
        })?;
        rows.map(|row| {
            let (id, unix_ms, elapsed_ms, segment, evidence) = row?;
            Ok(Observation { id, unix_ms, elapsed_ms, segment, evidence: serde_json::from_str(&evidence)? })
        }).collect()
    }
}
