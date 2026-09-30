//! Content-addressed regions in bounded append-only packs. Metadata references
//! are published only after a complete batch is durable. No legacy read path.
use crate::{
    codec,
    history::{History, ScreenshotSample},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const TILE_SIZE: u32 = 256;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Original,
    Bundle,
    Thumbnail,
}
const PACK_TARGET: u64 = 16 * 1024 * 1024;

pub(crate) fn initialize(directory: &Path, db: &Connection) -> Result<()> {
    fs::create_dir_all(directory.join("tiles"))?;
    fs::set_permissions(directory.join("tiles"), fs::Permissions::from_mode(0o700))?;
    for name in [
        "images.lock",
        "tile-append.lock",
        "image-read-0.lock",
        "image-read-1.lock",
        "mcp-read.lock",
    ] {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(directory.join(name))?;
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS images (
        name TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, width INTEGER NOT NULL,
        height INTEGER NOT NULL, tile_size INTEGER NOT NULL, map BLOB NOT NULL);
        CREATE INDEX IF NOT EXISTS images_by_fingerprint ON images(fingerprint);
        CREATE TABLE IF NOT EXISTS tiles (
        id INTEGER PRIMARY KEY AUTOINCREMENT, fingerprint BLOB NOT NULL UNIQUE,
        pack TEXT NOT NULL, offset INTEGER NOT NULL, length INTEGER NOT NULL,
        width INTEGER NOT NULL, height INTEGER NOT NULL, refs INTEGER NOT NULL DEFAULT 0,
        checksum BLOB NOT NULL);
        CREATE INDEX IF NOT EXISTS tiles_by_pack ON tiles(pack);
        CREATE TABLE IF NOT EXISTS screenshots (
        id INTEGER PRIMARY KEY AUTOINCREMENT, unix_ms INTEGER NOT NULL,
        monotonic_ms INTEGER NOT NULL, segment INTEGER NOT NULL, monitor TEXT NOT NULL,
        width INTEGER NOT NULL, height INTEGER NOT NULL, transform INTEGER NOT NULL,
        image TEXT, unavailable TEXT, retention_ms INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS screenshots_by_time ON screenshots(unix_ms);
        CREATE INDEX IF NOT EXISTS screenshots_by_order ON screenshots(segment,monitor,monotonic_ms,id);
        CREATE INDEX IF NOT EXISTS screenshots_by_retention ON screenshots(retention_ms);
        CREATE INDEX IF NOT EXISTS screenshots_by_image ON screenshots(image);
        PRAGMA user_version=5;")?;
    Ok(())
}

#[derive(Clone)]
struct Tile {
    fingerprint: Vec<u8>,
    pack: String,
    offset: u64,
    length: u64,
    width: u32,
    height: u32,
    checksum: Vec<u8>,
}
fn tile_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Tile> {
    Ok(Tile {
        fingerprint: row.get(0)?,
        pack: row.get(1)?,
        offset: row.get::<_, u32>(2)? as u64,
        length: row.get::<_, u32>(3)? as u64,
        width: row.get(4)?,
        height: row.get(5)?,
        checksum: row.get(6)?,
    })
}
fn pack_path(directory: &Path, name: &str) -> Result<PathBuf> {
    ensure!(
        name.starts_with("pack-")
            && name.ends_with(".bin")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'),
        "invalid pack name"
    );
    Ok(directory.join("tiles").join(name))
}
pub(crate) fn new_pack(directory: &Path) -> Result<(String, File)> {
    let temporary = tempfile::Builder::new()
        .prefix("pack-")
        .suffix(".part")
        .tempfile_in(directory.join("tiles"))?;
    let name = format!(
        "{}.bin",
        temporary.path().file_stem().unwrap().to_string_lossy()
    );
    let file = temporary.persist_noclobber(pack_path(directory, &name)?)?;
    Ok((name, file))
}
fn append_batch(directory: &Path, bytes: &[u8]) -> Result<(String, u64)> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= codec::MAX_ENCODED,
        "invalid tile batch size"
    );
    let lock = File::open(directory.join("tile-append.lock"))?;
    lock.lock()?;
    let current = match fs::read_to_string(directory.join("tiles/active")) {
        Ok(name) => {
            let path = pack_path(directory, &name)?;
            match OpenOptions::new().read(true).append(true).open(path) {
                Ok(file) if file.metadata()?.len() + bytes.len() as u64 <= PACK_TARGET => {
                    Some((name, file))
                }
                Ok(_) => None,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let (name, mut file, created) = match current {
        Some((name, file)) => (name, file, false),
        None => {
            let (name, file) = new_pack(directory)?;
            (name, file, true)
        }
    };
    let offset = file.seek(SeekFrom::End(0))?;
    file.write_all(bytes)?;
    file.sync_data()?;
    if created {
        let mut marker = tempfile::Builder::new()
            .prefix(".active-")
            .tempfile_in(directory.join("tiles"))?;
        marker.write_all(name.as_bytes())?;
        marker.as_file().sync_all()?;
        marker.persist(directory.join("tiles/active"))?;
        File::open(directory.join("tiles"))?.sync_all()?;
    }
    Ok((name, offset))
}

/// Holds the physical publication lease until the recorder commits or discards it.
pub struct PreparedImage {
    pub(crate) directory: PathBuf,
    width: u32,
    height: u32,
    tile_size: u32,
    tiles: Vec<Tile>,
    _lease: File,
}
impl PreparedImage {
    pub(crate) fn publish(
        &self,
        tx: &Transaction<'_>,
        sample: &ScreenshotSample,
        fingerprint: &str,
    ) -> Result<String> {
        ensure!(
            fingerprint.len() <= 256 && sample.width == self.width && sample.height == self.height,
            "prepared image dimensions or fingerprint mismatch"
        );
        let name = fingerprint.to_owned();
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM images WHERE name=?1)",
            [&name],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(name);
        }
        let mut map = Vec::with_capacity(self.tiles.len() * 8);
        for tile in &self.tiles {
            tx.execute("INSERT OR IGNORE INTO tiles(fingerprint,pack,offset,length,width,height,checksum) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![tile.fingerprint,tile.pack,i64::try_from(tile.offset)?,i64::try_from(tile.length)?,tile.width,tile.height,tile.checksum])?;
            let id: i64 = tx.query_row(
                "UPDATE tiles SET refs=refs+1 WHERE fingerprint=?1 RETURNING id",
                [&tile.fingerprint],
                |r| r.get(0),
            )?;
            crate::tile_map::push(&mut map, id)?;
        }
        tx.execute("INSERT INTO images(name,fingerprint,width,height,tile_size,map) VALUES (?1,?2,?3,?4,?5,?6)", params![name,fingerprint,self.width,self.height,self.tile_size,map])?;
        Ok(name)
    }
}

fn prepare(
    directory: &Path,
    width: u32,
    height: u32,
    pixels: Option<&[u8]>,
    encoded: Option<&[u8]>,
) -> Result<PreparedImage> {
    codec::pixel_bytes(width, height)?;
    let lease = History::image_lease(directory)?;
    let history = History::read(directory)?;
    let tile_size = if pixels.is_some() {
        TILE_SIZE
    } else {
        width.max(height)
    };
    let mut known = HashMap::<Vec<u8>, Tile>::new();
    let mut order = Vec::new();
    let mut batch = Vec::new();
    for y in (0..height).step_by(tile_size as usize) {
        for x in (0..width).step_by(tile_size as usize) {
            let (w, h) = (tile_size.min(width - x), tile_size.min(height - y));
            let mut raw = Vec::new();
            let mut hash = blake3::Hasher::new();
            hash.update(if pixels.is_some() {
                b"timeline-raw-tile-v1"
            } else {
                b"timeline-encoded-region-v1"
            });
            hash.update(&w.to_le_bytes());
            hash.update(&h.to_le_bytes());
            if let Some(pixels) = pixels {
                raw.reserve((w * h * 4) as usize);
                for row in y..y + h {
                    let start = ((row * width + x) * 4) as usize;
                    raw.extend_from_slice(&pixels[start..start + (w * 4) as usize]);
                }
                hash.update(&crate::config::WEBP_QUALITY.to_bits().to_le_bytes());
                hash.update(&crate::config::WEBP_METHOD.to_le_bytes());
                hash.update(&raw);
            } else {
                hash.update(encoded.unwrap());
            }
            let fingerprint = hash.finalize().as_bytes().to_vec();
            if !known.contains_key(&fingerprint) {
                let existing = history.connection.query_row("SELECT fingerprint,pack,offset,length,width,height,checksum FROM tiles WHERE fingerprint=?1", [&fingerprint], tile_row).optional()?;
                let tile = if let Some(tile) = existing {
                    ensure!(
                        pack_path(directory, &tile.pack)?.is_file(),
                        "retained tile pack missing"
                    );
                    tile
                } else {
                    let data = if pixels.is_some() {
                        codec::encode_bgra(&raw, w, h, false)?
                    } else {
                        encoded.unwrap().to_vec()
                    };
                    ensure!(
                        batch.len() + data.len() <= codec::MAX_ENCODED,
                        "tile batch exceeds encoded budget"
                    );
                    let tile = Tile {
                        fingerprint: fingerprint.clone(),
                        pack: String::new(),
                        offset: batch.len() as u64,
                        length: data.len() as u64,
                        width: w,
                        height: h,
                        checksum: blake3::hash(&data).as_bytes().to_vec(),
                    };
                    batch.extend_from_slice(&data);
                    tile
                };
                known.insert(fingerprint.clone(), tile);
            }
            order.push(fingerprint);
        }
    }
    if !batch.is_empty() {
        let (pack, offset) = append_batch(directory, &batch)?;
        for tile in known.values_mut().filter(|t| t.pack.is_empty()) {
            tile.pack = pack.clone();
            tile.offset += offset;
        }
    }
    Ok(PreparedImage {
        directory: directory.into(),
        width,
        height,
        tile_size,
        tiles: order.iter().map(|key| known[key].clone()).collect(),
        _lease: lease,
    })
}

fn map_ids(bytes: &[u8]) -> Result<Vec<i64>> {
    crate::tile_map::decode(bytes)
}

pub(crate) fn expire_images(tx: &Transaction<'_>, cutoff: i64) -> Result<()> {
    loop {
        // Bounded batches keep retention's working set independent of history
        // size. Collect before mutation, aggregate repeated references, and
        // reuse statements rather than preparing one per tile occurrence.
        let rows = {
            let mut stmt = tx.prepare_cached("SELECT name,map FROM images WHERE name IN (SELECT image FROM screenshots WHERE retention_ms<?1 AND image IS NOT NULL) AND NOT EXISTS (SELECT 1 FROM screenshots WHERE image=images.name AND retention_ms>=?1) LIMIT 128")?;
            stmt.query_map([cutoff], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.is_empty() {
            break;
        }
        let mut counts = HashMap::<i64, i64>::new();
        for (_, map) in &rows {
            for id in map_ids(map)? {
                *counts.entry(id).or_default() += 1;
            }
        }
        let mut decrement =
            tx.prepare_cached("UPDATE tiles SET refs=refs-?2 WHERE id=?1 AND refs>=?2")?;
        for (id, count) in counts {
            ensure!(
                decrement.execute(params![id, count])? == 1,
                "tile reference count corrupt"
            );
        }
        let mut delete = tx.prepare_cached("DELETE FROM images WHERE name=?1")?;
        for (name, _) in rows {
            delete.execute([name])?;
        }
    }
    Ok(())
}

pub struct ImageRead {
    bytes: std::io::Cursor<Vec<u8>>,
    _lease: File,
    _slot: File,
}
impl Read for ImageRead {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.bytes.read(buffer)
    }
}

#[derive(Serialize)]
struct BundleTile {
    key: String,
    width: u32,
    height: u32,
    length: usize,
}
#[derive(Serialize)]
struct Bundle {
    version: u32,
    width: u32,
    height: u32,
    tile_size: u32,
    tiles: Vec<BundleTile>,
    map: Vec<usize>,
}

impl History {
    pub(crate) fn image_lease(directory: &Path) -> Result<File> {
        let file = File::open(directory.join("images.lock"))?;
        file.lock_shared()?;
        Ok(file)
    }
    pub(crate) fn try_image_lease(&self) -> Result<File> {
        let file = File::open(self.directory.join("images.lock"))?;
        file.try_lock_shared().context("image cleanup busy")?;
        Ok(file)
    }
    pub fn prepare_image(directory: &Path, bytes: &[u8]) -> Result<PreparedImage> {
        let (width, height, _) = codec::decode_bgra(bytes)?;
        prepare(directory, width, height, None, Some(bytes))
    }
    pub fn prepare_pixels(
        directory: &Path,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<PreparedImage> {
        ensure!(
            pixels.len() == codec::pixel_bytes(width, height)?,
            "invalid pixel length"
        );
        prepare(directory, width, height, Some(pixels), None)
    }
    pub fn image(&self, id: i64) -> Result<Option<ImageRead>> {
        ensure!(id > 0, "invalid screenshot identifier");
        let mut slot = None;
        for index in 0..2 {
            let file = File::open(self.directory.join(format!("image-read-{index}.lock")))?;
            match file.try_lock() {
                Ok(()) => {
                    slot = Some(file);
                    break;
                }
                Err(std::fs::TryLockError::WouldBlock) => (),
                Err(e) => return Err(e.into()),
            }
        }
        let slot = slot.context("image reads busy")?;
        let lease = self.try_image_lease()?;
        let _snapshot = self.connection.unchecked_transaction()?;
        self.image_admitted(id, slot, lease, ImageFormat::Original)
    }
    /// Caller acquires reclamation lease BEFORE opening its SQLite snapshot.
    pub(crate) fn image_admitted(
        &self,
        id: i64,
        slot: File,
        lease: File,
        format: ImageFormat,
    ) -> Result<Option<ImageRead>> {
        let row=self.connection.query_row("SELECT images.width,images.height,images.tile_size,CASE WHEN length(images.map)<=5760 THEN images.map ELSE NULL END FROM screenshots JOIN images ON images.name=screenshots.image WHERE screenshots.id=?1",[id],|r|Ok((r.get::<_,u32>(0)?,r.get::<_,u32>(1)?,r.get::<_,u32>(2)?,r.get::<_,Vec<u8>>(3)?))).optional()?;
        let Some((width, height, tile_size, map)) = row else {
            return Ok(None);
        };
        let pixels = codec::pixel_bytes(width, height)?;
        ensure!(tile_size > 0 && tile_size <= 16383, "invalid tile grid");
        let ids = map_ids(&map)?;
        let columns = width.div_ceil(tile_size);
        ensure!(
            ids.len() == (columns * height.div_ceil(tile_size)) as usize,
            "invalid tile map dimensions"
        );
        // Validate the retained screenshot and map before considering cached data.
        let cache_key = format!(
            "{}:{width}:{height}:{tile_size}:{}",
            self.directory.display(),
            blake3::hash(&map)
        );
        if format == ImageFormat::Thumbnail
            && let Some(bytes) = crate::thumbnails::get(&cache_key)
        {
            return Ok(Some(ImageRead {
                bytes: std::io::Cursor::new(bytes),
                _lease: lease,
                _slot: slot,
            }));
        }
        let mut header = Bundle {
            version: 1,
            width,
            height,
            tile_size,
            tiles: Vec::new(),
            map: Vec::new(),
        };
        let mut index = HashMap::new();
        let mut payloads = Vec::<Vec<u8>>::new();
        let mut total = 0;
        let mut packs = HashMap::<String, File>::new();
        for (position, id) in ids.iter().enumerate() {
            let number = if let Some(&number) = index.get(id) {
                number
            } else {
                let tile=self.connection.query_row("SELECT fingerprint,pack,offset,length,width,height,checksum FROM tiles WHERE id=?1",[id],tile_row).optional()?.context("missing tile descriptor")?;
                ensure!(
                    tile.length > 0
                        && tile.length <= codec::MAX_ENCODED as u64
                        && tile.checksum.len() == 32,
                    "invalid tile descriptor"
                );
                total += tile.length as usize;
                ensure!(total <= codec::MAX_ENCODED, "image tile bytes exceed limit");
                if !packs.contains_key(&tile.pack) {
                    let file = match File::open(pack_path(&self.directory, &tile.pack)?) {
                        Ok(f) => f,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                        Err(e) => return Err(e.into()),
                    };
                    packs.insert(tile.pack.clone(), file);
                }
                let file = packs.get_mut(&tile.pack).unwrap();
                ensure!(
                    tile.offset
                        .checked_add(tile.length)
                        .is_some_and(|end| file.metadata().is_ok_and(|m| end <= m.len())),
                    "truncated tile pack"
                );
                file.seek(SeekFrom::Start(tile.offset))?;
                let mut data = vec![0; tile.length as usize];
                file.read_exact(&mut data)?;
                ensure!(
                    blake3::hash(&data).as_bytes().as_slice() == tile.checksum,
                    "tile checksum mismatch"
                );
                ensure!(
                    codec::dimensions(&data)? == (tile.width, tile.height),
                    "tile dimensions mismatch"
                );
                let number = payloads.len();
                header.tiles.push(BundleTile {
                    key: blake3::Hash::from_bytes(tile.fingerprint.as_slice().try_into()?)
                        .to_hex()
                        .to_string(),
                    width: tile.width,
                    height: tile.height,
                    length: data.len(),
                });
                payloads.push(data);
                index.insert(id, number);
                number
            };
            let x = position as u32 % columns * tile_size;
            let y = position as u32 / columns * tile_size;
            ensure!(
                (header.tiles[number].width, header.tiles[number].height)
                    == (tile_size.min(width - x), tile_size.min(height - y)),
                "tile dimensions do not fit map"
            );
            header.map.push(number);
        }
        let bytes = if format == ImageFormat::Bundle {
            let json = serde_json::to_vec(&header)?;
            ensure!(json.len() <= 1024 * 1024, "tile header exceeds limit");
            let mut bytes = Vec::with_capacity(4 + json.len() + total);
            bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&json);
            for data in payloads {
                bytes.extend_from_slice(&data);
            }
            bytes
        } else if ids.len() == 1 && format == ImageFormat::Original {
            payloads.pop().unwrap()
        } else {
            let mut raw = vec![0; pixels];
            for (position, &number) in header.map.iter().enumerate() {
                let x = position as u32 % columns * tile_size;
                let y = position as u32 / columns * tile_size;
                let tile = &header.tiles[number];
                codec::decode_into(
                    &payloads[number],
                    &mut raw[((y * width + x) * 4) as usize..],
                    tile.width,
                    tile.height,
                    width * 4,
                )?;
            }
            if format == ImageFormat::Thumbnail {
                codec::thumbnail(&raw, width, height)?
            } else {
                codec::encode_bgra(&raw, width, height, true)?
            }
        };
        if format == ImageFormat::Thumbnail {
            crate::thumbnails::insert(cache_key, &bytes);
        }
        Ok(Some(ImageRead {
            bytes: std::io::Cursor::new(bytes),
            _lease: lease,
            _slot: slot,
        }))
    }

    pub(crate) fn reclaim_images(&self, stop: &AtomicBool) -> Result<bool> {
        let mut complete = true;
        // A separate writer connection keeps the metadata event thread out of
        // physical compaction. Never wait on a lease while holding a transaction.
        let mut db = Connection::open(self.directory.join("history.sqlite3"))?;
        db.busy_timeout(Duration::from_millis(500))?;
        db.execute_batch("PRAGMA synchronous=FULL;")?;
        for entry in fs::read_dir(self.directory.join("tiles"))? {
            if stop.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "active" {
                continue;
            }
            let lease = File::open(self.directory.join("images.lock"))?;
            match lease.try_lock() {
                Ok(()) => (),
                Err(std::fs::TryLockError::WouldBlock) => {
                    complete = false;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
            if name.ends_with(".part") || name.starts_with(".active-") {
                fs::remove_file(entry.path())?;
                continue;
            }
            if !name.starts_with("pack-") || !name.ends_with(".bin") {
                continue;
            }
            let length = entry.metadata()?.len();
            let live: i64 = db.query_row(
                "SELECT COALESCE(SUM(length),0) FROM tiles WHERE pack=?1 AND refs>0",
                [&name],
                |r| r.get(0),
            )?;
            ensure!(live >= 0, "invalid live pack size");
            let live = live as u64;
            if live == 0 {
                db.execute("DELETE FROM tiles WHERE pack=?1", [&name])?;
                fs::remove_file(entry.path())?;
            } else if live.saturating_mul(4) < length.saturating_mul(3) {
                let mut source = File::open(entry.path())?;
                let rows = {
                    let mut stmt=db.prepare("SELECT id,offset,length,checksum FROM tiles WHERE pack=?1 AND refs>0 ORDER BY offset")?;
                    stmt.query_map([&name], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, u32>(1)? as u64,
                            r.get::<_, u32>(2)? as usize,
                            r.get::<_, Vec<u8>>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                };
                let (next, mut file) = new_pack(&self.directory)?;
                let mut updates = Vec::new();
                let mut offset = 0u64;
                for (id, old, size, checksum) in rows {
                    ensure!(
                        size <= codec::MAX_ENCODED
                            && old
                                .checked_add(size as u64)
                                .is_some_and(|end| end <= length),
                        "corrupt pack descriptor"
                    );
                    source.seek(SeekFrom::Start(old))?;
                    let mut bytes = vec![0; size];
                    source.read_exact(&mut bytes)?;
                    ensure!(
                        blake3::hash(&bytes).as_bytes().as_slice() == checksum,
                        "corrupt tile during compaction"
                    );
                    file.write_all(&bytes)?;
                    updates.push((id, offset));
                    offset += size as u64;
                }
                file.sync_all()?;
                File::open(self.directory.join("tiles"))?.sync_all()?;
                let tx = db.transaction()?;
                for (id, offset) in updates {
                    tx.execute(
                        "UPDATE tiles SET pack=?1,offset=?2 WHERE id=?3",
                        params![next, i64::try_from(offset)?, id],
                    )?;
                }
                tx.execute("DELETE FROM tiles WHERE pack=?1 AND refs=0", [&name])?;
                tx.commit()?;
                fs::remove_file(entry.path())?;
            } else {
                db.execute("DELETE FROM tiles WHERE pack=?1 AND refs=0", [&name])?;
            }
        }
        File::open(self.directory.join("tiles"))?.sync_all()?;
        Ok(complete)
    }
}
