//! Explicit offline v3 -> v4 conversion. Original WebP bytes become single-region
//! images in packs. The active database is replaced only after full verification.
use crate::{codec, tiles};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

#[derive(Serialize)]
pub struct MigrationReport {
    pub images: u64,
    pub bytes: u64,
    pub already_converted: bool,
}
fn snapshot_id(db: &Connection) -> Result<String> {
    Ok(db.query_row("SELECT COALESCE((SELECT MAX(id) FROM observations),0)||':'||(SELECT COUNT(*) FROM observations)||':'||COALESCE((SELECT MAX(id) FROM screenshots),0)||':'||(SELECT COUNT(*) FROM screenshots)||':'||COALESCE((SELECT value FROM state WHERE key='retention_epoch'),0)",[],|r|r.get(0))?)
}
fn cleanup(directory: &Path) -> Result<()> {
    // This path belongs exclusively to the obsolete image store. Conversion has
    // verified every referenced byte; there is no runtime legacy fallback.
    if directory.join("images").exists() {
        fs::remove_dir_all(directory.join("images"))?;
    }
    if directory.join("tile-migration").exists() {
        fs::remove_dir_all(directory.join("tile-migration"))?;
    }
    File::open(directory)?.sync_all()?;
    Ok(())
}
pub fn migrate(directory: &Path) -> Result<MigrationReport> {
    ensure!(
        directory.join("history.sqlite3").is_file(),
        "history database missing"
    );
    let writer = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join("recorder.lock"))?;
    writer
        .try_lock()
        .context("stop the recorder before migrating images")?;
    let lease = File::open(directory.join("images.lock"))?;
    lease
        .try_lock()
        .context("image readers active; close them before migration")?;
    let source = Connection::open(directory.join("history.sqlite3"))?;
    source.busy_timeout(Duration::from_secs(5))?;
    let version: i64 = source.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if matches!(version, 4 | 5) {
        crate::tile_map::upgrade(&source)?;
        ensure!(
            directory.join("tile-migration-verified").exists(),
            "history is already packed; no migration cleanup is required"
        );
        let images =
            source.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))? as u64;
        source.execute_batch("DROP TABLE IF EXISTS migration_images;")?;
        drop(source);
        cleanup(directory)?;
        return Ok(MigrationReport {
            images,
            bytes: 0,
            already_converted: true,
        });
    }
    ensure!(version == 3, "migration requires schema 3, found {version}");
    let checkpoint: (i64, i64, i64) =
        source.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    ensure!(
        checkpoint.0 == 0,
        "history checkpoint busy; close all history readers before migration"
    );
    let identity = snapshot_id(&source)?;
    let stage = directory.join("tile-migration");
    fs::create_dir_all(&stage)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
    let staged_db = stage.join("history.sqlite3");
    if !staged_db.exists() {
        let mut marker = File::create(stage.join("source"))?;
        marker.write_all(identity.as_bytes())?;
        marker.sync_all()?;
        let copying = stage.join("history.copying.sqlite3");
        if copying.exists() {
            fs::remove_file(&copying)?;
        }
        source.execute(
            "VACUUM INTO ?1",
            [copying.to_str().context("non UTF-8 history path")?],
        )?;
        File::open(&copying)?.sync_all()?;
        fs::rename(&copying, &staged_db)?;
        File::open(&stage)?.sync_all()?;
    }
    ensure!(
        fs::read_to_string(stage.join("source"))? == identity,
        "source changed since migration began; preserve/remove staging and restart migration"
    );
    if directory.join("tiles").exists() && !stage.join("tiles").exists() {
        fs::rename(directory.join("tiles"), stage.join("tiles"))?;
    }
    let mut db = Connection::open(&staged_db)?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
    let old: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='migration_images')",
        [],
        |r| r.get(0),
    )?;
    if !old {
        db.execute_batch("BEGIN IMMEDIATE; ALTER TABLE images RENAME TO migration_images; DROP INDEX images_by_fingerprint;")?;
        tiles::initialize(&stage, &db)?;
        db.execute_batch("COMMIT;")?;
    }
    crate::tile_map::upgrade(&db)?;
    let mut copied =
        db.query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))? as u64;
    let mut total: u64 = 0;
    loop {
        let pending = {
            let mut stmt=db.prepare("SELECT name,fingerprint FROM migration_images WHERE NOT EXISTS(SELECT 1 FROM images WHERE images.name=migration_images.name) ORDER BY name LIMIT 64")?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if pending.is_empty() {
            break;
        }
        let mut batch = Vec::new();
        let mut rows = Vec::new();
        for (name, fingerprint) in pending {
            ensure!(
                name.starts_with("image-")
                    && name.ends_with(".webp")
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'),
                "invalid legacy image name"
            );
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(directory.join("images").join(&name))
                .with_context(|| format!("missing legacy image {name}"))?;
            ensure!(
                file.metadata()?.len() <= codec::MAX_ENCODED as u64,
                "legacy image exceeds byte limit"
            );
            let mut bytes = Vec::new();
            file.take(codec::MAX_ENCODED as u64 + 1)
                .read_to_end(&mut bytes)?;
            let (width, height) = codec::dimensions(&bytes)?;
            let mismatch:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM screenshots WHERE image=?1 AND (width!=?2 OR height!=?3))",params![name,width,height],|r|r.get(0))?;
            ensure!(!mismatch, "legacy screenshot dimensions mismatch");
            if !batch.is_empty() && batch.len() + bytes.len() > 16 * 1024 * 1024 {
                break;
            }
            let mut hash = blake3::Hasher::new();
            hash.update(b"timeline-encoded-region-v1");
            hash.update(&width.to_le_bytes());
            hash.update(&height.to_le_bytes());
            hash.update(&bytes);
            rows.push((
                name,
                fingerprint,
                width,
                height,
                batch.len() as i64,
                bytes.len() as i64,
                hash.finalize().as_bytes().to_vec(),
                blake3::hash(&bytes).as_bytes().to_vec(),
            ));
            batch.extend_from_slice(&bytes);
        }
        let (pack, mut file) = tiles::new_pack(&stage)?;
        file.write_all(&batch)?;
        file.sync_all()?;
        File::open(stage.join("tiles"))?.sync_all()?;
        let tx = db.transaction()?;
        for (name, fingerprint, width, height, offset, length, key, checksum) in rows {
            tx.execute("INSERT OR IGNORE INTO tiles(fingerprint,pack,offset,length,width,height,checksum) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![key,pack,offset,length,width,height,checksum])?;
            let id: i64 = tx.query_row(
                "UPDATE tiles SET refs=refs+1 WHERE fingerprint=?1 RETURNING id",
                [key],
                |r| r.get(0),
            )?;
            tx.execute("INSERT INTO images(name,fingerprint,width,height,tile_size,map) VALUES (?1,?2,?3,?4,?5,?6)",params![name,fingerprint,width,height,width.max(height),{ let mut map = Vec::new(); crate::tile_map::push(&mut map, id)?; map }])?;
            copied += 1;
        }
        tx.commit()?;
        total += batch.len() as u64;
        if copied % 1000 < 64 {
            eprintln!("migration: {copied} images packed");
        }
    }
    // Validate every retained asset against the original byte checksum before
    // switching the database. This also checks resumed batches after a crash.
    eprintln!("migration: verifying {copied} packed images");
    {
        let mut stmt =
            db.prepare("SELECT pack,offset,length,checksum FROM tiles ORDER BY pack,offset")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)? as u64,
                r.get::<_, u32>(2)? as usize,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut active = String::new();
        let mut file = None;
        for row in rows {
            let (name, offset, length, checksum) = row?;
            if name != active {
                file = Some(File::open(stage.join("tiles").join(&name))?);
                active = name;
            }
            let file = file.as_mut().unwrap();
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes)?;
            ensure!(
                blake3::hash(&bytes).as_bytes().as_slice() == checksum,
                "migrated pack verification failed"
            );
        }
    }
    let missing:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM screenshots WHERE image IS NOT NULL AND NOT EXISTS(SELECT 1 FROM images WHERE name=screenshots.image))",[],|r|r.get(0))?;
    ensure!(!missing, "migration would lose screenshot references");
    let checkpoint: (i64, i64, i64) = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    ensure!(checkpoint.0 == 0, "staging checkpoint busy");
    db.execute_batch("PRAGMA journal_mode=DELETE;")?;
    db.close().map_err(|(_, e)| e)?;
    source.close().map_err(|(_, e)| e)?;
    // A durable old database hard link is a recovery artifact, not a read path.
    let backup = directory.join("history-before-tiles.sqlite3");
    if !backup.exists() {
        fs::hard_link(directory.join("history.sqlite3"), &backup)?;
    }
    File::open(&backup)?.sync_all()?;
    for name in ["history.sqlite3-wal", "history.sqlite3-shm"] {
        let path = directory.join(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    ensure!(
        !directory.join("tiles").exists(),
        "destination tile directory already exists"
    );
    fs::rename(stage.join("tiles"), directory.join("tiles"))?;
    File::open(&stage)?.sync_all()?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join("tile-append.lock"))?
        .sync_all()?;
    let mut marker = File::create(directory.join("tile-migration-verified"))?;
    marker.write_all(b"v3 images verified; safe to finish obsolete-file cleanup\n")?;
    marker.sync_all()?;
    File::open(directory)?.sync_all()?;
    fs::rename(&staged_db, directory.join("history.sqlite3"))?;
    File::open(&stage)?.sync_all()?;
    File::open(directory)?.sync_all()?;
    let final_db = Connection::open(directory.join("history.sqlite3"))?;
    final_db.execute_batch("DROP TABLE IF EXISTS migration_images;")?;
    final_db.close().map_err(|(_, e)| e)?;
    cleanup(directory)?;
    Ok(MigrationReport {
        images: copied,
        bytes: total,
        already_converted: false,
    })
}
