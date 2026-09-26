# Team retirement is not mission completion

The root-authenticated `retire_seat` action takes only team, seat, expected_generation and the required boolean accept_checkpoint_loss. It never accepts an owner, PID, checkpoint verdict or launch selection. Only GLaDOS can invoke it; root must obtain operator approval before accepting loss of uncheckpointed in-memory work.

With accept_checkpoint_loss=false, the existing native validated checkpoint collector must still yield Valid immediately before stopping. With true, checkpoint recovery is honestly recorded as None, no checkpoint is requested or fabricated. Neither path grants a replacement permit or starts anything. Both retain complete ownership collection (including peer attribution), PID/birth checks, unknown-process denial, bounded exact stop, durable revocation and token deletion.

An explicit retirement has a one-shot child in the existing runtime-attempt ledger. A prior pre-effect Failed may precede it; pending, post-effect, Unknown and Ready attempts do not permit it. There is no automatic retry. A new explicit root call may follow a Failed retirement only when its effects file is absent; it publishes a fresh child admission and preserves every prior fact. Pending, Ready, Unknown and effectful attempts remain denied. Any partial/uncertain result requires factual inspection.

Native stop publishes retired.json only after stop/revocation readback under team/seat locks. It binds the exact owner, team snapshot, team and owner generations, token digest and checkpoint-loss decision. Archive derives Retirement only when all seats have these facts; a mixture fails closed. Revalidation checks facts and process absence under locks immediately before the existing archive journal is written.

Retirement reports process_stop and revocation Verified; mission reconciliation, reviews, metrics, worktrees and remote effects remain Unknown. No BEADS task or epic is changed. The existing journal disables manifests, marks stopped owners Stale and moves the team/history to archive. Rollback restores registry/history but leaves revoked owners Stale: it does not resurrect processes or credentials. The updated control binary must be retained for inverse operations; older readers reject the new category.

Live verification remains separate from source tests. UI/Open improvements are not prerequisites for retiring a stopped team.

## Already stopped, ordinary UNKNOWN attempt

`reconcile_stopped_retirement` is an explicit GLaDOS-only headless action with the same selector-only input as `retire_seat`. It does not retry the ordinary attempt or construct a runtime: no signals, revocation writes, start permits, replacement, or remote-effect resolution. With `accept_checkpoint_loss=false`, it requires the existing native checkpoint collector to yield Valid.

Under team/seat locks, it admits only the exact-generation ordinary directory containing exactly its bound admission, EffectsMayHaveOccurred, and terminal Unknown, after the original 170-second budget plus 10 seconds has elapsed. All recorded processes must be Gone, the exact token must be revoked and absent, and the current observed Active owner must still match the team. Only then can the existing writer append retired.json, no-replace. A second call fails without changing the earlier fact. Missing, malformed, symlinked, unfinished, nested-retirement or mismatched evidence is denied.

The original UNKNOWN and incomplete remote observation remain unchanged; neither becomes a remote PASS. Once every seat has a factual retirement record, the existing Retirement archive journal applies. This is operational retirement, not an assertion of complete external-effect inventory. The operator's acceptance is recorded separately in BEADS. No GUI reinstall is required to invoke the reviewed native control binary for this bounded operation; retain that exact binary and source for the native inverse.
