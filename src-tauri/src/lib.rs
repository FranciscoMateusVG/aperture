mod agents;
mod beads;
mod beads_parser;
mod codex_harness;
mod config;
mod objectives;
mod poller;
mod pty;
mod spawner;
mod state;
mod tmux;
mod warroom;

use pty::PtyState;
use std::sync::{Arc, Mutex};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(Mutex::new(config::default_state()));
    let pty_state = Mutex::new(PtyState {
        writer: None,
        master: None,
    });

    // Initialize BEADS database
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let beads_dir = format!("{}/.aperture/.beads", home);
    let current_path = std::env::var("PATH").unwrap_or_default();
    let go_bin = format!("{}/go/bin", home);
    let path_env = format!("/opt/homebrew/bin:/usr/local/bin:{}:{}", go_bin, current_path);
    let bd_bin = format!("{}/go/bin/bd", home);

    // Ensure dolt is initialized in .beads dir
    if !std::path::Path::new(&format!("{}/config.json", beads_dir)).exists() {
        let _ = std::fs::create_dir_all(&beads_dir);
        let _ = std::process::Command::new("dolt")
            .arg("init")
            .current_dir(&beads_dir)
            .env("PATH", &path_env)
            .output();
    }

    // Initialize BEADS if not yet done
    // NOTE: dolt server lifecycle is owned by `bd dolt start` — Tauri no longer
    // spawns its own dolt sql-server on port 3307. This was removed to avoid
    // orphaned processes and conflicts with bd's managed server mode.
    {
        let mut cmd = std::process::Command::new(&bd_bin);
        cmd.args(["init", "--quiet"]);
        cmd.env("BEADS_DIR", &beads_dir);
        cmd.env("PATH", &path_env);
        cmd.current_dir(&app_state.lock().unwrap().project_dir);
        match cmd.output() {
            Ok(output) if output.status.success() => {
                println!("BEADS ready at {}", beads_dir);
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("already initialized") {
                    eprintln!("BEADS init warning: {}", stderr);
                }
            }
            Err(e) => {
                eprintln!("BEADS init failed (bd not found?): {}", e);
            }
        }
    }

    // Start background message delivery poller
    let poller_state = Arc::clone(&app_state);
    std::thread::spawn(move || {
        poller::run_message_poller(poller_state);
    });

    tauri::Builder::default()
        .manage(app_state)
        .manage(pty_state)
        .invoke_handler(tauri::generate_handler![
            tmux::tmux_create_session,
            tmux::tmux_list_windows,
            tmux::tmux_create_window,
            tmux::tmux_kill_window,
            tmux::tmux_select_window,
            tmux::tmux_rename_window,
            tmux::tmux_send_keys,
            pty::start_pty,
            pty::write_pty,
            pty::resize_pty,
            agents::start_agent,
            agents::stop_agent,
            agents::list_agents,
            agents::update_agent_model,
            agents::get_recent_messages,
            agents::clear_message_history,
            agents::clear_conversation_history,
            agents::send_chat,
            agents::get_chat_messages,
            agents::clear_chat_history,
            warroom::create_warroom,
            warroom::get_warroom_state,
            warroom::get_warroom_transcript,
            warroom::warroom_interject,
            warroom::warroom_skip,
            warroom::warroom_conclude,
            warroom::warroom_cancel,
            warroom::warroom_invite_participant,
            warroom::list_warroom_history,
            warroom::get_warroom_history_transcript,
            spawner::list_spiderlings,
            spawner::kill_spiderling_cmd,
            beads::list_beads_tasks,
            beads::update_beads_task_status,
            objectives::list_objectives,
            objectives::create_objective,
            objectives::update_objective,
            objectives::delete_objective,
            objectives::open_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
