mod common;
use std::{io::Read, path::Path, process::Command};
use timeline::history::{History, ScreenshotSample};

fn legacy(dir: &Path, count: usize) {
    std::fs::create_dir_all(dir.join("images")).unwrap();
    for name in [
        "images.lock",
        "image-read-0.lock",
        "image-read-1.lock",
        "mcp-read.lock",
    ] {
        std::fs::File::create(dir.join(name)).unwrap();
    }
    let db = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    db.execute_batch(r#"
        CREATE TABLE observations(id INTEGER PRIMARY KEY AUTOINCREMENT, unix_ms INTEGER NOT NULL,
          elapsed_ms INTEGER NOT NULL, segment INTEGER NOT NULL, evidence TEXT NOT NULL, retention_ms INTEGER NOT NULL);
        INSERT INTO observations VALUES(7,100,10,1,'{"kind":"fixture","title":"preserved"}',100);
        CREATE TABLE state(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
        INSERT INTO state VALUES('manual_paused',1),('segment',1);
        CREATE TABLE images(name TEXT PRIMARY KEY,fingerprint TEXT NOT NULL);
        CREATE INDEX images_by_fingerprint ON images(fingerprint);
        CREATE TABLE screenshots(id INTEGER PRIMARY KEY AUTOINCREMENT,unix_ms INTEGER NOT NULL,
          monotonic_ms INTEGER NOT NULL,segment INTEGER NOT NULL,monitor TEXT NOT NULL,width INTEGER NOT NULL,
          height INTEGER NOT NULL,transform INTEGER NOT NULL,image TEXT,unavailable TEXT,retention_ms INTEGER NOT NULL);
        PRAGMA user_version=3;
    "#).unwrap();
    for i in 0..count {
        let name = format!("image-fixture-{i:04}.webp");
        std::fs::write(
            dir.join("images").join(&name),
            common::webp(&i.to_le_bytes()),
        )
        .unwrap();
        db.execute(
            "INSERT INTO images VALUES(?1,?2)",
            rusqlite::params![name, format!("legacy-{i}")],
        )
        .unwrap();
        db.execute(
            "INSERT INTO screenshots VALUES(?1,100,10,1,'DP-1',2,1,0,?2,NULL,100)",
            rusqlite::params![40 + i as i64, name],
        )
        .unwrap();
    }
    db.execute("INSERT INTO screenshots VALUES(1000,300,30,1,'DP-2',2,1,0,'image-fixture-0000.webp',NULL,300)", []).unwrap();
}

fn migrate(root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_timeline"))
        .env("XDG_DATA_HOME", root)
        .arg("migrate-images")
        .output()
        .unwrap()
}

fn assert_migrated(dir: &Path, count: usize) {
    let mut history = History::open(dir).unwrap();
    assert!(history.manual_paused().unwrap());
    let observations = history.observations(0, 100).unwrap();
    assert_eq!((observations[0].id, observations[0].monotonic_ms), (7, 10));
    assert_eq!(observations[0].evidence["title"], "preserved");
    assert_eq!(history.screenshots(0, 100).unwrap().len(), count + 1);
    for i in 0..count {
        let mut bytes = Vec::new();
        history
            .image(40 + i as i64)
            .unwrap()
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, common::webp(&i.to_le_bytes()));
    }
    let mut shared = Vec::new();
    history
        .image(1000)
        .unwrap()
        .unwrap()
        .read_to_end(&mut shared)
        .unwrap();
    assert_eq!(shared, common::webp(&0usize.to_le_bytes()));
    let prepared = History::prepare_image(dir, &common::webp(b"new")).unwrap();
    assert!(
        history
            .commit_screenshot(
                &ScreenshotSample {
                    unix_ms: 400,
                    monotonic_ms: 40,
                    segment: 1,
                    monitor: "DP-1".into(),
                    width: 2,
                    height: 1,
                    transform: 0
                },
                "new",
                &prepared
            )
            .unwrap()
            > 1000
    );
    assert!(!dir.join("images").exists());
    assert!(!common::pack_files(dir).is_empty());
    let db = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='migration_images'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn offline_cli_conversion_preserves_bytes_ids_metadata_and_shared_images() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("timeline");
    legacy(&dir, 2);
    assert!(History::open(&dir).is_err());
    let result = migrate(root.path());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_migrated(&dir, 2);
}

#[test]
fn failed_partial_conversion_preserves_originals_and_resumes_completed_batches() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("timeline");
    // More than one migration batch allows a committed first batch before failure.
    legacy(&dir, 65);
    let missing = dir.join("images/image-fixture-0064.webp");
    let bytes = std::fs::read(&missing).unwrap();
    std::fs::remove_file(&missing).unwrap();
    let result = migrate(root.path());
    assert!(!result.status.success());
    assert!(dir.join("images/image-fixture-0000.webp").is_file());
    let source = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    assert_eq!(
        source
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        source
            .query_row("SELECT COUNT(*) FROM screenshots", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        66
    );
    drop(source);
    // Exercise reopening a durable staged database after finalization/checkpoint.
    let stage = rusqlite::Connection::open(dir.join("tile-migration/history.sqlite3")).unwrap();
    assert_eq!(
        stage
            .query_row("SELECT COUNT(*) FROM images", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        64
    );
    stage
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
        .unwrap();
    drop(stage);
    std::fs::write(&missing, bytes).unwrap();
    let result = migrate(root.path());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let db = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT SUM(refs) FROM tiles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        65,
        "resumed batch doubled references"
    );
    drop(db);
    assert_migrated(&dir, 65);
}

#[test]
fn interrupted_cutover_after_pack_move_resumes_without_losing_or_duplicating_references() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("timeline");
    legacy(&dir, 2);
    // Deterministic filesystem fault after verification and moving the pack
    // directory, immediately before publishing the completed database.
    std::fs::create_dir(dir.join("tile-append.lock")).unwrap();
    let result = migrate(root.path());
    assert!(!result.status.success());
    assert!(dir.join("tiles").is_dir());
    assert!(!dir.join("tile-migration/tiles").exists());
    assert!(dir.join("images/image-fixture-0000.webp").is_file());
    let source = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    assert_eq!(
        source
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    drop(source);
    let stage = rusqlite::Connection::open(dir.join("tile-migration/history.sqlite3")).unwrap();
    assert_eq!(
        stage
            .query_row("SELECT COUNT(*) FROM migration_images", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        stage
            .query_row("SELECT SUM(refs) FROM tiles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(stage);
    std::fs::remove_dir(dir.join("tile-append.lock")).unwrap();
    let result = migrate(root.path());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let db = rusqlite::Connection::open(dir.join("history.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT SUM(refs) FROM tiles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(db);
    assert_migrated(&dir, 2);
}
