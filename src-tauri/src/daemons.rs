//! Shared startup under an already-held controller lease. F2 replaces legacy supervision.
use crate::{controller::ControllerLock, state::AppState};
use std::sync::{Arc, Mutex};
pub(crate) fn start(_lease: &ControllerLock, state: Arc<Mutex<AppState>>) -> Result<(), String> {
    crate::hub_auth::provision_token("watchdog")?;
    let project = state
        .lock()
        .map_err(|_| "application state unavailable")?
        .project_dir
        .clone();
    let poller = Arc::clone(&state);
    std::thread::spawn(move || crate::poller::run_message_poller(poller));
    crate::ws_hub::spawn_ws_hub(project);
    crate::watchdog::spawn_watchdog(state);
    Ok(())
}
