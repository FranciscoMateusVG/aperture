//! Native installed layout shared by server, team-control and boot gate.
//! No checkout, PATH, environment override or legacy Sentry-path fallback.
//! Consumers still pin and recheck every leaf they use; this is path selection,
//! not immutable-release / atomic-exec attestation.
use std::path::{Component, Path, PathBuf};

pub(crate) const BUS: &str = "mcp-server/dist/index.js";
pub(crate) const HUB_CLIENT: &str = "mcp-server/dist/hub-client.js";
pub(crate) const HUB: &str = "mcp-server/dist/ws-hub.js";
pub(crate) const SENTRY: &str = "mcp-server-sentry/dist/src/index.js";
pub(crate) const BOOT: &str = "bin/aperture-boot";
const ERROR: &str = "E_LOCAL_PACKAGE";

pub(crate) fn for_process() -> Result<PathBuf, &'static str> {
    root_for_executable(&std::env::current_exe().map_err(|_| ERROR)?)
}

pub(crate) fn root_for_executable(executable: &Path) -> Result<PathBuf, &'static str> {
    if !executable.is_absolute()
        || executable.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || !matches!(executable.file_name().and_then(|n| n.to_str()),
            Some("aperture-server" | "aperture-team-control" | "aperture-boot"))
    { return Err(ERROR); }
    let bin = executable.parent().ok_or(ERROR)?;
    if bin.file_name().and_then(|n| n.to_str()) != Some("bin") { return Err(ERROR); }
    Ok(bin.parent().ok_or(ERROR)?.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_layout_is_identical_for_three_roles_and_canonical_helpers() {
        for root in ["/fixture/Applications/Aperture-Web-sha", "/fixture/.aperture"] {
            for role in ["aperture-server", "aperture-team-control", "aperture-boot"] {
                let package = root_for_executable(&Path::new(root).join("bin").join(role)).unwrap();
                assert_eq!(package, Path::new(root));
                assert_eq!(root_for_executable(&package.join(BOOT)).unwrap(), package);
                assert_eq!(package.join(SENTRY), Path::new(root).join("mcp-server-sentry/dist/src/index.js"));
                assert_eq!(package.join(HUB_CLIENT), Path::new(root).join("mcp-server/dist/hub-client.js"));
            }
        }
        for invalid in ["relative/bin/aperture-boot", "/fixture/target/release/aperture-boot",
            "/fixture/bin/other", "/fixture/../bin/aperture-boot", "/fixture/aperture-boot"] {
            assert!(root_for_executable(Path::new(invalid)).is_err());
        }
    }
}
