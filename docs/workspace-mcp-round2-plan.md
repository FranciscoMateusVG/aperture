# Plan: Workspace MCP round 2 — File Storage + Google Research

**Author:** Wheatley (planning) · **Executor:** Peppy · **For:** operator decision packet
**Date:** 2026-08-09 · **Target:** `mini:~/projects/mcp-for-claude` (Node/TS stdio MCP, openclaw). NOT git — Mini dir is source-of-truth; deploy = `npm run build` + restart openclaw.
**Constraint:** pull-based personal-assistant tooling. Two operator wants: (1) file storage, (2) Google research (cinema tickets, local services).

---

## TL;DR — what to build, what to skip, what the operator must decide

| Item | Verdict |
|---|---|
| **Google Drive toolset** | BUILD — but GATED on operator re-consent (token lacks Drive scope). |
| **Fix broken Google Tasks tools** | DO IT — free rider on the same re-consent (confirmed live 403 bug). |
| **Places API (local services)** | BUILD — the one structured research tool that beats generic web. |
| **Custom Search JSON API** | SKIP — closed to new customers, retires Jan 2027. Dead end. |
| **Cinema/showtimes tool** | DON'T BUILD — no clean API (BR or global); generic web_search/web_fetch already covers it. |
| **Generic "research" (most queries)** | ALREADY WORKS — openclaw has web_search + web_fetch; no new tool needed. |

**Operator must decide 3 things** (see §4): (a) approve the single re-consent + its scope set; (b) `drive.file` vs full `drive` scope; (c) approve a Google Cloud billing account for the Places API key.

---

## 1. File Storage — Google Drive toolset (GATED on re-consent)

### The gating fact (verified live on the Mini, 2026-08-09)
Exchanged the existing `GOOGLE_REFRESH_TOKEN` → access token → `tokeninfo`. **Granted scopes are ONLY:** `mail.google.com`, `calendar`, `contacts`. **No Drive scope. No Tasks scope.** Peppy independently confirmed with live API calls: Drive `files.list` → **HTTP 403 insufficient scopes**; Tasks `tasklists` → **HTTP 403** (see §1.1).

**Consequence:** the Drive code is incremental (new `drive.repository.ts` + `drive.tool.ts` reusing the wired OAuth client), but it is **dead code until the operator re-consents** to re-mint the refresh token with Drive scope added. Scopes are baked at consent time; they cannot be added to an existing refresh token.

### 1.1 FREE RIDER — the Tasks tools are currently broken
The inventory lists Google Tasks tools (create/get/complete), but the token has no `tasks` scope → **they 403 in production right now** (confirmed live). This is an existing latent bug independent of the new feature. Because Drive forces a re-consent anyway, **fold `tasks` scope into the same re-mint** — one consent action fixes the broken tools AND unlocks Drive. Frame to operator as its own line: *"3 tools you already have are silently broken; this same action fixes them."* — the easy yes.

### 1.2 Scope set for the single re-mint
Preserve what works + add the two gaps:
- `https://mail.google.com/` (keep — Gmail)
- `…/auth/calendar` (keep)
- `…/auth/contacts` (keep)
- `…/auth/tasks` (**NEW — fixes the 403 bug**)
- `…/auth/drive.file` **or** `…/auth/drive` (**NEW — file storage; operator decision, see §1.3**)

### 1.3 OPERATOR DECISION — `drive.file` vs full `drive`
This is the load-bearing scope choice and it maps directly to *what the assistant can see*:

| | `drive.file` (RECOMMENDED default) | full `drive` |
|---|---|---|
| Assistant can access | ONLY files it created or the operator explicitly opened with it | ALL of the operator's existing Drive |
| "Store this receipt / here's a doc I made" | ✅ works | ✅ works |
| "Find my tax document from last year" (pre-existing file) | ❌ can't see it | ✅ works |
| Consent screen | mild | **scary** ("see and delete ALL your Drive files") + possible Google app-verification friction |
| Blast radius if compromised | assistant's own files only | entire Drive |

**Recommendation:** default to **`drive.file`** — matches the stated "file storage" use case (assistant creates/manages its own files), narrow blast radius, gentle consent. Opt up to full `drive` ONLY if the operator explicitly wants the assistant to read/organize his *pre-existing* Drive — with the blast-radius tradeoff made explicit. Don't default to the scary scope.

### 1.4 Drive tool surface (build after re-consent)
`drive.repository.ts` (REST via the existing OAuth client) + `drive.tool.ts`:
- `drive-list` (recent / by folder), `drive-search` (by name/type — NOTE under `drive.file` this only sees app-created/opened files, which is correct for the use case), `drive-upload` (bytes/local path → Drive), `drive-download` (file id → bytes), `drive-create-folder`, `drive-get-link` (shareable link, with explicit permission setting — default private, opt-in link sharing).
- Pull-based, on-demand; no background sync.

### 1.5 Sequencing (re-consent is a hard prereq gate)
0. **Operator re-consents** with the full §1.2 scope set → new refresh token replaces the one in `.env` (keep the `.env.bak` discipline).
1. **Verify** the new token via `tokeninfo` — assert all 5 scopes present (same probe used for this recon).
2. **Build** `drive.repository.ts` + `drive.tool.ts`; deploy.
3. **Free regression check** (Peppy's, post-re-mint): the Tasks tools now return 200 not 403 — proves the re-mint took.

---

## 2. Google Research — build exactly ONE tool (Places), lean on existing web for the rest

### The key reframe: generic research ALREADY works today
openclaw has `web_search` + `web_fetch` in its tools profile. So "research cinema tickets / general lookups" **works right now at the assistant level with no new tool.** The only thing a purpose-built tool can beat generic web on is a **proprietary structured dataset** — which exists for exactly one of the operator's two examples.

### 2.1 BUILD — Places API (New) for "local services near me"
This is the one case where a structured tool genuinely wins: Google owns a proprietary dataset (ratings, review counts, open-now hours, phone, distance-ranking) that generic web-scraping cannot cleanly or reliably replicate.
- **Endpoints:** Text Search (`places:searchText`, natural-language "electrician near Copacabana"); Nearby Search (`places:searchNearby`, the only one that server-side ranks by `DISTANCE`); Place Details (enrich one result).
- **Fields the operator wants** (rating, userRatingCount, currentOpeningHours/open-now, nationalPhoneNumber, websiteUri, address, location) — all available.
- **BR settings (mandatory for good results):** `languageCode=pt-BR`, `regionCode=BR`; query in Portuguese; prefer `internationalPhoneNumber` (+55…) for click-to-call.
- **Tool surface:** `places-search` (text query + optional location/radius → ranked list with the structured fields), optionally `place-details` (id → full record incl. reviews). On-demand, pull-based.
- **Coverage caveat (BR):** strong for storefront businesses in metros; thinner/staler for informal tradespeople (independent electricians may lack hours/website). Degrade gracefully — return what's present, don't assume all fields populated.

### 2.2 SKIP — Custom Search JSON API
**Closed to new customers as of 2025**; existing users supported only until **Jan 1, 2027**, then retired (Google points to Vertex AI Search). Even if available, it's a paid, rate-capped reimplementation of web search returning the same title/link/snippet the assistant's generic `web_search` already gives. **No advantage. Do not build.**

### 2.3 DON'T BUILD — cinema/showtimes tool
No clean official API exists: Google Showtimes has no public API; Fandango's is gated/approval-only + US-centric; BR chains (ingresso.com, Cinemark BR, Kinoplex, Cinépolis) have **no public APIs**. A "cinema tool" would just scrape showtime pages — exactly what generic `web_search`/`web_fetch` already does. Leave cinema to the existing web tools.

---

## 3. Inventory sharpening (Peppy's "anything else while we're in there")
- **The Tasks 403 bug (§1.1)** is the one concrete find — fixed by the re-consent, verified by the regression check.
- **Recommendation:** treat the re-consent moment as a cue to **verify-against-reality the whole inventory** — a quick live probe that each of the ~40 tools actually authorizes (the Tasks bug was invisible until probed). Cheap insurance; the same access-token-probe pattern that caught Tasks. Not a blocker, just good hygiene while the hood's up.

---

## 4. Consolidated operator decision packet (what Peppy takes to the operator)
1. **Re-consent approval + scope set.** "To add file storage AND fix 3 currently-broken Tasks tools, re-authorize Google once with scopes: mail + calendar + contacts (existing) + tasks (fix) + drive.file (new)." One action, two wins.
2. **`drive.file` vs full `drive`** (§1.3). Recommend `drive.file` (assistant's own files; gentle consent). Full `drive` only if he wants the assistant to read/organize his *existing* Drive — with the blast-radius tradeoff stated.
3. **Places API billing account.** Requires a Google Cloud project with **billing enabled** (a card on file) even for free-tier use. Runtime cost for a single operator is **negligible** — ~1,000 free Enterprise-tier calls/month, then ~$35/1,000; realistically $0–few dollars/month. The cost is *setup friction* (billing card + enable the *New* Places API), not ongoing spend. Approve provisioning a key?

**Not requiring a decision (just doing it):** generic research stays on the existing web_search/web_fetch; Custom Search and a cinema tool are explicitly NOT built.

---

## 5. Suggested build order
1. **Re-consent** (operator) → verify 5 scopes → **Tasks bug fixed** (immediate win, no code) + **Drive tools** built.
2. **Places tool** (parallel-able; independent of the Google OAuth re-consent — it uses a separate Cloud API key, not the user-OAuth token) once the operator approves the billing account.
Order is flexible; the two tracks share no code. Re-consent gates Drive+Tasks; billing-account gates Places.

## Open verification notes (Phase-0 discipline for the builder)
- Before building Drive tools: confirm the re-minted token's scopes via `tokeninfo` (assert all 5).
- Before pinning Places request shapes: hit the live Places API (New) with a real BR query (`languageCode=pt-BR`, `regionCode=BR`) and confirm the field set + that it's the *New* API enabled (not legacy Places).
- Places uses a **Cloud API key** (separate from the user-OAuth token) — keep it in `.env` with the existing key-handling convention; restrict the key to Places API.
