# Codex BEADS Bridge — Wave 0 Audit Findings

> **Task:** src-tauri-cah  
> **Auditor:** peppy  
> **Date:** 2026-04-13  
> **Status:** Complete

---

## Summary

Full audit of `src-tauri/src/agents.rs`, `poller.rs`, `spawner.rs`, and `tmux.rs`. Three questions answered. One requires runtime confirmation. No ambiguity about where the harness hooks in.

---

## Q1: Does Codex load MCP tools from config.toml?

**Status: Unconfirmed — requires runtime test.**

The `config.toml` IS correctly generated with `[mcp_servers.aperture-bus]` (agents.rs lines 85–88):

```toml
[mcp_servers.aperture-bus]
command = "node"
args = ["/path/to/mcp-server.js"]
env = { AGENT_NAME = "glados", AGENT_ROLE = "...", BEADS_DIR = "...", BD_ACTOR = "glados" }
```

The env var `CODEX_HOME` is set before launch, and Codex's config lookup is from `$CODEX_HOME/config.toml`. The plumbing is correct.

**What code cannot confirm:** Whether `codex --yolo` actually instantiates MCP tools from this config and makes them callable inside the agent session. This is a runtime question — run `codex --yolo` with the config and verify `aperture-bus` tools appear in the tool list.

**Implication for harness design:**
- If MCP works → Codex can call `send_message`, `update_task` etc. natively; harness only needs to solve inbound delivery
- If MCP broken → Harness must also proxy outbound BEADS commands; `@@BEADS@@` block execution is mandatory

---

## Q2: Where does Tauri intercept Codex API responses?

**Finding: It doesn't. There is no intercept point.**

The Codex launch path (agents.rs lines 61–136):

1. Tauri builds `/tmp/aperture-launch-{name}.sh` containing:
   ```bash
   export CODEX_HOME="/tmp/aperture-codex-{name}"
   exec codex --yolo
   ```
2. Script is written to disk, chmod +x'd, then **sent via `tmux_send_keys`** (line 136)
3. Codex runs as a fully autonomous process inside the tmux window

**There is NO:**
- stdout/stderr pipe back to Tauri
- subprocess handle or PID reference stored anywhere
- IPC mechanism between Tauri and the running Codex process
- callback, event hook, or response parsing
- any Codex-specific branch in `run_message_poller()`

The only observation capability is `tmux_capture_pane()` (tmux.rs lines 156–162), which reads raw terminal output. Currently used only to detect the Claude workspace trust dialog (agents.rs lines 148–161) — not used for response interception.

**Critical: Poller treats all agents the same.**  
`run_message_poller()` (poller.rs lines 297–353) has no Codex-specific branch. It applies the same delivery mechanism to all running agents:
```rust
let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
```
This sends a shell command to the tmux pane. After `exec codex --yolo`, the shell is replaced by the codex process. Keystrokes sent to the pane go into Codex's interactive interface — not a shell. Whether Codex incorporates these as context is unconfirmed (probably not, or at best unreliably).

---

## Q3: Correct harness hook point

**Recommendation confirmed: Option A — new `src-tauri/src/codex_harness.rs` module.**

### Hook Point 1: Pre-prompt injection (startup)

**Location:** `start_agent()` in agents.rs, Codex branch, lines 70–74.

```rust
// Current:
let prompt_content = fs::read_to_string(&agent.prompt_file)?;
let prompt_content = inject_skills(prompt_content, &project_dir);
let prompt_dest = format!("{}/prompt.md", codex_home);
fs::write(&prompt_dest, &prompt_content)?;
```

**Harness addition:** After `inject_skills()`, call `codex_harness::inject_pending_messages(agent_name, prompt_content)` which queries BEADS for unread messages and prepends them to the prompt before writing `prompt.md`. This gives Codex its message context at boot.

**Limitation:** Only runs at startup. Does not handle messages arriving mid-session.

### Hook Point 2: Per-turn inbound delivery

**Location:** `run_message_poller()` in poller.rs — needs a Codex-specific branch.

Current code (lines 297–353) delivers messages the same way for all agents. For Codex agents, add a branch:

```rust
// Detect Codex agent
let is_codex = { state.lock()?.agents.get(agent_name)?.model.starts_with("codex/") };

if is_codex {
    // Route to harness instead of tmux injection
    codex_harness::buffer_message(agent_name, &formatted);
    // Harness appends to a pending-msgs file; Codex picks it up next turn
} else {
    // existing tmux injection
    let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
    let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
}
```

Pending messages would be appended to `/tmp/aperture-codex-{name}/pending-msgs.md`. The Codex system prompt instructs the agent to `cat` this file at the start of each turn and act on its contents.

### Hook Point 3: Post-response `@@BEADS@@` scraping

**Location:** New background thread spawned per Codex agent in `start_agent()`.

No existing hook point — must be created. Pattern:

```rust
// After launching Codex (agents.rs, after line 136):
if agent.model.starts_with("codex/") {
    let window_id_clone = window_id.clone();
    let agent_name_clone = name.clone();
    std::thread::spawn(move || {
        codex_harness::monitor_output(window_id_clone, agent_name_clone);
    });
}
```

`monitor_output()` in `codex_harness.rs`:
1. Polls `tmux_capture_pane()` every 2–3 seconds
2. Diffs against last-seen output to find new lines
3. Scans new lines for `@@BEADS ... @@` blocks
4. Parses and executes corresponding BEADS MCP calls
5. Deduplicates by tracking already-executed block hashes

---

## File Map

| File | Function | Line(s) | Role |
|------|----------|---------|------|
| `src-tauri/src/agents.rs` | `start_agent()` | 61–111 | Codex prompt assembly + launch |
| `src-tauri/src/agents.rs` | `inject_skills()` | 416–443 | System prompt injection point |
| `src-tauri/src/poller.rs` | `run_message_poller()` | 297–353 | Message delivery — NO Codex branch |
| `src-tauri/src/tmux.rs` | `tmux_capture_pane()` | 156–162 | Only observation mechanism |
| `src-tauri/src/tmux.rs` | `tmux_send_keys()` | 165–186 | Current delivery mechanism (all agents) |
| `src-tauri/src/spawner.rs` | `spawn_spiderling()` | 46–242 | Spiderlings — Claude only, no Codex path |

**Note:** Spiderlings (`spawner.rs`) are Claude-only — no Codex path exists there. Harness only needed in `agents.rs` / permanent agent lifecycle.

---

## Architecture Decision

**Tauri has zero interception of Codex API responses.** The only path to `@@BEADS@@` parsing is tmux output scraping via `tmux_capture_pane`. This is the implementation path regardless of MCP status.

**If MCP works:** Codex can self-report via MCP tools. The `@@BEADS@@` scraper becomes a fallback/safety net, not the primary channel.

**If MCP broken:** `@@BEADS@@` scraper is the only outbound channel. It must be reliable.

Either way: build the scraper. Determine MCP status via runtime test. Document the result in src-tauri-ae5.

---

## Open Items for Wave 1

1. **Runtime MCP test** — Boot a Codex agent, check if `aperture-bus` tools appear. Report in src-tauri-ae5 notes.
2. **`codex_harness.rs` module** — New file. Three functions: `inject_pending_messages()`, `buffer_message()`, `monitor_output()`.
3. **Poller Codex branch** — Modify `run_message_poller()` to route Codex agents to harness buffer instead of tmux injection.
4. **System prompt update** — Codex agents need `@@BEADS@@` syntax instructions + `cat pending-msgs.md` directive (task src-tauri-s7b, owner: glados).
