//! Commit evidence from real temporary git repositories, isolated from the
//! developer's git configuration.
mod common;
isolate_git!();
use axum::http::StatusCode;
use common::git::{commit, init, run};
use common::web::{HOST, browser, rpc, session, tool};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use timeline::{
    history::History,
    queries::{Client, ExactTitle, Queries, QueryError, Range, Selection},
    settings::Settings,
    web::{Web, secret},
};

/// 2026-09-22 04:00 Europe/Oslo, the start of that activity day, in seconds.
const DAY: i64 = 1_790_042_400;

fn queries(history: &Path, settings: &str) -> Queries {
    History::open(history).unwrap();
    Queries::with_policy(history, "Europe/Oslo", Box::new(ExactTitle))
        .unwrap()
        .with_settings(Arc::new(Settings::parse(settings).unwrap()))
}

fn sources(root: &Path, extra: &str) -> String {
    format!("[git]\npaths = [{:?}]\n{extra}", root.display().to_string())
}

fn date(text: &str) -> Selection {
    Selection::ActivityDate(text.parse().unwrap())
}

/// Every file under `directory` with its size and modification time.
fn snapshot(directory: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut files = BTreeMap::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(directory).unwrap().flatten() {
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                files.insert(entry.path(), (metadata.len(), metadata.modified().unwrap()));
            }
        }
    }
    files
}

/// (repository, subject, kind, branch, seconds after 04:00, amends)
fn summary(list: &Value) -> Vec<(String, String, String, Value, i64, u64)> {
    list["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|commit| {
            (
                commit["repository"].as_str().unwrap().into(),
                commit["subject"].as_str().unwrap().into(),
                commit["kind"].as_str().unwrap().into(),
                commit["branch"].clone(),
                commit["time_ms"].as_i64().unwrap() / 1000 - DAY,
                commit["amends"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn reflog_commits_of_the_user_across_nested_repositories_and_worktrees() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let app = root.join("personal/app");
    init(&app);
    commit(&app, DAY + 3600, "first", &[]);
    commit(&app, DAY + 7200, "second", &[]);
    run(
        &app,
        DAY + 7800,
        &["commit", "-q", "--amend", "-m", "second, amended"],
        &[],
    );
    commit(
        &app,
        DAY + 9000,
        "by a colleague",
        &[("GIT_AUTHOR_EMAIL", "colleague@example.com")],
    );
    commit(
        &app,
        DAY + 9500,
        "as an alias",
        &[("GIT_AUTHOR_EMAIL", "Me.Alias@Example.com")],
    );
    run(
        &app,
        DAY + 10_000,
        &["checkout", "-q", "-b", "feature"],
        &[],
    );
    let feature = commit(&app, DAY + 10_800, "feature work", &[]);
    commit(&app, DAY + 11_400, "feature polish", &[]);
    run(&app, DAY + 12_000, &["checkout", "-q", "main"], &[]);
    // A cherry-pick keeps the author date but is made, and shown, later.
    run(&app, DAY + 14_400, &["cherry-pick", &feature], &[]);
    // Rebasing rewrites existing commits; the rewritten copies are not counted.
    run(&app, DAY + 18_000, &["checkout", "-q", "feature"], &[]);
    run(&app, DAY + 18_000, &["rebase", "-q", "main"], &[]);
    let vendored = app.join("vendor/lib");
    init(&vendored);
    commit(&vendored, DAY + 21_600, "vendored", &[]);
    let dependency = app.join("node_modules/package");
    init(&dependency);
    commit(&dependency, DAY + 21_700, "dependency", &[]);
    std::os::unix::fs::symlink(root, root.join("personal/loop")).unwrap();

    let site = root.join("clients/acme/site");
    init(&site);
    commit(
        &site,
        DAY + 25_000,
        "home identity in a client repository",
        &[("GIT_AUTHOR_EMAIL", "me@home.example")],
    );
    commit(&site, DAY + 25_200, "client fix", &[]);
    run(
        &site,
        DAY + 26_000,
        &["worktree", "add", "-q", "-b", "review", "../site-review"],
        &[],
    );
    commit(
        &root.join("clients/acme/site-review"),
        DAY + 28_800,
        "review fix",
        &[],
    );
    commit(&site, DAY + 86_400 + 3600, "next day", &[]);

    std::fs::create_dir_all(root.join("broken")).unwrap();
    std::fs::write(root.join("broken/.git"), "gitdir: ../missing\n").unwrap();
    std::fs::create_dir_all(root.join("mount/.git")).unwrap();

    let history = tempfile::tempdir().unwrap();
    let queries = queries(
        history.path(),
        &sources(root, "authors = ['me.alias@example.com']"),
    );
    let before = snapshot(root);
    let list =
        serde_json::to_value(queries.commits(Client::Mcp, date("2026-09-22")).unwrap()).unwrap();
    let main = json!("main");
    let s = |text: &str| text.to_owned();
    assert_eq!(
        summary(&list),
        [
            (
                s("personal/app"),
                s("first"),
                s("commit"),
                main.clone(),
                3600,
                0
            ),
            (
                s("personal/app"),
                s("second, amended"),
                s("commit"),
                main.clone(),
                7800,
                1
            ),
            (
                s("personal/app"),
                s("as an alias"),
                s("commit"),
                main.clone(),
                9500,
                0
            ),
            (
                s("personal/app"),
                s("feature work"),
                s("commit"),
                json!("feature"),
                10_800,
                0
            ),
            (
                s("personal/app"),
                s("feature polish"),
                s("commit"),
                json!("feature"),
                11_400,
                0
            ),
            (
                s("personal/app"),
                s("feature work"),
                s("cherry_pick"),
                main.clone(),
                14_400,
                0
            ),
            (
                s("personal/app/vendor/lib"),
                s("vendored"),
                s("commit"),
                main.clone(),
                21_600,
                0
            ),
            (
                s("clients/acme/site"),
                s("client fix"),
                s("commit"),
                main.clone(),
                25_200,
                0
            ),
            (
                s("clients/acme/site"),
                s("review fix"),
                s("commit"),
                json!("review"),
                28_800,
                0
            ),
        ]
    );
    let amended = &list["commits"][1];
    assert_eq!(amended["first_ms"], (DAY + 7200) * 1000);
    assert_eq!(amended["timing"], "reflog");
    assert_eq!(amended["short_hash"].as_str().unwrap().len(), 7);
    assert!(list["commits"][0].get("first_ms").is_none());
    assert_eq!(list["configured"], true);
    assert_eq!(list["repositories"], 3);
    assert_eq!(
        list["days"],
        json!([{"date":"2026-09-22","range":{"start_ms":DAY*1000,"end_ms":(DAY+86_400)*1000},"commits":9}])
    );
    let problems = list["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0]["repository"], "broken");
    assert!(
        problems[0]["problem"]
            .as_str()
            .unwrap()
            .contains("broken .git file")
    );

    let week = queries
        .commits(
            Client::Browser,
            Selection::ActivityDates {
                first: "2026-09-21".parse().unwrap(),
                last: "2026-09-27".parse().unwrap(),
            },
        )
        .unwrap();
    let counts: Vec<usize> = week.days.iter().map(|day| day.commits).collect();
    assert_eq!(counts, [0, 9, 1, 0, 0, 0, 0]);
    let range = queries
        .commits(
            Client::Browser,
            Selection::Range(Range {
                start_ms: (DAY + 10_000) * 1000,
                end_ms: (DAY + 14_400) * 1000,
            }),
        )
        .unwrap();
    assert_eq!(range.commits.len(), 2, "end-exclusive range");

    let repositories = serde_json::to_value(queries.repositories(false)).unwrap();
    let names: Vec<&str> = repositories["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repository| repository["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "clients/acme/site",
            "personal/app",
            "personal/app/vendor/lib"
        ]
    );
    assert_eq!(
        repositories["repositories"][0]["path"],
        site.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(repositories["authors"], json!(["me.alias@example.com"]));
    assert_eq!(repositories["depth"], 8);
    assert_eq!(snapshot(root), before, "reading modifies no repository");
}

#[test]
fn unreadable_repositories_are_reported_without_failing_the_others() {
    let root = tempfile::tempdir().unwrap();
    let readable = root.path().join("readable");
    let unreadable = root.path().join("unreadable");
    for repository in [&readable, &unreadable] {
        init(repository);
        commit(repository, DAY + 3600, "work", &[]);
    }
    let logs = unreadable.join(".git/logs/HEAD");
    std::fs::set_permissions(&logs, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    if std::fs::File::open(&logs).is_ok() {
        return; // Permissions do not apply, e.g. as root.
    }
    let history = tempfile::tempdir().unwrap();
    let list = queries(history.path(), &sources(root.path(), ""))
        .commits(Client::Browser, date("2026-09-22"))
        .unwrap();
    std::fs::set_permissions(&logs, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    assert_eq!(list.commits.len(), 1);
    assert_eq!(list.commits[0].repository, "readable");
    assert_eq!(list.problems.len(), 1);
    assert_eq!(list.problems[0].repository, "unreadable");
    assert!(
        list.problems[0].problem.contains("Permission denied"),
        "{:?}",
        list.problems
    );
}

#[test]
fn repositories_without_reflogs_fall_back_to_committer_dates() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("no-reflog");
    init(&repository);
    run(
        &repository,
        0,
        &["config", "core.logAllRefUpdates", "false"],
        &[],
    );
    commit(&repository, DAY + 3600, "without a reflog", &[]);
    commit(
        &repository,
        DAY + 3700,
        "by a colleague",
        &[("GIT_AUTHOR_EMAIL", "colleague@example.com")],
    );
    assert!(!repository.join(".git/logs/HEAD").exists());
    let history = tempfile::tempdir().unwrap();
    let list = serde_json::to_value(
        queries(history.path(), &sources(root.path(), ""))
            .commits(Client::Browser, date("2026-09-22"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(list["commits"].as_array().unwrap().len(), 1);
    assert_eq!(list["commits"][0]["subject"], "without a reflog");
    assert_eq!(list["commits"][0]["timing"], "committer_date");
    assert_eq!(list["commits"][0]["time_ms"], (DAY + 3600) * 1000);
    assert_eq!(list["commits"][0]["branch"], Value::Null);
}

#[test]
fn commit_queries_validate_ranges_and_report_settings() {
    let history = tempfile::tempdir().unwrap();
    let unconfigured = queries(history.path(), "");
    let list = unconfigured
        .commits(Client::Mcp, date("2026-09-22"))
        .unwrap();
    assert!(!list.configured && list.commits.is_empty() && list.problems.is_empty());
    assert_eq!(list.days.len(), 1);
    assert!(unconfigured.repositories(false).repositories.is_empty());
    let too_long = unconfigured
        .commits(
            Client::Mcp,
            Selection::Range(Range {
                start_ms: DAY * 1000,
                end_ms: (DAY + 32 * 86_400) * 1000,
            }),
        )
        .unwrap_err();
    assert_eq!(too_long.downcast_ref(), Some(&QueryError::InvalidRequest));

    let missing = queries(
        history.path(),
        "[git]\npaths = ['/nonexistent/timeline-test']\n",
    )
    .commits(Client::Mcp, date("2026-09-22"))
    .unwrap();
    assert_eq!(missing.problems.len(), 1);
    assert_eq!(missing.problems[0].repository, "/nonexistent/timeline-test");

    for invalid in ["[git]\npaths = ['relative']\n", "[git]\npath = ['/x']\n"] {
        assert!(Settings::parse(invalid).is_err(), "{invalid}");
    }
}

#[tokio::test]
async fn browser_and_mcp_share_commits_from_configured_repositories() {
    let dir = tempfile::tempdir().unwrap();
    History::open(dir.path()).unwrap();
    let repositories = tempfile::tempdir().unwrap();
    let repository = repositories.path().join("project");
    init(&repository);
    // 2026-09-22 10:00 Europe/Oslo.
    commit(&repository, 1_790_064_000, "fix the parser", &[]);
    let config = tempfile::tempdir().unwrap();
    let settings = config.path().join("config.toml");
    std::fs::write(
        &settings,
        format!(
            "[git]\npaths = [{:?}]\n",
            repositories.path().display().to_string()
        ),
    )
    .unwrap();
    let web = Web::configured(dir.path(), HOST, Some(settings.clone())).unwrap();
    let app = web.router();
    let cookie = session(&app, &web).await;
    let bearer = format!("Bearer {}", secret(dir.path()).unwrap());
    let tools = rpc(&app, &bearer, "tools/list", json!({})).await.1;
    let schema = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "commits")
        .unwrap()["inputSchema"]
        .clone();
    assert!(schema["properties"]["dates"].is_object() && schema["properties"]["range"].is_object());

    let args = json!({"dates":{"first":"2026-09-21","last":"2026-09-23"}});
    let list = tool(&app, &bearer, "commits", args.clone()).await["structuredContent"].clone();
    assert_eq!(list, browser(&app, &cookie, "commits", args).await.1);
    assert_eq!(list["commits"][0]["repository"], "project");
    assert_eq!(list["commits"][0]["subject"], "fix the parser");
    assert_eq!(list["commits"][0]["branch"], "main");
    let counts: Vec<&Value> = list["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|day| &day["commits"])
        .collect();
    assert_eq!(counts, [&json!(0), &json!(1), &json!(0)]);

    let found = tool(&app, &bearer, "repositories", json!({})).await["structuredContent"].clone();
    assert_eq!(found["repositories"][0]["name"], "project");
    assert_eq!(found["paths"][0], repositories.path().display().to_string());
    let (status, refreshed) = browser(&app, &cookie, "repositories", json!({"refresh":true})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(refreshed["repositories"], found["repositories"]);
    let invalid = rpc(
        &app,
        &bearer,
        "tools/call",
        json!({"name":"repositories","arguments":{"refresh":true}}),
    )
    .await
    .1;
    assert_eq!(invalid["error"]["code"], -32602, "refresh is browser-only");
    let error = tool(
        &app,
        &bearer,
        "commits",
        json!({"range":{"start_ms":0,"end_ms":40_i64 * 86_400_000}}),
    )
    .await;
    assert_eq!(
        (
            error["isError"].clone(),
            error["content"][0]["text"].clone()
        ),
        (json!(true), json!("invalid_request"))
    );

    // An invalid git section is reported like any invalid settings file.
    std::fs::write(&settings, "[git]\npaths = ['relative']\n").unwrap();
    let list =
        tool(&app, &bearer, "commits", json!({"date":"2026-09-22"})).await["structuredContent"]
            .clone();
    assert_eq!(list["configured"], false);
    assert!(
        list["settings_problem"]
            .as_str()
            .unwrap()
            .contains("relative")
    );
}
