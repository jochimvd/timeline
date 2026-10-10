//! On-demand aggregate storage snapshot. Reads only; no tracking database or files.
use crate::history::History;
use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const DATABASE_FILES: [&str; 3] = [
    "history.sqlite3",
    "history.sqlite3-wal",
    "history.sqlite3-shm",
];

pub(crate) fn snapshot(directory: &Path) -> Result<Value> {
    let history = History::read(directory)?;
    let db = &history.connection;
    let _snapshot = db.unchecked_transaction()?;
    let images = image_totals(db)?;
    let tiles = tile_totals(db)?;
    let captures: i64 = db.query_row(
        "SELECT COUNT(*) FROM screenshots WHERE image IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let (packs, pack_count) = pack_usage(&directory.join("tiles"))?;
    let database = database_bytes(directory)?;
    Ok(json!({
        "total_bytes": packs + database,
        "pack_bytes": packs,
        "pack_count": pack_count,
        "database_bytes": database,
        "payload_bytes": tiles.payload,
        "map_bytes": images.map_bytes,
        "map_uncompressed_bytes": tiles.references * 8,
        "images": images.count,
        "regions": tiles.count,
        "captures": captures,
        "tile_reuse_percent": tiles.reuse_percent(),
        "thumbnail_cache_bytes": crate::thumbnails::usage(),
        "thumbnail_cache_limit": crate::thumbnails::BYTE_LIMIT,
        "metadata_retention_days": crate::config::METADATA_RETENTION_MS / 86_400_000,
        "screenshot_retention_days": crate::config::SCREENSHOT_RETENTION_MS / 86_400_000,
        "measured_ms": SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64
    }))
}

struct ImageTotals {
    count: i64,
    map_bytes: i64,
}

fn image_totals(db: &Connection) -> Result<ImageTotals> {
    Ok(db.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(map)), 0) FROM images",
        [],
        |r| {
            Ok(ImageTotals {
                count: r.get(0)?,
                map_bytes: r.get(1)?,
            })
        },
    )?)
}

struct TileTotals {
    count: i64,
    payload: i64,
    references: i64,
    /// Bytes of grid-sized tiles as if every reference stored its own copy.
    referenced_bytes: i64,
    /// Bytes of grid-sized tiles as actually stored once.
    unique_bytes: i64,
}

impl TileTotals {
    fn reuse_percent(&self) -> Option<f64> {
        (self.referenced_bytes > 0)
            .then(|| (1.0 - self.unique_bytes as f64 / self.referenced_bytes as f64) * 100.0)
    }
}

fn tile_totals(db: &Connection) -> Result<TileTotals> {
    Ok(db.query_row(
        "SELECT
            COUNT(*),
            COALESCE(SUM(length), 0),
            COALESCE(SUM(refs), 0),
            COALESCE(SUM(CASE WHEN width<=256 AND height<=256 THEN length*refs ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN width<=256 AND height<=256 THEN length ELSE 0 END), 0)
        FROM tiles WHERE refs>0",
        [],
        |r| {
            Ok(TileTotals {
                count: r.get(0)?,
                payload: r.get(1)?,
                references: r.get(2)?,
                referenced_bytes: r.get(3)?,
                unique_bytes: r.get(4)?,
            })
        },
    )?)
}

/// Total bytes and number of pack files.
fn pack_usage(tiles: &Path) -> Result<(u64, u64)> {
    let mut bytes = 0;
    let mut count = 0;
    for entry in fs::read_dir(tiles)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("pack-") && name.ends_with(".bin")) {
            continue;
        }
        // Reclamation can remove a pack after the directory entry was read.
        if let Some(length) = present(entry.metadata())? {
            bytes += length;
            count += 1;
        }
    }
    Ok((bytes, count))
}

fn database_bytes(directory: &Path) -> Result<u64> {
    let mut bytes = 0;
    for name in DATABASE_FILES {
        bytes += present(fs::metadata(directory.join(name)))?.unwrap_or(0);
    }
    Ok(bytes)
}

/// File length, or `None` for a file that does not (or no longer) exist.
fn present(metadata: io::Result<fs::Metadata>) -> Result<Option<u64>> {
    match metadata {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
