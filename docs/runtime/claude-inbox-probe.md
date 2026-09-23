# Claude inbox probe: one post-observation kickoff

The operator approved one narrow exception to the managed Claude no-input rule:
a single fixed native kickoff **after** exact pre-input model/session observation
and the owner CAS to Active. Public Claude launch remains disabled.

`team_claude_inbox_probe` takes only `{team, seat, expected_generation: 0}`.
It authenticates canonical GLaDOS before validating selectors, requires a NEW
approved Sonnet 5 / reasoning None seat, and shares the startup diagnostic's
admission, pins, reservation, PTY gate, observation and native cleanup. It is not
an ordinary bootstrap or a way to reuse a quarantined generation.

After Active the native helper admits a generation/owner-bound kickoff once,
sends only `launcher::KICKOFF_TEXT`, and records successful submission. No
mission/message body is passed through keys. Admission precedes input; an
uncertain outcome must be inspected, never retried. The target must be a fresh,
unexposed diagnostic pane. Native pane metadata can reject a visible pane or
copy mode, but cannot prove Claude's internal editor is empty or exclude an
unrelated same-UID actor. This residual is not represented as a passing check.

The receipt reports startup `verified`, kickoff `sent`, owner `quarantined`, cleanup `verified`, MCP
readiness `pending_verification`, reasoning `not_observed`, public enabled `false`. It does
NOT mean the model executed a turn, that Monitor exists, that the hub accepted
hello, or that BEADS tools are callable. Those are separate installed witnesses.
The original control call retains the reservation for a fixed window of at most
30 seconds after kickoff (clamped to the original forward budget). Then it ALWAYS
cleans up the same candidate, including on successful input submission, and
consumes the existing SmokeCleanupProof before returning. Errors use the same
finally; cleanup uncertainty takes priority. No automatic replacement/retry.
A full boot may exceed this window: no reply/read-state means roundtrip NOT_RUN,
not permission to substitute presence as a passing result. Ordinary mission
stop/checkpoint validation remains a separate gap; this probe does not bypass it.

The Claude-only prompt appendix instructs the native persistent Monitor to run
`node "$APERTURE_MANAGED_HUB_CLIENT" SEAT`, then read/process/ack BEADS. The
launcher supplies a pinned absolute client path and canonical token FILE path;
the prompt never requests a credential. Identity rejection is a blocker, not a
reason to reconnect with another identity. No hooks, broker, file channel,
headless replacement, global permission bypass or new daemon is introduced.

## Installed acceptance (not established by source tests)

1. Use a fresh approved diagnostic target; leave old and quarantined seats alone.
2. Exact pre-input model/session receipt -> Active -> one native kickoff receipt.
3. Native Monitor hello from that same owner is accepted; process presence alone
   is not evidence of tools or a turn.
4. Enqueue the BEADS diagnostic before launch; only inbox/ack work is allowed.
   A BEADS test message is fetched, answered, and marked read by the same seat.
   Verify the reply and durable read state from GLaDOS. No business task before
   this evidence. Missing tools or permission prompts remain explicit blockers.
5. Verify Quarantined, exact process absence and revocation after the native
   finally. Retain the BEADS reply/read-state as evidence. No owner reset or
   fabricated checkpoint; archival reconciliation remains a separate action.

The older `team_claude_startup_smoke` contract is unchanged: no kickoff, always
cleanup/quarantine, MCP `not_run`. A startup-only PASS is not this journey.
