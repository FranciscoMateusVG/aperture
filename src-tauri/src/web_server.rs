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
    shutdown: tokio::sync::watch::Sender<bool>,
    authority: String,
    origin: String,
    auth: Arc<Mutex<BrowserAuth>>,
    app: Arc<Mutex<AppState>>,
    runtime: Arc<crate::daemons::RuntimeOwner>,
    home: PathBuf,
    project: PathBuf,
    ui: PathBuf,
    ui_builds: Arc<Mutex<std::collections::BTreeMap<String, ui_files::Build>>>,
    #[cfg(test)]
    test_counts: Arc<[std::sync::atomic::AtomicUsize; 2]>,
    #[cfg(test)]
    test_schema: Arc<std::sync::atomic::AtomicUsize>,
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
fn api_schema(s: &WebState) -> &'static str {
    #[cfg(test)]
    if s.test_schema.load(std::sync::atomic::Ordering::SeqCst)==2 { return "2"; }
    let _ = s;
    "1"
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
    let path = request.uri().path();
    if path != "/api/version" && !path.ends_with("/bootstrap")
        && header(request.headers(), "x-aperture-api-schema") != Some(api_schema(&s)) {
        return error(409, "E_WEB_API_INCOMPATIBLE", "UI/API incompatible; reload Aperture");
    }
    // Only this trusted boundary constructs web authority; body fields cannot.
    request
        .extensions_mut()
        .insert(Operator(Arc::new(AuthenticatedActor::operator_ui())));
    let mut response = next.run(request).await;
    response.headers_mut().insert("x-aperture-api-schema", HeaderValue::from_static(api_schema(&s)));
    response
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
    if matches!(path.as_str(), "/session/mint" | "/session/status" | "/session/shutdown") {
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
    if matches!(path.as_str(), "/session/status" | "/session/shutdown") {
        if !auth.native_operator(&credential) { return auth_error(401); }
        if path == "/session/shutdown" { s.shutdown.send_replace(true); }
        return Json(json!({"state": if *s.shutdown.borrow() { "stopping" } else { "running" }})).into_response();
    }
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
/// Closed allowlist of legacy (`Result<T, String>`) error codes that may cross
/// the wire as the discriminant (aperture-fr859). Everything else collapses to
/// `E_WEB_COMMAND_FAILED`, so the UI can distinguish e.g. a lifecycle refusal
/// from a tmux failure without any raw detail (paths, indices, config) leaking.
const LEGACY_ERROR_CODES: &[&str] = &[
    "E_CODEX_HOME_UNVERIFIED",
    "E_CODEX_LAUNCH_INPUTS_UNVERIFIED",
    "E_CODEX_WAIT_UNKNOWN",
    "E_COORDINATOR_SELF_STOP",
    "E_LIFECYCLE_DESCENDANTS_UNVERIFIED",
    "E_LIFECYCLE_OUTCOME_UNKNOWN",
    "E_LIFECYCLE_PROCESS_UNKNOWN",
    "E_LOCAL_PROMPT_UNAVAILABLE",
    "E_LOCAL_THREAD_UNVERIFIED",
    "E_LOCAL_TOOL_MISSING",
    "E_RUNTIME_SELECTOR",
    "E_TMUX_OUTCOME_UNKNOWN",
    "E_TMUX_UNVERIFIED",
];
const LEGACY_ERROR_MESSAGE: &str = "command could not be completed; refresh before retry";
/// Legacy errors are `CODE` or `CODE: detail`. Match only an exact allowlisted
/// CODE followed by end-of-string or ':'; the detail is never inspected or
/// reflected, and the message is always the fixed string.
fn legacy_error_code(raw: &str) -> &'static str {
    LEGACY_ERROR_CODES
        .iter()
        .copied()
        .find(|code| matches!(raw.strip_prefix(code), Some(rest) if rest.is_empty() || rest.starts_with(':')))
        .unwrap_or("E_WEB_COMMAND_FAILED")
}
fn legacy<T: serde::Serialize>(r: Result<T, String>) -> Result<Value, teams::TeamError> {
    r.map_err(|raw| teams::TeamError {
        code: legacy_error_code(&raw).into(),
        message: LEGACY_ERROR_MESSAGE.into(),
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
            legacy(crate::agents::list_agents_local(&s.app,work))
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
            crate::team_terminal::open_shared(&s.home, decode(value)?, work).and_then(serialize)
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
    #[cfg(test)]
    s.test_counts[0].fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let work = match s.runtime.admit(seat) {
        Ok(work) => work,
        Err(_) => return error(409, "E_RUNTIME_UNAVAILABLE", "runtime admission refused"),
    };
    match tokio::task::spawn_blocking(move || {
        let _body = work.body().map_err(|message| teams::TeamError { code: "E_RUNTIME_UNAVAILABLE".into(), message })?;
        #[cfg(test)]
        s.test_counts[1].fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    let result = s.ui_builds.lock().ok().and_then(|mut builds|
        ui_files::serve(&s.ui, request.uri().path(), &mut builds).ok());
    match result {
        Some((mime, bytes)) => ([("content-type", mime)], if request.method() == Method::HEAD { vec![] } else { bytes }).into_response(),
        None => error(404, "E_WEB_NOT_FOUND", "UI build unavailable"),
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
        .route("/session/status", post(session))
        .route("/session/shutdown", post(session))
        .route("/session/link", post(session))
        .route("/session/logout", post(session))
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(s.clone(), outer))
        .with_state(s)
}

/// Production composition. Never invokes the Tauri GUI or initializes a database.
fn local_package_paths(executable:&std::path::Path)->Result<(PathBuf,PathBuf,PathBuf),String>{
    let package=crate::local_package::root_for_executable(executable)?;
    let ui=package.join("ui");
    if !std::fs::symlink_metadata(&ui).is_ok_and(|m|m.is_dir()&&!m.file_type().is_symlink()){return Err("E_LOCAL_UI_MISSING".into());}
    let bus=package.join(crate::local_package::BUS);
    let sentry=package.join(crate::local_package::SENTRY);
    for path in [&bus,&sentry,&package.join(crate::local_package::HUB)]{
        let m=std::fs::symlink_metadata(path).map_err(|_|"E_LOCAL_MCP_OUTPUT_MISSING")?;
        if !m.is_file()||m.file_type().is_symlink(){return Err("E_LOCAL_MCP_OUTPUT_MISSING".into());}
    }
    Ok((ui,bus,sentry))
}

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
    let tools=crate::daemons::LocalTools::resolve(&home)?;
    let executable=std::env::current_exe().map_err(|_|"E_LOCAL_PACKAGE")?;
    let (ui,bus,sentry)=local_package_paths(&executable)?;
    {
        let mut state=app.lock().map_err(|_|"E_RUNTIME_STATE")?;
        state.mcp_server_path=bus.to_string_lossy().into_owned();
        state.mcp_sentry_server_path=sentry.to_string_lossy().into_owned();
    }
    let runtime = Arc::new(crate::daemons::RuntimeOwner::local(lease,tools)?);
    let (shutdown, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let s = WebState {
        shutdown,
        runtime: runtime.clone(),
        authority: "127.0.0.1:4519".into(),
        origin: "http://127.0.0.1:4519".into(),
        auth: Arc::new(Mutex::new(auth)),
        app: app.clone(),
        ui,
        ui_builds: Arc::new(Mutex::new(Default::default())),
        home,
        project,
        #[cfg(test)]
        ui_fixture: false,
        #[cfg(test)]
        test_counts: Arc::new(Default::default()),
        #[cfg(test)]
        test_schema: Arc::new(std::sync::atomic::AtomicUsize::new(1)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4519")
        .await
        .map_err(|_| "local address unavailable")?;
    runtime.start(app)?;
    let closing = runtime.clone();
    let result = axum::serve(listener, router(s))
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = shutdown_rx.wait_for(|stop| *stop) => {},
            }
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

/// CLI capabilities stay in memory/header, never argv/environment/output.
fn operator_capability(home: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let root = home.join(".aperture");
    crate::controller::private_dir_readonly(&root).map_err(|_| "operator capability unavailable")?;
    let path = crate::journal::validate_component_path(&root, "run/operator.token", false)
        .map_err(|_| "operator capability unavailable")?;
    let mut value = String::new();
    crate::journal::open_private_file_nofollow(&path)
        .map_err(|_| "operator capability unavailable")?.take(44).read_to_string(&mut value)
        .map_err(|_| "operator capability unavailable")?;
    if value.len() != 43 || !value.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c)) {
        return Err("operator capability unavailable".into());
    }
    Ok(value)
}
#[derive(Clone, Copy)]
enum NativeControl { Open, Status, Stop }
async fn native_control(home: &std::path::Path, authority: &str, action: NativeControl) -> Result<Option<Value>, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = match tokio::net::TcpStream::connect(authority).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(None),
            Err(_) => return Err("local server status unavailable".into()),
        };
        let capability = operator_capability(home)?;
        let path = match action { NativeControl::Open => "mint", NativeControl::Status => "status", NativeControl::Stop => "shutdown" };
        let request = format!("POST /session/{path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {capability}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.map_err(|_| "native request failed")?;
        let mut bytes = Vec::new();
        stream.take(8193).read_to_end(&mut bytes).await.map_err(|_| "native response failed")?;
        if bytes.len() > 8192 { return Err("native response exceeds limit".into()); }
        let text = std::str::from_utf8(&bytes).map_err(|_| "native response invalid")?;
        if !text.starts_with("HTTP/1.1 200 ") { return Err("native request refused; reopen or check the server version".into()); }
        let (_,body) = text.split_once("\r\n\r\n").ok_or("native response invalid")?;
        let value = serde_json::from_str(body).map_err(|_| "native response invalid")?;
        Ok(Some(value))
    }).await.map_err(|_| "native request timed out".to_string())?
}
fn local_home() -> Result<PathBuf, String> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| "home unavailable".into())
}
async fn server_status(home: &std::path::Path, authority: &str) -> Result<&'static str, String> {
    match native_control(home, authority, NativeControl::Status).await? {
        None if crate::controller::available(home)? => Ok("stopped"),
        None => Err("controller busy; server may be starting or draining".into()),
        Some(v) => match v.get("state").and_then(Value::as_str) {
            Some("running") => Ok("running"), Some("stopping") => Ok("stopping"),
            _ => Err("native status invalid".into()),
        }
    }
}
pub async fn status() -> Result<&'static str, String> { server_status(&local_home()?, "127.0.0.1:4519").await }
/// One authenticated shutdown request, then read-only observation. Never sends
/// a process signal or retries a mutation; hub/agent lifetimes are unchanged.
pub async fn stop() -> Result<(), String> {
    let home = local_home()?;
    if let Some(v) = native_control(&home, "127.0.0.1:4519", NativeControl::Stop).await? {
        if v.get("state").and_then(Value::as_str) != Some("stopping") { return Err("shutdown not acknowledged".into()); }
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        if matches!(server_status(&home, "127.0.0.1:4519").await, Ok("stopped")) { return Ok(()); }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("shutdown not confirmed; no retry or force-stop was attempted".into())
}
pub async fn open() -> Result<(), String> {
    let v = native_control(&local_home()?, "127.0.0.1:4519", NativeControl::Open).await?
        .ok_or("local server is stopped")?;
    let exchange = v.get("exchange").and_then(Value::as_str).ok_or("open response invalid")?;
    if exchange.len()!=43 || !exchange.bytes().all(|c| c.is_ascii_alphanumeric()||b"-_".contains(&c)) { return Err("open response invalid".into()); }
    let status = std::process::Command::new("/usr/bin/open")
        .arg(format!("http://127.0.0.1:4519/#t={exchange}"))
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map_err(|_| "browser unavailable")?;
    if status.success() { Ok(()) } else { Err("browser unavailable".into()) }
}

#[cfg(test)]
#[path = "web_server_tests.rs"]
mod tests;

// UI-only read descriptor. No launch authority, E1 runtime schema or publisher.
// Uses the same native openat/nofollow/fdopendir idiom as the RO release reader.
mod ui_files {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{collections::{BTreeMap, BTreeSet}, ffi::{CStr, CString}, fs::{File, Metadata}, io::Read,
        os::{fd::{AsRawFd, FromRawFd, IntoRawFd}, unix::fs::MetadataExt}, path::{Component, Path}};
    type R<T> = Result<T, ()>;
    const FLAGS: i32 = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Entry { path: String, bytes: u64, sha256: String }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Manifest { schema_version: u32, ui_id: String, api_schema: u32, files: Vec<Entry> }
    #[derive(Clone, PartialEq, Eq)]
    struct Pin(u64,u64,u32,u32,u64,u64,i64,i64,i64,i64);
    fn pin(m: &Metadata) -> Pin { Pin(m.dev(),m.ino(),m.uid(),m.mode(),m.nlink(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()) }
    fn anchor(a: &Pin, b: &Pin) -> bool { (a.0,a.1,a.2,a.3)==(b.0,b.1,b.2,b.3) }
    fn hex(v: &str, n: usize) -> bool { v.len()==n && v.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) }
    fn relative(v: &str) -> bool {
        !v.is_empty() && v.len()<=512 && v.split('/').count()<=8 && v.split('/').all(|p|
            !p.is_empty() && p.len()<=128 && p!="." && p!=".." && p.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)))
    }
    fn mime(v: &str) -> Option<&'static str> {
        match v.rsplit('.').next()? {
            "html" if v=="index.html" => Some("text/html; charset=utf-8"),
            "js"=>Some("text/javascript; charset=utf-8"),"css"=>Some("text/css; charset=utf-8"),
            "svg"=>Some("image/svg+xml"),"png"=>Some("image/png"),"woff2"=>Some("font/woff2"),_=>None
        }
    }
    fn open(parent: &File, name: &str, dir: bool) -> R<File> {
        let name=CString::new(name).map_err(|_|())?;
        let fd=unsafe{libc::openat(parent.as_raw_fd(),name.as_ptr(),FLAGS | if dir {libc::O_DIRECTORY}else{0})};
        if fd<0 {Err(())}else{Ok(unsafe{File::from_raw_fd(fd)})}
    }
    fn directory(fd: &File) -> R<Pin> {
        let m=fd.metadata().map_err(|_|())?;
        if !m.is_dir() || m.uid()!=unsafe{libc::geteuid()} || m.mode()&0o7022!=0 || m.mode()&0o500!=0o500 {return Err(());}
        Ok(pin(&m))
    }
    // Every ancestor checked without canonicalize-following a symlink. System
    // /private/tmp is the sole explicit root-owned sticky temporary exception.
    fn root(path: &Path) -> R<File> {
        if !path.is_absolute(){return Err(());}
        let raw=unsafe{libc::open(c"/".as_ptr(),FLAGS|libc::O_DIRECTORY)};
        if raw<0{return Err(());} let mut fd=unsafe{File::from_raw_fd(raw)};
        let mut here=PathBuf::from("/");
        for part in path.components().skip(1) {
            let Component::Normal(name)=part else{return Err(());};
            let before=pin(&fd.metadata().map_err(|_|())?);
            let next=open(&fd,name.to_str().ok_or(())?,true)?;
            if !anchor(&before,&pin(&fd.metadata().map_err(|_|())?)){return Err(());}
            fd=next;here.push(name);
            let m=fd.metadata().map_err(|_|())?;
            let tmp=here==Path::new("/private/tmp") && m.uid()==0 && m.mode()&0o7777==0o1777;
            if !m.is_dir() || (m.uid()!=0 && m.uid()!=unsafe{libc::geteuid()}) || (!tmp && m.mode()&0o7022!=0){return Err(());}
        }
        directory(&fd)?;Ok(fd)
    }
    fn read(parent: &File, name: &str, cap: u64) -> R<(Vec<u8>,Pin)> {
        let mut fd=open(parent,name,false)?;let m=fd.metadata().map_err(|_|())?;
        if !m.is_file() || m.uid()!=unsafe{libc::geteuid()} || m.nlink()!=1 || m.mode()&0o7022!=0 || m.mode()&0o400==0 || m.len()>cap{return Err(());}
        let before=pin(&m);let mut bytes=Vec::new();(&mut fd).take(cap+1).read_to_end(&mut bytes).map_err(|_|())?;
        if bytes.len() as u64>cap || bytes.len() as u64!=m.len() || pin(&fd.metadata().map_err(|_|())?)!=before{return Err(());}
        let rebound=open(parent,name,false)?;
        if pin(&rebound.metadata().map_err(|_|())?)!=before{return Err(());}
        Ok((bytes,before))
    }
    fn names(fd: &File, remaining: usize) -> R<Vec<String>> {
        let copy=open(fd,".",true)?.into_raw_fd();let raw=unsafe{libc::fdopendir(copy)};
        if raw.is_null(){unsafe{libc::close(copy)};return Err(());}
        struct Dir(*mut libc::DIR);impl Drop for Dir {fn drop(&mut self){unsafe{libc::closedir(self.0)};}}
        let dir=Dir(raw);let mut out=Vec::new();
        loop {
            unsafe{*libc::__error()=0;}
            let ent=unsafe{libc::readdir(dir.0)};
            if ent.is_null(){if unsafe{*libc::__error()}!=0{return Err(());}break;}
            let name=unsafe{CStr::from_ptr((*ent).d_name.as_ptr())}.to_str().map_err(|_|())?;
            if name=="." || name==".."{continue;}
            if out.len()>=remaining{return Err(());}out.push(name.to_owned());
        }
        out.sort();Ok(out)
    }
    fn parse(bytes: &[u8], id: &str) -> R<Manifest> {
        let m:Manifest=serde_json::from_slice(bytes).map_err(|_|())?;
        if m.schema_version!=1 || m.api_schema!=1 || m.ui_id!=id || !hex(id,32) || m.files.is_empty() || m.files.len()>512{return Err(());}
        let mut aliases=BTreeMap::new();let mut leaves=BTreeSet::new();let mut total=0u64;
        for (i,e) in m.files.iter().enumerate() {
            if !relative(&e.path) || mime(&e.path).is_none() || e.path.eq_ignore_ascii_case("UI.json") || !hex(&e.sha256,64) || e.bytes>8*1024*1024 || (e.path=="index.html" && e.bytes>256*1024) || (i>0 && m.files[i-1].path>=e.path){return Err(());}
            total=total.checked_add(e.bytes).filter(|v|*v<=32*1024*1024).ok_or(())?;
            leaves.insert(e.path.to_ascii_lowercase());
            let mut prefix=String::new();
            for part in e.path.split('/') {
                if !prefix.is_empty(){prefix.push('/');}prefix.push_str(part);
                if let Some(old)=aliases.insert(prefix.to_ascii_lowercase(),prefix.clone()){if old!=prefix{return Err(());}}
            }
        }
        for e in &m.files {for (i,_) in e.path.match_indices('/') {if leaves.contains(&e.path[..i].to_ascii_lowercase()){return Err(());}}}
        if !m.files.iter().any(|e|e.path=="index.html"){return Err(());}Ok(m)
    }
    fn inventory(fd:&File, prefix:&str, m:&Manifest, out:&mut BTreeMap<String,Pin>, selected:&str, content:&mut Option<Vec<u8>>) -> R<()> {
        let initial=directory(fd)?;
        for name in names(fd,4096usize.checked_sub(out.len()).ok_or(())?)? {
            let rel=format!("{prefix}{name}");if !relative(&rel) || out.len()>=4096{return Err(());}
            if rel=="UI.json" {let (_,p)=read(fd,&name,128*1024)?;out.insert(rel,p);continue;}
            if let Some(e)=m.files.iter().find(|e|e.path==rel) {
                let (bytes,p)=read(fd,&name,8*1024*1024)?;
                if e.bytes!=bytes.len() as u64 || format!("{:x}",Sha256::digest(&bytes))!=e.sha256{return Err(());}
                if rel==selected {*content=Some(bytes);}out.insert(rel,p);
            } else if m.files.iter().any(|e|e.path.starts_with(&(rel.clone()+"/"))) {
                let dir=open(fd,&name,true)?;let p=directory(&dir)?;out.insert(rel.clone(),p.clone());
                inventory(&dir,&(rel+"/"),m,out,selected,content)?;
                if directory(&open(fd,&name,true)?)?!=p{return Err(());}
            } else {return Err(());}
        }
        if directory(fd)?!=initial{return Err(());}Ok(())
    }
    pub(super) struct Build {fd:File, root_pin:Pin, manifest:Manifest, pins:BTreeMap<String,Pin>, valid:bool}
    impl Build {
        fn new(fd:File,id:&str)->R<Self> {
            let p=directory(&fd)?;let (bytes,mp)=read(&fd,"UI.json",128*1024)?;let m=parse(&bytes,id)?;
            let mut pins=BTreeMap::new();inventory(&fd,"",&m,&mut pins,"",&mut None)?;
            if pins.get("UI.json")!=Some(&mp) || m.files.iter().any(|e|!pins.contains_key(&e.path)) || directory(&fd)?!=p{return Err(());}
            Ok(Self{fd,root_pin:p,manifest:m,pins,valid:true})
        }
        fn bytes(&mut self, current:&File, path:&str)->R<Vec<u8>> {
            if !self.valid{return Err(());}
            let result=(|| {
                if directory(current)?!=self.root_pin || directory(&self.fd)?!=self.root_pin{return Err(());}
                let mut pins=BTreeMap::new();let mut content=None;
                inventory(&self.fd,"",&self.manifest,&mut pins,path,&mut content)?;
                if pins!=self.pins{return Err(());}content.ok_or(())
            })();
            if result.is_err(){self.valid=false;}result
        }
    }
    pub(super) fn serve(path:&Path, url:&str, builds:&mut BTreeMap<String,Build>)->R<(&'static str,Vec<u8>)> {
        let ui=root(path)?;
        let (id,leaf)=if url=="/" || url=="/index.html" {
            let link_pin = || -> R<(u64,u64,i64,i64)> {
                let mut st=std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe{libc::fstatat(ui.as_raw_fd(),c"current".as_ptr(),st.as_mut_ptr(),libc::AT_SYMLINK_NOFOLLOW)}!=0{return Err(());}
                let st=unsafe{st.assume_init()};
                if st.st_mode & libc::S_IFMT != libc::S_IFLNK || st.st_uid!=unsafe{libc::geteuid()} || st.st_nlink!=1{return Err(());}
                Ok((st.st_dev as u64,st.st_ino,st.st_ctime,st.st_ctime_nsec))
            };
            let before=link_pin()?;
            let mut buf=[0u8;33];let n=unsafe{libc::readlinkat(ui.as_raw_fd(),c"current".as_ptr(),buf.as_mut_ptr().cast(),buf.len())};
            if n!=32 || link_pin()?!=before{return Err(());}let id=std::str::from_utf8(&buf[..32]).map_err(|_|())?;
            if !hex(id,32){return Err(());}(id.to_owned(),"index.html")
        } else {
            let rest=url.strip_prefix("/ui/").ok_or(())?;let (id,leaf)=rest.split_once('/').ok_or(())?;
            if !hex(id,32) || !relative(leaf){return Err(());}(id.to_owned(),leaf)
        };
        let mime=mime(leaf).ok_or(())?;
        let selected=open(&ui,&id,true)?;
        if !builds.contains_key(&id) {
            // Bounded retained descriptors; no eviction that would forget drift.
            if builds.len()>=512{return Err(());}builds.insert(id.clone(),Build::new(selected.try_clone().map_err(|_|())?,&id)?);
        }
        let build=builds.get_mut(&id).ok_or(())?;
        // A request for a missing asset is not evidence that a valid build drifted.
        if !build.manifest.files.iter().any(|e|e.path==leaf){return Err(());}
        let data=build.bytes(&selected,leaf)?;
        let rebound=root(path)?;
        if !anchor(&directory(&ui)?,&directory(&rebound)?) || directory(&open(&rebound,&id,true)?)?!=directory(&selected)?{return Err(());}
        Ok((mime,data))
    }
}
