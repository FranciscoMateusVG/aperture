use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub const PRIVATE_DIR_MODE: u32 = 0o700;
pub const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_JSON_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalOperation {
    Activate,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalRoot {
    Teams,
    Staging,
    Agents,
    Owner,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JournalObjectKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalMove {
    pub from_root: JournalRoot,
    pub from_rel: String,
    pub to_root: JournalRoot,
    pub to_rel: String,
    pub kind: JournalObjectKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Journal {
    pub schema_version: u32,
    pub operation: JournalOperation,
    pub team: String,
    pub uuid: String,
    pub moves: Vec<JournalMove>,
    /// Progress hint only. Recovery always reconciles the physical tree.
    pub step: usize,
    pub preimage_sha256: String,
}

#[derive(Debug, Clone)]
pub struct JournalRoots {
    pub teams: PathBuf,
    pub staging: PathBuf,
    pub agents: PathBuf,
    pub owner: PathBuf,
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn ensure_relative(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("E_PATH_UNSAFE: empty relative path".into());
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("E_PATH_UNSAFE: path must contain normal relative components only".into());
    }
    Ok(path.to_path_buf())
}

pub fn ensure_private_dir(path: &Path) -> Result<(), String> {
    if path.exists() {
        let meta = fs::symlink_metadata(path).map_err(|e| format!("E_PERMISSION_UNSAFE: {e}"))?;
        if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != current_uid() {
            return Err(format!("E_PERMISSION_UNSAFE: unsafe directory {}", path.display()));
        }
        if meta.mode() & 0o077 != 0 {
            return Err(format!("E_PERMISSION_UNSAFE: directory is not private {}", path.display()));
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            ensure_private_dir(parent)?;
        } else {
            let meta = fs::symlink_metadata(parent).map_err(|e| format!("E_PERMISSION_UNSAFE: {e}"))?;
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || meta.uid() != current_uid()
                || (parent.file_name().is_some_and(|name| matches!(name.to_str(), Some(".aperture" | "run" | "teams" | ".staging" | "owner" | "team-locks")))
                    && meta.mode() & 0o077 != 0)
            {
                return Err(format!("E_PERMISSION_UNSAFE: unsafe parent {}", parent.display()));
            }
        }
    }
    fs::create_dir(path).map_err(|e| format!("E_STAGING_IO: {e}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|e| format!("E_STAGING_IO: {e}"))?;
    sync_parent(path)?;
    ensure_private_dir(path)
}

pub fn validate_private_file(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("E_PERMISSION_UNSAFE: {e}"))?;
    if !meta.is_file()
        || meta.file_type().is_symlink()
        || meta.uid() != current_uid()
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
    {
        return Err(format!("E_PERMISSION_UNSAFE: unsafe private file {}", path.display()));
    }
    Ok(())
}

fn c_string(value: &std::ffi::OsStr) -> Result<CString, String> {
    CString::new(value.as_bytes()).map_err(|_| "E_PATH_UNSAFE: NUL path".to_string())
}

/// Opens a directory one component at a time. Holding every parent descriptor
/// while opening the child makes a path-component swap observable rather than
/// following a newly inserted symlink between validation and use.
fn open_dir_nofollow(path: &Path) -> Result<File, String> {
    // Resolve operating-system aliases above the managed root (macOS `/var`
    // is normally a symlink), then walk the resolved path without following
    // any component that can be swapped by a caller inside the managed tree.
    let canonical = fs::canonicalize(path).map_err(|e| format!("E_PATH_UNSAFE: cannot resolve directory: {e}"))?;
    let expected = fs::metadata(&canonical).map_err(|e| format!("E_PATH_UNSAFE: cannot stat directory: {e}"))?;
    let absolute = canonical.is_absolute();
    let start = if absolute { c_string(std::ffi::OsStr::new("/"))? } else { c_string(std::ffi::OsStr::new("."))? };
    let initial = unsafe { libc::open(start.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if initial < 0 {
        return Err(format!("E_PATH_UNSAFE: cannot open path root: {}", std::io::Error::last_os_error()));
    }
    let mut current = unsafe { File::from_raw_fd(initial) };
    for component in canonical.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => c_string(name)?,
            Component::ParentDir | Component::Prefix(_) => return Err("E_PATH_UNSAFE: unsafe directory component".into()),
        };
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(format!("E_PATH_UNSAFE: unsafe directory component: {}", std::io::Error::last_os_error()));
        }
        current = unsafe { File::from_raw_fd(fd) };
    }
    let opened = current.metadata().map_err(|e| format!("E_PATH_UNSAFE: cannot stat opened directory: {e}"))?;
    if opened.dev() != expected.dev() || opened.ino() != expected.ino() {
        return Err("E_PATH_UNSAFE: directory identity changed while opening".into());
    }
    Ok(current)
}

fn validate_open_private_file(file: &File) -> Result<std::fs::Metadata, String> {
    let meta = file.metadata().map_err(|e| format!("E_PERMISSION_UNSAFE: {e}"))?;
    if !meta.is_file() || meta.uid() != current_uid() || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
        return Err("E_PERMISSION_UNSAFE: unsafe open private file".into());
    }
    Ok(meta)
}

fn open_private_file_nofollow(path: &Path) -> Result<File, String> {
    let parent = path.parent().ok_or_else(|| "E_PATH_UNSAFE: file has no parent".to_string())?;
    let name = c_string(path.file_name().ok_or_else(|| "E_PATH_UNSAFE: file has no name".to_string())?)?;
    let directory = open_dir_nofollow(parent)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(format!("E_PERMISSION_UNSAFE: cannot open private file: {}", std::io::Error::last_os_error()));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    validate_open_private_file(&file)?;
    Ok(file)
}

pub fn validate_component_path(root: &Path, relative: &str, allow_missing_leaf: bool) -> Result<PathBuf, String> {
    ensure_private_dir(root)?;
    let rel = ensure_relative(relative)?;
    let mut current = root.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for (index, part) in parts.iter().enumerate() {
        current.push(part.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || meta.uid() != current_uid() {
                    return Err(format!("E_PATH_UNSAFE: unsafe path component {}", current.display()));
                }
                if index + 1 < parts.len() && !meta.is_dir() {
                    return Err(format!("E_PATH_UNSAFE: non-directory component {}", current.display()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && allow_missing_leaf && index + 1 == parts.len() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!("E_PATH_UNSAFE: missing path component {}", current.display()));
            }
            Err(e) => return Err(format!("E_PATH_UNSAFE: {e}")),
        }
    }
    if !current.starts_with(root) {
        return Err("E_PATH_UNSAFE: escaped fixed root".into());
    }
    Ok(current)
}

pub fn write_private_bytes_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "E_PATH_UNSAFE: file has no parent".to_string())?;
    ensure_private_dir(parent)?;
    let directory = open_dir_nofollow(parent)?;
    let target_name = c_string(path.file_name().ok_or_else(|| "E_PATH_UNSAFE: file has no name".to_string())?)?;
    let existing = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            target_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if existing >= 0 {
        let existing = unsafe { File::from_raw_fd(existing) };
        validate_open_private_file(&existing)?;
        if !replace {
            return Err(format!("E_NAME_COLLISION: {} already exists", path.display()));
        }
    } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
        return Err(format!("E_PERMISSION_UNSAFE: unsafe destination: {}", std::io::Error::last_os_error()));
    }
    let temp_name = format!(".{}.{}.tmp", path.file_name().and_then(|v| v.to_str()).unwrap_or("write"), Uuid::new_v4());
    let temp_c = c_string(std::ffi::OsStr::new(&temp_name))?;
    let result = (|| {
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                temp_c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                PRIVATE_FILE_MODE,
            )
        };
        if fd < 0 {
            return Err(format!("E_STAGING_IO: cannot create private temp: {}", std::io::Error::last_os_error()));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes).map_err(|e| format!("E_STAGING_IO: {e}"))?;
        file.sync_all().map_err(|e| format!("E_STAGING_IO: {e}"))?;
        validate_open_private_file(&file)?;
        if replace {
            let rc = unsafe { libc::renameat(directory.as_raw_fd(), temp_c.as_ptr(), directory.as_raw_fd(), target_name.as_ptr()) };
            if rc != 0 {
                return Err(format!("E_STAGING_IO: replace rename failed: {}", std::io::Error::last_os_error()));
            }
        } else {
            rename_no_replace_at(directory.as_raw_fd(), &temp_c, &target_name)?;
        }
        directory.sync_all().map_err(|e| format!("E_STAGING_IO: directory fsync failed: {e}"))?;
        open_private_file_nofollow(path).map(|_| ())
    })();
    if result.is_err() {
        unsafe { libc::unlinkat(directory.as_raw_fd(), temp_c.as_ptr(), 0); }
    }
    result
}

pub fn write_private_json_atomic<T: Serialize>(path: &Path, value: &T, replace: bool) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| format!("E_STAGING_IO: {e}"))?;
    bytes.push(b'\n');
    write_private_bytes_atomic(path, &bytes, replace)
}

pub fn read_private_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let mut file = open_private_file_nofollow(path)?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take((PRIVATE_JSON_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("E_STAGING_IO: {e}"))?;
    if bytes.len() > PRIVATE_JSON_MAX_BYTES {
        return Err("E_JOURNAL_INCONSISTENT: private JSON exceeds size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "E_JOURNAL_INCONSISTENT: malformed private JSON".into())
}

pub fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("E_STAGING_IO: directory fsync failed: {e}"))
}

pub fn sync_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) if parent.exists() => sync_dir(parent),
        _ => Ok(()),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> Result<(), String> {
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| "E_PATH_UNSAFE: NUL path".to_string())?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| "E_PATH_UNSAFE: NUL path".to_string())?;
    let rc = unsafe { libc::renameatx_np(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 { Ok(()) } else { Err(format!("E_NAME_COLLISION: no-replace rename failed: {}", std::io::Error::last_os_error())) }
}

#[cfg(target_os = "macos")]
fn rename_no_replace_at(directory_fd: i32, from: &CString, to: &CString) -> Result<(), String> {
    let rc = unsafe { libc::renameatx_np(directory_fd, from.as_ptr(), directory_fd, to.as_ptr(), libc::RENAME_EXCL) };
    if rc == 0 { Ok(()) } else { Err(format!("E_NAME_COLLISION: no-replace rename failed: {}", std::io::Error::last_os_error())) }
}

#[cfg(target_os = "linux")]
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> Result<(), String> {
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| "E_PATH_UNSAFE: NUL path".to_string())?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| "E_PATH_UNSAFE: NUL path".to_string())?;
    let rc = unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) };
    if rc == 0 { Ok(()) } else { Err(format!("E_NAME_COLLISION: no-replace rename failed: {}", std::io::Error::last_os_error())) }
}

#[cfg(target_os = "linux")]
fn rename_no_replace_at(directory_fd: i32, from: &CString, to: &CString) -> Result<(), String> {
    let rc = unsafe { libc::renameat2(directory_fd, from.as_ptr(), directory_fd, to.as_ptr(), libc::RENAME_NOREPLACE) };
    if rc == 0 { Ok(()) } else { Err(format!("E_NAME_COLLISION: no-replace rename failed: {}", std::io::Error::last_os_error())) }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_no_replace(_from: &Path, _to: &Path) -> Result<(), String> {
    Err("E_STAGING_IO: no-replace rename unsupported on this platform".into())
}


#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rename_no_replace_at(_directory_fd: i32, _from: &CString, _to: &CString) -> Result<(), String> {
    Err("E_STAGING_IO: no-replace rename unsupported on this platform".into())
}

fn root_path<'a>(roots: &'a JournalRoots, root: &JournalRoot) -> &'a Path {
    match root {
        JournalRoot::Teams => &roots.teams,
        JournalRoot::Staging => &roots.staging,
        JournalRoot::Agents => &roots.agents,
        JournalRoot::Owner => &roots.owner,
    }
}

fn move_paths(roots: &JournalRoots, mv: &JournalMove) -> Result<(PathBuf, PathBuf), String> {
    let from = validate_component_path(root_path(roots, &mv.from_root), &mv.from_rel, false)?;
    let to_root = root_path(roots, &mv.to_root);
    let to = validate_component_path(to_root, &mv.to_rel, true)?;
    if let Some(parent) = to.parent() {
        ensure_private_dir(parent)?;
    }
    Ok((from, to))
}

fn validate_kind(path: &Path, kind: &JournalObjectKind) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("E_JOURNAL_INCONSISTENT: {e}"))?;
    if meta.file_type().is_symlink() || meta.uid() != current_uid() {
        return Err("E_JOURNAL_INCONSISTENT: unsafe move object".into());
    }
    let matches = match kind {
        JournalObjectKind::File => meta.is_file() && meta.nlink() == 1 && meta.mode() & 0o077 == 0,
        JournalObjectKind::Directory => meta.is_dir() && meta.mode() & 0o077 == 0,
    };
    if !matches {
        return Err("E_JOURNAL_INCONSISTENT: move object type or permissions mismatch".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhysicalMoveState { Done, NotDone }

fn physical_state(roots: &JournalRoots, mv: &JournalMove) -> Result<(PhysicalMoveState, PathBuf, PathBuf), String> {
    let from_root = root_path(roots, &mv.from_root);
    let to_root = root_path(roots, &mv.to_root);
    let from_rel = ensure_relative(&mv.from_rel)?;
    let to_rel = ensure_relative(&mv.to_rel)?;
    let from = from_root.join(from_rel);
    let to = to_root.join(to_rel);
    let from_exists = fs::symlink_metadata(&from).is_ok();
    let to_exists = fs::symlink_metadata(&to).is_ok();
    match (from_exists, to_exists) {
        (true, false) => { validate_component_path(from_root, &mv.from_rel, false)?; validate_kind(&from, &mv.kind)?; Ok((PhysicalMoveState::NotDone, from, to)) }
        (false, true) => { validate_component_path(to_root, &mv.to_rel, false)?; validate_kind(&to, &mv.kind)?; Ok((PhysicalMoveState::Done, from, to)) }
        _ => Err("E_JOURNAL_INCONSISTENT: move has both or neither physical side".into()),
    }
}

pub fn write_journal(path: &Path, journal: &Journal) -> Result<(), String> {
    if journal.schema_version != 1 || journal.moves.is_empty() || journal.step > journal.moves.len() {
        return Err("E_JOURNAL_INCONSISTENT: invalid journal shape".into());
    }
    write_private_json_atomic(path, journal, false)
}

pub fn read_journal(path: &Path) -> Result<Journal, String> {
    let journal: Journal = read_private_json(path)?;
    if journal.schema_version != 1 || journal.moves.is_empty() || journal.step > journal.moves.len() {
        return Err("E_JOURNAL_INCONSISTENT: invalid journal shape".into());
    }
    Ok(journal)
}

pub fn apply_or_recover_journal(path: &Path, roots: &JournalRoots) -> Result<Journal, String> {
    let mut journal = read_journal(path)?;
    for index in 0..journal.moves.len() {
        let mv = &journal.moves[index];
        let (state, from, to) = physical_state(roots, mv)?;
        if state == PhysicalMoveState::NotDone {
            let (_, checked_to) = move_paths(roots, mv)?;
            if checked_to != to { return Err("E_PATH_UNSAFE: move path changed".into()); }
            rename_no_replace(&from, &to)?;
            sync_parent(&from)?;
            sync_parent(&to)?;
            validate_kind(&to, &mv.kind)?;
        }
        journal.step = index + 1;
        write_private_json_atomic(path, &journal, true)?;
    }
    Ok(journal)
}

pub fn remove_journal(path: &Path) -> Result<(), String> {
    validate_private_file(path)?;
    fs::remove_file(path).map_err(|e| format!("E_STAGING_IO: {e}"))?;
    sync_parent(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("aperture-journal-{tag}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn reconciles_rename_that_happened_before_step_fsync() {
        let base = root("crash");
        let roots = JournalRoots { teams: base.join("teams"), staging: base.join("staging"), agents: base.join("agents"), owner: base.join("owner") };
        for p in [&roots.teams, &roots.staging, &roots.agents, &roots.owner] { ensure_private_dir(p).unwrap(); }
        ensure_private_dir(&roots.staging.join("u/seats/s1")).unwrap();
        let journal_path = roots.teams.join("journal.json");
        let journal = Journal { schema_version:1, operation:JournalOperation::Activate, team:"t1".into(), uuid:"u".into(), moves:vec![JournalMove { from_root:JournalRoot::Staging, from_rel:"u/seats/s1".into(), to_root:JournalRoot::Agents, to_rel:"s1".into(), kind:JournalObjectKind::Directory }], step:0, preimage_sha256:"0".repeat(64) };
        write_journal(&journal_path, &journal).unwrap();
        rename_no_replace(&roots.staging.join("u/seats/s1"), &roots.agents.join("s1")).unwrap();
        let recovered = apply_or_recover_journal(&journal_path, &roots).unwrap();
        assert_eq!(recovered.step, 1);
        assert!(roots.agents.join("s1").is_dir());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn both_sides_is_terminal_and_never_deleted() {
        let base = root("both");
        let roots = JournalRoots { teams: base.join("teams"), staging: base.join("staging"), agents: base.join("agents"), owner: base.join("owner") };
        for p in [&roots.teams, &roots.staging, &roots.agents, &roots.owner] { ensure_private_dir(p).unwrap(); }
        ensure_private_dir(&roots.staging.join("u/seats/s1")).unwrap();
        ensure_private_dir(&roots.agents.join("s1")).unwrap();
        let path = roots.teams.join("journal.json");
        write_journal(&path, &Journal { schema_version:1, operation:JournalOperation::Activate, team:"t1".into(), uuid:"u".into(), moves:vec![JournalMove { from_root:JournalRoot::Staging, from_rel:"u/seats/s1".into(), to_root:JournalRoot::Agents, to_rel:"s1".into(), kind:JournalObjectKind::Directory }], step:0, preimage_sha256:"0".repeat(64) }).unwrap();
        assert!(apply_or_recover_journal(&path, &roots).unwrap_err().contains("E_JOURNAL_INCONSISTENT"));
        assert!(roots.staging.join("u/seats/s1").exists());
        assert!(roots.agents.join("s1").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn private_json_io_never_follows_a_leaf_swapped_to_a_symlink() {
        let base = root("leaf-swap");
        let target = base.join("state.json");
        let outside = base.parent().unwrap().join(format!("aperture-outside-{}.json", Uuid::new_v4()));
        fs::write(&outside, b"{\"outside\":true}\n").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
        write_private_json_atomic(&target, &json!({"inside": true}), false).unwrap();
        validate_private_file(&target).unwrap();

        fs::remove_file(&target).unwrap();
        symlink(&outside, &target).unwrap();
        assert!(read_private_json::<serde_json::Value>(&target).unwrap_err().contains("E_PERMISSION_UNSAFE"));
        assert!(write_private_json_atomic(&target, &json!({"inside": false}), true)
            .unwrap_err()
            .contains("E_PERMISSION_UNSAFE"));
        assert_eq!(fs::read_to_string(&outside).unwrap(), "{\"outside\":true}\n");

        fs::remove_dir_all(base).unwrap();
        fs::remove_file(outside).unwrap();
    }
}
