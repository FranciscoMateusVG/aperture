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

## Claude limitation

The installed Monitor schema is finite (default 5min, at most 30min), with no
`persistent` parameter. Root's integrated instruction correction uses the
actual schema and distinguishes socket reconnection from process expiry.
Managed Claude offline under a trustworthy subscriber for 60s produces a
bounded generation/owner-bound diagnostic fact and attention indication. It
never sends keys, respawns, parses transcripts or claims the Monitor expired
solely from silence. A live hub-client can reconnect; an expired Monitor needs
rearming; an unavailable provider may prevent that. Without a native proof of
empty input buffer, autonomous rearming/communication recovery is **PARTIAL**.

## Evidence boundaries

Hermetic tests cover original nonce checks, lost-nonce observed-candidate
completion, phase reconciliation/no duplicate dispatch, concurrent exclusion,
binding/token/receipt/phase drift, strict wire accepts and direct/link cleanup.
Socket/process fixtures are local inert evidence, not a provider test. The
whole live lifecycle and BEADS outcome remain NOT_RUN until root authorizes
and witnesses the narrow operation after independent review.
