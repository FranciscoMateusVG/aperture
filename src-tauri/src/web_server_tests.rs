//! Real TCP against the production router, with isolated filesystem state, no daemons.
use super::*;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Fixture {
    state: WebState,
    open: String,
    task: tokio::task::JoinHandle<()>,
    root: PathBuf,
    preserve: bool,
}
impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("aperture-web-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for p in [
            "ui",
            ".aperture/run/owner",
            ".aperture/run/attempts",
            ".aperture/run/managed",
            ".aperture/teams",
            ".aperture/journal",
        ] {
            std::fs::create_dir_all(root.join(p)).unwrap();
        }
        use std::os::unix::fs::PermissionsExt;
        for dir in [".aperture", ".aperture/run"] {
            std::fs::set_permissions(root.join(dir), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        std::fs::write(root.join("ui/index.html"), "packaged fixture").unwrap();
        std::fs::write(root.join(".aperture/run/owner/sentinel"), "unchanged").unwrap();
        let app = AppState {
            tmux_session: "fixture-never-started".into(),
            agents: HashMap::new(),
            mcp_server_path: String::new(),
            mcp_sentry_server_path: String::new(),
            db_path: String::new(),
            project_dir: root.to_string_lossy().into_owned(),
            team_preparations: Arc::new(Mutex::new(crate::state::RuntimePermitStore::new())),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let open = crate::web_auth::credential().unwrap();
        let state = WebState {
            authority: address.clone(),
            origin: format!("http://{address}"),
            auth: Arc::new(Mutex::new(BrowserAuth::new(&open).unwrap())),
            app: Arc::new(Mutex::new(app)),
            home: root.clone(),
            project: root.clone(),
            ui: root.join("ui"),
            ui_fixture: false,
        };
        let app = router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            open,
            task,
            root,
            preserve: false,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, String) {
        let mut stream = tokio::net::TcpStream::connect(&self.state.authority)
            .await
            .unwrap();
        let host = if headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
            String::new()
        } else {
            format!("Host: {}\r\n", self.state.authority)
        };
        let h: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        let request=format!("{method} {path} HTTP/1.1\r\n{host}{h}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head.to_string(),
            body.to_string(),
        )
    }
    async fn mint(&self) -> String {
        let auth = format!("Bearer {}", self.open);
        let (status, _, body) = self
            .request("POST", "/session/mint", &[("Authorization", &auth)], "")
            .await;
        assert_eq!(status, 200);
        serde_json::from_str::<Value>(&body).unwrap()["exchange"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn redeem(&self, exchange: &str) -> (u16, String, String) {
        self.request(
            "POST",
            "/session",
            &[
                ("Origin", &self.state.origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Content-Type", "application/json"),
            ],
            &json!({"exchange":exchange}).to_string(),
        )
        .await
    }
    async fn session(&self) -> String {
        let exchange = self.mint().await;
        let (code, _, body) = self.redeem(&exchange).await;
        assert_eq!(code, 200);
        serde_json::from_str::<Value>(&body).unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn api(
        &self,
        method: &str,
        path: &str,
        session: &str,
        body: &str,
    ) -> (u16, String, String) {
        self.request(
            method,
            path,
            &[
                ("Origin", &self.state.origin),
                ("Sec-Fetch-Site", "same-origin"),
                ("Content-Type", "application/json"),
                ("Authorization", &format!("Bearer {session}")),
            ],
            body,
        )
        .await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        if !self.preserve {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, all: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                all.insert(p.strip_prefix(root).unwrap().to_owned(), vec![]);
                walk(root, &p, all)
            } else {
                all.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    let mut all = BTreeMap::new();
    walk(root, root, &mut all);
    all
}
#[tokio::test]
async fn host_origin_fetch_metadata_and_preflight_are_real_boundaries() {
    let f = Fixture::new().await;
    assert_eq!(f.request("GET", "/", &[], "").await.0, 200);
    for host in [
        "localhost:4519",
        "127.0.0.1:4519.evil",
        "rebind.invalid",
        "127.0.0.1.:4519",
    ] {
        assert_eq!(f.request("GET", "/", &[("Host", host)], "").await.0, 421);
    }
    assert_eq!(
        f.request(
            "GET",
            "/",
            &[("Host", &f.state.authority), ("Host", &f.state.authority)],
            ""
        )
        .await
        .0,
        421
    ); // Duplicate Host is rejected before auth/routing.
    let session = f.session().await;
    let auth = format!("Bearer {session}");
    for origin in [None, Some("null"), Some("http://external.invalid")] {
        let mut h = vec![
            ("Sec-Fetch-Site", "same-origin"),
            ("Authorization", auth.as_str()),
        ];
        if let Some(o) = origin {
            h.push(("Origin", o));
        }
        assert_eq!(f.request("POST", "/session/link", &h, "").await.0, 403);
        assert_eq!(
            f.request("POST", "/api/teams/t/seats/s/bootstrap", &h, "malformed")
                .await
                .0,
            403
        );
    }
    for site in [None, Some("none"), Some("cross-site"), Some("same-site")] {
        let mut h = vec![
            ("Origin", f.state.origin.as_str()),
            ("Authorization", auth.as_str()),
        ];
        if let Some(v) = site {
            h.push(("Sec-Fetch-Site", v));
        }
        assert_eq!(f.request("POST", "/session/link", &h, "").await.0, 403);
        assert_eq!(f.request("GET", "/api/version", &h, "").await.0, 403);
    }
    let (_, headers, _) = f
        .request(
            "OPTIONS",
            "/api/teams",
            &[
                ("Origin", "http://external.invalid"),
                ("Access-Control-Request-Method", "POST"),
            ],
            "",
        )
        .await;
    assert!(!headers.to_lowercase().contains("access-control-allow"));
    assert_eq!(
        f.request(
            "POST",
            "/session/mint",
            &[
                ("Authorization", &format!("Bearer {}", f.open)),
                ("Sec-Fetch-Site", "none")
            ],
            ""
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.request(
            "GET",
            "/",
            &[("Host", "evil"), ("X-Forwarded-Host", &f.state.authority)],
            ""
        )
        .await
        .0,
        421
    );
}
#[tokio::test]
async fn atomic_exchange_refresh_logout_link_and_restart() {
    let f = Fixture::new().await;
    let exchange = f.mint().await;
    let (one, two) = tokio::join!(f.redeem(&exchange), f.redeem(&exchange));
    assert_eq!([one.0, two.0].iter().filter(|s| **s == 200).count(), 1);
    assert_eq!([one.0, two.0].iter().filter(|s| **s == 410).count(), 1);
    let session = serde_json::from_str::<Value>(if one.0 == 200 { &one.2 } else { &two.2 })
        .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    for _ in 0..2 {
        assert_eq!(f.api("GET", "/api/version", &session, "").await.0, 200);
    } // Same browsing-context credential across refresh.
    assert_eq!(f.api("GET", "/api/version", &f.open, "").await.0, 401);
    assert_eq!(f.api("GET", "/api/version", "", "").await.0, 401);
    assert_eq!(f.api("POST", "/session/link", "", "").await.0, 401);
    let link = f.api("POST", "/session/link", &session, "").await;
    assert_eq!(link.0, 200);
    let linked = serde_json::from_str::<Value>(&link.2).unwrap()["exchange"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(f.api("POST", "/session/logout", &session, "").await.0, 200);
    assert_eq!(f.api("GET", "/api/version", &session, "").await.0, 401);
    assert_eq!(f.redeem(&linked).await.0, 410);
    let old = f.mint().await;
    let next = Fixture::new().await;
    assert_eq!(next.redeem(&old).await.0, 410);
    assert_eq!(next.api("GET", "/api/version", &session, "").await.0, 401);
    assert_eq!(
        next.request(
            "POST",
            "/session/mint",
            &[("Authorization", &format!("Bearer {}", f.open))],
            ""
        )
        .await
        .0,
        401
    );
    assert_eq!(
        next.api("GET", "/api/version", &next.session().await, "")
            .await
            .0,
        200
    );
    let expired = f.mint().await;
    f.state
        .auth
        .lock()
        .unwrap()
        .advance(Duration::from_secs(30));
    assert_eq!(f.redeem(&expired).await.0, 410);
}
#[tokio::test]
async fn bootstrap_denial_precedes_json_and_domain_with_identical_native_tree() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let before = tree(&f.root);
    for body in [
        json!({"expected_generation":0}).to_string(),
        json!({"expected_generation":1}).to_string(),
        json!({"expected_generation":2}).to_string(),
        "not JSON".into(),
        "x".repeat(BODY_LIMIT + 1),
    ] {
        let (status, _, body) = f
            .api(
                "POST",
                "/api/teams/invalid../seats/no-seat/bootstrap",
                &session,
                &body,
            )
            .await;
        assert_eq!(status, 403);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({"code":"E_WEB_AUTHORITY_DENIED","message":"action is not available through the browser operator surface"})
        );
    }
    // The registered denial handler has no State, engine or body parameter.
    assert_eq!(tree(&f.root), before);
}
#[tokio::test]
async fn finite_routes_reject_control_forgery_and_bounded_bad_input() {
    let f = Fixture::new().await;
    let session = f.session().await;
    let before = tree(&f.root);
    for path in [
        "/api/invoke",
        "/api/shell",
        "/api/command",
        "/api/teams/t/approve",
        "/api/teams/t/activate",
        "/api/teams/t/stop-seat",
        "/api/teams/t/retire-seat",
        "/api/teams/t/validate-checkpoint",
        "/api/teams/t/replace",
        "/api/teams/t/rollback-archive",
        "/api/teams/t/claude-inbox-probe",
        "/api/teams/t/inspect-remote",
        "/api/teams/t/resolve-remote",
        "/api/repositories",
    ] {
        assert_eq!(f.api("POST", path, &session, "{}").await.0, 404, "{path}");
    }
    for (path,body) in [
        ("/api/teams","{\"actor\":\"glados\"}"),("/api/teams/presets","{\"principal\":\"glados\"}"),
        ("/api/teams/t/cancel","{\"expected_generation\":0,\"creation_request_id\":\"fixture\",\"capability\":\"forged\"}"),
        ("/api/tmux/session","{\"session_name\":\"aperture\",\"argv\":[\"bad\"]}"),
        ("/api/agents/n/model","{\"name\":\"other\",\"model\":\"sonnet\"}"),
        ("/api/teams/t/replacement/start","{\"selection\":{\"actor\":\"glados\"}}"),
    ] { assert_eq!(f.api("POST",path,&session,body).await.0,400,"{path}"); }
    assert_eq!(
        f.api("POST", "/api/teams", &session, &"x".repeat(BODY_LIMIT + 1))
            .await
            .0,
        413
    );
    assert_eq!(f.redeem(&"x".repeat(1200)).await.0, 413);
    assert_eq!(
        f.api(
            "POST",
            "/session",
            &session,
            "{\"exchange\":\"bad\",\"actor\":\"glados\"}"
        )
        .await
        .0,
        400
    );
    assert_eq!(tree(&f.root), before);
}
#[tokio::test]
async fn every_registered_command_is_behind_session_and_errors_do_not_echo_secrets() {
    let f = Fixture::new().await;
    for (method, path) in [
        ("GET", "/api/version"),
        ("GET", "/api/agents"),
        ("POST", "/api/agents/n/start"),
        ("POST", "/api/agents/n/stop"),
        ("POST", "/api/agents/n/restart"),
        ("POST", "/api/agents/n/model"),
        ("POST", "/api/agents/n/attention/clear"),
        ("POST", "/api/tmux/session"),
        ("POST", "/api/tmux/select-window"),
        ("GET", "/api/teams/catalog"),
        ("GET", "/api/teams/presets"),
        ("POST", "/api/teams/presets"),
        ("POST", "/api/teams"),
        ("GET", "/api/teams"),
        ("POST", "/api/teams/t/cancel"),
        ("POST", "/api/teams/t/replacement/prepare"),
        ("POST", "/api/teams/t/replacement/start"),
        ("POST", "/api/teams/t/archive"),
        ("POST", "/api/teams/t/seats/s/open"),
        ("POST", "/api/teams/t/seats/s/bootstrap"),
    ] {
        let (code, headers, body) = f.api(method, path, &f.open, "").await;
        assert_eq!(code, 401, "{path}");
        assert!(!body.contains(&f.open));
        assert!(!body.contains(&f.root.to_string_lossy().to_string()));
        assert!(headers.contains("cache-control: no-store"));
        assert!(headers.contains(CSP));
        assert!(!headers.to_lowercase().contains("set-cookie"));
    }
    let (code, _, body) = f.request("GET", "/?t=redacted", &[], "").await;
    assert_eq!(code, 400);
    assert!(!body.contains("redacted"));
    let session = f.session().await;
    let result = f
        .api(
            "POST",
            "/api/teams",
            &session,
            &format!("{{\"secret\":\"{session}\"}}"),
        )
        .await;
    assert_eq!(result.0, 400);
    assert!(!result.2.contains(&session));
}

// Test-only completion protocol. Production routing/auth/CSP remain untouched.
#[cfg(target_os = "macos")]
mod browser_fixture {
    use super::*;
    use std::{
        io::Read,
        os::fd::AsRawFd,
        os::unix::{fs::PermissionsExt, process::CommandExt},
        process::{Child, Command, ExitStatus, Stdio},
        sync::atomic::{AtomicBool, Ordering},
        thread::JoinHandle,
        time::Instant,
    };

    const REPORT_PATH: &str = "/__fixture/report";
    const POLL_GAP: Duration = Duration::from_millis(2500);
    const CLEANUP_BUDGET: Duration = Duration::from_secs(3);
    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Phase {
        Loaded,
        Refreshed,
        Frame,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum Violation {
        Inline,
        Eval,
        External,
    }
    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct Report {
        run: String,
        phase: Phase,
        renders: [u32; 2],
        session_restored: bool,
        self_script: bool,
        violations: Vec<Violation>,
        control: bool,
        forbidden_frame_ran: bool,
    }
    #[derive(Default)]
    struct Traffic {
        init: u32,
        teams: u32,
        version: u32,
        renders: Vec<(u32, Instant)>,
    }
    struct Proof {
        run: String,
        origin: String,
        other_origin: String,
        reports: Vec<Phase>,
        traffic: [Traffic; 2],
        sequence: u32,
        external: u32,
        control: u32,
        failure: Option<&'static str>,
    }
    impl Proof {
        fn new(origin: String, other_origin: String) -> Self {
            Self {
                run: uuid::Uuid::new_v4().to_string(),
                origin,
                other_origin,
                reports: vec![],
                traffic: Default::default(),
                sequence: 0,
                external: 0,
                control: 0,
                failure: None,
            }
        }
        fn expected(&self) -> Option<Phase> {
            [Phase::Loaded, Phase::Refreshed, Phase::Frame]
                .get(self.reports.len())
                .copied()
        }
        fn accept(&mut self, r: Report) -> Result<(), &'static str> {
            let result = self.validate(&r);
            if let Err(e) = result {
                self.failure = Some(e);
                return Err(e);
            }
            self.reports.push(r.phase);
            Ok(())
        }
        fn validate(&self, r: &Report) -> Result<(), &'static str> {
            if self.failure.is_some() {
                return Err("proof_failed");
            }
            if r.run != self.run || self.expected() != Some(r.phase) {
                return Err("phase_binding");
            }
            if self.external != 0 {
                return Err("external_executed");
            }
            if r.phase == Phase::Frame {
                return if r.control
                    && self.control > 0
                    && !r.forbidden_frame_ran
                    && r.renders == [0, 0]
                    && r.violations.is_empty()
                    && !r.session_restored
                    && !r.self_script
                {
                    Ok(())
                } else {
                    Err("frame_control_missing")
                };
            }
            let t = &self.traffic[self.reports.len()];
            // The probe never fetches these application endpoints. An authenticated
            // 200 through the actual router is counted before a fixture body is substituted.
            let first = t.renders.iter().find(|(n, _)| *n == r.renders[0]);
            let last = t.renders.iter().find(|(n, _)| *n == r.renders[1]);
            let polled = match (first, last) {
                (Some((a, at)), Some((b, bt))) => {
                    a < b && bt.saturating_duration_since(*at) >= POLL_GAP
                }
                _ => false,
            };
            if t.init == 0 || t.teams == 0 || t.version == 0 || !polled {
                return Err("ui_not_loaded_and_polled");
            }
            if !r.session_restored
                || !r.self_script
                || r.control
                || r.forbidden_frame_ran
                || r.violations.len() != 3
                || ![Violation::Inline, Violation::Eval, Violation::External]
                    .iter()
                    .all(|v| r.violations.contains(v))
            {
                return Err("dom_or_csp_incomplete");
            }
            Ok(())
        }
    }
    type Shared = Arc<Mutex<Proof>>;

    async fn report(proof: Shared, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = match to_bytes(body, 2048).await {
            Ok(v) => v,
            Err(_) => {
                proof.lock().unwrap().failure = Some("invalid_report");
                return StatusCode::BAD_REQUEST.into_response();
            }
        };
        let r = match serde_json::from_slice::<Report>(&body) {
            Ok(r) => r,
            Err(_) => {
                proof.lock().unwrap().failure = Some("invalid_report");
                return StatusCode::BAD_REQUEST.into_response();
            }
        };
        let mut p = proof.lock().unwrap();
        let origin = if r.phase == Phase::Frame {
            &p.other_origin
        } else {
            &p.origin
        };
        if header(&parts.headers, "host") != origin.strip_prefix("http://")
            || header(&parts.headers, "origin") != Some(origin.as_str())
            || header(&parts.headers, "sec-fetch-site") != Some("same-origin")
            || header(&parts.headers, "content-type") != Some("application/json")
            || parts.headers.contains_key("authorization")
            || parts.headers.contains_key("cookie")
            || parts.uri.query().is_some()
        {
            return StatusCode::BAD_REQUEST.into_response();
        }
        if p.accept(r).is_ok() {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::BAD_REQUEST
        }
        .into_response()
    }
    async fn measure(State(proof): State<Shared>, request: Request, next: Next) -> Response {
        let path = request.uri().path().to_owned();
        let method = request.method().clone();
        let epoch = proof.lock().unwrap().reports.len();
        let response = next.run(request).await; // actual production Host/Origin/session gate first
        if response.status() != StatusCode::OK || epoch > 1 {
            return response;
        }
        let mut p = proof.lock().unwrap();
        if epoch != p.reports.len() {
            return response;
        }
        match (method, path.as_str()) {
            (Method::POST, "/api/tmux/session") => p.traffic[epoch].init += 1,
            (Method::GET, "/api/teams") => p.traffic[epoch].teams += 1,
            (Method::GET, "/api/version") => p.traffic[epoch].version += 1,
            (Method::GET, "/api/agents") => {
                p.sequence += 1;
                let n = p.sequence;
                p.traffic[epoch].renders.push((n, Instant::now()));
                // Rendered by the unchanged packaged AgentList/AgentCard, never by the probe.
                let body = serde_json::to_vec(&json!([{
                    "name":"glados", "model":format!("browser-fixture-{n}"), "role":"fixture",
                    "prompt_file":"fixture", "tmux_window_id":null, "status":"stopped"
                }]))
                .unwrap();
                let (mut parts, _) = response.into_parts();
                parts.headers.remove("content-length");
                return Response::from_parts(parts, axum::body::Body::from(body));
            }
            _ => {}
        }
        response
    }
    fn ui_router(state: WebState, proof: Shared) -> Router {
        assert!(
            state.ui_fixture,
            "never dispatch native Agents/HOME in this oracle"
        );
        let reports = proof.clone();
        router(state)
            .layer(middleware::from_fn_with_state(proof, measure))
            .route(REPORT_PATH, post(move |r| report(reports.clone(), r)))
    }

    #[derive(Default)]
    struct Scan {
        ancestor: bool,
        leaked: bool,
        overflow: bool,
        eof: bool,
    }
    // Drains are nonblocking and stop-bounded, including when a descendant holds a pipe.
    struct Drain {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<Scan>>,
        ancestor: Arc<AtomicBool>,
    }
    impl Drain {
        fn start(
            mut pipe: std::process::ChildStderr,
            origin: String,
            private: Vec<String>,
        ) -> Self {
            let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
            assert!(
                flags >= 0
                    && unsafe {
                        libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
                    } == 0
            );
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let ancestor = Arc::new(AtomicBool::new(false));
            let seen = ancestor.clone();
            let thread = std::thread::spawn(move || {
                let mut scan = Scan::default();
                let mut line = Vec::new();
                let mut total = 0usize;
                let mut buf = [0u8; 4096];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) => {
                            scan.eof = true;
                            break;
                        }
                        Ok(n) => {
                            total += n;
                            if total > 2 * 1024 * 1024 {
                                scan.overflow = true;
                            }
                            for b in &buf[..n] {
                                line.push(*b);
                                if *b == b'\n' || line.len() >= 16384 {
                                    let text = String::from_utf8_lossy(&line);
                                    if text.contains("frame-ancestors")
                                        && text.contains(&origin)
                                        && (text.contains("Framing")
                                            || text.contains("Refused to frame"))
                                    {
                                        scan.ancestor = true;
                                        seen.store(true, Ordering::SeqCst);
                                    }
                                    if private.iter().any(|v| !v.is_empty() && text.contains(v)) {
                                        scan.leaked = true;
                                    }
                                    if *b != b'\n' {
                                        scan.overflow = true;
                                    }
                                    line.clear();
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if stopping.load(Ordering::SeqCst) {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => {
                            scan.overflow = true;
                            break;
                        }
                    }
                    if stopping.load(Ordering::SeqCst) {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&line);
                if private.iter().any(|v| !v.is_empty() && text.contains(v)) {
                    scan.leaked = true;
                }
                scan
            });
            Self {
                stop,
                thread: Some(thread),
                ancestor,
            }
        }
        fn finish(&mut self, until: Instant) -> Result<Scan, &'static str> {
            while self.thread.as_ref().is_some_and(|h| !h.is_finished()) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.stop.store(true, Ordering::SeqCst);
            if self.thread.as_ref().is_some_and(|h| h.is_finished()) {
                let scan = self
                    .thread
                    .take()
                    .unwrap()
                    .join()
                    .map_err(|_| "drain_panicked")?;
                if !scan.eof {
                    return Err("drain_incomplete");
                }
                Ok(scan)
            } else {
                Err("drain_unverified")
            }
        }
    }
    #[derive(Debug, PartialEq, Eq)]
    enum ExitKind {
        Natural,
        FixtureSignal,
    }
    struct NativeChild {
        child: Child,
        identity: Option<crate::team_process::ProcessMetadata>,
        drain: Option<Drain>,
        status: Option<ExitStatus>,
        profile: PathBuf,
        attempted: bool,
        finished: bool,
    }
    impl NativeChild {
        fn spawn(
            command: &mut Command,
            profile: PathBuf,
            origin: String,
            private: Vec<String>,
        ) -> Result<Self, &'static str> {
            std::fs::create_dir(&profile).map_err(|_| "profile_create")?;
            std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "profile_mode")?;
            let child = command
                .process_group(0)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|_| "spawn")?;
            let mut owned = Self {
                child,
                identity: None,
                drain: None,
                status: None,
                profile,
                attempted: false,
                finished: false,
            };
            let observed = crate::team_process::observe(owned.child.id())
                .map_err(|_| "identity_unreadable")?;
            if let Some(p) = observed {
                if p.ppid != std::process::id()
                    || p.pgid != owned.child.id()
                    || p.uid != unsafe { libc::geteuid() }
                {
                    return Err("identity_mismatch");
                }
                owned.identity = Some(p);
            }
            owned.drain = Some(Drain::start(
                owned.child.stderr.take().unwrap(),
                origin,
                private,
            ));
            Ok(owned)
        }
        fn exited(&mut self) -> Result<bool, &'static str> {
            if self.status.is_none() {
                self.status = self.child.try_wait().map_err(|_| "wait")?;
            }
            Ok(self.status.is_some())
        }
        fn cleanup(&mut self) -> Result<(ExitKind, Scan), &'static str> {
            if self.attempted {
                return Err("cleanup_already_attempted");
            }
            self.attempted = true;
            let until = Instant::now() + CLEANUP_BUDGET;
            let natural = self.exited()?;
            let mut signaled = false;
            if !natural {
                let expected = self.identity.as_ref().ok_or("identity_missing")?;
                let now = crate::team_process::observe(self.child.id())
                    .map_err(|_| "identity_unreadable")?;
                match now {
                    Some(p)
                        if p.identity == expected.identity
                            && p.ppid == std::process::id()
                            && p.uid == expected.uid
                            && p.pgid == self.child.id() =>
                    {
                        if unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) } != 0 {
                            return Err("signal_failed");
                        }
                        signaled = true;
                    }
                    None if self.exited()? => {} // raced natural exit; never signal a recycled group
                    _ => return Err("identity_changed"),
                }
            }
            while !self.exited()? && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            if self.status.is_none() {
                return Err("reap_unverified");
            }
            loop {
                let exists = unsafe { libc::kill(-(self.child.id() as i32), 0) };
                if exists == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                if Instant::now() >= until {
                    return Err("group_cleanup_unverified");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let scan = match &mut self.drain {
                Some(d) => d.finish(until)?,
                None => Scan::default(),
            };
            std::fs::remove_dir_all(&self.profile).map_err(|_| "profile_cleanup")?;
            self.finished = true;
            Ok((
                {
                    use std::os::unix::process::ExitStatusExt;
                    if self.status.unwrap().success() {
                        ExitKind::Natural
                    } else if signaled && self.status.unwrap().signal() == Some(libc::SIGKILL) {
                        ExitKind::FixtureSignal
                    } else {
                        return Err("unexpected_child_exit");
                    }
                },
                scan,
            ))
        }
    }
    impl Drop for NativeChild {
        fn drop(&mut self) {
            if !self.attempted {
                let _ = self.cleanup();
            }
            if let Some(d) = &mut self.drain {
                if d.thread.is_some() {
                    let _ = d.finish(Instant::now() + Duration::from_millis(100));
                }
            }
        }
    }
    fn wait_proof(
        child: &mut NativeChild,
        proof: &Shared,
        frame: bool,
        until: Instant,
    ) -> Result<(), &'static str> {
        loop {
            let p = proof.lock().unwrap();
            if let Some(e) = p.failure {
                return Err(e);
            }
            let complete = p.reports.len() == if frame { 3 } else { 2 };
            drop(p);
            let ancestor = !frame
                || child
                    .drain
                    .as_ref()
                    .is_some_and(|d| d.ancestor.load(Ordering::SeqCst));
            if complete && ancestor {
                return Ok(());
            }
            if child.exited()? {
                return Err("browser_early_exit");
            }
            if Instant::now() >= until {
                return Err("proof_timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    async fn browse(
        chrome: &Path,
        profile: PathBuf,
        url: String,
        proof: Shared,
        frame: bool,
        private: Vec<String>,
    ) {
        let chrome = chrome.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut command = Command::new(chrome);
            command
                .env_clear()
                .env("HOME", &profile)
                .env("TMPDIR", &profile)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .args([
                    "--headless=new",
                    "--disable-gpu",
                    "--no-first-run",
                    "--no-default-browser-check",
                    "--disable-background-networking",
                    "--disable-component-update",
                    "--disable-sync",
                    "--disable-extensions",
                    "--disable-features=MediaRouter,OptimizationHints",
                    "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
                    "--no-proxy-server",
                    "--password-store=basic",
                    "--use-mock-keychain",
                    "--enable-logging=stderr",
                ])
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg(url);
            let origin = proof.lock().unwrap().origin.clone();
            let mut child = NativeChild::spawn(&mut command, profile, origin, private)
                .expect("fixture spawn/identity");
            let result = wait_proof(
                &mut child,
                &proof,
                frame,
                Instant::now() + Duration::from_secs(30),
            );
            let (exit, scan) = child
                .cleanup()
                .expect("fixture cleanup must be confirmed before verdict");
            assert!(
                !scan.leaked && !scan.overflow,
                "fixture diagnostic stream invalid"
            );
            assert!(
                proof.lock().unwrap().failure.is_none(),
                "late invalid/duplicate proof"
            );
            assert_eq!(result, Ok(()), "finite browser proof (not OS diagnosis)");
            if frame {
                assert!(scan.ancestor, "actual frame-ancestors violation required");
            }
            println!("browser fixture proof complete; cleanup verified; exit={exit:?}");
        })
        .await
        .expect("fixture runner panicked");
    }
    struct AbortServer(tokio::task::JoinHandle<()>);
    impl Drop for AbortServer {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    pub(super) async fn run() {
        let chrome = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        assert!(chrome.is_file(), "existing native Chrome required");
        let mut f = Fixture::new().await;
        f.preserve = true;
        std::fs::set_permissions(&f.root, std::fs::Permissions::from_mode(0o700)).unwrap();
        f.task.abort();
        f.state.ui_fixture = true;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        f.state.authority = listener.local_addr().unwrap().to_string();
        f.state.origin = format!("http://{}", f.state.authority);
        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_origin = format!("http://{}", other.local_addr().unwrap());
        let proof = Arc::new(Mutex::new(Proof::new(
            f.state.origin.clone(),
            other_origin.clone(),
        )));
        let run = proof.lock().unwrap().run.clone();
        let ending = concat!("<", "/script>");
        let frame_end = concat!("<", "/iframe>");
        let frame_document =
            format!("<!doctype html><html><body><script src='/frame-target.js'>{ending}");
        let parent = format!("<!doctype html><html><body><script src='/parent.js'>{ending}<iframe id='denied' src='{}/index.html'>{frame_end}<iframe id='control' src='/control-frame'>{frame_end}", f.state.origin);
        let parent_js = r#"
let good=false,bad=false;
window.addEventListener('message',e=>{
 if(e.data!=='fixture-frame-ran')return;
 if(e.source===document.querySelector('#control').contentWindow&&e.origin===location.origin)good=true;
 if(e.source===document.querySelector('#denied').contentWindow&&e.origin==='APP')bad=true;
});
setTimeout(()=>fetch('/__fixture/report',{method:'POST',headers:{'Content-Type':'application/json'},credentials:'omit',body:JSON.stringify({run:'RUN',phase:'frame',renders:[0,0],session_restored:false,self_script:false,violations:[],control:good,forbidden_frame_ran:bad})}),1800);
"#.replace("APP", &f.state.origin).replace("RUN", &run);
        let frame_js = "parent.postMessage('fixture-frame-ran','*');";
        let bad = proof.clone();
        let good = proof.clone();
        let reports = proof.clone();
        let control_document = frame_document.clone();
        let other_router = Router::new()
            .route(
                "/external.js",
                get(move || {
                    bad.lock().unwrap().external += 1;
                    async {
                        (
                            [("content-type", "text/javascript")],
                            "window.externalExecuted=true;",
                        )
                    }
                }),
            )
            .route(
                "/frame-parent",
                get(move || {
                    let p = parent.clone();
                    async move { ([("content-type", "text/html")], p) }
                }),
            )
            .route(
                "/parent.js",
                get(move || {
                    let p = parent_js.clone();
                    async move { ([("content-type", "text/javascript")], p) }
                }),
            )
            .route(
                "/frame-target.js",
                get(move || async move { ([("content-type", "text/javascript")], frame_js) }),
            )
            .route(
                "/control-frame",
                get(move || {
                    good.lock().unwrap().control += 1;
                    let p = control_document.clone();
                    async move { ([("content-type", "text/html")], p) }
                }),
            )
            .route(REPORT_PATH, post(move |r| report(reports.clone(), r)));
        let _other = AbortServer(tokio::spawn(async move {
            axum::serve(other, other_router).await.unwrap()
        }));
        fn copy_ui(from: &Path, to: &Path, origin: &str) {
            std::fs::create_dir_all(to).unwrap();
            for item in std::fs::read_dir(from).unwrap() {
                let p = item.unwrap().path();
                let target = to.join(p.file_name().unwrap());
                if p.is_dir() {
                    copy_ui(&p, &target, origin);
                } else {
                    let text = std::fs::read_to_string(&p)
                        .unwrap()
                        .replace("http://127.0.0.1:4519", origin);
                    assert!(!text.contains("serviceWorker.register"));
                    std::fs::write(target, text).unwrap();
                }
            }
        }
        let built = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("dist");
        assert!(
            built.join("index.html").is_file(),
            "task-owned UI build required; never build implicitly"
        );
        copy_ui(&built, &f.state.ui, &f.state.origin);
        let index = std::fs::read_to_string(f.state.ui.join("index.html")).unwrap();
        assert!(!index.contains("http://") && !index.contains("https://"));
        std::fs::write(
            f.state.ui.join("index.html"),
            format!("{index}<script src='/probe.js'>{ending}"),
        )
        .unwrap();
        std::fs::write(
            f.state.ui.join("positive.js"),
            "document.documentElement.dataset.selfScript='pass';",
        )
        .unwrap();
        let probe=r#"
const root=document.documentElement,violations=new Set();
document.addEventListener('securitypolicyviolation',e=>{if(e.disposition!=='enforce'||!['script-src','script-src-elem'].includes(e.effectiveDirective))return;if(e.blockedURI==='inline'||e.blockedURI==='eval')violations.add(e.blockedURI);if(e.blockedURI==='OTHER/external.js'||e.blockedURI==='OTHER')violations.add('external')});
const inline=document.createElement('script');inline.textContent='window.inlineExecuted=true';document.head.append(inline);
try{(0,eval)('window.evalExecuted=true')}catch{}
const external=document.createElement('script');external.src='OTHER/external.js';document.head.append(external);
const positive=document.createElement('script');positive.src='/positive.js';document.head.append(positive);
const prior=sessionStorage.getItem('fixture-session'),phase=prior?'refreshed':'loaded';
let first=null,firstAt=0,done=false;
const tick=setInterval(async()=>{
 if(done)return;
 const node=document.querySelector('.agent-mini[data-agent-name="glados"] .agent-mini__model');
 const match=node?.textContent.match(/^browser-fixture-(\d+)$/);
 if(!match||!document.querySelector('.web-session')?.textContent.includes('Local operator session'))return;
 const n=Number(match[1]);if(first===null){first=n;firstAt=performance.now();return}
 if(n<=first||performance.now()-firstAt<2500)return;
 if(root.dataset.selfScript!=='pass'||violations.size!==3)return;
 done=true;clearInterval(tick);
 const session=sessionStorage.getItem('aperture.web.session.v1');
 // Private equality stays inside the browser; no token/hash is reported or logged.
 const restored=!!session&&location.hash===''&&(!prior||prior===session);
 const result=await fetch('/__fixture/report',{method:'POST',headers:{'Content-Type':'application/json'},credentials:'omit',body:JSON.stringify({run:'RUN',phase,renders:[first,n],session_restored:restored,self_script:root.dataset.selfScript==='pass'&&!window.inlineExecuted&&!window.evalExecuted&&!window.externalExecuted,violations:[...violations],control:false,forbidden_frame_ran:false})});
 if(result.ok&&phase==='loaded'){sessionStorage.setItem('fixture-session',session);location.reload()}
},50);
"#.replace("OTHER",&other_origin).replace("RUN",&run);
        std::fs::write(f.state.ui.join("probe.js"), probe).unwrap();
        let app = ui_router(f.state.clone(), proof.clone());
        f.task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let exchange = f.mint().await;
        browse(
            chrome,
            f.root.join("chrome-ui"),
            format!("{}/#t={exchange}", f.state.origin),
            proof.clone(),
            false,
            vec![exchange, f.open.clone()],
        )
        .await;
        // The actual static route serves index.html, not arbitrary HTML filenames.
        // Both framed documents have byte-identical HTML+JS; only app response headers differ.
        std::fs::write(f.state.ui.join("index.html"), &frame_document).unwrap();
        std::fs::write(f.state.ui.join("frame-target.js"), frame_js).unwrap();
        let (status, headers, body) = f.request("GET", "/index.html", &[], "").await;
        assert_eq!(status, 200);
        assert!(headers.contains("text/html"));
        assert!(headers.contains(CSP));
        assert_eq!(body, frame_document);
        browse(
            chrome,
            f.root.join("chrome-frame"),
            format!("{other_origin}/frame-parent"),
            proof.clone(),
            true,
            vec![f.open.clone()],
        )
        .await;
        let p = proof.lock().unwrap();
        assert_eq!(p.external, 0);
        assert_eq!(p.reports.len(), 3);
        assert!(p.failure.is_none());
        f.preserve = false;
    }

    #[cfg(test)]
    mod completion_tests {
        use super::*;
        fn proof() -> Proof {
            Proof::new(
                "http://127.0.0.1:1111".into(),
                "http://127.0.0.1:2222".into(),
            )
        }
        fn ready(p: &mut Proof, phase: Phase) -> Report {
            let i = if phase == Phase::Loaded { 0 } else { 1 };
            let n = (i as u32) * 2 + 1;
            p.traffic[i] = Traffic {
                init: 1,
                teams: 1,
                version: 1,
                renders: vec![
                    (n, Instant::now() - Duration::from_secs(3)),
                    (n + 1, Instant::now()),
                ],
            };
            Report {
                run: p.run.clone(),
                phase,
                renders: [n, n + 1],
                session_restored: true,
                self_script: true,
                violations: vec![Violation::Inline, Violation::Eval, Violation::External],
                control: false,
                forbidden_frame_ran: false,
            }
        }
        #[test]
        fn completion_incomplete_or_boolean_only_cannot_pass() {
            let mut p = proof();
            let mut r = ready(&mut p, Phase::Loaded);
            r.violations.pop();
            assert_eq!(p.accept(r), Err("dom_or_csp_incomplete"));
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            p.traffic[0] = Traffic::default();
            assert_eq!(p.accept(r), Err("ui_not_loaded_and_polled"));
        }
        #[test]
        fn completion_duplicate_wrong_run_and_fast_initial_calls_deny() {
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            p.accept(r.clone()).unwrap();
            assert_eq!(p.accept(r), Err("phase_binding"));
            let mut p = proof();
            let mut r = ready(&mut p, Phase::Loaded);
            r.run = "other".into();
            assert_eq!(p.accept(r), Err("phase_binding"));
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            p.traffic[0].renders[0].1 = Instant::now();
            assert_eq!(p.accept(r), Err("ui_not_loaded_and_polled"));
        }
        #[test]
        fn completion_refresh_requires_new_application_traffic() {
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            p.accept(r).unwrap();
            let mut r = ready(&mut p, Phase::Refreshed);
            r.renders = [1, 2];
            assert_eq!(p.accept(r), Err("ui_not_loaded_and_polled"));
        }
        #[test]
        fn completion_control_missing_external_and_unknown_fields_deny() {
            let mut p = proof();
            for phase in [Phase::Loaded, Phase::Refreshed] {
                let r = ready(&mut p, phase);
                p.accept(r).unwrap();
            }
            let r = Report {
                run: p.run.clone(),
                phase: Phase::Frame,
                renders: [0, 0],
                session_restored: false,
                self_script: false,
                violations: vec![],
                control: true,
                forbidden_frame_ran: false,
            };
            assert_eq!(p.accept(r), Err("frame_control_missing"));
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            p.external = 1;
            assert_eq!(p.accept(r), Err("external_executed"));
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            let mut v = serde_json::to_value(r).unwrap();
            v["session"] = json!("forbidden");
            assert!(serde_json::from_value::<Report>(v).is_err());
        }
        fn inert(exits: bool) -> NativeChild {
            let profile =
                std::env::temp_dir().join(format!("aperture-csp-inert-{}", uuid::Uuid::new_v4()));
            let mut cmd = Command::new("/bin/sleep");
            cmd.arg(if exits { "0.05" } else { "20" });
            NativeChild::spawn(&mut cmd, profile, "http://127.0.0.1:1111".into(), vec![]).unwrap()
        }
        #[test]
        fn completion_success_requires_owned_cleanup_and_reports_signal_exit() {
            let mut p = proof();
            for phase in [Phase::Loaded, Phase::Refreshed] {
                let r = ready(&mut p, phase);
                p.accept(r).unwrap();
            }
            let mut child = inert(false);
            let pid = child.child.id();
            let profile = child.profile.clone();
            assert_eq!(
                wait_proof(
                    &mut child,
                    &Arc::new(Mutex::new(p)),
                    false,
                    Instant::now() + Duration::from_secs(1)
                ),
                Ok(())
            );
            assert!(!child.exited().unwrap());
            let (kind, scan) = child.cleanup().unwrap();
            assert_eq!(kind, ExitKind::FixtureSignal);
            assert!(!scan.overflow);
            assert!(crate::team_process::observe(pid).unwrap().is_none());
            assert!(!profile.exists());
            assert!(child.finished);
        }
        #[test]
        fn completion_early_exit_is_not_timeout_or_success() {
            let mut child = inert(true);
            let p = Arc::new(Mutex::new(proof()));
            assert_eq!(
                wait_proof(
                    &mut child,
                    &p,
                    false,
                    Instant::now() + Duration::from_secs(2)
                ),
                Err("browser_early_exit")
            );
            assert_eq!(child.cleanup().unwrap().0, ExitKind::Natural);
        }
        #[test]
        fn completion_timeout_and_panic_reap_only_fixture_child() {
            let mut child = inert(false);
            let p = Arc::new(Mutex::new(proof()));
            assert_eq!(
                wait_proof(&mut child, &p, false, Instant::now()),
                Err("proof_timeout")
            );
            child.cleanup().unwrap();
            let child = inert(false);
            let pid = child.child.id();
            let profile = child.profile.clone();
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _guard = child;
                panic!("inert cleanup oracle")
            }));
            assert!(panic.is_err());
            assert!(crate::team_process::observe(pid).unwrap().is_none());
            assert!(!profile.exists());
        }
        #[test]
        fn completion_frame_callback_without_native_violation_never_passes() {
            let mut p = proof();
            for phase in [Phase::Loaded, Phase::Refreshed] {
                let r = ready(&mut p, phase);
                p.accept(r).unwrap();
            }
            p.control = 1;
            p.accept(Report {
                run: p.run.clone(),
                phase: Phase::Frame,
                renders: [0, 0],
                session_restored: false,
                self_script: false,
                violations: vec![],
                control: true,
                forbidden_frame_ran: false,
            })
            .unwrap();
            let mut child = inert(false);
            assert_eq!(
                wait_proof(&mut child, &Arc::new(Mutex::new(p)), true, Instant::now()),
                Err("proof_timeout")
            );
            child.cleanup().unwrap();
        }
        #[tokio::test]
        async fn completion_handler_binds_origin_run_and_rejects_duplicate() {
            let mut p = proof();
            let r = ready(&mut p, Phase::Loaded);
            let shared = Arc::new(Mutex::new(p));
            let make = |origin: &str| {
                Request::builder()
                    .method("POST")
                    .uri(REPORT_PATH)
                    .header("Host", "127.0.0.1:1111")
                    .header("Origin", origin)
                    .header("Sec-Fetch-Site", "same-origin")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&r).unwrap()))
                    .unwrap()
            };
            assert_eq!(
                report(shared.clone(), make("http://127.0.0.1:3333"))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
            assert!(shared.lock().unwrap().reports.is_empty());
            assert_eq!(
                report(shared.clone(), make("http://127.0.0.1:1111"))
                    .await
                    .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                report(shared.clone(), make("http://127.0.0.1:1111"))
                    .await
                    .status(),
                StatusCode::BAD_REQUEST
            );
            assert_eq!(shared.lock().unwrap().failure, Some("phase_binding"));
        }
        #[tokio::test]
        async fn completion_fixture_requires_synthetic_dispatch_and_serves_real_frame_html() {
            let mut f = Fixture::new().await;
            f.task.abort();
            f.state.ui_fixture = true;
            let proof = Arc::new(Mutex::new(proof()));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            f.state.authority = listener.local_addr().unwrap().to_string();
            f.state.origin = format!("http://{}", f.state.authority);
            let app = ui_router(f.state.clone(), proof.clone());
            f.task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            assert_eq!(f.request("GET", "/api/agents", &[], "").await.0, 403);
            assert_eq!(f.api("GET", "/api/agents", "invalid", "").await.0, 401);
            assert_eq!(proof.lock().unwrap().sequence, 0);
            let session = f.session().await;
            let result = f.api("GET", "/api/agents", &session, "").await;
            assert_eq!(result.0, 200);
            assert!(result.2.contains("browser-fixture-1"));
            let document = "<!doctype html><html><body>frame fixture";
            std::fs::write(f.state.ui.join("index.html"), document).unwrap();
            let (status, headers, body) = f.request("GET", "/index.html", &[], "").await;
            assert_eq!(status, 200);
            assert!(headers.contains("text/html"));
            assert!(headers.contains(CSP));
            assert_eq!(body, document);
        }
    }
}
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires explicit native Chrome fixture authorization; hermetic completion_tests do not launch Chrome"]
async fn packaged_ui_refresh_and_csp_in_native_chrome() {
    browser_fixture::run().await;
}

#[tokio::test]
async fn same_home_restart_rotates_open_capability_before_new_router() {
    use std::os::unix::fs::MetadataExt;
    let mut f = Fixture::new().await;
    let lease = crate::controller::ControllerLock::acquire(&f.root).unwrap();
    lease.rotate_open_capability(&f.open).unwrap();
    let old_session = f.session().await;
    let old_exchange = f.mint().await;
    let old_open = f.open.clone();
    let before = std::fs::read(f.root.join(".aperture/run/owner/sentinel")).unwrap();
    f.task.abort();
    let _ = (&mut f.task).await;
    drop(lease);
    let lease = crate::controller::ControllerLock::acquire(&f.root).unwrap();
    f.open = crate::web_auth::credential().unwrap();
    lease.rotate_open_capability(&f.open).unwrap();
    assert_ne!(f.open, old_open);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.state.authority = listener.local_addr().unwrap().to_string();
    f.state.origin = format!("http://{}", f.state.authority);
    f.state.auth = Arc::new(Mutex::new(BrowserAuth::new(&f.open).unwrap()));
    let app = router(f.state.clone());
    f.task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert_eq!(f.redeem(&old_exchange).await.0, 410);
    assert_eq!(f.api("GET", "/api/version", &old_session, "").await.0, 401);
    assert_eq!(
        f.request(
            "POST",
            "/session/mint",
            &[("Authorization", &format!("Bearer {old_open}"))],
            ""
        )
        .await
        .0,
        401
    );
    let new_session = f.session().await;
    assert_eq!(f.api("GET", "/api/version", &new_session, "").await.0, 200);
    assert_eq!(
        std::fs::metadata(f.root.join(".aperture/run/operator.token"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read(f.root.join(".aperture/run/owner/sentinel")).unwrap(),
        before
    );
}


fn has_csp_events(dom: &str, external: &str) -> bool {
    let attribute = regex::Regex::new(r#"<html\b[^>]*\bdata-csp="([^"]*)""#).unwrap();
    let Some(found) = attribute.captures(dom) else { return false; };
    let values: std::collections::HashSet<_> = found[1].split(',').collect();
    values.contains("inline") && values.contains("eval") && values.contains(external)
}

#[test]
fn csp_oracle_requires_listener_attribute_not_dom_substrings() {
    let external = "http://127.0.0.1:54321/external.js";
    let misleading = format!(r#"<html data-inline="blocked" data-eval="blocked"><script src="{external}">"#);
    assert!(!has_csp_events(&misleading, external));
    for values in ["inline", "eval", "inline,eval", "inline,eval,http://127.0.0.1:1/external.js"] {
        assert!(!has_csp_events(&format!(r#"<html data-csp="{values}">"#), external));
    }
    assert!(has_csp_events(&format!(r#"<html lang="en" data-csp="eval,{external},inline" data-inline="blocked">"#), external));
}
