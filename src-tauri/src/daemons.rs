//! Shared startup under an already-held controller lease.
//! F2-A is intentionally NOT launchable: even a pristine registry cannot prove
//! a free endpoint, and passing a port probe to the legacy supervisor would race
//! its kill-by-port path. B must replace that supervisor before removing this
//! unconditional fence. There is no environment/flag/test bypass.
use crate::{controller::ControllerLock, state::AppState};
use std::sync::{Arc, Mutex};
pub(crate) fn start(lease: &ControllerLock, state: Arc<Mutex<AppState>>) -> Result<(), String> {
    start_checked(lease, || {
        crate::hub_auth::provision_token("watchdog")?;
        let project = state
            .lock()
            .map_err(|_| "application state unavailable")?
            .project_dir
            .clone();
        let poller = Arc::clone(&state);
        std::thread::spawn(move || crate::poller::run_message_poller(poller));
        crate::ws_hub::spawn_ws_hub(lease, project)?;
        crate::watchdog::spawn_watchdog(state);
        Ok(())
    })
}

// The actual production composition and inert counted-effects tests pass
// through precisely this preflight, not a separate mock registry algorithm.
pub(crate) fn start_checked(
    lease: &ControllerLock,
    _downstream: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let registry = crate::daemon_registry::Registry::open(lease)?;
    let _unverified_metadata = registry.inspect()?;
    Err("E_DAEMON_SUPERVISION_PENDING: F2-A startup is fenced until safe supervision is implemented and reviewed".into())
}
