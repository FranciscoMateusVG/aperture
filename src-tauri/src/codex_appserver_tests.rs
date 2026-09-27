//! Native C1 fixtures only. No Codex, operational HOME, credentials or RPC.
use super::probe_registered;
use crate::{
    controller::ControllerLock,
    daemon_registry::{Endpoint, Provenance, Registry},
    team_process,
    team_replacement::{ProcessIdentity, ProcessState},
};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::{
        fs::{symlink, MetadataExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedChild {
    child: Child,
    identity: ProcessIdentity,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            assert_eq!(team_process::state(&self.identity), ProcessState::Same);
            self.child.kill().unwrap();
        }
        self.child.wait().unwrap();
        assert_eq!(team_process::state(&self.identity), ProcessState::Gone);
        eprintln!("C1 fixture cleanup: owned identity reaped/Gone");
    }
}
struct Fixture {
    root: PathBuf,
    home: PathBuf,
    children: Vec<OwnedChild>,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/ac-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        // Explicit fixture setup, NOT part of the read-only probe.
        fs::create_dir_all(home.join(".claude/aperture/fixture")).unwrap();
        Self {
            root,
            home,
            children: vec![],
        }
    }
    fn lease(&self) -> ControllerLock {
        ControllerLock::acquire(&self.home).unwrap()
    }
    fn socket(&self) -> PathBuf {
        self.home.join(".aperture/run/fixture.sock")
    }
    fn listener(&mut self) -> ProcessIdentity {
        assert!(self.children.is_empty());
        let output = fs::File::create(self.root.join("child.log")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "codex_appserver::registered_tests::c1_inert_unix_entry",
                "--ignored",
                "--nocapture",
            ])
            .env("APERTURE_C1_FIXTURE", &self.root)
            .env("HOME", &self.home)
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap();
        let identity = team_process::observe(child.id()).unwrap().unwrap().identity;
        self.children.push(OwnedChild {
            child,
            identity: identity.clone(),
        });
        wait_for(|| self.root.join("ready").is_file());
        identity
    }
    fn eof(&self, count: usize) {
        wait_for(|| {
            fs::read_to_string(self.root.join("counts")).ok().as_deref()
                == Some(&format!("{count}:0"))
        });
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.children.clear();
        // Diagnostic survives the ephemeral root in the parent test log. Bound
        // the copy; fixture output contains only synthetic phase/error metadata.
        if let Ok(file) = fs::File::open(self.root.join("child.log")) {
            let mut bytes = Vec::new();
            file.take(8193).read_to_end(&mut bytes).unwrap();
            let truncated = bytes.len() > 8192;
            bytes.truncate(8192);
            eprintln!(
                "C1 child diagnostic begin truncated={truncated}\n{}\nC1 child diagnostic end",
                String::from_utf8_lossy(&bytes)
            );
        }
        fs::remove_dir_all(&self.root).unwrap();
        assert!(!self.root.exists());
        eprintln!("C1 fixture cleanup: synthetic root removed");
    }
}
fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            Instant::now() < deadline,
            "bounded fixture readiness/EOF timeout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
#[ignore = "inert child entry; only invoked explicitly by owned fixtures"]
fn c1_inert_unix_entry() {
    let root = PathBuf::from(std::env::var_os("APERTURE_C1_FIXTURE").unwrap());
    assert!(
        root.starts_with("/private/tmp")
            && root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("ac-")
    );
    let socket = root.join("home/.aperture/run/fixture.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::write(root.join("ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut connections, mut bytes) = (0, 0);
    eprintln!("C1 child phase=ready connections=0 bytes=0");
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                eprintln!("C1 child phase=accepted connections={connections} bytes={bytes}");
                stream.set_nonblocking(true).unwrap();
                let read_deadline = Instant::now() + Duration::from_secs(2);
                let mut buf = [0; 64];
                loop {
                    assert!(Instant::now() < read_deadline,
                        "fixture read deadline exceeded before EOF: connections={connections} bytes={bytes}");
                    match stream.read(&mut buf) {
                        Ok(0) => {
                            eprintln!("C1 child phase=eof connections={connections} bytes={bytes}");
                            break;
                        }
                        Ok(n) => {
                            bytes += n;
                            eprintln!("C1 child phase=unexpected_payload connections={connections} bytes={bytes}");
                            panic!("fixture received application payload");
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => {
                            eprintln!("C1 child phase=read_error kind={:?} raw={:?} connections={connections} bytes={bytes}", e.kind(), e.raw_os_error());
                            panic!("fixture read failed before EOF: {e}");
                        }
                    }
                }
                connections += 1;
                fs::write(root.join("counts"), format!("{connections}:{bytes}")).unwrap();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => {
                eprintln!("C1 child phase=accept_error kind={:?} raw={:?} connections={connections} bytes={bytes}", e.kind(), e.raw_os_error());
                panic!("fixture accept failed: {e}");
            }
        }
    }
}
// Snapshot directory/leaf identity and bytes, not atime or socket streams.
fn snapshot(home: &Path) -> BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, (u64, u64, u32, Vec<u8>)>) {
        let m = fs::symlink_metadata(at).unwrap();
        let bytes = if m.file_type().is_symlink() {
            fs::read_link(at)
                .unwrap()
                .to_string_lossy()
                .as_bytes()
                .to_vec()
        } else if m.is_file() {
            fs::read(at).unwrap()
        } else {
            vec![]
        };
        out.insert(
            at.strip_prefix(root).unwrap().into(),
            (m.dev(), m.ino(), m.mode(), bytes),
        );
        if m.is_dir() && !m.file_type().is_symlink() {
            for entry in fs::read_dir(at).unwrap() {
                walk(root, &entry.unwrap().path(), out);
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(home, home, &mut result);
    result
}
fn publish(registry: &Registry<'_>, identity: &ProcessIdentity) {
    let reservation = registry
        .reserve(
            Endpoint::CodexAppServer {
                seat: "fixture".into(),
            },
            Provenance::LegacyUnknown,
            100,
        )
        .unwrap();
    let record = registry.record(&reservation, identity, 101).unwrap();
    registry.publish_current(&record).unwrap();
}
fn deny_unchanged(f: &Fixture, registry: &Registry<'_>, seat: &str) {
    let before = snapshot(&f.home);
    assert!(probe_registered(registry, seat).is_err());
    assert_eq!(snapshot(&f.home), before);
}

#[test]
fn c1_registered_native_peer_is_observation_only_zero_payload() {
    let mut f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let child = f.listener();
    publish(&registry, &child);
    let before = snapshot(&f.home);
    assert!(probe_registered(&registry, "fixture").is_ok());
    f.eof(1);
    assert_eq!(snapshot(&f.home), before);
    assert_eq!(team_process::state(&child), ProcessState::Same);
    assert!(probe_registered(&registry, "fixture").is_ok());
    f.eof(2);
}
#[test]
fn c1_initial_absence_and_unregistered_listener_never_enroll() {
    let mut f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    deny_unchanged(&f, &registry, "fixture");
    f.listener();
    deny_unchanged(&f, &registry, "fixture");
    assert!(!f.root.join("counts").exists());
    assert!(!f.home.join(".aperture/run/daemons/codex-fixture").exists());
}
#[test]
fn c1_other_live_listener_cannot_match_registered_pid() {
    let mut f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let other = f.listener();
    publish(&registry, lease.identity().unwrap());
    deny_unchanged(&f, &registry, "fixture");
    f.eof(1);
    assert_eq!(team_process::state(&other), ProcessState::Same);
}
#[test]
fn c1_missing_directories_after_setup_are_not_recreated() {
    for relative in [
        ".aperture/run",
        ".aperture/run/daemons",
        ".aperture/run/daemons/codex-fixture",
    ] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        publish(&registry, lease.identity().unwrap());
        fs::remove_dir_all(f.home.join(relative)).unwrap();
        deny_unchanged(&f, &registry, "fixture");
        assert!(!f.home.join(relative).exists());
    }
}
#[test]
fn c1_incomplete_history_and_endpoint_mismatch_never_connect() {
    for variant in 0..4 {
        let mut f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let identity = f.listener();
        let r = registry
            .reserve(
                Endpoint::CodexAppServer {
                    seat: "fixture".into(),
                },
                Provenance::LegacyUnknown,
                100,
            )
            .unwrap();
        if variant > 0 {
            let record = registry.record(&r, &identity, 101).unwrap();
            if variant > 1 {
                registry.publish_current(&record).unwrap();
            }
        }
        let slot = f.home.join(".aperture/run/daemons/codex-fixture");
        if variant == 2 {
            fs::write(slot.join("orphan.tmp"), b"unresolved").unwrap();
        }
        if variant == 3 {
            // Corrupt exact Endpoint in the immutable record, not the selector API.
            for entry in fs::read_dir(&slot).unwrap() {
                let p = entry.unwrap().path();
                let name = p.file_name().unwrap().to_str().unwrap();
                if name.ends_with(".json") && !name.starts_with("reservation-") {
                    let mut v: serde_json::Value =
                        serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    v["reservation"]["endpoint"]["seat"] = "different".into();
                    fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
            }
        }
        deny_unchanged(&f, &registry, "fixture");
        assert!(!f.root.join("counts").exists());
    }
}
#[test]
fn c1_unsafe_socket_registry_and_unknown_membership_deny_unchanged() {
    for variant in 0..6 {
        let mut f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let identity = f.listener();
        publish(&registry, &identity);
        match variant {
            0 => fs::set_permissions(f.socket(), fs::Permissions::from_mode(0o666)).unwrap(),
            1 => {
                fs::rename(f.socket(), f.root.join("other.sock")).unwrap();
                symlink(f.root.join("other.sock"), f.socket()).unwrap();
            }
            2 => fs::set_permissions(
                f.home.join(".aperture/run/daemons"),
                fs::Permissions::from_mode(0o777),
            )
            .unwrap(),
            3 => {
                let slot = f.home.join(".aperture/run/daemons/codex-fixture");
                fs::rename(&slot, f.root.join("slot")).unwrap();
                symlink(f.root.join("slot"), slot).unwrap();
            }
            4 => fs::write(f.home.join(".claude/aperture/fixture/TEAM"), b"synthetic").unwrap(),
            5 => fs::write(f.home.join(".aperture/teams"), b"unreadable registry shape").unwrap(),
            _ => unreachable!(),
        }
        deny_unchanged(&f, &registry, "fixture");
        assert!(!f.root.join("counts").exists());
    }
}
#[test]
fn c1_invalid_and_missing_selectors_never_choose_arbitrary_path() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    for seat in [
        "",
        "../fixture",
        "/fixture",
        "fixture/other",
        "other",
        "Fixture",
    ] {
        deny_unchanged(&f, &registry, seat);
    }
}
#[test]
fn c1_injected_identity_change_before_and_after_real_peer_denies() {
    for after in [false, true] {
        for fault in [
            ProcessState::Gone,
            ProcessState::Recycled,
            ProcessState::Unreadable,
        ] {
            let mut f = Fixture::new();
            let lease = f.lease();
            let registry = Registry::open(&lease).unwrap();
            let identity = f.listener();
            publish(&registry, &identity);
            let before = snapshot(&f.home);
            let mut calls = 0;
            assert!(crate::team_terminal::registered_socket_test_observation(
                &registry,
                "fixture",
                |id| {
                    calls += 1;
                    if !after || calls > 1 {
                        fault.clone()
                    } else {
                        team_process::state(id)
                    }
                },
                || {}
            )
            .is_err());
            if after {
                f.eof(1);
            } else {
                assert!(!f.root.join("counts").exists());
            }
            assert_eq!(snapshot(&f.home), before);
            assert_eq!(team_process::state(&identity), ProcessState::Same);
        }
    }
}
#[test]
fn c1_real_birth_mismatch_and_gone_registered_process_are_denied() {
    for gone in [false, true] {
        let mut f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let identity = f.listener();
        publish(&registry, &identity);
        if gone {
            f.children.clear();
        } else {
            let slot = f.home.join(".aperture/run/daemons/codex-fixture");
            for entry in fs::read_dir(slot).unwrap() {
                let p = entry.unwrap().path();
                let name = p.file_name().unwrap().to_str().unwrap();
                if name.ends_with(".json") && !name.starts_with("reservation-") {
                    let mut v: serde_json::Value =
                        serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
                    let birth = v["process"]["start_time_us"].as_u64().unwrap();
                    v["process"]["start_time_us"] = (birth + 1).into();
                    fs::write(p, serde_json::to_vec(&v).unwrap()).unwrap();
                }
            }
        }
        deny_unchanged(&f, &registry, "fixture");
        assert!(!f.root.join("counts").exists());
    }
}
#[test]
fn c1_post_peer_path_or_membership_swap_is_detected_without_repair() {
    for socket in [false, true] {
        let mut f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let identity = f.listener();
        publish(&registry, &identity);
        let mut replacement = None;
        let mut injected_image = None;
        let result = crate::team_terminal::registered_socket_test_observation(
            &registry,
            "fixture",
            team_process::state,
            || {
                if socket {
                    fs::rename(f.socket(), f.root.join("old.sock")).unwrap();
                    replacement = Some(UnixListener::bind(f.socket()).unwrap());
                    fs::set_permissions(f.socket(), fs::Permissions::from_mode(0o600)).unwrap();
                } else {
                    fs::write(
                        f.home.join(".claude/aperture/fixture/TEAM"),
                        b"injected drift",
                    )
                    .unwrap();
                }
                injected_image = Some(snapshot(&f.home));
            },
        );
        assert!(result.is_err());
        f.eof(1);
        assert_eq!(snapshot(&f.home), injected_image.unwrap());
        drop(replacement);
    }
}

#[test]
fn c1_managed_membership_denies_even_without_team_marker() {
    let mut f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let identity = f.listener();
    publish(&registry, &identity);
    let dir = f.home.join(".aperture/teams/t1");
    crate::journal::ensure_private_dir(&dir).unwrap();
    crate::journal::write_private_json_atomic(&dir.join("team.json"), &serde_json::json!({
        "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"Fixture mission","acceptance":"Fixture gate",
        "preset":{"id":null,"sha256":null},"lead":"fixture","seats":[{"name":"fixture","role":"backend","harness":"claude","model":"opus","reasoning":null}],
        "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()
    }), false).unwrap();
    crate::journal::write_private_json_atomic(&dir.join("state.json"), &serde_json::json!({
        "schema_version":1,"state":"pending","generation":0,"epic_id":null,"failure":null,"updated_at":"2026-09-20T00:00:00Z"
    }), false).unwrap();
    assert!(crate::teams::classify_managed_seat(&f.home, "fixture")
        .unwrap()
        .is_some());
    deny_unchanged(&f, &registry, "fixture");
    assert!(!f.root.join("counts").exists());
}
#[test]
fn c1_lease_getters_and_registry_reads_never_repair_removed_roots() {
    for relative in [".aperture", ".aperture/run", ".aperture/run/daemons"] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        publish(&registry, lease.identity().unwrap());
        fs::remove_dir_all(f.home.join(relative)).unwrap();
        let before = snapshot(&f.home);
        assert!(registry.current("codex-fixture").is_err());
        assert!(registry.inspect().is_err());
        if relative != ".aperture/run/daemons" {
            assert!(lease.verify_live().is_err());
            assert!(lease.run_dir().is_err());
            assert!(lease.identity().is_err());
        }
        assert_eq!(snapshot(&f.home), before);
    }
}
#[test]
fn c1_lease_replaced_lock_and_unsafe_or_symlinked_run_deny_without_repair() {
    for variant in 0..3 {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let run = f.home.join(".aperture/run");
        match variant {
            0 => {
                fs::rename(run.join("daemons.lock"), f.root.join("old.lock")).unwrap();
                fs::write(run.join("daemons.lock"), b"different inode").unwrap();
                fs::set_permissions(run.join("daemons.lock"), fs::Permissions::from_mode(0o600))
                    .unwrap();
            }
            1 => fs::set_permissions(&run, fs::Permissions::from_mode(0o777)).unwrap(),
            2 => {
                fs::rename(&run, f.root.join("old-run")).unwrap();
                symlink(f.root.join("old-run"), &run).unwrap();
            }
            _ => unreachable!(),
        }
        let before = snapshot(&f.home);
        assert!(lease.verify_live().is_err());
        assert!(registry.current("codex-fixture").is_err());
        deny_unchanged(&f, &registry, "fixture");
        assert_eq!(snapshot(&f.home), before);
    }
}

#[test]
fn c1_registry_read_projection_revalidates_lost_context_without_repair() {
    for relative in [".aperture/run/daemons", ".aperture/run"] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        publish(&registry, lease.identity().unwrap());
        let before = snapshot(&f.home);
        // The projection contains only a path, not the controller, child guard,
        // mutation closure or reusable process authority. Source/API review is
        // paired with these real functional read-context drift assertions.
        let path: &Path = registry.verified_run_dir().unwrap();
        assert_eq!(path, f.home.join(".aperture/run"));
        registry.verify_read_context().unwrap();
        assert_eq!(snapshot(&f.home), before);
        fs::remove_dir_all(f.home.join(relative)).unwrap();
        let after_loss = snapshot(&f.home);
        assert!(registry.verified_run_dir().is_err());
        assert!(registry.verify_read_context().is_err());
        assert!(probe_registered(&registry, "fixture").is_err());
        assert_eq!(snapshot(&f.home), after_loss);
    }
}

// C2b reuses the same test executable and private Unix fixture roots. No Codex.
struct C2Fixture {
    inner: Fixture,
}
impl C2Fixture {
    fn new() -> Self {
        let f = Fixture::new();
        fs::create_dir(f.root.join("codex-home")).unwrap();
        fs::set_permissions(f.root.join("codex-home"), fs::Permissions::from_mode(0o700)).unwrap();
        Self { inner: f }
    }
    fn spec(&self, fault: Option<&str>) -> super::NativeCodexSpec {
        super::NativeCodexSpec {
            seat: "fixture".into(),
            executable: std::env::current_exe().unwrap(),
            codex_home: self.inner.root.join("codex-home"),
            provenance: Provenance::LegacyUnknown,
            fixture: Some((self.inner.root.clone(), "direct".into())),
            fault: fault.map(str::to_owned),
        }
    }
    fn pid(&self) -> ProcessIdentity {
        wait_for(|| {
            fs::read(self.inner.root.join("c2-pid.json"))
                .ok()
                .is_some_and(|b| serde_json::from_slice::<ProcessIdentity>(&b).is_ok())
        });
        serde_json::from_slice(&fs::read(self.inner.root.join("c2-pid.json")).unwrap()).unwrap()
    }
    fn eof(&self, at_least: usize) {
        wait_for(|| {
            fs::read_to_string(self.inner.root.join("c2-counts"))
                .ok()
                .is_some_and(|v| {
                    let Some((n, b)) = v.split_once(':') else {
                        return false;
                    };
                    n.parse::<usize>().is_ok_and(|n| n >= at_least) && b == "0"
                })
        });
    }
}
impl Drop for C2Fixture {
    fn drop(&mut self) {
        if let Ok(bytes) = fs::read(self.inner.root.join("c2-pid.json")) {
            let identity: ProcessIdentity = serde_json::from_slice(&bytes).unwrap();
            match team_process::state(&identity) {
                ProcessState::Same => {
                    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGKILL) }, 0)
                }
                ProcessState::Gone => {}
                state => panic!("own C2 child identity became {state:?}"),
            }
            wait_for(|| {
                // Reap if this fixture process is the actual parent; adopted
                // orphan cleanup has no fabricated wait/exit observation.
                let mut status = 0;
                let rc = unsafe { libc::waitpid(identity.pid as i32, &mut status, libc::WNOHANG) };
                assert!(
                    rc >= 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
                );
                team_process::state(&identity) == ProcessState::Gone
            });
            eprintln!("C2 fixture own daemon reaped/Gone pid={}", identity.pid);
        }
        let path = self.inner.home.join(".aperture/logs/codex-fixture.log");
        // The negative log fixture is an actual FIFO with no reader/writer.
        // Diagnostic cleanup must not turn a bounded refusal into a blocking read.
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
        {
            if !file.metadata().unwrap().is_file() {
                eprintln!("C2 finite child diagnostic: non-regular leaf not read");
                return;
            }
            let mut data = vec![];
            file.take(8193).read_to_end(&mut data).unwrap();
            let truncated = data.len() > 8192;
            data.truncate(8192);
            eprintln!(
                "C2 finite child log truncated={truncated}: {}",
                String::from_utf8_lossy(&data)
            );
        }
        // inner Fixture subsequently removes and asserts absence of own root.
    }
}
static C2_TERMS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
extern "C" fn c2_term(_: libc::c_int) {
    C2_TERMS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}
#[test]
#[ignore = "inert C2 child/controller entry, invoked only by owned fixtures"]
fn c2_inert_native_entry() {
    let root = PathBuf::from(std::env::var_os("APERTURE_C2_FIXTURE").unwrap());
    assert!(
        root.starts_with("/private/tmp")
            && root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("ac-")
    );
    let mode = std::env::var("APERTURE_C2_MODE").unwrap();
    if mode == "boundary-fifo" || mode == "boundary-env" {
        c2_fix_boundary_controller(&root, &mode);
        return;
    }
    if mode == "controller" {
        let lease = ControllerLock::acquire(&root.join("home")).unwrap();
        let registry = Registry::open(&lease).unwrap();
        let spec = super::NativeCodexSpec {
            seat: "fixture".into(),
            executable: std::env::current_exe().unwrap(),
            codex_home: root.join("codex-home"),
            provenance: Provenance::LegacyUnknown,
            fixture: Some((root.clone(), "direct".into())),
            fault: None,
        };
        let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
        let observed = supervisor.reconcile().unwrap();
        fs::write(
            root.join("controller-ready.json"),
            serde_json::to_vec(&observed.identity).unwrap(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    assert!(mode == "direct" || mode == "link" || mode == "env-audit");
    if mode == "env-audit" {
        assert!(std::env::var_os("C2_SYNTHETIC_SENTINEL").is_none());
        for key in ["PATH", "TMPDIR", "LC_ALL"] {
            assert!(
                std::env::var_os(key).is_none(),
                "implicit fixture environment key {key}"
            );
        }
        assert_eq!(
            PathBuf::from(std::env::var_os("HOME").unwrap()),
            root.join("home")
        );
        assert_eq!(
            PathBuf::from(std::env::var_os("CODEX_HOME").unwrap()),
            root.join("codex-home")
        );
        assert_eq!(std::env::var("APERTURE_C2_MODE").unwrap(), "env-audit");
        assert_eq!(
            PathBuf::from(std::env::var_os("APERTURE_C2_SOCKET").unwrap()),
            root.join("home/.aperture/run/fixture.sock")
        );
        // Observe the actual inherited stdout/stderr descriptors, not Command's model.
        for fd in [1, 2] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            assert_eq!(flags & libc::O_NONBLOCK, 0);
            assert_ne!(flags & libc::O_APPEND, 0);
        }
        fs::write(
            root.join("env-flags-ok"),
            b"explicit-inputs-present; ambient-absent; regular-blocking-append",
        )
        .unwrap();
    }
    assert_ne!(
        unsafe { libc::signal(libc::SIGTERM, c2_term as *const () as libc::sighandler_t) },
        libc::SIG_ERR
    );
    let identity = team_process::observe(std::process::id())
        .unwrap()
        .unwrap()
        .identity;
    let pidfile = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join("c2-pid.json"))
        .unwrap();
    serde_json::to_writer(pidfile, &identity).unwrap();
    let socket = PathBuf::from(std::env::var_os("APERTURE_C2_SOCKET").unwrap());
    assert_eq!(socket, root.join("home/.aperture/run/fixture.sock"));
    let target = if mode == "link" {
        root.join("a".repeat(64))
    } else {
        socket.clone()
    };
    let listener = UnixListener::bind(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    if mode == "link" {
        symlink(&target, &socket).unwrap();
    }
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut connections = 0;
    while Instant::now() < deadline {
        let terms = C2_TERMS.load(std::sync::atomic::Ordering::SeqCst);
        if terms > 0 {
            fs::write(root.join("c2-terms"), terms.to_string()).unwrap();
            return;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(true).unwrap();
                let read_deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    assert!(Instant::now() < read_deadline, "C2 missing real EOF");
                    let mut bytes = [0; 32];
                    match stream.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => panic!("C2 unexpected payload bytes={n}"),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(e) => panic!("C2 read kind={:?} raw={:?}", e.kind(), e.raw_os_error()),
                    }
                }
                connections += 1;
                fs::write(root.join("c2-counts"), format!("{connections}:0")).unwrap();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(e) => panic!("C2 accept kind={:?} raw={:?}", e.kind(), e.raw_os_error()),
        }
    }
    panic!("C2 child exceeded fixture deadline");
}

#[test]
fn c2b_native_pristine_adopt_term_and_cleanup_no_successor() {
    use crate::daemon_registry::{CleanupOutcomeV2 as C, StopOutcomeV2 as S};
    let f = C2Fixture::new();
    let lease = f.inner.lease();
    let registry = Registry::open(&lease).unwrap();
    let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
    let result = supervisor.reconcile().unwrap();
    assert!(result.created);
    assert_eq!(result.identity, f.pid());
    let adopted = supervisor.reconcile().unwrap();
    assert!(!adopted.created);
    assert_eq!(adopted.wait, "NOT_OBSERVED");
    assert_eq!(result.identity, adopted.identity);
    f.eof(3);
    let id = registry
        .codex_snapshot("fixture")
        .unwrap()
        .unwrap()
        .incarnation;
    let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
    assert_eq!(
        supervisor.stop(&id).unwrap(),
        S::TermSentThenDaemonGoneDescendantsUnverified
    );
    assert_eq!(
        fs::read_to_string(f.inner.root.join("c2-terms")).unwrap(),
        "1"
    );
    assert!(supervisor.stop(&id).is_err());
    assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals + 1);
    let unlinks = crate::team_terminal::codex_unlink_calls();
    assert_eq!(supervisor.cleanup(&id).unwrap(), C::FixedDirectEntryRemoved);
    assert!(!f.inner.socket().exists());
    assert!(supervisor.cleanup(&id).is_err());
    assert!(supervisor.reconcile().is_err());
    assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks + 1);
    let slot = lease.codex_slot("fixture").unwrap();
    let op = slot.enter().unwrap();
    assert_eq!(op.spawn_attempts(), 1);
}

#[test]
fn c2b_two_callers_one_native_spawn_and_unrelated_unknown_not_global_ready() {
    let f = C2Fixture::new();
    let lease = f.inner.lease();
    let registry = Registry::open(&lease).unwrap();
    {
        let s = lease.codex_slot("unrelated").unwrap();
        let op = s.enter().unwrap();
        registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 1)
            .unwrap();
    }
    let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| supervisor.reconcile().unwrap());
        let b = scope.spawn(|| supervisor.reconcile().unwrap());
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_ne!(a.created, b.created);
    assert_eq!(a.identity, b.identity);
    f.eof(3);
    let slot = lease.codex_slot("fixture").unwrap();
    let op = slot.enter().unwrap();
    assert_eq!(op.spawn_attempts(), 1);
    assert!(registry.inspect().is_err());
}

#[test]
fn c2b_postspawn_faults_keep_wait_handle_and_never_retry_or_signal() {
    for fault in [
        "post-spawn-retained",
        "spawned",
        "recorded",
        "ready",
        "published",
    ] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let supervisor =
            super::NativeCodexSupervisor::new(&lease, &registry, f.spec(Some(fault))).unwrap();
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        assert!(supervisor.reconcile().is_err());
        let identity = f.pid();
        {
            let slot = lease.codex_slot("fixture").unwrap();
            let mut op = slot.enter().unwrap();
            assert_eq!(op.retained_pid(), Some(identity.pid));
            assert_eq!(op.spawn_attempts(), 1);
            assert_eq!(
                op.try_wait().unwrap(),
                crate::controller::WaitObservation::Running
            );
        }
        let next = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None))
            .unwrap()
            .reconcile();
        if fault == "published" {
            assert!(!next.unwrap().created);
        } else {
            assert!(next.is_err());
        }
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        assert_eq!(op.spawn_attempts(), 1);
    }
}

#[test]
fn c2b_crash_stop_cleanup_intents_and_posteffect_never_retry() {
    for fault in [
        "stop-intent",
        "term-sent",
        "cleanup-intent",
        "cleanup-effect",
    ] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let good = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
        good.reconcile().unwrap();
        f.eof(2);
        let id = registry
            .codex_snapshot("fixture")
            .unwrap()
            .unwrap()
            .incarnation;
        let faulty =
            super::NativeCodexSupervisor::new(&lease, &registry, f.spec(Some(fault))).unwrap();
        if fault.starts_with("stop") || fault == "term-sent" {
            assert!(faulty.stop(&id).is_err());
        } else {
            good.stop(&id).unwrap();
            assert!(faulty.cleanup(&id).is_err());
        }
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        assert!(good.stop(&id).is_err());
        assert!(good.cleanup(&id).is_err());
        assert!(good.reconcile().is_err());
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        assert_eq!(f.inner.socket().exists(), fault != "cleanup-effect");
    }
}

#[test]
fn c2b_preconditions_never_spawn_signal_or_unlink_and_v1_is_not_enrolled() {
    for case in [
        "unregistered-socket",
        "v1",
        "invalid-selector",
        "missing-config",
        "orphan",
        "intent",
    ] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let listener = if case == "unregistered-socket" {
            Some(UnixListener::bind(f.inner.socket()).unwrap())
        } else {
            None
        };
        if case == "v1" {
            let r = registry
                .reserve(
                    Endpoint::CodexAppServer {
                        seat: "fixture".into(),
                    },
                    Provenance::LegacyUnknown,
                    1,
                )
                .unwrap();
            let r = registry.record(&r, lease.identity().unwrap(), 2).unwrap();
            registry.publish_current(&r).unwrap();
        }
        if case == "orphan" {
            fs::write(
                f.inner.home.join(".aperture/run/daemons/orphan"),
                b"unknown",
            )
            .unwrap();
        }
        if case == "missing-config" {
            fs::remove_dir(f.inner.home.join(".claude/aperture/fixture")).unwrap();
        }
        let mut spec = f.spec(if case == "intent" {
            Some("intent")
        } else {
            None
        });
        if case == "invalid-selector" {
            spec.seat = "../managed".into();
        }
        let before = snapshot(&f.inner.home);
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        let result =
            super::NativeCodexSupervisor::new(&lease, &registry, spec).and_then(|s| s.reconcile());
        assert!(result.is_err());
        assert!(!f.inner.root.join("c2-pid.json").exists());
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        if case != "intent" {
            assert_eq!(snapshot(&f.inner.home), before);
        }
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        assert_eq!(op.spawn_attempts(), 0);
        drop(listener);
    }
}

#[test]
fn c2b_real_peer_pins_and_snapshot_drift_deny_before_effects() {
    use crate::daemon_registry::Identity;
    let f = C2Fixture::new();
    let lease = f.inner.lease();
    let registry = Registry::open(&lease).unwrap();
    let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
    supervisor.reconcile().unwrap();
    f.eof(2);
    let original = registry.codex_snapshot("fixture").unwrap().unwrap();
    let dir = f.inner.home.join(".aperture/run/daemons/codex-fixture");
    let pinsfile = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("v2-002-")
        })
        .unwrap();
    let original_bytes = fs::read(&pinsfile).unwrap();
    for case in [
        "dev", "ino", "uid", "mode", "links", "length", "order", "revision",
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
        if case == "revision" {
            value["at_ms"] = serde_json::json!(value["at_ms"].as_u64().unwrap() + 1);
        } else {
            let pins = &mut value["event"]["pins"];
            if case == "length" {
                pins["parents"].as_array_mut().unwrap().pop();
            } else if case == "order" {
                pins["parents"].as_array_mut().unwrap().swap(0, 1);
            } else {
                let v = pins["entry"][case].as_u64().unwrap();
                pins["entry"][case] = serde_json::json!(v + 1);
            }
        }
        crate::journal::write_private_json_atomic(&pinsfile, &value, true).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        assert!(crate::team_terminal::codex_live_pins(
            &registry,
            &op,
            &original,
            &crate::team_terminal::CodexSocketResolver::fixture(&f.inner.root).unwrap()
        )
        .is_err());
        drop(op);
        drop(slot);
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        if case != "revision" {
            assert!(supervisor.stop(&original.incarnation).is_err());
        }
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        let original_value: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
        crate::journal::write_private_json_atomic(&pinsfile, &original_value, true).unwrap();
    }
    let processfile = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("v2-001-")
        })
        .unwrap();
    let process_bytes = fs::read(&processfile).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&process_bytes).unwrap();
    value["event"]["process"] =
        serde_json::to_value(Identity::from_native(lease.identity().unwrap()).unwrap()).unwrap();
    crate::journal::write_private_json_atomic(&processfile, &value, true).unwrap();
    // Claimed identity is genuinely Same, but the kernel peer is the daemon.
    let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
    assert!(supervisor.stop(&original.incarnation).is_err());
    assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
    crate::journal::write_private_json_atomic(
        &processfile,
        &serde_json::from_slice::<serde_json::Value>(&process_bytes).unwrap(),
        true,
    )
    .unwrap();
}

#[test]
fn c2b_own_controller_term_kill_survival_then_same_identity_adoption() {
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        let mut f = C2Fixture::new();
        let log = fs::File::create(f.inner.root.join("controller.log")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "codex_appserver::registered_tests::c2_inert_native_entry",
                "--ignored",
                "--nocapture",
            ])
            .env("APERTURE_C2_FIXTURE", &f.inner.root)
            .env("APERTURE_C2_MODE", "controller")
            .env("HOME", &f.inner.home)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let identity = team_process::observe(child.id()).unwrap().unwrap().identity;
        f.inner.children.push(OwnedChild {
            child,
            identity: identity.clone(),
        });
        wait_for(|| {
            fs::read(f.inner.root.join("controller-ready.json"))
                .ok()
                .is_some_and(|b| serde_json::from_slice::<ProcessIdentity>(&b).is_ok())
        });
        let daemon = f.pid();
        assert_eq!(team_process::state(&daemon), ProcessState::Same);
        // Fixture-owner action on its own synthetic controller, NOT product policy.
        assert_eq!(team_process::state(&identity), ProcessState::Same);
        assert_eq!(unsafe { libc::kill(identity.pid as i32, signal) }, 0);
        f.inner.children[0].child.wait().unwrap();
        assert_eq!(team_process::state(&identity), ProcessState::Gone);
        assert_eq!(team_process::state(&daemon), ProcessState::Same);
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let supervisor =
            super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
        let adopted = supervisor.reconcile().unwrap();
        assert_eq!(adopted.identity, daemon);
        assert!(!adopted.created);
        assert_eq!(adopted.wait, "NOT_OBSERVED");
        let slot = lease.codex_slot("fixture").unwrap();
        let mut op = slot.enter().unwrap();
        assert_eq!(op.spawn_attempts(), 0);
        assert_eq!(
            op.try_wait().unwrap(),
            crate::controller::WaitObservation::NotObserved
        );
        f.eof(3);
    }
}

#[test]
fn c2b_native_link_full_supervisor_retains_target_and_denies_successor() {
    use crate::daemon_registry::{CleanupOutcomeV2 as C, StopOutcomeV2 as S};
    let f = C2Fixture::new();
    let lease = f.inner.lease();
    let registry = Registry::open(&lease).unwrap();
    let mut spec = f.spec(None);
    spec.fixture.as_mut().unwrap().1 = "link".into();
    let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
    let made = supervisor.reconcile().unwrap();
    assert!(made.created);
    let adopted = supervisor.reconcile().unwrap();
    assert!(!adopted.created);
    assert_eq!(made.identity, adopted.identity);
    f.eof(3);
    let snapshot = registry.codex_snapshot("fixture").unwrap().unwrap();
    let target = f.inner.root.join("a".repeat(64));
    assert_eq!(fs::read_link(f.inner.socket()).unwrap(), target);
    assert_eq!(
        snapshot.pins.unwrap().native_target.unwrap().basename,
        "a".repeat(64)
    );
    assert_eq!(
        supervisor.stop(&snapshot.incarnation).unwrap(),
        S::TermSentThenDaemonGoneDescendantsUnverified
    );
    let unlinks = crate::team_terminal::codex_unlink_calls();
    assert_eq!(
        supervisor.cleanup(&snapshot.incarnation).unwrap(),
        C::FixedLinkRemovedTargetRetained
    );
    assert!(f.inner.socket().symlink_metadata().is_err());
    assert!(target.exists());
    assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks + 1);
    assert!(supervisor.reconcile().is_err());
    assert!(supervisor.cleanup(&snapshot.incarnation).is_err());
    assert!(target.exists());
    assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks + 1);
    // Fixture teardown removes its own retained target separately, not product cleanup.
}

#[test]
fn c2b_actual_postspawn_lease_loss_retains_handle_without_new_authority() {
    use crate::controller::CodexSpawnError;
    use std::os::unix::{ffi::OsStrExt, process::CommandExt};
    let f = C2Fixture::new();
    let lease = f.inner.lease();
    let registry = Registry::open(&lease).unwrap();
    let slot = lease.codex_slot("fixture").unwrap();
    let mut op = slot.enter().unwrap();
    registry
        .begin_codex_v2(&op, Provenance::LegacyUnknown, 1)
        .unwrap();
    let lock = std::ffi::CString::new(
        f.inner
            .home
            .join(".aperture/run/daemons.lock")
            .as_os_str()
            .as_bytes(),
    )
    .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "codex_appserver::registered_tests::c2_inert_native_entry",
            "--ignored",
            "--nocapture",
        ])
        .env("HOME", &f.inner.home)
        .env("APERTURE_C2_FIXTURE", &f.inner.root)
        .env("APERTURE_C2_MODE", "direct")
        .env("APERTURE_C2_SOCKET", f.inner.socket())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Fixture fault at actual child pre-exec: only async-signal-safe syscalls,
    // on its own synthetic controller path; no production recovery behavior.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 || libc::unlink(lock.as_ptr()) != 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    assert_eq!(
        op.spawn_retained(&mut command),
        Err(CodexSpawnError::PostSpawnUnknown)
    );
    let identity = f.pid();
    assert_eq!(op.retained_pid(), Some(identity.pid));
    assert_eq!(op.spawn_attempts(), 1);
    assert!(
        op.try_wait().is_err(),
        "stale lease has no operation authority"
    );
    assert_eq!(
        op.spawn_retained(&mut command),
        Err(CodexSpawnError::Refused)
    );
    assert_eq!(op.spawn_attempts(), 1);
    assert_eq!(team_process::state(&identity), ProcessState::Same);
}

#[test]
fn c2b_cleanup_ambiguity_and_revision_drift_keep_unknown_zero_unlink() {
    use crate::daemon_registry::{CleanupOutcomeV2 as C, CodexEventV2 as E};
    for case in [
        "missing",
        "wrong-listener",
        "mode",
        "revision",
        "lost-lease",
    ] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let supervisor =
            super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
        supervisor.reconcile().unwrap();
        f.eof(2);
        let id = registry
            .codex_snapshot("fixture")
            .unwrap()
            .unwrap()
            .incarnation;
        supervisor.stop(&id).unwrap();
        let before = crate::team_terminal::codex_unlink_calls();
        let mut listener = None;
        if case == "missing" {
            fs::remove_file(f.inner.socket()).unwrap();
        }
        if case == "wrong-listener" {
            fs::remove_file(f.inner.socket()).unwrap();
            listener = Some(UnixListener::bind(f.inner.socket()).unwrap());
            fs::set_permissions(f.inner.socket(), fs::Permissions::from_mode(0o600)).unwrap();
        }
        if case == "mode" {
            fs::set_permissions(f.inner.socket(), fs::Permissions::from_mode(0o640)).unwrap();
        }
        if case == "revision" || case == "lost-lease" {
            let slot = lease.codex_slot("fixture").unwrap();
            let op = slot.enter().unwrap();
            registry
                .append_codex_v2(&op, &id, E::CleanupIntent {}, u64::MAX - 1)
                .unwrap();
            let expected = registry.codex_snapshot("fixture").unwrap().unwrap();
            if case == "revision" {
                let dir = f.inner.home.join(".aperture/run/daemons/codex-fixture");
                let file = fs::read_dir(&dir)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .starts_with("v2-006-")
                    })
                    .unwrap();
                let mut value: serde_json::Value =
                    serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
                value["at_ms"] = serde_json::json!(u64::MAX);
                crate::journal::write_private_json_atomic(&file, &value, true).unwrap();
            } else {
                fs::remove_file(f.inner.home.join(".aperture/run/daemons.lock")).unwrap();
            }
            assert!(crate::team_terminal::codex_cleanup_fixed(
                &registry,
                &op,
                &expected,
                &crate::team_terminal::CodexSocketResolver::fixture(&f.inner.root).unwrap()
            )
            .is_err());
        } else {
            assert_eq!(supervisor.cleanup(&id).unwrap(), C::Unknown);
        }
        assert_eq!(crate::team_terminal::codex_unlink_calls(), before);
        assert!(supervisor.cleanup(&id).is_err());
        assert!(supervisor.reconcile().is_err());
        assert_eq!(crate::team_terminal::codex_unlink_calls(), before);
        drop(listener);
    }
}

#[test]
fn c2b_malformed_namespace_denies_stop_and_cleanup_without_target_mutation() {
    for stage in ["stop", "cleanup"] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
        supervisor.reconcile().unwrap();
        f.eof(2);
        let id = registry.codex_snapshot("fixture").unwrap().unwrap().incarnation;
        if stage == "cleanup" { supervisor.stop(&id).unwrap(); }
        fs::write(f.inner.home.join(".aperture/run/daemons/orphan"), b"unknown").unwrap();
        let before = snapshot(&f.inner.home);
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        assert!(supervisor.stop(&id).is_err());
        assert!(supervisor.cleanup(&id).is_err());
        assert!(supervisor.reconcile().is_err());
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        assert_eq!(snapshot(&f.inner.home), before);
    }
}

#[test]
fn c2b_injected_signal_and_observation_errors_remain_unknown_without_resend() {
    use crate::daemon_registry::{StopOutcomeV2 as O, CodexPhaseV2 as P};
    for fault in ["term-esrch-injected", "term-error-injected", "term-timeout-injected",
        "term-recycled-injected", "term-unreadable-injected", "term-wait-error-injected"] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let good = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(None)).unwrap();
        good.reconcile().unwrap();
        f.eof(2);
        let id = registry.codex_snapshot("fixture").unwrap().unwrap().incarnation;
        let faulty = super::NativeCodexSupervisor::new(&lease, &registry, f.spec(Some(fault))).unwrap();
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        assert_eq!(faulty.stop(&id).unwrap(), O::Unknown);
        let expected_calls = if fault == "term-esrch-injected" || fault == "term-error-injected" { 0 } else { 1 };
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals + expected_calls);
        assert_eq!(registry.codex_snapshot("fixture").unwrap().unwrap().phase, P::Unknown);
        assert!(good.stop(&id).is_err());
        assert!(good.cleanup(&id).is_err());
        assert!(good.reconcile().is_err());
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals + expected_calls);
        // Explicit fault injections are not claims of observed OS PID reuse,
        // real ESRCH or a native wait failure. The successful TERM path is real.
    }
}


// Deterministic cfg(test) drift after the real durable SpawnIntent. The mutation
// target must be the fixture's private copy, never the shared Cargo test image.
pub(super) fn c2_fix_executable_drift(spec: &super::NativeCodexSpec) -> Result<(), String> {
    let fault = spec.fault.as_deref();
    if !matches!(fault, Some("exec-mode-drift" | "exec-inode-drift")) {
        return Ok(());
    }
    let root = &spec.fixture.as_ref().expect("private drift fixture").0;
    assert_eq!(spec.executable, root.join("private-executable"));
    assert_ne!(spec.executable, std::env::current_exe().unwrap());
    assert_eq!(fs::symlink_metadata(&spec.executable).unwrap().nlink(), 1);
    if fault == Some("exec-mode-drift") {
        // Still a valid executable mode; equality of the captured pin must deny.
        fs::set_permissions(&spec.executable, fs::Permissions::from_mode(0o500)).unwrap();
    } else {
        fs::rename(&spec.executable, root.join("retained-original-executable")).unwrap();
        fs::write(&spec.executable, b"not executed: private replacement").unwrap();
        fs::set_permissions(&spec.executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    Ok(())
}

fn c2_fix_boundary_controller(root: &Path, mode: &str) {
    use crate::daemon_registry::{CodexPhaseV2, StopOutcomeV2};
    assert_eq!(
        std::env::var("C2_SYNTHETIC_SENTINEL").unwrap(),
        "synthetic-only"
    );
    let home = root.join("home");
    let lease = ControllerLock::acquire(&home).unwrap();
    let registry = Registry::open(&lease).unwrap();
    let spec = super::NativeCodexSpec {
        seat: "fixture".into(),
        executable: std::env::current_exe().unwrap(),
        codex_home: root.join("codex-home"),
        provenance: Provenance::LegacyUnknown,
        fixture: Some((root.into(), "env-audit".into())),
        fault: None,
    };
    let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
    let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
    let unlinks = crate::team_terminal::codex_unlink_calls();
    if mode == "boundary-fifo" {
        let logs = home.join(".aperture/logs");
        crate::journal::ensure_private_dir(&logs).unwrap();
        let fifo = logs.join("codex-fixture.log");
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let before = fs::symlink_metadata(&fifo).unwrap();
        let start = Instant::now();
        assert_eq!(supervisor.reconcile().unwrap_err(), super::CODEX_UNKNOWN);
        assert!(start.elapsed() < Duration::from_secs(2));
        let intent = registry.codex_snapshot("fixture").unwrap().unwrap();
        assert_eq!(intent.phase, CodexPhaseV2::SpawnIntentUnknown);
        let history = snapshot(&home.join(".aperture/run/daemons"));
        assert!(supervisor.reconcile().is_err());
        assert_eq!(snapshot(&home.join(".aperture/run/daemons")), history);
        assert_eq!(registry.codex_snapshot("fixture").unwrap().unwrap(), intent);
        let after = fs::symlink_metadata(&fifo).unwrap();
        assert_eq!(
            (before.dev(), before.ino(), before.mode()),
            (after.dev(), after.ino(), after.mode())
        );
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        assert!(!root.join("c2-pid.json").exists());
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        assert_eq!(op.spawn_attempts(), 0);
        assert!(op.retained_pid().is_none());
        fs::write(
            root.join("boundary-ok"),
            b"fifo-bounded; intent-retained; zero-spawn-term-unlink",
        )
        .unwrap();
    } else {
        let spawned = supervisor.reconcile().unwrap();
        assert!(spawned.created);
        assert!(root.join("env-flags-ok").is_file());
        let log = fs::symlink_metadata(home.join(".aperture/logs/codex-fixture.log")).unwrap();
        assert!(log.is_file());
        assert_eq!(
            (log.uid(), log.mode() & 0o777, log.nlink()),
            (unsafe { libc::geteuid() }, 0o600, 1)
        );
        let id = registry
            .codex_snapshot("fixture")
            .unwrap()
            .unwrap()
            .incarnation;
        assert_eq!(
            supervisor.stop(&id).unwrap(),
            StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified
        );
        assert_eq!(team_process::state(&spawned.identity), ProcessState::Gone);
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals + 1);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        fs::write(
            root.join("boundary-ok"),
            b"environment-and-log-flags; native-child-reaped-gone",
        )
        .unwrap();
    }
}

// Same existing inert entry and identity-owned cleanup. The parent enforces a
// deadline even if the FIFO regression blocks inside the controller's syscall.
fn c2_fix_run_boundary(mode: &str) {
    let mut f = C2Fixture::new();
    let log = fs::File::create(f.inner.root.join("child.log")).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "codex_appserver::registered_tests::c2_inert_native_entry",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("APERTURE_C2_FIXTURE", &f.inner.root)
        .env("APERTURE_C2_MODE", mode)
        .env("C2_SYNTHETIC_SENTINEL", "synthetic-only")
        .env("PATH", "/synthetic/not-a-path")
        .env("TMPDIR", "/synthetic/not-a-tmp")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let identity = team_process::observe(child.id()).unwrap().unwrap().identity;
    f.inner.children.push(OwnedChild {
        child,
        identity: identity.clone(),
    });
    let mut status = None;
    wait_for(|| {
        status = f.inner.children[0].child.try_wait().unwrap();
        status.is_some()
    });
    assert!(
        status.unwrap().success(),
        "bounded synthetic boundary controller failed"
    );
    assert_eq!(team_process::state(&identity), ProcessState::Gone);
    assert!(f.inner.root.join("boundary-ok").is_file());
}

#[test]
fn c2b_fix_fifo_no_reader_is_bounded_unknown_without_effects() {
    c2_fix_run_boundary("boundary-fifo");
}
#[test]
fn c2b_fix_explicit_environment_and_regular_log_flags_reach_native_child() {
    c2_fix_run_boundary("boundary-env");
}
#[test]
fn c2b_fix_executable_mode_and_inode_drift_after_intent_deny_spawn() {
    for fault in ["exec-mode-drift", "exec-inode-drift"] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let mut spec = f.spec(Some(fault));
        spec.executable = f.inner.root.join("private-executable");
        fs::copy(std::env::current_exe().unwrap(), &spec.executable).unwrap();
        fs::set_permissions(&spec.executable, fs::Permissions::from_mode(0o700)).unwrap();
        let before = super::CodexExecutablePin::capture(&spec.executable).unwrap();
        let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        assert_eq!(supervisor.reconcile().unwrap_err(), super::CODEX_UNKNOWN);
        assert_ne!(
            super::CodexExecutablePin::capture(&supervisor.spec.executable).unwrap(),
            before
        );
        assert_eq!(
            registry.codex_snapshot("fixture").unwrap().unwrap().phase,
            crate::daemon_registry::CodexPhaseV2::SpawnIntentUnknown
        );
        let facts = snapshot(&f.inner.home.join(".aperture/run/daemons"));
        assert!(supervisor.reconcile().is_err());
        assert_eq!(snapshot(&f.inner.home.join(".aperture/run/daemons")), facts);
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        assert_eq!(op.spawn_attempts(), 0);
        assert!(op.retained_pid().is_none());
    }
}
#[test]
fn c2b_fix_unsafe_executable_denied_before_intent() {
    for case in [
        "group-write",
        "world-write",
        "no-exec",
        "symlink",
        "hardlink",
        "directory",
    ] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let mut spec = f.spec(None);
        let private = f.inner.root.join("private-executable");
        fs::write(&private, b"private inert nonexecuted bytes").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        spec.executable = private.clone();
        match case {
            "group-write" => {
                fs::set_permissions(&private, fs::Permissions::from_mode(0o720)).unwrap()
            }
            "world-write" => {
                fs::set_permissions(&private, fs::Permissions::from_mode(0o702)).unwrap()
            }
            "no-exec" => fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap(),
            "symlink" => {
                spec.executable = f.inner.root.join("exec-link");
                symlink(&private, &spec.executable).unwrap();
            }
            "hardlink" => fs::hard_link(&private, f.inner.root.join("exec-hardlink")).unwrap(),
            "directory" => {
                spec.executable = f.inner.root.join("exec-dir");
                fs::create_dir(&spec.executable).unwrap();
            }
            _ => unreachable!(),
        }
        let before = snapshot(&f.inner.home);
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
        assert!(supervisor.reconcile().is_err());
        assert_eq!(snapshot(&f.inner.home), before);
        assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
        assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
        let slot = lease.codex_slot("fixture").unwrap();
        assert_eq!(slot.enter().unwrap().spawn_attempts(), 0);
    }
}
#[test]
fn c2b_fix_adoption_provenance_must_match_without_relabel_or_effects() {
    let release = |c: &str| Provenance::Release {
        release_sha: c.repeat(40),
    };
    for stored in [Provenance::LegacyUnknown, release("a")] {
        let f = C2Fixture::new();
        let lease = f.inner.lease();
        let registry = Registry::open(&lease).unwrap();
        let mut spec = f.spec(None);
        spec.provenance = stored.clone();
        let supervisor = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
        let created = supervisor.reconcile().unwrap();
        f.eof(2);
        let before = snapshot(&f.inner.home.join(".aperture/run/daemons"));
        let signals = super::C2_SIGNAL_CALLS.with(|v| v.get());
        let unlinks = crate::team_terminal::codex_unlink_calls();
        for requested in [Provenance::LegacyUnknown, release("a"), release("b")] {
            let mut spec = f.spec(None);
            spec.provenance = requested.clone();
            let observer = super::NativeCodexSupervisor::new(&lease, &registry, spec).unwrap();
            let result = observer.reconcile();
            if requested == stored {
                let result = result.unwrap();
                assert_eq!(result.identity, created.identity);
                assert!(!result.created);
                assert_eq!(result.wait, "NOT_OBSERVED");
            } else {
                assert_eq!(result.unwrap_err(), super::CODEX_UNKNOWN);
            }
            assert_eq!(
                snapshot(&f.inner.home.join(".aperture/run/daemons")),
                before
            );
            assert_eq!(
                registry
                    .codex_snapshot("fixture")
                    .unwrap()
                    .unwrap()
                    .provenance,
                stored
            );
            assert_eq!(super::C2_SIGNAL_CALLS.with(|v| v.get()), signals);
            assert_eq!(crate::team_terminal::codex_unlink_calls(), unlinks);
            let slot = lease.codex_slot("fixture").unwrap();
            assert_eq!(slot.enter().unwrap().spawn_attempts(), 1);
        }
    }
}
