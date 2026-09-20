//! Common denial boundary for legacy lifecycle entrypoints. Registry membership
//! is supplied by the authoritative teams collector; this module does not build
//! a second registry or infer authority from a UI list/status cache.
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Standing,
    Team,
    Unknown,
}

pub const DENIED: &str = "E_TEAM_LIFECYCLE_REQUIRED";

/// Any TEAM entry (including broken symlink/directory), failed filesystem read,
/// team membership in ANY lifecycle, or unavailable registry evidence denies
/// legacy effects. The caller reuses this before both GUI/headless boot and
/// before legacy restart/stop/model mutation. No fallback on a failed team list.
pub fn ensure_legacy(root: &Path, name: &str, membership: Membership) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 31
        || !name.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'-' || b == b'_'))
        })
    {
        return Err(DENIED.into());
    }
    if membership != Membership::Standing {
        return Err(DENIED.into());
    }
    let seat = root.join(name);
    match std::fs::symlink_metadata(&seat) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
        _ => return Err(DENIED.into()),
    }
    match std::fs::symlink_metadata(seat.join("TEAM")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(DENIED.into()),
    }
}
