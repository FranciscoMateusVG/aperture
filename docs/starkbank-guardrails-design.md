# StarkBank money-movement guardrails — security design (v3.2, final crypto-custody patch)

**Revision history (Cipher PASS on v3.2, 2026-08-09 — DESIGN COMPLETE):** v1→v2 corrected the core model (provider-site approval IS execution; no local dispatch to guard). v2 architecture ACCEPTED (all 7 blockers closed). v3 folded 6 implementation-clause tightenings (global `BEGIN IMMEDIATE` reservation, split-ledger release, two-stage query consume, `limit:2` ambiguity, array-length-1 create assert, in-txn audit). v3.1 closed 2 textual/control gaps: §8 cap-release now IDENTICAL to §4.1's split-ledger (was contradictory), and audit made DB-enforced (append-only table + `BEFORE UPDATE/DELETE` triggers `RAISE(ABORT)`, same-txn coupling) + OCI backup pinned concrete. v3.2 fixed backup key custody to **ASYMMETRIC age** (public recipient on the Mini; private identity in operator OFFLINE custody — a Mini compromise can encrypt but NEVER decrypt). **Implementation is GATED on: operator go + dashboard attestation + the §4.6/§9 sandbox proofs + a fresh Cipher diff-review of the built code. No build yet.**


**Author:** Wheatley (planning) · **Security owner/reviewer:** Peppy → **Cipher** · **Date:** 2026-08-09 · **SDK:** `starkbank@2.39.1` · **Status:** DESIGN ONLY.
**v2 core correction (Cipher HOLD, 7 blockers):** v1 mixed a nonexistent LOCAL post-approval payment dispatch into the PaymentRequest model. **With PaymentRequest, the human's StarkBank-SITE approval ITSELF executes the payment. There is NO local APPROVED→SUBMITTING dispatch for us to guard.** The only provider call WE make is `PaymentRequest.create` (the approval request). Everything is reframed around the **approval-REQUEST lifecycle**; a post-approval mismatch is an **incident/audit signal that freezes FUTURE ops — it can never retroactively stop the current payment.** Every SDK claim verified against the installed source.

---

## 1. Current state (grounded, `mini` source)
- All 3 money tools fire `*.create` IMMEDIATELY (no preview/approval): `brcodePayment.create` (repo:188), DICT-then-`transfer.create` (repo:114; DICT is routing, not a gate), `boletoPayment.create` (repo:220).
- No idempotency, no cap, no allow-list, no confirmation.
- `STARKBANK_ENVIRONMENT ?? 'production'` (auth:25) — absent config = REAL MONEY.
- taxId mislabel: pix-payment (tool:54) + boleto (tool:155) say "Your CPF (payer)" but the SDK field is the RECEIVER (§3.1).
- **R3 — WORSE than "accidental safety" (Cipher correction b):** creates ride `withRetry`. The classifier is Google-shaped, BUT **Axios-transport errors (429/502/503) CAN already be retried today** (they surface a numeric `response.status`) → a live payment create can already double-fire on those. This is a present bug, not a latent caveat.
- No audit: logger writes operation+success+duration to stderr only (logger:11-29); amount/destination/id never captured or persisted.

## 2. Target architecture — PREPARE-only; provider-site approval IS execution
The MCP has NO money-moving power and NO local payment dispatch. It PREPAREs an approval request; a human approving on StarkBank's site executes the payment; we OBSERVE the outcome by polling.

**Flow:**
1. **PREPARE** (model-callable): validate inputs → BRCode/Boleto `paymentPreview` or Transfer DICT read (§4.2, all BEFORE any create) → enforce caps (§4.1) → build immutable canonical intent (§5) in the durable store (§6), committing `REQUEST_SUBMITTING` **before** the create → call **`PaymentRequest.create`** (the approval request; SDK-confirmed it moves no money) → compare the provider-returned embedded payment+amount FIELD-FOR-FIELD to the canonical intent (§5) → store provider request id → return a public `intentId` + masked pt-BR summary (no reusable secret, no raw taxId/BRCode/key in model output).
2. **Human approves out-of-band** on StarkBank's site via the cost-center policy. **The approval executes the payment** — the model cannot approve, and there is no local step between approval and money movement.
3. **RECONCILE** (background polling, §8): observe the provider terminal (approved-executed / rejected / expired-if-proven / unknown). Polling-detected "approved" means **the money already moved** — reconciliation is AUDIT, not a dispatch gate.

**Hard invariants:**
- **Zero DIRECT money-resource create** (`transfer`/`brcodePayment`/`boletoPayment.create`) is reachable from ANY model-visible route (§4.4/§10). PREPARE's only provider create is `PaymentRequest.create`.
- **Zero money movement without a provider out-of-band approval.** The real gate is StarkBank's cost-center approval POLICY (which member may approve), enforced provider-side — NOT any local check.
- **PREPARE actor provenance (correction a):** the stdio MCP has NO trustworthy Slack/actor param from model input. The recorded actor is a FIXED configured operator principal (or an authenticated-wrapper-derived principal) — **NEVER an actor value from model input.**

## 3. SDK reality (verified vs starkbank@2.39.1) — keystone + constraints
Keystone HOLDS: `PaymentRequest.parsePayment` wraps Transfer + BrcodePayment + BoletoPayment; `create()` makes an approval request, moves no money. Constraints that shape the design:
- **3.1 taxId = RECEIVER** (all 3 resources' field docs). Current "payer" copy is WRONG — the REBUILD corrects it (§10), not a separate hotfix to the frozen tools.
- **3.2 `paymentPreview` covers BRCode/Boleto/tax/utility, NOT Transfer/pix-key.** For Transfers the pre-create read is `dictKey.get` (destination) + caller-supplied integer-cents amount (bound into the digest) — this is a LOCAL integrity input, never called a "provider preview."
- **3.3 cost-center/approver eligibility is NOT API-probeable** — no costCenter/member/org resource; `centerId` is an opaque operator-supplied string.
- **3.4 approver identity** lives only in the return-only `actions[]` array (`{type:'member', id, action}`) — no `approvedBy` scalar, no documented status/action enum.
- **3.5 no PaymentRequest webhook/Log** — approval reconciliation is polling-only.
- **3.6 `paymentRequest.query` is ASYNC and returns `Promise<AsyncGenerator>` (Cipher #3):** the real use is TWO-STAGE — `const generator = await paymentRequest.query(filters)` THEN a bounded `for-await` over `generator`; OR `await paymentRequest.page(...)`. Awaiting `query` alone does NO GET; a direct `for-await` over the unresolved promise is INVALID. A successful bounded consume proves only credentials + filter-syntax accepted — NOT center access or approver policy.
- **3.7 `PaymentRequest.create` accepts AND returns ARRAYS (Cipher #5):** one HTTP POST is not one approval request. Send a batch of EXACTLY 1 and require a response of EXACTLY 1 (§6).

## 4. Controls

**4.1 Multi-dimensional caps (fail-closed), enforced at PREPARE inside ONE serialized txn BEFORE `PaymentRequest.create`.** Caps: per-txn amount · rolling-24h amount · rolling-24h count/velocity · per-destination cooldown · **global single-inflight (singleton active request)**. Missing/invalid config → FAILS CLOSED (registration refused).
**⚠️ Global atomicity across DISTINCT intents (Cipher #1):** a per-row CAS only dedups concurrent attempts on the SAME intent — 10 DISTINCT PREPAREs could each win their own CAS, each independently see cap room, and issue 10 creates. So cap evaluation + reservation + the single winning transition happen in ONE serialized SQLite transaction (`BEGIN IMMEDIATE`) that atomically: evaluates all persisted reservations, enforces the SINGLETON active-request constraint, reserves rolling amount/count/destination/global-inflight, transitions EXACTLY ONE intent to `REQUEST_SUBMITTING`, and commits BEFORE any provider I/O (§6).
**Reservation-holding states:** `REQUEST_SUBMITTING`/`REQUEST_PENDING_APPROVAL`/`REQUEST_CREATE_AMBIGUOUS`/`PROVIDER_UNKNOWN_FROZEN`/settlement-ambiguous hold ALL reservations incl. global-inflight.
**Split-ledger release (Cipher #2 — avoids the post-first-approval deadlock):** `PROVIDER_APPROVED_EXECUTED` RELEASES the global-inflight lease IMMEDIATELY (so the next payment can prepare) BUT its spend stays counted in rolling-24h amount/count + destination cooldown until those windows age out; a PROVIDER-proven rejected/expired releases ALL reservations and contributes NO spend; ambiguous/unknown/mismatch stays fully blocking. Counters live in the durable store → survive restart; rejection is pre-provider.

**4.2 Validation is PRE-CREATE, not pre-dispatch (there is no local dispatch — Cipher B2).**
- BRCode/Boleto: `paymentPreview.create` (read-only) BEFORE `PaymentRequest.create`; reject open/unknown/zero amount (BRCode `amount:0`/`allowChange` unless an explicit expected amount matches).
- Transfer: `dictKey.get` destination read + caller integer-cents amount BEFORE create.
- After `PaymentRequest.create` AND on every poll: compare the provider-returned embedded payment + amount FIELD-FOR-FIELD to the canonical intent. **A mismatch is an INCIDENT/audit signal that FREEZES future ops — it CANNOT retroactively stop a payment the human already approved.** There is NO "re-validate immediately before dispatch → zero I/O on drift" — that step does not exist in this architecture. Amounts are integer-cents, `Number.isSafeInteger`-guarded; display via `Intl.NumberFormat('pt-BR',{style:'currency',currency:'BRL'})`.
- **Provider-side edits (B2/O5):** SDK has no `update`. A local TTL/EXPIRED is NOT preventive unless StarkBank PROVES a pending request cannot be approved after the TTL — since it can't (no cancel/update), a locally-stale request stays BLOCKING + counted until the provider shows an actual rejected/expired terminal. (Else a human could approve the old request after local caps released.)

**4.3 Idempotency (SDK-grounded).**
- The wrapped **Transfer has `externalId`** (server-enforced unique) — generate + persist BEFORE the create, reuse forever.
- **BrcodePayment/BoletoPayment have NO externalId** — idempotency is the durable intent + atomic state (§6).
- **`PaymentRequest.create` idempotency (correction d + B6):** persist a stable unique **operation tag BEFORE** the create; `PaymentRequest.create` is ALSO zero-retry (below). `tags` are correlation only, never a uniqueness mechanism.

**4.4 Zero-retry for `PaymentRequest.create`; direct money creates UNREACHABLE.** The only provider create we make (`PaymentRequest.create`) rides NO retry — a blind retry can spawn DUPLICATE approval requests a human may approve twice. The direct `transfer`/`brcodePayment`/`boletoPayment.create` methods are **removed from every model-visible route AND from the generic retry path** (fixes the R3 bug, §1). There is no post-approval dispatch to retry — approval executes provider-side.

**4.6 Money-tool REGISTRATION GATES (fail-closed — sandbox/operator proofs are GATES, not footnotes — Cipher B4).** A payment type registers ONLY if ALL hold; else that type (or the whole surface) is UNAVAILABLE:
- `STARKBANK_ENVIRONMENT` present (missing = FATAL, §4.7) + `STARKBANK_MONEY_MOVEMENT_ENABLED=true`
- valid cap config (§4.1) + durable store healthy (§6) + destination-HMAC key
- **recorded operator DASHBOARD ATTESTATION** of the exact cost center + the authorized approver policy (the eligibility proof — §3.3)
- well-formed `centerId` + expected approver member id + a bounded read-only readiness consume of `paymentRequest.query`/`page` (proves ACCESS only, §3.6)
- **O1 sanitized sandbox lifecycle/action fixtures pinned** before reconciliation code recognizes ANY terminal
- **O2 immutable provider/UI snapshot proof** before **Transfer** registers; **provider/UI exact-field proof for BRCode/Boleto** too
- **sandbox proof that an unauthorized member CANNOT approve** at the provider boundary
- **NO CLI fallback (Cipher B3):** provider ineligibility = money tools remain UNREGISTERED. The local-CLI approver is a SEPARATE fallback architecture needing its own fresh review — explicitly NOT part of this design.

**4.7 `STARKBANK_ENVIRONMENT` missing = FATAL.** Never default to production (fixes auth:25).

## 5. Canonical intent digest (integrity, not prevention-after-approval)
At PREPARE compute an immutable digest over `{op type, env+account fingerprint, integer-cents amount+currency, destination HMAC fingerprint, payment-code digest, RECEIVER-taxId digest, schedule, description, tags, policy version, expiry}`. Compare it FIELD-FOR-FIELD against the provider-returned embedded payment: (a) immediately AFTER `PaymentRequest.create`, and (b) on EVERY reconciliation poll (§8). A pre-create mismatch (preview/DICT drift) → abort with zero create. A post-create/post-approval mismatch → **FREEZE the intent + all future money requests + raise an incident; it does NOT undo an approved payment.** Never clear caps on a mismatch/unknown.

## 6. Durable transactional store (SQLite+WAL) + state machine (Cipher B1)
- File mode 0600, WAL, on the Mini.
- **State machine (approval-REQUEST lifecycle):**
  `LOCAL_PREPARED → REQUEST_SUBMITTING → REQUEST_PENDING_APPROVAL` (with a `REQUEST_CREATE_AMBIGUOUS` branch when create may have succeeded without a response), then PROVIDER-observed terminals: `PROVIDER_APPROVED_EXECUTED` / `PROVIDER_REJECTED` / `PROVIDER_TERMINAL_EXPIRED` (only if provider-proven) / `PROVIDER_UNKNOWN_FROZEN`.
  **All of {cap evaluation + singleton enforcement + reservation + the winning transition} happen in ONE `BEGIN IMMEDIATE` serialized transaction, committed BEFORE `PaymentRequest.create` (§4.1).** This is NOT a per-row CAS (which would let 10 DISTINCT intents each win their own row) — it is a single serialized reservation txn so that under N concurrent DISTINCT intents (even across connections/processes) exactly ONE transitions to `REQUEST_SUBMITTING` and issues the create; all others block on the singleton/cap constraint with zero provider I/O. This proves exactly one `PaymentRequest.create` POST — not a post-approval payment POST (there is none). `PROVIDER_APPROVED_EXECUTED` is a terminal where the money ALREADY MOVED.
  - **Array-shape assertion on create (Cipher #5):** send a batch of EXACTLY 1; require the response length EXACTLY 1; verify the returned id/tag/payment/amount vs the canonical intent BEFORE binding. A zero- or multi-entity response FREEZES the intent as ambiguous/critical and is NEVER retried.
  - **Stranded `REQUEST_SUBMITTING` after crash/restart** enters the §8 ambiguity lookup and NEVER calls `create` again.
- **Append-only immutable audit as a CONTROL (Cipher #6, DB-enforced not promised):** an append-only audit TABLE with **NO application UPDATE/DELETE code path**, PLUS SQLite `BEFORE UPDATE`/`BEFORE DELETE` triggers that `RAISE(ABORT)` — immutability is DB-enforced, not convention. Every state/cap mutation writes its event row **IN THE SAME TRANSACTION** as the mutation, so a mutation without its paired row ROLLS BACK atomically (no state/cap change persists without its audit row). Each row carries: fixed actor (§2) + provider approver action+id + before/after state + integer-cents amount + canonical digest + HMAC fingerprints + provider PaymentRequest+payment ids + status + policy version + cap counters + error class + timestamps.
- **NEVER stored (store, logs, model output):** raw BRCode, raw barcode, full PIX key, full CPF/CNPJ, private key. Destinations = keyed-HMAC; taxIds = digests.
- **OCI backup policy — PINNED CONCRETE (Peppy infra lane; fail-closed test-10 gate):**
  - **Bucket:** a DEDICATED versioned OCI bucket `starkbank-audit-replica`, ISOLATED from the general openclaw/incluir backups (financial-audit data gets its own store).
  - **Authorized principals:** (1) the operator via OCI console (human owner); (2) a single Peppy-owned rclone credential scoped **write-only** to that bucket for the cron. **NO application/MCP/model/openclaw access** to the bucket or its credential.
  - **Encryption (ASYMMETRIC — `age` only, no gpg):** the upload cron CLIENT-SIDE encrypts the SQLite dump to an **age PUBLIC RECIPIENT** BEFORE upload (plaintext financial audit never lands in OCI) + OCI server-side encryption-at-rest as defense-in-depth. Encrypted-then-uploaded, never raw. **ONLY the age public recipient lives on the Mini / upload path.**
  - **Key custody (asymmetric — a Mini compromise can encrypt+upload but can NEVER decrypt):** the PRIVATE age identity is held EXCLUSIVELY in operator-controlled OFFLINE custody — **NOT on the Mini, NOT in MCP/openclaw/Infisical runtime scope, NOT in the OCI bucket.** Restore requires EXPLICIT operator key presentation and is logged.
  - **Rotation:** create a new offline identity → switch the Mini's public recipient → preserve OLD offline identities only through the longest retained ciphertext window (90 days) → then securely retire them.
  - **Retention/deletion:** 90-day retention with OCI versioning + a lifecycle rule deleting versions older than 90 days; local dumps pruned to 14 days.
  - **Restore/access logging:** OCI bucket access logging → an OCI log; a local append-only restore-attempt log at a 0600 Mini path. No raw financial PII in either.
  - **Peppy implements + verifies this at build time as a FAIL-CLOSED test-10 gate — test 10 cannot PASS until the encrypted backup + the audit triggers are proven working.**

## 7. Provider ineligibility handling (CLI fallback REMOVED — Cipher B3)
If the account lacks a usable cost-center/approver (dashboard attestation fails, §4.6): **money tools remain UNREGISTERED.** There is no runtime fallback. A local-CLI operator approver would be a distinct architecture (its own execution boundary, auth, replay + ambiguity model) requiring a separate fresh security review — it is NOT specified or claimed here.

## 8. Reconciliation (polling-only, bounded, restart-safe — Cipher B2/B5/B6/O1/O4/O5)
- **Bounded polling by EXACT request id** once an id is bound: `const g = await paymentRequest.query({centerId, ids:[paymentRequestId], limit:1})` then bounded `for-await` over `g` (or `paymentRequest.page`) — NO broad status scans (§3.6, two-stage consume).
- **Ambiguous-create cardinality with `limit:2` (Cipher #4):** after a `REQUEST_CREATE_AMBIGUOUS`, query by the unique operation tag + a narrow creation window with **`limit:2`** (consume at most two — `limit:1` can NEVER detect a duplicate, so 0/1/2 must be observable):
  - **0 matches** → stays `REQUEST_CREATE_AMBIGUOUS`, blocks new prep, NEVER auto-recreate.
  - **exactly 1** → bind its provider request id → `REQUEST_PENDING_APPROVAL`.
  - **≥2** → CRITICAL duplicate-request incident → FREEZE all money tools → operator adjudication.
  - A **stranded `REQUEST_SUBMITTING` found after crash/restart enters THIS SAME lookup** (never re-calls `create`).
- **Approver identity/status (O1/B7-T2):** local `actions[]` parsing CANNOT PREVENT a wrong-user approval (money moved by the time we poll) — prevention is the provider cost-center POLICY. Local parsing marks `PROVIDER_APPROVED_EXECUTED` for AUDIT only when BOTH a recognized provider status AND an exact allowlisted member `action`+id are present. **Unknown/missing/duplicate/conflicting actions or an unexpected polled member → FAIL CLOSED, FREEZE future ops, raise an incident, and stay counted against caps.**
- **Snapshot compare every poll (O5):** compare embedded payment+amount vs digest (§5) each poll; divergence FREEZES the intent + future requests; never clear caps on unknown.
- **Cap release rule (IDENTICAL to §4.1 — no contradiction):** local expiry releases NOTHING; `PROVIDER_APPROVED_EXECUTED` releases ONLY the global-inflight lease immediately while rolling amount/count + destination cooldown age out on their own; a PROVIDER-proven REJECTED/EXPIRED releases pending reservations/inflight and contributes NO spend; ambiguous/unknown/mismatch RETAINS ALL reservations.
- **Restart-safe:** on start, re-reconcile every non-terminal intent (bounded, by id/tag) before accepting a new PREPARE.

## 9. Rollout
- **Sandbox E2E first.** The three sandbox/operator proofs (§4.6: O1 fixtures, O2 Transfer + BRCode/Boleto snapshot/UI immutability, unauthorized-can't-approve) MAY be collected during build ONLY because the design makes them explicit fail-closed registration gates — **no registration/implementation clearance for an affected payment type until its evidence is pinned.**
- **No real-money canary** without SEPARATE explicit operator authorization AND an operator-owned destination.

## 10. Current→target corrections (folded into the REBUILD — frozen tools untouched until it ships whole)
- taxId current(PAYER, wrong)→target(RECEIVER). `STARKBANK_ENVIRONMENT` current(`?? 'production'`)→target(missing=FATAL). Retry: current(creates on Google-shaped `withRetry`; Axios 429/502/503 DO retry today — real double-pay risk)→target(direct money creates unreachable from model routes + removed from retry; `PaymentRequest.create` zero-retry). Actor: current(none)→target(fixed configured operator principal, never model input). Add durable store + caps + digest + registration gates.

## 11. Open items — RESOLVED by Cipher; remaining = sandbox evidence gates
O1 approver strings, O2 Transfer snapshot proof, O3 dashboard attestation, O4 bounded-poll, O5 no-update-freeze — all folded into §4/§5/§6/§8. **taxId=RECEIVER accepted.** The remaining items are the §4.6 fail-closed EVIDENCE GATES (dashboard attestation, O1 fixtures, O2/BRCode/Boleto snapshot proofs, unauthorized-can't-approve) — each blocks registration of its payment type until pinned in sandbox. Operator dashboard answer (primary eligibility) inbound via Chrome.

## 12. Acceptance-test mapping — revised to PROVIDER REALITY (Cipher B7)
The tests are corrected to match "approval executes the payment" — NOT claimed verbatim-satisfied where the verbatim wording assumed a local dispatch.

| # | Test (revised to provider reality) | Design section(s) |
|---|---|---|
| 1 | PREPARE DOES make a provider `PaymentRequest.create`; the invariant is **zero DIRECT money-resource create + zero money movement without provider OOB approval** (not "no provider create"). Preview/DICT repeatable, read-only. | §2, §4.4 |
| 2 | Wrong-user/team approval → zero money is enforced by the provider COST-CENTER POLICY, not local parsing (money moved by poll time). Test: an unauthorized member CANNOT approve at the PROVIDER boundary (sandbox, §4.6). An unexpected polled member/action → FREEZE future ops + incident. Forged/replayed local "approval" or chat code is not an approval channel → zero I/O. | §4.6, §8, §2 |
| 3 | Unapproved/mutated → zero create (pre-create digest/preview). Expired/cancelled zero-money guarantees require a PROVIDER terminal; **local expiry alone NEVER releases caps.** | §5, §4.2, §8 |
| 4 | Ten concurrent **DISTINCT** intents (across connections/processes) → exactly ONE `PaymentRequest.create` POST via the single `BEGIN IMMEDIATE` serialized reservation txn (not a per-row CAS); create request batch len 1 + response len 1 asserted before bind; no cap exceeded. | §4.1, §6 |
| 5 | Missing store/limit/env/approval-attestation/evidence config → fails registration or request closed. | §4.6, §4.7, §4.1 |
| 6 | Caps/velocity survive restart; rejection is pre-provider (before the create). | §4.1, §6 |
| 7 | The wrapped Transfer persists/reuses exact externalId. A `PaymentRequest.create` ambiguity stays `REQUEST_CREATE_AMBIGUOUS`, never auto-recreates; recovery uses `limit:2` tag/window cardinality; create request/response array length exactly 1 (zero/multi → freeze). | §4.3, §4.4, §6, §8 |
| 8 | Crash/restart at each REQUEST-lifecycle boundary never duplicates the `PaymentRequest.create`; a stranded `REQUEST_SUBMITTING` re-enters the `limit:2` ambiguity lookup and never re-creates. | §6, §8 |
| 9 | Preview/DICT reads prove zero creates BEFORE `PaymentRequest.create` (BRCode/Boleto→`paymentPreview`; Transfer→DICT + caller-amount-rehash). Snapshot/UI immutability proof replaces "re-preview before dispatch" (which doesn't exist). | §4.2, §3.2, §4.6 |
| 10 | Append-only audit: a direct UPDATE AND a direct DELETE against the audit table both FAIL (BEFORE-triggers `RAISE(ABORT)`); any state/cap mutation without its paired event-row insert ROLLS BACK atomically; privacy grep finds no raw financial PII (store+logs+backup); asymmetric-age backup proven — (i) the Mini contains NO private age identity (scan proves absence), (ii) an encrypted fixture restores ONLY when the operator-held test identity is supplied and FAILS without it; `starkbank-audit-replica` access/retention/restore-logging policy proven working (Peppy build-time fail-closed gate). | §6 |
| 11 | Sandbox E2E first; no real-money canary without separate operator auth + operator-owned destination. | §9 |
