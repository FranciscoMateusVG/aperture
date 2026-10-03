# Managed runtime recovery (g4nyg)

This is source-only until the reviewed package/control is applied by the root.
No live recovery is performed by the tests. Starting is not communication
readiness, and accepted loss is not evidence that no remote effects occurred.

## Codex reconnection

An already Active owner is checked against `thread/loaded/list` and exact
`thread/read` metadata. A loaded exact thread needs no resume. A non-loaded
thread requires actual persisted-path metadata before exact resume. Absent,
contradictory, unobservable or changed bindings block without starting another
thread or delivering a turn. Classification and initial MCP proof share 20s;
the earlier singleflight/one bounded readiness re-entry remains. Catalog plus
the authenticated read tool must pass before delivery. Durable BEADS ACKs are
not replayed on a reconnect.

## Explicit loss-accepting operation

`recover_codex` is available only through the existing authenticated GLaDOS
native control/MCP. It takes the original team, seat, generation, raw owner-file
SHA256 and exact thread ID, one operation UUID, and separate literal accepts
for context loss and unverified effects. `mission_withdrawn` is an explicit
root acknowledgement, not a backend proof of BEADS message disposition. Root
must withdraw old business dispatches before invocation. No name-based special
case, model override, checkpoint forgery, nonce reconstruction or caller path.

The admission retains original private owner bytes and snapshot binding under
the existing team `runtime-attempts/SEAT/gN/codex-recovery` subtree. Original
admission/effects/Unknown files are never rewritten. This is a new, explicitly
discriminated history child; old strict readers may refuse this history rather
than infer mission completion. No archive compatibility claim is made.

One operation lock prevents concurrent execution. Native complete/disjoint
collection and the existing persisted process guard remain signal authority.
Each exact signal, revocation, fixed endpoint release, quarantine and new start
has a no-replace intent/result. After interruption, the same operation may
reconcile a proven postcondition, but cannot resend an uncertain effect.
Genuine ambiguity stays UNKNOWN. A completed signal fact is not proof of Gone.

Before stopping, the fixed seat socket and native peer PID/birth are pinned.
After full Gone proof, cleanup rechecks parent/leaf/native target pins and
requires explicit connection refusal before unlinking only the fixed basename
through its parent FD. A native symlink target is retained, not deleted.

Gone plus floor-bound revocation and token absence precede the typed owner
Quarantined CAS. New admission uses the approved repository/tuple and the
existing launcher. Every invocation retains the 170s native cap/40s cleanup
reserve, without poll-based renewal; an explicit later call can reconcile the
same operation's facts, not repeat a known effect. A start intent with the exact
old quarantined generation and no next-generation facts is provably before the
first reserve; otherwise no second reserve/spawn. A crash after genuine
observed Starting is reconciled by the exact private attempt/receipt and native
PID/birth/token checks, never by reconstructing its lost nonce. Ordinary
`read_native`/`commit_start` continue to require the original nonce.

The result is `started`, `wait_go`, `not_verified`, `unknown_accepted`. Root's
next authorized live check must be only a pre-enqueued technical BEADS
read/ack/reply challenge on the withdrawn QA seat, with exact successor owner
and process readback. No old PR/business dispatch, automatic retry or restart
of healthy seats. Unknown requires inspection of this same operation.

## Claude native inbox: new normal generations

New normal publications select `plugin_v1` internally. A private directory at
`managed/SEAT/gN/inbox-plugin/.claude-plugin/plugin.json` contains exactly one
`experimental.monitors` entry (`when: always`). Its command is rederived from
the pinned absolute Node + packaged hub-client + seat; `--plugin-dir` is
rederived by the launcher, never a caller path. The whole closed plugin tree
is inventoried: extra hooks/settings/MCP/scripts, links, unsafe modes, missing
or changed pins fail closed. The existing gate rechecks before exec. Old facts
omit `inbox_mode` and remain readable; diagnostics keep their pre-input/finite
recipe. A failed new plugin never silently falls back to the Monitor tool.

The appended normal prompt explicitly prohibits a competing Monitor/client.
The installed Claude 2.1.281 native plugin loader supports whole-session
interactive monitors; the strict CLI validator accepts a credential-free
fixture of this manifest schema. This is **parser/source evidence**, not a
native monitor session or functional recovery PASS. Monitor availability and
host/provider gates still apply; no hidden flag is enabled here. The plugin
must run from session spawn, not be hot-loaded into existing sessions.

A live client reconnects to the local WS and sends the same native hello. This
removes the finite Monitor tool's 30-minute expiry from NEW normal launches.
A terminal process exit remains distinct: no automatic host restart was
proved. Disable/reload is not teardown and must not be used as proof of unique
subscription. Codes 4000/4001/4003 and owner/generation/token fences remain
terminal. Existing sessions are not migrated by this source change.

## Pending unread while WS remains connected

A provider failure may consume a notification without a model tool call while
the local socket stays connected. Native plugin queues have no demonstrated
BEADS read-ack binding or eventual provider-error retry. Therefore the EXISTING
hub maintains one coalesced pending reminder for an owner-proven **managed
Claude agent connection only**, never standing, Codex, producer or subscriber.
The helper owns only scheduling; query and authority stay in `ws-hub`.

After hello replay settles, unread checks use the existing authorized/capped
`getUnreadMessages`. A live unread entry yields a finite fixed inbox reminder
and up to 20 stable IDs, **not mission bodies, automatic read, model turn/start
or business-effect replay**. Delay is 30/60/120/300 seconds with bounded jitter
and a 300-second cap, singleflight per connection. Unread remains pending until
BEADS says empty/read; query errors are not empty. New notify during an older
empty query is versioned so it cannot be lost. Pending state need not be
persisted: reconnect's authoritative unread replay restores it.

Socket, current registry, exact Active/observed owner generation/thread/tuple/
PID-birth, token identity and revocation floor are fenced after query and
before send. Read/empty quiesces the timer; close/replacement/revocation/drift
cancels it, including callbacks/queries already queued. Reminders can continue
at the cap during a long outage but cannot make the provider accessible.
Processing/ack/reply is still required; presence or accepted notification is
not recovery. No transport, daemon, polling model turns or secret access path
is added. Known failed business dispatches require explicit root disposition.

## Reviewer follow-ups in this child

F3 preserves the observed pending-startup timeout category when the single
20-second deadline expires inside the status RPC. Owner/terminal drift still
wins and delivery stays blocked; the causal test holds a reply only after the
remaining deadline is below the independent 10-second RPC cap.

F1 permits only the exact **pre-CAS** quarantine re-entry: same operation and
admission, current exact Active owner, team/seat locks, snapshot binding,
complete Gone process proof, revocation/token absence and fixed socket absent.
It cannot retry a generic effect or signal/revoke/spawn. Post-CAS Quarantined
reconciles its existing postcondition without a second CAS; any drift denies.
The earlier 180s MCP vs native/cleanup budget shape is unchanged (review F2).

## Evidence boundaries

Hermetic tests cover original nonce checks, lost-nonce observed-candidate
completion, phase reconciliation/no duplicate dispatch, concurrent exclusion,
binding/token/receipt/phase drift, strict wire accepts and direct/link cleanup.
Socket/process fixtures are local inert evidence, not a provider test. The
whole live lifecycle and BEADS outcome remain NOT_RUN until root authorizes
and witnesses the narrow operation after independent review.

### Scope of the next witness (root disposition, NOT_RUN here)

Use only the withdrawn broken QA for the reviewed typed Codex recovery and a
technical BEADS read/ack/reply. Plugin A applies only to a separately approved
new normal Claude generation; no restart of healthy sessions or plugin reload.
For Claude A/B, observe one native plugin process and owner binding, deliver a
technical inbox challenge across a controlled WS reconnection and separately
retain an unread challenge while the local WS stays up. Only read/ack/reply
when the provider returns closes the functional claim; no deliberate public
Internet outage or unapproved provider invocation is part of these tests.

### Application plan (not executed)

This child changes Rust launch/control **and** MCP; a hub-only compiled swap
cannot apply A or F1. Root must approve a newly paired package/control/boot
from the reviewed integrated head via the existing native packaging workflow,
retain current worker-input trees and record exact current server/hub/process
bindings. Do not overwrite live files. The existing native launcher/Supervisor
owns server stop, exact hub disposition and Same/Gone adoption; no new broker.
A transient UI/comms gap is distinct from worker lifetime. Existing appservers,
Claude sessions, owner/thread/token facts must remain untouched by package
application. Any native watchdog-only rotation is recorded, not counted as
worker recovery. Fresh readbacks must precede the explicit withdrawn-QA
operation; another invocation of that SAME operation only reconciles proven
facts, never a new operation UUID to evade Unknown. Plugin migration of a
currently healthy Claude session is not included. Rollback retains old package
inputs, but cannot undo accepted context loss or erase a completed lifecycle
transition; older readers may fail closed on new facts.
