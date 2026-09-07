---
name: verify-user-path
description: Verify an explicitly scoped primary browser journey or consequential browser-only regression on the exact candidate. Choose smallest sufficient evidence, isolate runner failures, preserve durable safe receipts, and record release risk acceptance honestly. Not an every-control E2E requirement or a trigger for extra tests at every PR.
---

# Verify the Required User Journey

Operator retrospective 2026-09-07 supersedes this skill's older every-surface/manual-plus-E2E mandates. Runtime evidence matters, but duplicating it does not make it stronger. This skill applies when the assigned acceptance requires a real user journey; it does not turn every unit/component change or PR-open into a browser campaign.

## 1. Select the smallest faithful evidence

- Ordinary UI logic (editing, paste/delete, normalization, reveal state, labels, submit payload): unit/component tests by default.
- API/auth/storage/adapter contracts: focused integration tests across the actual relevant boundary, not a mock that bypasses the suspected failure.
- E2E: the normal accepted user journey, plus a consequential reproduced case only when lower tests cannot represent it faithfully and the added scope is approved.
- Real Chromium driven by Playwright is browser evidence. Do not run a second manual walk merely because the first was automated. Operator testing is valid when its scope/revision are known; do not invent those facts.
- Unit tests cannot prove browser caret/WS/mobile feel. Fake-provider tests cannot prove paid-provider integration or human-audible output. State the gap rather than silently opening a new test track.

Historical motivation: aperture-w9v2 and aperture-w537 missed frontend proxy routes despite backend tests passing. The lesson is to cover the actual submission path, not every control or every layer twice. `e2e-catches-what-lower-cant` explains test-apparatus blind spots.

## 2. Write the bounded journey before running

Record candidate/build, environment/role, fixture source, required outcome, assertions and stop condition. Reuse an existing runner and its known assertions; validate needed fixture state and target isolation first. No secret values in model output or traces. A scoped test is not authorization for production writes, provider calls or credential retrieval.

Example auth journey: ordinary CPF/password submit -> authenticated identity/role -> protected access -> logout -> former session rejected. Optional reveal/hide detours and rapid-input variations are not prerequisites unless acceptance or an approved consequential defect requires them.

Observe the outcome and the minimum relevant durable state: e.g. a successful mutation followed by an authorized read. Do not query every DB/audit/email layer for a simple navigation check. An HTTP200 alone does not prove a user outcome, but no additional probe is implied beyond authorization.

### Test-walker credentials for prod walks (banked 2026-05-25)

**TWO permanent test-walker users live on prod.** Use the one that matches your verification surface.

| User | Email | mempalace drawer | role | Use when |
|---|---|---|---|---|
| **staff walker** (o0kt) | `test-walker@programaincluir.org` | `drawer_peppy_secrets_099780bfab08d98a8dcb5a33` | `user` | Walking surfaces gated on staff permissions (`gestao_de_pessoas`, `secretaria`, `coordenacao_de_ensino`, `financeiro`) — the COMMON case |
| **admin walker** (9yaa) | `test-walker-admin@programaincluir.org` | `drawer_peppy_secrets_38d8c201c77b8c01ef881e71` | `admin` | Walking surfaces gated on `user.role='admin'` (e.g. AdminShell-protected pages, admin-only escape hatches, `?reveal=full` on volunteer-applications detail) |

**Decision rubric — which walker to use:**
- Default to **staff walker** — covers most surfaces + has narrower blast-radius if credentials leak
- Switch to **admin walker** only when the assigned acceptance explicitly requires an admin surface and use is authorized; a 403 alone is not escalation permission
- Use BOTH if a single E2E walk crosses admin + non-admin surfaces (e.g. user-as-volunteer creates a thing, admin-as-staff reviews it)

**Obtaining the credential — NON-MODEL delivery only (standing rule `credential-drawer-plaintext-read-ban`, Cipher 2026-08-28, binding):**

Do NOT read the drawer with a model-visible tool. No `get_drawer`, no `search`, no `cat` of a secrets file, no path that puts the password into agent context — every such read persists plaintext into a session transcript on disk (13 transcript copies of one shared prod key were found this way, and the count grew while it was being investigated). The drawer ids in the table above are *escrow pointers for the helper*, not something to open.

Use the authorized existing non-model delivery contract: name the **logical secret** (`test-walker` or `test-walker-admin`) and an **approved, allowlisted destination or action** (e.g. write a Playwright `storageState` / `.env` for a walk, mint a session at the exact origin the walk will use), and let the approved existing non-model integration move the value from the store to that destination. The agent receives **the approved non-secret receipt only** — never the value.

**If the approved existing non-model integration is not available in your session: STOP and ask Peppy/GLaDOS. Do not improvise a substitute** (no drawer read, no copy-paste, no ad-hoc script that prints the value). A walk that cannot be authenticated without a model-visible read is blocked, not worked around — record it as blocked on the bead.

The drawer still holds, for the helper: email + password + CPF + BetterAuth `user_id` + `volunteer_id` + permissions list + surfaces this user CAN/CANNOT walk + browser auth pattern + API auth pattern + rotation procedure. What the agent may hold in context is only the non-secret half: email, role, `user_id`, `volunteer_id`, permissions, surfaces.

**Why two sibling drawers (not one extended drawer):**
- Reading "which user has admin?" can't get confused — different drawers, distinct entries
- Independent rotation paths — rotating staff walker doesn't touch admin walker creds
- Failure-mode isolation — admin walker compromise doesn't auto-leak staff walker

**Why mempalace drawer storage (not inline-in-skill):**
- Credentials stay out of the aperture repo (no risk of secret-shaped strings tripping GitGuardian or leaking through `git log`)
- Rotation happens without skill-file edits (Peppy regens; drawer updates; skill pointer stays valid)
- Only the approved non-model helper touches the value; agents hold the pointer, never the contents

**Banked precedents:**
- **aperture-o0kt** (2026-05-25): Peppy created staff walker after the morning prod-walk discipline shipped. DB tagged `volunteers.observation = 'Aperture swarm test-walker user. DO NOT DELETE. See bead aperture-o0kt.'`
- **aperture-9yaa** (2026-05-25): Peppy created admin walker after Izzy's hcvt walk surfaced the admin-board access gap (test-walker has 4 staff perms but no admin role, so admin-shell-gated surfaces redirected to /home). DB tagged similarly with `aperture-9yaa` reference.

**Rotation:** Ping Peppy if credentials seem compromised OR the password expires. Don't hand-rotate without coordination — the user_id/volunteer_id stay stable; only the password changes; drawer updates after. Each walker rotates independently.

## 3. Classify and stop correctly

- Mandatory flow failure: record the exact failed assertion, not a generic AssertionError, and stop dependent steps.
- Optional issue: record a non-blocking finding; when safe and within scope, continue the independent required journey without that detour. Never label later unrun steps as failed.
- Runner defect: distinguish oracle/setup failure from product failure. Within an unchanged authorized run scope, permit one bounded correction using an established condition assertion, not sleeps/timing sweeps. Explicit STOP, one-shot or no-retry instructions override this default.
- Wrong target, credential exposure, unauthorized side effect or unsafe fixture state: stop immediately and escalate; do not rerun or work around it.
- Scope complete: stop. A new revision does not justify a broad rerun; test the changed boundary only, plus any genuinely invalidated acceptance evidence.

## 4. Durable receipts before cleanup

Persist a safe artifact before terminating an ephemeral runner/container: exact candidate/build, journey stage, assertion code, PASS/FAIL/NOT_RUN and outcome. Capture only approved non-secret evidence; no password, session, provider key, request body or credential-derived diagnostics. Check receipt availability before removing the runner. Truncated/lost output is missing evidence, not PASS.

## 5. Review and repair

Review only the assigned acceptance. For a bounded easy/medium defect the finder may fix it after current-owner file consent under `communicate` §10, then hand the diff and focused regression to an independent reviewer. Architectural/security/infra or unclear-contract issues remain routed. Do not make every defect a P0 or file new beads yourself.

Inspect route contracts when the change affects routing; do not repeat a whole route audit on unrelated input logic.

## 6. Report without inflating evidence

One concise result per exact candidate:

- Required journey: what actually ran and its outcome.
- Evidence: artifact link, test layer, target/build and safe assertion codes.
- Known limitations: optional issues and NOT_RUN steps, classified against acceptance.
- Disposition: PASS, HOLD with the specific required gap, or explicit operator risk acceptance naming its limits.

A reviewer cannot rewrite a failed test as PASS. An operator can accept a named release risk; record that decision separately. Manual success on an unknown revision is not proof of a particular artifact. API-cycle PASS is not browser-cycle PASS.

## 7. Do not

- Block login/logout evidence behind a non-required visibility toggle.
- Require an E2E for every CTA, form control, viewport or small UI PR.
- Launch another agent merely to duplicate a completed browser walk.
- Use retry loops, fixture writes or paid calls beyond the authorized boundary.
- Treat optional findings as new mandatory release criteria after implementation.
