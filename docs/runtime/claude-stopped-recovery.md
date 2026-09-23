# Stopped Claude diagnostic recovery

`reconcile_claude_startup` is an authenticated native control action, not a
bootstrap or a replacement. It accepts only team, seat and expected_generation.
The canonical GLaDOS capability is checked before admission and again at effects.

The eligible case is narrowly an unobserved Sonnet diagnostic still Starting g1,
with the exact durable Claude attempt, snapshot and expired g0 admission/effects.
A live, recycled, unreadable or unaccounted process blocks recovery. Collection
and persistence use the existing native process proof. No signal is sent.
The existing hub control revokes the exact generation token, then a native-only
proof holding team and seat locks allows OwnerStore to quarantine the unchanged
candidate. Generation, incarnation and observation history are retained; no
nonce is reconstructed and no start is made eligible again.

The original terminal fact is preserved. An append-only `reconciled.json` records
`stopped_reconciled`; it does not turn an absent/unknown smoke result into PASS.
The response says startup `not_verified`, cleanup `stopped_reconciled`, owner
`quarantined`, public_enabled false. Any failure after an effect remains UNKNOWN;
inspect facts before further action, never reset or retry a launch.

Separately, a genuinely held observation lock now returns pending to the existing
bounded poll, rather than a false model mismatch. Unsafe locks and invalid owner
identity still fail. This reproduces/fixes a possible failure path, not proof of
the initiating historical error (that error was not durably captured).

Recovery may be run using the reviewed source-built native control binary on
stdin. It needs no GUI replacement and does not update any installed executable.
Future launcher behavior changes still require the usual paired release.
