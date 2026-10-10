//! Authenticated loopback adapters. All evidence semantics remain in Queries.
use crate::queries::{Client, PageRequest, Queries, QueryError, Range, Selection};
use crate::settings::Settings;
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path as AxumPath, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

pub const ADDRESS: &str = crate::config::HTTP_ADDRESS;
const VERSIONS: &[&str] = &["2025-03-26", "2025-06-18", "2025-11-25"];
const IMAGE_LIMIT: u64 = 24 * 1024 * 1024;
const MCP_IMAGE_EDGE: u32 = 1920;

struct Credentials {
    bootstrap: HashMap<String, Instant>,
    sessions: HashMap<String, Instant>,
}
struct SelectionWork {
    generation: u64,
    cancelled: Weak<AtomicBool>,
}
pub struct Web {
    directory: PathBuf,
    /// Settings file with title rules, categories and git paths; analysis uses none without one.
    settings: Option<PathBuf>,
    authority: String,
    secret: String,
    credentials: Mutex<Credentials>,
    work: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    selections: Mutex<HashMap<(String, String), SelectionWork>>,
    storage: Mutex<Option<(Instant, Value)>>,
}
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
/// Persist the bearer credential without ever putting it in a URL or log.
pub fn secret(directory: &Path) -> Result<String> {
    let path = directory.join("http.token");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(random_token()?.as_bytes())?;
            file.sync_all()?;
            File::open(directory)?.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.permissions().mode() & 0o077 == 0,
        "http.token must be a user-only regular file"
    );
    let mut token = String::new();
    Read::by_ref(&mut file)
        .take(65)
        .read_to_string(&mut token)?;
    ensure!(
        token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid http.token"
    );
    Ok(token)
}
impl Web {
    pub fn new(directory: &Path, authority: &str) -> Result<Arc<Self>> {
        Self::configured(directory, authority, None)
    }
    pub fn configured(
        directory: &Path,
        authority: &str,
        settings: Option<PathBuf>,
    ) -> Result<Arc<Self>> {
        ensure!(
            authority
                .parse::<std::net::SocketAddr>()?
                .ip()
                .is_loopback(),
            "HTTP must use loopback"
        );
        Ok(Arc::new(Self {
            directory: directory.into(),
            settings,
            authority: authority.into(),
            secret: secret(directory)?,
            credentials: Mutex::new(Credentials {
                bootstrap: HashMap::new(),
                sessions: HashMap::new(),
            }),
            work: Arc::new(Semaphore::new(4)),
            requests: Arc::new(Semaphore::new(16)),
            selections: Mutex::new(HashMap::new()),
            storage: Mutex::new(None),
        }))
    }
    fn selection_cancellation(&self, headers: &HeaderMap) -> Result<Option<Arc<AtomicBool>>> {
        let Some(selection) = header(headers, "x-timeline-selection") else {
            return Ok(None);
        };
        let (tab, generation) = selection.rsplit_once(':').ok_or_else(invalid)?;
        ensure!(
            !tab.is_empty()
                && tab.len() <= 64
                && tab.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            QueryError::InvalidRequest
        );
        let generation: u64 = generation.parse().map_err(|_| invalid())?;
        let session = session_cookie(headers).ok_or_else(invalid)?;
        let flag = Arc::new(AtomicBool::new(false));
        let mut selections = self.selections.lock().unwrap();
        // Only active work retains flags, so the map is bounded by admission.
        selections.retain(|_, previous| previous.cancelled.strong_count() != 0);
        let key = (session.to_owned(), tab.to_owned());
        if let Some(previous) = selections.get(&key) {
            if generation < previous.generation {
                flag.store(true, Ordering::Relaxed);
                return Ok(Some(flag));
            }
            if let Some(previous) = previous.cancelled.upgrade() {
                previous.store(true, Ordering::Relaxed);
            }
        }
        selections.insert(
            key,
            SelectionWork {
                generation,
                cancelled: Arc::downgrade(&flag),
            },
        );
        Ok(Some(flag))
    }
    pub fn launch_url(&self) -> Result<String> {
        let token = random_token()?;
        let mut credentials = self.credentials.lock().unwrap();
        credentials
            .bootstrap
            .retain(|_, until| *until > Instant::now());
        ensure!(
            credentials.bootstrap.len() < 16,
            "too many pending browser launches"
        );
        credentials
            .bootstrap
            .insert(token.clone(), Instant::now() + Duration::from_secs(60));
        Ok(format!("http://{}/#{}", self.authority, token))
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(
                "/",
                get(|| async {
                    (
                        [("content-type", "text/html; charset=utf-8")],
                        include_str!("../web/index.html"),
                    )
                }),
            )
            .route(
                "/app.js",
                get(|| async {
                    (
                        [("content-type", "text/javascript")],
                        include_str!("../web/app.js"),
                    )
                }),
            )
            .route(
                "/style.css",
                get(|| async {
                    (
                        [("content-type", "text/css")],
                        include_str!("../web/style.css"),
                    )
                }),
            )
            .route(
                "/favicon.svg",
                get(|| async {
                    (
                        [("content-type", "image/svg+xml")],
                        include_str!("../web/favicon.svg"),
                    )
                }),
            )
            .route("/bootstrap", post(bootstrap))
            .route("/api/query/{operation}", post(query))
            .route("/api/image/{id}", get(image))
            .route("/api/tiles/{id}", get(tiles))
            .route("/api/thumbnail/{id}", get(thumbnail))
            .route("/api/storage", get(storage))
            .route("/api/status", get(status))
            .route(
                "/api/config",
                get(|State(state): State<Arc<Web>>| async move {
                    Json(json!({
                        "timezone": crate::config::activity_timezone(),
                        "settings": state.settings,
                    }))
                }),
            )
            .route("/api/recording/{command}", post(recording))
            .route(
                "/mcp",
                post(mcp).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
            )
            .layer(DefaultBodyLimit::max(16 * 1024))
            .layer(middleware::from_fn_with_state(self.clone(), guard))
            .with_state(self.clone())
    }
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    if headers.get_all(name).iter().count() != 1 {
        return None;
    }
    headers.get(name)?.to_str().ok()
}
fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    header(headers, "cookie")?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("timeline_session="))
}
fn failure(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}
fn error(error: anyhow::Error) -> Response {
    let (status, code) = error_code(&error);
    failure(status, code)
}
fn error_code(error: &anyhow::Error) -> (StatusCode, &'static str) {
    match error.downcast_ref::<QueryError>() {
        Some(QueryError::Busy) => (StatusCode::SERVICE_UNAVAILABLE, "busy"),
        Some(QueryError::Unavailable) => (StatusCode::NOT_FOUND, "unavailable"),
        Some(QueryError::CursorInvalidated) => (StatusCode::CONFLICT, "cursor_invalidated"),
        Some(QueryError::LimitExceeded) => (StatusCode::UNPROCESSABLE_ENTITY, "limit_exceeded"),
        Some(QueryError::InvalidRequest) => (StatusCode::BAD_REQUEST, "invalid_request"),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}
async fn guard(State(state): State<Arc<Web>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let origin = format!("http://{}", state.authority);
    if header(headers, "host") != Some(state.authority.as_str())
        || (headers.contains_key("origin") && header(headers, "origin") != Some(origin.as_str()))
    {
        return failure(StatusCode::FORBIDDEN, "invalid_host_or_origin");
    }
    let path = request.uri().path();
    let public = matches!(
        path,
        "/" | "/app.js" | "/style.css" | "/favicon.svg" | "/bootstrap"
    );
    if path == "/mcp" {
        if header(headers, "authorization") != Some(format!("Bearer {}", state.secret).as_str()) {
            return failure(StatusCode::UNAUTHORIZED, "unauthorized");
        }
        if header(headers, "mcp-protocol-version").is_some_and(|v| !VERSIONS.contains(&v)) {
            return failure(StatusCode::BAD_REQUEST, "unsupported_protocol_version");
        }
    } else {
        if !public {
            let session = session_cookie(headers);
            let mut credentials = state.credentials.lock().unwrap();
            credentials
                .sessions
                .retain(|_, until| *until > Instant::now());
            if !session.is_some_and(|s| credentials.sessions.contains_key(s)) {
                return failure(StatusCode::UNAUTHORIZED, "unauthorized");
            }
        }
        if request.method() != Method::GET
            && (header(headers, "origin") != Some(origin.as_str())
                || header(headers, "x-timeline-request") != Some("1"))
        {
            return failure(StatusCode::FORBIDDEN, "csrf");
        }
    }
    let Ok(_request_permit) = state.requests.clone().try_acquire_owned() else {
        return error(QueryError::Busy.into());
    };
    let mut response = match tokio::time::timeout(Duration::from_secs(15), next.run(request)).await
    {
        Ok(response) => response,
        Err(_) => failure(StatusCode::REQUEST_TIMEOUT, "request_timeout"),
    };
    for (key, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(key, HeaderValue::from_static(value));
    }
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    token: String,
}
async fn bootstrap(State(state): State<Arc<Web>>, Json(input): Json<Bootstrap>) -> Response {
    let mut credentials = state.credentials.lock().unwrap();
    if !credentials
        .bootstrap
        .remove(&input.token)
        .is_some_and(|until| until > Instant::now())
    {
        return failure(StatusCode::UNAUTHORIZED, "invalid_bootstrap");
    }
    credentials
        .sessions
        .retain(|_, until| *until > Instant::now());
    if credentials.sessions.len() >= 16 {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "session_limit");
    }
    let token = match random_token() {
        Ok(token) => token,
        Err(e) => return error(e),
    };
    credentials.sessions.insert(
        token.clone(),
        Instant::now() + Duration::from_secs(12 * 3600),
    );
    let mut response = Json(json!({"ok":true})).into_response();
    response.headers_mut().insert(
        "set-cookie",
        format!("timeline_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200")
            .parse()
            .unwrap(),
    );
    response
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arguments {
    date: Option<String>,
    dates: Option<Dates>,
    range: Option<Range>,
    cursor: Option<String>,
    limit: Option<usize>,
    display_points: Option<usize>,
    direction: Option<i32>,
    since: Option<String>,
    monitor: Option<String>,
    selected_ms: Option<i64>,
    id: Option<i64>,
    max_edge: Option<u32>,
    refresh: Option<bool>,
}
/// Consecutive activity dates, both inclusive.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dates {
    first: String,
    last: String,
}
fn invalid() -> anyhow::Error {
    QueryError::InvalidRequest.into()
}
fn evidence(
    web: &Web,
    client: Client,
    operation: &str,
    a: Arguments,
    cancelled: Option<Arc<AtomicBool>>,
) -> Result<Value> {
    let mut queries = Queries::open(&web.directory)?.cancel_when(cancelled);
    if let Some(settings) = &web.settings {
        queries = queries.with_settings(Settings::load(settings));
    }
    let page = PageRequest {
        cursor: a.cursor,
        limit: a.limit,
    };
    let date = |date: &str| date.parse().map_err(|_| invalid());
    let selection = || -> Result<Selection> {
        match (&a.date, &a.dates, a.range) {
            (Some(day), None, None) => Ok(Selection::ActivityDate(date(day)?)),
            (None, Some(dates), None) => Ok(Selection::ActivityDates {
                first: date(&dates.first)?,
                last: date(&dates.last)?,
            }),
            (None, None, Some(range)) => Ok(Selection::Range(range)),
            _ => Err(invalid()),
        }
    };
    Ok(match operation {
        "resolve_range" => serde_json::to_value(queries.resolve(selection()?)?)?,
        "categories" => serde_json::to_value(queries.categories())?,
        "overview" => serde_json::to_value(queries.overview_display(
            client,
            selection()?,
            page,
            a.display_points,
        )?)?,
        "overview_updates" => serde_json::to_value(queries.overview_updates(
            client,
            selection()?,
            page,
            a.display_points,
            a.since.as_deref().ok_or_else(invalid)?,
        )?)?,
        "focus_metrics" => serde_json::to_value(queries.focus_metrics(client, selection()?)?)?,
        "commits" => serde_json::to_value(queries.commits(client, selection()?)?)?,
        "repositories" => serde_json::to_value(queries.repositories(a.refresh.unwrap_or(false)))?,
        "activity_bounds" => serde_json::to_value(queries.activity_bounds(client, selection()?)?)?,
        "adjacent_screenshot" => serde_json::to_value(queries.adjacent_screenshot(
            client,
            a.selected_ms.ok_or_else(invalid)?,
            a.direction.ok_or_else(invalid)?,
            a.monitor.as_deref(),
        )?)?,
        "blocks" => {
            serde_json::to_value(queries.blocks(client, a.range.ok_or_else(invalid)?, page)?)?
        }
        "observations" => serde_json::to_value(queries.observations(
            client,
            a.range.ok_or_else(invalid)?,
            page,
        )?)?,
        "observation" => {
            serde_json::to_value(queries.observation(client, a.id.ok_or_else(invalid)?)?)?
        }
        "screenshots" => serde_json::to_value(queries.screenshots(
            client,
            a.range.ok_or_else(invalid)?,
            a.monitor.as_deref(),
            page,
        )?)?,
        "selection" => serde_json::to_value(queries.cursor_evidence(
            client,
            a.selected_ms.ok_or_else(invalid)?,
            a.monitor.as_deref(),
        )?)?,
        _ => return Err(invalid()),
    })
}
async fn blocking(
    state: Arc<Web>,
    work: impl FnOnce(&Path) -> Result<Value> + Send + 'static,
) -> Response {
    let Ok(permit) = state.work.clone().try_acquire_owned() else {
        return error(QueryError::Busy.into());
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(&state.directory)
    })
    .await
    {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => error(e),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "worker_failed"),
    }
}
async fn query(
    State(state): State<Arc<Web>>,
    AxumPath(operation): AxumPath<String>,
    headers: HeaderMap,
    Json(a): Json<Arguments>,
) -> Response {
    let cancelled = if operation == "selection" {
        match state.selection_cancellation(&headers) {
            Ok(flag) => flag,
            Err(e) => return error(e),
        }
    } else {
        None
    };
    let web = state.clone();
    blocking(state, move |_| {
        evidence(&web, Client::Browser, &operation, a, cancelled)
    })
    .await
}
async fn status(State(state): State<Arc<Web>>) -> Response {
    blocking(state, |directory| {
        Ok(serde_json::from_str(&crate::daemon::request(
            directory, "status",
        )?)?)
    })
    .await
}
async fn recording(State(state): State<Arc<Web>>, AxumPath(command): AxumPath<String>) -> Response {
    if !matches!(command.as_str(), "pause" | "resume") {
        return error(invalid());
    }
    blocking(state, move |directory| {
        Ok(serde_json::from_str(&crate::daemon::request(
            directory, &command,
        )?)?)
    })
    .await
}
async fn image(State(state): State<Arc<Web>>, AxumPath(id): AxumPath<i64>) -> Response {
    image_response(state, id, crate::tiles::ImageFormat::Original).await
}
async fn tiles(State(state): State<Arc<Web>>, AxumPath(id): AxumPath<i64>) -> Response {
    image_response(state, id, crate::tiles::ImageFormat::Bundle).await
}
async fn thumbnail(State(state): State<Arc<Web>>, AxumPath(id): AxumPath<i64>) -> Response {
    image_response(state, id, crate::tiles::ImageFormat::Thumbnail).await
}
async fn storage(State(state): State<Arc<Web>>) -> Response {
    {
        let cache = state.storage.lock().unwrap();
        if let Some((_, value)) = cache
            .as_ref()
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(30))
        {
            return Json(value.clone()).into_response();
        }
    }
    let cache = state.clone();
    blocking(state, move |directory| {
        let value = crate::storage::snapshot(directory)?;
        *cache.storage.lock().unwrap() = Some((Instant::now(), value.clone()));
        Ok(value)
    })
    .await
}
async fn image_response(state: Arc<Web>, id: i64, format: crate::tiles::ImageFormat) -> Response {
    let Ok(permit) = state.work.clone().try_acquire_owned() else {
        return error(QueryError::Busy.into());
    };
    let result = tokio::task::spawn_blocking(move || {
        let queries = Queries::open(&state.directory)?;
        let retrieved = match format {
            crate::tiles::ImageFormat::Bundle => queries.tiles(Client::Browser, id)?,
            crate::tiles::ImageFormat::Original => queries.image(Client::Browser, id)?,
            crate::tiles::ImageFormat::Thumbnail => queries.thumbnail(Client::Browser, id)?,
            crate::tiles::ImageFormat::Preview(edge) => {
                queries.preview(Client::Browser, id, edge)?
            }
        };
        Ok::<_, anyhow::Error>((retrieved, permit))
    })
    .await;
    let (mut retrieved, permit) = match result {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return error(e),
        Err(_) => return failure(StatusCode::INTERNAL_SERVER_ERROR, "worker_failed"),
    };
    let (sender, receiver) = tokio::sync::mpsc::channel::<std::io::Result<Bytes>>(2);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        loop {
            let mut bytes = vec![0; 64 * 1024];
            match retrieved.stream.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.truncate(n);
                    if sender.blocking_send(Ok(Bytes::from(bytes))).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = sender.blocking_send(Err(e));
                    break;
                }
            }
        }
    });
    (
        [(
            "content-type",
            if format == crate::tiles::ImageFormat::Bundle {
                "application/vnd.timeline.tiles"
            } else {
                "image/webp"
            },
        )],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receiver)),
    )
        .into_response()
}

fn tool_definitions() -> Value {
    let range = json!({"type":"object","properties":{"start_ms":{"type":"integer"},"end_ms":{"type":"integer"}},"required":["start_ms","end_ms"],"additionalProperties":false});
    let page =
        json!({"cursor":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":500}});
    let zone = crate::config::activity_timezone();
    let dates = json!({"type":"object","description":"Consecutive activity dates, both inclusive, at most 31","properties":{"first":{"type":"string","format":"date"},"last":{"type":"string","format":"date"}},"required":["first","last"],"additionalProperties":false});
    let overview = format!(
        "Overview of an activity date (04:00 {zone}), consecutive dates or half-open range, at \
        most 31 days. Page all breakdown rows: category totals (every configured category in \
        tree order with parent and depth, then uncategorized as category null; elapsed_ms \
        includes subcategories, own_ms is the category's own matches), application and exact-title totals \
        (titles carry their category), then focus and screenshot coverage. Focus includes idle; \
        gaps are unknown, not task time. `metrics` summarizes the range as in focus_metrics."
    );
    let metrics = format!(
        "Per activity day (04:00 {zone}) of a date, consecutive dates (e.g. a week) or half-open \
        range, at most 31 days: focus totals, focus by category (its own matches, not \
        subcategories; roll up via parent) and by application (focus_ms includes idle, active_ms excludes it), evidence, and focus metrics. evidence splits the \
        day into observed time, recording gaps (with reasons) and unknown time up to now; a day \
        without observed evidence is missing evidence, not zero activity. active_ms is non-idle \
        focus. Context switches count application (and, with categories, category) changes \
        between consecutive focus within continuous non-idle evidence; idle, gaps, pause/lock \
        and timing boundaries never count. Focus stretches are continuous active focus on one \
        category (else application); glances away under {}s are bridged while they stay within \
        {}% of a stretch. longest_stretch has a time range to inspect; stretches buckets long \
        >=25 min, medium 10-25, short <10; hours holds hourly active time and switches. Days add \
        up except longest_stretch (take the maximum) and rates (recompute from switches and \
        active_ms). To compare periods, use equal lengths and say where either lacks evidence. \
        Observed focus patterns only: not task time and not a productivity judgement.",
        crate::queries::BRIEF_INTERRUPTION_MS / 1000,
        crate::queries::MAX_INTERRUPTED_PERCENT,
    );
    let commits = format!(
        "The user's own git commits in a date (activity day from 04:00 {zone}), consecutive dates \
        or half-open range, at most 31 days, from repositories under the configured git paths, \
        with commits per activity day. Each has its repository (path relative to the configured \
        path), hash, subject, branch and time_ms. timing says where the time comes from: the \
        reflog, recording when the commit was made in this clone, or for repositories without \
        reflogs the committer date. Only commits made here count (commit, amend, merge, \
        cherry-pick, revert, am): rebased copies, fast-forwards and fetched commits do not. An \
        amended commit appears once, at its final version, with amends and first_ms. Commits are \
        read live from the repositories, not recorded: evidence of work on a repository at a \
        moment, not task time. problems lists repositories that could not be read; their commits \
        are missing, not absent."
    );
    let tools: Vec<Value> = [
        ("overview", overview.as_str(), vec![]),
        ("focus_metrics", metrics.as_str(), vec![]),
        ("commits", commits.as_str(), vec![]),
        ("repositories", "The configured git paths and extra author emails, and the repositories found under them: name (relative to its configured path), path, and repositories or paths that could not be read. Repository names help relate commits and windows to projects.", vec![]),
        ("categories", "The user's category definitions: ordered rules matching an exact application and/or a regex over the cleaned window title; the first matching rule in configuration order applies, at any level. A category may have a parent, making it a subcategory; its time rolls up to every ancestor. Categories classify observed focus; they are hints, not task attribution.", vec![]),
        ("blocks", "Activity blocks in a half-open range up to 48 hours, each with its category (null when uncategorized). Stable block and observation references; grouping is not task attribution.", vec!["range"]),
        ("observations", "Original window observations in a range up to 48 hours. Page before inferring tasks.", vec!["range"]),
        ("observation", "Retrieve one original observation by its stable ID; expired evidence is unavailable.", vec!["id"]),
        ("screenshots", "List screenshot metadata in a range up to 48 hours, optionally by monitor. Retrieve only selected images.", vec!["range"]),
        ("image", "Retrieve one selected screenshot as a lossy WebP downscaled to at most max_edge pixels on its longest edge (default 1920, never upscaled). The text content carries the original metadata: actual capture timestamp, monitor and native size. No replacement of expired evidence.", vec!["id"]),
    ].into_iter().map(|(name, description, required)| {
        let mut properties = serde_json::Map::new();
        if matches!(name, "overview" | "blocks" | "observations" | "screenshots") {
            properties.extend(page.as_object().unwrap().clone()); properties.insert("range".into(), range.clone());
        }
        if matches!(name, "focus_metrics" | "commits") { properties.insert("range".into(), range.clone()); }
        if matches!(name, "overview" | "focus_metrics" | "commits") {
            properties.insert("date".into(), json!({"type":"string","format":"date"}));
            properties.insert("dates".into(), dates.clone());
        }
        if name == "screenshots" { properties.insert("monitor".into(), json!({"type":"string"})); }
        if matches!(name, "image" | "observation") { properties.insert("id".into(), json!({"type":"integer"})); }
        if name == "image" { properties.insert("max_edge".into(), json!({"type":"integer","minimum":64,"maximum":4096})); }
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":true,"destructiveHint":false,"openWorldHint":false}})
    }).collect();
    json!({"tools":tools})
}
fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}
async fn mcp(State(state): State<Arc<Web>>, headers: HeaderMap, body: Bytes) -> Response {
    let accepts = header(&headers, "accept").unwrap_or("");
    if !accepts.contains("application/json") || !accepts.contains("text/event-stream") {
        return failure(StatusCode::NOT_ACCEPTABLE, "accept_json_and_event_stream");
    }
    if header(&headers, "content-type")
        .is_none_or(|v| v.split(';').next() != Some("application/json"))
    {
        return failure(StatusCode::UNSUPPORTED_MEDIA_TYPE, "expected_json");
    }
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return rpc_error(Value::Null, -32700, "Parse error"),
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || !value.is_object()
        || (!id.is_null() && !id.is_string() && !id.is_number())
    {
        return rpc_error(Value::Null, -32600, "Invalid Request");
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return rpc_error(id, -32600, "Invalid Request");
    };
    if value.get("id").is_none() {
        return if matches!(
            method,
            "notifications/initialized" | "notifications/cancelled"
        ) {
            StatusCode::ACCEPTED.into_response()
        } else {
            failure(StatusCode::BAD_REQUEST, "unsupported_notification")
        };
    }
    let result = match method {
        "initialize" => {
            let Some(version) = value
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
            else {
                return rpc_error(id, -32602, "Missing protocolVersion");
            };
            json!({"protocolVersion":if VERSIONS.contains(&version) {version} else {"2025-11-25"},"capabilities":{"tools":{}},"serverInfo":{"name":"timeline","version":env!("CARGO_PKG_VERSION")},"instructions":"Read metadata first. Focus is not task time; categories are the user's rule-based focus classification, useful hints but not task attribution. Focus metrics describe observed switching and sustained focus, not productivity. Git commits are the user's own, read live from their repositories: evidence of work at a moment, not task time. Keep ambiguity unassigned and gaps unknown; a day without observed evidence is missing evidence, not zero activity. Do not double count monitors; cite evidence IDs. Tasks and corrections belong in conversation."})
        }
        "ping" => json!({}),
        "tools/list" => tool_definitions(),
        "tools/call" => {
            let Some(name) = value.pointer("/params/name").and_then(Value::as_str) else {
                return rpc_error(id, -32602, "Missing tool name");
            };
            if !matches!(
                name,
                "overview"
                    | "focus_metrics"
                    | "commits"
                    | "repositories"
                    | "categories"
                    | "blocks"
                    | "observations"
                    | "observation"
                    | "screenshots"
                    | "image"
            ) {
                return rpc_error(id, -32602, "Unknown read-only tool");
            }
            let definitions = tool_definitions();
            let schema = &definitions["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .unwrap()["inputSchema"];
            let raw = value
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or(json!({}));
            if raw.as_object().is_none_or(|object| {
                object
                    .keys()
                    .any(|key| schema["properties"].get(key).is_none())
                    || schema["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|key| !object.contains_key(key.as_str().unwrap()))
            }) {
                return rpc_error(id, -32602, "Invalid arguments");
            }
            let arguments: Arguments = match serde_json::from_value(
                value
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(json!({})),
            ) {
                Ok(a) => a,
                Err(_) => return rpc_error(id, -32602, "Invalid arguments"),
            };
            let Ok(permit) = state.work.clone().try_acquire_owned() else {
                return Json(json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":"busy"}]}})).into_response();
            };
            let name = name.to_owned();
            match tokio::task::spawn_blocking(move || -> Result<Value> {
                let _permit = permit;
                if name == "image" {
                    // Model clients downscale large images anyway; send a bounded rendition.
                    let mut image = Queries::open(&state.directory)?.preview(Client::Mcp, arguments.id.ok_or_else(invalid)?, arguments.max_edge.unwrap_or(MCP_IMAGE_EDGE))?;
                    let mut bytes = Vec::new();
                    image.stream.by_ref().take(IMAGE_LIMIT + 1).read_to_end(&mut bytes)?;
                    ensure!(bytes.len() as u64 <= IMAGE_LIMIT, QueryError::LimitExceeded);
                    Ok(json!({"content":[{"type":"text","text":serde_json::to_string(&image.screenshot)?},{"type":"image","mimeType":"image/webp","data":base64::engine::general_purpose::STANDARD.encode(bytes)}]}))
                } else {
                    let value = evidence(&state, Client::Mcp, &name, arguments, None)?;
                    Ok(json!({"content":[{"type":"text","text":serde_json::to_string(&value)?}],"structuredContent":value}))
                }
            }).await {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => json!({"isError":true,"content":[{"type":"text","text":error_code(&e).1}]}),
                Err(_) => return rpc_error(id, -32603, "Worker failed"),
            }
        }
        _ => return rpc_error(id, -32601, "Method not found"),
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

/// Bind before returning so the caller can report an unavailable endpoint. The
/// server stops once `stop` is set; its own failure never stops recording.
pub fn start(
    state: Arc<Web>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<Result<()>>> {
    let listener = std::net::TcpListener::bind(&state.authority)
        .with_context(|| format!("cannot bind HTTP endpoint {}", state.authority))?;
    listener.set_nonblocking(true)?;
    Ok(std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(6)
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            axum::serve(listener, state.router())
                .with_graceful_shutdown(async move {
                    while !stop.load(Ordering::Relaxed) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                })
                .await?;
            Ok(())
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    #[test]
    fn newer_selections_cancel_only_the_same_session_and_tab() {
        let dir = tempfile::tempdir().unwrap();
        let web = Web::new(dir.path(), ADDRESS).unwrap();
        let request = |session: &str, tab: &str, generation: u64| {
            let mut headers = HeaderMap::new();
            headers.insert(
                "cookie",
                format!("timeline_session={session}").parse().unwrap(),
            );
            headers.insert(
                "x-timeline-selection",
                format!("{tab}:{generation}").parse().unwrap(),
            );
            web.selection_cancellation(&headers).unwrap().unwrap()
        };
        let old = request("session-a", "tab-a", 1);
        let other_tab = request("session-a", "tab-b", 1);
        let other_session = request("session-b", "tab-a", 1);
        let latest = request("session-a", "tab-a", 3);
        assert!(old.load(Ordering::Relaxed));
        assert!(!latest.load(Ordering::Relaxed));
        assert!(!other_tab.load(Ordering::Relaxed));
        assert!(!other_session.load(Ordering::Relaxed));
        let late_arrival = request("session-a", "tab-a", 2);
        assert!(late_arrival.load(Ordering::Relaxed));
        assert!(!latest.load(Ordering::Relaxed));
        drop((old, latest, other_tab, other_session, late_arrival));
        let _current = request("session-a", "tab-c", 1);
        assert_eq!(web.selections.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn bootstrap_expiry_and_session_expiry_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let web = Web::new(dir.path(), ADDRESS).unwrap();
        let url = web.launch_url().unwrap();
        let token = url.split('#').nth(1).unwrap();
        web.credentials
            .lock()
            .unwrap()
            .bootstrap
            .insert(token.into(), Instant::now() - Duration::from_secs(1));
        let request = Request::builder()
            .method("POST")
            .uri("/bootstrap")
            .header("host", ADDRESS)
            .header("origin", format!("http://{ADDRESS}"))
            .header("content-type", "application/json")
            .header("x-timeline-request", "1")
            .body(Body::from(json!({"token":token}).to_string()))
            .unwrap();
        assert_eq!(
            web.router().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        web.credentials
            .lock()
            .unwrap()
            .sessions
            .insert("expired".into(), Instant::now() - Duration::from_secs(1));
        let request = Request::builder()
            .uri("/api/config")
            .header("host", ADDRESS)
            .header("cookie", "timeline_session=expired")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            web.router().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
