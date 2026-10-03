//! Claude-only managed inbox recipe (aperture-337gb, operator exception A).
//!
//! The publisher appends this constant text to the rendered role prompt of a
//! managed Claude seat. Its normal positional boot prompt may run while native
//! model observation is still Starting; the client waits locally for Active.
//! The text itself
//! starts no turn, sends no keys, reads no token and adds no channel. It only
//! tells the seat how to use tooling that already exists: the native Claude
//! Code `Monitor` tool, the existing `hub-client.js`, and the BEADS inbox tools
//! on aperture-bus.
//!
//! The absolute client path reaches the process only through the launcher
//! environment variable named by [`MANAGED_HUB_CLIENT_ENV`]; the bearer reaches
//! the client only through the existing launcher token pointer, which this
//! text never names and never asks the seat to read.

/// Environment variable holding the absolute path of `hub-client.js` for a
/// managed Claude seat. Pinned by the launcher/publisher, never by the seat.
pub(crate) const MANAGED_HUB_CLIENT_ENV: &str = "APERTURE_MANAGED_HUB_CLIENT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboxRecipeError {
    /// The seat is not a canonical seat name (`agent_loader::is_valid_seat_name`).
    InvalidSeat,
}

/// Exact Monitor command line for `seat`. The seat is the only variable part;
/// the env reference is written without braces because the rendered prompt
/// must not contain `${`.
fn monitor_command(seat: &str) -> String {
    format!("node \"${env}\" {seat}", env = MANAGED_HUB_CLIENT_ENV)
}

/// Build the inbox recipe for one managed Claude seat. Deterministic, bounded,
/// no secrets, no other harness or channel. Errors only on an invalid seat.
pub(crate) fn managed_inbox_recipe(seat: &str) -> Result<String, InboxRecipeError> {
    if !crate::agent_loader::is_valid_seat_name(seat) {
        return Err(InboxRecipeError::InvalidSeat);
    }
    let command = monitor_command(seat);
    Ok(format!(
        "\n# Managed inbox (Claude seat {seat})\n\n\
Your only inbox is BEADS through aperture-bus, pushed live by the hub. Use exactly the tooling below; all of it already exists.\n\n\
1. Monitor first. Before any inbox call, start the inbox monitor with the native Claude Code Monitor tool as a finite bash command, exactly:\n\
   Monitor(command: {command}, timeout_ms: 1800000)\n\
   The current tool has no persistent parameter and expires after at most 30 minutes. Do not add unsupported fields or assume a session-length watch.\n\
   The client path comes from that environment variable and the client reads its own credential from the launcher environment. Never look up, type, print, store or pass a credential. Do not use the Monitor ws source: it cannot send the hello and the hub would treat you as offline.\n\
2. If the monitor reports HUB_OWNER_PENDING, wait for HUB_OWNER_ACTIVE before any inbox call; do not restart it or do project work. This is native activation in progress. HUB_OWNER_ACTIVE confirms only local activation, not hub delivery or working tools. Then drain the inbox: call get_messages, process each message, and call mark_as_read for a message only after you have actually handled it.\n\
3. Every Monitor event whose type is \"message\" means a BEADS message is waiting: get_messages, process, mark_as_read. Do not poll on a timer instead.\n\
4. A HUB_RECONNECTING line means wait; the client reconnects by itself. HUB_RECONNECTED means unread messages are replaying now.\n\
   Tool expiry is different: the Monitor kills its command. Only after an explicit Monitor expiry and confirmed end of that monitor, with no rejection/replacement from rule 5, re-arm exactly one Monitor using the same command and timeout. Never create a second monitor while the first is alive or its outcome is unknown. After reconnection, drain and acknowledge the inbox normally; do not replay project effects. If an API outage prevents rearming, communication is degraded, not recovered; report that limit once tools work again.\n\
5. Do NOT restart the monitor after HUB_IDENTITY_INVALID or HUB_OWNER_TIMEOUT, or after HUB_SOCKET_CLOSED with code 4000 (a newer monitor replaced this one), 4001 (hello rejected) or 4003 (managed identity rejected). Keep the exact line; if the aperture-bus update_task tool is callable, record it on your assigned bead, then wait for dispatch.\n\
6. If the Monitor tool is unavailable, or the command exits at once (for example a HUB_CLIENT_ERROR line), that is an honest blocker: record it on your assigned bead with update_task only if that tool is callable, then stop. If aperture-bus itself is absent there is no way to send anything: stay idle and say so in your terminal; never invent another channel: no background Bash loops, no file watching, no tmux, no hooks, no other harness.\n\
7. Do no project work before a scoped dispatch arrives through this inbox. Boot order is fixed: monitor, then get_messages, then only the work that was dispatched.\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEAT: &str = "t1-worker";

    fn recipe() -> String {
        managed_inbox_recipe(SEAT).unwrap()
    }

    #[test]
    fn pins_exact_monitor_command_with_seat_and_env_pointer() {
        let r = recipe();
        assert!(r.contains(
            "Monitor(command: node \"$APERTURE_MANAGED_HUB_CLIENT\" t1-worker, timeout_ms: 1800000)"
        ));
        assert_eq!(
            monitor_command(SEAT),
            "node \"$APERTURE_MANAGED_HUB_CLIENT\" t1-worker"
        );
        assert_eq!(MANAGED_HUB_CLIENT_ENV, "APERTURE_MANAGED_HUB_CLIENT");
        assert_eq!(r.matches("Monitor(command:").count(), 1);
        assert!(r.contains("# Managed inbox (Claude seat t1-worker)"));
        assert_eq!(recipe(), r, "deterministic");
    }

    #[test]
    fn finite_monitor_expiry_is_not_socket_reconnection_or_recovery() {
        let r = recipe();
        assert!(!r.contains("persistent: true"));
        assert!(r.contains("current tool has no persistent parameter"));
        assert!(r.contains("expires after at most 30 minutes"));
        assert!(r.contains("Only after an explicit Monitor expiry and confirmed end"));
        assert!(r.contains("with no rejection/replacement from rule 5"));
        assert!(r.contains("Never create a second monitor while the first is alive or its outcome is unknown"));
        assert!(r.contains("do not replay project effects"));
        assert!(r.contains("communication is degraded, not recovered"));
    }

    #[test]
    fn shipped_inbox_instructions_match_the_finite_monitor_schema() {
        let sources = [
            ("glados", include_str!("../../prompts/glados.md")),
            ("wheatley", include_str!("../../prompts/wheatley.md")),
            ("peppy", include_str!("../../prompts/peppy.md")),
            ("izzy", include_str!("../../prompts/izzy.md")),
            ("vance", include_str!("../../prompts/vance.md")),
            ("scout", include_str!("../../prompts/scout.md")),
            ("cipher", include_str!("../../prompts/cipher.md")),
            ("communicate", include_str!("../../.claude/skills/communicate/SKILL.md")),
            ("hub-client", include_str!("../../mcp-server/src/hub-client.ts")),
        ];
        for (name, text) in sources {
            assert!(text.contains("timeout_ms: 1800000"), "{name}");
            assert!(!text.contains("persistent: true"), "{name}");
            assert!(text.contains("Monitor expiry"), "{name}");
            assert!(text.contains("4003"), "{name}");
        }
    }

    #[test]
    fn rejects_invalid_seats_without_output() {
        for seat in [
            "",
            "Operator",
            "-lead",
            "seat name",
            "../x",
            "a b",
            "seat\n",
            &"a".repeat(32),
        ] {
            assert_eq!(
                managed_inbox_recipe(seat),
                Err(InboxRecipeError::InvalidSeat),
                "{seat:?}"
            );
        }
        assert!(managed_inbox_recipe(&"a".repeat(31)).is_ok());
    }

    #[test]
    fn never_names_or_carries_a_credential() {
        let r = recipe();
        for banned in [
            "APERTURE_HUB_TOKEN",
            "hub-tokens",
            ".token",
            "token=",
            "Bearer",
        ] {
            assert!(!r.contains(banned), "{banned}");
        }
        let hex_run = r
            .split(|c: char| !c.is_ascii_hexdigit())
            .any(|run| run.len() >= 32);
        assert!(!hex_run, "no hex secret-shaped runs");
        assert!(r.contains("Never look up, type, print, store or pass a credential."));
    }

    #[test]
    fn is_renderer_safe_and_bounded() {
        let r = recipe();
        for forbidden in ["${", "{{", "}}"] {
            assert!(!r.contains(forbidden), "{forbidden}");
        }
        assert!(r.len() < 4096);
        assert!(r.is_ascii());
    }

    #[test]
    fn names_no_other_harness_or_channel() {
        let r = recipe();
        for banned in [
            "codex",
            "Codex",
            "send-keys",
            "--resume",
            "--continue",
            "app-server",
            "stream-json",
            "ws:",
            "positional",
        ] {
            assert!(!r.contains(banned), "{banned}");
        }
        // tmux and hooks appear exactly once, inside the prohibition sentence.
        let prohibition = r
            .lines()
            .find(|l| l.contains("never invent another channel"))
            .unwrap();
        for word in ["tmux", "hooks"] {
            assert_eq!(r.matches(word).count(), 1, "{word}");
            assert!(prohibition.contains(word), "{word}");
        }
        assert!(r.contains("Do not use the Monitor ws source"));
        assert!(r.contains("native Claude Code Monitor tool"));
    }

    #[test]
    fn fixes_boot_order_monitor_then_inbox_then_dispatch_only() {
        let r = recipe();
        let at = |needle: &str| r.find(needle).unwrap_or_else(|| panic!("{needle}"));
        assert!(at("Monitor(command:") < at("get_messages"));
        assert!(at("get_messages") < at("mark_as_read"));
        assert!(
            r.contains("call mark_as_read for a message only after you have actually handled it")
        );
        assert!(
            r.contains("Do no project work before a scoped dispatch arrives through this inbox.")
        );
        assert!(r.contains("Boot order is fixed: monitor, then get_messages, then only the work that was dispatched."));
    }

    #[test]
    fn pins_hub_client_outcomes_no_restart_and_wait_rules() {
        let r = recipe();
        let no_restart = r
            .lines()
            .find(|l| l.contains("Do NOT restart the monitor"))
            .unwrap();
        for code in [
            "HUB_IDENTITY_INVALID",
            "HUB_SOCKET_CLOSED",
            "4000",
            "4001",
            "4003",
        ] {
            assert!(no_restart.contains(code), "{code}");
        }
        assert!(no_restart.contains("if the aperture-bus update_task tool is callable"));
        assert!(no_restart.contains("Keep the exact line"));
        assert!(r.contains("A HUB_RECONNECTING line means wait"));
        assert!(r.contains("HUB_RECONNECTED means unread messages are replaying now."));
        let blocker = r.lines().find(|l| l.contains("honest blocker")).unwrap();
        assert!(blocker.contains("Monitor tool is unavailable"));
        assert!(blocker.contains("HUB_CLIENT_ERROR"));
        assert!(blocker.contains("update_task only if that tool is callable"));
        assert!(
            blocker.contains("If aperture-bus itself is absent there is no way to send anything")
        );
        assert!(blocker.contains("never invent another channel"));
        assert!(!r.contains("restart the monitor after HUB_RECONNECTING"));
    }
}
