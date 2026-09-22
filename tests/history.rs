use timeline::history::History;
use serde_json::json;

#[test]
fn committed_observations_survive_reopening_with_stable_identifiers() {
    let directory = tempfile::tempdir().unwrap();
    let id = {
        let mut history = History::open(directory.path()).unwrap();
        history.append(1_000, 20, 1, json!({"kind":"focus", "title":"Design, notes"})).unwrap()
    };
    let history = History::open(directory.path()).unwrap();
    let observations = history.observations(0, 100).unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].id, id);
    assert_eq!(observations[0].evidence["title"], "Design, notes");
    assert!(history.observations(id, 100).unwrap().is_empty());
}
