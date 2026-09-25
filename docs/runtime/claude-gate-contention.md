# Claude exec gate: bounded lock contention

The parent polls startup observations while the released helper acquires the
same team and seat advisory locks. Previously, one `Busy` result killed the
helper before `exec`, even though the parent treats contention as pending.

The exec gate now retries **lock acquisition only** within its original
`GATE_WAIT` deadline (10 seconds, including the existing release wait and
validation). Each acquisition re-reads the owner and snapshot. Only `Busy`
waits; other errors are terminal. Expiry returns `Closed`.

The callback remains `FnOnce` and runs outside the acquisition loop, with the
validated locks held through the existing release checks and `exec`. A callback
error, including `Busy`, is never retried. Release publication remains one-shot;
there is no new launch, recovery selector, generation reset, or public option.

Hermetic tests use an actual advisory lock, release it after observing contention,
and verify acquisition completes with one effect callback. They also cover timeout,
an already-expired deadline, and every non-contention error. Reinstating single-shot
acquisition makes the contention and timeout tests fail; the error test still passes.

This fixes a demonstrated source defect. It does **not** establish the historical
cause of the eunenem-convites QA generation-2 failure: the helper's original error
was not retained. No live launch or provider call is part of this verification.
