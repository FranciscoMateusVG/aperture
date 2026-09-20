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
   consumer at an unrelated stale debug build or change its configured override.
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
