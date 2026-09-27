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

#[test]
fn b2_capacity_accounts_for_new_pair_and_current_without_pruning_history() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let first = published(&registry);
    let dir = registry.slot_path("hub", false).unwrap();
    let mut previous = first.reservation.incarnation.clone();
    // 255 complete incarnations => 511 facts including current. Adding the
    // next reservation+record would exceed512, even though a reservation alone
    // would fit. Build valid history directly, without any spawn authority.
    for _ in 1..255 {
        let mut r = first.clone();
        r.reservation.incarnation = Uuid::new_v4().to_string();
        r.reservation.previous = Some(previous);
        journal::write_private_json_atomic(
            &dir.join(format!("reservation-{}.json", r.reservation.incarnation)),
            &r.reservation,
            false,
        )
        .unwrap();
        journal::write_private_json_atomic(
            &dir.join(format!("{}.json", r.reservation.incarnation)),
            &r,
            false,
        )
        .unwrap();
        previous = r.reservation.incarnation;
    }
    journal::write_private_json_atomic(&dir.join("current"), &previous, true).unwrap();
    assert!(registry.inspect().is_ok());
    error(registry.capacity("hub"), "E_DAEMON_CAPACITY");
    error(
        registry.reserve(
            Endpoint::Hub { port: 4517 },
            Provenance::LegacyUnknown,
            1002,
        ),
        "E_DAEMON_CAPACITY",
    );
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 511);
    assert_eq!(
        registry
            .current("hub")
            .unwrap()
            .unwrap()
            .reservation
            .incarnation,
        previous
    );
}

#[test]
fn b2_slot_capacity_denies_new_slot_without_erasing_existing_history() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    for n in 0..MAX_SLOTS {
        let reservation = registry
            .reserve(
                Endpoint::CodexAppServer {
                    seat: format!("fixture-{n}"),
                },
                Provenance::LegacyUnknown,
                1,
            )
            .unwrap();
        let r = registry
            .record(&reservation, lease.identity().unwrap(), 2)
            .unwrap();
        registry.publish_current(&r).unwrap();
    }
    error(registry.capacity("hub"), "E_DAEMON_CAPACITY");
    error(
        registry.reserve(Endpoint::Hub { port: 4517 }, Provenance::LegacyUnknown, 1),
        "E_DAEMON_CAPACITY",
    );
    assert_eq!(registry.inspect().unwrap().len(), MAX_SLOTS);
    assert!(registry.current("hub").unwrap().is_none());
}

// C2a claims below are synthetic metadata, NOT native socket/stop witnesses.
fn v2_pins(link: bool) -> SocketPinsV2 {
    let dir = NodePinV2 {
        dev: 1,
        ino: 2,
        uid: unsafe { libc::geteuid() },
        mode: libc::S_IFDIR as u32 | 0o700,
        links: 0,
    };
    let socket = NodePinV2 {
        dev: 1,
        ino: 3,
        uid: dir.uid,
        mode: libc::S_IFSOCK as u32 | 0o600,
        links: 1,
    };
    SocketPinsV2 {
        format_version: 1,
        parents: vec![dir.clone()],
        entry: if link {
            NodePinV2 {
                mode: libc::S_IFLNK as u32 | 0o777,
                ..socket.clone()
            }
        } else {
            socket.clone()
        },
        native_target: if link {
            Some(NativeTargetV2 {
                basename: "a".repeat(64),
                parents: vec![dir],
                leaf: socket,
            })
        } else {
            None
        },
    }
}
fn v2_ready(
    registry: &Registry<'_>,
    operation: &crate::controller::CodexOperation<'_>,
    link: bool,
) -> String {
    let id = registry
        .begin_codex_v2(operation, Provenance::LegacyUnknown, 10)
        .unwrap();
    registry
        .append_codex_v2(
            operation,
            &id,
            CodexEventV2::Spawned {
                process: Identity::from_native(registry.lease.identity().unwrap()).unwrap(),
            },
            11,
        )
        .unwrap();
    registry
        .append_codex_v2(
            operation,
            &id,
            CodexEventV2::SocketReady {
                pins: v2_pins(link),
            },
            12,
        )
        .unwrap();
    assert_eq!(
        registry
            .codex_metadata(operation.seat())
            .unwrap()
            .unwrap()
            .phase,
        CodexPhaseV2::PublicationIncomplete
    );
    registry.publish_codex_v2(operation, &id).unwrap();
    id
}
#[test]
fn c2_v2_explicit_roundtrip_keeps_v1_hub_bytes_and_denies_enrollment() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let hub = published(&registry);
    let hub_before = snapshot(&registry.root.join("hub"));
    let slot = lease.codex_slot("fixture").unwrap();
    let operation = slot.enter().unwrap();
    let id = v2_ready(&registry, &operation, false);
    assert_eq!(
        registry.codex_metadata("fixture").unwrap().unwrap(),
        CodexMetadataV2 {
            incarnation: id,
            process: Some(lease.identity().unwrap().clone()),
            phase: CodexPhaseV2::ReadyMetadataOnly,
            entries: 4
        }
    );
    assert_eq!(snapshot(&registry.root.join("hub")), hub_before);
    assert_eq!(registry.current("hub").unwrap().unwrap(), hub);
    assert_eq!(registry.inspect().unwrap().len(), 2);
    // v1 observation API does not manufacture a v1 record from new v2 facts.
    assert!(registry.current("codex-fixture").is_err());
    let before = snapshot(&registry.root);
    assert!(registry
        .begin_codex_v2(&operation, Provenance::LegacyUnknown, 20)
        .is_err());
    assert_eq!(snapshot(&registry.root), before);
    // Old strict v1 structs cannot silently consume the new wire format.
    let h = registry.history("codex-fixture").unwrap();
    assert!(
        serde_json::from_value::<Reservation>(serde_json::to_value(&h.codex[&0]).unwrap()).is_err()
    );
    assert!(serde_json::from_value::<Record>(serde_json::to_value(&h.codex[&1]).unwrap()).is_err());
}
#[test]
fn c2_v1_codex_stays_observable_but_never_becomes_v2_or_successor_authority() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let endpoint = Endpoint::CodexAppServer {
        seat: "fixture".into(),
    };
    let intent = registry
        .reserve(endpoint.clone(), Provenance::LegacyUnknown, 1)
        .unwrap();
    let r = registry
        .record(&intent, lease.identity().unwrap(), 2)
        .unwrap();
    registry.publish_current(&r).unwrap();
    let before = snapshot(&registry.root);
    let slot = lease.codex_slot("fixture").unwrap();
    let operation = slot.enter().unwrap();
    assert!(registry.codex_metadata("fixture").is_err());
    assert!(registry
        .begin_codex_v2(&operation, Provenance::LegacyUnknown, 20)
        .is_err());
    assert!(registry
        .reserve(endpoint, Provenance::LegacyUnknown, 21)
        .is_err());
    assert_eq!(registry.current("codex-fixture").unwrap().unwrap(), r);
    assert_eq!(snapshot(&registry.root), before);
}
#[test]
fn c2_dropped_operation_cannot_resume_unknown_with_new_nonce_or_controller() {
    for stage in 0..3 {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let operation = slot.enter().unwrap();
        let id = registry
            .begin_codex_v2(&operation, Provenance::LegacyUnknown, 10)
            .unwrap();
        if stage > 0 {
            registry
                .append_codex_v2(
                    &operation,
                    &id,
                    CodexEventV2::Spawned {
                        process: Identity::from_native(lease.identity().unwrap()).unwrap(),
                    },
                    11,
                )
                .unwrap();
        }
        if stage > 1 {
            registry
                .append_codex_v2(
                    &operation,
                    &id,
                    CodexEventV2::SocketReady {
                        pins: v2_pins(false),
                    },
                    12,
                )
                .unwrap();
        }
        drop(operation);
        let new = slot.enter().unwrap();
        let before = snapshot(&registry.root);
        assert!(registry
            .begin_codex_v2(&new, Provenance::LegacyUnknown, 20)
            .is_err());
        let event = if stage == 0 {
            CodexEventV2::Spawned {
                process: Identity::from_native(lease.identity().unwrap()).unwrap(),
            }
        } else {
            CodexEventV2::SocketReady {
                pins: v2_pins(false),
            }
        };
        assert!(registry.append_codex_v2(&new, &id, event, 21).is_err());
        assert!(registry.publish_codex_v2(&new, &id).is_err());
        assert_eq!(snapshot(&registry.root), before);
        drop(new);
        drop(slot);
        drop(registry);
        drop(lease);
        let restarted = f.lease();
        let registry = Registry::open(&restarted).unwrap();
        let slot = restarted.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        assert!(registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 22)
            .is_err());
        assert!(registry.publish_codex_v2(&op, &id).is_err());
        assert_eq!(snapshot(&registry.root), before);
    }
}
#[test]
fn c2_stop_syscall_and_outcome_are_separate_unknown_never_reconciles() {
    for result in [
        TermResultV2::ReturnedZero,
        TermResultV2::Esrch,
        TermResultV2::OtherError,
        TermResultV2::Unobserved,
    ] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        let id = v2_ready(&registry, &op, false);
        registry
            .append_codex_v2(&op, &id, CodexEventV2::StopIntent {}, 13)
            .unwrap();
        registry
            .append_codex_v2(
                &op,
                &id,
                CodexEventV2::TermResult {
                    result: result.clone(),
                },
                14,
            )
            .unwrap();
        if result != TermResultV2::ReturnedZero {
            assert!(registry
                .append_codex_v2(
                    &op,
                    &id,
                    CodexEventV2::StopOutcome {
                        outcome: StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified
                    },
                    15
                )
                .is_err());
        }
        registry
            .append_codex_v2(
                &op,
                &id,
                CodexEventV2::StopOutcome {
                    outcome: StopOutcomeV2::Unknown,
                },
                15,
            )
            .unwrap();
        let before = snapshot(&registry.root);
        assert_eq!(
            registry.codex_metadata("fixture").unwrap().unwrap().phase,
            CodexPhaseV2::Unknown
        );
        for event in [
            CodexEventV2::CleanupIntent {},
            CodexEventV2::StopIntent {},
            CodexEventV2::StopOutcome {
                outcome: StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified,
            },
        ] {
            assert!(registry.append_codex_v2(&op, &id, event, 16).is_err());
        }
        // Endpoint is absent and native process could later disappear: neither
        // is consulted to erase Unknown or authorize another incarnation.
        assert!(!lease.run_dir().unwrap().join("fixture.sock").exists());
        assert!(registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 17)
            .is_err());
        assert_eq!(snapshot(&registry.root), before);
    }
}
#[test]
fn c2_represented_cleanup_is_narrow_and_never_unlocks_successor() {
    for link in [false, true] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        let id = v2_ready(&registry, &op, link);
        for (time, event) in [
            (13, CodexEventV2::StopIntent {}),
            (
                14,
                CodexEventV2::TermResult {
                    result: TermResultV2::ReturnedZero,
                },
            ),
            (
                15,
                CodexEventV2::StopOutcome {
                    outcome: StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified,
                },
            ),
            (16, CodexEventV2::CleanupIntent {}),
        ] {
            registry.append_codex_v2(&op, &id, event, time).unwrap();
        }
        let wrong = if link {
            CleanupOutcomeV2::FixedDirectEntryRemoved
        } else {
            CleanupOutcomeV2::FixedLinkRemovedTargetRetained
        };
        assert!(registry
            .append_codex_v2(
                &op,
                &id,
                CodexEventV2::CleanupOutcome { outcome: wrong },
                17
            )
            .is_err());
        let outcome = if link {
            CleanupOutcomeV2::FixedLinkRemovedTargetRetained
        } else {
            CleanupOutcomeV2::FixedDirectEntryRemoved
        };
        registry
            .append_codex_v2(&op, &id, CodexEventV2::CleanupOutcome { outcome }, 17)
            .unwrap();
        assert_eq!(
            registry.codex_metadata("fixture").unwrap().unwrap().entries,
            CODEX_PLAN_ENTRIES
        );
        let before = snapshot(&registry.root);
        assert!(registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 18)
            .is_err());
        assert!(registry
            .append_codex_v2(
                &op,
                &id,
                CodexEventV2::Spawned {
                    process: Identity::from_native(lease.identity().unwrap()).unwrap()
                },
                19
            )
            .is_err());
        assert_eq!(snapshot(&registry.root), before);
    }
}
#[test]
fn c2_crashed_stop_and_cleanup_cannot_continue_from_new_operation() {
    for last in 3..8 {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        let id = v2_ready(&registry, &op, false);
        let events = [
            CodexEventV2::StopIntent {},
            CodexEventV2::TermResult {
                result: TermResultV2::ReturnedZero,
            },
            CodexEventV2::StopOutcome {
                outcome: StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified,
            },
            CodexEventV2::CleanupIntent {},
            CodexEventV2::CleanupOutcome {
                outcome: CleanupOutcomeV2::Unknown,
            },
        ];
        for (i, event) in events.iter().enumerate().take(last - 2) {
            registry
                .append_codex_v2(&op, &id, event.clone(), 13 + i as u64)
                .unwrap();
        }
        drop(op);
        let fresh = slot.enter().unwrap();
        let before = snapshot(&registry.root);
        assert!(registry
            .begin_codex_v2(&fresh, Provenance::LegacyUnknown, 30)
            .is_err());
        // Completed narrow stop may start a separate cleanup intent, but never
        // resume the interrupted syscall/cleanup or proceed after Unknown.
        if last != 5 {
            let next = events
                .get(last - 2)
                .cloned()
                .unwrap_or(CodexEventV2::StopIntent {});
            assert!(registry.append_codex_v2(&fresh, &id, next, 31).is_err());
        }
        assert_eq!(snapshot(&registry.root), before);
    }
}
#[test]
fn c2_unknown_versions_mixed_orphan_holes_and_unsafe_facts_deny_without_repair() {
    for case in [
        "version",
        "mixed",
        "hole",
        "orphan",
        "duplicate",
        "unsafe",
        "new_incarnation",
    ] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        let id = v2_ready(&registry, &op, false);
        let dir = registry.root.join("codex-fixture");
        let h = registry.history("codex-fixture").unwrap();
        let mut fact = h.codex[&1].clone();
        match case {
            "version" => {
                fact.schema_version = 3;
                journal::write_private_json_atomic(&dir.join(fact.filename()), &fact, true)
                    .unwrap();
            }
            "mixed" => {
                let hub_intent = Reservation {
                    schema_version: 1,
                    incarnation: id.clone(),
                    previous: None,
                    endpoint: Endpoint::CodexAppServer {
                        seat: "fixture".into(),
                    },
                    spawned_by: Identity::from_native(lease.identity().unwrap()).unwrap(),
                    provenance: Provenance::LegacyUnknown,
                    reserved_at_ms: 1,
                };
                write(&dir.join(format!("reservation-{id}.json")), &hub_intent);
            }
            "hole" => fs::remove_file(dir.join(fact.filename())).unwrap(),
            "orphan" => fs::write(dir.join(".atomic.tmp"), b"pending").unwrap(),
            "duplicate" => fs::copy(dir.join(fact.filename()), dir.join("v2-duplicate.json"))
                .map(|_| ())
                .unwrap(),
            "unsafe" => {
                fs::set_permissions(dir.join(fact.filename()), fs::Permissions::from_mode(0o644))
                    .unwrap()
            }
            "new_incarnation" => {
                fs::remove_file(dir.join(fact.filename())).unwrap();
                fact.incarnation = Uuid::new_v4().to_string();
                write(&dir.join(fact.filename()), &fact);
            }
            _ => unreachable!(),
        }
        let before = snapshot(&registry.root);
        assert!(registry.codex_metadata("fixture").is_err());
        let other = lease.codex_slot("other").unwrap();
        let other_op = other.enter().unwrap();
        assert!(registry
            .begin_codex_v2(&other_op, Provenance::LegacyUnknown, 30)
            .is_err());
        assert!(registry
            .reserve(Endpoint::Hub { port: 4517 }, Provenance::LegacyUnknown, 30)
            .is_err());
        assert!(!registry.root.join("codex-other").exists());
        assert!(!registry.root.join("hub").exists());
        assert!(registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 30)
            .is_err());
        assert_eq!(snapshot(&registry.root), before);
    }
}
#[test]
fn c2_budget_counts_current_and_no_schema_fields_hide_extra_authority() {
    assert_eq!(CODEX_PLAN_ENTRIES, 8 + 1);
    assert!(budget(503, CODEX_PLAN_ENTRIES).is_ok());
    assert!(budget(504, CODEX_PLAN_ENTRIES).is_err());
    assert!(budget(512, 1).is_err());
    assert!(budget(usize::MAX, 1).is_err());
    let mut pins = v2_pins(false);
    pins.parents.clear();
    assert!(pins.validate().is_err());
    let mut pins = v2_pins(true);
    pins.native_target.as_mut().unwrap().basename = "../arbitrary".into();
    assert!(pins.validate().is_err());
    let mut event = serde_json::to_value(CodexEventV2::CleanupIntent {}).unwrap();
    event["replacement"] = true.into();
    assert!(serde_json::from_value::<CodexEventV2>(event).is_err());
    assert!(serde_json::from_str::<StopOutcomeV2>("\"stopped\"").is_err());
    assert!(serde_json::from_str::<CodexEventV2>("{\"kind\":\"restart\"}").is_err());
}
#[test]
fn c2_operation_cannot_mutate_another_registry_and_read_projection_stays_narrow() {
    let a = Fixture::new();
    let b = Fixture::new();
    let lease_a = a.lease();
    let lease_b = b.lease();
    let registry = Registry::open(&lease_b).unwrap();
    let slot = lease_a.codex_slot("fixture").unwrap();
    let op = slot.enter().unwrap();
    let before = snapshot(&registry.root);
    assert!(registry
        .begin_codex_v2(&op, Provenance::LegacyUnknown, 10)
        .is_err());
    let path: &Path = registry.verified_run_dir().unwrap();
    assert_eq!(path, lease_b.run_dir().unwrap());
    registry.verify_read_context().unwrap();
    assert_eq!(snapshot(&registry.root), before);
}

// Both contenders call real metadata writers. Only their scheduling is injected.
#[test]
fn c2_admission_serializes_persisted_cap_v2_v2_and_hub_both_winners() {
    use std::sync::{atomic::AtomicBool, mpsc, Mutex};
    use std::time::Duration;
    for (winner_hub, loser_hub) in [(false, false), (true, false), (false, true)] {
        let f = Fixture::new();
        {
            let lease = f.lease();
            let registry = Registry::open(&lease).unwrap();
            for n in 0..127 {
                let r = registry
                    .reserve(
                        Endpoint::CodexAppServer {
                            seat: format!("persisted-{n}"),
                        },
                        Provenance::LegacyUnknown,
                        1,
                    )
                    .unwrap();
                let r = registry.record(&r, lease.identity().unwrap(), 2).unwrap();
                registry.publish_current(&r).unwrap();
            }
        }
        // Fresh lease has an empty in-memory slot map but 127 persisted slots.
        let mut lease = f.lease();
        let (counted_tx, counted_rx) = mpsc::channel();
        let (contended_tx, contended_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        lease.set_admission_probe(crate::controller::AdmissionProbe {
            counted: counted_tx,
            contended: contended_tx,
            resume: Mutex::new(resume_rx),
            pause_once: AtomicBool::new(true),
        });
        let registry = Registry::open(&lease).unwrap();
        let before = snapshot(&registry.root);
        let attempt = |hub: bool, seat: &str| -> Result<String> {
            let registry = Registry::open(&lease)?;
            if hub {
                let _transition = lease.hub_transition()?;
                registry
                    .reserve(Endpoint::Hub { port: 4517 }, Provenance::LegacyUnknown, 3)
                    .map(|r| r.incarnation)
            } else {
                let slot = lease.codex_slot(seat)?;
                let op = slot.enter()?;
                registry.begin_codex_v2(&op, Provenance::LegacyUnknown, 3)
            }
        };
        std::thread::scope(|scope| {
            let winner = scope.spawn(|| attempt(winner_hub, "winner"));
            counted_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("winner at count-before-mkdir");
            assert_eq!(snapshot(&registry.root), before);
            let loser = scope.spawn(|| attempt(loser_hub, "loser"));
            contended_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("actual admission mutex contention");
            // No timing race: both are now known to be before their first effect.
            assert_eq!(snapshot(&registry.root), before);
            resume_tx.send(()).unwrap();
            assert!(winner.join().unwrap().is_ok());
            assert!(loser.join().unwrap().is_err());
        });
        assert_eq!(fs::read_dir(&registry.root).unwrap().count(), 128);
        let loser_slot = if loser_hub { "hub" } else { "codex-loser" };
        assert!(!registry.root.join(loser_slot).exists());
        let after = snapshot(&registry.root);
        for (path, bytes) in &before {
            assert_eq!(after.get(path), Some(bytes));
        }
        assert_eq!(after.len(), before.len() + 1, "only one durable intent");
    }
}

#[test]
fn c2_admission_poison_or_lost_lease_denies_both_writers_without_effects() {
    for poison in [true, false] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let op = slot.enter().unwrap();
        if poison {
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _admission = lease.registry_admission().unwrap();
                panic!("deliberate admission poison");
            }))
            .is_err());
        } else {
            fs::remove_file(f.0.join(".aperture/run/daemons.lock")).unwrap();
        }
        let before = snapshot(&registry.root);
        assert!(registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 1)
            .is_err());
        assert!(registry
            .reserve(Endpoint::Hub { port: 4517 }, Provenance::LegacyUnknown, 1)
            .is_err());
        assert_eq!(snapshot(&registry.root), before);
        assert_eq!(fs::read_dir(&registry.root).unwrap().count(), 0);
    }
}

#[test]
fn c2_each_nonready_or_terminal_phase_retains_slot_without_global_outage() {
    use CodexEventV2 as E;
    use CodexPhaseV2 as P;
    // Every non-ready/terminal projection, with distinct Unknown origins.
    for (step, expected) in [
        (0, P::SpawnIntentUnknown),
        (1, P::SpawnedUnready),
        (2, P::PublicationIncomplete),
        (4, P::StopIntentUnknown),
        (5, P::TermResultUnresolved),
        (6, P::Unknown),
        (7, P::Unknown),
        (8, P::TermSentThenDaemonGoneDescendantsUnverified),
        (9, P::CleanupIntentUnknown),
        (10, P::Unknown),
        (11, P::FixedDirectEntryRemoved),
        (12, P::FixedLinkRemovedTargetRetained),
    ] {
        let f = Fixture::new();
        let lease = f.lease();
        let registry = Registry::open(&lease).unwrap();
        let slot = lease.codex_slot("original").unwrap();
        let op = slot.enter().unwrap();
        let id = registry
            .begin_codex_v2(&op, Provenance::LegacyUnknown, 1)
            .unwrap();
        let append = |event| registry.append_codex_v2(&op, &id, event, 2).unwrap();
        if step >= 1 {
            append(E::Spawned {
                process: Identity::from_native(lease.identity().unwrap()).unwrap(),
            });
        }
        if step >= 2 {
            append(E::SocketReady {
                pins: v2_pins(step == 12),
            });
        }
        if step >= 3 {
            registry.publish_codex_v2(&op, &id).unwrap();
        }
        if step >= 4 {
            append(E::StopIntent {});
        }
        if step >= 5 {
            append(E::TermResult {
                result: if step == 6 {
                    TermResultV2::Esrch
                } else {
                    TermResultV2::ReturnedZero
                },
            });
        }
        if step >= 7 {
            append(E::StopOutcome {
                outcome: if step == 7 {
                    StopOutcomeV2::Unknown
                } else {
                    StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified
                },
            });
        }
        if step >= 9 {
            append(E::CleanupIntent {});
        }
        if step >= 10 {
            append(E::CleanupOutcome {
                outcome: match step {
                    10 => CleanupOutcomeV2::Unknown,
                    11 => CleanupOutcomeV2::FixedDirectEntryRemoved,
                    12 => CleanupOutcomeV2::FixedLinkRemovedTargetRetained,
                    _ => unreachable!(),
                },
            });
        }
        assert_eq!(
            registry.codex_metadata("original").unwrap().unwrap().phase,
            expected
        );
        let original = snapshot(&registry.root);
        assert!(
            registry.inspect().is_err(),
            "adoption projection not relaxed"
        );
        error(
            registry.begin_codex_v2(&op, Provenance::LegacyUnknown, 3),
            "E_CODEX_NOT_PRISTINE",
        );
        assert!(registry
            .reserve(
                Endpoint::CodexAppServer {
                    seat: "original".into()
                },
                Provenance::LegacyUnknown,
                3
            )
            .is_err());
        assert_eq!(snapshot(&registry.root), original);
        let new_slot = lease.codex_slot("different").unwrap();
        let new_op = new_slot.enter().unwrap();
        registry
            .begin_codex_v2(&new_op, Provenance::LegacyUnknown, 3)
            .unwrap();
        // The first unrelated v2 is itself now incomplete; hub admission still works.
        registry
            .reserve(Endpoint::Hub { port: 4517 }, Provenance::LegacyUnknown, 3)
            .unwrap();
        assert_eq!(fs::read_dir(&registry.root).unwrap().count(), 3);
        let after = snapshot(&registry.root);
        for (path, bytes) in &original {
            assert_eq!(after.get(path), Some(bytes));
        }
        assert_eq!(after.len(), original.len() + 2);
        assert!(registry.inspect().is_err());
    }
}

#[test]
fn c2b_snapshot_revision_covers_hidden_fact_values_and_current_without_authority() {
    let f = Fixture::new();
    let lease = f.lease();
    let registry = Registry::open(&lease).unwrap();
    let slot = lease.codex_slot("fixture").unwrap();
    let op = slot.enter().unwrap();
    let id = v2_ready(&registry, &op, false);
    let before = registry.codex_snapshot("fixture").unwrap().unwrap();
    let dir = registry.root.join("codex-fixture");
    let original = registry.history("codex-fixture").unwrap();
    // Equal phase, identity and visible pins; change a hidden timestamp or nonce.
    for field in ["time", "nonce"] {
        for fact in original.codex.values() {
            let mut changed = fact.clone();
            if field == "time" {
                changed.at_ms += 100;
            } else {
                changed.operation = Uuid::new_v4().to_string();
            }
            // A shared nonce is needed for the valid same-operation prefix.
            if field == "nonce" {
                changed.operation = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into();
            }
            journal::write_private_json_atomic(&dir.join(changed.filename()), &changed, true)
                .unwrap();
        }
        let after = registry.codex_snapshot("fixture").unwrap().unwrap();
        assert_eq!(before.phase, after.phase);
        assert_eq!(before.identity, after.identity);
        assert_eq!(before.pins, after.pins);
        assert_ne!(before, after);
        assert!(registry.recheck_snapshot("fixture", &before).is_err());
        for fact in original.codex.values() {
            journal::write_private_json_atomic(&dir.join(fact.filename()), fact, true).unwrap();
        }
    }
    fs::remove_file(dir.join("current")).unwrap();
    assert_ne!(registry.codex_snapshot("fixture").unwrap().unwrap(), before);
    write(&dir.join("current"), &id);
    registry.recheck_snapshot("fixture", &before).unwrap();
    assert_eq!(registry.validate_namespace().unwrap(), 1);
    assert_eq!(
        registry.verified_run_dir().unwrap(),
        f.0.join(".aperture/run")
    );
}
