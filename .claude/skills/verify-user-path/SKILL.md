---
name: verify-user-path
description: Manual user-path verification before sign-off — NON-NEGOTIABLE for Izzy and any agent gating a feature's promotion to "done" or "ready for operator". Open the production URL in a browser, walk the user path, use the actual feature, verify the network response and the DB/audit/log trail — CI green is necessary but NOT sufficient. Triggers on any sign-off claim, "Gate 7" review, "approved" comment, "ready for review" status, close-on-PR-open for user-facing features.
---

# Verify User Path — Manual Walk Before Sign-Off

You are about to sign off a feature. Or you are about to declare a PR ready for review. Or you are about to close a bead with "shipped + tested." Stop. **Did you walk the user path in a real browser?**

If the answer is "no, but CI passed" or "no, but my Playwright E2E went green" or "no, but the code review looked clean" — your sign-off is **incomplete**. CI catches some bugs. Code review catches others. Playwright catches yet others. **None of them catch the bug class where the user clicks Submit and gets a 404 because the frontend code calls a URL that doesn't exist on the backend.**

This skill is non-negotiable for **Izzy** (E2E + functional QA + final QA-gate sign-off) and **Peppy** (post-deploy verify). It is also recommended for **Vance** + **Rex** + **Cipher** at PR-open time for any user-facing change.

---

## 1. The Failure Mode This Prevents

Two banked precedents in 24h (2026-05-24 → 2026-05-25):

**Precedent #1: `aperture-w9v2` broken-image** (Vance lvzh F1, 2026-05-24)
- PR shipped + CI green + Cipher sign-off recorded + auto-merged
- Operator walked the feature → screenshot uploaded fine → preview rendered as **broken image**
- Root cause: `apps/frontend/src/app/api/files/` Next.js proxy route DID NOT EXIST. Browser hit Next.js → 404 → broken image fallback. The hono backend route was perfect; the Next.js proxy was missing.
- Caught only because operator opened the page in a browser.

**Precedent #2: `aperture-w537` submit-404** (Vance lvzh F1, 2026-05-25)
- PR shipped + CI green + Cipher sign-off + Atlas docs + Sterling conditional approval + auto-merged
- Operator walked the feature → submitted the form → **404 from Next.js**
- Root cause: `apps/frontend/src/app/api/volunteer-applications/route.ts` proxy DID NOT EXIST. Same exact bug class as w9v2, one day later.
- Caught only because operator submitted in a browser.

**Operator quote (2026-05-25):**

> "again you guys are sending things to prod and not doing any testing if it works or not, we need improve sterling and izzy skills so that they test the path manually... this cannot happen"

This skill is the response. **No more "shipped + tested" claims based on CI + code review alone for user-facing features.**

---

## 2. What "Walk the Path" Means

The user-walk has FOUR layers. All four must pass.

### Layer A — Open the URL in a real browser

Not curl. Not Playwright. A real browser (Chrome/Safari/Firefox) hitting the production OR staging URL. Cookie-authenticated as the role the user would hold (or anonymous if it's a public surface).

```bash
# If you can't open a browser yourself (CLI-only agent), use the operator's session OR your own session
# OR delegate to operator with the explicit ask: "open <URL>, do <action>, screenshot the response"
```

For agents without browser access: **dispatch a subagent with `playwright-mini` MCP tools** to drive a real headless browser. The subagent's playwright is a real browser, not a mock.

### Test-walker credentials for prod walks (banked 2026-05-25)

**TWO permanent test-walker users live on prod.** Use the one that matches your verification surface.

| User | Email | mempalace drawer | role | Use when |
|---|---|---|---|---|
| **staff walker** (o0kt) | `test-walker@programaincluir.org` | `drawer_peppy_secrets_099780bfab08d98a8dcb5a33` | `user` | Walking surfaces gated on staff permissions (`gestao_de_pessoas`, `secretaria`, `coordenacao_de_ensino`, `financeiro`) — the COMMON case |
| **admin walker** (9yaa) | `test-walker-admin@programaincluir.org` | `drawer_peppy_secrets_38d8c201c77b8c01ef881e71` | `admin` | Walking surfaces gated on `user.role='admin'` (e.g. AdminShell-protected pages, admin-only escape hatches, `?reveal=full` on volunteer-applications detail) |

**Decision rubric — which walker to use:**
- Default to **staff walker** — covers most surfaces + has narrower blast-radius if credentials leak
- Switch to **admin walker** ONLY when the surface returns 403 / redirect to /home / admin-shell-gate observed with staff walker
- Use BOTH if a single E2E walk crosses admin + non-admin surfaces (e.g. user-as-volunteer creates a thing, admin-as-staff reviews it)

**Obtaining the credential — NON-MODEL delivery only (standing rule `credential-drawer-plaintext-read-ban`, Cipher 2026-08-28, binding):**

Do NOT read the drawer with a model-visible tool. No `get_drawer`, no `search`, no `cat` of a secrets file, no path that puts the password into agent context — every such read persists plaintext into a session transcript on disk (13 transcript copies of one shared prod key were found this way, and the count grew while it was being investigated). The drawer ids in the table above are *escrow pointers for the helper*, not something to open.

The only permitted path is the non-model delivery contract: name the **logical secret** (`test-walker` or `test-walker-admin`) and an **approved, allowlisted destination or action** (e.g. write a Playwright `storageState` / `.env` for a walk, mint a session at the exact origin the walk will use), and let the approved helper move the value from the store to that destination. The agent receives **status + fingerprint only** — never the value.

**If the approved helper is not available in your session: STOP and ask Peppy/GLaDOS. Do not improvise a substitute** (no drawer read, no copy-paste, no ad-hoc script that prints the value). A walk that cannot be authenticated without a model-visible read is blocked, not worked around — record it as blocked on the bead.

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

### Layer B — Perform the actual user action

Click the button. Submit the form. Upload the file. Trigger the AI. Whatever the feature IS — do it as a user would.

DO NOT skip this with "the unit test exercised the function." Unit tests run in Node, not a browser. Browsers have:
- Origin checks
- Cookie policies
- CORS
- Service workers
- Next.js middleware
- Subdomain auth
- localStorage shapes
- All the integration layers your unit tests bypass

### Layer C — Observe the network response

Open browser devtools (Network tab). Verify:
- The request fired against the URL you expected
- The status code is what you expected (200 / 201 / etc., NOT 404 / 500 / CORS-blocked)
- The response body is the shape your code expects (JSON parse, no HTML 404 page)
- Any chained requests (analytics, audit, follow-up GETs) all returned green

### Layer D — Verify the side-effect trail

Did the action actually do what it claimed? Check the durable evidence:
- **DB row** — query the database for the row that should have been inserted/updated. Use `incluir-prod-postgres` skill or equivalent. (Wait — production-prod-postgres is operator-driven; use staging-prod-postgres for non-operator probes.)
- **Audit event** — query Loki for the structured log line (e.g. `volunteer_application.submitted`). Use `observability-query` skill.
- **Email send** — check Mailtrap (or equivalent) for the actual delivered email if the feature triggers one.
- **Storage object** — check MinIO/S3 for the uploaded file if the feature involves upload.
- **State transition** — check the next read returns the new state (refresh the page, query the API as the next-step user).

If any of Layers A-D fails: **the feature is NOT done.** Re-open the bead. File a P0/P1 follow-up. DO NOT close-on-PR-open.

---

## 3. QA-Gate Sign-off Discipline (Izzy)

You are the QA gate (Izzy) — the final sign-off. The operator should never have to find a prod-broken state that you missed.

**On EVERY QA-gate (Izzy) sign-off bead (historically St1 / 6aqw / o7y0 / rark / nwqq / 1gcw etc.):**

1. **List every user-facing surface the feature touches.** Public page? Authenticated dashboard? Mobile viewport? Admin panel? Email-triggered link? Each is a separate walk.
2. **Walk each surface** per Layers A-D above. Document the walk on the bead notes — URLs, screenshots, response codes, DB-row IDs, audit-event timestamps.
3. **NEVER record "conditional approval" without documenting what's conditional.** If you can't walk it yourself (operator-only credential, missing env, etc.), say so EXPLICITLY in the bead notes and the operator-walk checklist. Don't leave the gate ambiguous.
4. **Refuse sign-off if Layer A is impossible.** If the feature isn't actually reachable in a browser (DNS not propagated, deploy not complete, env var missing), the feature is NOT done. Block the sign-off; file a P0 for the deploy gap.

**Banked failure mode (Sterling, 2026-05-25):** The nwqq conditional approval was correctly held open pending operator visual walk. That part is RIGHT. The part that's WRONG: Sterling could have walked it herself FIRST (she has the prod URL, she had cookies via session-sharing or could test-cookie her way in). The operator caught what Sterling didn't drive. Next time: walk it first, surface to operator only when YOU've verified the green-state OR found the breakage.

---

## 4. Izzy-Specific Discipline

Your E2E suite is a layer ABOVE this skill, not a substitute for it.

**On EVERY Izzy E2E (Q1) bead:**

1. **Run the E2E in CI.** Existing discipline. Keep doing it.
2. **AND walk the feature in a real browser** before close-on-PR-open. Yes, you ran Playwright. Yes, vitest is green. Open a real browser anyway. Submit a real request. Watch the Network tab.
3. **Cross-check the E2E against the user-walk.** If your Playwright test passes but the user-walk fails, that's a Playwright-vs-reality drift — file a follow-up to extend the E2E to catch what the walk caught.
4. **Run the E2E against a production-equivalent build**, not just dev. dev != prod (env vars, CORS posture, Next.js build mode, image optimization, etc.). The w537 failure was reachable only post-build because dev has different proxy behavior than prod's static build.

**Banked failure mode (Izzy, 2026-05-25):** The oadk E2E for v1.2 multi-attachment passed CI + Sterling-approved + operator-untested. The TESTS were correct. The PRODUCTION USER-WALK was not run by Izzy. Next time: tests green → AND a 5-minute browser walk → AND only then close-on-PR-open.

---

## 5. The "audit-route-contract" Cross-Reference

The `aperture:audit-route-contract` skill covers the SPECIFIC failure mode that bit twice: frontend HTTP calls pointing to non-existent backend routes. **Read it.** It teaches the grep-pattern + the "for every fetch URL, verify a Next.js + Hono handler exists" discipline.

**Add to your sign-off checklist:** for any user-facing PR, run the audit-route-contract recon BEFORE you sign off. It takes 5 minutes and catches the entire silent-404 bug class.

---

## 6. The Output Discipline

When you sign off, your bead notes MUST include:

```
═══ MANUAL USER-WALK ═══
- Surface 1: <URL> — walked as <role> — <result> (screenshot/devtools-evidence: <link or inline>)
- Surface 2: <URL> — walked as <role> — <result>
- ...
- Layer D verifications: DB row id=<x>, audit event ts=<y>, email send-id=<z>
- Issues found: <list, or "none — all surfaces green">
```

If you skip this section, your sign-off is INCOMPLETE and the bead acceptance criteria are not met.

---

## 7. Anti-Patterns

| Don't | Why |
|---|---|
| "CI green + Cipher approved → close + sign off" without browser walk | Two banked prod-breaks in 24h proved this is insufficient for user-facing features |
| Defer the walk to "the operator can test in the morning" | The operator should never find a prod-broken state you didn't try first. They test for sanity-check, not to be your first user. |
| Run Playwright + call it "manual" | Playwright is automated. "Manual" means a real human OR a verify-using-playwright-mini-MCP-subagent driving a real browser as if it were a user. |
| Skip Layer D ("the API returned 200 so we're good") | A 200 from a poorly-wired backend can mean "endpoint exists but did nothing." Verify the side-effect trail. |
| Assume dev = prod | Different env vars, different proxy posture, different build mode. w537 was reachable only in prod because of Next.js build-mode routing differences. |
| Sign off on "conditional" without writing the condition | "Conditional approval" with no documented condition is just a sign-off in disguise. State the condition explicitly. |
| Skip the audit-route-contract recon before sign-off | The whole silent-404 class catches if you grep for it. Run the recon, take 5 minutes, save the operator finding the prod-break. |

---

## 8. The Promise

Going forward, for every feature touching a user-facing surface:

**No close-on-PR-open invariant fires without a documented manual user-walk in the bead notes. No QA sign-off without documented Layer A-D verification per surface. No Izzy close without the E2E green AND a real-browser walk against a production-equivalent build.**

This is the discipline. It is not optional. The operator should never find a prod-break that one of us could have caught in a 5-minute walk.
