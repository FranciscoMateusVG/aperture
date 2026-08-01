# Hermes Lane 3 — Broad-Read Capability: SECURITY-ARCHITECTURE PROPOSAL

**This is a PROPOSAL for Cipher to set her VM-path conditions — NOT a final spec.** It proposes the security architecture for a broad-read Hermes capability so the reviewer can pin conditions, caps, and evidence requirements. Nothing here is build-cleared; §8 lists the open decisions that must be settled before deep design.

- **Bead:** aperture-qyct9 (broad-read) — SEPARATE from the narrow-lane bot (aperture-jxwbd).
- **Origin:** supersedes `hermes-lane3-broad-read-DESIGN-SKETCH.md` (the GLaDOS/operator pre-design sketch). That sketch offered three shapes (expanding typed ops / generic-SQL-in-VM / curated views). **The operator has since chosen the maximal-flexibility shape (Option 2, generic read).** This proposal designs that shape only.
- **Date:** 2026-07-30
- **Path taken:** the DEDICATED-LINUX-VM path from the start. This is deliberately NOT the hardened-container-on-Mini waiver: Cipher's v3 container waiver was explicitly conditioned on *"fixed typed lookups, NOT a generic query API,"* and she ruled that any generic/broader query capability reopens the dedicated-VM requirement. A generic SQL surface is exactly that case, so the VM is assumed, not argued.

---

## §0 — Shape (LOCKED; operator informed decision)

The operator chose this shape explicitly and repeatedly, and was informed of the consequences. Recorded here as a locked, LGPD-significant posture — not an infra default:

- **Interface:** chat + exactly **ONE** tool — a **generic read-only SQL/query tool over the ENTIRE incluir database**. Not typed lookups; a generic query surface.
- **Fields:** **ALL fields returnable, including sensitive PII** — CPF, DOB, guardians, phone/email/address, health/support notes, financial, documents — **UNREDACTED**, crossing to **OpenAI (processing)** and **Slack (retention)**. **NO field-level filtering.** The operator confirmed the specific consequence: sensitive student PII for ~1042 students can be surfaced un-redacted, via chat, to OpenAI and Slack. **Record this as an explicit, informed acceptance** (see §7).
- **Access:** **operator-DM-only**, fail-closed Slack allowlist = member id **`U050U6BNCS0`**, `GATEWAY_ALLOW_ALL_USERS=false`.

Any change to this shape — a second tool, generic HTTP, any write/bulk-export path, or field scope beyond "everything" — voids this design and requires re-review.

---

## §1 — Isolation: a DEDICATED LINUX VM

The generic query surface is the open-ended-tool case, so isolation is a **dedicated Linux VM** — its own kernel and its own container engine — not a container on the Mac Mini.

- **No bridges to the operator's world:** NO Tailscale, NO host/shared folders, NO docker socket, NO route to the Mac Mini host, the private LAN, or the tailnet. The VM's only reachable peers are the three egress destinations in §4.
- **Defense-in-depth INSIDE the VM (not a substitute for the VM):** run Hermes in a hardened container within the VM — non-root, `cap_drop: ALL` + `no-new-privileges`, seccomp + AppArmor **enforced** (proven, not declared), read-only root + tmpfs `/run` `/tmp`, no published ports. This is a second wall behind the VM boundary, carried over from the narrow lane's hardening discipline.

**OPEN DECISION (VM host) — see §8.1.** Where the VM runs is not settled. **Recommendation:** a dedicated cloud instance in its own isolation boundary (own project/VPC/security group), explicitly **NOT** a prod host (not xerox, not the Mini). Note the cost: this is a new standing instance with its own bill and its own patching/lifecycle burden — that cost is the price of the chosen shape.

---

## §2 — DB access: READ-ONLY REPLICA + SELECT-only role

The prod database (`incluir_hono` on xerox, ~1042 live students) serves production. Arbitrary, expensive generic SQL against the prod-primary risks degrading production. So the query tool connects to a **READ-ONLY REPLICA** of incluir prod — **strongly recommended**.

- **No writes ever, two ways:** structurally (a replica cannot be written) **and** role-enforced.
- **DB role = SELECT-ONLY:** no INSERT/UPDATE/DELETE/DDL/COPY/TRUNCATE and no side-effect functions; **schema-scoped** to the incluir schema(s) only.
- **Resource governors, enforced at the DB/role:** `statement_timeout`, **max result rows**, **max result bytes**, **connection limit**, **single-concurrency**. A generic surface must not be able to table-scan the whole DB into context or pin the replica.

**OPEN DECISION (replica vs primary) — see §8.2.** Recommended: read-replica (structural no-write + isolates prod from expensive queries). Alternative: a read-only role on the prod-primary with tight caps — simpler to stand up, but riskier (expensive generic SQL now lands on the primary serving 1042 students; no structural write barrier, only the role).

---

## §3 — The generic-query tool

Exactly one tool: it executes **read-only SQL**. This is the generic surface that mandates the VM (§1). Guardrails:

- **Reject any non-SELECT at BOTH layers:** parse-level rejection in the tool (statement is parsed; anything that isn't a single read-only SELECT is refused before dispatch) **and** the SELECT-only DB role (§2) as the structural backstop. Neither layer is trusted alone.
- **Resource caps** as in §2: `statement_timeout`, row cap, byte cap, single-concurrency.
- **Per-query AUDIT (every query, no exceptions):** query text + **authenticated operator Slack uid** + timestamp + row-count + outcome. The uid is **bound from the Slack event in trusted adapter code — NEVER a model-supplied argument.** The model can influence the query text within the caps; it can never assert who the actor is.

**OPEN DECISION (SQL vs NL) — see §8.4.** Does the operator type SQL directly, or ask natural language that the model translates into SQL? Either way, the tool runs read-only SQL under the same guardrails; the NL-translation variant adds a model-in-the-loop step that the audit (query text captured post-translation) still covers.

---

## §4 — Egress: same routing-proxy discipline as the narrow lane

Hermes reaches **ONLY** three destinations, enforced by ROUTING (segment/listener binding), not by trusting env vars or source-IP:

1. **OpenAI** — via a **local OpenAI gateway** that holds the key and enforces a hard token/cost ceiling. The gateway is **structurally unavoidable**: Hermes points at the gateway directly and is on no segment that can reach `api.openai.com`; the gateway (not Hermes) makes the upstream connection.
2. **Slack** — the Socket Mode `wss-*` host family only.
3. **The read-replica DB endpoint** (§2).

Everything else is **default-denied by routing**: no host-gateway route, no tailnet, no cloud metadata (`169.254.169.254`), no RFC1918 / CGNAT / ULA / loopback / link-local, no raw-IP dial, no arbitrary public host, no DNS-exfil. Re-resolve and reject at dial time (DNS-rebinding defense). This mirrors the narrow lane's five-segment / listener-bound proof discipline.

**OPEN DECISION (broad-read cost ceiling) — see §8.3.** Generic queries return larger result sets into context than the narrow lane's two fixed lookups, so the OpenAI token budget is likely higher than the narrow lane's **50,000 tokens/UTC-day**. Proposed starting point: **150,000 tokens/UTC-day** (raise only from observed legitimate usage, operator-approved) — carrying over the narrow lane's exact enforcement discipline: pinned tokenizer, atomic reserve of tokenized-input + max-output before dispatch, reconcile from the returned `usage`, kill-closed (reject, don't queue) on breach, counters persist across gateway restart and reset on the UTC-day boundary, **fail closed if counter persistence is unavailable.** Cipher pins the final number.

---

## §5 — Tool policy: chat + the ONE generic-SQL read tool ONLY

- **Everything else OFF:** no shell, filesystem, browser, code-exec, cron, delegation, subagents, MCP (beyond the one query tool), hooks, plugins, installers, or durable memory.
- **Proven absent at runtime**, not merely unconfigured — black-box prove the effective tool inventory is `{chat, generic-SQL-read}` and nothing else.
- **Memory-provider-bypass image check** (advisory for versions ≤ 0.16.0): reconcile the advisory against the pinned image; record `hermes --version`; **REJECT the image** if the affected memory/plugin bypass path is present.
- **Void condition:** any additional or second tool, any generic-HTTP / write / bulk-export capability, voids the design and requires re-review.

---

## §6 — Slack: operator-DM-only, fail-closed

- **DM-only:** `message.im` events only; bot + subtype events ignored. No channels, groups, app-mentions, or message history beyond what Bolt strictly needs.
- **Minimal scopes:** `connections:write` + `chat:write` + the minimum proven `im` scope. Pinned at manifest **and** runtime.
- **Fail-closed allowlist:** `SLACK_ALLOWED_USERS=U050U6BNCS0`, `GATEWAY_ALLOW_ALL_USERS=false`, bots denied.
- **Unauthorized user rejected BEFORE any LLM or tool call** — a denied event yields ZERO OpenAI, ZERO DB query, ZERO tool call. Proven at the request layer (request-count evidence).

---

## §7 — Data-boundary (stated plainly)

The operator made an **explicit, informed decision** to allow **ALL incluir PII, unredacted**, to cross to **OpenAI (processing)** and **Slack (retention)** via a chat bot, for ~1042 students. **This is the accepted boundary.** Even a perfectly isolated VM does not reduce it — the exfil channel (bot answers → OpenAI + Slack) is the intended feature.

The compensating controls are:

- the **per-query audit trail** with authenticated actor (§3),
- **operator-only access** (fail-closed allowlist, §6),
- **no writes ever** (replica + SELECT-only role, §2),
- **VM isolation** (§1),
- **read-replica** (production integrity, §2).

**These are NOT field redaction.** There is no field-level filtering in this shape — that is the locked operator decision (§0). This section exists so the boundary is recorded, not softened.

---

## §8 — OPEN DECISIONS (for Cipher + operator to close)

| # | Decision | Recommendation | Trade-off |
|---|---|---|---|
| **8.1** | **VM host** — where the dedicated Linux VM runs | Dedicated cloud instance in its own isolation boundary, NOT a prod host | Carries a standing instance cost + patching/lifecycle burden |
| **8.2** | **Replica vs primary** — DB target for the generic SQL tool | **Read-replica** (structural no-write + isolates prod from expensive queries) | Read-only role on prod-primary is simpler to stand up but riskier (expensive SQL on the live DB; no structural write barrier) |
| **8.3** | **OpenAI cost ceiling** for broad-read | Start at **~150K tokens/UTC-day** (vs narrow lane's 50K), raise only from observed usage | Higher ceiling = higher cost exposure; same pinned-tokenizer / atomic-reserve / fail-closed discipline. Cipher pins the number |
| **8.4** | **SQL vs NL** — operator types SQL, or NL that the model translates | Either is acceptable; audit captures the executed SQL regardless | NL-translation adds a model-in-the-loop step; SQL-direct is more transparent but less friendly |

---

## §9 — Carried-over non-negotiables from the narrow lane (the floor, not the ceiling)

- **Egress routing discipline** — routing/listener-bound, default-deny; no host-gateway / tailnet / metadata / RFC1918 / raw-IP / arbitrary-public / DNS-exfil; re-resolve-and-reject at dial time (§4).
- **OpenAI gateway holds the key; Hermes never sees it** — gateway structurally unavoidable; hard token ceiling with atomic reserve + reconcile-from-`usage` + kill-closed + durable counters + fail-closed-on-persistence-loss.
- **Gateway reconstructs an allowlisted upstream body from scratch** — fixed model snapshot, `store=false`, text-only, bounded message count, only the one tool schema; drops any caller-supplied URL / model / auth / metadata / files / images / audio / extra tools.
- **Authenticated actor in every audit** — bound from the Slack event in trusted adapter code, never model-supplied.
- **Slack DM-only, fail-closed allowlist**, unauthorized rejected before any LLM/tool call (§6).
- **Container hardening PROVEN, not described** — non-root, `cap_drop: ALL`, `no-new-privileges`, read-only root + tmpfs, seccomp + AppArmor enforced (via `docker inspect` + `/proc/<pid>/attr`), no host binds, no docker.sock, no published ports (§1).
- **PII-log OFF everywhere** — Hermes, gateway, proxy, Docker stdout, telemetry: metadata only (op / outcome / count / timestamp); no prompts, no response bodies, no keys.
- **Secrets injected at container start, shredded post-start** — Hermes gets a dummy OpenAI cred; the real key lives only in the gateway; containers never reach Infisical/tailnet.
- **Pinned, scanned image** — digest pinned after SBOM/CVE scan; memory-provider-bypass advisory reconciled (§5).
- **Evidence is config-level + probe-count, not one failed lookup** — go-live only on Cipher's PASS against the proof set.
