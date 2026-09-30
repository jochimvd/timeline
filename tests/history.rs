use serde_json::json;
use timeline::history::History;

#[test]
fn committed_observations_survive_reopening_with_stable_identifiers() {
    let directory = tempfile::tempdir().unwrap();
    let id = {
        let mut history = History::open(directory.path()).unwrap();
        history
            .append(
                1_000,
                20,
                1,
                json!({"kind":"focus", "title":"Design, notes"}),
            )
            .unwrap()
    };
    let history = History::open(directory.path()).unwrap();
    let observations = history.observations(0, 100).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].id, id);
    assert_eq!(observations[0].evidence["title"], "Design, notes");
    assert!(history.observations(id, 100).unwrap().is_empty());
}

#[test]
fn only_one_writer_is_allowed_but_readers_can_inspect_and_retention_never_reuses_ids() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = History::open(directory.path()).unwrap();
    assert!(History::open(directory.path()).is_err());
    let old_id = writer.append(100, 10, 1, json!({"kind":"focus"})).unwrap();
    let reader = History::read(directory.path()).unwrap();
    assert_eq!(reader.observations(0, 100).unwrap().len(), 1);
    writer.cleanup_before(200).unwrap();
    assert!(reader.observations(0, 100).unwrap().is_empty());
    let new_id = writer.append(300, 30, 1, json!({"kind":"focus"})).unwrap();
    assert!(new_id > old_id);
    assert!(reader.observations(0, 501).is_err());
}

#[test]
fn large_evidence_pages_are_bounded_without_losing_pagination() {
    let directory = tempfile::tempdir().unwrap();
    let mut history = History::open(directory.path()).unwrap();
    history
        .append(100, 1, 1, json!({"title": "a".repeat(600_000)}))
        .unwrap();
    history
        .append(200, 2, 1, json!({"title": "b".repeat(600_000)}))
        .unwrap();
    let first = history.observations(0, 500).unwrap();
    assert_eq!(first.len(), 1);
    let next = history.observations(first[0].id, 500).unwrap();
    assert_eq!(next.len(), 1);
    assert!(next[0].id > first[0].id);
}

#[test]
fn unsupported_old_history_requires_explicit_migration_without_modifying_data() {
    let directory = tempfile::tempdir().unwrap();
    let old = rusqlite::Connection::open(directory.path().join("history.sqlite3")).unwrap();
    old.execute_batch("CREATE TABLE evidence(value TEXT); INSERT INTO evidence VALUES('retained'); PRAGMA user_version=1;").unwrap();
    assert!(History::open(directory.path()).is_err());
    assert!(History::read(directory.path()).is_err());
    assert_eq!(
        old.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        old.query_row("SELECT value FROM evidence", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "retained"
    );
}
