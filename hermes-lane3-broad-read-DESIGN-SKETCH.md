# Hermes — broad read-access DESIGN SKETCH (for GLaDOS review before deep design)

**Status:** SKETCH — not a spec. For GLaDOS + operator decisions before Cipher and I go deep. · **Bead:** (new, filed alongside) · **Date:** 2026-07-30

## 1. What changed
Operator (explicitly, not tentative): Hermes needs READ access to **ALL** data in the incluir DB, not just the two fixed student lookups. His framing: *"maybe an MCP for him with only read permission."*

## 2. Why this is a different animal than the narrow lane
Cipher's v3 container waiver was **explicitly conditioned** on *"fixed typed lookups, NOT a generic query API,"* and she stated any generic/broader query capability **reopens the dedicated-VM requirement.** So broad-read is not an extension of the narrow lane — it's a new risk class. Two axes get much worse:

- **Isolation:** a generic query surface = the open-ended-tool case → **dedicated Linux VM comes back** (no Tailscale/host/socket/route), not the hardened-container-on-Mini waiver.
- **DATA-BOUNDARY (the crux, bigger than the VM):** "all data" includes the fields we deliberately default-denied in the narrow lane — **cpf, dob, phone/email/address, guardians, health/support notes, documents, financial data, free-text.** A broad-read tool means a chat bot can surface ANY of that to **OpenAI (processing) + Slack (retention)** for ~1042 students. That is a fundamentally different privacy posture, and it's an **org-level data-protection decision (LGPD/consent territory)**, not just an infra call. Even a perfectly isolated VM does not reduce this — the exfil channel is the intended feature (bot answers → OpenAI+Slack).
- **Prompt-injection blast radius:** with broad read, a steered/injected bot can enumerate or dump the whole DB, or run expensive queries. The narrow lane bounds this to two exact-match ops; broad-read does not.

## 3. Confirm the real need FIRST (GLaDOS flagged this)
"Read all data" could mean two very different things:
- **(A) Literal generic query** — a read-only SQL/MCP tool over the whole schema. Max flexibility, max risk. Matches "MCP with read permission" literally.
- **(B) Broad coverage, bounded interface** — "I can ask about any entity/field," served by a comprehensive-but-typed/curated read layer rather than raw SQL. Most operators asking "read everything" actually want this (ask arbitrary questions), not a SQL console.

Recommend confirming which, because it changes everything below.

## 4. Options
| Option | Shape | Isolation | Data-boundary | Verdict |
|---|---|---|---|---|
| **1. Expanding typed ops** | Keep adding fixed typed read ops (students/courses/classes/volunteers/financial/attendance…) to the narrow lane as needs surface | **No VM** (stays within Cipher's waiver — typed, not generic) | Per-op field allowlist (controlled) | Safest; doesn't cover "arbitrary questions"; grows a maintained list. Good for "the N things he actually asks." |
| **2. Generic read in a dedicated VM** | Read-only DB role / **read replica** + a query MCP tool, in the VM Cipher requires. Controls: statement timeout, row/result caps, query audit, cost ceiling | **VM required** | **Whole-DB PII → OpenAI+Slack** unless a field-redaction policy is enforced at the query layer | Max flexibility; biggest lift; biggest privacy stakes. The literal "MCP read permission." |
| **3. Curated read-view layer** | Read-only **views** / schema-scoped role covering the entities that matter, queried via structured filters (not raw SQL) | Likely still "generic" → **VM likely** | Controlled at the view layer (can pre-strip sensitive columns) | Middle ground; smaller/PII-controlled surface than raw SQL; more design work than Option 1. |

## 5. Recommendation (for GLaDOS)
1. **Ship the narrow 2-op lane now** (v3.1, build-cleared) as the fast/safe path — Rex + I proceed; it's not wasted regardless of how broad-read shakes out.
2. **Broad-read = its own bead + its own Cipher review cycle**, expecting the VM to reopen.
3. **Before any build, get two explicit decisions from the operator:**
   - (a) Which shape — generic-SQL (A / Option 2) vs broad-but-bounded (B / Option 1 or 3)? I lean toward **starting with Option 1 (expanding typed ops)** or **Option 3 (curated views)** and only going full generic (Option 2 + VM) if the operator genuinely needs arbitrary SQL.
   - (b) **Explicit whole-DB-PII data-boundary acceptance** — does the operator/org accept that sensitive student PII (cpf/dob/guardians/health/financial) can be surfaced to OpenAI + Slack by a chat bot? If NOT, we enforce **field-level redaction even in the broad tool** (deny the sensitive columns at the query/view layer), which narrows Option 2/3 considerably. This is the single biggest question and it's org-policy, not infra.
4. Then take the chosen shape to Cipher fresh for the (likely-VM) security design.

## 6. Non-negotiables carried over regardless of shape
Operator-DM-only fail-closed Slack allowlist; read-only credential (no writes ever); per-request auth (HMAC or equivalent) not a durable bearer; query timeouts + result caps; audit with authenticated actor (not model-supplied); the local OpenAI-gateway cost ceiling; no host/tailnet/socket reach; PII-log-off. The narrow lane's controls are the floor, not the ceiling.
