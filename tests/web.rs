mod common;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use timeline::{
    history::{History, ScreenshotSample},
    queries::{Client, Queries},
    recording::{Input, Recorder, Stamp, Window},
    web::{Web, secret},
};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:47831";
async fn call(
    app: &Router,
    path: &str,
    value: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .method(if value.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("host", HOST);
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    if value.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(value.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 40 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, body)
}
async fn session(app: &Router, web: &Web) -> String {
    let url = web.launch_url().unwrap();
    let (status, headers, _) = call(
        app,
        "/bootstrap",
        Some(json!({"token":url.split('#').nth(1).unwrap()})),
        &[
            ("origin", "http://127.0.0.1:47831"),
            ("x-timeline-request", "1"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = headers["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    cookie.split(';').next().unwrap().into()
}
async fn browser(app: &Router, cookie: &str, operation: &str, args: Value) -> (StatusCode, Value) {
    let (status, _, body) = call(
        app,
        &format!("/api/query/{operation}"),
        Some(args),
        &[
            ("cookie", cookie),
            ("origin", "http://127.0.0.1:47831"),
            ("x-timeline-request", "1"),
        ],
    )
    .await;
    (status, serde_json::from_slice(&body).unwrap())
}
async fn rpc(app: &Router, token: &str, method: &str, params: Value) -> (StatusCode, Value) {
    let (status, _, body) = call(
        app,
        "/mcp",
        Some(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})),
        &[
            ("authorization", token),
            ("accept", "application/json, text/event-stream"),
            ("mcp-protocol-version", "2025-11-25"),
        ],
    )
    .await;
    (status, serde_json::from_slice(&body).unwrap())
}
async fn tool(app: &Router, token: &str, name: &str, args: Value) -> Value {
    rpc(
        app,
        token,
        "tools/call",
        json!({"name":name,"arguments":args}),
    )
    .await
    .1["result"]
        .clone()
}
fn stamp(ms: i64) -> Stamp {
    Stamp {
        unix_ms: ms,
        monotonic_ms: ms,
        boot_ms: ms,
    }
}
fn fixture(path: &std::path::Path) {
    let mut recorder = Recorder::open(path, stamp(1000)).unwrap();
    recorder
        .apply(
            Input::Connected {
                locked: false,
                idle: false,
            },
            stamp(1000),
        )
        .unwrap();
    recorder
        .apply(
            Input::Focus(Some(Window {
                address: "a".into(),
                application: "editor".into(),
                title: "<script>original title</script>".into(),
            })),
            stamp(1000),
        )
        .unwrap();
    recorder.apply(Input::Tick, stamp(5000)).unwrap();
    recorder.apply(Input::Pause, stamp(6000)).unwrap();
    drop(recorder);
}
#[tokio::test]
async fn browser_resolves_activity_day_boundaries_without_loading_evidence() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let web = Web::new(dir.path(), HOST).unwrap();
    let app = web.router();
    let cookie = session(&app, &web).await;
    // Activity days start at 04:00, so the day before each clock change
    // contains the transition. These dates have no recorded evidence.
    for (date, hours) in [("2026-03-28", 23), ("2026-10-24", 25)] {
        let (status, resolved) =
            browser(&app, &cookie, "resolve_range", json!({"date": date})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resolved["timezone"], "Europe/Oslo");
        assert_eq!(
            resolved["range"]["end_ms"].as_i64().unwrap()
                - resolved["range"]["start_ms"].as_i64().unwrap(),
            hours * 3_600_000
        );
        assert!(resolved.get("breakdown").is_none());
    }
    for args in [
        json!({}),
        json!({"date": "not-a-date"}),
        json!({"range": {"start_ms": 2, "end_ms": 1}}),
    ] {
        assert_eq!(
            browser(&app, &cookie, "resolve_range", args).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}
#[tokio::test]
async fn authentication_bootstrap_origin_host_and_read_only_mcp() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let web = Web::new(dir.path(), HOST).unwrap();
    let app = web.router();
    assert_eq!(
        std::fs::metadata(dir.path().join("http.token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        call(&app, "/api/status", None, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "/mcp", None, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "/", None, &[("host", "evil.test")]).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "/", None, &[("origin", "https://evil.test")])
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let url = web.launch_url().unwrap();
    let token = url.split('#').nth(1).unwrap();
    assert!(!url.contains(&secret(dir.path()).unwrap()));
    let headers = [
        ("origin", "http://127.0.0.1:47831"),
        ("x-timeline-request", "1"),
    ];
    assert_eq!(
        call(&app, "/bootstrap", Some(json!({"token":token})), &[])
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "/bootstrap", Some(json!({"token":token})), &headers)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "/bootstrap", Some(json!({"token":token})), &headers)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let cookie = session(&app, &web).await;
    assert_eq!(
        call(
            &app,
            "/api/recording/pause",
            Some(json!({})),
            &[("cookie", &cookie)]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let oversized = json!({"date":"2000-01-01", "cursor":"x".repeat(16384)});
    assert_eq!(
        call(
            &app,
            "/api/query/overview",
            Some(oversized),
            &[
                ("cookie", &cookie),
                ("origin", "http://127.0.0.1:47831"),
                ("x-timeline-request", "1")
            ]
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let bearer = format!("Bearer {}", secret(dir.path()).unwrap());
    assert_eq!(
        call(&app, "/api/status", None, &[("authorization", &bearer)])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "/mcp", None, &[("cookie", &cookie)]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "/mcp", None, &[("authorization", &bearer)])
            .await
            .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        call(
            &app,
            "/mcp",
            None,
            &[("authorization", &bearer), ("origin", "null")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "/mcp",
            None,
            &[("authorization", &bearer), ("mcp-protocol-version", "2099")]
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let init = rpc(&app,&bearer,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}})).await.1;
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    let list = rpc(&app, &bearer, "tools/list", json!({})).await.1;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 6);
    assert!(
        list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true)
    );
    assert_eq!(
        rpc(
            &app,
            &bearer,
            "tools/call",
            json!({"name":"pause","arguments":{}})
        )
        .await
        .1["error"]["code"],
        -32602
    );
    let (status, _, body) = call(
        &app,
        "/mcp",
        Some(json!({"jsonrpc":"2.0","method":"notifications/initialized"})),
        &[
            ("authorization", &bearer),
            ("accept", "application/json, text/event-stream"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.is_empty());
}
#[tokio::test]
async fn adapters_share_paged_history_gaps_empty_days_and_retention_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let web = Web::new(dir.path(), HOST).unwrap();
    let app = web.router();
    let cookie = session(&app, &web).await;
    let bearer = format!("Bearer {}", secret(dir.path()).unwrap());
    let range = json!({"start_ms":0,"end_ms":10000});
    for op in ["overview", "blocks", "observations", "screenshots"] {
        let args = json!({"range":range,"limit":1});
        let (status, b) = browser(&app, &cookie, op, args.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(b, tool(&app, &bearer, op, args).await["structuredContent"]);
    }
    let overview = browser(
        &app,
        &cookie,
        "overview",
        json!({"range":range,"limit":500}),
    )
    .await
    .1;
    assert!(
        overview["breakdown"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["focus"]["kind"] == "gap")
    );
    assert_eq!(
        browser(&app, &cookie, "overview", json!({"date":"2000-01-01"}))
            .await
            .1["totals"]["focus_ms"],
        0
    );
    assert_eq!(
        tool(&app, &bearer, "overview", json!({"date":"2000-01-01"})).await["structuredContent"]["totals"]
            ["focus_ms"],
        0
    );
    let page = browser(
        &app,
        &cookie,
        "observations",
        json!({"range":range,"limit":1}),
    )
    .await
    .1;
    let cursor = &page["next_cursor"];
    assert!(cursor.is_string());
    let args = json!({"range":range,"limit":1,"cursor":cursor});
    assert_eq!(
        browser(&app, &cookie, "observations", args.clone()).await.1,
        tool(&app, &bearer, "observations", args.clone()).await["structuredContent"]
    );
    let mut h = History::open(dir.path()).unwrap();
    h.cleanup_before(6001).unwrap();
    assert_eq!(
        browser(&app, &cookie, "observations", args.clone()).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        tool(&app, &bearer, "observations", args).await["content"][0]["text"],
        "cursor_invalidated"
    );
    assert_eq!(
        tool(
            &app,
            &bearer,
            "overview",
            json!({"range":{"start_ms":0,"end_ms":i64::MAX}})
        )
        .await["isError"],
        true
    );
}
#[tokio::test]
async fn selected_images_missing_files_expiry_and_shared_busy_limits() {
    use base64::Engine;
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let mut h = History::open(dir.path()).unwrap();
    let prepared =
        History::prepare_image(dir.path(), &common::webp(b"selected image bytes")).unwrap();
    let id = h
        .commit_screenshot(
            &ScreenshotSample {
                unix_ms: 2000,
                monotonic_ms: 2000,
                segment: 1,
                monitor: "A".into(),
                width: 2,
                height: 1,
                transform: 0,
            },
            "pixels",
            &prepared,
        )
        .unwrap();
    drop(prepared);
    let web = Web::new(dir.path(), HOST).unwrap();
    let app = web.router();
    let cookie = session(&app, &web).await;
    let bearer = format!("Bearer {}", secret(dir.path()).unwrap());
    let image = tool(&app, &bearer, "image", json!({"id":id})).await;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(image["content"][1]["data"].as_str().unwrap())
            .unwrap(),
        common::webp(b"selected image bytes")
    );
    assert_eq!(
        call(
            &app,
            &format!("/api/image/{id}"),
            None,
            &[("cookie", &cookie)]
        )
        .await
        .2,
        common::webp(b"selected image bytes")
    );
    let q = Queries::open(dir.path()).unwrap();
    let read = q.image(Client::Mcp, id).unwrap();
    assert_eq!(
        tool(&app, &bearer, "overview", json!({"date":"1970-01-01"})).await["content"][0]["text"],
        "busy"
    );
    let second = q.image(Client::Browser, id).unwrap();
    assert_eq!(
        browser(&app, &cookie, "selection", json!({"selected_ms":2000}))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(second);
    h.cleanup_before(6001).unwrap();
    assert_eq!(
        call(
            &app,
            &format!("/api/image/{id}"),
            None,
            &[("cookie", &cookie)]
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    drop(read);
    assert_eq!(
        tool(&app, &bearer, "image", json!({"id":id})).await["content"][0]["text"],
        "unavailable"
    );
    let prepared = History::prepare_image(dir.path(), &common::webp(b"missing bytes")).unwrap();
    let missing = h
        .commit_screenshot(
            &ScreenshotSample {
                unix_ms: 10000,
                monotonic_ms: 10000,
                segment: 1,
                monitor: "B".into(),
                width: 2,
                height: 1,
                transform: 0,
            },
            "missing",
            &prepared,
        )
        .unwrap();
    drop(prepared);
    for entry in std::fs::read_dir(dir.path().join("tiles")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::remove_file(path).unwrap();
        }
    }
    assert_eq!(
        call(
            &app,
            &format!("/api/image/{missing}"),
            None,
            &[("cookie", &cookie)]
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        tool(&app, &bearer, "image", json!({"id":missing})).await["content"][0]["text"],
        "unavailable"
    );
}

#[tokio::test]
async fn browser_pause_acknowledges_a_durable_recorder_transition() {
    use std::{
        io::{BufRead, Write},
        os::unix::net::UnixListener,
    };
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let socket = UnixListener::bind(dir.path().join("control.sock")).unwrap();
    let path = dir.path().to_owned();
    let worker = std::thread::spawn(move || {
        let mut recorder = Recorder::open(&path, stamp(10000)).unwrap();
        for (input, command, when) in [
            (Input::Resume, "resume", 11000),
            (Input::Pause, "pause", 12000),
        ] {
            let (mut stream, _) = socket.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line.trim(), command);
            recorder.apply(input, stamp(when)).unwrap();
            writeln!(stream, "{{\"ok\":true}}").unwrap();
        }
    });
    let web = Web::new(dir.path(), HOST).unwrap();
    let app = web.router();
    let cookie = session(&app, &web).await;
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", "http://127.0.0.1:47831"),
        ("x-timeline-request", "1"),
    ];
    for (command, paused) in [("resume", false), ("pause", true)] {
        assert_eq!(
            call(
                &app,
                &format!("/api/recording/{command}"),
                Some(json!({})),
                &headers
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            History::read(dir.path()).unwrap().manual_paused().unwrap(),
            paused
        );
    }
    worker.join().unwrap();
    let recorder = Recorder::open(dir.path(), stamp(13000)).unwrap();
    assert!(recorder.status().manual_paused);
}

#[tokio::test]
async fn storage_and_previews_are_authenticated_bounded_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let mut h = History::open(dir.path()).unwrap();
    let raw = [10, 20, 200, 255].repeat(1024 * 512);
    let prepared = History::prepare_pixels(dir.path(), 1024, 512, &raw).unwrap();
    let id = h
        .commit_screenshot(
            &ScreenshotSample {
                unix_ms: 2000,
                monotonic_ms: 2000,
                segment: 1,
                monitor: "A".into(),
                width: 1024,
                height: 512,
                transform: 0,
            },
            "preview-fixture",
            &prepared,
        )
        .unwrap();
    drop(prepared);
    let files = std::fs::read_dir(dir.path().join("tiles")).unwrap().count();
    let web = Web::new(dir.path(), HOST).unwrap();
    let app = web.router();
    let preview_path = format!("/api/thumbnail/{id}");
    for path in ["/api/storage", preview_path.as_str()] {
        assert_eq!(
            call(&app, path, None, &[]).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let cookie = session(&app, &web).await;
    let (status, headers, bytes) = call(&app, &preview_path, None, &[("cookie", &cookie)]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/webp");
    assert_eq!(timeline::codec::dimensions(&bytes).unwrap(), (640, 320));
    assert_eq!(
        call(&app, &preview_path, None, &[("cookie", &cookie)])
            .await
            .2,
        bytes
    );
    let (status, _, bytes) = call(&app, "/api/storage", None, &[("cookie", &cookie)]).await;
    assert_eq!(status, StatusCode::OK);
    let data: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["captures"], 1);
    assert_eq!(data["images"], 1);
    assert_eq!(data["regions"], 1);
    assert_eq!(data["map_bytes"], 8);
    assert_eq!(data["map_uncompressed_bytes"], 64);
    assert_eq!(data["tile_reuse_percent"], 87.5);
    assert_eq!(
        data["total_bytes"].as_u64().unwrap(),
        data["pack_bytes"].as_u64().unwrap() + data["database_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        std::fs::read_dir(dir.path().join("tiles")).unwrap().count(),
        files
    );
    assert_eq!(
        call(&app, "/api/storage", None, &[("cookie", &cookie)])
            .await
            .2,
        bytes,
        "snapshot is cached"
    );
    h.cleanup_before(6001).unwrap();
    assert_eq!(
        call(&app, &preview_path, None, &[("cookie", &cookie)])
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
