//! E1 read-only internal consistency, NOT provenance, execution or authority.
//! No caller is wired; all startup/lifecycle fences remain in their own modules.
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const UNAVAILABLE: &str = "E_RELEASE_UNAVAILABLE";
type Result<T> = std::result::Result<T, &'static str>;
const MANIFEST_CAP: u64 = 2 * 1024 * 1024;
const FILE_CAP: u64 = 128 * 1024 * 1024;
const TOTAL_CAP: u64 = 512 * 1024 * 1024;
const FILES_CAP: usize = 8192;
const TREE_CAP: usize = 10000;
const FLAGS: i32 = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    ApertureServer,
    ApertureBoot,
    ApertureTeamControl,
    McpServerEntry,
    HubServerEntry,
    HubClientEntry,
    SentryServerEntry,
}
const ROLES: [Role; 7] = [
    Role::ApertureServer,
    Role::ApertureBoot,
    Role::ApertureTeamControl,
    Role::McpServerEntry,
    Role::HubServerEntry,
    Role::HubClientEntry,
    Role::SentryServerEntry,
];
impl Role {
    fn entry(self) -> (&'static str, Kind) {
        match self {
            Self::ApertureServer => ("bin/aperture-server", Kind::Executable),
            Self::ApertureBoot => ("bin/aperture-boot", Kind::Executable),
            Self::ApertureTeamControl => ("bin/aperture-team-control", Kind::Executable),
            Self::McpServerEntry => ("mcp-server/dist/index.js", Kind::Data),
            Self::HubServerEntry => ("mcp-server/dist/ws-hub.js", Kind::Data),
            Self::HubClientEntry => ("mcp-server/dist/hub-client.js", Kind::Data),
            Self::SentryServerEntry => ("mcp-server-sentry/dist/index.js", Kind::Data),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Data,
    Executable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InventoryRole {
    ApertureServer,
    ApertureBoot,
    ApertureTeamControl,
    McpServerEntry,
    HubServerEntry,
    HubClientEntry,
    SentryServerEntry,
    Payload,
}
// JSON enums are strings only, not serde's alternate externally-tagged map
// representation; this keeps the two explicit object visitors the whole schema.
impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "data" => Ok(Self::Data),
            "executable" => Ok(Self::Executable),
            _ => Err(de::Error::custom("unknown kind")),
        }
    }
}
impl<'de> Deserialize<'de> for InventoryRole {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "aperture_server" => Ok(Self::ApertureServer),
            "aperture_boot" => Ok(Self::ApertureBoot),
            "aperture_team_control" => Ok(Self::ApertureTeamControl),
            "mcp_server_entry" => Ok(Self::McpServerEntry),
            "hub_server_entry" => Ok(Self::HubServerEntry),
            "hub_client_entry" => Ok(Self::HubClientEntry),
            "sentry_server_entry" => Ok(Self::SentryServerEntry),
            "payload" => Ok(Self::Payload),
            _ => Err(de::Error::custom("unknown role")),
        }
    }
}
impl InventoryRole {
    fn selected(self) -> Option<Role> {
        match self {
            Self::ApertureServer => Some(Role::ApertureServer),
            Self::ApertureBoot => Some(Role::ApertureBoot),
            Self::ApertureTeamControl => Some(Role::ApertureTeamControl),
            Self::McpServerEntry => Some(Role::McpServerEntry),
            Self::HubServerEntry => Some(Role::HubServerEntry),
            Self::HubClientEntry => Some(Role::HubClientEntry),
            Self::SentryServerEntry => Some(Role::SentryServerEntry),
            Self::Payload => None,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    path: String,
    sha256: String,
    size: u64,
    kind: Kind,
    role: InventoryRole,
}
#[derive(Debug, PartialEq, Eq)]
struct Manifest {
    schema_version: u32,
    release_sha: String,
    api_schema: u32,
    files: Vec<Entry>,
}

// Explicit visitors reject repeated keys before decoding their second value.
// No intermediate JSON map can erase a duplicate, including inside files[].
macro_rules! field {
    ($slot:ident, $map:ident) => {{
        if $slot.is_some() {
            return Err(de::Error::custom("duplicate field"));
        }
        $slot = Some($map.next_value()?);
    }};
}
impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entry;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("closed file record")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Entry, M::Error> {
                let (mut path, mut sha256, mut size, mut kind, mut role) =
                    (None, None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "path" => field!(path, map),
                        "sha256" => field!(sha256, map),
                        "size" => field!(size, map),
                        "kind" => field!(kind, map),
                        "role" => field!(role, map),
                        _ => return Err(de::Error::custom("unknown field")),
                    }
                }
                Ok(Entry {
                    path: path.ok_or_else(|| de::Error::missing_field("path"))?,
                    sha256: sha256.ok_or_else(|| de::Error::missing_field("sha256"))?,
                    size: size.ok_or_else(|| de::Error::missing_field("size"))?,
                    kind: kind.ok_or_else(|| de::Error::missing_field("kind"))?,
                    role: role.ok_or_else(|| de::Error::missing_field("role"))?,
                })
            }
        }
        d.deserialize_map(V)
    }
}
struct Files(Vec<Entry>);
impl<'de> Deserialize<'de> for Files {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Files;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bounded inventory")
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Files, S::Error> {
                let mut files = Vec::new();
                // At the limit only IgnoredAny is decoded: no extra Entry allocation.
                while files.len() < FILES_CAP {
                    match seq.next_element()? {
                        Some(v) => files.push(v),
                        None => return Ok(Files(files)),
                    }
                }
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("file cap"));
                }
                Ok(Files(files))
            }
        }
        d.deserialize_seq(V)
    }
}
impl<'de> Deserialize<'de> for Manifest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Manifest;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("closed manifest")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Manifest, M::Error> {
                let (mut schema_version, mut release_sha, mut api_schema, mut files) =
                    (None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "schema_version" => field!(schema_version, map),
                        "release_sha" => field!(release_sha, map),
                        "api_schema" => field!(api_schema, map),
                        "files" => field!(files, map),
                        _ => return Err(de::Error::custom("unknown field")),
                    }
                }
                let Files(files) = files.ok_or_else(|| de::Error::missing_field("files"))?;
                Ok(Manifest {
                    schema_version: schema_version
                        .ok_or_else(|| de::Error::missing_field("schema_version"))?,
                    release_sha: release_sha
                        .ok_or_else(|| de::Error::missing_field("release_sha"))?,
                    api_schema: api_schema.ok_or_else(|| de::Error::missing_field("api_schema"))?,
                    files,
                })
            }
        }
        d.deserialize_map(V)
    }
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@+".contains(&b))
}
fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value.split('/').count() <= 32
        && value.split('/').all(component)
}
fn parse(bytes: &[u8], sha: &str) -> Result<(Manifest, Vec<String>)> {
    if bytes.len() as u64 > MANIFEST_CAP {
        return Err(UNAVAILABLE);
    }
    let m: Manifest = serde_json::from_slice(bytes).map_err(|_| UNAVAILABLE)?;
    if m.schema_version != 1
        || m.api_schema != 1
        || !hex(sha, 40)
        || m.release_sha != sha
        || m.files.is_empty()
    {
        return Err(UNAVAILABLE);
    }
    let mut total = 0u64;
    for (i, e) in m.files.iter().enumerate() {
        if !relative(&e.path)
            || e.path.eq_ignore_ascii_case("RELEASE.json")
            || !hex(&e.sha256, 64)
            || e.size > FILE_CAP
            || (i > 0 && m.files[i - 1].path >= e.path)
        {
            return Err(UNAVAILABLE);
        }
        total = total
            .checked_add(e.size)
            .filter(|v| *v <= TOTAL_CAP)
            .ok_or(UNAVAILABLE)?;
        match e.role.selected() {
            Some(role) if role.entry() != (e.path.as_str(), e.kind) => return Err(UNAVAILABLE),
            None if e.kind != Kind::Data => return Err(UNAVAILABLE),
            _ => {}
        }
    }
    for role in ROLES {
        if m.files
            .iter()
            .filter(|e| e.role.selected() == Some(role))
            .count()
            != 1
        {
            return Err(UNAVAILABLE);
        }
    }
    // Check aliases and prefix collisions before a map or payload filesystem access.
    let mut paths: Vec<_> = m
        .files
        .iter()
        .map(|e| e.path.to_ascii_lowercase())
        .collect();
    paths.sort();
    if paths.windows(2).any(|p| p[0] == p[1]) {
        return Err(UNAVAILABLE);
    }
    let mut dirs: Vec<(String, String)> = Vec::new();
    for e in &m.files {
        for (at, _) in e.path.match_indices('/') {
            let path = &e.path[..at];
            let lower = path.to_ascii_lowercase();
            if paths.binary_search(&lower).is_ok() || lower == "release.json" {
                return Err(UNAVAILABLE);
            }
            match dirs.binary_search_by(|(fold, _)| fold.cmp(&lower)) {
                Ok(index) if dirs[index].1 != path => return Err(UNAVAILABLE),
                Ok(_) => {}
                Err(index) => {
                    if m.files
                        .len()
                        .checked_add(dirs.len())
                        .and_then(|n| n.checked_add(2))
                        .is_none_or(|n| n > TREE_CAP)
                    {
                        return Err(UNAVAILABLE);
                    }
                    dirs.insert(index, (lower, path.into()));
                }
            }
        }
    }
    let mut dirs: Vec<_> = dirs.into_iter().map(|(_, p)| p).collect();
    dirs.sort();
    Ok((m, dirs))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pin {
    dev: u64,
    ino: u64,
    uid: u32,
    mode: u32,
    nlink: u64,
    size: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
}
impl Pin {
    fn of(m: &Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
            mode: m.mode(),
            nlink: m.nlink(),
            size: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
        }
    }
    fn anchor_eq(&self, other: &Self) -> bool {
        (self.dev, self.ino, self.uid, self.mode) == (other.dev, other.ino, other.uid, other.mode)
    }
}
fn immutable_dir(file: &File) -> Result<Pin> {
    let m = file.metadata().map_err(|_| UNAVAILABLE)?;
    if !m.is_dir()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o500 != 0o500
        || m.mode() & 0o7222 != 0
    {
        return Err(UNAVAILABLE);
    }
    Ok(Pin::of(&m))
}
fn leaf(file: &File, kind: Kind, cap: u64) -> Result<Pin> {
    let m = file.metadata().map_err(|_| UNAVAILABLE)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o400 == 0
        || m.mode() & 0o7222 != 0
        || m.len() > cap
        || (kind == Kind::Data && m.mode() & 0o111 != 0)
        || (kind == Kind::Executable && m.mode() & 0o100 == 0)
    {
        return Err(UNAVAILABLE);
    }
    Ok(Pin::of(&m))
}
fn openat(parent: &File, name: &str, dir: bool) -> Result<File> {
    let name = CString::new(name).map_err(|_| UNAVAILABLE)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            FLAGS | if dir { libc::O_DIRECTORY } else { 0 },
        )
    };
    if fd < 0 {
        Err(UNAVAILABLE)
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn root_fd() -> Result<File> {
    let fd = unsafe { libc::open(c"/".as_ptr(), FLAGS | libc::O_DIRECTORY) };
    if fd < 0 {
        Err(UNAVAILABLE)
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
// Mutable ancestors are checked for identity/ownership/mode, not timestamps:
// changing UI/current or an unrelated temporary directory must not drift A.
struct Ancestor {
    path: PathBuf,
    pin: Pin,
}
fn open_home(home: &Path) -> Result<(File, Vec<Ancestor>)> {
    if !home.is_absolute() || home == Path::new("/") {
        return Err(UNAVAILABLE);
    }
    let mut fd = root_fd()?;
    let m = fd.metadata().map_err(|_| UNAVAILABLE)?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & 0o7022 != 0 {
        return Err(UNAVAILABLE);
    }
    let mut path = PathBuf::from("/");
    let mut anchors = Vec::new();
    for part in home.components().skip(1) {
        let Component::Normal(part) = part else {
            return Err(UNAVAILABLE);
        };
        let name = part.to_str().ok_or(UNAVAILABLE)?;
        fd = openat(&fd, name, true)?;
        path.push(part);
        let m = fd.metadata().map_err(|_| UNAVAILABLE)?;
        let system_tmp =
            path == Path::new("/private/tmp") && m.uid() == 0 && m.mode() & 0o7777 == 0o1777;
        if !m.is_dir()
            || (m.uid() != 0 && m.uid() != unsafe { libc::geteuid() })
            || (!system_tmp && m.mode() & 0o7022 != 0)
            || (path == home
                && (m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o500 != 0o500))
        {
            return Err(UNAVAILABLE);
        }
        anchors.push(Ancestor {
            path: path.clone(),
            pin: Pin::of(&m),
        });
    }
    Ok((fd, anchors))
}
fn open_named_root(home: &Path, sha: &str) -> Result<(File, Vec<Ancestor>)> {
    if !hex(sha, 40) {
        return Err(UNAVAILABLE);
    }
    let (mut fd, mut anchors) = open_home(home)?;
    let mut path = home.to_path_buf();
    for name in [".aperture", "releases"] {
        fd = openat(&fd, name, true)?;
        path.push(name);
        let m = fd.metadata().map_err(|_| UNAVAILABLE)?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o700 {
            return Err(UNAVAILABLE);
        }
        anchors.push(Ancestor {
            path: path.clone(),
            pin: Pin::of(&m),
        });
    }
    let fd = openat(&fd, sha, true)?;
    immutable_dir(&fd)?;
    Ok((fd, anchors))
}
fn same_anchors(a: &[Ancestor], b: &[Ancestor]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.path == b.path && a.pin.anchor_eq(&b.pin))
}
fn file_read(
    parent: &File,
    name: &str,
    kind: Kind,
    cap: u64,
    keep: bool,
    expected_size: Option<u64>,
) -> Result<(Pin, String, Vec<u8>)> {
    let mut file = openat(parent, name, false)?;
    let before = leaf(&file, kind, cap)?;
    if expected_size.is_some_and(|size| size != before.size) {
        return Err(UNAVAILABLE);
    }
    let mut sha = Sha256::new();
    let mut count = 0u64;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16384];
    loop {
        let n = file.read(&mut buffer).map_err(|_| UNAVAILABLE)?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .filter(|n| *n <= cap)
            .ok_or(UNAVAILABLE)?;
        sha.update(&buffer[..n]);
        if keep {
            bytes.extend_from_slice(&buffer[..n]);
        }
    }
    if count != before.size
        || leaf(&file, kind, cap)? != before
        || leaf(&openat(parent, name, false)?, kind, cap)? != before
    {
        return Err(UNAVAILABLE);
    }
    Ok((before, format!("{:x}", sha.finalize()), bytes))
}
struct Dir(*mut libc::DIR);
impl Drop for Dir {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}
fn names(fd: &File, left: &mut usize) -> Result<Vec<String>> {
    // Reuse the native fdopendir/openat(.) pattern, not dup's shared offset.
    let copy = openat(fd, ".", true)?.into_raw_fd();
    let raw = unsafe { libc::fdopendir(copy) };
    if raw.is_null() {
        unsafe { libc::close(copy) };
        return Err(UNAVAILABLE);
    }
    let dir = Dir(raw);
    let mut out = Vec::new();
    loop {
        #[cfg(target_os = "macos")]
        let errno = unsafe { libc::__error() };
        #[cfg(not(target_os = "macos"))]
        let errno = unsafe { libc::__errno_location() };
        unsafe {
            *errno = 0;
        }
        let entry = unsafe { libc::readdir(dir.0) };
        if entry.is_null() {
            if unsafe { *errno } != 0 {
                return Err(UNAVAILABLE);
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
            .to_str()
            .map_err(|_| UNAVAILABLE)?;
        if name == "." || name == ".." {
            continue;
        }
        *left = left.checked_sub(1).ok_or(UNAVAILABLE)?;
        if !component(name) {
            return Err(UNAVAILABLE);
        }
        out.push(name.into());
    }
    out.sort();
    if out.windows(2).any(|n| n[0] == n[1]) {
        return Err(UNAVAILABLE);
    }
    Ok(out)
}
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    path: String,
    pin: Pin,
    hash: Option<String>,
}
fn observe_tree(root: &File, m: &Manifest, dirs: &[String]) -> Result<Vec<Observed>> {
    fn walk(
        fd: &File,
        prefix: &str,
        m: &Manifest,
        dirs: &[String],
        left: &mut usize,
        out: &mut Vec<Observed>,
    ) -> Result<()> {
        let before = immutable_dir(fd)?;
        for name in names(fd, left)? {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if !relative(&path) {
                return Err(UNAVAILABLE);
            }
            if dirs.binary_search(&path).is_ok() {
                let sub = openat(fd, &name, true)?;
                let pin = immutable_dir(&sub)?;
                walk(&sub, &path, m, dirs, left, out)?;
                if immutable_dir(&openat(fd, &name, true)?)? != pin {
                    return Err(UNAVAILABLE);
                }
                out.push(Observed {
                    path,
                    pin,
                    hash: None,
                });
            } else {
                let (kind, cap, expected) = if path == "RELEASE.json" {
                    (Kind::Data, MANIFEST_CAP, None)
                } else {
                    let index = m
                        .files
                        .binary_search_by(|e| e.path.cmp(&path))
                        .map_err(|_| UNAVAILABLE)?;
                    let e = &m.files[index];
                    (e.kind, FILE_CAP, Some(e))
                };
                let (pin, hash, _) =
                    file_read(fd, &name, kind, cap, false, expected.map(|e| e.size))?;
                if expected.is_some_and(|e| e.size != pin.size || e.sha256 != hash) {
                    return Err(UNAVAILABLE);
                }
                out.push(Observed {
                    path,
                    pin,
                    hash: Some(hash),
                });
            }
        }
        if immutable_dir(fd)? != before {
            return Err(UNAVAILABLE);
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut left = TREE_CAP;
    walk(root, "", m, dirs, &mut left, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    let mut expected: Vec<_> = m
        .files
        .iter()
        .map(|e| e.path.clone())
        .chain(dirs.iter().cloned())
        .chain(["RELEASE.json".into()])
        .collect();
    expected.sort();
    if out.len() != expected.len() || out.iter().zip(&expected).any(|(a, b)| a.path != *b) {
        return Err(UNAVAILABLE);
    }
    Ok(out)
}
struct Binding {
    home: PathBuf,
    root_path: PathBuf,
    root: File,
    root_pin: Pin,
    ancestors: Vec<Ancestor>,
    manifest: Manifest,
    dirs: Vec<String>,
    observed: Vec<Observed>,
    unavailable: AtomicBool,
}
impl Binding {
    fn load(home: &Path, sha: &str) -> Result<Self> {
        let (root, ancestors) = open_named_root(home, sha)?;
        let root_pin = immutable_dir(&root)?;
        let (manifest_pin, manifest_hash, bytes) =
            file_read(&root, "RELEASE.json", Kind::Data, MANIFEST_CAP, true, None)?;
        let (manifest, dirs) = parse(&bytes, sha)?;
        let observed = observe_tree(&root, &manifest, &dirs)?;
        if !observed.iter().any(|e| {
            e.path == "RELEASE.json"
                && e.pin == manifest_pin
                && e.hash.as_ref() == Some(&manifest_hash)
        }) {
            return Err(UNAVAILABLE);
        }
        let result = Self {
            home: home.into(),
            root_path: home.join(".aperture/releases").join(sha),
            root,
            root_pin,
            ancestors,
            manifest,
            dirs,
            observed,
            unavailable: AtomicBool::new(false),
        };
        // Confirm the complete name set and all pins/hashes again at return.
        result.recheck()?;
        Ok(result)
    }
    fn live(&self) -> Result<()> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err(UNAVAILABLE)
        } else {
            Ok(())
        }
    }
    fn recheck(&self) -> Result<()> {
        self.live()?;
        let result = (|| {
            let (named, anchors) = open_named_root(&self.home, &self.manifest.release_sha)?;
            if !same_anchors(&self.ancestors, &anchors)
                || immutable_dir(&named)? != self.root_pin
                || immutable_dir(&self.root)? != self.root_pin
            {
                return Err(UNAVAILABLE);
            }
            if observe_tree(&self.root, &self.manifest, &self.dirs)? != self.observed {
                return Err(UNAVAILABLE);
            }
            let (named, anchors) = open_named_root(&self.home, &self.manifest.release_sha)?;
            if !same_anchors(&self.ancestors, &anchors)
                || immutable_dir(&named)? != self.root_pin
                || immutable_dir(&self.root)? != self.root_pin
            {
                return Err(UNAVAILABLE);
            }
            Ok(())
        })();
        if result.is_err() {
            self.unavailable.store(true, Ordering::SeqCst);
            return Err(UNAVAILABLE);
        }
        self.live()
    }
    fn role(&self, role: Role) -> Result<PathBuf> {
        self.recheck()?;
        Ok(self.root_path.join(role.entry().0))
    }
}
// No conversion, Deref, FD getter or arbitrary relative lookup for these types.
pub(crate) struct ProcessRelease(Binding);
pub(crate) struct RecordedRelease(Binding);
impl ProcessRelease {
    pub(crate) fn for_process(home: &Path) -> Result<Self> {
        let executable = std::env::current_exe().map_err(|_| UNAVAILABLE)?;
        Self::from_process_path(home, &executable)
    }
    fn from_process_path(home: &Path, executable: &Path) -> Result<Self> {
        let base = home.join(".aperture/releases");
        let relative = executable.strip_prefix(&base).map_err(|_| UNAVAILABLE)?;
        let parts: Vec<_> = relative.components().collect();
        if parts.len() != 3 || parts[1].as_os_str() != "bin" {
            return Err(UNAVAILABLE);
        }
        let sha = parts[0].as_os_str().to_str().ok_or(UNAVAILABLE)?;
        let role = ROLES
            .into_iter()
            .take(3)
            .find(|r| Path::new(r.entry().0) == Path::new("bin").join(parts[2].as_os_str()))
            .ok_or(UNAVAILABLE)?;
        if executable.as_os_str() != base.join(sha).join(role.entry().0).as_os_str() {
            return Err(UNAVAILABLE);
        }
        // This is a disk identity anchor, not an attestation of mapped bytes.
        let before = std::fs::symlink_metadata(executable).map_err(|_| UNAVAILABLE)?;
        let binding = Binding::load(home, sha)?;
        let expected = &binding
            .observed
            .iter()
            .find(|e| e.path == role.entry().0)
            .ok_or(UNAVAILABLE)?
            .pin;
        if &Pin::of(&before) != expected {
            return Err(UNAVAILABLE);
        }
        binding.recheck()?;
        Ok(Self(binding))
    }
    #[cfg(test)]
    fn fixture(home: &Path, executable: &Path) -> Result<Self> {
        Self::from_process_path(home, executable)
    }
}
impl RecordedRelease {
    /// The caller must obtain sha from its separately validated durable record.
    pub(crate) fn for_record(home: &Path, sha: &str) -> Result<Self> {
        Binding::load(home, sha).map(Self)
    }
}
macro_rules! inspection {
    ($ty:ident) => {
        impl $ty {
            pub(crate) fn release_sha(&self) -> Result<&str> {
                self.0.live()?;
                Ok(&self.0.manifest.release_sha)
            }
            pub(crate) fn api_schema(&self) -> Result<u32> {
                self.0.live()?;
                Ok(self.0.manifest.api_schema)
            }
            pub(crate) fn role(&self, role: Role) -> Result<PathBuf> {
                self.0.role(role)
            }
            pub(crate) fn recheck(&self) -> Result<()> {
                self.0.recheck()
            }
        }
    };
}
inspection!(ProcessRelease);
inspection!(RecordedRelease);
#[cfg(test)]
#[path = "runtime_release_tests.rs"]
mod tests;
