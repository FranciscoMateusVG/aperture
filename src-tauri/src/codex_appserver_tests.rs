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
