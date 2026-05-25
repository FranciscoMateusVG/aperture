---
name: incluir-intake-to-bead
description: End-to-end pipeline for turning approved Incluir /reportar user_reports into well-shaped BEADS tasks. Use when the operator says "find approved reports", "file these as beads", "any new feature requests", or asks you to process the AI intake queue. Triggers on user_reports, intake, /reportar, approved reports, "let's file this as a bead", admin reports board, AI intake pipeline. Covers querying prod Postgres, fetching attachments from MinIO, BEADS filing discipline (title-to-body pairing + provenance + artifacts), specialist routing, and status writeback when the bead closes.
---

# Incluir Intake → BEADS Pipeline

The Incluir /reportar feature lets staff submit bug reports and feature requests through an AI-mediated chat. Reports flow through `draft → draft_pending_confirm → pending → approved | rejected`. When an approved report represents real work that needs to land in the team queue, **GLaDOS is the intake handler** — pulling the row, attachments, and metadata out of prod and turning it into a well-shaped BEADS task assigned to the right specialist.

This skill is the canonical pipeline for that handoff. It also banks the failure modes seen in real intake batches so they don't recur.

---

## 0. The Pipeline (one diagram)

```
              Prod Postgres        MinIO         BEADS         Wheatley         Operator       Specialist        Prod Postgres
              ─────────────        ─────         ─────         ────────         ────────       ──────────        ─────────────
Approve  →  user_reports='approved'  →  attachments  →  bead created  →  plan added  →  approves plan  →  reassigned  →  admin_notes updated
                  │                          │                │                │                │                 │                  ↑
                  │                          │                │                │                │                 │                  │
                  └─ (1) query ─ (2) fetch blobs ─ (3) file ─ (4a) plan ─ (4b) approve ─ (4c) route ─ (5) writeback ┘
```

| Stage | What happens | Owner | Tools |
|---|---|---|---|
| 1 | Query prod for approved reports | GLaDOS | `incluir-prod-postgres` |
| 2 | Fetch attachment blobs from MinIO `user-reports` bucket | GLaDOS | recipe §4 |
| 3 | File one BEADS task per report — verbatim PT-BR body, provenance, attachment artifacts | GLaDOS | `create_task` + filing §5 |
| **4a** | **PLANNING GATE — bead assigned to Wheatley. He researches, sharpens scope, resolves open questions, verifies dependencies actually exist in the repo, tightens acceptance criteria.** | **Wheatley** | his planning lane |
| **4b** | **Wheatley bounces the plan back to GLaDOS. GLaDOS surfaces the plan to the operator for approval.** | **GLaDOS** | `send_message` to operator |
| **4c** | **On operator approval, GLaDOS reassigns the bead to the right specialist per the routing matrix §6.** | **GLaDOS** | `update_task assignee:` |
| 5 | When the specialist's bead closes (PR-opened), append `BEADS: <id> shipped <date> — <pr-url>` to `user_reports.admin_notes` so the feedback loop closes | GLaDOS | SQL §7 |

**The planning gate is the centerpiece of this flow.** Intake-derived asks arrive with operator urgency rankings (low/medium/high) and PT-BR description quality that varies — sometimes thorough, sometimes one sentence with attachments. Wheatley turns that raw intake into a spec a specialist can claim and execute against, without losing days to back-and-forth clarification. Skipping the planning gate is the failure mode that produces half-specified beads that stall.

Stages 1–3 typically run in one operator interaction ("file these as beads"). Stage 4a runs asynchronously in Wheatley's queue. Stage 4b–4c run when Wheatley pings back. Stage 5 runs ambient, on PR-opened events from intake-derived beads.

---

## 1. When to invoke this skill

**Strong triggers:**
- Operator: "I approved a bunch of requests, see if you can find it", "file these as beads", "process the intake queue"
- You're scanning `/home/admin/reports` and see new `status='approved'` rows
- A specialist asks "is there an intake bead for X?" and you need to check the user_reports → BEADS mapping

**Soft triggers** (use judgment):
- A user_reports row sits in `pending` longer than ~24h with no approval — escalate to the operator rather than self-deciding
- A `rejected` row that the operator changes their mind about — re-file as a fresh bead, link to the original report id

**Do NOT use this skill for:**
- Filing a bead from an in-conversation operator request that did NOT come through /reportar (those file directly via `create_task`)
- Reading the report board for general triage (use `incluir-prod-postgres` directly)
- Implementing the requested feature (that's the assigned specialist's job — this skill stops at the filing + routing)

---

## 2. Query approved reports (stage 1)

The headline query — approved reports with reporter + reviewer info, ordered by approval time:

```sql
SELECT
  ur.id,
  ur.kind,
  ur.urgency,
  ur.title,
  ur.description,
  ur.area,
  ur.repro_steps,
  ur.admin_notes,
  reporter.name AS reporter_name,
  reporter.email AS reporter_email,
  reviewer.name AS reviewer_name,
  ur.reviewed_at AT TIME ZONE 'America/Sao_Paulo' AS reviewed_local,
  ur.created_at,
  ur.updated_at
FROM user_reports ur
LEFT JOIN "user" reporter ON reporter.id = ur.user_id
LEFT JOIN "user" reviewer ON reviewer.id = ur.reviewed_by
WHERE ur.status = 'approved'
  AND (ur.admin_notes IS NULL OR ur.admin_notes NOT LIKE 'BEADS:%')
ORDER BY ur.reviewed_at DESC NULLS LAST
LIMIT 50;
```

**The `admin_notes NOT LIKE 'BEADS:%'` filter** is the duplicate-filing guard — see §7 for the writeback that puts `BEADS: <id>` into the notes when the bead is filed. New runs of this query skip already-filed reports.

Run via the `incluir-prod-postgres` skill — same container, same credentials. Don't reinvent the connection.

Bulk-quote-safe variant for shell scripting (returns `§`-delimited rows that survive PT-BR text without breaking on commas/newlines):

```bash
ssh xerox "docker exec compose-override-solid-state-port-349ude-postgres-1 psql -U incluir -d incluir_hono -t -A -F'§' -c \"
  SELECT id, kind, urgency, title, description, COALESCE(area,''), COALESCE(repro_steps,''), COALESCE(admin_notes,'')
  FROM user_reports
  WHERE status = 'approved'
    AND (admin_notes IS NULL OR admin_notes NOT LIKE 'BEADS:%')
  ORDER BY reviewed_at DESC;
\""
```

---

## 3. Inspect attachments (stage 2 — metadata only)

For each approved report id, list its attachments:

```sql
SELECT id, report_id, url, filename, mime_type, size, ordinal, uploaded_at
FROM report_attachments
WHERE report_id IN ('<id1>', '<id2>', ...)
ORDER BY report_id, ordinal;
```

The `url` column is the MinIO object key under the `user-reports` bucket. Three patterns you'll see:

| URL shape | Origin | Notes |
|---|---|---|
| `<report_id>/attachment/<uuid>.webp` | v1.2 multi-attachment (aperture-h1uk) | Always populated, transcoded WebP for images, sanitized filename for non-images |
| `<report_id>/<uuid>.webp` | v1.1 single-screenshot (aperture-4j8a) | Legacy. Often zero-byte stubs from the v1.1 → v1.2 migration. Check `size` before trying to fetch. |
| Anything else | Likely stale or misrouted | Treat as not-fetchable |

**Sanity-check `size` BEFORE fetching.** A row with `size = 0` is a legacy stub from the v1.1 migration window — fetching it returns 0 bytes. Don't waste a roundtrip; note it in the bead as "legacy stub, not usable" and move on.

**PDF / non-image attachments referenced in `description` but NOT in `report_attachments`** are surprisingly common. The AI summary mentions "user anexou um PDF chamado X.pdf" because the user said so in chat, but the actual upload either failed mid-flight or never happened. Note the discrepancy in the bead. Don't synthesize the file. The text usually carries enough signal.

---

## 4. Fetch attachment blobs from MinIO (stage 2 continued)

MinIO at `infra-minio-6b6568-minio-1` has **no published ports** — reach it via the `prod-main-app-main-apps-wfjeox_app-network` docker network. The hono-app container is on the same network and proxies its access; you'll do the same from a one-shot container.

### The credentials (read once from the running hono-app):

```bash
ssh xerox 'docker exec compose-override-solid-state-port-349ude-hono-app-1 env | grep -E "MINIO|BLOB"'
# MINIO_ACCESS_KEY=mini-ameno
# MINIO_SECRET_KEY=xQ59hAsPSc93gPHEignLNeRfs
# MINIO_ENDPOINT=minio
# MINIO_PORT=9000
```

Bucket: **`user-reports`** (constant defined at `apps/hono-app/src/use-cases/user-report/screenshot-url.ts:32`).

### Why a one-shot python container

The hono-app container itself doesn't ship `python3` and runs as non-root (apk install denied). `mc` is not installed inside the MinIO container either. The pragmatic workaround:

- Use an image that IS local on xerox (no Docker Hub auth) — `postgres:16-alpine` works since it's local AND alpine-based.
- Mount the script via `-v /tmp/fetch-minio.py:/fetch.py`.
- Attach to `prod-main-app-main-apps-wfjeox_app-network` so `minio:9000` resolves.
- `apk add python3` inside the throwaway container (fast, ~3s).

### The fetch script (`/tmp/fetch-minio.py` on xerox)

```python
import hashlib, hmac, urllib.request, datetime as dt, sys
ak = 'mini-ameno'
sk = 'xQ59hAsPSc93gPHEignLNeRfs'
bucket = 'user-reports'
host = 'minio:9000'
key = sys.argv[1]
now = dt.datetime.utcnow()
amzdate = now.strftime('%Y%m%dT%H%M%SZ')
datestamp = now.strftime('%Y%m%d')
region = 'us-east-1'; service = 's3'
canonical_uri = '/' + bucket + '/' + key
canonical_headers = f'host:{host}\nx-amz-content-sha256:UNSIGNED-PAYLOAD\nx-amz-date:{amzdate}\n'
signed_headers = 'host;x-amz-content-sha256;x-amz-date'
canonical_request = f'GET\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\nUNSIGNED-PAYLOAD'
credential_scope = f'{datestamp}/{region}/{service}/aws4_request'
string_to_sign = f'AWS4-HMAC-SHA256\n{amzdate}\n{credential_scope}\n{hashlib.sha256(canonical_request.encode()).hexdigest()}'
def sign(k,m): return hmac.new(k,m.encode(),hashlib.sha256).digest()
k_date = sign(('AWS4'+sk).encode(), datestamp)
k_region = sign(k_date, region)
k_service = sign(k_region, service)
k_signing = sign(k_service, 'aws4_request')
signature = hmac.new(k_signing, string_to_sign.encode(), hashlib.sha256).hexdigest()
auth = f'AWS4-HMAC-SHA256 Credential={ak}/{credential_scope},SignedHeaders={signed_headers},Signature={signature}'
req = urllib.request.Request('http://'+host+canonical_uri,
    headers={'Authorization': auth, 'x-amz-date': amzdate, 'x-amz-content-sha256': 'UNSIGNED-PAYLOAD'})
sys.stdout.buffer.write(urllib.request.urlopen(req, timeout=10).read())
```

Stage it once per session, then loop:

```bash
# Stage the script
scp /local/path/fetch-minio.py xerox:/tmp/fetch-minio.py

# Pull all images for a report
ssh xerox '
mkdir -p /tmp/aperture-images
for KEY in "<report_id>/attachment/<uuid1>.webp" "<report_id>/attachment/<uuid2>.webp"; do
  FNAME=$(basename "$KEY")
  docker run --rm \
    --network prod-main-app-main-apps-wfjeox_app-network \
    -v /tmp/fetch-minio.py:/fetch.py \
    postgres:16-alpine \
    sh -c "apk add --quiet --no-cache python3 2>&1 >/dev/null; python3 /fetch.py $KEY" \
    > /tmp/aperture-images/$FNAME 2>/dev/null
  echo "$FNAME → $(stat -c%s /tmp/aperture-images/$FNAME) bytes"
done'

# Copy locally for Read tool inspection
mkdir -p ~/.claude/aperture-images
scp xerox:/tmp/aperture-images/*.webp ~/.claude/aperture-images/
```

**Verify bytes match the DB row's `size` field** before treating the fetch as successful. A 135-byte response is usually an XML error from MinIO (AccessDenied / NoSuchKey), not actual image data.

**View the fetched images** via the Read tool — Claude is multimodal and will render WebP. This is how you actually understand what the user wanted to show you, beyond the text description. Often the image makes a request immediately concrete that prose alone leaves ambiguous (e.g. the screenshot of `/home/admin/errors` flooded with 401s — see precedent in §10).

---

## 5. File as BEADS (stage 3 — the filing discipline)

### 5.1 Bead shape

```
Title:       <Action verb> <object> <where> (intake <8-char-id>)
Priority:    Per §5.4 (urgency → priority mapping)
Type:        feature | bug | chore — match the user_reports.kind
Labels:      project:incluir   (mandatory, exactly one)
Assignee:    wheatley   (ALWAYS — planning gate per §0/§6)
Description: Verbatim PT-BR + structured fields per §5.3
```

The `(intake <8-char-id>)` suffix on the title is **non-negotiable** — it makes the provenance grep-able later. The 8-char id is the first segment of the user_reports uuid.

**Assignee is ALWAYS wheatley at filing time.** Specialist routing happens at stage 4c, AFTER Wheatley produces a plan AND the operator approves it. See §6.

### 5.2 The strict mapping discipline (READ THIS BEFORE BATCH FILING)

**Banked precedent (2026-05-25, this skill's genesis):** GLaDOS filed 4 approved reports in a batch and miswired the title-to-body pairing TWICE in a row — the title belonged to report A, the description text belonged to report B. Both miswired beads had to be closed-as-superseded and re-filed.

**The antidote:** Before calling `create_task` for ANY batch of >1 report, build a strict mapping table in your head OR in a scratch buffer. Each row is one (id, title, body) triple. When you compose each `create_task` call, look up the body BY ID from the mapping, not from memory.

Example mapping (the one I should have built first):

| id | title (short) | body (one-line gist) |
|---|---|---|
| fef2eff6 | Botão de presença | mark/unmark presence direct on alunos page |
| eddd8c55 | Filtrar erros sessão | hide 401 get-session noise on admin/errors |
| 69e5d449 | Aba Responsáveis | new tab showing guardianship records |
| 1811a05a | Editar dados cadastrais | secretaria edits aluno name/CPF/etc |

Then each filing call references the mapping by ID rather than reaching into a shared mental buffer. The bug was reaching into the buffer.

### 5.3 Description template

```
Surfaced via /reportar AI intake. Approved by operator <reviewed_local> BRT.

Source row: user_reports id=<full-uuid>, kind=<kind>, urgency=<urgency>, reporter=<reporter_name>.

USER REQUEST (verbatim, PT-BR): <description verbatim from DB>

AREA (verbatim): <area field, or "(none provided)" if null>

ATTACHMENTS:
- <relative path to saved image OR "none" OR "legacy zero-byte stub, not usable">
- For each: bytes count + one-sentence description of what it shows
- If a PDF or other file was referenced in the description but NOT in report_attachments, note that explicitly.

PROPOSED SCOPE (<specialists relevant>):
- <2-5 bullets translating the PT-BR ask into concrete implementation pointers>
- <Include any open question that needs operator clarification>

WHO BENEFITS: <which role gets the win — secretaria / coordenador / admin / operator>

DEPENDENCIES:
- <relevant existing schema, route, or epic that this work intersects>

ACCEPTANCE:
- <2-4 testable conditions defining "done">
```

### 5.4 Urgency → Priority mapping

| user_reports.urgency | BEADS priority | Reasoning |
|---|---|---|
| `high` | P2 | Substantive — the user flagged it as blocking or near-blocking |
| `medium` | P3 | Standard quality-of-life improvement |
| `low` | P4 | Backlog — polish, optimization |

Do NOT promote to P0 or P1 from intake alone. P0/P1 are reserved for live fires and operator-directed work. If an intake genuinely is a P1, the operator will say so out-of-band.

### 5.5 The literal-close-tag footgun (READ EVERY TIME)

**Banked precedent (2026-05-25):** GLaDOS pasted literal `</description>` and `</invoke>` patterns into a `create_task` description payload. Both miswired beads had the close-tag patterns visibly stored in the description. This is the BEADS wire-format failure mode the `beads` skill specifically warns about (§4 "🚨 Tool-argument escaping").

**Before EVERY `create_task` call, scan your description text for `</`.** If you find it:
- Paraphrase ("the description field" instead of `</description>`)
- OR HTML-escape (`&lt;/description&gt;`)
- OR add a zero-width space (`</​description>` with U+200B between `</` and `description>`)

Plain prose without any `</` is always safe. The intake descriptions are PT-BR prose — they won't naturally contain `</` unless you accidentally include a fragment of an XML/HTML example.

### 5.6 Storing attachment artifacts

After the bead is created, attach each fetched image:

```
store_artifact(task_id: "<bead-id>", type: "file", value: "~/.claude/aperture-images/<basename>")
store_artifact(task_id: "<bead-id>", type: "note", value: "<one-sentence summary of what the image shows>")
```

The `file` artifact gives future agents the path to re-read the image. The `note` artifact gives them a text summary in case they're triaging without image-render capability.

---

## 6. Routing (two-stage)

Routing is **two-stage**: every intake bead lands with Wheatley first, then gets reassigned to a specialist AFTER planning + operator approval. The intermediate planning step is what makes the specialist's claim productive instead of half-specified.

### 6.1 Stage 4a — Wheatley (always, no exceptions)

Every intake-derived bead is filed with `assignee: wheatley`. He owns the planning lane, so he produces:

- Concrete scope (resolves the open questions in the bead's PROPOSED SCOPE section)
- Dependency verification (greps the repo to confirm the schema / route / component actually exists)
- Sharpened acceptance criteria (testable conditions, not vibes)
- Routing recommendation (which specialist should claim once approved)
- Cipher / cross-cutting flags (security posture concerns, multi-specialist work, etc.)

Wheatley's deliverable lands as a notes-append on the bead, not a separate document. The bead description stays in operator-intake shape; the plan accumulates as Wheatley's progress notes.

### 6.2 Stage 4b — operator approval

When Wheatley pings GLaDOS that a plan is ready, GLaDOS surfaces it to the operator with a short summary + recommended routing. The operator either:

- ✅ Approves → proceed to 4c (specialist reassignment)
- 🔁 Requests changes → bounce back to Wheatley with the requested adjustments
- ❌ Rejects → close the bead with reason; writeback to user_reports per §7.2 (`wont-fix`)

This is the only stage where the operator interacts with intake-derived work directly. Keep the surfacing crisp — bead id + title + Wheatley's plan summary in ≤5 bullets + recommended specialist.

### 6.3 Stage 4c — specialist routing

After operator approval, GLaDOS reassigns via `update_task(id, assignee: <specialist>)`. The routing matrix:

| Signal in description / area | Specialist |
|---|---|
| Frontend tab, button, CSS, layout, page added | **vance** |
| Database schema, migration, API endpoint, server logic | **rex** |
| Mobile / React Native / app store | **scout** |
| Auth, permissions, audit log, secrets, RBAC | **cipher** (posture review) + **rex** (implementation) |
| Infra, deploy, env var, container, DNS | **peppy** |
| Documentation, runbook, API reference | **atlas** |
| SEO, content strategy, conversion funnel | **sage** |
| Test coverage, regression, E2E | **izzy** |
| Cross-discipline (UI + API + tests) | **leave with wheatley as sub-task coordinator**, OR file split sub-beads with `discovered-from:<parent>` |

If Wheatley's plan flagged an ambiguous routing, GLaDOS pings the operator before reassigning. Don't force-route an unclear ask.

**For PII-touching mutations (any "edit user data" intake):** ALWAYS add a co-assignment hint to **cipher** in the bead notes — Cipher needs to review the audit-log discipline before implementation lands.

### 6.4 Why the planning gate is non-negotiable

Skipping straight from filing to specialist routing produces half-specified work. Specialists then either:
- Stall waiting on clarification ("does this apply per-day or per-session?")
- Make assumptions that don't match operator intent
- Burn a session on scope refinement that should have happened upstream

Wheatley's lane IS this gap. He turns operator-approved intake into spec-shaped work. The cost is one extra stage in the pipeline; the win is specialists getting work they can actually execute.

---

## 7. Status writeback (stage 5)

When GLaDOS sees an intake-derived bead close (PR opened per the BEADS close-on-PR-open invariant), write a status note back to the original `user_reports` row. This closes the feedback loop with the original reporter.

### 7.1 The writeback

```sql
UPDATE user_reports
SET admin_notes = COALESCE(admin_notes || E'\n', '') ||
                  'BEADS: ' || $bead_id || ' shipped ' || NOW()::date || ' — ' || $pr_url,
    updated_at = NOW()
WHERE id = $report_id;
```

Shell shape (with the prod connection from the `incluir-prod-postgres` skill):

```bash
ssh xerox "docker exec compose-override-solid-state-port-349ude-postgres-1 psql -U incluir -d incluir_hono -c \"
  UPDATE user_reports
  SET admin_notes = COALESCE(admin_notes || E'\n', '') || 'BEADS: <bead-id> shipped ' || NOW()::date || ' — <pr-url>',
      updated_at = NOW()
  WHERE id = '<report_id>';
\""
```

### 7.2 When to writeback

- **Bead closes with PR-opened** (the standard path): write `BEADS: <id> shipped <date> — <pr-url>`.
- **Bead closes as won't-fix** (operator decision): write `BEADS: <id> wont-fix <date> — <reason>`.
- **Bead split into multiple sub-tasks**: write `BEADS: <epic-id> in-progress <date> — split into <child1>, <child2>, ...`. Update on each child's close.

### 7.3 Idempotency

The writeback APPENDS to admin_notes. If the same bead closes twice (re-opened, re-shipped), you get two lines. That's a feature, not a bug — the history is the audit trail. Don't try to "fix" it by replacing.

The duplicate-filing guard in §2 (`admin_notes NOT LIKE 'BEADS:%'`) prevents re-filing — once one BEADS line is in admin_notes, the row won't surface again in a fresh intake query.

### 7.4 What the reporter sees

This writeback eventually surfaces in the admin board's report-detail view (the `admin_notes` field is rendered to the reviewer). For now, the original reporter doesn't see it directly — surfacing it back to the user-facing /reportar history is a follow-up the operator may want eventually. Not in this skill's scope.

---

## 8. Common operator interactions

### "Find any approved reports I haven't filed yet"

Run the query in §2 with the duplicate-filing guard. Show the operator a short table:

```
| id | kind | urgency | reviewed (BRT) | title |
|----|------|---------|----------------|-------|
| ... | feature | medium | 15:36 | Foo bar baz |
| ... | bug | high | 11:34 | Botão não funciona |
```

Then ask: "File all of them as beads, or pick specific ones?"

### "File the approved reports"

Build the strict mapping per §5.2, fetch any attachments per §4, then file in a single batched message (multiple `create_task` calls). Verify the literal-close-tag pattern check in §5.5 before each call. **Every bead gets `assignee: wheatley` at filing time — no exceptions, no routing at this stage.**

After filing, ping Wheatley via `send_message` listing the new bead IDs + titles + priorities + a one-line "what I want from you" framing. Reference the screenshots' on-disk paths if any are attached. Wheatley orders the queue by his own judgment of complexity.

Report back to the operator with a clean table of new bead IDs + titles. All assignee=wheatley. Note that the operator will see specialist routing in stage 4c, after Wheatley plans.

### "Show me what they're asking for"

Read the fetched images via the Read tool. Operator may want you to summarize the visual content beyond the PT-BR description.

### "Wheatley finished planning bead X, what next?"

This is stage 4b. Read the bead's notes (Wheatley appends his plan there), summarize for the operator in ≤5 bullets: scope, key dependency findings, sharpened acceptance criteria, recommended specialist, any open questions Wheatley flagged. Ask the operator: approve / request changes / reject.

On approve → `update_task(id, assignee: <specialist>)` per the §6.3 matrix. Ping the specialist with a fresh dispatch message including the bead ID + the Wheatley-enriched scope.

On request-changes → bounce back to Wheatley with the operator's adjustments as new notes.

On reject → `close_task` with the operator's reason; run the §7.2 `wont-fix` writeback to user_reports.

### "Close the loop on bead X"

Run the writeback in §7.1 with the bead's PR URL. Confirm the admin_notes row updated.

---

## 9. Anti-patterns

| Don't | Why |
|---|---|
| File a bead from a `pending` (not-yet-approved) report | Operator hasn't decided yet. Wait for `approved` status, OR ping the operator with the queue if it's been sitting. |
| Synthesize a title — always use a verb-object phrase that maps to the user's actual ask | Vague titles ("User feedback") rot fast. The user already gave you the verb. |
| Skip the `(intake <id>)` suffix | Provenance becomes ungrepable. Future GLaDOS needs to trace a bead back to a report and won't be able to. |
| Translate the PT-BR description to English in the verbatim block | Verbatim means verbatim. The PROPOSED SCOPE section is where English engineering analysis lives; the USER REQUEST block stays in the user's language. |
| **Skip the planning gate and route straight to a specialist** | **Half-specified work stalls. Specialists burn cycles on clarification that should have happened upstream. Wheatley's lane IS this gap.** |
| **Assign directly to a specialist at filing time** | The §5.1 assignee is ALWAYS wheatley. Specialist routing happens at stage 4c, after planning + operator approval. |
| **Surface a Wheatley plan to the operator without summarizing in ≤5 bullets** | Operator approval is the bottleneck; respect their attention budget. Bullet form, not essay form. |
| Pull the same approved report into two beads | Use the §2 duplicate-filing guard. If you bypassed it (operator override), at least cross-link the two beads with `related:` |
| Forget to writeback after PR-opened | The feedback loop never closes. Use §7's pattern reliably. |
| Compose batch filings by reaching into a shared mental buffer for "the description" | This is the title-to-body cross-pollination bug. Build the mapping FIRST, then file by ID lookup. |
| Paste a literal close-tag fragment (`&lt;/description&gt;`, `&lt;/invoke&gt;`) into ANY argument | Wire-format truncation. Scan for `&lt;/` before every `create_task`. |

---

## 10. Banked precedents (failure modes seen in real intake batches)

### 2026-05-25 — the 4-report batch that birthed this skill

Operator approved 4 feature requests (`fef2eff6`, `eddd8c55`, `69e5d449`, `1811a05a`) plus 1 test-fixture report. GLaDOS filed them and hit three distinct failure modes:

1. **Title-to-body cross-pollination, attempt 1**: filed `aperture-r6u8` with title "Aba Responsáveis" (which belonged to 69e5d449) but body about "session errors" (which belonged to eddd8c55). Closed as superseded.
2. **Title-to-body cross-pollination, attempt 2**: filed `aperture-9qz5` with title "Botão de presença" (fef2eff6) but the eddd8c55 body again. Closed as superseded.
3. **Literal close-tag pollution**: both r6u8 and 9qz5 had visible `</description>` and `</invoke>` patterns in their stored description — the BEADS wire-format truncation footgun. The skill `beads` warned about this explicitly; GLaDOS ignored the warning.

Third try (`aperture-pubp`, `aperture-9v6e`, `aperture-le5k`, `aperture-nw4b`) worked because:
- Built a strict id→title→body mapping table BEFORE the filing batch
- Did all 4 `create_task` calls in a single parallel message, each referencing its body by id-lookup rather than mental buffer
- Pre-scanned every description payload for `</` (none found, PT-BR prose is naturally close-tag-free)

Also during this batch:
- `eddd8c55` had 2 real image attachments + 1 PDF mentioned. The 2 images were fetchable from MinIO (sizes 138814 + 82322 bytes, matching DB rows exactly). The PDF was NOT in `report_attachments` — likely a chat-mentioned-but-not-uploaded artifact. Documented in the bead as "referenced but not persisted."
- `fef2eff6` had a single zero-byte legacy `screenshot.webp` row (v1.1 stub from the v1.2 migration window). Not fetchable. Documented in the bead as "legacy stub, not usable."

The two real images turned out to be screenshots of `/home/admin/errors` itself, captured by the user to illustrate the 401-flood problem — they made the request immediately concrete in a way the prose alone didn't. Image fetching is high-signal; don't skip it when the size > 0.

### 2026-05-25 — the planning-gate added (this skill's first revision)

After the initial 4-bead batch shipped with `assignee:` filled in directly (vance / rex / etc. per the original routing matrix), the operator codified a new flow: **intake → GLaDOS files → Wheatley plans → operator approves plan → specialist claims**.

Reasoning (operator's framing, verbatim): "this is a good flow valid in the skill, wheats plan them, we approve dispatch to specialists".

The four beads from the morning batch (`aperture-pubp`, `-9v6e`, `-le5k`, `-nw4b`) were retroactively reassigned to wheatley with a notes-append explaining the planning brief. The skill was updated to make wheatley the default assignee at filing time, with the §6.3 specialist matrix moving to stage 4c (post-approval reassignment).

Subsequent intake batches MUST follow this flow. Skipping the planning gate is a banned anti-pattern in §9.

---

## 11. Tools / skills used

- `incluir-prod-postgres` — connection + safe-tier guidance for prod Postgres
- `aperture:beads` — BEADS lifecycle + escaping discipline (§5.5 cross-references)
- `aperture:communicate` — how to report intake batch results to the operator
- Read tool — multimodal WebP rendering for fetched images
- ssh xerox + docker exec — MinIO fetch path
- `incluir-prod-backup` — if you're about to do a destructive writeback test, snapshot first (rarely needed for §7)

---

## 12. Safety tiers

| Tier | Operations | Rule |
|---|---|---|
| Read-only | Stage 1 query, stage 3 metadata read, stage 4 MinIO fetch | Run freely |
| BEADS creation | `create_task` for intake-derived beads | Run freely — same discipline as any BEADS filing |
| Postgres mutation | Stage 5 admin_notes writeback | Run freely — append-only single-row UPDATE, idempotent in the append sense |
| BULK admin_notes mutation | Backfilling old reports retroactively | Operator approval — non-trivial change |
| PROHIBITED | UPDATE user_reports.status from this skill, DELETE user_reports, DELETE report_attachments | Never. Status mutations belong to the admin board UI. |
