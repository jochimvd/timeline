//! Requests to the authenticated loopback adapters.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use timeline::web::Web;
use tower::ServiceExt;

pub const HOST: &str = "127.0.0.1:47831";
pub async fn call(
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
pub async fn session(app: &Router, web: &Web) -> String {
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
pub async fn browser(
    app: &Router,
    cookie: &str,
    operation: &str,
    args: Value,
) -> (StatusCode, Value) {
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
pub async fn rpc(app: &Router, token: &str, method: &str, params: Value) -> (StatusCode, Value) {
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
pub async fn tool(app: &Router, token: &str, name: &str, args: Value) -> Value {
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
