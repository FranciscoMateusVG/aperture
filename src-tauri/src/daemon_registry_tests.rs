use super::*;
use std::{
    cell::Cell,
    os::unix::fs::{symlink, PermissionsExt},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("aperture-daemon-registry-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn lease(&self) -> ControllerLock {
        ControllerLock::acquire(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn reserve(registry: &Registry<'_>) -> Reservation {
    registry
        .reserve(
            Endpoint::Hub { port: 4517 },
            Provenance::LegacyUnknown,
            1000,
        )
        .unwrap()
}
fn record(registry: &Registry<'_>, reservation: &Reservation) -> Record {
    // Native identity of this test process, metadata only. This neither adopts
    // the process nor claims to have spawned a daemon; no signal/CLI is invoked.
    registry
        .record(reservation, registry.lease.identity().unwrap(), 1001)
        .unwrap()
}
fn published(registry: &Registry<'_>) -> Record {
    let reservation = reserve(registry);
    let r = record(registry, &reservation);
    registry.publish_current(&r).unwrap();
    r
}
fn error<T>(r: Result<T>, code: &str) {
    let e = r.err().expect("must reject");
    assert!(e.starts_with(code), "expected {code}; got {e}");
}
fn downstream(lease: &ControllerLock, code: &str) {
    let token = Cell::new(0);
    let poller = Cell::new(0);
    let hub = Cell::new(0);
    let watchdog = Cell::new(0);
    // Same preflight function invoked by daemons::start, with only the native
    // effects replaced. Never call production token/threads in a test.
    error(
        crate::daemons::start_checked(lease, || {
            token.set(token.get() + 1);
            poller.set(poller.get() + 1);
            hub.set(hub.get() + 1);
            watchdog.set(watchdog.get() + 1);
            Ok(())
        }),
        code,
    );
    assert_eq!(
        [token.get(), poller.get(), hub.get(), watchdog.get()],
        [0; 4]
    );
}
fn write<T: Serialize>(path: &Path, value: &T) {
    journal::write_private_json_atomic(path, value, false).unwrap();
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(p.strip_prefix(root).unwrap().into(), fs::read(p).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[test]
fn pristine_registry_is_not_permission_to_start_legacy_supervision() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    assert!(registry.inspect().unwrap().is_empty());
    downstream(&lease, "E_DAEMON_SUPERVISION_PENDING");
    assert!(!f.0.join(".aperture/hub-tokens").exists());
}

#[test]
fn unknown_listener_without_registry_never_reaches_legacy_kill_path() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let f = Fixture::new();
    let lease = f.lease();
    downstream(&lease, "E_DAEMON_SUPERVISION_PENDING");
    assert_eq!(listener.local_addr().unwrap(), address);
    assert!(std::net::TcpStream::connect(address).is_ok());
    assert!(listener.accept().is_ok());
}

#[test]
fn reservation_blocks_even_when_endpoint_is_free_and_new_uuid_requested() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    registry
        .reserve(Endpoint::Hub { port }, Provenance::LegacyUnknown, 1000)
        .unwrap();
    let before = snapshot(&registry.root);
    downstream(&lease, "E_DAEMON_RESERVATION_INCOMPLETE");
    error(
        registry.reserve(Endpoint::Hub { port }, Provenance::LegacyUnknown, 2000),
        "E_DAEMON_RESERVATION_INCOMPLETE",
    );
    assert_eq!(snapshot(&registry.root), before);
}

#[test]
fn each_publication_edge_is_incomplete_until_current_and_history_are_complete() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    // A crash after mkdir but before reservation is ambiguous too.
    registry.slot_path("hub", true).unwrap();
    downstream(&lease, "E_DAEMON_RESERVATION_INCOMPLETE");
    fs::remove_dir(registry.root.join("hub")).unwrap(); // fixture reset, not product recovery
    let reservation = reserve(&registry);
    downstream(&lease, "E_DAEMON_RESERVATION_INCOMPLETE"); // includes spawn -> record crash
    let r = record(&registry, &reservation);
    downstream(&lease, "E_DAEMON_PUBLICATION_INCOMPLETE"); // record -> current crash
    registry.publish_current(&r).unwrap();
    let inspected = registry.inspect().unwrap();
    assert_eq!(inspected.len(), 1);
    assert_eq!(inspected[0].identity, ProcessState::Same);
    assert_eq!(inspected[0].provenance, Provenance::LegacyUnknown);
    downstream(&lease, "E_DAEMON_SUPERVISION_PENDING"); // Same is NOT Adopted
    fs::remove_file(registry.root.join("hub/current")).unwrap();
    downstream(&lease, "E_DAEMON_PUBLICATION_INCOMPLETE"); // history is not Absent
}

#[test]
fn immutable_record_reservation_and_current_are_not_overwritten() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let reservation = reserve(&registry);
    let r = record(&registry, &reservation);
    let before = snapshot(&registry.root);
    error(
        registry.record(&reservation, lease.identity().unwrap(), 1002),
        "E_NAME_COLLISION",
    );
    assert_eq!(snapshot(&registry.root), before);
    registry.publish_current(&r).unwrap();
    let before = snapshot(&registry.root);
    error(registry.publish_current(&r), "E_DAEMON_RECORD");
    error(
        registry.reserve(
            Endpoint::Hub { port: 4517 },
            Provenance::LegacyUnknown,
            2000,
        ),
        "E_DAEMON_IDENTITY_UNVERIFIED",
    );
    assert_eq!(snapshot(&registry.root), before);
}

#[test]
fn stale_current_cannot_hide_new_record_or_unfinished_reservation() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let first = published(&registry);
    let old = snapshot(&registry.root);
    let mut next = first.reservation.clone();
    next.incarnation = Uuid::new_v4().to_string();
    next.previous = Some(first.reservation.incarnation.clone());
    next.reserved_at_ms = 2000;
    let dir = registry.root.join("hub");
    write(
        &dir.join(format!("reservation-{}.json", next.incarnation)),
        &next,
    );
    downstream(&lease, "E_DAEMON_RESERVATION_INCOMPLETE");
    let second = Record {
        schema_version: 1,
        reservation: next.clone(),
        process: first.process,
        spawned_at_ms: 2001,
    };
    write(&dir.join(format!("{}.json", next.incarnation)), &second);
    downstream(&lease, "E_DAEMON_PUBLICATION_INCOMPLETE");
    // Atomic advance of current makes the complete chain reachable; all old
    // immutable facts remain. This fixture does not claim a real respawn.
    registry.publish_current(&second).unwrap();
    assert_eq!(registry.inspect().unwrap()[0].incarnation, next.incarnation);
    let now = snapshot(&registry.root);
    for (path, bytes) in old {
        if path != Path::new("hub/current") {
            assert_eq!(now.get(&path), Some(&bytes));
        }
    }
    downstream(&lease, "E_DAEMON_SUPERVISION_PENDING");
}

#[test]
fn schema_malformed_and_unknown_fields_reject_before_downstream_effects() {
    for case in ["future_reservation", "future_record", "malformed", "extra"] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let r = published(&registry);
        let reservation = case == "future_reservation";
        let name = if reservation {
            format!("reservation-{}.json", r.reservation.incarnation)
        } else {
            format!("{}.json", r.reservation.incarnation)
        };
        let path = registry.root.join("hub").join(name);
        if case == "malformed" {
            fs::write(&path, b"{").unwrap();
        } else {
            let mut v = if reservation {
                serde_json::to_value(&r.reservation).unwrap()
            } else {
                serde_json::to_value(&r).unwrap()
            };
            if case == "extra" {
                v["caller_authority"] = serde_json::json!("forbidden");
            } else {
                v["schema_version"] = serde_json::json!(2);
            }
            fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        }
        downstream(
            &lease,
            if case.starts_with("future") {
                "E_DAEMON_SCHEMA"
            } else {
                "E_DAEMON_RECORD"
            },
        );
    }
}

#[test]
fn paths_symlinks_hardlinks_and_unsafe_modes_fail_closed() {
    for case in ["root", "slot", "record", "hardlink", "mode", "traversal"] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let r = published(&registry);
        let file = registry
            .root
            .join("hub")
            .join(format!("{}.json", r.reservation.incarnation));
        match case {
            "root" => {
                let target = f.0.join("elsewhere");
                fs::rename(&registry.root, &target).unwrap();
                symlink(target, &registry.root).unwrap();
            }
            "slot" => {
                let target = f.0.join("elsewhere");
                fs::rename(registry.root.join("hub"), &target).unwrap();
                symlink(target, registry.root.join("hub")).unwrap();
            }
            "record" => {
                let target = f.0.join("elsewhere");
                fs::rename(&file, &target).unwrap();
                symlink(target, &file).unwrap();
            }
            "hardlink" => {
                fs::hard_link(&file, f.0.join("other-link")).unwrap();
            }
            "mode" => {
                fs::set_permissions(registry.root.join("hub"), fs::Permissions::from_mode(0o777))
                    .unwrap();
            }
            "traversal" => {
                fs::create_dir(registry.root.join("unexpected-slot")).unwrap();
            }
            _ => unreachable!(),
        }
        downstream(
            &lease,
            if case == "hardlink" {
                "E_DAEMON_RECORD"
            } else {
                "E_DAEMON_PATH"
            },
        );
    }
}

#[test]
fn orphan_record_dangling_current_and_interrupted_atomic_temp_never_mean_absent() {
    for case in ["orphan", "dangling", "temp"] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let r = published(&registry);
        let dir = registry.root.join("hub");
        match case {
            "orphan" => {
                fs::remove_file(dir.join(format!("reservation-{}.json", r.reservation.incarnation)))
                    .unwrap()
            }
            "dangling" => journal::write_private_json_atomic(
                &dir.join("current"),
                &Uuid::new_v4().to_string(),
                true,
            )
            .unwrap(),
            "temp" => fs::write(dir.join(".current.interrupted.tmp"), b"partial").unwrap(),
            _ => unreachable!(),
        }
        downstream(
            &lease,
            match case {
                "orphan" => "E_DAEMON_RESERVATION_INCOMPLETE",
                "dangling" => "E_DAEMON_PUBLICATION_INCOMPLETE",
                _ => "E_DAEMON_RECORD",
            },
        );
    }
}

#[test]
fn identity_conversion_is_checked_exact_and_mismatch_never_publishes_record() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let native = lease.identity().unwrap();
    assert_eq!(Identity::from_native(native).unwrap().native(), *native);
    for birth in [
        "1",
        "1.1",
        "01.000001",
        "0.000001",
        "18446744073709551615.999999",
        "1.00000x",
    ] {
        error(
            Identity::from_native(&ProcessIdentity {
                pid: 42,
                start_time: birth.into(),
            }),
            "E_DAEMON_IDENTITY",
        );
    }
    let r = reserve(&registry);
    let before = snapshot(&registry.root);
    let wrong = ProcessIdentity {
        pid: native.pid,
        start_time: "1.000001".into(),
    };
    error(
        registry.record(&r, &wrong, 1001),
        "E_DAEMON_IDENTITY_UNVERIFIED",
    );
    assert_eq!(snapshot(&registry.root), before);
}

#[test]
fn identity_observations_are_not_adoption_or_respawn_permissions() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    published(&registry);
    for state in [
        ProcessState::Same,
        ProcessState::Gone,
        ProcessState::Recycled,
        ProcessState::Unreadable,
    ] {
        assert_eq!(registry.inspect_with(|_| state).unwrap()[0].identity, state);
    }
    downstream(&lease, "E_DAEMON_SUPERVISION_PENDING");
}

#[test]
fn provenance_unknown_is_explicit_and_never_fabricates_release_sha() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let r = published(&registry);
    let text = serde_json::to_string(&r).unwrap();
    assert!(text.contains("legacy_unknown"));
    assert!(!text.contains("release_sha"));
    for sha in [
        "",
        "HEAD",
        "abeb9ae",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        error(
            Provenance::Release {
                release_sha: sha.into(),
            }
            .validate(),
            "E_DAEMON_RECORD",
        );
    }
    // Shape validation alone is not runtime-release trust/adoption (F2-E).
    Provenance::Release {
        release_sha: "a".repeat(40),
    }
    .validate()
    .unwrap();
}

#[test]
fn replaced_lock_path_revokes_metadata_mutation_and_second_controller_is_denied() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    assert!(ControllerLock::acquire(&f.0).is_err());
    let root = lease.run_dir().unwrap().to_owned();
    fs::rename(root.join("daemons.lock"), root.join("original.lock")).unwrap();
    fs::write(root.join("daemons.lock"), b"replacement").unwrap();
    fs::set_permissions(root.join("daemons.lock"), fs::Permissions::from_mode(0o600)).unwrap();
    error(
        registry.reserve(
            Endpoint::Hub { port: 4517 },
            Provenance::LegacyUnknown,
            1000,
        ),
        "E_CONTROLLER_LEASE",
    );
    downstream(&lease, "E_CONTROLLER_LEASE");
    assert!(fs::read_dir(&registry.root).unwrap().next().is_none());
}

#[test]
fn selectors_and_record_binding_are_strict() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    for seat in ["../hub", "", "CAPS", "x/y", "seat_"] {
        error(
            registry.reserve(
                Endpoint::CodexAppServer { seat: seat.into() },
                Provenance::LegacyUnknown,
                1000,
            ),
            "E_DAEMON_RECORD",
        );
    }
    let original = reserve(&registry);
    let mut forged = original.clone();
    forged.spawned_by.pid += 1;
    error(
        registry.record(&forged, lease.identity().unwrap(), 1001),
        "E_DAEMON_RECORD",
    );
    assert_eq!(
        registry
            .history("hub")
            .unwrap()
            .reservations
            .get(&original.incarnation),
        Some(&original)
    );
}
