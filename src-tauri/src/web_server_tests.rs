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
        let _ = std::fs::remove_dir_all(&self.root);
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

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Requires task-owned production UI build and explicit native Chrome fixture authorization"]
async fn packaged_ui_refresh_and_csp_in_native_chrome() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let chrome = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    assert!(
        chrome.is_file(),
        "existing native Chrome required; never install a browser"
    );
    let version = std::process::Command::new(chrome)
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    println!(
        "native browser: {}",
        String::from_utf8_lossy(&version.stdout).trim()
    );
    let mut f = Fixture::new().await;
    f.task.abort();
    f.state.ui_fixture = true;
    // Restart only the synthetic TCP router, not the production controller.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    f.state.authority = listener.local_addr().unwrap().to_string();
    f.state.origin = format!("http://{}", f.state.authority);
    let external_calls = Arc::new(AtomicUsize::new(0));
    let control_calls = Arc::new(AtomicUsize::new(0));
    let other_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_origin = format!("http://{}", other_listener.local_addr().unwrap());
    let bad = external_calls.clone();
    let good = control_calls.clone();
    let ending = concat!("<", "/script>");
    let frame_end = concat!("<", "/iframe>");
    let frame_parent=format!("<html><body><iframe src='{}/frame-target.html'>{frame_end}<iframe src='/control-frame'>{frame_end}<script src='/parent.js'>{ending}",f.state.origin);
    let other=Router::new()
        .route("/external.js",get(move || {let bad=bad.clone();async move {bad.fetch_add(1,Ordering::SeqCst); ([("content-type","text/javascript")],"window.externalExecuted=true;")}}))
        .route("/frame-parent",get(move || {let html=frame_parent.clone();async move { ([("content-type","text/html")],html) }}))
        .route("/parent.js",get(||async{ ([("content-type","text/javascript")],"window.addEventListener('message',e=>{if(e.data==='control')document.body.dataset.control='pass';if(e.data==='blocked')document.body.dataset.frame='failed'});setTimeout(()=>{if(!document.body.dataset.frame)document.body.dataset.frame='blocked'},1500);") }))
        .route("/control-frame",get(move || {let good=good.clone();async move {good.fetch_add(1,Ordering::SeqCst); ([("content-type","text/html")],format!("<script>parent.postMessage('control','*'){ending}")) }}));
    let other_task = tokio::spawn(async move {
        axum::serve(other_listener, other).await.unwrap();
    });
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("dist");
    assert!(
        built.join("index.html").is_file(),
        "run pnpm build in task tree first"
    );
    fn copy_ui(from: &Path, to: &Path, origin: &str) {
        std::fs::create_dir_all(to).unwrap();
        for item in std::fs::read_dir(from).unwrap() {
            let p = item.unwrap().path();
            let target = to.join(p.file_name().unwrap());
            if p.is_dir() {
                copy_ui(&p, &target, origin)
            } else {
                let content = std::fs::read_to_string(&p).unwrap();
                // Only fixture authority differs: production source/build stays canonical 4519.
                let content = content.replace("http://127.0.0.1:4519", origin);
                assert!(!content.contains("serviceWorker.register"));
                std::fs::write(target, content).unwrap();
            }
        }
    }
    copy_ui(&built, &f.state.ui, &f.state.origin);
    let index = std::fs::read_to_string(f.state.ui.join("index.html")).unwrap();
    assert!(
        !index.contains("http://") && !index.contains("https://"),
        "no remote production assets"
    );
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
    std::fs::write(
        f.state.ui.join("frame-target.html"),
        format!("<script src='/frame-target.js'>{ending}"),
    )
    .unwrap();
    std::fs::write(
        f.state.ui.join("frame-target.js"),
        "parent.postMessage('blocked','*');",
    )
    .unwrap();
    let probe=r#"
const root=document.documentElement;
const violations=new Set();document.addEventListener('securitypolicyviolation',e=>{violations.add(e.blockedURI);root.dataset.csp=[...violations].sort().join(',')});
const inline=document.createElement('script');inline.textContent='window.inlineExecuted=true';document.head.append(inline);
try { (0,eval)('window.evalExecuted=true'); } catch {root.dataset.eval='blocked'}
const external=document.createElement('script');external.src='OTHER/external.js';document.head.append(external);
const positive=document.createElement('script');positive.src='/positive.js';document.head.append(positive);
async function digest(v){const b=await crypto.subtle.digest('SHA-256',new TextEncoder().encode(v));return Array.from(new Uint8Array(b)).map(x=>x.toString(16).padStart(2,'0')).join('')}
let count=0;
const ready=setInterval(async()=>{
 if(++count>80){clearInterval(ready);root.dataset.ui='failed';return}
 if(!document.querySelector('.web-session')?.textContent.includes('Local operator session'))return;
 clearInterval(ready);
 const session=sessionStorage.getItem('aperture.web.session.v1');
 const hash=await digest(session||'');
 if(!sessionStorage.getItem('fixture-refresh')){sessionStorage.setItem('fixture-refresh',hash);location.reload();return}
 root.dataset.refresh=session&&sessionStorage.getItem('fixture-refresh')===hash&&location.hash===''?'pass':'failed';
 const response=await fetch('/api/version',{headers:{Authorization:'Bearer '+session},credentials:'omit'});
 root.dataset.api=response.ok?'pass':'failed';
 setTimeout(()=>{root.dataset.ui=document.querySelector('#navbar')&&document.querySelector('#sidebar-agents')?'pass':'failed';root.dataset.inline=window.inlineExecuted?'failed':'blocked';root.dataset.external=window.externalExecuted?'failed':'blocked';},3500);
},100);
"#.replace("OTHER",&other_origin);
    std::fs::write(f.state.ui.join("probe.js"), probe).unwrap();
    let app = router(f.state.clone());
    f.task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let exchange = f.mint().await;
    async fn dump(chrome: PathBuf, profile: PathBuf, url: String) -> std::process::Output {
        tokio::task::spawn_blocking(move || {
            use std::os::unix::process::CommandExt;
            use std::process::{Command, Stdio};
            std::fs::create_dir_all(&profile).unwrap();
            let mut child = Command::new(chrome)
                .process_group(0)
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
                    "--timeout=12000",
                    "--dump-dom",
                ])
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg(url)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            // Drain both pipes concurrently: Chrome's stderr can fill before exit.
            use std::io::Read;
            let out = child.stdout.take().unwrap();
            let err = child.stderr.take().unwrap();
            let out_thread = std::thread::spawn(move || {
                let mut b = Vec::new();
                out.take(2 * 1024 * 1024).read_to_end(&mut b).unwrap();
                b
            });
            let err_thread = std::thread::spawn(move || {
                let mut b = Vec::new();
                err.take(2 * 1024 * 1024).read_to_end(&mut b).unwrap();
                b
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let status = loop {
                if let Some(s) = child.try_wait().unwrap() {
                    break s;
                };
                if std::time::Instant::now() > deadline {
                    // The child created its own process group; never signal operator Chrome.
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let status = child.wait().unwrap();
                    let stdout = out_thread.join().unwrap();
                    let stderr = err_thread.join().unwrap();
                    let raw = String::from_utf8_lossy(&stderr);
                    let redact = regex::Regex::new(r"[A-Za-z0-9_-]{43,}").unwrap();
                    let sanitized = redact.replace_all(&raw, "[redacted]");
                    let tail: String = sanitized
                        .chars()
                        .rev()
                        .take(3000)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    eprintln!("bounded synthetic Chrome timeout diagnostics: {tail}");
                    return std::process::Output {
                        status,
                        stdout,
                        stderr,
                    };
                };
                std::thread::sleep(Duration::from_millis(50));
            };
            std::process::Output {
                status,
                stdout: out_thread.join().unwrap(),
                stderr: err_thread.join().unwrap(),
            }
        })
        .await
        .unwrap()
    }
    let output = dump(
        chrome.to_owned(),
        f.root.join("chrome-ui"),
        format!("{}/#t={exchange}", f.state.origin),
    )
    .await;
    assert!(output.status.success());
    let dom = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!dom.contains(&exchange) && !stderr.contains(&exchange) && !stderr.contains(&f.open));
    for marker in [
        "data-refresh=\"pass\"",
        "data-api=\"pass\"",
        "data-ui=\"pass\"",
        "data-inline=\"blocked\"",
        "data-eval=\"blocked\"",
        "data-external=\"blocked\"",
        "data-self-script=\"pass\"",
    ] {
        assert!(dom.contains(marker), "missing browser proof {marker}");
    }
    assert!(has_csp_events(&dom, &format!("{other_origin}/external.js")),
        "require securitypolicyviolation blockedURI values from the root data-csp attribute");
    assert_eq!(
        external_calls.load(Ordering::SeqCst),
        0,
        "cross-origin CSP negative must not hit network"
    );
    let frame = dump(
        chrome.to_owned(),
        f.root.join("chrome-frame"),
        format!("{other_origin}/frame-parent"),
    )
    .await;
    assert!(frame.status.success());
    let dom = String::from_utf8(frame.stdout).unwrap();
    let log = String::from_utf8_lossy(&frame.stderr);
    assert!(
        dom.contains("data-control=\"pass\"") && control_calls.load(Ordering::SeqCst) > 0,
        "framing positive control failed"
    );
    assert!(dom.contains("data-frame=\"blocked\"") && !dom.contains("data-frame=\"failed\""));
    assert!(
        log.contains("frame-ancestors"),
        "require native CSP ancestor violation, not just a timeout"
    );
    other_task.abort();
    println!("browser proof: packaged UI, API polling, session refresh, inline/eval/external CSP negatives, self-script and frame controls, frame-ancestors denial; isolated profiles cleaned on fixture drop");
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
