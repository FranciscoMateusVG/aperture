# Managed Codex MCP readiness (G4NYG)

`OwnerRecord.Active` / `BootstrapView.started` proves the existing native D1
process/model gate. It does **not** prove MCP startup, tools or business delivery.
Standing seats are unchanged. No authority is granted in Starting or generation 0.

## Reproduced installed failure

Read-only inspection of one existing owner-bound thread on codex-cli **0.155.1**:
- aperture-bus: runtimeStatus failed, tools 0, handshake/initialize connection closed;
- Sentry: failed, tools 0, ENOENT;
- another server: connected, tools 133 (not a substitute for aperture-bus);
- same active owner before/after, no turn/reload/restart/config mutation.

The native generated PATH had no executable `node` in any of its six directories.
Node was installed through Volta, outside that PATH. Source used bare `node` for
Sentry and `/bin/sh start.sh` -> `exec node` for aperture-bus. An isolated shell
lookup with precisely that PATH exited 1. Starting was **not** the proven cause:
MCP initialization authenticates registry/token metadata, not managedHelloFields.
No raw config, token, auth/env value or error body was surfaced by the probes.

## Native correction

Resolve Node through fixed native installation candidates, without shell startup
files or caller/environment overrides. Use the existing bounded subprocess helper
(3-second deadline, capped output, isolated group cleanup), sanitized environment,
and `node -p process.execPath` to avoid executing the Volta shim as the MCP process.
Validate the actual regular executable, UID/mode/link/size and SHA; pin/revalidate
it with the existing installed artifacts. Both MCP commands use that absolute
Node executable with their compiled index.js path, not a shell or bare command.
This is only for newly published managed generations. Existing config/owners are
not rewritten and installed workers are not automatically restarted.

## Bridge admission contract

Protocol fields are from `codex app-server generate-ts` 0.155.1. Reference:
https://learn.chatgpt.com/docs/app-server

After the exact native owner becomes Active, and again before unread delivery:
1. Read `mcpServerStatus/list` for **that owner's threadId**, toolsAndAuthOnly,
   bounded to four pages of twenty entries. Reject malformed/duplicate/cyclic or
   truncated catalogs. Both aperture-bus and Sentry must be connected with no
   discovery error and a nonempty catalog. The bus must advertise get_messages,
   send_message, mark_as_read, query_tasks and update_task.
2. Pending startup may settle within one 20-second read-only admission budget;
   terminal failure does not reload, reconnect, restart or send a turn to repair it.
3. Once per connection/readiness epoch, call aperture-bus **get_messages** with
   empty arguments through `mcpServer/tool/call` on the same thread. Require a
   non-error MCP content response. Never ack/send, never log/forward inbox bodies,
   and never call Sentry/provider tooling as a readiness probe.
4. Re-read owner/registry before and after each await. Changed identity, socket,
   generation or startup epoch fails closed. Required-server startup failure
   invalidates the proof. A subsequent dispatch rechecks status before injection.
5. Only then bind/publish presence and send the initial kickoff or unread mission.
   Failure leaves model/process started but bridge unready and messages unread.

`mcpReadiness` is connection-local (not_checked/checking/ready/blocked), not a
persisted owner fact. Logs use codex_managed_mcp_ready / codex_managed_mcp_blocked
with finite code, required-server name, generation and counts only. `waitReady`
still means protocol initialize, not tool readiness. Consumers must not conflate
these. No new transport, broker, token authority or implicit business retry.

## Evidence and remaining integration gate

Hermetic fake app-server oracles cover exact-thread RO proof ordering, no MCP call
before Active, missing/empty/failed/duplicate/malformed catalog, read-probe error,
owner drift, late startup failure and pending startup becoming ready. Native
fixtures cover Volta-only resolution, actual executable pin, config commands,
absence/removal/mode/content drift and existing private/runtime guards.

Source/tests are not installed journey evidence. After the single integrated
package and approved window: verify the same managed thread lists required tools,
one RO call succeeds, then witness the authorized lead -> QA -> root roundtrip.
No business/provider turn or live recovery was run in this source task. Do not
claim this proof for the still-running old generation or perform an automatic retry.

## Claude normal launch: process, observation and messaging are separate

The normal-launch change uses the standing launcher's fixed **positional** boot
prompt, not simulated keys and not the startup diagnostic's 30-second teardown.
An actual status-line model/session observation may arrive before or after that
first input. Configuration is never substituted for observation. Starting still
means activation is pending; Active still requires the observed exact tuple.
Historical pre-input diagnostics keep their own stricter contract.

The existing hub client waits **locally**, without opening a socket, if its
private owner is Starting with its exact token digest and generation. It re-reads
that same identity up to the native 170-second total launch budget. A changed,
revoked, unsafe or disappearing owner fails closed; it never follows a new
generation. Once Active it uses the unchanged managed hello and hub admission.
Standing clients without managed owner records retain their previous behavior.
`HUB_OWNER_PENDING` means wait; `HUB_OWNER_ACTIVE` means only local activation,
not hub acceptance or callable MCP. `HUB_OWNER_TIMEOUT` is a distinct terminal
startup timeout, not a bad-token claim; inspect instead of retrying the launch.

Bootstrap's MCP preflight recognizes only exact Claude `claude-sonnet-5` with
reasoning `null`, and still requires native `capabilities.start`. Its success
parser still rejects missing actual observation. This does not enable the native
Claude gate by itself. Business dispatch remains separate: witness a real BEADS
message, reply and read acknowledgment through the new seat before assigning
work. Permission policy and workspace trust must be explicit; neither is silently
changed by this client wait. Installed normal-launch/roundtrip evidence is pending.

### Operator-approved normal Claude permissions (2026-09-23)

The operator explicitly chose the same autonomous tool-permission policy as
standing Wheatley. Normal managed Claude therefore receives the native
`--dangerously-skip-permissions` flag exactly once, before the fixed positional
boot prompt. This bypasses interactive tool approval; it does not grant a new
mission, publication authority, access to another seat, or permission to expose
credentials. Authenticated native lifecycle controls remain unchanged.

The flag is derived from the private normal launch mode, never from a caller
field or inherited environment. Historical pre-input diagnostics do not receive
it. Personal settings and trust records are not edited. This is an explicit
operator risk acceptance, not evidence of installed readiness or a passed live
journey; public Claude admission remains disabled pending the remaining gates.

The operator also explicitly confirmed the same autonomy for Codex team seats.
Their existing native-generated configuration already sets `approval_policy =
"never"` and `sandbox_mode = "danger-full-access"`; regression assertions now pin
both values. No running Codex process is restarted or reconfigured by this change.
