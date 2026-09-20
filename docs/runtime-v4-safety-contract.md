# V4 runtime safety — K310B implementation checkpoint

Base: `origin/master` 2703dc47. Shared P1 contract: v3, digest prefix
`0abf1b00`. This document is a source/integration plan, **not installation or
live-operation authorization**. Existing standing seats remain untouched.

## Boundaries and ownership

Rex owns the only owner/CAS, secure filesystem helpers, journal/recovery and
control/messaging authority. Runtime modules consume those implementations.
Peppy owns checkpoint policy, exact-process observations, replacement and
archive orchestration and the existing native launcher integration. No second
journal, broker, tool-fencing service or universal effect instrumentation.

The modules are registered in the native library and exercised by unit tests
against the actual compiled modules (not copied path-module implementations).
Passing them does not expose a Tauri button or prove native replacement.
Capabilities remain false until their authoritative adapter is integrated.
The legacy GUI/headless lifecycle and watchdog re-kick paths deny managed or
ambiguous seats before any nudge, teardown, model mutation or new process effect;
only authoritative filesystem classification can admit a standing seat.

## Two distinct replacement actions

1. **Prepare / stop / verify** validates owner generation and captures the exact
   PID/OS birth identity, process group and descendants, including persisted
   reparented children. Unowned cwd/command matches are blockers, not kill
   authority. Command bytes are hashed privately; raw command/environment is
   never a receipt. Missing, recycled or unreadable ownership fails closed.
2. Busy/429 recovery requests a bounded checkpoint; failure to produce a hook
   result does not wait forever. Recheck all process identities after the wait.
   TERM children first, bounded wait, then KILL only exact surviving identities;
   recheck birth immediately before every signal and final absence. macOS
   check-to-signal is not an atomic kernel capability; no stronger claim.
3. Revoke the exact old generation/token using shared control. Require durable
   generation floor, exact-socket closure and fsynced token deletion before a
   new token. Corrupt or unavailable revocation evidence blocks replacement.
4. Reconcile effects. An empty transcript/list is **not** zero in-flight work.
   Shell/SSH effects without durable completion evidence remain UNKNOWN until
   an authenticated lead/operator resolution is recorded. Never auto-replay.
5. **Start replacement** consumes a backend-only preparation permit, rechecks
   generation/absence/revocation/effects and authorizes the exact immutable
   harness/model/reasoning tuple. It uses a fresh thread, never “most recent”.
   Reserve durably before spawn/token creation; attach exact PID/birth while
   the child is gated. Shared owner commit is the only ownership writer.

### Authenticated agent control composition (root decision 8zuo5a)

The same `aperture-team-control` native entrypoint may expose an authenticated
worker's own checkpoint and a lead's one-shot replacement within immutable team
policy. Rex owns that authentication/command seam. Authenticated seat, generation
and actor provenance are derived from canonical authority, never request fields
or environment assertions. A replacement request's `expected_generation` is
permitted only as the TARGET owner CAS selector; it is not actor authority.
Collector/process/model/revocation observations come from native adapters, never
caller-provided proofs. Self-replacement must prove the control process is outside
the entire stop set; without that proof, deny and retain the existing operator
path. Do not kill the control process halfway through its own operation.

This one-shot agent action does not alter the frozen human prepare/start dialog
contract and never serializes `PreparedReplacement`. Its implementation cannot
be published or composed as ready before the consolidated owner/hub H1-H4 fixes
and negative-matrix review. No second broker, capability store or journal.

### Root decisions D1–D4

- **D1 normative delta:** exact actual model must be observed before replacement
  completes. Mismatch/unobservable model invokes exact-new-process stop and
  durable revoke, records failure, returns no active owner/task dispatch and
  never automatically starts g+2. Startup-window events and the final invariant
  are separate; this is not a claim that no action was possible before detection.
- **D2 clarification:** legitimately persisted messages remain stable seat intent
  after replacement. Current recipient/registry/policy are revalidated; stable
  ID replay-until-ack remains. Origin generation is provenance, not a new reason
  to discard queued messages. Revocation blocks future frames/sends.
- **D3:** monotonic revoked-through generation, never-reused token IDs, fsynced
  deletion; corruption fails closed across restart.
- **D4:** typed, bounded checkpoint data and sanitized decisions/next-step text.
  Known sentinels, controls/bidi, credential URLs and env/tool dumps are rejected.
  This is intentionally **not** universal secret detection.

## Checkpoints

Required task ID, validated relative worktree/branch, exact head, dirty paths,
structured PR, PID/birth references, coded decisions, bounded next step and
remote-effect status. No arbitrary transcript or environment. Caller inputs
cannot assign authenticated writer/generation, sequence, timestamps or validation.
Claude Stop-hook and explicit writes share launcher-assigned sequence and the
five-second hash dedupe. Codex is explicit only. Unknown schema is retained
rejected and not recoverable. Latest usable checkpoint means highest validated
sequence, not simply newest file. Git/PR observations are compared before marking
valid; stale/divergent/pending remain distinguishable.

Native persistence uses the shared secure IO and lock implementation: team lock
then seat lock, component checks, private UID/0700 parents, regular single-link
0600 append-only files, no-replace publication and fsync. The internal writer
checks the exact active owner/model tuple and revalidates the transport capability
before lock, under lock, and immediately before publication.

Lead validation is stored separately at
`checkpoints/<seat>/.validation/<g>-<checkpoint-seq>-<fact-seq>.json`. Facts bind the
checkpoint ID and content hash, current authenticated lead generation, bounded
artifact observation, native timestamp and derived result. Original checkpoint
bytes remain unchanged. Facts are ordered under the team lock then all needed
seat locks in lexical order. The current active lead may validate an older worker
generation; an old worker cannot write into that generation. The latest fact for
a checkpoint controls the read projection, including a later divergent result.
Unknown schema remains rejected without invoking an artifact collector. Invalid,
missing, duplicate, unsafe or hash-mismatched evidence fails closed rather than
silently projecting a green checkpoint. Capability revocation before publication
leaves no fact; a destination collision never overwrites existing evidence.

These adapters remain internal: the authenticated worker/hook/lead control seam,
bounded actual git/gh collector and sanitized BEADS mirror are not yet wired.
Tests inject collector/authority callbacks in private temporary homes; no live
bearer, process stop or provider call is exercised. No callback is exposed as a
caller boolean or arbitrary observation in a public DTO. The eventual BEADS
mirror contains only the sanitized bounded receipt.

## Archive

Fresh authoritative reconciliation covers every assigned task and required
review/metric. Completed work needs evidence; cancellation needs approval;
transfer needs outside owner acceptance/history and reparenting where required.
Exact process stop, revocation, effects reconciliation and clean/protected
worktrees are gates. The team lock precedes lexicographically ordered seat locks.
Archive delegates to Rex's single no-replace journal. Recovery reconciles physical
state, never trusts a step counter alone. Both/neither endpoints are terminal
inconsistent evidence; never delete them or recycle archived identity/generation.

## Installation / coexistence / rollback plan (not executed)

1. Freeze cumulative reviewed head/tree across Rex, Peppy, Vance and role inputs.
   Build/test Rust and MCP with existing native commands. Record source/output
   hashes and independent QA/security verdicts. No build against concurrently
   edited inputs is a release receipt.
2. Isolated fixtures use a private temporary home and explicit synthetic seats,
   fake clock/process effects and the real filesystem primitives. Test two-process
   owner CAS, crash points, no-replace collisions and APFS behavior. No standing
   roster, live hub, provider or production token is a test target.
3. Before any approved install, retain private preimages of the installed binary,
   MCP output and affected configuration, plus exact runtime binding and versions.
   Existing standing seats must remain byte-for-byte and generation-stable.
   First install must not infer ownership of already-running legacy processes.
   Include the existing headless control binary explicitly: MCP team-control
   expects `~/.aperture/bin/aperture-team-control` by default. Building the GUI or
   `aperture-boot` alone does not install it; an absent binary yields
   `E_CONTROL_UNAVAILABLE`. Build from the same frozen cumulative head with
   `cargo build --manifest-path src-tauri/Cargo.toml --release --bin aperture-boot
   --bin aperture-team-control` (one command). Record the actual Cargo output
   directory, both executable hashes and compiler versions. Do not point the
   consumer at an unrelated stale debug build. Production uses this fixed path;
   `APERTURE_TEAM_CONTROL_BIN` is a fixture/test override only, not a production
   deployment mechanism (source contract confirmed by Rex).
   Publication belongs in the existing native recipe/lifecycle, not a new
   transport or auto-install-on-tool-call path. Before publication, validate the
   destination parents without following caller-controlled symlinks, current UID
   and private directory modes. Retain the previous control binary privately, or
   record that it was absent. Stage the reviewed control executable as a regular
   single-link current-UID file mode0700, fsync, and replace only the exact guarded
   destination; fsync its directory and verify the installed hash/owner/mode/link
   count. Never `cp` blindly onto a symlink. The consumer permits executable
   current-UID single-link files without group/other write; installation uses the
   stricter private0700 mode. Recipe integration and operational execution remain
   gated; these instructions are not evidence that either has happened.
4. Obtain the separately approved shared-app/hub interruption window. Use the
   existing native install/lifecycle path only; no automatic setup/restart here.
   Verify installed artifact, process readiness, registry coexistence, then an
   explicitly authorized synthetic-team user path. Report source, installation,
   control, model observation and functional results separately.
   Control verification must include the real consumer resolving that installed
   hash and a bounded authorized read-only control response; binary presence or
   a GUI-ready signal alone does not prove the MCP activation path. Keep all
   capability/bearer values out of receipts. No activation or managed start is
   implied by this install check.
5. Rollback is not journal deletion or generation reset. Stop only the exact
   newly owned test incarnation and revoke it durably using the same seam.
   Reconcile active journal moves before reverting software. If new state cannot
   be read by the prior binary, hold for forward repair rather than exposing old
   code to incompatible state. Restore approved binary/config preimages only
   after explicit operational authorization; preserve all owner/revocation/
   checkpoint/archive evidence and record unresolved effects as UNKNOWN.
   Restore the control binary and matching MCP artifacts as one compatible
   software set using their preimages and hash readback. If it was previously
   absent, do not leave the new command usable under the old consumer by accident;
   any removal/disable action is part of the separately approved rollback, not
   automatic cleanup. Never restore old mutable owner or revocation state.

## Remaining integration gates

Shared owner/journal/secure-IO signatures and native checkpoint persistence are
integrated. Authenticated checkpoint/control transport and artifact collection,
GUI/headless managed start path, actual-model observation and cleanup, durable
hub revoke acknowledgment, authoritative archive reconciliation, and the full
crash/two-process composition suite remain required. Pure fixtures alone do not
satisfy these gates or authorize installation.

## Native remote authorization seam (root pye513, source only)

`team_replacement::remote` owns one native inventory/fact projection over the
existing checkpoint and journal substrate. `inspect_native`, `resolve_native`
and `project_native` take a trusted home plus target team/seat/generation CAS.
Resolution alone additionally takes `ResolutionAuthority::Operator` referencing
an existing internal OperatorUi actor, or `Lead` referencing the authenticated
seat control identity. Neither is deserializable. Control supplies only the
strict `ResolutionRequest`: inventory hash CAS, scope, optional reference,
decision and bounded evidence reference. The existing agent replace input stays
exactly target seat, expected generation and execution selection.

Inventory collects the union of retained references from this target generation's
bounded, hash-validated checkpoints. Worker-declared finished/cancelled states
remain **Unknown**; no instrumented external adapter exists here. Even an empty
history has `complete_observation=false`. Sorted references plus generation and
observed states determine the native inventory hash. The current limits are
1,024 checkpoint directory entries, 16 MiB selected checkpoint bytes, 64 distinct
references and 256 resolution facts. Limits/corrupt/unknown-schema histories fail
closed, never truncate to a convenient empty inventory.

Facts are private append-only files in the existing checkpoint sidecar substrate
`checkpoints/<seat>/.remote-resolution`. They use team then lexical seat locks,
current active owner/lead checks, the shared no-follow private IO, no-replace
publication and fsync. Inventory/owner/capability are rechecked before publishing.
Fact metadata/time/sequence/principal are native-derived; evidence references are
bounded and sanitized, not arbitrary transcript/tool/provider payloads. Known
sentinel filtering is not universal secret detection.

Only OperatorUi may accept an incomplete inventory's unobserved-effect risk.
An authenticated current lead may decide finished/cancelled for an existing
reference on another seat in its immutable team, never itself. Each Unknown
reference needs its own bound effect decision; inventory risk acceptance does
not resolve named effects, and per-reference decisions do not make an incomplete
inventory complete. Stale generation/hash facts grant nothing. Exact replay
returns the original; conflicting same-scope facts fail closed rather than using
latest-wins or letting a lead overwrite operator intent.

Projection retains Unknown states and incomplete observation, recording
`authorized_decision` separately. Durable fact readback establishes binding and
persistence, **not truth or zero in-flight effects**. The replacement core reads
this projection and checks exact generation/reference coverage; it cannot accept
an authenticated boolean from a replacement request. Auth/control registration,
human resolution UI and the complete native replacement journey still require
composition and consolidated review. No live resolution, process effect, new
capability or installation is authorized by this source checkpoint.

### Authorized inventory inspection and native process collection

Control must call `remote::inspect_authorized`, not the internal collector-only
`inspect_native`: inventory reads use the same authenticated principal, current
lead/other-seat policy, team then lexical owner locks, and capability revalidation
as resolution. OperatorUi can inspect its selected target. No lead inventory is
returned for a self-target, other team, stale owner or missing authority.

`team_process::native::collect_native(home,team,seat,generation)` reads the current
managed OwnerRecord and derives an OS observation; it is not signal authority.
The separate `persist_for_stop` lock/CAS guard is still required before effects.
The macOS adapter uses the SDK's PID table and exact birth records, privately
hydrates cwd/argument digests, keeps persisted orphan identities, and refreshes
child depth before children-first stop ordering. It checks a second topology
observation and all exact identities before return. Recycled/unreadable/missing
live rows, newly unobserved descendants or same-user outsiders invalidate the
snapshot. Same cwd/command matches outside ownership are blockers only; shared
working directories may therefore block conservatively, never widen kill scope.
A control process included in the owned stop set is rejected before argument
reads. There is no claim of atomic kernel process fencing or observation of a
child that detached before it could ever be recorded.

PID/metadata/argument/time bounds fail closed. `KERN_PROCARGS2` necessarily returns
a native buffer containing environment as well as argv; only its argc-delimited
argv is hashed and the entire private allocation is best-effort wiped. No raw
arguments or environment are returned/logged/serialized. This is not a universal
memory-erasure guarantee. Current collector tests supply synthetic native tables
and byte buffers: the real OS collector, live process-stop and complete launcher
journey remain NOT_RUN. No public capability or live action follows from these
source fixtures.

### Managed launch primitives (unwired source checkpoint)

`hub_auth::managed` is a separate no-overwrite token publisher; the standing
provisioner remains byte-identical. It validates the native Starting reservation,
team membership, canonical private roots and prior revocation floor, generates
fresh entropy privately, and publishes with shared no-replace/file+directory
fsync. Existing token files are never reused or removed. The publisher now uses
`OwnerStore::bind_and_publish_token`: the shared owner lock binds and fsyncs the
provisional digest BEFORE invoking no-replace publication. The callback never
relocks the owner; it verifies the bound record and revocation floor, then writes
and reads back the token. Publication failure retains the exact provisional
identity without an incarnation, allowing owner-bound cleanup rather than a
fake PID/thread or automatic retry. Ten publisher fixtures include actual
lock-held/durable-preimage observation and failure preservation. The complete
spawn-failure revoke journey is still not wired or proven.

`team_replacement::launch_gate` starts an internal pipe-gated child in its own
session/process group. The shell program is fixed and all executable arguments
are positional data. Parent pipe loss exits before the harness exec; no signal
or restart fallback occurs. Release rechecks the private Starting owner under
its OS lock, reservation nonce, exact root PID/OS birth and recorded root, and
requires unobserved state. Only then does one pipe write release exec. An
ambiguous release must use exact native stop/revoke cleanup, never retry start.
The returned child is not an Active owner: actual tuple/thread observation and
owner CAS are later requirements. Managed Codex must not use the legacy
app-server supervisor, thread-resume selection or automatic restart.

Four focused fixtures actually run disposable local gate shells that can only
create a private temporary marker with `touch`: pre-owner blocking, exact-owner
release, wrong/absent birth rejection, parent EOF and argument-as-data handling.
These are not live harness, gateway, team replacement or provider tests. The
full one-shot adapter, provisional-token crash composition and actual-model
observation remain unproven; public capabilities and installation remain off.

### Native actual-model receipt consumer

`team_replacement::model_observation::read_native` reads only the fixed private
bridge artifacts `<seat>.g<generation>.managed-start-attempt.json` and
`<seat>.g<generation>.managed-observation.json`. It takes a native reservation,
not a caller-supplied proof or artifact path. Strict schemas and an 8 KiB cap
bind seat/generation/token/root PID+birth, requested model/reasoning, returned
thread/model/reasoning and observation time to the current Starting owner.
Both the attempt and final receipt are required. Time must be within this
reservation's native start time and the current clock; clock anomalies block.

The reader holds team then owner locks, checks the canonical token privately
and the existing revocation history, verifies the exact native root identity
before and after reading, and rechecks the owner and token before returning.
Missing/corrupt/unsafe/stale/mismatched facts produce finite errors with no raw
values. Returned `VerifiedObservation` is neither deserializable nor an Active
owner. The existing owner CAS must still record the native observation and
commit activation; the adapter must recheck process identity at effects. No
claim of atomic OS process fencing or universal memory erasure is made.

Eight focused tests use synthetic receipt/native-state fixtures, including the
shared owner record/commit path, full-field mismatch matrix, missing/corrupt
attempt, time/size bounds, private-file violations, recycled/dead/unreadable
process state, owner/token races and revoked/corrupt/symlinked revocation state.
They do not execute an actual Codex session or enable public lifecycle actions.

## Native initial-journey composition checkpoint (K310B, 2026-09-20)

This section supersedes earlier *unwired* descriptions for the concrete native
start/cleanup and human-permit adapters. It is a **partial source checkpoint**,
not V4 completion, release approval, capability enablement or installation.
Archive acceptance remains open and its capability must remain false.

### Exact internal entrypoints for Rex's composition

All reside in `team_replacement::native`; none is registered by this change:

- `bootstrap_authorized(home, actor, team, seat, expected_generation)` requires
  native OperatorUi and exactly Stale owner g0. It derives the tuple from the
  immutable snapshot; no replacement selector, fallback or legacy boot path.
- `replace_authorized(home, ReplacementAuthority, RemoteTarget, selection,
  sentinels)` is the agent one-shot path. Current authenticated lead identity
  and target CAS remain distinct; no observations/resolutions in the request.
- `prepare_operator(home, actor, team, seat, expected_generation, sentinels)`
  returns `NativePreparedReplacement`, which is neither Clone nor serializable.
  Its team/seat/generation/recovery getters are bounded metadata for the UI.
- `start_operator(actor, NativePreparedReplacement, selection)` consumes that
  object. The Tauri state map must remove the opaque ID before invocation.
  Do not serialize the core permit or map one-shot replace onto these dialogs.

The native success type is internal `StartedReplacement`: **do not serialize
its thread ID**. Transport rereads/projects the safe OwnerSummary and fixed
view contract. Runtime errors expose only `ReplacementError::code()`. Keep
commands/capabilities gated until registration plus the integrated review.

### Launch and D1 cleanup

The installed Codex app-server is started once through the existing private
seat socket, never the legacy respawn supervisor. Source/build/private-runtime
pins, tuple, immutable repository and cwd identity are revalidated. A fresh
private generation home contains bounded prompt/skills, native config and a
non-model copy of the existing auth. Config publication is no-overwrite and
preserves failed artifacts. No token/auth/password enters argv or receipts.
Only the shared owner callback may bind/publish a fresh token; the pipe stays
closed until exact PID/birth is durable. A failed release retains its exact
child handle for cleanup instead of losing ownership evidence.

The bridge's exact generation/token/PID/birth/model/reasoning/thread receipt is
read, persisted through shared owner CAS and reread while observed-Starting.
Active is never accepted as a receipt substitute. Only after this check does
shared commit publish Active. A late failure even after Active commit stops
and revokes the same candidate, then calls the shared launcher-only quarantine
CAS; it does not erase incarnation/process-union evidence or start g+2.
Uncertain cleanup remains UNKNOWN and requires reconciliation.

### Worktree identity is not recovery freshness

Bootstrap alone may use the bound repository root (not implicit edit authority).
Replacement requires a previously authenticated/validated checkpoint binding
for this target, then fresh membership in that repository's Git worktree list.
Current artifact divergence makes recovery stale, but does not erase the
historical authenticated worktree identity. Historical facts retain their raw
Pending checkpoint projection; only fresh native artifact equality permits
current recovery contents in the new prompt. Pending/none without another
trusted task/seat binding fails `E_WORKTREE_UNBOUND` before lifecycle effects.
One registered worktree, a project label, process cwd or a worker path alone
never supplies that authority. No implicit worktree or new registry is created.
The lead path may validate a candidate internally; OperatorUi never impersonates
a lead. A stale recovery prompts inventory of the bound worktree, not guessed
completion of previous effects.

### Finite clocks and human pause

Native total is 170 seconds; parent's replace watchdog is 180 seconds. Forward
work ends by 130 seconds, retaining 40 seconds for cleanup: two capped native
collectors (10+10), TERM/KILL verification (10+1), hub control (3), and metadata
margin (6). Before the first effect the adapter requires 116 forward seconds
plus this 40-second reserve; if preflight consumed the margin it fails before
effects. Launch/observation receives at most 90 seconds (85 observation), never
a renewed overall budget. Native collectors/control also honor the outer
cleanup deadline. Missing checkpoint hook returns unavailable immediately;
this adapter does not fabricate a 90-second hook acknowledgement or observer.
Filesystem/kernel scheduling is not hard-real-time; watchdog timeout is UNKNOWN,
not rollback or proof of no effects. Admission/terminal evidence blocks blind
retry. Same-UID/kernel TOCTOU residuals remain as previously documented.

Human Prepare writes terminal Ready and retains a **clock-free** object. Reading
the dialog consumes no runtime budget. Start begins a fresh deadline and checks
permit CAS, immutable pins, owner, stopped identities, revocation, inventory and
selection again. Invalidation before new effects is `E_PREPARATION_EXPIRED`,
blocked, preserving the previous stopped/revoked fact—not a rollback. Unknown
is reserved for actually uncertain effects/transport. No arbitrary TTL added.

### Evidence and remaining integrated gates

Tests use temporary private files, synthetic auth/config/receipts, fake process
observations, loopback fixture hub sockets and disposable pipe-gate children.
No real Codex session, provider, live team process or installed auth is exercised.
Native start/prepare/cleanup composition is source-tested, not a witnessed
installed journey. Standing launcher behavior is not replaced.

Rex still composes the native commands/state map with the already frozen UI and
control bridge. QA/security must review the resulting exact head before any
capability exposure/installation. The current native launch implementation is
Codex-only; unsupported harness/control changes fail closed rather than using a
legacy path. Archive remains required: Peppy still owes the bounded current
BEADS epic/children inventory, typed authenticated per-item reconciliation and
review/metric references, joined with exact-seat stop/revoke/remote evidence
before using Rex's one journal. Notes/free text/caller booleans cannot fill that
gap. No archive completion or installation is claimed by this checkpoint.
