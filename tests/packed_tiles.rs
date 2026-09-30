mod common;

use std::{io::Read, path::Path};
use timeline::{
    codec::decode_bgra,
    history::{History, ScreenshotSample},
};

fn sample(time: i64, width: u32, height: u32) -> ScreenshotSample {
    ScreenshotSample {
        unix_ms: time,
        monotonic_ms: time,
        segment: 1,
        monitor: "DP-1".into(),
        width,
        height,
        transform: 0,
    }
}

fn pixels(width: u32, height: u32, colors: &[[u8; 4]]) -> Vec<u8> {
    (0..height)
        .flat_map(|_| (0..width).flat_map(|x| colors[(x / 256) as usize % colors.len()]))
        .collect()
}

const RED: [u8; 4] = [0, 0, 255, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

fn record(
    history: &mut History,
    dir: &Path,
    time: i64,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> i64 {
    let prepared = History::prepare_pixels(dir, width, height, bytes).unwrap();
    history
        .commit_screenshot(
            &sample(time, width, height),
            &format!("capture-{time}"),
            &prepared,
        )
        .unwrap()
}

fn tile_count(dir: &Path) -> i64 {
    rusqlite::Connection::open(dir.join("history.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM tiles", [], |row| row.get(0))
        .unwrap()
}

fn pack_bytes(dir: &Path) -> u64 {
    common::pack_files(dir)
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum()
}

fn export(history: &History, id: i64) -> Vec<u8> {
    let mut bytes = Vec::new();
    history
        .image(id)
        .unwrap()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn switching_workspaces_reuses_all_retained_tiles_even_after_recorder_restart() {
    let dir = tempfile::tempdir().unwrap();
    let a = pixels(512, 256, &[RED, BLUE]);
    let b = pixels(512, 256, &[GREEN, BLUE]);
    let mut history = History::open(dir.path()).unwrap();
    let first = record(&mut history, dir.path(), 100, 512, 256, &a);
    let expected = decode_bgra(&export(&history, first)).unwrap();
    record(&mut history, dir.path(), 200, 512, 256, &b);
    let count = tile_count(dir.path());
    let size = pack_bytes(dir.path());
    assert_eq!(count, 3);
    drop(history);
    let mut history = History::open(dir.path()).unwrap();
    let returned = record(&mut history, dir.path(), 300, 512, 256, &a);
    assert_eq!(tile_count(dir.path()), count);
    assert_eq!(
        pack_bytes(dir.path()),
        size,
        "return to A wrote duplicate tile data"
    );
    assert_eq!(decode_bgra(&export(&history, returned)).unwrap(), expected);
}

#[test]
fn identical_tiles_reuse_content_across_screen_positions() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    record(
        &mut history,
        dir.path(),
        100,
        512,
        256,
        &pixels(512, 256, &[RED, BLUE]),
    );
    let size = pack_bytes(dir.path());
    let id = record(
        &mut history,
        dir.path(),
        200,
        512,
        256,
        &pixels(512, 256, &[BLUE, RED]),
    );
    assert_eq!(tile_count(dir.path()), 2);
    assert_eq!(pack_bytes(dir.path()), size);
    let (width, height, decoded) = decode_bgra(&export(&history, id)).unwrap();
    assert_eq!((width, height), (512, 256));
    assert!(decoded[0] > 240 && decoded[2] < 15);
    assert!(decoded[256 * 4 + 2] > 240 && decoded[256 * 4] < 15);
}

#[test]
fn partial_edge_tiles_reconstruct_the_original_dimensions_without_padding() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let id = record(
        &mut history,
        dir.path(),
        100,
        257,
        259,
        &pixels(257, 259, &[RED, BLUE]),
    );
    assert_eq!(tile_count(dir.path()), 4);
    let (width, height, decoded) = decode_bgra(&export(&history, id)).unwrap();
    assert_eq!((width, height, decoded.len()), (257, 259, 257 * 259 * 4));
    for y in [0usize, 255, 256, 258] {
        let red = (y * 257 + 255) * 4;
        let blue = (y * 257 + 256) * 4;
        assert!(decoded[red + 2] > 240 && decoded[red] < 15);
        assert!(decoded[blue] > 240 && decoded[blue + 2] < 15);
    }
}

#[test]
fn tiles_with_identical_pixel_bytes_but_different_dimensions_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let bytes = [RED, BLUE].concat();
    record(&mut history, dir.path(), 100, 2, 1, &bytes);
    let id = record(&mut history, dir.path(), 200, 1, 2, &bytes);
    assert_eq!(tile_count(dir.path()), 2);
    let (w, h, _) = decode_bgra(&export(&history, id)).unwrap();
    assert_eq!((w, h), (1, 2));
}

#[test]
fn expiry_compacts_garbage_while_preserving_tiles_shared_with_retained_images() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let expired = record(
        &mut history,
        dir.path(),
        100,
        512,
        256,
        &pixels(512, 256, &[RED, BLUE]),
    );
    let kept = record(
        &mut history,
        dir.path(),
        300,
        512,
        256,
        &pixels(512, 256, &[BLUE, GREEN]),
    );
    let expected = export(&history, kept);
    let old_size = pack_bytes(dir.path());
    history.cleanup_before(200).unwrap();
    assert!(history.image(expired).unwrap().is_none());
    assert_eq!(export(&history, kept), expected);
    assert_eq!(tile_count(dir.path()), 2);
    assert!(
        pack_bytes(dir.path()) < old_size,
        "unused tile bytes were not compacted"
    );
    drop(history);
    let mut history = History::open(dir.path()).unwrap();
    assert_eq!(export(&history, kept), expected);
    history.cleanup_before(400).unwrap();
    assert_eq!(tile_count(dir.path()), 0);
    assert_eq!(pack_bytes(dir.path()), 0);
}

#[test]
fn prepared_tiles_survive_expiry_of_their_previous_last_reference() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let bytes = pixels(512, 256, &[RED, BLUE]);
    record(&mut history, dir.path(), 100, 512, 256, &bytes);
    let prepared = History::prepare_pixels(dir.path(), 512, 256, &bytes).unwrap();
    history.cleanup_before(200).unwrap();
    let id = history
        .commit_screenshot(&sample(300, 512, 256), "new", &prepared)
        .unwrap();
    drop(prepared);
    history.cleanup_before(200).unwrap();
    assert_eq!(decode_bgra(&export(&history, id)).unwrap().0, 512);
    assert_eq!(tile_count(dir.path()), 2);
}

#[test]
fn malformed_pixels_and_dimension_mismatches_never_publish_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    for (w, h, bytes) in [
        (0, 1, vec![]),
        (1, 0, vec![]),
        (2, 1, vec![0; 4]),
        (1, 1, vec![0; 8]),
        (u32::MAX, u32::MAX, vec![]),
    ] {
        assert!(History::prepare_pixels(dir.path(), w, h, &bytes).is_err());
    }
    assert!(History::prepare_image(dir.path(), b"not webp").is_err());
    let prepared = History::prepare_pixels(dir.path(), 2, 1, &[RED, BLUE].concat()).unwrap();
    assert!(
        history
            .commit_screenshot(&sample(100, 1, 2), "bad dimensions", &prepared)
            .is_err()
    );
    assert!(history.screenshots(0, 100).unwrap().is_empty());
    history
        .commit_screenshot(&sample(100, 2, 1), "valid", &prepared)
        .unwrap();
    let image = history.find_image("valid").unwrap().unwrap();
    assert!(!matches!(
        history.reuse_screenshot(&sample(200, 1, 2), &image),
        Ok(Some(_))
    ));
    assert_eq!(history.screenshots(0, 100).unwrap().len(), 1);
}

#[test]
fn truncated_or_missing_packs_never_return_a_partial_screenshot() {
    for missing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path()).unwrap();
        let id = record(
            &mut history,
            dir.path(),
            100,
            512,
            256,
            &pixels(512, 256, &[RED, BLUE]),
        );
        for path in common::pack_files(dir.path()) {
            if missing {
                std::fs::remove_file(path).unwrap();
            } else {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_len(1)
                    .unwrap();
            }
        }
        match history.image(id) {
            Ok(None) | Err(_) => (),
            Ok(Some(_)) => panic!("corrupt capture was returned as complete evidence"),
        }
    }
}

#[test]
fn damaged_tile_payload_is_rejected_even_when_its_record_length_is_unchanged() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let id = record(
        &mut history,
        dir.path(),
        100,
        512,
        256,
        &pixels(512, 256, &[RED, BLUE]),
    );
    let db = rusqlite::Connection::open(dir.path().join("history.sqlite3")).unwrap();
    let (pack, offset, length): (String, i64, i64) = db
        .query_row(
            "SELECT pack, offset, length FROM tiles LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join("tiles").join(pack))
        .unwrap();
    file.seek(SeekFrom::Start((offset + length - 1) as u64))
        .unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start((offset + length - 1) as u64))
        .unwrap();
    file.write_all(&[byte[0] ^ 0x80]).unwrap();
    match history.image(id) {
        Ok(None) | Err(_) => (),
        Ok(Some(_)) => panic!("checksum mismatch was served as valid evidence"),
    }
}

#[test]
fn repeated_tiles_within_and_between_images_have_balanced_retention_references() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let bytes = pixels(768, 256, &[BLUE]);
    record(&mut history, dir.path(), 100, 768, 256, &bytes);
    let kept = record(&mut history, dir.path(), 300, 768, 256, &bytes);
    assert_eq!(tile_count(dir.path()), 1);
    history.cleanup_before(200).unwrap();
    assert!(history.image(kept).unwrap().is_some());
    let db = rusqlite::Connection::open(dir.path().join("history.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT refs FROM tiles", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        3
    );
    history.cleanup_before(400).unwrap();
    assert_eq!(tile_count(dir.path()), 0);
}

#[test]
fn schema_four_upgrade_preserves_pixels_references_and_retention() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let id = record(
        &mut history,
        dir.path(),
        100,
        512,
        256,
        &pixels(512, 256, &[RED, BLUE]),
    );
    let original = export(&history, id);
    let packs = pack_bytes(dir.path());
    drop(history);
    let db = rusqlite::Connection::open(dir.path().join("history.sqlite3")).unwrap();
    let old = [1i64.to_le_bytes(), 2i64.to_le_bytes()].concat();
    db.execute("UPDATE images SET map=?1", [&old]).unwrap();
    db.execute_batch("PRAGMA user_version=4").unwrap();
    assert!(
        History::read(dir.path()).is_err(),
        "runtime must not read old maps"
    );
    let mut history = History::open(dir.path()).unwrap();
    assert_eq!(export(&history, id), original);
    assert_eq!(pack_bytes(dir.path()), packs);
    assert_eq!(
        db.query_row("SELECT length(map) FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT SUM(refs) FROM tiles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    history.cleanup_before(200).unwrap();
    assert_eq!(tile_count(dir.path()), 0);
}
