use super::*;
use serde_json::{json, Value};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::time::{Duration, Instant};
const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
struct Fixture {
    home: PathBuf,
}
fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
fn role_name(role: Role) -> &'static str {
    match role {
        Role::ApertureServer => "aperture_server",
        Role::ApertureBoot => "aperture_boot",
        Role::ApertureTeamControl => "aperture_team_control",
        Role::McpServerEntry => "mcp_server_entry",
        Role::HubServerEntry => "hub_server_entry",
        Role::HubClientEntry => "hub_client_entry",
        Role::SentryServerEntry => "sentry_server_entry",
    }
}
fn manifest(sha: &str) -> Value {
    let mut files:Vec<_>=ROLES.into_iter().map(|r|{
        let (path,kind)=r.entry();let bytes=format!("inert bytes {path}");
        json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes.as_bytes())),"size":bytes.len(),
            "kind":if kind==Kind::Executable{"executable"}else{"data"},"role":role_name(r)})
    }).collect();
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    json!({"schema_version":1,"release_sha":sha,"api_schema":1,"files":files})
}
impl Fixture {
    fn new() -> Self {
        let home = PathBuf::from(format!(
            "/private/tmp/aperture-release-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .unwrap();
        for name in [".aperture", ".aperture/releases", ".aperture/runtime"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(home.join(name))
                .unwrap();
        }
        let f = Self { home };
        f.add(A);
        f
    }
    fn root(&self, sha: &str) -> PathBuf {
        self.home.join(".aperture/releases").join(sha)
    }
    fn add(&self, sha: &str) {
        let root = self.root(sha);
        std::fs::create_dir(&root).unwrap();
        let m = manifest(sha);
        for e in m["files"].as_array().unwrap() {
            let p = root.join(e["path"].as_str().unwrap());
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("inert bytes {}", e["path"].as_str().unwrap())).unwrap();
        }
        std::fs::write(root.join("RELEASE.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        self.seal(sha);
    }
    // Owned setup only: macOS rejected rename of sealed directories (errno13).
    // Restore the complete original mode before taking any oracle snapshot.
    fn rename_owned_dir(&self, from: &Path, to: &Path) {
        let retained_home = self.home.with_file_name(format!(
            "{}-retained",
            self.home.file_name().unwrap().to_str().unwrap()
        ));
        for path in [from, to] {
            assert!(path.starts_with(&self.home) || path == retained_home);
        }
        let metadata = std::fs::symlink_metadata(from).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        let mode = metadata.mode() & 0o7777;
        chmod(from, mode | 0o200);
        let result = std::fs::rename(from, to);
        let actual_path = if result.is_ok() { to } else { from };
        let actual = std::fs::symlink_metadata(actual_path).unwrap();
        assert!(actual.is_dir() && !actual.file_type().is_symlink());
        assert_eq!(
            (actual.dev(), actual.ino(), actual.uid()),
            (metadata.dev(), metadata.ino(), metadata.uid())
        );
        chmod(actual_path, mode);
        assert_eq!(
            std::fs::symlink_metadata(actual_path).unwrap().mode() & 0o7777,
            mode
        );
        result.unwrap();
    }
    fn seal(&self, sha: &str) {
        fn seal(path: &Path) {
            for entry in std::fs::read_dir(path).unwrap() {
                let p = entry.unwrap().path();
                let m = std::fs::symlink_metadata(&p).unwrap();
                if m.is_dir() {
                    seal(&p);
                } else if m.is_file() {
                    chmod(
                        &p,
                        if p.parent().unwrap().file_name().unwrap() == "bin" {
                            0o500
                        } else {
                            0o400
                        },
                    );
                }
            }
            chmod(path, 0o500);
        }
        seal(&self.root(sha));
    }
    fn edit_manifest(&self, m: &Value) {
        self.raw_manifest(&serde_json::to_vec(m).unwrap());
    }
    fn raw_manifest(&self, bytes: &[u8]) {
        let p = self.root(A).join("RELEASE.json");
        chmod(&p, 0o600);
        std::fs::write(&p, bytes).unwrap();
        chmod(&p, 0o400);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable(path: &Path) {
            if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
                chmod(path, 0o700);
                for e in std::fs::read_dir(path).unwrap() {
                    writable(&e.unwrap().path());
                }
            }
        }
        writable(&self.home);
        std::fs::remove_dir_all(&self.home).unwrap();
        assert!(!self.home.exists());
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Snapshot(Vec<(PathBuf, Pin, Vec<u8>)>);
fn snapshot(root: &Path) -> Snapshot {
    fn walk(root: &Path, p: &Path, out: &mut Vec<(PathBuf, Pin, Vec<u8>)>) {
        let m = std::fs::symlink_metadata(p).unwrap();
        // Never open a special file. Large sparse overflow setup is represented
        // by its complete metadata, not loaded into RAM by the snapshot helper.
        let bytes = if m.file_type().is_symlink() {
            std::fs::read_link(p)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if m.is_file() && m.len() <= MANIFEST_CAP {
            std::fs::read(p).unwrap()
        } else {
            vec![]
        };
        out.push((p.strip_prefix(root).unwrap().into(), Pin::of(&m), bytes));
        if m.is_dir() {
            for e in std::fs::read_dir(p).unwrap() {
                walk(root, &e.unwrap().path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Snapshot(out)
}
fn unchanged<T>(f: &Fixture, call: impl FnOnce() -> T) -> T {
    let before = snapshot(&f.home);
    let result = call();
    assert_eq!(snapshot(&f.home), before, "validator wrote fixture state");
    result
}
fn refused(f: &Fixture) {
    assert_eq!(
        unchanged(f, || RecordedRelease::for_record(&f.home, A)).err(),
        Some(UNAVAILABLE)
    );
}
#[test]
fn e1_closed_tree_two_bindings_and_current_switch_are_read_only() {
    let f = Fixture::new();
    f.add(B);
    let current = f.home.join(".aperture/runtime/current");
    std::os::unix::fs::symlink(f.root(A), &current).unwrap();
    for role in ROLES.into_iter().take(3) {
        let a = unchanged(&f, || {
            ProcessRelease::fixture(&f.home, &f.root(A).join(role.entry().0))
        })
        .unwrap();
        assert_eq!(a.release_sha().unwrap(), A);
        assert_eq!(a.api_schema().unwrap(), 1);
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(f.root(B), &current).unwrap();
        assert_eq!(
            unchanged(&f, || a.role(role)).unwrap(),
            f.root(A).join(role.entry().0)
        );
        unchanged(&f, || a.recheck()).unwrap();
        let b = unchanged(&f, || RecordedRelease::for_record(&f.home, B)).unwrap();
        assert_eq!(b.release_sha().unwrap(), B);
        assert_eq!(b.api_schema().unwrap(), 1);
        assert_eq!(
            unchanged(&f, || b.role(Role::HubClientEntry)).unwrap(),
            f.root(B).join("mcp-server/dist/hub-client.js")
        );
        // Even an unreadable/dangling current target is irrelevant to both bindings.
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink("missing", &current).unwrap();
        unchanged(&f, || a.recheck()).unwrap();
        unchanged(&f, || b.recheck()).unwrap();
    }
}
#[test]
fn e1_process_selection_requires_exact_layout_role_and_same_inode() {
    let f = Fixture::new();
    for path in [
        f.root(A).join("bin/unlisted"),
        f.root(A).join("mcp-server/dist/index.js"),
        f.home.join("checkout/target/aperture-server"),
        f.root(A).join("bin/../bin/aperture-server"),
        PathBuf::from(format!("{}/bin//aperture-server", f.root(A).display())),
    ] {
        assert_eq!(
            unchanged(&f, || ProcessRelease::fixture(&f.home, &path)).err(),
            Some(UNAVAILABLE)
        );
    }
    for sha in [
        "",
        "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "../a",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "cccccccccccccccccccccccccccccccccccccccc",
    ] {
        assert_eq!(
            unchanged(&f, || RecordedRelease::for_record(&f.home, sha)).err(),
            Some(UNAVAILABLE)
        );
    }
    let exe = f.root(A).join("bin/aperture-server");
    let bound = unchanged(&f, || ProcessRelease::fixture(&f.home, &exe)).unwrap();
    let old = std::fs::metadata(&exe).unwrap().ino();
    chmod(exe.parent().unwrap(), 0o700);
    std::fs::rename(&exe, exe.with_file_name("retained-old")).unwrap();
    std::fs::write(&exe, b"inert bytes bin/aperture-server").unwrap();
    chmod(&exe, 0o500);
    assert_ne!(std::fs::metadata(&exe).unwrap().ino(), old);
    // Remove old only as explicit fixture setup; new tree remains byte-consistent.
    std::fs::remove_file(exe.with_file_name("retained-old")).unwrap();
    chmod(exe.parent().unwrap(), 0o500);
    assert_eq!(unchanged(&f, || bound.recheck()), Err(UNAVAILABLE));
    assert_eq!(bound.release_sha(), Err(UNAVAILABLE));
    assert_eq!(
        unchanged(&f, || bound.role(Role::ApertureServer)),
        Err(UNAVAILABLE)
    );
}
#[test]
fn e1_schema_is_closed_and_duplicate_keys_are_never_erased() {
    let f = Fixture::new();
    let original = manifest(A);
    let bytes = serde_json::to_string(&original).unwrap();
    for key in ["schema_version", "release_sha", "api_schema", "files"] {
        let duplicate = format!(
            "{{{}:{},{}",
            serde_json::to_string(key).unwrap(),
            original[key],
            &bytes[1..]
        );
        f.raw_manifest(duplicate.as_bytes());
        refused(&f);
    }
    for key in ["path", "sha256", "size", "kind", "role"] {
        let entry = &original["files"][0];
        let encoded = serde_json::to_string(entry).unwrap();
        let duplicate = format!(
            "{{{}:{},{}",
            serde_json::to_string(key).unwrap(),
            entry[key],
            &encoded[1..]
        );
        f.raw_manifest(bytes.replacen(&encoded, &duplicate, 1).as_bytes());
        refused(&f);
    }
    for case in [
        "future",
        "api",
        "sha",
        "tools",
        "entry-extra",
        "missing",
        "kind",
        "role",
        "null",
        "negative",
        "hash",
        "role-duplicate",
        "kind-object",
        "role-object",
    ] {
        let mut m = original.clone();
        match case {
            "future" => m["schema_version"] = json!(2),
            "api" => m["api_schema"] = json!(2),
            "sha" => m["release_sha"] = json!(B),
            "tools" => m["tools"] = json!([]),
            "entry-extra" => m["files"][0]["env"] = json!({}),
            "missing" => {
                m["files"].as_array_mut().unwrap().remove(0);
            }
            "kind" => m["files"][0]["kind"] = json!("data"),
            "role" => m["files"][0]["role"] = json!("payload"),
            "null" => m["files"] = Value::Null,
            "negative" => m["files"][0]["size"] = json!(-1),
            "hash" => m["files"][0]["sha256"] = json!("F".repeat(64)),
            "kind-object" => m["files"][0]["kind"] = json!({"executable":null}),
            "role-object" => m["files"][0]["role"] = json!({"aperture_server":null}),
            _ => m["files"][1]["role"] = m["files"][0]["role"].clone(),
        }
        f.edit_manifest(&m);
        refused(&f);
    }
    f.raw_manifest(b"{invalid");
    refused(&f);
    f.raw_manifest(&[0xff]);
    refused(&f);
}
fn payload(path: &str, size: u64) -> Value {
    json!({"path":path,"sha256":"0".repeat(64),"size":size,"kind":"data","role":"payload"})
}
fn sort_files(m: &mut Value) {
    m["files"]
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
}
#[test]
fn e1_paths_aliases_prefixes_and_caps_fail_before_payload_reads() {
    let f = Fixture::new();
    for paths in [
        vec!["../escape"],
        vec!["/absolute"],
        vec!["bad//path"],
        vec!["a/./b"],
        vec!["a/../b"],
        vec!["bad space"],
        vec!["nonascii-é"],
        vec!["bad\0name"],
        vec!["RELEASE.json"],
        vec!["release.JSON/x"],
        vec!["a", "a/b"],
        vec!["A/x", "a/y"],
        vec!["dup", "DUP"],
        vec!["dup", "dup"],
        vec!["bin/APERTURE-server/child"],
    ] {
        let mut m = manifest(A);
        for p in paths {
            m["files"].as_array_mut().unwrap().push(payload(p, 0));
        }
        sort_files(&mut m);
        f.edit_manifest(&m);
        refused(&f);
    }
    for path in [
        "x".repeat(256),
        std::iter::repeat_n("x", 33).collect::<Vec<_>>().join("/"),
        std::iter::repeat_n("x".repeat(255), 5)
            .collect::<Vec<_>>()
            .join("/"),
    ] {
        let mut m = manifest(A);
        m["files"].as_array_mut().unwrap().push(payload(&path, 0));
        sort_files(&mut m);
        f.edit_manifest(&m);
        refused(&f);
    }
    let mut m = manifest(A);
    m["files"].as_array_mut().unwrap().reverse();
    f.edit_manifest(&m);
    refused(&f);
    for index in 0..7 {
        // root precision: cap applies to executables and all seven roles
        let mut m = manifest(A);
        m["files"][index]["size"] = json!(FILE_CAP + 1);
        f.edit_manifest(&m);
        refused(&f);
    }
    let mut m = manifest(A);
    for i in 0..5 {
        m["files"][i]["size"] = json!(FILE_CAP);
    }
    f.edit_manifest(&m);
    refused(&f);
    let mut m = manifest(A);
    for i in 0..FILES_CAP {
        m["files"]
            .as_array_mut()
            .unwrap()
            .push(payload(&format!("p{i:05}"), 0));
    }
    sort_files(&mut m);
    f.edit_manifest(&m);
    refused(&f);
    let mut m = manifest(A);
    for i in 0..5000 {
        m["files"]
            .as_array_mut()
            .unwrap()
            .push(payload(&format!("p{i:05}/leaf"), 0));
    }
    sort_files(&mut m);
    f.edit_manifest(&m);
    refused(&f); // implied descendant count >10000
    f.raw_manifest(&vec![b' '; MANIFEST_CAP as usize + 1]);
    refused(&f);
}
#[test]
fn e1_physical_tree_is_closed_and_regular_files_have_strict_modes() {
    for case in [
        "missing",
        "extra",
        "empty-dir",
        "hardlink",
        "symlink-leaf",
        "writable",
        "specialbits",
        "data-exec",
        "exec-no-owner-x",
        "manifest-write",
        "oversize",
    ] {
        let f = Fixture::new();
        let root = f.root(A);
        let data = root.join("mcp-server/dist/index.js");
        match case {
            "missing" => {
                chmod(data.parent().unwrap(), 0o700);
                std::fs::remove_file(&data).unwrap();
                chmod(data.parent().unwrap(), 0o500);
            }
            "extra" => {
                chmod(&root, 0o700);
                std::fs::write(root.join("unlisted"), b"extra").unwrap();
                chmod(&root.join("unlisted"), 0o400);
                chmod(&root, 0o500);
            }
            "empty-dir" => {
                chmod(&root, 0o700);
                std::fs::create_dir(root.join("extra-dir")).unwrap();
                chmod(&root.join("extra-dir"), 0o500);
                chmod(&root, 0o500);
            }
            "hardlink" => {
                chmod(&root, 0o700);
                std::fs::hard_link(&data, root.join("extra-link")).unwrap();
                chmod(&root, 0o500);
            }
            "symlink-leaf" => {
                chmod(data.parent().unwrap(), 0o700);
                std::fs::remove_file(&data).unwrap();
                std::os::unix::fs::symlink("ws-hub.js", &data).unwrap();
                chmod(data.parent().unwrap(), 0o500);
            }
            "writable" => chmod(&data, 0o600),
            "specialbits" => chmod(&data, 0o4400),
            "data-exec" => chmod(&data, 0o500),
            "exec-no-owner-x" => chmod(&root.join("bin/aperture-server"), 0o440),
            "manifest-write" => chmod(&root.join("RELEASE.json"), 0o600),
            "oversize" => {
                chmod(&data, 0o600);
                File::options()
                    .write(true)
                    .open(&data)
                    .unwrap()
                    .set_len(FILE_CAP + 1)
                    .unwrap();
                chmod(&data, 0o400);
            }
            _ => unreachable!(),
        }
        refused(&f);
    }
}
#[test]
fn e1_symlink_or_unsafe_ancestor_classes_are_never_followed_or_repaired() {
    for component in [
        "home",
        ".aperture",
        ".aperture/releases",
        "release",
        "bin",
        "mcp-server/dist",
    ] {
        let f = Fixture::new();
        let path = match component {
            "home" => f.home.clone(),
            "release" => f.root(A),
            "bin" | "mcp-server/dist" => f.root(A).join(component),
            _ => f.home.join(component),
        };
        let parent = path.parent().unwrap();
        let original_parent = std::fs::metadata(parent).unwrap().mode() & 0o7777;
        // Never chmod shared /private/tmp: only our own tree parents need write.
        if path != f.home {
            chmod(parent, 0o700);
        }
        let moved = path.with_file_name(format!(
            "{}-retained",
            path.file_name().unwrap().to_str().unwrap()
        ));
        f.rename_owned_dir(&path, &moved);
        std::os::unix::fs::symlink(&moved, &path).unwrap();
        if path != f.home {
            chmod(parent, original_parent);
        }
        // Home alias snapshot cannot traverse symlink: snapshot both own roots.
        let before = snapshot(&moved);
        let link = std::fs::read_link(&path).unwrap();
        assert_eq!(
            RecordedRelease::for_record(&f.home, A).err(),
            Some(UNAVAILABLE)
        );
        assert_eq!(snapshot(&moved), before);
        assert_eq!(std::fs::read_link(&path).unwrap(), link);
        if path != f.home {
            chmod(parent, 0o700);
        }
        std::fs::remove_file(&path).unwrap();
        f.rename_owned_dir(&moved, &path);
        if path != f.home {
            chmod(parent, original_parent);
        }
    }
    for path in [".aperture", ".aperture/releases"] {
        let f = Fixture::new();
        chmod(&f.home.join(path), 0o755);
        refused(&f);
    }
    for path in ["", "bin", "mcp-server/dist"] {
        let f = Fixture::new();
        chmod(&f.root(A).join(path), 0o700);
        refused(&f);
    }
    let f = Fixture::new();
    let missing = f.root(A).join("missing");
    assert_eq!(
        unchanged(&f, || RecordedRelease::for_record(&missing, A)).err(),
        Some(UNAVAILABLE)
    );
    assert!(!missing.exists());
}
#[test]
fn e1_drift_is_permanent_even_after_bytes_or_pointer_are_restored() {
    for case in ["bytes", "mode", "manifest", "directory", "root-name"] {
        let f = Fixture::new();
        f.add(B);
        let binding = unchanged(&f, || RecordedRelease::for_record(&f.home, A)).unwrap();
        let root = f.root(A);
        let data = root.join("mcp-server/dist/index.js");
        let original = std::fs::read(&data).unwrap();
        match case {
            "bytes" => {
                chmod(&data, 0o600);
                std::fs::write(&data, b"drift").unwrap();
                chmod(&data, 0o400);
            }
            "mode" => chmod(&data, 0o440),
            "manifest" => {
                let mut m = manifest(A);
                m["api_schema"] = json!(2);
                f.edit_manifest(&m);
            }
            "directory" => chmod(&root.join("bin"), 0o550),
            _ => {
                f.rename_owned_dir(&root, &root.with_file_name(format!("{A}-retained")));
                f.rename_owned_dir(&f.root(B), &root);
            }
        }
        assert_eq!(unchanged(&f, || binding.recheck()), Err(UNAVAILABLE));
        match case {
            "bytes" => {
                chmod(&data, 0o600);
                std::fs::write(&data, &original).unwrap();
                chmod(&data, 0o400);
            }
            "mode" => chmod(&data, 0o400),
            "manifest" => f.edit_manifest(&manifest(A)),
            "directory" => chmod(&root.join("bin"), 0o500),
            _ => {
                f.rename_owned_dir(&root, &f.root(B));
                f.rename_owned_dir(&root.with_file_name(format!("{A}-retained")), &root);
            }
        }
        assert_eq!(unchanged(&f, || binding.recheck()), Err(UNAVAILABLE));
        assert_eq!(binding.release_sha(), Err(UNAVAILABLE));
        assert_eq!(binding.api_schema(), Err(UNAVAILABLE));
        assert_eq!(
            unchanged(&f, || binding.role(Role::HubServerEntry)),
            Err(UNAVAILABLE)
        );
    }
}
#[test]
#[ignore = "owned bounded FIFO entry only; never operational executable"]
fn inert_fifo_entry() {
    use std::io::Read;
    let home = PathBuf::from(std::env::var_os("APERTURE_RELEASE_FIXTURE_HOME").unwrap());
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("aperture-release-"));
    let mut go = [0];
    std::io::stdin().read_exact(&mut go).unwrap();
    let start = Instant::now();
    assert_eq!(
        RecordedRelease::for_record(&home, A).err(),
        Some(UNAVAILABLE)
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}
#[test]
fn e1_no_writer_fifo_at_manifest_or_payload_refuses_with_finite_reap() {
    use std::io::Write;
    for relative in ["RELEASE.json", "mcp-server/dist/index.js"] {
        let f = Fixture::new();
        let leaf = f.root(A).join(relative);
        let parent = leaf.parent().unwrap();
        chmod(parent, 0o700);
        std::fs::remove_file(&leaf).unwrap();
        let name = CString::new(leaf.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o400) }, 0);
        chmod(parent, 0o500);
        unchanged(&f, || {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runtime_release::tests::inert_fifo_entry",
                    "--ignored",
                    "--nocapture",
                ])
                .env_clear()
                .env("APERTURE_RELEASE_FIXTURE_HOME", &f.home)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let identity = crate::team_process::observe(child.id())
                .unwrap()
                .unwrap()
                .identity;
            child.stdin.take().unwrap().write_all(b"g").unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            let exit = loop {
                if let Some(exit) = child.try_wait().unwrap() {
                    break exit;
                }
                if Instant::now() >= until {
                    if crate::team_process::state(&identity)
                        == crate::team_replacement::ProcessState::Same
                    {
                        child.kill().unwrap();
                    }
                    let _ = child.wait();
                    panic!("own FIFO validator child deadline");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(exit.success());
            assert_eq!(
                crate::team_process::state(&identity),
                crate::team_replacement::ProcessState::Gone
            );
        });
    }
}

#[test]
fn e1_physical_descendant_cap_is_enforced_by_fd_enumeration() {
    let f = Fixture::new();
    let root = f.root(A);
    chmod(&root, 0o700);
    for i in 0..TREE_CAP {
        let path = root.join(format!("extra-{i:05}"));
        std::fs::write(&path, b"").unwrap();
        chmod(&path, 0o400);
    }
    chmod(&root, 0o500);
    refused(&f);
}
