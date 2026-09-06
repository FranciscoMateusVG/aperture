//! Pure command-builders for the per-agent launcher scripts and configs.
//!
//! Extracted from `agents.rs::start_agent` (aperture-xt16e) as a
//! behavior-preserving refactor: with default parameters (bin = "claude" /
//! "codex", no PATH prefix) the output of every builder is byte-identical to
//! the templates that previously lived inline in `start_agent`. No I/O, no
//! env reads, no tauri types here — the side-effecting layer (`agents.rs`)
//! resolves env knobs (APERTURE_CLAUDE_BIN, APERTURE_CODEX_BIN,
//! APERTURE_LAUNCHER_PATH_PREFIX) and passes plain values in.

/// Static first-turn kickoff (comms v2, aperture-syepg). Appended as a
/// positional argv to the `claude` launch on FRESH sessions so turn 1 runs at
/// spawn — the agent starts its inbox Monitor + hub hello with zero manual
/// keystrokes. Passed as argv DATA via a single shell-quoted token, never
/// interpolated with user/BEADS content (Cipher constraint).
///
/// MUST stay byte-identical to the Codex-side constant `KICKOFF_TEXT` in
/// `mcp-server/src/codex-bridge.ts` (aperture-17amw). Source of truth: the
/// aperture-syepg bead. If you edit this, edit both halves.
pub const KICKOFF_TEXT: &str = "Session start. Run your boot routine now: start your inbox monitor per your system prompt, then check get_messages and process any unread messages, marking each read after you handle it.";

/// Wrap a string as a single POSIX-shell-quoted token: literal, no expansion.
/// Embedded single quotes are escaped via the `'\''` idiom so the launcher
/// construction stays safe if `KICKOFF_TEXT` ever changes (Izzy L1 quoting
/// torture, aperture-xt16e). Today's static text has no quotes/dollars/newlines.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the bash launcher script for a Claude agent pane.
///
/// * `claude_bin` — binary to exec; the default call site passes "claude"
///   unless APERTURE_CLAUDE_BIN overrides it.
/// * `path_prefix` — optional directory list prepended to the exported PATH
///   (before /opt/homebrew/bin); lets tests shadow binaries. `None` keeps the
///   historical PATH line byte-identical.
/// * `fresh_session` — reserved for the kickoff-prompt work (aperture-syepg):
///   fresh sessions will append a static positional kickoff argument after
///   `--name`; resume/continue sessions will not. Until that lands the flag
///   has no effect on output; current call sites pass `true`.
pub fn build_claude_launcher(
    claude_bin: &str,
    path_prefix: Option<&str>,
    project_dir: &str,
    prompt_path: &str,
    model: &str,
    mcp_config_path: &str,
    name: &str,
    fresh_session: bool,
) -> String {
    let path_line = match path_prefix {
        Some(prefix) => format!(
            r#"export PATH="{}:/opt/homebrew/bin:/usr/local/bin:$PATH""#,
            prefix
        ),
        None => r#"export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH""#.to_string(),
    };
    // aperture-syepg: a FRESH session appends the static kickoff as a single
    // shell-quoted positional AFTER --name (alongside --system-prompt), so turn
    // 1 fires at spawn. Empirically verified 2026-07-19: a positional prompt
    // here keeps the TUI interactive (does NOT imply print-then-exit), in both
    // trusted and untrusted dirs. Resume/continue sessions pass false and omit
    // it (they already have a conversation; re-kicking would double-fire).
    let kickoff = if fresh_session {
        format!(" {}", shell_single_quote(KICKOFF_TEXT))
    } else {
        String::new()
    };
    format!(
        r#"#!/bin/bash
{path_line}
export APERTURE_HUB_TOKEN_FILE="${{APERTURE_HUB_TOKEN_DIR:-$HOME/.aperture/run/hub-tokens}}/{name}.token"
export APERTURE_PROJECT_DIR="{project_dir}"
cd "{project_dir}"
PROMPT=$(cat "{prompt_path}")
exec {claude_bin} --dangerously-skip-permissions --model {model} --system-prompt "$PROMPT" --mcp-config {mcp_config_path} --name {name}{kickoff}
"#
    )
}

/// Build the bash launcher script for a Codex agent pane (the interactive TUI
/// that attaches to the supervised app-server socket).
///
/// * `codex_bin` — binary to exec; default call site passes "codex" unless
///   APERTURE_CODEX_BIN overrides it.
/// * `path_prefix` — same semantics as in [`build_claude_launcher`].
pub fn build_codex_launcher(
    codex_bin: &str,
    path_prefix: Option<&str>,
    codex_home: &str,
    sock_path: &str,
) -> String {
    let path_line = match path_prefix {
        Some(prefix) => format!(
            r#"export PATH="{}:/opt/homebrew/bin:/usr/local/bin:$HOME/.npm-global/bin:$PATH""#,
            prefix
        ),
        None => r#"export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.npm-global/bin:$PATH""#
            .to_string(),
    };
    // aperture-syepg/17amw: the exact-thread-id handoff file lives beside the
    // socket (<run>/<name>.sock → <run>/<name>.thread-id). The codex-bridge
    // (Rex, PR #34) atomically writes it mode-0600 after it binds the
    // kickoff thread; we derive the path here so the pure builder stays a
    // function of its 4 args (no new param — the test contract leaves the file
    // path unpinned).
    let thread_id_file = sock_path
        .strip_suffix(".sock")
        .map(|base| format!("{base}.thread-id"))
        .unwrap_or_else(|| format!("{sock_path}.thread-id"));
    format!(
        r#"#!/bin/bash
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
{path_line}
export CODEX_HOME="{codex_home}"
# Wait for the supervised app-server (codex_appserver.rs) to bring the
# socket up before attaching — its spawn thread runs concurrently.
for _ in $(seq 1 40); do [ -S "{sock_path}" ] && break; sleep 0.25; done
# aperture-syepg/17amw: bare `codex --remote` opens a NEW EMPTY TUI that never
# attaches to the bridge-created kickoff thread (verified in isolated
# CODEX_HOME). Wait for the codex-bridge to publish the thread id (exact-id
# handoff, up to 60s), then RESUME that thread so turn 1 (the kickoff) is live
# in the pane.
THREAD_ID_FILE="{thread_id_file}"
for _ in $(seq 1 240); do [ -f "$THREAD_ID_FILE" ] && [ ! -L "$THREAD_ID_FILE" ] && [ -s "$THREAD_ID_FILE" ] && break; sleep 0.25; done
THREAD_ID=$(cat "$THREAD_ID_FILE" 2>/dev/null)
# aperture-3x136 + aperture-278a4: never exec `codex resume ""` — a missing or
# malformed thread id means the codex-bridge (inside ws-hub.js) never published
# a valid handoff. Reject empty AND any non-[A-Za-z0-9-] content, failing loud
# in the pane instead of dying on a cryptic resume error or a poisoned id.
case "$THREAD_ID" in
  ''|*[!A-Za-z0-9-]*) echo "invalid thread handoff" >&2; exit 1 ;;
esac
exec {codex_bin} resume "$THREAD_ID" --remote "unix://{sock_path}"
"#
    )
}

/// Inputs for the per-agent Codex `config.toml` (written to
/// `$CODEX_HOME/config.toml`). All values are interpolated verbatim — see the
/// KNOWN-BROKEN(quoting) tests below for the current escaping gaps.
pub struct CodexConfigParams<'a> {
    /// Model id with the `codex/` prefix stripped (top-level `model = ...`).
    pub bare_model: &'a str,
    /// Path the agent prompt was copied to (`$CODEX_HOME/prompt.md`).
    pub prompt_dest: &'a str,
    pub project_dir: &'a str,
    /// `<project_dir>/mcp-server/start.sh` — aperture-bus launcher.
    pub bus_start_sh: &'a str,
    pub mcp_sentry_server_path: &'a str,
    pub name: &'a str,
    pub role: &'a str,
    /// Full model string as configured (keeps the `codex/` prefix).
    pub model: &'a str,
    pub mailbox_dir: &'a str,
    pub beads_dir: &'a str,
    pub beads_dolt_password: &'a str,
    pub home_dir: &'a str,
    pub hub_token_path: &'a str,
}

/// Build the Codex `config.toml` contents. Byte-identical to the template
/// formerly inline in `agents.rs::start_agent`.
pub fn build_codex_config_toml(p: &CodexConfigParams) -> String {
    format!(
        r#"model = "{bare_model}"
model_reasoning_effort = "high"
model_instructions_file = "{prompt_dest}"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[projects."{project_dir}"]
trust_level = "trusted"

[mcp_servers.aperture-bus]
command = "sh"
args = ["{bus_start_sh}"]
env = {{ AGENT_NAME = "{name}", AGENT_ROLE = "{role}", AGENT_MODEL = "{model}", APERTURE_MAILBOX = "{mailbox_dir}", BEADS_DIR = "{beads_dir}", BD_ACTOR = "{name}", BEADS_DOLT_PASSWORD = "{beads_dolt_password}", APERTURE_HUB_TOKEN_FILE = "{hub_token_path}" }}

# Sentry MCP wrap layer — enforces Cipher's 9 constraints (aperture-ttzz).
[mcp_servers.sentry]
command = "node"
args = ["{mcp_sentry_server_path}"]
env = {{ AGENT_NAME = "{name}", AGENT_ROLE = "{role}", AGENT_MODEL = "{model}", HOME = "{home_dir}", BEADS_DIR = "{beads_dir}", BD_ACTOR = "{name}" }}
"#,
        bare_model = p.bare_model,
        beads_dolt_password = p.beads_dolt_password,
        prompt_dest = p.prompt_dest,
        project_dir = p.project_dir,
        bus_start_sh = p.bus_start_sh,
        mcp_sentry_server_path = p.mcp_sentry_server_path,
        name = p.name,
        role = p.role,
        model = p.model,
        mailbox_dir = p.mailbox_dir,
        beads_dir = p.beads_dir,
        home_dir = p.home_dir,
        hub_token_path = p.hub_token_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    /// Fresh per-test scratch directory under the OS temp dir.
    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aperture-launcher-test-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write an executable stub binary that prints its cwd on line 1 and then
    /// each argv element on its own line. Used in place of the real `claude`
    /// so tests can observe exactly what the launcher script executes.
    fn make_stub(dir: &Path, file_name: &str) -> String {
        let stub = dir.join(file_name);
        fs::write(
            &stub,
            "#!/bin/bash\npwd\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n",
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        stub.to_str().unwrap().to_string()
    }

    /// Run a launcher script through bash; returns (cwd line, argv lines) as
    /// printed by the stub. If the script blows up before exec'ing the stub,
    /// cwd comes back empty / wrong — which is exactly what the quoting
    /// torture tests assert on.
    fn run_launcher(dir: &Path, launcher: &str) -> (String, Vec<String>) {
        let script = dir.join("launcher.sh");
        fs::write(&script, launcher).unwrap();
        let out = Command::new("bash").arg(&script).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let mut lines = stdout.lines().map(|s| s.to_string());
        let cwd = lines.next().unwrap_or_default();
        (cwd, lines.collect())
    }

    /// Value following `flag` in an argv list.
    fn argv_value(argv: &[String], flag: &str) -> Option<String> {
        argv.iter()
            .position(|a| a == flag)
            .and_then(|i| argv.get(i + 1).cloned())
    }

    /// Whatever trails `--name <name>` on the exec line (the kickoff slot).
    fn kickoff_suffix(script: &str, name: &str) -> String {
        let exec_line = script
            .lines()
            .find(|l| l.starts_with("exec "))
            .expect("launcher has an exec line");
        let marker = format!("--name {}", name);
        let idx = exec_line.find(&marker).expect("exec line has --name");
        exec_line[idx + marker.len()..].trim().to_string()
    }

    fn sample_codex_params<'a>() -> CodexConfigParams<'a> {
        CodexConfigParams {
            bare_model: "gpt-5.3-codex",
            prompt_dest: "/tmp/aperture-codex-vex/prompt.md",
            project_dir: "/Users/x/projects/aperture",
            bus_start_sh: "/Users/x/projects/aperture/mcp-server/start.sh",
            mcp_sentry_server_path: "/Users/x/projects/aperture/mcp-server-sentry/dist/index.js",
            name: "vex",
            role: "builder",
            model: "codex/gpt-5.3-codex",
            mailbox_dir: "/Users/x/.aperture/mailbox",
            beads_dir: "/Users/x/.aperture/.beads",
            beads_dolt_password: "hunter2",
            home_dir: "/Users/x",
            hub_token_path: "/Users/x/.aperture/run/hub-tokens/vex.token",
        }
    }

    // ------------------------------------------------------------------
    // Current-behavior pins (must pass now)
    // ------------------------------------------------------------------

    /// Byte-identity pin against the template that lived inline in agents.rs
    /// before the aperture-xt16e extraction. If this fails, the refactor is
    /// no longer behavior-preserving.
    #[test]
    fn claude_launcher_golden_default() {
        let s = build_claude_launcher(
            "claude",
            None,
            "/Users/x/projects/aperture",
            "/tmp/aperture-prompt-atlas.md",
            "opus",
            "/tmp/aperture-mcp-atlas.json",
            "atlas",
            true,
        );
        assert_eq!(
            s,
            r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
export APERTURE_HUB_TOKEN_FILE="${APERTURE_HUB_TOKEN_DIR:-$HOME/.aperture/run/hub-tokens}/atlas.token"
export APERTURE_PROJECT_DIR="/Users/x/projects/aperture"
cd "/Users/x/projects/aperture"
PROMPT=$(cat "/tmp/aperture-prompt-atlas.md")
exec claude --dangerously-skip-permissions --model opus --system-prompt "$PROMPT" --mcp-config /tmp/aperture-mcp-atlas.json --name atlas 'Session start. Run your boot routine now: start your inbox monitor per your system prompt, then check get_messages and process any unread messages, marking each read after you handle it.'
"#
        );
    }

    #[test]
    fn claude_launcher_contains_expected_flags() {
        let s = build_claude_launcher(
            "claude",
            None,
            "/proj",
            "/tmp/prompt.md",
            "sonnet",
            "/tmp/mcp.json",
            "borealis",
            true,
        );
        assert!(s.contains("--dangerously-skip-permissions"));
        assert!(s.contains("--model sonnet"));
        assert!(s.contains(r#"--system-prompt "$PROMPT""#));
        assert!(s.contains("--mcp-config /tmp/mcp.json"));
        assert!(s.contains("--name borealis"));
        assert!(s.contains(r#"cd "/proj""#));
        // aperture-3kavd P0: the hooks in .claude/settings.json resolve their scripts through
        // APERTURE_PROJECT_DIR, so the launch-selected runtime root must be exported to the
        // session (a worktree-built app must run worktree hooks, never a hard-coded checkout).
        assert!(s.contains(r#"export APERTURE_PROJECT_DIR="/proj""#));
        assert!(s.contains(r#"PROMPT=$(cat "/tmp/prompt.md")"#));
    }

    /// Byte-identity pin for the codex pane launcher.
    #[test]
    fn codex_launcher_golden_default() {
        let s = build_codex_launcher(
            "codex",
            None,
            "/tmp/aperture-codex-vex",
            "/Users/x/.aperture/run/vex.sock",
        );
        assert_eq!(
            s,
            r#"#!/bin/bash
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.npm-global/bin:$PATH"
export CODEX_HOME="/tmp/aperture-codex-vex"
# Wait for the supervised app-server (codex_appserver.rs) to bring the
# socket up before attaching — its spawn thread runs concurrently.
for _ in $(seq 1 40); do [ -S "/Users/x/.aperture/run/vex.sock" ] && break; sleep 0.25; done
# aperture-syepg/17amw: bare `codex --remote` opens a NEW EMPTY TUI that never
# attaches to the bridge-created kickoff thread (verified in isolated
# CODEX_HOME). Wait for the codex-bridge to publish the thread id (exact-id
# handoff, up to 60s), then RESUME that thread so turn 1 (the kickoff) is live
# in the pane.
THREAD_ID_FILE="/Users/x/.aperture/run/vex.thread-id"
for _ in $(seq 1 240); do [ -f "$THREAD_ID_FILE" ] && [ ! -L "$THREAD_ID_FILE" ] && [ -s "$THREAD_ID_FILE" ] && break; sleep 0.25; done
THREAD_ID=$(cat "$THREAD_ID_FILE" 2>/dev/null)
# aperture-3x136 + aperture-278a4: never exec `codex resume ""` — a missing or
# malformed thread id means the codex-bridge (inside ws-hub.js) never published
# a valid handoff. Reject empty AND any non-[A-Za-z0-9-] content, failing loud
# in the pane instead of dying on a cryptic resume error or a poisoned id.
case "$THREAD_ID" in
  ''|*[!A-Za-z0-9-]*) echo "invalid thread handoff" >&2; exit 1 ;;
esac
exec codex resume "$THREAD_ID" --remote "unix:///Users/x/.aperture/run/vex.sock"
"#
        );
    }

    /// aperture-3x136 + aperture-278a4: the pane must fail loud on an empty OR
    /// malformed thread id, never exec `codex resume ""` or a poisoned id.
    #[test]
    fn codex_launcher_guards_empty_thread_id() {
        let s = build_codex_launcher("codex", None, "/tmp/ch", "/run/a.sock");
        assert!(s.contains(r#"''|*[!A-Za-z0-9-]*) echo "invalid thread handoff" >&2; exit 1 ;;"#));
        // The guard must sit BEFORE the exec line.
        let guard = s.find("invalid thread handoff").unwrap();
        let exec = s.find("exec codex resume").unwrap();
        assert!(guard < exec);
    }

    #[test]
    fn codex_launcher_contains_codex_home_and_socket_wait() {
        let s = build_codex_launcher("codex", None, "/tmp/ch", "/run/a.sock");
        assert!(s.contains(r#"export CODEX_HOME="/tmp/ch""#));
        assert!(s.contains(r#"for _ in $(seq 1 40); do [ -S "/run/a.sock" ] && break; sleep 0.25; done"#));
        assert!(s.contains(r#"exec codex resume "$THREAD_ID" --remote "unix:///run/a.sock""#));
    }

    /// Byte-identity pin for the codex config.toml template.
    #[test]
    fn codex_config_toml_golden_default() {
        let s = build_codex_config_toml(&sample_codex_params());
        assert_eq!(
            s,
            r#"model = "gpt-5.3-codex"
model_reasoning_effort = "high"
model_instructions_file = "/tmp/aperture-codex-vex/prompt.md"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[projects."/Users/x/projects/aperture"]
trust_level = "trusted"

[mcp_servers.aperture-bus]
command = "sh"
args = ["/Users/x/projects/aperture/mcp-server/start.sh"]
env = { AGENT_NAME = "vex", AGENT_ROLE = "builder", AGENT_MODEL = "codex/gpt-5.3-codex", APERTURE_MAILBOX = "/Users/x/.aperture/mailbox", BEADS_DIR = "/Users/x/.aperture/.beads", BD_ACTOR = "vex", BEADS_DOLT_PASSWORD = "hunter2", APERTURE_HUB_TOKEN_FILE = "/Users/x/.aperture/run/hub-tokens/vex.token" }

# Sentry MCP wrap layer — enforces Cipher's 9 constraints (aperture-ttzz).
[mcp_servers.sentry]
command = "node"
args = ["/Users/x/projects/aperture/mcp-server-sentry/dist/index.js"]
env = { AGENT_NAME = "vex", AGENT_ROLE = "builder", AGENT_MODEL = "codex/gpt-5.3-codex", HOME = "/Users/x", BEADS_DIR = "/Users/x/.aperture/.beads", BD_ACTOR = "vex" }
"#
        );
    }

    #[test]
    fn codex_config_toml_round_trips_with_expected_fields() {
        let s = build_codex_config_toml(&sample_codex_params());
        let t: toml::Table = s.parse().expect("config.toml must be valid TOML");
        assert_eq!(t["model"].as_str(), Some("gpt-5.3-codex"));
        assert_eq!(t["model_reasoning_effort"].as_str(), Some("high"));
        assert_eq!(
            t["model_instructions_file"].as_str(),
            Some("/tmp/aperture-codex-vex/prompt.md")
        );
        assert_eq!(t["approval_policy"].as_str(), Some("never"));
        assert_eq!(t["sandbox_mode"].as_str(), Some("danger-full-access"));
        assert_eq!(
            t["projects"]["/Users/x/projects/aperture"]["trust_level"].as_str(),
            Some("trusted")
        );
        let bus = &t["mcp_servers"]["aperture-bus"];
        assert_eq!(bus["command"].as_str(), Some("sh"));
        assert_eq!(
            bus["args"][0].as_str(),
            Some("/Users/x/projects/aperture/mcp-server/start.sh")
        );
        assert_eq!(bus["env"]["AGENT_NAME"].as_str(), Some("vex"));
        assert_eq!(bus["env"]["BEADS_DOLT_PASSWORD"].as_str(), Some("hunter2"));
        let sentry = &t["mcp_servers"]["sentry"];
        assert_eq!(sentry["command"].as_str(), Some("node"));
        assert_eq!(sentry["env"]["BD_ACTOR"].as_str(), Some("vex"));
    }

    #[test]
    fn claude_bin_knob_substitutes() {
        let s = build_claude_launcher(
            "/tmp/fake/claude-stub",
            None,
            "/proj",
            "/p.md",
            "opus",
            "/m.json",
            "atlas",
            true,
        );
        assert!(s.contains("exec /tmp/fake/claude-stub --dangerously-skip-permissions"));
        assert!(!s.contains("exec claude "));
    }

    #[test]
    fn codex_bin_knob_substitutes() {
        let s = build_codex_launcher("/tmp/fake/codex-stub", None, "/tmp/ch", "/run/a.sock");
        assert!(s.contains(r#"exec /tmp/fake/codex-stub resume "$THREAD_ID" --remote "unix:///run/a.sock""#));
        assert!(!s.contains("exec codex "));
    }

    #[test]
    fn path_prefix_prepends_in_claude_launcher() {
        let s = build_claude_launcher(
            "claude",
            Some("/tmp/shadow/bin"),
            "/proj",
            "/p.md",
            "opus",
            "/m.json",
            "atlas",
            true,
        );
        assert!(
            s.contains(r#"export PATH="/tmp/shadow/bin:/opt/homebrew/bin:/usr/local/bin:$PATH""#)
        );
    }

    #[test]
    fn path_prefix_prepends_in_codex_launcher() {
        let s = build_codex_launcher("codex", Some("/tmp/shadow/bin"), "/tmp/ch", "/run/a.sock");
        assert!(s.contains(
            r#"export PATH="/tmp/shadow/bin:/opt/homebrew/bin:/usr/local/bin:$HOME/.npm-global/bin:$PATH""#
        ));
    }

    /// Functional baseline: with clean parameters the generated script cd's
    /// into the project dir and execs the binary with the argv intact. This
    /// proves the bash harness works, so failures in the quoting torture
    /// tests below mean the template is broken, not the harness.
    #[test]
    fn claude_launcher_execs_stub_with_intact_argv() {
        let dir = test_dir("baseline");
        let stub = make_stub(&dir, "claude-stub");
        let project_dir = dir.join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();

        let s = build_claude_launcher(
            &stub,
            None,
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            "opus",
            "/tmp/mcp.json",
            "atlas",
            true,
        );
        let (cwd, argv) = run_launcher(&dir, &s);
        assert_eq!(cwd, project_dir.to_str().unwrap());
        assert_eq!(argv_value(&argv, "--model").as_deref(), Some("opus"));
        assert_eq!(
            argv_value(&argv, "--system-prompt").as_deref(),
            Some("PROMPT_CONTENT")
        );
        assert_eq!(argv_value(&argv, "--name").as_deref(), Some("atlas"));
    }

    /// Functional: PATH-prefix knob lets a shadow dir win binary resolution
    /// for a bare (non-absolute) binary name.
    #[test]
    fn path_prefix_shadow_binary_wins() {
        let dir = test_dir("shadow");
        let shadow_bin = dir.join("shadowbin");
        fs::create_dir_all(&shadow_bin).unwrap();
        make_stub(&shadow_bin, "claude");
        let project_dir = dir.join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();

        let s = build_claude_launcher(
            "claude", // bare name — resolved via the exported PATH
            Some(shadow_bin.to_str().unwrap()),
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            "opus",
            "/tmp/mcp.json",
            "atlas",
            true,
        );
        let (cwd, argv) = run_launcher(&dir, &s);
        assert_eq!(cwd, project_dir.to_str().unwrap());
        assert_eq!(argv_value(&argv, "--name").as_deref(), Some("atlas"));
    }

    /// Pin: a semicolon smuggled into the model string does NOT execute a
    /// second command today — `exec` replaces the shell before bash reaches
    /// the text after the `;`. (The argv is still truncated; see the
    /// KNOWN-BROKEN companion test below.)
    #[test]
    fn model_semicolon_does_not_spawn_second_command() {
        let dir = test_dir("semicolon-pin");
        let stub = make_stub(&dir, "claude-stub");
        let project_dir = dir.join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();
        let marker = dir.join("pwned");

        let model = format!("opus; touch {}", marker.display());
        let s = build_claude_launcher(
            &stub,
            None,
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            &model,
            "/tmp/mcp.json",
            "atlas",
            true,
        );
        let _ = run_launcher(&dir, &s);
        assert!(
            !marker.exists(),
            "injected command after ';' must not run (exec should have replaced the shell)"
        );
    }

    // ------------------------------------------------------------------
    // Quoting torture cases — DESIRED behavior, currently broken.
    // The launcher templates interpolate with zero escaping; each test below
    // asserts what SHOULD happen and is ignored until the escaping work
    // lands. Verified to fail today (see aperture-xt16e).
    // ------------------------------------------------------------------

    // KNOWN-BROKEN(quoting): a double quote in project_dir terminates the
    // cd "..." string early and corrupts the rest of the script.
    #[test]
    #[ignore = "known-broken: unescaped interpolation, see aperture-xt16e"]
    fn quoting_project_dir_with_double_quote() {
        let dir = test_dir("quote-dir");
        let stub = make_stub(&dir, "claude-stub");
        let project_dir = dir.join(r#"we"ird"#);
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();

        let s = build_claude_launcher(
            &stub,
            None,
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            "opus",
            "/tmp/mcp.json",
            "atlas",
            true,
        );
        let (cwd, _argv) = run_launcher(&dir, &s);
        assert_eq!(
            cwd,
            project_dir.to_str().unwrap(),
            "script must cd into the literal directory even when its name contains a double quote"
        );
    }

    // KNOWN-BROKEN(quoting): `$HOME` inside project_dir is interpolated into
    // a double-quoted cd context, so bash EXPANDS it instead of treating it
    // as a literal path component.
    #[test]
    #[ignore = "known-broken: unescaped interpolation, see aperture-xt16e"]
    fn quoting_project_dir_with_dollar_home_literal() {
        let dir = test_dir("dollar-home");
        let stub = make_stub(&dir, "claude-stub");
        let project_dir = dir.join("$HOME").join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();

        let s = build_claude_launcher(
            &stub,
            None,
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            "opus",
            "/tmp/mcp.json",
            "atlas",
            true,
        );
        let (cwd, _argv) = run_launcher(&dir, &s);
        assert_eq!(
            cwd,
            project_dir.to_str().unwrap(),
            "a literal $HOME path component must NOT be expanded by the shell"
        );
    }

    // KNOWN-BROKEN(quoting): the agent name is interpolated unquoted after
    // --name, so a space splits it into two argv words.
    #[test]
    #[ignore = "known-broken: unescaped interpolation, see aperture-xt16e"]
    fn quoting_agent_name_with_space() {
        let dir = test_dir("name-space");
        let stub = make_stub(&dir, "claude-stub");
        let project_dir = dir.join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();

        let s = build_claude_launcher(
            &stub,
            None,
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            "opus",
            "/tmp/mcp.json",
            "space name",
            true,
        );
        let (_cwd, argv) = run_launcher(&dir, &s);
        assert_eq!(
            argv_value(&argv, "--name").as_deref(),
            Some("space name"),
            "an agent name containing a space must arrive as a single argv element"
        );
        assert_eq!(
            argv.last().map(String::as_str),
            Some("space name"),
            "no stray argv after the name"
        );
    }

    // KNOWN-BROKEN(quoting): the model is interpolated unquoted, so a
    // semicolon truncates the exec argv (everything after the ';' is parsed
    // as a second command — which never runs thanks to exec, but the flags
    // after --model are silently dropped).
    #[test]
    #[ignore = "known-broken: unescaped interpolation, see aperture-xt16e"]
    fn quoting_model_with_semicolon_preserves_full_argv() {
        let dir = test_dir("semicolon-argv");
        let stub = make_stub(&dir, "claude-stub");
        let project_dir = dir.join("proj");
        fs::create_dir_all(&project_dir).unwrap();
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "PROMPT_CONTENT").unwrap();

        let model = "opus; touch /tmp/should-not-matter";
        let s = build_claude_launcher(
            &stub,
            None,
            project_dir.to_str().unwrap(),
            prompt.to_str().unwrap(),
            model,
            "/tmp/mcp.json",
            "atlas",
            true,
        );
        let (_cwd, argv) = run_launcher(&dir, &s);
        assert_eq!(
            argv_value(&argv, "--model").as_deref(),
            Some(model),
            "the model string must arrive verbatim as one argv element"
        );
        assert!(
            argv.iter().any(|a| a == "--system-prompt"),
            "flags after --model must not be swallowed by the ';'"
        );
    }

    // KNOWN-BROKEN(quoting): a double quote in prompt_dest breaks the TOML
    // basic string and the whole config.toml fails to parse.
    #[test]
    #[ignore = "known-broken: unescaped interpolation, see aperture-xt16e"]
    fn quoting_toml_double_quote_in_prompt_dest() {
        let mut p = sample_codex_params();
        let prompt_dest = r#"/tmp/aperture-codex-vex/pro"mpt.md"#;
        p.prompt_dest = prompt_dest;
        let s = build_codex_config_toml(&p);
        let t: toml::Table = s
            .parse()
            .expect("config.toml must stay parseable when prompt_dest contains a double quote");
        assert_eq!(
            t["model_instructions_file"].as_str(),
            Some(prompt_dest),
            "the quote must round-trip intact"
        );
    }

    // KNOWN-BROKEN(quoting): a double quote in the BEADS dolt password
    // breaks the inline env table and the whole config.toml fails to parse.
    #[test]
    #[ignore = "known-broken: unescaped interpolation, see aperture-xt16e"]
    fn quoting_toml_double_quote_in_beads_password() {
        let mut p = sample_codex_params();
        let password = r#"hu"nter2"#;
        p.beads_dolt_password = password;
        let s = build_codex_config_toml(&p);
        let t: toml::Table = s
            .parse()
            .expect("config.toml must stay parseable when the password contains a double quote");
        assert_eq!(
            t["mcp_servers"]["aperture-bus"]["env"]["BEADS_DOLT_PASSWORD"].as_str(),
            Some(password),
            "the quote must round-trip intact"
        );
    }

    // ------------------------------------------------------------------
    // Kickoff-prompt contract (aperture-syepg) — documents what Peppy's
    // change must satisfy; his PR un-ignores these.
    // ------------------------------------------------------------------

    /// Fresh sessions append a STATIC kickoff token as a shell-SINGLE-quoted
    /// positional argument after --name (alongside --system-prompt): non-empty
    /// and identical regardless of agent/model/paths — zero interpolation of
    /// user or BEADS content. (Shape confirmed with Peppy's empirical test.)
    #[test]
    fn kickoff_fresh_session_appends_static_positional_prompt() {
        let a = build_claude_launcher(
            "claude", None, "/proj-a", "/tmp/pa.md", "opus", "/tmp/ma.json", "atlas", true,
        );
        let b = build_claude_launcher(
            "claude", None, "/proj-b", "/tmp/pb.md", "sonnet", "/tmp/mb.json", "borealis", true,
        );
        let ka = kickoff_suffix(&a, "atlas");
        let kb = kickoff_suffix(&b, "borealis");
        assert!(
            !ka.is_empty(),
            "fresh session must append a positional kickoff argument after --name"
        );
        assert!(
            ka.starts_with('\'') && ka.ends_with('\'') && ka.len() >= 3,
            "kickoff must be a shell-single-quoted static token, got: {}",
            ka
        );
        assert_eq!(
            ka, kb,
            "kickoff text must be static — identical across agents/models/paths"
        );
        assert!(
            a.contains(r#"--system-prompt "$PROMPT""#),
            "--system-prompt stays alongside the kickoff positional"
        );
    }

    /// Resume/continue sessions (fresh_session = false) must NOT append the
    /// kickoff positional argument.
    #[test]
    fn kickoff_resume_session_omits_positional_prompt() {
        let s = build_claude_launcher(
            "claude", None, "/proj", "/tmp/p.md", "opus", "/tmp/m.json", "atlas", false,
        );
        assert_eq!(
            kickoff_suffix(&s, "atlas"),
            "",
            "resume sessions must end the exec line at --name <name>"
        );
    }

    // ------------------------------------------------------------------
    // Codex pane kickoff contract (aperture-syepg/17amw) — Peppy+Rex are
    // pivoting the pane from `exec codex --remote unix://sock` (which opens
    // an empty TUI that won't attach to bridge threads) to
    // `codex resume <thread-id> --remote unix://sock` behind a thread-id
    // wait gate. The thread-id-file mechanism is NOT locked yet, so these
    // assertions stay LOOSE (no pin on the thread-id file path). The
    // un-ignored `codex_launcher_golden_default` pin above documents today's
    // behavior; the PR that changes it must update that pin in the same
    // diff (visible-contract-change discipline).
    // ------------------------------------------------------------------

    #[test]
    fn codex_pane_resumes_bridge_thread_behind_wait_gate() {
        let s = build_codex_launcher("codex", None, "/tmp/ch", "/run/a.sock");
        assert!(
            s.contains("codex resume"),
            "pane must resume the bridge thread, not open a bare TUI"
        );
        assert!(
            s.contains(r#"--remote "unix://"#),
            "pane must still attach over the app-server unix socket"
        );
        let exec_idx = s.find("\nexec ").expect("launcher has an exec line");
        let before_exec = &s[..exec_idx];
        assert!(
            before_exec.contains("for ") || before_exec.contains("until "),
            "a wait/poll gate must run before exec (socket and/or thread-id readiness)"
        );
    }
}
