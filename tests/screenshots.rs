use std::io::Read;
use timeline::history::{History, ScreenshotSample};

fn sample(time: i64, monitor: &str) -> ScreenshotSample {
    ScreenshotSample {
        unix_ms: time,
        monotonic_ms: time,
        segment: 1,
        monitor: monitor.into(),
        width: 2,
        height: 1,
        transform: 0,
    }
}

#[test]
fn durable_images_are_independent_evidence_and_shared_until_the_last_reference_expires() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let prepared = History::prepare_image(dir.path(), b"encoded webp fixture").unwrap();
    let first = history
        .commit_screenshot(&sample(100, "DP-1"), "pixels", &prepared)
        .unwrap();
    let image = history.find_image("pixels").unwrap().unwrap();
    let second = history
        .reuse_screenshot(&sample(300, "DP-2"), &image)
        .unwrap()
        .unwrap();
    drop(prepared);
    history.cleanup_before(200).unwrap();
    assert!(history.image(first).unwrap().is_none());
    let mut read = history.image(second).unwrap().unwrap();
    history.cleanup_before(400).unwrap();
    assert!(history.image(second).unwrap().is_none());
    let mut bytes = Vec::new();
    read.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"encoded webp fixture");
    drop(read);
    history.cleanup_before(400).unwrap();
    assert!(history.find_image("pixels").unwrap().is_none());
    drop(history);
    let mut history = History::open(dir.path()).unwrap();
    assert!(history.screenshots(0, 100).unwrap().is_empty());
    let prepared = History::prepare_image(dir.path(), b"new image").unwrap();
    assert!(
        history
            .commit_screenshot(&sample(500, "DP-1"), "new", &prepared)
            .unwrap()
            > second
    );
}

#[test]
fn interrupted_publication_is_reclaimed_but_inflight_preparation_is_protected() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let prepared = History::prepare_image(dir.path(), b"complete but not committed").unwrap();
    history.cleanup_before(0).unwrap();
    // Inject failure at the SQLite boundary, after durable file publication.
    let fault = rusqlite::Connection::open(dir.path().join("history.sqlite3")).unwrap();
    fault.execute_batch("CREATE TRIGGER fail_image_commit BEFORE INSERT ON screenshots BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END;").unwrap();
    assert!(
        history
            .commit_screenshot(&sample(100, "DP-1"), "pixels", &prepared)
            .is_err()
    );
    assert!(history.screenshots(0, 100).unwrap().is_empty());
    assert!(history.find_image("pixels").unwrap().is_none());
    drop(prepared);
    drop(history);
    std::fs::write(
        dir.path().join("images/image-interrupted.part"),
        b"unfinished",
    )
    .unwrap();
    let mut history = History::open(dir.path()).unwrap();
    history.cleanup_before(0).unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path().join("images"))
            .unwrap()
            .count(),
        0
    );
    fault
        .execute_batch("DROP TRIGGER fail_image_commit")
        .unwrap();
    let prepared = History::prepare_image(dir.path(), b"committed").unwrap();
    history.cleanup_before(0).unwrap();
    let id = history
        .commit_screenshot(&sample(100, "DP-1"), "pixels", &prepared)
        .unwrap();
    drop(prepared);
    drop(history);
    assert!(
        History::open(dir.path())
            .unwrap()
            .image(id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn image_reads_are_bounded_and_missing_files_do_not_substitute_other_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::open(dir.path()).unwrap();
    let prepared = History::prepare_image(dir.path(), b"fixture").unwrap();
    let id = history
        .commit_screenshot(&sample(100, "DP-1"), "pixels", &prepared)
        .unwrap();
    drop(prepared);
    let a = history.image(id).unwrap().unwrap();
    let reader = History::read(dir.path()).unwrap();
    let b = reader.image(id).unwrap().unwrap();
    assert!(reader.image(id).err().unwrap().to_string().contains("busy"));
    drop(a);
    drop(b);
    let name = history.screenshots(0, 1).unwrap()[0].image.clone().unwrap();
    std::fs::remove_file(dir.path().join("images").join(name)).unwrap();
    assert!(reader.image(id).unwrap().is_none());
    assert!(reader.screenshots(0, 501).is_err());
}
