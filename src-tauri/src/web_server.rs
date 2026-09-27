//! Finite loopback operator transport. GLaDOS/seat control is not reachable here.
use crate::{state::AppState, team_auth::AuthenticatedActor, teams, web_auth::BrowserAuth};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; object-src 'none'; worker-src 'none'";
const BODY_LIMIT: usize = 16 * 1024;
#[derive(Clone)]
struct WebState {
    authority: String,
    origin: String,
    auth: Arc<Mutex<BrowserAuth>>,
    app: Arc<Mutex<AppState>>,
    runtime: Arc<crate::daemons::RuntimeOwner>,
    home: PathBuf,
    project: PathBuf,
    ui: PathBuf,
    #[cfg(test)]
    ui_fixture: bool,
}
#[derive(Clone)]
struct Operator(Arc<AuthenticatedActor>);
fn error(status: u16, code: &str, message: &str) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({"code":code,"message":message})),
    )
        .into_response()
}
fn auth_error(status: u16) -> Response {
    error(
        status,
        match status {
            401 => "E_WEB_SESSION_ENDED",
            410 => "E_WEB_EXCHANGE_EXPIRED",
            429 => "E_WEB_RATE_LIMIT",
            _ => "E_WEB_UNAVAILABLE",
        },
        "session unavailable; reopen Aperture",
    )
}
fn header<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    let mut values = headers.get_all(key).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(first)
    }
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    header(headers, "authorization")?.strip_prefix("Bearer ")
}
fn browser_boundary(s: &WebState, h: &HeaderMap, write: bool) -> bool {
    header(h, "sec-fetch-site") == Some("same-origin")
        && (!write && !h.contains_key("origin") || header(h, "origin") == Some(s.origin.as_str()))
}
async fn outer(State(s): State<WebState>, request: Request, next: Next) -> Response {
    let mut response = if header(request.headers(), "host") != Some(s.authority.as_str()) {
        error(421, "E_WEB_HOST", "invalid request authority")
    } else if request.uri().query().is_some() {
        error(400, "E_WEB_REQUEST", "query parameters are not accepted")
    } else {
        next.run(request).await
    };
    let h = response.headers_mut();
    for (key, value) in [
        ("content-security-policy", CSP),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
        ),
        ("cache-control", "no-store"),
    ] {
        h.insert(key, HeaderValue::from_static(value));
    }
    response
}
async fn api_gate(State(s): State<WebState>, mut request: Request, next: Next) -> Response {
    if !browser_boundary(
        &s,
        request.headers(),
        request.method() != Method::GET && request.method() != Method::HEAD,
    ) {
        return error(403, "E_WEB_ORIGIN", "same-origin browser request required");
    }
    let valid = bearer(request.headers())
        .is_some_and(|b| s.auth.lock().map(|mut a| a.valid(b)).unwrap_or(false));
    if !valid {
        return auth_error(401);
    }
    // Only this trusted boundary constructs web authority; body fields cannot.
    request
        .extensions_mut()
        .insert(Operator(Arc::new(AuthenticatedActor::operator_ui())));
    next.run(request).await
}
async fn bootstrap_denied() -> Response {
    // Deliberately no engine/state/body extractor: before domain/JSON parsing.
    error(
        403,
        "E_WEB_AUTHORITY_DENIED",
        "action is not available through the browser operator surface",
    )
}
async fn session(State(s): State<WebState>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let h = request.headers();
    if path == "/session/mint" {
        if h.contains_key("origin") || h.keys().any(|k| k.as_str().starts_with("sec-fetch-")) {
            return error(403, "E_WEB_ORIGIN", "native open request required");
        }
    } else if !browser_boundary(&s, h, true) {
        return error(403, "E_WEB_ORIGIN", "same-origin browser request required");
    }
    let credential = bearer(h).unwrap_or("").to_owned();
    let is_json = header(h, "content-type") == Some("application/json");
    let bytes = match to_bytes(request.into_body(), 1024).await {
        Ok(b) => b,
        Err(_) => return error(413, "E_WEB_BODY", "request body too large"),
    };
    if path != "/session" && !bytes.is_empty() && bytes.as_ref() != b"{}" {
        return error(400, "E_WEB_REQUEST", "unexpected body");
    }
    let mut auth = match s.auth.lock() {
        Ok(a) => a,
        Err(_) => return auth_error(503),
    };
    let result = match path.as_str() {
        "/session/mint" => auth.mint_open(&credential).map(|v| json!({"exchange":v})),
        "/session" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Exchange {
                exchange: String,
            }
            if !is_json {
                return error(415, "E_WEB_REQUEST", "JSON required");
            }
            let input = match serde_json::from_slice::<Exchange>(&bytes) {
                Ok(i) => i,
                Err(_) => return error(400, "E_WEB_REQUEST", "invalid exchange request"),
            };
            auth.redeem(&input.exchange).map(|v| json!({"session":v}))
        }
        "/session/link" => auth.link(&credential).map(|v| json!({"exchange":v})),
        "/session/logout" => {
            if auth.logout(&credential) {
                Ok(json!({"revoked":true}))
            } else {
                Err(401)
            }
        }
        _ => return error(404, "E_WEB_NOT_FOUND", "route not found"),
    };
    match result {
        Ok(v) => Json(v).into_response(),
        Err(code) => auth_error(code),
    }
}

#[derive(Clone, Copy)]
enum Command {
    Version,
    Agents,
    Start,
    Stop,
    Restart,
    Model,
    Attention,
    TmuxSession,
    TmuxSelect,
    Catalog,
    Presets,
    SavePreset,
    Create,
    Teams,
    Cancel,
    Prepare,
    Replace,
    Archive,
    Open,
}
fn bounded(value: &Value, depth: usize) -> bool {
    if depth > 12 {
        return false;
    }
    match value {
        Value::String(s) => s.len() <= 8192,
        Value::Array(a) => a.len() <= 128 && a.iter().all(|v| bounded(v, depth + 1)),
        Value::Object(o) => {
            o.len() <= 32
                && o.iter()
                    .all(|(k, v)| k.len() <= 80 && bounded(v, depth + 1))
        }
        _ => true,
    }
}
fn decode<T: DeserializeOwned>(v: Value) -> Result<T, teams::TeamError> {
    serde_json::from_value(v).map_err(|_| teams::TeamError {
        code: "E_WEB_REQUEST".into(),
        message: "invalid command input".into(),
    })
}
fn exact(v: &Value, fields: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|o| o.keys().all(|k| fields.contains(&k.as_str())))
}
fn wire_error() -> teams::TeamError {
    teams::TeamError {
        code: "E_WEB_REQUEST".into(),
        message: "invalid command input".into(),
    }
}
fn serialize<T: serde::Serialize>(v: T) -> Result<Value, teams::TeamError> {
    serde_json::to_value(v).map_err(|_| wire_error())
}
fn legacy<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, teams::TeamError> {
    r.map_err(|_| teams::TeamError {
        code: "E_WEB_COMMAND_FAILED".into(),
        message: "command could not be completed; refresh before retry".into(),
    })
    .and_then(serialize)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    name: String,
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionName {
    session_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    window_id: String,
}
fn short_selector(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
fn execute(
    s: &WebState,
    actor: &AuthenticatedActor,
    command: Command,
    value: Value,
    work: &crate::daemons::RuntimeWork,
) -> Result<Value, teams::TeamError> {
    work.check_open().map_err(|message| teams::TeamError { code: "E_RUNTIME_UNAVAILABLE".into(), message })?;
    if value
        .get("selection")
        .is_some_and(|v| !exact(v, &["harness", "model", "reasoning"]))
    {
        return Err(wire_error());
    }
    for source in [&value, value.get("preset").unwrap_or(&Value::Null)] {
        if source
            .get("fallbacks")
            .and_then(Value::as_array)
            .is_some_and(|a| {
                a.iter()
                    .any(|v| !exact(v, &["harness", "model", "reasoning"]))
            })
        {
            return Err(wire_error());
        }
    }
    #[cfg(test)]
    if s.ui_fixture {
        match command {
            Command::Agents | Command::Teams => return Ok(json!([])),
            Command::TmuxSession => {
                let v: SessionName = decode(value)?;
                if v.session_name != "aperture" {
                    return Err(wire_error());
                }
                return Ok(json!("fixture"));
            }
            _ => {}
        }
    }
    let engine = teams::TeamEngine::new(s.home.clone(), s.project.clone());
    match command {
        Command::Version => Ok(crate::get_version()),
        Command::Agents => {
            work.require_tools().map_err(|message| teams::TeamError { code: "E_RUNTIME_UNAVAILABLE".into(), message })?;
            legacy(crate::agents::list_agents_shared(&s.app))
        },
        Command::Start | Command::Stop | Command::Restart | Command::Attention => {
            let n: Name = decode(value)?;
            if !short_selector(&n.name) {
                return Err(wire_error());
            }
            legacy(match command {
                Command::Start => crate::agents::start_agent_shared(n.name.clone(), &s.app, &work.lifecycle(&n.name).map_err(|_| wire_error())?),
                Command::Stop => crate::agents::stop_agent_shared(n.name.clone(), &s.app, &work.lifecycle(&n.name).map_err(|_| wire_error())?),
                Command::Restart => crate::agents::restart_agent_shared(n.name.clone(), &s.app, &work.lifecycle(&n.name).map_err(|_| wire_error())?),
                _ => crate::agents::clear_attention_shared(n.name, &s.app, work),
            })
        }
        Command::Model => {
            let m: Model = decode(value)?;
            if !short_selector(&m.name) || m.model.len() > 128 {
                return Err(wire_error());
            }
            legacy(crate::agents::update_agent_model_shared(
                m.name.clone(), m.model, &s.app, &work.lifecycle(&m.name).map_err(|_| wire_error())?,
            ))
        }
        Command::TmuxSession => {
            let v: SessionName = decode(value)?;
            if v.session_name != "aperture" {
                return Err(wire_error());
            }
            legacy(crate::tmux::tmux_create_session_shared(v.session_name, work))
        }
        Command::TmuxSelect => {
            let v: Window = decode(value)?;
            if !v.window_id.starts_with('@')
                || v.window_id.len() < 2
                || v.window_id.len() > 23
                || !v.window_id[1..].bytes().all(|c| c.is_ascii_digit())
            {
                return Err(wire_error());
            }
            legacy(crate::tmux::tmux_select_window_shared(v.window_id, work))
        }
        Command::Catalog => engine.catalog().and_then(serialize),
        Command::Presets => engine.list_presets().and_then(serialize),
        Command::SavePreset => {
            if !exact(&value, &["preset", "expected_sha256"]) {
                return Err(wire_error());
            }
            engine
                .save_preset(actor, decode(value)?)
                .and_then(serialize)
        }
        Command::Create => engine
            .create_team(actor, decode(value)?)
            .and_then(serialize),
        Command::Teams => engine.list_teams().and_then(serialize),
        Command::Cancel => engine
            .cancel_pending(actor, decode(value)?)
            .and_then(serialize),
        Command::Prepare | Command::Replace => {
            let permits = s
                .app
                .lock()
                .map_err(|_| wire_error())?
                .team_preparations
                .clone();
            if matches!(command, Command::Prepare) {
                teams::team_prepare_replacement_shared(decode(value)?, &engine, &permits, actor)
                    .and_then(serialize)
            } else {
                teams::team_start_replacement_shared(decode(value)?, &engine, &permits, actor)
                    .and_then(serialize)
            }
        }
        Command::Archive => teams::inspect_archive(&engine, &decode(value)?).and_then(serialize),
        Command::Open => {
            crate::team_terminal::open_shared(&s.home, decode(value)?).and_then(serialize)
        }
    }
}
async fn command(s: WebState, actor: Operator, cmd: Command, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let read = request.method() == Method::GET || request.method() == Method::HEAD;
    if !read && header(request.headers(), "content-type") != Some("application/json") {
        return error(415, "E_WEB_REQUEST", "JSON required");
    }
    let bytes = match to_bytes(request.into_body(), BODY_LIMIT).await {
        Ok(b) => b,
        Err(_) => return error(413, "E_WEB_BODY", "request body too large"),
    };
    let mut input = if read && bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => return error(400, "E_WEB_REQUEST", "invalid JSON"),
        }
    };
    if !bounded(&input, 0) || !input.is_object() || read && input != json!({}) {
        return error(400, "E_WEB_REQUEST", "invalid command input");
    }
    // Route selectors are authoritative; duplicated body selectors must match.
    let segments: Vec<_> = path.split('/').collect();
    let selectors: Vec<(&str, &str)> = if path.starts_with("/api/agents/") {
        vec![("name", segments[3])]
    } else if path.starts_with("/api/teams/")
        && !matches!(
            cmd,
            Command::Catalog | Command::Presets | Command::SavePreset
        )
    {
        let mut v = vec![("team", segments[3])];
        if matches!(cmd, Command::Open) {
            v.push(("seat", segments[5]));
        }
        v
    } else {
        vec![]
    };
    for (key, value) in selectors {
        if input.get(key).is_some_and(|v| v.as_str() != Some(value)) {
            return error(400, "E_WEB_REQUEST", "selector mismatch");
        }
        input
            .as_object_mut()
            .unwrap()
            .insert(key.into(), Value::String(value.into()));
    }
    let seat = if matches!(cmd, Command::Start | Command::Stop | Command::Restart | Command::Model) {
        input.get("name").and_then(Value::as_str)
    } else { None };
    let work = match s.runtime.admit(seat) {
        Ok(work) => work,
        Err(_) => return error(409, "E_RUNTIME_UNAVAILABLE", "runtime admission refused"),
    };
    match tokio::task::spawn_blocking(move || {
        let _body = work.body().map_err(|message| teams::TeamError { code: "E_RUNTIME_UNAVAILABLE".into(), message })?;
        execute(&s, &actor.0, cmd, input, &work)
    }).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => {
            let safe = e.code.starts_with("E_")
                && e.code.len() <= 80
                && e.code
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
            error(
                if e.code == "E_WEB_REQUEST" { 400 } else { 409 },
                if safe {
                    &e.code
                } else {
                    "E_WEB_COMMAND_FAILED"
                },
                "command could not be completed; refresh before retry",
            )
        }
        Err(_) => error(
            500,
            "E_WEB_OUTCOME_UNKNOWN",
            "command outcome unknown; refresh before retry",
        ),
    }
}
async fn static_file(State(s): State<WebState>, request: Request) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return error(404, "E_WEB_NOT_FOUND", "route not found");
    }
    let raw = request.uri().path();
    let path = if raw == "/" {
        "index.html"
    } else {
        raw.trim_start_matches('/')
    };
    if !path
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(&c))
        || path
            .split('/')
            .any(|p| p.is_empty() || p == ".." || p == ".")
    {
        return error(404, "E_WEB_NOT_FOUND", "route not found");
    }
    let mime = match path.rsplit('.').next() {
        Some("html") if path == "index.html" => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => return error(404, "E_WEB_NOT_FOUND", "route not found"),
    };
    let root = match std::fs::canonicalize(&s.ui) {
        Ok(p) => p,
        Err(_) => return error(404, "E_WEB_NOT_FOUND", "UI build unavailable"),
    };
    let target = match std::fs::canonicalize(root.join(path)) {
        Ok(p) if p.starts_with(&root) => p,
        _ => return error(404, "E_WEB_NOT_FOUND", "route not found"),
    };
    if !std::fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() <= 8 * 1024 * 1024) {
        return error(404, "E_WEB_NOT_FOUND", "route not found");
    }
    match std::fs::read(target) {
        Ok(bytes) => ([("content-type", mime)], bytes).into_response(),
        Err(_) => error(404, "E_WEB_NOT_FOUND", "route not found"),
    }
}
fn router(s: WebState) -> Router {
    let mut api = Router::new();
    for (path, write, cmd) in [
        ("/api/version", false, Command::Version),
        ("/api/agents", false, Command::Agents),
        ("/api/agents/{name}/start", true, Command::Start),
        ("/api/agents/{name}/stop", true, Command::Stop),
        ("/api/agents/{name}/restart", true, Command::Restart),
        ("/api/agents/{name}/model", true, Command::Model),
        (
            "/api/agents/{name}/attention/clear",
            true,
            Command::Attention,
        ),
        ("/api/tmux/session", true, Command::TmuxSession),
        ("/api/tmux/select-window", true, Command::TmuxSelect),
        ("/api/teams/catalog", false, Command::Catalog),
        ("/api/teams/presets", false, Command::Presets),
        ("/api/teams/presets", true, Command::SavePreset),
        ("/api/teams", false, Command::Teams),
        ("/api/teams", true, Command::Create),
        ("/api/teams/{team}/cancel", true, Command::Cancel),
        (
            "/api/teams/{team}/replacement/prepare",
            true,
            Command::Prepare,
        ),
        (
            "/api/teams/{team}/replacement/start",
            true,
            Command::Replace,
        ),
        ("/api/teams/{team}/archive", true, Command::Archive),
        ("/api/teams/{team}/seats/{seat}/open", true, Command::Open),
    ] {
        let handler = move |State(s): State<WebState>,
                            Extension(actor): Extension<Operator>,
                            request: Request| command(s, actor, cmd, request);
        api = api.route(path, if write { post(handler) } else { get(handler) });
    }
    api = api
        .route(
            "/api/teams/{team}/seats/{seat}/bootstrap",
            post(bootstrap_denied),
        )
        .route_layer(middleware::from_fn_with_state(s.clone(), api_gate));
    api.route("/session", post(session))
        .route("/session/mint", post(session))
        .route("/session/link", post(session))
        .route("/session/logout", post(session))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(s.clone(), outer))
        .with_state(s)
}

/// Production composition. Never invokes the Tauri GUI or initializes a database.
pub async fn serve() -> Result<(), String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("home unavailable")?;
    let lease = crate::controller::ControllerLock::acquire(&home)?;
    let capability = crate::web_auth::credential().map_err(|_| "random source unavailable")?;
    let auth = BrowserAuth::new(&capability).map_err(|_| "session initialization failed")?;
    lease.rotate_open_capability(&capability)?;
    let app = Arc::new(Mutex::new(crate::config::default_state()));
    let project = PathBuf::from(
        &app.lock()
            .map_err(|_| "application state unavailable")?
            .project_dir,
    );
    let runtime = Arc::new(crate::daemons::RuntimeOwner::new(lease));
    let s = WebState {
        runtime: runtime.clone(),
        authority: "127.0.0.1:4519".into(),
        origin: "http://127.0.0.1:4519".into(),
        auth: Arc::new(Mutex::new(auth)),
        app: app.clone(),
        ui: home.join(".aperture/ui/current"),
        home,
        project,
        #[cfg(test)]
        ui_fixture: false,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4519")
        .await
        .map_err(|_| "local address unavailable")?;
    runtime.start(app)?;
    let closing = runtime.clone();
    let result = axum::serve(listener, router(s))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            // Close admission before waiting for HTTP request futures. The
            // synchronous collector must not occupy the async worker.
            let _ = tokio::task::spawn_blocking(move || closing.close()).await;
        })
        .await;
    let drain = runtime.clone();
    tokio::task::spawn_blocking(move || drain.close()).await
        .map_err(|_| "E_RUNTIME_DRAIN_INCOMPLETE")??;
    result.map_err(|_| "local server stopped unexpectedly")?;
    // Detach only. This does NOT prove D close-admission/drain/join; startup remains fenced.
    crate::ws_hub::shutdown();
    crate::codex_appserver::shutdown();
    Ok(())
}

/// CLI-only open flow. Capability stays in memory/header, never argv/environment/output.
pub async fn open() -> Result<(), String> {
    use std::io::Read;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("home unavailable")?;
    let root = home.join(".aperture");
    if !root.is_dir() {
        return Err("operator capability unavailable".into());
    }
    crate::journal::ensure_private_dir(&root).map_err(|_| "operator capability unavailable")?;
    let path = crate::journal::validate_component_path(&root, "run/operator.token", false)
        .map_err(|_| "operator capability unavailable")?;
    let mut value = String::new();
    crate::journal::open_private_file_nofollow(&path)
        .map_err(|_| "operator capability unavailable")?
        .take(44)
        .read_to_string(&mut value)
        .map_err(|_| "operator capability unavailable")?;
    if value.len() != 43
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err("operator capability unavailable".into());
    }
    let exchange=tokio::time::timeout(Duration::from_secs(5),async {
        let mut stream=tokio::net::TcpStream::connect("127.0.0.1:4519").await.map_err(|_|())?;
        let request=format!("POST /session/mint HTTP/1.1\r\nHost: 127.0.0.1:4519\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",value);
        stream.write_all(request.as_bytes()).await.map_err(|_|())?;
        let mut bytes=Vec::new(); stream.take(8192).read_to_end(&mut bytes).await.map_err(|_|())?;
        let text=std::str::from_utf8(&bytes).map_err(|_|())?;
        if !text.starts_with("HTTP/1.1 200 ") { return Err(()); }
        let (_,body)=text.split_once("\r\n\r\n").ok_or(())?;
        let data:Value=serde_json::from_str(body).map_err(|_|())?;
        let exchange=data.get("exchange").and_then(Value::as_str).ok_or(())?;
        if exchange.len()!=43 || !exchange.bytes().all(|c|c.is_ascii_alphanumeric()||b"-_".contains(&c)) { return Err(()); }
        Ok(exchange.to_owned())
    }).await.map_err(|_|"open request timed out")?.map_err(|_|"open request failed")?;
    let status = std::process::Command::new("/usr/bin/open")
        .arg(format!("http://127.0.0.1:4519/#t={exchange}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| "browser unavailable")?;
    if status.success() {
        Ok(())
    } else {
        Err("browser unavailable".into())
    }
}

#[cfg(test)]
#[path = "web_server_tests.rs"]
mod tests;
