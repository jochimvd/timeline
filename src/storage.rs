//! On-demand aggregate storage snapshot. Reads only; no tracking database or files.
use crate::history::History;
use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, path::Path};
pub(crate) fn snapshot(directory: &Path) -> Result<Value> {
    let history = History::read(directory)?;
    let db = &history.connection;
    let _snapshot = db.unchecked_transaction()?;
    let (images, maps): (i64, i64) = db.query_row(
        "SELECT COUNT(*),COALESCE(SUM(length(map)),0) FROM images",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (regions, payload, references, reused_total, unique_total): (i64,i64,i64,i64,i64) = db.query_row(
        "SELECT COUNT(*),COALESCE(SUM(length),0),COALESCE(SUM(refs),0),COALESCE(SUM(CASE WHEN width<=256 AND height<=256 THEN length*refs ELSE 0 END),0),COALESCE(SUM(CASE WHEN width<=256 AND height<=256 THEN length ELSE 0 END),0) FROM tiles WHERE refs>0",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let captures: i64 = db.query_row(
        "SELECT COUNT(*) FROM screenshots WHERE image IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let mut packs = 0u64;
    let mut pack_count = 0u64;
    for entry in fs::read_dir(directory.join("tiles"))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("pack-") && name.ends_with(".bin") {
            // Reclamation can remove a pack after the directory entry was read.
            match entry.metadata() {
                Ok(m) => {
                    packs += m.len();
                    pack_count += 1;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
    }
    let mut database = 0u64;
    for name in [
        "history.sqlite3",
        "history.sqlite3-wal",
        "history.sqlite3-shm",
    ] {
        match fs::metadata(directory.join(name)) {
            Ok(m) => database += m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(json!({
        "total_bytes": packs + database, "pack_bytes": packs, "pack_count": pack_count,
        "database_bytes": database, "payload_bytes": payload, "map_bytes": maps,
        "map_uncompressed_bytes": references * 8, "images": images, "regions": regions,
        "captures": captures, "tile_reuse_percent": if reused_total > 0 { Some((1.0 - unique_total as f64 / reused_total as f64) * 100.0) } else { None },
        "thumbnail_cache_bytes": crate::thumbnails::usage(), "thumbnail_cache_limit": 16 * 1024 * 1024,
        "retention_days": crate::config::RETENTION_MS / 86_400_000,
        "measured_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64
    }))
}
