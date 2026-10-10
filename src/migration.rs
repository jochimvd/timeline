//! Explicit offline v3 -> v4 conversion. Original WebP bytes become single-region
//! images in packs. The active database is replaced only after full verification.
use crate::{codec, history, tiles};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

/// Legacy images converted per pack and transaction.
const BATCH_IMAGES: i64 = 64;
const BATCH_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
pub struct MigrationReport {
    pub images: u64,
    pub bytes: u64,
    pub already_converted: bool,
}

pub fn migrate(directory: &Path) -> Result<MigrationReport> {
    ensure!(
        directory.join("history.sqlite3").is_file(),
        "history database missing"
    );
    let _writer = history::lock_writer(directory, "stop the recorder before migrating images")?;
    let lease = File::open(directory.join("images.lock"))?;
    lease
        .try_lock()
        .context("image readers active; close them before migration")?;
    let source = open_database(&directory.join("history.sqlite3"))?;
    let version: i64 = source.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if matches!(version, 4 | 5) {
        return finish_cleanup(directory, source);
    }
    ensure!(version == 3, "migration requires schema 3, found {version}");
    ensure!(
        checkpoint(&source)?,
        "history checkpoint busy; close all history readers before migration"
    );
    let stage = directory.join("tile-migration");
    let staged_db = stage_copy(directory, &stage, &source)?;
    let mut db = open_staged(&stage, &staged_db)?;
    let (copied, total) = pack_images(directory, &stage, &mut db)?;
    // Validate every retained asset against the original byte checksum before
    // switching the database. This also checks resumed batches after a crash.
    eprintln!("migration: verifying {copied} packed images");
    verify_packs(&stage, &db)?;
    ensure!(
        !lost_references(&db)?,
        "migration would lose screenshot references"
    );
    ensure!(checkpoint(&db)?, "staging checkpoint busy");
    db.execute_batch("PRAGMA journal_mode=DELETE;")?;
    db.close().map_err(|(_, e)| e)?;
    source.close().map_err(|(_, e)| e)?;
    cut_over(directory, &stage, &staged_db)?;
    cleanup(directory)?;
    Ok(MigrationReport {
        images: copied,
        bytes: total,
        already_converted: false,
    })
}

fn open_database(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    Ok(db)
}

/// Truncate the WAL; `false` means another reader still holds a snapshot.
fn checkpoint(db: &Connection) -> Result<bool> {
    let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    Ok(busy == 0)
}

fn snapshot_id(db: &Connection) -> Result<String> {
    Ok(db.query_row(
        "SELECT COALESCE((SELECT MAX(id) FROM observations), 0)
            || ':' || (SELECT COUNT(*) FROM observations)
            || ':' || COALESCE((SELECT MAX(id) FROM screenshots), 0)
            || ':' || (SELECT COUNT(*) FROM screenshots)
            || ':' || COALESCE((SELECT value FROM state WHERE key='retention_epoch'), 0)",
        [],
        |r| r.get(0),
    )?)
}

/// A packed history only needs the obsolete files removed after a verified run.
fn finish_cleanup(directory: &Path, source: Connection) -> Result<MigrationReport> {
    crate::tile_map::upgrade(&source)?;
    ensure!(
        directory.join("tile-migration-verified").exists(),
        "history is already packed; no migration cleanup is required"
    );
    let images = source.query_row("SELECT count(*) FROM images", [], |r| r.get::<_, i64>(0))?;
    source.execute_batch("DROP TABLE IF EXISTS migration_images;")?;
    drop(source);
    cleanup(directory)?;
    Ok(MigrationReport {
        images: images as u64,
        bytes: 0,
        already_converted: true,
    })
}

/// Copy the source once into the staging directory, bound to its snapshot
/// identity so a resumed run never mixes histories.
fn stage_copy(directory: &Path, stage: &Path, source: &Connection) -> Result<PathBuf> {
    let identity = snapshot_id(source)?;
    fs::create_dir_all(stage)?;
    fs::set_permissions(stage, fs::Permissions::from_mode(0o700))?;
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
        File::open(stage)?.sync_all()?;
    }
    ensure!(
        fs::read_to_string(stage.join("source"))? == identity,
        "source changed since migration began; preserve/remove staging and restart migration"
    );
    if directory.join("tiles").exists() && !stage.join("tiles").exists() {
        fs::rename(directory.join("tiles"), stage.join("tiles"))?;
    }
    Ok(staged_db)
}

/// Move legacy rows aside once and create the packed image tables.
fn open_staged(stage: &Path, staged_db: &Path) -> Result<Connection> {
    let db = open_database(staged_db)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
    let renamed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='migration_images')",
        [],
        |r| r.get(0),
    )?;
    if !renamed {
        db.execute_batch(
            "BEGIN IMMEDIATE;
            ALTER TABLE images RENAME TO migration_images;
            DROP INDEX images_by_fingerprint;",
        )?;
        tiles::initialize(stage, &db)?;
        db.execute_batch("COMMIT;")?;
    }
    crate::tile_map::upgrade(&db)?;
    Ok(db)
}

/// One legacy image stored as a single region of a pack.
struct Region {
    name: String,
    fingerprint: String,
    width: u32,
    height: u32,
    offset: i64,
    length: i64,
    key: Vec<u8>,
    checksum: Vec<u8>,
}

/// Resumable: each batch commits only images not yet converted.
fn pack_images(directory: &Path, stage: &Path, db: &mut Connection) -> Result<(u64, u64)> {
    let mut copied =
        db.query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))? as u64;
    let mut total: u64 = 0;
    loop {
        let pending = unconverted(db)?;
        if pending.is_empty() {
            break;
        }
        let (bytes, regions) = read_batch(directory, db, pending)?;
        let pack = write_pack(stage, &bytes)?;
        copied += insert_regions(db, &pack, regions)?;
        total += bytes.len() as u64;
        if copied % 1000 < BATCH_IMAGES as u64 {
            eprintln!("migration: {copied} images packed");
        }
    }
    Ok((copied, total))
}

fn unconverted(db: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = db.prepare(
        "SELECT name, fingerprint FROM migration_images
        WHERE NOT EXISTS(SELECT 1 FROM images WHERE images.name=migration_images.name)
        ORDER BY name LIMIT ?1",
    )?;
    Ok(stmt
        .query_map([BATCH_IMAGES], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Read legacy files until the batch byte budget; the rest wait for the next batch.
fn read_batch(
    directory: &Path,
    db: &Connection,
    pending: Vec<(String, String)>,
) -> Result<(Vec<u8>, Vec<Region>)> {
    let mut batch = Vec::new();
    let mut regions = Vec::new();
    for (name, fingerprint) in pending {
        let bytes = read_legacy(directory, &name)?;
        let (width, height) = codec::dimensions(&bytes)?;
        let mismatch: bool = db.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM screenshots WHERE image=?1 AND (width!=?2 OR height!=?3)
            )",
            params![name, width, height],
            |r| r.get(0),
        )?;
        ensure!(!mismatch, "legacy screenshot dimensions mismatch");
        if !batch.is_empty() && batch.len() + bytes.len() > BATCH_BYTES {
            break;
        }
        regions.push(Region {
            name,
            fingerprint,
            width,
            height,
            offset: batch.len() as i64,
            length: bytes.len() as i64,
            key: region_key(width, height, &bytes),
            checksum: blake3::hash(&bytes).as_bytes().to_vec(),
        });
        batch.extend_from_slice(&bytes);
    }
    Ok((batch, regions))
}

fn read_legacy(directory: &Path, name: &str) -> Result<Vec<u8>> {
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
        .open(directory.join("images").join(name))
        .with_context(|| format!("missing legacy image {name}"))?;
    ensure!(
        file.metadata()?.len() <= codec::MAX_ENCODED as u64,
        "legacy image exceeds byte limit"
    );
    let mut bytes = Vec::new();
    file.take(codec::MAX_ENCODED as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn region_key(width: u32, height: u32, bytes: &[u8]) -> Vec<u8> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"timeline-encoded-region-v1");
    hash.update(&width.to_le_bytes());
    hash.update(&height.to_le_bytes());
    hash.update(bytes);
    hash.finalize().as_bytes().to_vec()
}

/// Durable before any row references it.
fn write_pack(stage: &Path, bytes: &[u8]) -> Result<String> {
    let (pack, mut file) = tiles::new_pack(stage)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(stage.join("tiles"))?.sync_all()?;
    Ok(pack)
}

fn insert_regions(db: &mut Connection, pack: &str, regions: Vec<Region>) -> Result<u64> {
    let tx = db.transaction()?;
    let count = regions.len() as u64;
    for region in regions {
        tx.execute(
            "INSERT OR IGNORE INTO tiles(fingerprint, pack, offset, length, width, height, checksum)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                region.key,
                pack,
                region.offset,
                region.length,
                region.width,
                region.height,
                region.checksum
            ],
        )?;
        let id: i64 = tx.query_row(
            "UPDATE tiles SET refs=refs+1 WHERE fingerprint=?1 RETURNING id",
            [&region.key],
            |r| r.get(0),
        )?;
        let mut map = Vec::new();
        crate::tile_map::push(&mut map, id)?;
        tx.execute(
            "INSERT INTO images(name, fingerprint, width, height, tile_size, map)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                region.name,
                region.fingerprint,
                region.width,
                region.height,
                region.width.max(region.height),
                map
            ],
        )?;
    }
    tx.commit()?;
    Ok(count)
}

fn verify_packs(stage: &Path, db: &Connection) -> Result<()> {
    let mut stmt =
        db.prepare("SELECT pack, offset, length, checksum FROM tiles ORDER BY pack, offset")?;
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
    Ok(())
}

fn lost_references(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM screenshots
            WHERE image IS NOT NULL
                AND NOT EXISTS(SELECT 1 FROM images WHERE name=screenshots.image)
        )",
        [],
        |r| r.get(0),
    )?)
}

/// Replace the active database with the verified staging copy. Each step is
/// durable and safe to repeat after an interruption.
fn cut_over(directory: &Path, stage: &Path, staged_db: &Path) -> Result<()> {
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
    File::open(stage)?.sync_all()?;
    history::lock_file(&directory.join("tile-append.lock"))?.sync_all()?;
    let mut marker = File::create(directory.join("tile-migration-verified"))?;
    marker.write_all(b"v3 images verified; safe to finish obsolete-file cleanup\n")?;
    marker.sync_all()?;
    File::open(directory)?.sync_all()?;
    fs::rename(staged_db, directory.join("history.sqlite3"))?;
    File::open(stage)?.sync_all()?;
    File::open(directory)?.sync_all()?;
    let final_db = Connection::open(directory.join("history.sqlite3"))?;
    final_db.execute_batch("DROP TABLE IF EXISTS migration_images;")?;
    final_db.close().map_err(|(_, e)| e)?;
    Ok(())
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
