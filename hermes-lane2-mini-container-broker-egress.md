# Hermes — Lane 2 v3.6: Mini hardened container + broker + OpenAI gateway + segmented egress (network graph + probe plan)

**Owner:** Peppy (infra) · **Security gate:** Cipher (reviews this design + the running evidence before go-live) · **Bead:** aperture-jxwbd · **v3.6 2026-07-30** (version-synced with Lane 1 v3.6 — no substantive Lane 2 change this round; the sole v3.5-hash blocker was Lane 1 §8's boundary-test correctness, Lane 2 already closed. Pairs with Lane 1 Contract v3.6.) · **v3.5 2026-07-30** (security-reviewer hash-review — Cipher's 3 remaining v3.4-hash blockers applied (all prior v3.4 blockers preserved untouched): (B1) **nonce TTL > timestamp validity span** — Lane 1's BOTH nonce namespaces bumped `EX 120`→`EX 180` (strictly greater than the ±60s = 120s span) + boundary-replay regression; reflected in the §5 auth-flow summary here; (B2) **signature-mismatch rotation proofs** — every mixed-key rotation probe (§5/§6a) must be FRESHLY SIGNED (in-window timestamp + fresh unused nonce) and assert the rejection is SPECIFICALLY signature mismatch (old-ingress+new-service → AT NEXT for ingress sig mismatch; new-ingress+old-service → AT HONO for service sig mismatch; new+new → accepted), never a false pass via expiry/replay; (B3) **streaming-proxy contradiction removed** — Lane-1-side reword to the bounded-buffer model. Plus 2 nonblocking cleanups: (a) BOTH secret env names (`HERMES_BOT_INGRESS_SECRET` + `HERMES_BOT_SIGNING_SECRET`) made explicit in the pre-live gate (§9 GATE 5); (b) the §5 auth-flow summary now states Next CONSUMES the ingress nonce (not just verifies+strips the ingress HMAC). All v3.4 content otherwise preserved.) · **v3.4 2026-07-30** (security-reviewer hash-review — Cipher's 10 exact-hash blockers applied, targeted, all v3.3 passed content preserved: (7) rotation proof now tests BOTH verifiers independently — old-ingress+new-service rejected AT NEXT, new-ingress+old-service rejected AT HONO, new+new accepted (§5/§6a); (9) pre-parse limits PINNED to exact numbers for broker + gateway — max body bytes, header count, per-header bytes, total-header bytes, header/read/slow-body timeouts, and the gateway max message count, each boundary-probed (§4.4/§5/§6a); (10) token-counting contract PINNED — tiktoken with the `o200k_base` encoding matching `gpt-4o-mini-2024-07-18`, fail-closed if the tokenizer is unavailable/unknown, adversarial-Unicode + tool-schema boundary counting, conservative worst-case reservation so the 50K/UTC-day ceiling cannot overshoot (§4.2). Nonblocking residue fixed: the broker holds BOTH `HERMES_BOT_INGRESS_SECRET` + `HERMES_BOT_SIGNING_SECRET` (§5/§9, previously read as a singular signing secret), and the §5 Lane-1 cross-reference bumped v3.2 → v3.4. All blockers 1–6, 8 land in Lane 1 v3.4.) · **v3.3 2026-07-30** (security-reviewer correction — blocker-4 REDESIGNED: v3.3 REPLACES v3.2's single trusted-ingress-identity pre-auth-bucket fix, which created an IMPOSSIBLE LOCKOUT (a public invalid-signature flood exhausted the one shared-ingress pre-auth bucket and blocked the legit broker — caught by Rex), with a **two-domain-separated-per-request-HMAC representable contract** (Cipher). The BROKER now signs BOTH envelopes: an INGRESS envelope with `HERMES_BOT_INGRESS_SECRET` (verified + stripped at the Next proxy before any Hono call) and the existing SERVICE envelope with `HERMES_BOT_SIGNING_SECRET` (verified FIRST at Hono). Broker holds BOTH secrets; Hermes holds NEITHER. Rotation now covers both keys; the 5 blocker-4 tests are updated. All other v3.2 content preserved. · **v3.2 2026-07-30** (security-reviewer corrections — broker re-projection realigned to Lane 1 v3.2 VERIFIED schema: `enrollmentActive` REPLACED by derived `activeThisSemester`; academicOutcome now INCLUDED (operator explicitly accepted crossing OpenAI + Slack, GLaDOS-confirmed 2026-07-30 — no longer a gate); broker + gateway bound to their ingress-segment IP only, not `0.0.0.0` (blocker 9); DNS containment written as a pinned executable compose shape (blocker 10); broker + gateway pre-parse hardening limits added (blocker 11); pre-live SBOM/source-review + state-volume/log-contents gates pinned; supersedes v3.1's academicOutcome-pending posture + `enrollmentActive` field) · **Pairs with:** Lane 1 Contract v3.6 (hermes-lane1-incluir-bot-api-contract.md, Rex building)
> **Scope note:** the separately-proposed ALL-incluir-data broad-read expansion (bead aperture-qyct9) is a SEPARATE scope explicitly NOT covered by this narrow-lane review/waiver.

## 0. Shape (shape-locked per Cipher's waiver)
Operator-DM-only chat agent + exactly ONE tool (`incluir_lookup`) that calls the **broker**, never incluir directly. All exec/persistence tools disabled + runtime-proven absent. Any drift (2nd/generic tool, raw HTTP, memory, shell/fs/browser/code, cron, delegation, MCP, hook, plugin, installer, write, bulk) VOIDS the waiver → dedicated-VM required.

**Four components:** HERMES · BROKER · OPENAI-GATEWAY · SQUID-PROXY. **Five internal segments + one external** (v3 topology fix): `hermes-broker`, `hermes-llm`, `hermes-slack`, `broker-proxy`, `openai-proxy`, plus external `ext-net`. Separation is enforced by **which segment a listener binds**, never by mutable source-IP ACLs.

**v3 topology fix (the headline blocker):** v2's graph was BROKEN. Squid listener A permitted `api.openai.com`, so Hermes could CONNECT straight to real OpenAI and **bypass the gateway**; and the gateway sat on `ext-net` with raw reach to host/tailnet/metadata. v3 makes the gateway **structurally unavoidable**: Hermes's `OPENAI_BASE_URL` points DIRECTLY at the gateway's static internal address (NOT through Squid), Hermes is not on any segment that can reach `api.openai.com`, and Squid's Slack listener is Slack-only. The gateway — not Hermes — CONNECTs `api.openai.com` through Squid. This also removes the impossible "CONNECT api.openai.com but route to a local gateway" TLS/authority problem.

## 1. Network graph (4 components, 5 internal segments + 1 external — Docker Compose on Mini/OrbStack)

```
   [ all 5 named segments internal:true — SQUID is the ONLY container with ext-net / a default route ]

  ┌────────────┐   hermes-broker (tool)   ┌──────────┐   broker-proxy   ┌───────────────┐
  │            │──────────────────────────│  BROKER  │─────────────────▶│  SQUID        │
  │            │                          │ sidecar  │   (→ listener B) │  listener B   │──┐
  │            │                          └──────────┘                  │  incluir only │  │
  │            │                                                        └───────────────┘  │
  │   HERMES   │   hermes-slack (→ listener A)   ┌──────────────────────────────────────┐  │
  │ container  │────────────────────────────────│  SQUID  listener A  (Slack ONLY)      │  │
  │            │                                └──────────────────────────────────────┘  │
  │            │   hermes-llm (DIRECT, dummy cred, NOT via Squid)                          │
  │            │──────────────┐                                                            │
  └────────────┘              ▼                                                            │
                     ┌──────────────────┐   openai-proxy   ┌───────────────┐              │
                     │  OPENAI-GATEWAY  │─────────────────▶│  SQUID        │              │
                     │  holds real key  │   (→ listener C) │  listener C   │──┐           │
                     │  caps + ceiling  │                  │  openai only  │  │           │
                     └──────────────────┘                  └───────────────┘  │           │
                                                                               ▼           ▼
                                                                     ┌───────────────────────┐
                                                                     │   SQUID  ·  ext-net    │
                                                                     │  443-only, re-resolve  │
                                                                     │  +revalidate at dial   │
                                                                     └───────────────────────┘
                                                                        │        │        │
                                                                        ▼        ▼        ▼
                                                              api.slack.com  incluir  api.openai.com
                                                               (listener A) (listnr B) (listener C)

  Flow: HERMES ──hermes-llm(direct, dummy cred)──▶ GATEWAY  (validates + injects REAL key)
        GATEWAY ──openai-proxy──▶ SQUID listener C ──ext-net──▶ api.openai.com
  HERMES cannot reach OpenAI except THROUGH the gateway: it is not on openai-proxy, and listener A is Slack-only.
```

- **Segments** (five `internal: true`, one external): `hermes-broker` (HERMES↔BROKER tool traffic) · `hermes-llm` (HERMES↔OPENAI-GATEWAY, **direct**, NOT through Squid) · `hermes-slack` (HERMES↔Squid **listener A**) · `broker-proxy` (BROKER↔Squid **listener B**) · `openai-proxy` (OPENAI-GATEWAY↔Squid **listener C**) · plus `ext-net` (external — the only non-internal network, SQUID only).
- **HERMES** joins: `hermes-broker`, `hermes-llm`, `hermes-slack`. **NOT `ext-net`, NOT `broker-proxy`, NOT `openai-proxy`.** No default route. It reaches: the BROKER (tool), the OpenAI-gateway **directly** (`OPENAI_BASE_URL` = the gateway's static internal address on `hermes-llm`, with a dummy cred), and Squid **listener A** for Slack.
- **BROKER** joins: `hermes-broker`, `broker-proxy`. **NOT `ext-net`.** No default route. It reaches: Squid **listener B** only.
- **OPENAI-GATEWAY** joins: `hermes-llm` (accepts from Hermes), `openai-proxy` (egress to Squid **listener C**). **NO `ext-net`, NO default route, NO recursive resolver.** It cannot reach OpenAI directly — only via Squid listener C.
- **SQUID** joins: `hermes-slack` (binds **listener A**), `broker-proxy` (binds **listener B**), `openai-proxy` (binds **listener C**), and `ext-net`. **Sole component with `ext-net` / a default route.** Policy is keyed on **which listener** the CONNECT arrived on — not on the client's container IP (source-IP ACLs are mutable and were the v1 weakness).
- **Ingress-segment bind — BROKER and GATEWAY listen on their intended segment IP ONLY, never `0.0.0.0` [blocker 9]:** the BROKER binds only its `hermes-broker` interface address; the GATEWAY binds only its `hermes-llm` interface address. Because SQUID is multi-homed (on every internal segment plus `ext-net`), a service bound to `0.0.0.0` would be **reverse-reachable by Squid across `broker-proxy`/`openai-proxy`** — Squid could dial back into the broker/gateway on the wrong segment. Pinning the bind address closes that reverse path. Add runtime **NEGATIVE probes proving Squid cannot reverse-reach the broker or gateway on the wrong segment** (§6a).
- Result: neither HERMES nor BROKER nor the GATEWAY can open a raw socket to anything but its permitted Squid listener (the gateway) / direct internal peer (Hermes→gateway); and neither the broker nor the gateway is reachable from Squid on any segment other than its intended one. No host-gateway route (prod-DB:54321 / tailnet / watchtower / metadata reach is gone). The gateway is **structurally unavoidable** for OpenAI traffic. Enforced by ROUTING + LISTENER binding + per-service bind-address, not by trusting HTTPS_PROXY env or source-IP.

## 2. DNS — no usable recursive resolver for HERMES, BROKER, or the GATEWAY (blocker 7)
Docker's embedded `127.0.0.11` resolver can resolve arbitrary names on internal bridges → a DNS-exfil channel even with egress blocked. DNS containment must be an **EXECUTABLE mechanism, not a hand-wave** ("0.0.0.0 or a sink" is not a spec). Pin the exact mechanism:
- **Static pinned peers, zero recursion needed on the permitted paths.** Every name HERMES/BROKER/GATEWAY must reach internally is a **fixed compose address / `extra_hosts` entry**: Hermes→broker, Hermes→gateway (`hermes-llm` static IP = `OPENAI_BASE_URL` host), Hermes→Squid listener A, broker→Squid listener B, gateway→Squid listener C. No permitted path requires resolving an arbitrary name.
- **Configuration that makes arbitrary recursive queries impossible — PINNED EXECUTABLE shape [blocker 10]** (not "e.g./example"). Each of HERMES / BROKER / GATEWAY carries this concrete compose shape; every internal peer it may reach is a fixed `extra_hosts` entry so libc never needs the resolver for the allowed set, and the resolver itself is a non-routable sinkhole that cannot answer or forward anything:
```yaml
# per-service (hermes / broker / gateway) — pinned DNS containment
dns: ["0.0.0.0"]          # non-routable sinkhole — no recursion, no forwarder reachable
dns_search: []            # no search-domain expansion
dns_opt: ["ndots:0", "no-tld-query", "attempts:1", "timeout:1"]
extra_hosts:              # every PERMITTED internal peer pinned — resolver never consulted for these
  - "broker.hermes.internal:10.89.0.11"     # hermes-broker segment (broker bind addr)
  - "gateway.hermes.internal:10.89.1.11"    # hermes-llm segment (gateway bind addr = OPENAI_BASE_URL host)
  - "squid-a.hermes.internal:10.89.2.2"     # hermes-slack  → Squid listener A
  - "squid-b.hermes.internal:10.89.3.2"     # broker-proxy  → Squid listener B (broker only)
  - "squid-c.hermes.internal:10.89.4.2"     # openai-proxy  → Squid listener C (gateway only)
```
(IPs illustrate the shape; Peppy pins the actual per-segment addresses at build to match the §1 bind addresses. Each service carries only the `extra_hosts` entries for the peers IT is permitted to reach.) The point is not the literal `0.0.0.0` value — it is that **no query for an unpinned name can produce an answer or leave the container**, and every allowed name resolves from `extra_hosts` without touching a resolver.
- **Socket Mode still works** because Slack egress is a **CONNECT to a pinned Slack authority through Squid listener A — Squid does the DNS on `ext-net`**, re-validating every resolved address before dial (§3). Hermes never resolves Slack itself.
- **The OPENAI-GATEWAY has NO recursive resolver either** — it CONNECTs `api.openai.com` through Squid listener C, and **Squid resolves it**. The gateway resolves nothing.
- **Evidence is CONFIG-LEVEL + query-count, not one failed `nslookup`:** show the compose `dns:` / `dns_search` / `dns_opt` / `extra_hosts` config, AND packet-level or resolver-query-count evidence that arbitrary-name queries from HERMES/BROKER/GATEWAY never egress while Slack Socket Mode connects/reconnects successfully. A single failing lookup is NOT accepted as proof (§6a).

## 3. SQUID PROXY — one mature pinned instance, THREE listeners (blocker 8)
**Preferred:** a single **mature, pinned Squid** image with THREE network-bound listeners — *only if* it passes SBOM/CVE review (pin by digest after scan). Chosen over writing a new proxy parser because a battle-tested authority parser beats a fresh one.

Config (all three listeners):
- **CONNECT-only, 443-only.** No cache, no plain-HTTP forwarding, no ICP/peering.
- **Strict lowercased authority parsing.** DENY: userinfo (`user@host`), IP-literals, trailing-dot ambiguity, any non-canonical authority.
- **Re-resolve and REJECT** every A/AAAA result before dial if it lands in: loopback, link-local, RFC1918, CGNAT `100.64.0.0/10`, ULA `fc00::/7`, multicast, or metadata `169.254.169.254`. (DNS-rebinding defense — resolve at dial time, revalidate, then connect.)
- **Listener A** (bound to `hermes-slack`): permit **Slack ONLY** — `api.slack.com` + the documented `*.slack.com` Socket-Mode gateway suffix. **NO openai, NO incluir.**
- **Listener B** (bound to `broker-proxy`): permit **ONLY** `app.programaincluir.org`.
- **Listener C** (bound to `openai-proxy`): permit **ONLY** `api.openai.com`. This is the ONLY listener that permits OpenAI, and it is reachable ONLY from the OpenAI-gateway — Hermes is not on `openai-proxy`.
- Policy keys on the **listener**, so HERMES-origin Slack traffic physically cannot request incluir or openai (it only reaches listener A, Slack-only), BROKER-origin traffic physically cannot request openai/slack (it only reaches listener B), and only the GATEWAY can reach OpenAI (listener C). Belt-and-suspenders on the segment split.
- Proxy holds NO secrets. Logs allow/deny with host + listener, **no bodies, no URLs, no Authorization, no prompts/responses** (blocker 8).

**Residual (documented honestly):** the Slack `*.slack.com` suffix allowance is **NOT an exact-host allowlist** — the Socket-Mode gateway hosts are dynamic, so a suffix match is the tightest achievable. This is a known, accepted residual, minimized to the empirically-pinned set (§7 open item).

**Fallback:** if Squid cannot satisfy every rule/probe (esp. the authority-parse + re-resolve-reject + per-listener rules), a **small Go proxy** is allowed **ONLY after** source review + adversarial parser / SSRF / DNS-rebinding tests pass. Not a default; a reviewed exception.

## 4. OPENAI-GATEWAY — remove the real key + a CONCRETE hard token ceiling (blockers 6, 9)
**Context:** the operator chose the **shared "General" OpenAI key** over a dedicated Hermes key (operator-accepted residual — see §4 threat-model note; **do not reopen**). This gateway is what makes that choice safe: it puts a hard, enforceable cost ceiling in front of the shared key and contains key theft if the Hermes memory/plugin bypass advisory is real. **The gateway holds the shared key; HERMES never sees it.**

- **Local gateway** on `hermes-llm` (accepts Hermes traffic **directly**) + `openai-proxy` (egress to Squid **listener C**, which reaches `api.openai.com`). **The gateway has NO `ext-net` and NO default route** — it cannot reach OpenAI except through Squid listener C.

### 4.1 Credential handling — validate-only surface, never leak the key [blocker 9]
- Validates **ONLY the fixed Hermes-local API surface** (the one path + one model below). **Rejects caller-selected upstream URLs / hosts / arbitrary headers** — Hermes cannot ask the gateway to call anything else.
- **Strips any inbound `Authorization`** (Hermes's dummy cred) and **injects the real `Authorization: Bearer <shared key>` itself** (key resolved host-side into the gateway only, §8).
- **NEVER returns or logs upstream credential material** — the key never appears in a response, an error body, stdout, or telemetry.
- **HERMES** is configured with a **DUMMY local credential** + the gateway's base URL via `OPENAI_BASE_URL` = the gateway's static `hermes-llm` address (Hermes supports a custom base URL — confirmed; this points DIRECTLY at the gateway, NOT through Squid). Hermes's dummy key unlocks nothing upstream.

### 4.2 Hard cost ceiling — CONCRETE, token-based, persisted [blocker 6, Cipher-pinned values]
"100 req/day is **not** a token ceiling." Pin every dimension as a number and enforce it (values below are Cipher-approved; only the daily number is operator-tunable upward):
- **One model:** exact snapshot **`gpt-4o-mini-2024-07-18`** (NOT the drifting `gpt-4o-mini` alias) · **one API path:** `POST /v1/chat/completions` · reject any other model/path. **At build, confirm that snapshot is present/available on the real key WITHOUT printing the key.**
- **Text-only, NON-STREAMING ONLY for v1.** Do NOT carry a streaming branch. If Hermes proves incompatible with non-streaming during build, **STOP and return for a streaming amendment** — do not implement two live cases. (A single non-streaming path guarantees the authoritative final `usage` for reconcile.)
- **Input ceiling 4,096 tokens across the FULL serialized request** — system prompt + conversation history + tool schema + tool results, NOT just the latest DM. Count the whole outbound payload.
- **Token-counting contract — PINNED tokenizer [blocker 10].** Counting uses **`tiktoken` with the `o200k_base` encoding — the encoding matching `gpt-4o-mini-2024-07-18`** (pin the tiktoken library version at build; the encoding is the contract, not a heuristic char/4 estimate). Count over the **FULL reconstructed upstream request** (the gateway-built body of §4.3 — system prompt + history + the `incluir_lookup` tool schema + tool results + message framing), NOT Hermes's raw input. **Fail-closed if the tokenizer is unavailable or the encoding is unknown/unloadable → reject (503/kill-closed), never fall back to an estimate.** Counting MUST cover **adversarial Unicode** (multi-byte / combining / astral / normalization-sensitive text counts by real BPE tokens, not code points) and the **tool-schema boundary** (the serialized JSON schema is counted, and a request that would tip over 4,096 at the schema boundary is rejected — boundary-probed in §6a). **Reserve CONSERVATIVELY (worst-case): reserve the tokenized input + the full 1,024 output atomically BEFORE dispatch** so the running total can never overshoot the 50,000/UTC-day ceiling before the authoritative `usage` returns; reconcile down from the returned `usage` (§4.2 reserve bullet).
- **`max_completion_tokens` = 1,024** · **concurrency 1** · request ceilings **10/min + 100/day**.
- **HARD daily TOKEN ceiling = 50,000 total tokens/day (UTC-day)** (kill-closed on breach) — START here (≈10 worst-case calls or many normal lookups); **raise only from observed legitimate usage with operator approval**. This is the real ceiling, separate from the request count.
- **Reserve tokenized-input + 1,024 output ATOMICALLY and CONSERVATIVELY (worst-case) before upstream dispatch [blocker 10]** — using the pinned `o200k_base` count above so the reservation can never under-count and let the day's running total overshoot 50,000 before authoritative usage returns; **reconcile from the authoritative returned `usage`** (release the unused reservation); **kill-closed** (reject, don't queue) when the budget is exhausted. Counters **persist across gateway restart** (durable store) and **reset on the defined UTC-day boundary**. **Fail closed if usage/counter persistence OR the tokenizer is unavailable.**

### 4.3 Body reconstruction — the gateway builds the upstream request, never forwards Hermes's [blocker 9]
The gateway does **NOT** generically forward Hermes's JSON. It **reconstructs an allowlisted upstream body** from scratch: fixed `model` (the snapshot) + fixed path, `store=false`, **text-only** messages, **bounded message count (≤ 20, §4.4 [blocker 9])**, ONLY the `incluir_lookup` tool schema/name. It **drops/ignores** any caller-supplied upstream URL, model, `Authorization`, metadata, `files`, `images`, `audio`, streaming toggles it doesn't set, or extra tools. Hermes can influence only the text content within the caps — nothing about the upstream request shape.

- **Threat-model limitation (stated plainly, per Cipher):** the gateway's ceiling bounds **Hermes-originated** OpenAI use only. It does **not** bound the shared "General" key's **global blast radius** — if the gateway container itself is compromised, the shared key it holds is exposed to whatever that key can do across every other consumer of General. This is an inherent residual of the operator's shared-vs-dedicated-key choice; the gateway mitigates cost-amp + third-party-Hermes theft, not a gateway-host compromise. A dedicated Hermes-only key would cap that global blast radius; **the operator has accepted this residual — do not reopen.**

### 4.4 Pre-parse hardening limits [blocker 11]
The gateway's inbound HTTP surface (Hermes→gateway on `hermes-llm`) enforces, **BEFORE body parse**, these EXACT numeric limits [blocker 9 — no qualitative limits; each is boundary-probed in §6a]:
- **max body bytes = 16 KiB** (headroom for the full serialized chat request up to the 4,096-token input ceiling; larger → rejected pre-parse),
- **max header count = 30**,
- **max per-header bytes = 8 KiB**,
- **max total-header bytes = 32 KiB**,
- **header timeout = 2 s**, **read timeout = 5 s**, **slow-body / slowloris timeout = 5 s**,
- **max message count = 20** (the reconstructed §4.3 `messages[]` array is capped; more → rejected),
- **strict `Content-Type: application/json`** (reject else).
Same discipline as the broker (§5); each limit has a boundary probe in §6a (at-limit accepted, one-over rejected). A hung or oversized/slow client cannot wedge the gateway or hold the single concurrency slot.

## 5. BROKER sidecar (holds BOTH the ingress + signing secrets; one fixed typed lookup) (blockers 6 + 10)
> The broker is the SOLE holder of **BOTH** `HERMES_BOT_INGRESS_SECRET` (ingress HMAC) **and** `HERMES_BOT_SIGNING_SECRET` (service HMAC) — Hermes holds NEITHER (§8). It signs both envelopes per request (below).
- Tiny service (Node/Go, ~1 file). Exposes to HERMES a **fixed typed API** mirroring the tool — **ONE typed lookup** (`by-name`), not two. **POST-only JSON** (strict — names never enter a URL): `POST /lookup/by-name {name}`. **No GET alternative** (blocker 10). Strict zod/schema, length/char caps.
- **Pre-parse hardening limits [blocker 11 + blocker 9 — EXACT numbers, each boundary-probed in §6a] — enforced BEFORE body parse:**
  - **max body bytes = 2 KiB** (headroom over the ≤1 KiB tool payload; larger → rejected pre-parse),
  - **max header count = 30**,
  - **max per-header bytes = 8 KiB**,
  - **max total-header bytes = 32 KiB**,
  - **header timeout = 2 s**, **read timeout = 3 s**, **slow-body / slowloris timeout = 3 s**,
  - **strict `Content-Type: application/json`** (reject else). At-limit accepted, one-over rejected.
- **Bounds mirror Lane 1 v3.6 exactly:** exact-name match on the singular `user` table with all joins **soft-delete-filtered (`deleted_at IS NULL`)**, **dedup-to-people + deterministic latest-enrollment then LIMIT 6 (after dedup AND after soft-delete filters) / return ≤5**, **`tooMany:true` with NO count** on a 6th person, **single concurrent** lookup, **10/min + 100/day**. No count of "too many."
- **Deadlines with ACTUAL cancellation** (blocker 6/10): 2s DB (Lane 1 enforces) / **3s broker** / **5s tool** — real abort/cancel, not an abandoned promise.
- **Broker → Next proxy → Hono (Lane 1 v3.6 §2) — TWO domain-separated HMACs [blocker 4]:** the broker calls `POST https://app.programaincluir.org/api/bot/students/by-name` **via Squid listener B** (broker has no direct route). The broker signs the request **TWICE**:
  - **INGRESS envelope** with `HERMES_BOT_INGRESS_SECRET` — Lane 1 §2.0 canonical (DISTINCT aud constant `"incluir-bot-ingress"`, decimal timestamp, 32-hex nonce, `sha256hex(exact raw ≤1KiB body bytes)`), carried in DISTINCT headers `X-Hermes-Ingress-Timestamp/Nonce/Signature`. The **Next proxy** verifies this constant-time (±60s), **atomically CONSUMES the ingress nonce** (`SET bot:ingress-nonce:<nonce> NX EX 180` — TTL strictly greater than the ±60s = 120s validity span, so a replay cannot outlive its nonce entry), and **STRIPS the ingress headers before any Hono call**; a public caller lacking this key is terminated at Next, reaching zero Hono, and a replayed ingress envelope is rejected at Next.
  - **SERVICE envelope** with `HERMES_BOT_SIGNING_SECRET` — the existing Lane 1 §2.1 canonical (aud `"incluir-bot"`, decimal timestamp, 32-hex nonce, `sha256hex(exact raw body bytes)`), headers `X-Hermes-Aud/Timestamp/Nonce/Signature`, **±60s window, one-time 128-bit nonce**. Next forwards it UNTOUCHED; Hono verifies it FIRST. Lane 1 rejects expiry / replay / wrong-route / malformed.
  - **Strict domain separation:** distinct secrets, distinct aud constants, distinct header names; neither key is reused and neither layer accepts the other's signature. **No durable `X-Hermes-Token` bearer** — a captured request is not a reusable credential. Broker is the SOLE holder of BOTH `HERMES_BOT_INGRESS_SECRET` and `HERMES_BOT_SIGNING_SECRET`; Hermes holds NEITHER.
- **HMAC secret rotation [blocker 10] — BOUNDED-DOWNTIME, single mechanism, BOTH keys:** (1) **stop** the broker / Hermes tool path; (2) update **both** secrets on **both holders** — `HERMES_BOT_INGRESS_SECRET` on the **Next proxy** + broker, `HERMES_BOT_SIGNING_SECRET` on **xerox/Hono** + broker; (3) **start** the broker; (4) **prove BOTH verifiers rotated INDEPENDENTLY [blocker 7] — three tests, replacing the single "old signature rejected".** **Signature-mismatch isolation [v3.5 blocker]: each mixed-key probe MUST be FRESHLY SIGNED with an IN-WINDOW timestamp and a FRESH UNUSED nonce, and the test MUST ASSERT the rejection is SPECIFICALLY signature mismatch — never expiry, never replay (a probe rejected for expiry/replay is a FALSE PASS that could mask a still-accepted old key):** (i) **old-ingress + new-service → rejected AT NEXT for INGRESS signature mismatch**; (ii) **new-ingress + old-service → rejected AT HONO for SERVICE signature mismatch** (the fresh ingress envelope passes + consumes its nonce at Next, so the rejection is provably the service-HMAC check, not expiry/replay); (iii) **new-ingress + new-service → accepted.** **NO dual-key implementation, NO key-id header, NO overlap branch** for either secret. The rotation drill (§6a) exercises exactly this sequence.
- **Response field re-projection (defense-in-depth, MUST match Lane 1 §4 exactly):** strip anything not in the exact set `{ studentName, activeThisSemester, academicOutcome, enrollmentSemester, className, courseName, attendanceSummary }`. `activeThisSemester` is the derived live-student ⋈ live-active-class ⋈ live-active-semester boolean (Lane 1 §4) — it REPLACES the old `enrollmentActive`/`students.is_active` field, which is DROPPED. **`academicOutcome` is INCLUDED** — the operator explicitly accepted (GLaDOS-confirmed 2026-07-30) that pending/approved/rejected may cross OpenAI + Slack; grades remain out regardless. The OLD names `enrollmentStatus` / `currentSemester` / `enrollmentActive` are **REJECTED** (Lane 1 uses `activeThisSemester` / `enrollmentSemester`). The legacy `students.is_active` must never appear. Deterministic minimized JSON. Tool output is data, never instructions.

## 5a. AUDIT ACTOR — authenticated context, not inference (blocker 5)
"Any lookup implies the static operator" is **INFERENCE, not authenticated context** — a model-initiated or compromised-Hermes call would be **falsely attributed to the human**. Spec BOTH paths:

- **Preferred (authenticated context):** the trusted Slack adapter injects an **authenticated `slack_user_id` + `slack_event_id` OUTSIDE model-controlled tool args**, and the broker verifies an authenticated per-event context / capability before the lookup. The actor crosses HERMES→BROKER as authenticated context, never as free data.
- **Fallback (if the Hermes framework cannot do that cleanly — assume it likely cannot):** broker logs `service_actor=hermes` **ONLY**, and the **Slack gateway SEPARATELY records** the initiating authorized event. **Do NOT claim the human made a specific lookup** — the two logs are correlated out-of-band, not fused into a false "operator X looked up Y."
- Either way: no raw PII / token / name / nonce in the audit; op + outcome + result-count + timestamp only.

## 6. HERMES container (hardened, chat + 1 tool) (blockers 8 + 9)
- Image `nousresearch/hermes-agent@sha256:<pinned-digest>` (pin digest after SBOM/CVE scan; reconcile the memory-provider-bypass advisory ≤0.16.0 vs the v2026.7.20 tag — record `hermes --version`, black-box prove the effective tool inventory, REJECT image if the affected memory/plugin path is present).
- Config: chat + ONE tool `incluir_lookup` → points at the BROKER only (via `hermes-broker`); `OPENAI_BASE_URL` → the OpenAI-gateway's **static internal `hermes-llm` address, DIRECT (NOT through Squid)**; dummy OpenAI cred. DISABLE + runtime-prove-absent: terminal/shell, filesystem, browser/web, code-exec, delegation/subagents, cron, MCP, hooks, plugins, installers, durable-memory. Escape knobs off (HERMES_YOLO_MODE / HERMES_ACCEPT_HOOKS / SUDO_PASSWORD unset). Drop the `dashboard` service.
- **Slack — DM-only, pinned at MANIFEST **and** RUNTIME (blocker 9):** only the required **Socket Mode + `message.im`** scopes/events. **NO** channels / groups / app-mentions / message history beyond what Bolt strictly needs. **Bot + subtype events ignored.** Fail-closed allowlist: `SLACK_ALLOWED_USERS=<operator member id>`, `GATEWAY_ALLOW_ALL_USERS=false`, `SLACK_ALLOW_BOTS=none`, `unauthorized_dm_behavior=ignore`. A **denied user/bot/channel event → ZERO OpenAI, ZERO broker, ZERO tool call** (request-count evidence, §6a).
- **Hardening — PROVEN, not described (blocker 8):** every process runs **non-root** (UID10000; pre-own the state volume so no root chown at boot) — **no root s6 supervisor remains.** `cap_drop: ALL` + `no-new-privileges`, `read_only: true` rootfs + tmpfs `/run` `/tmp`, `pids_limit`, cpu/mem limits, NO host binds, NO docker.sock, NO published ports, NOT on host network. **Prove seccomp + AppArmor are actually ENFORCED by OrbStack** via `docker inspect` + `/proc/<pid>/attr` process attributes (§6a) — not merely declared. **Waiver caveat:** if the official Hermes image cannot run fully as UID10000 under read-only root, the waiver does **not** hold without explicit reassessment — flag it to Cipher/operator, do not silently relax.

## 7. PII retention + telemetry hygiene (blocker 8)
- **Disable prompt / tool-payload logging** in HERMES, BROKER, OPENAI-GATEWAY, SQUID, Docker stdout, and any telemetry pipeline — **metadata only** (op / outcome / count / timestamp).
- **Squid + gateway stdout MUST NOT log URLs, `Authorization`, prompts, or response bodies** — Squid logs host+listener+allow/deny only; the gateway logs op/outcome/token-count only, never the key or the prompt/response.
- **State volume:** define EXACTLY what the `0700` state volume stores; **prove durable-memory / conversation-history is OFF**; **exclude the volume from backups**; **cap Docker logs** (size + rotation); set explicit deletion / retention.
- Note: **`HERMES_REDACT_SECRETS` does NOT redact student PII** — it only touches secrets. PII containment is the no-logging + no-history + volume-scope discipline above, not that flag.

## 8. Secrets (host-side injection)
- Resolve at deploy on the host from Infisical: **OpenAI shared "General" key → OPENAI-GATEWAY only** (§4); `SLACK_BOT_TOKEN` + `SLACK_APP_TOKEN` → HERMES; **BOTH** `HERMES_BOT_SIGNING_SECRET` (service HMAC) **and** `HERMES_BOT_INGRESS_SECRET` (ingress HMAC) → **BROKER only** (not Hermes). Separately, `HERMES_BOT_SIGNING_SECRET` → **xerox/Hono** and `HERMES_BOT_INGRESS_SECRET` → **the Next proxy** (Lane 1 §9) — distinct secrets, each held only by the broker + its one verifier. Hermes gets a **dummy** OpenAI cred and NEITHER HMAC secret. Inject as env at container start; containers never reach Infisical/tailnet (consistent with the egress deny). 0600 rendered env, shredded post-start.

## 6a. Runtime probe evidence set (for Cipher pre-go-live)
**Network / listener negatives (all must FAIL from the respective container):**
- HERMES → host-gateway:54321 (incluir dev DB) · → 100.102.73.112:3005 (tailnet Infisical) · → watchtower :3002/:3001/:8090 · → 169.254.169.254 (metadata) · → a raw public IP:443 · → arbitrary public host (example.com:443) · → any non-443.
- **Wrong-listener domain probes — cover ALL THREE listeners (§6):**
  - HERMES-side (listener A, Slack-only) **cannot reach `api.openai.com`** and **cannot reach `app.programaincluir.org`** — only Slack.
  - HERMES **cannot reach OpenAI directly** either: it is not on `openai-proxy`, so `api.openai.com` is reachable ONLY through the gateway.
  - BROKER-side (listener B) **cannot reach openai OR slack** — only `app.programaincluir.org`.
  - GATEWAY-side (listener C) **cannot reach slack OR incluir** — only `api.openai.com`. Gateway has NO `ext-net`, so a direct dial to OpenAI (bypassing listener C) must also FAIL.
- **Reverse-reach negatives [blocker 9]:** from SQUID, attempt to reach the **BROKER** and the **GATEWAY** on every segment Squid touches (`broker-proxy`, `openai-proxy`, and any shared segment) — all must **FAIL**. Prove the broker binds only its `hermes-broker` address and the gateway binds only its `hermes-llm` address (bound-address inspection), so neither is reachable from Squid on the wrong segment (no `0.0.0.0` bind).

**DNS (blocker 7 / §6):**
- **CONFIG-LEVEL + query-count DNS-exfil proof:** show compose `dns:` / `dns_search` / `dns_opt` / `extra_hosts` / resolver config proving arbitrary-name queries from HERMES / BROKER / **GATEWAY** never egress, with packet-level or resolver-query-count evidence — WHILE Slack Socket Mode connects/reconnects through Squid listener A. A single failed `nslookup` is NOT sufficient.

**Proxy parser / SSRF / rebinding (blocker 3 / §6):**
- Authority-parser probes: userinfo (`u@host`), IP-literal, trailing-dot, uppercase/mixed-case, non-canonical authority — all REJECTED.
- IPv4 **and** IPv6 SSRF probes into loopback/link-local/RFC1918/CGNAT/ULA/multicast/metadata — all REJECTED after re-resolution.
- **DNS-rebinding probe:** a name that resolves public-then-private → REJECTED at dial time.

**OpenAI key containment + gateway credential handling (blockers 6, 9 / §6):**
- The **real OpenAI key is ABSENT** from HERMES — prove via `docker inspect`, env dump, and file scan of the Hermes container. Gateway is the only holder.
- **Gateway strips inbound `Authorization` and injects the real key itself;** caller-selected upstream URL / host / arbitrary headers are **REJECTED**; the key **never** appears in any gateway response, error body, stdout, or telemetry.
- **Hard token ceiling [blocker 10]:** prove the gateway counts with **pinned `tiktoken` / `o200k_base` (matching `gpt-4o-mini-2024-07-18`)** over the FULL reconstructed request, **reserves input + max-output atomically and CONSERVATIVELY (worst-case) before dispatch, reconciles with returned `usage`, and kill-closes at the 50,000-token/UTC-day ceiling** without overshoot; counters **survive a gateway restart** (stop/start the gateway, confirm the budget did not reset). Also prove: **an unavailable/unknown tokenizer → fail-closed (reject, no char-estimate fallback)**; an **adversarial-Unicode** payload (combining/astral/normalization-sensitive) is counted by real BPE tokens; and a request that tips over 4,096 at the **tool-schema boundary** is rejected.

**Signed-request rejection (blocker 1,2 / §6):**
- Expired (>±60s), replayed nonce, wrong-path/wrong-route, and **malformed/duplicate-header / oversize-body** signatures all **REJECTED** by Lane 1 before any query; **nonce is only inserted after a valid SERVICE signature** (verification order — service HMAC verified FIRST, no pre-HMAC bucket).

**Two-HMAC representable contract (blocker 4 / Lane 1 §2.0–§2.2, §6, §8) — exactly these five tests:**
- **(a) Public invalid flood → zero Hono AND cannot block a valid broker call.** A public flood without a valid INGRESS HMAC is terminated at the Next proxy (4xx), makes **ZERO Hono requests**, and does NOT block a subsequent valid broker call.
- **(b) Correct ingress HMAC + bad service HMAC → cannot consume/block the valid bucket.** A request with a valid ingress HMAC but invalid service HMAC reaches Hono, is rejected after the service-HMAC check, consumes only the invalid-only bucket, and cannot consume/lock the valid-request bucket.
- **(c) Valid request succeeds AFTER both floods** — a fully valid request (both HMACs correct) still succeeds within its 10/min+100/day budget after floods (a) and (b).
- **(d) Cross-key signatures fail** — the broker's ingress key cannot sign a valid service envelope and the service key cannot sign a valid ingress envelope (distinct aud constants + distinct secrets + distinct headers).
- **(e) BOTH verifiers rotated independently after bounded-downtime rotation [blocker 7], each probe FRESHLY SIGNED + signature-mismatch-isolated [v3.5 blocker]** — after §5 rotation of BOTH keys, prove all THREE, **each mixed-key probe freshly signed with an in-window timestamp + fresh unused nonce and the rejection asserted to be SPECIFICALLY signature mismatch (never expiry/replay):** **old-ingress + new-service → rejected AT NEXT for ingress signature mismatch**, **new-ingress + old-service → rejected AT HONO for service signature mismatch**, **new-ingress + new-service → accepted**; no dual-key/overlap path.

**Hardening — ALL FOUR components + 3 listeners (blocker 8 / §6):**
- **Non-root process list for HERMES, BROKER, OPENAI-GATEWAY, and SQUID** — no PID runs as root, no root s6 supervisor.
- Per component, prove: **cap set** (`cap_drop: ALL`), **read-only mounts** + tmpfs, **seccomp + AppArmor ENFORCED** via `docker inspect` + `/proc/<pid>/attr` (OrbStack, not just declared), **secret absence** (only the intended secret in the intended container), **resolver state** (no usable recursive resolver on Hermes/Broker/Gateway), and **PII-log absence**.
- **Squid + gateway stdout carry NO URLs, `Authorization`, prompts, or response bodies.** Squid binds exactly THREE listeners (A/B/C) on their respective segments.

**Pre-parse hardening — BROKER and GATEWAY (blocker 11 / §4.4, §5):**
- Per service, a **boundary probe for EACH EXACT limit [blocker 9]** — at-limit accepted, one-over rejected: body at the cap (broker 2 KiB / gateway 16 KiB) accepted, one byte over → rejected pre-parse; header count 30 accepted, 31 → rejected; per-header 8 KiB accepted, 8 KiB+1 → rejected; total headers 32 KiB accepted, over → rejected; wrong/missing `Content-Type` → rejected; a stalled connection that never sends headers → closed at the **header timeout** (broker 2 s / gateway 2 s); a **slow-body / slowloris** drip → closed at the slow-body timeout (broker 3 s / gateway 5 s) and the read timeout (broker 3 s / gateway 5 s) fires, **without holding the single concurrency slot**. **Gateway additionally: a reconstructed request with 21 messages (> the max message count of 20) → rejected [blocker 9].**

**PII / telemetry (blocker 8 / §6):**
- **Zero PII** in Docker stdout, traces, and the state volume — inspect logs + volume contents + telemetry attrs across all four components; metadata only.

**Slack negatives (blocker 9 / §6):**
- Channel event, bot event, and app-mention event each yield **ZERO OpenAI + ZERO broker + ZERO tool** (request-count evidence, not just Slack silence). Denied user → same.

**Positives (must SUCCEED):**
- HERMES → OpenAI-gateway **directly** on `hermes-llm` (dummy cred) → gateway injects real key → gateway → Squid listener C → `api.openai.com` returns a completion. Slack wss gateway via listener A: Bolt connects + RECONNECTS after a forced drop.
- BROKER → `app.programaincluir.org/api/bot/*` via listener B returns minimized JSON (signed request accepted).
- Tool inventory dump proves only `incluir_lookup` present; shell/fs/browser/memory invocation attempts all fail.

**Operational drills:**
- Kill switch: `docker stop hermes` (+ broker + gateway). **HMAC rotation drill [blocker 10] — bounded-downtime sequence, BOTH keys:** stop the broker/Hermes tool path → update `HERMES_BOT_INGRESS_SECRET` on the **Next proxy** + broker AND `HERMES_BOT_SIGNING_SECRET` on **xerox/Hono** + broker → start the broker → **prove BOTH verifiers independently [blocker 7], each mixed-key probe FRESHLY SIGNED (in-window timestamp + fresh unused nonce) and rejected SPECIFICALLY for signature mismatch, never expiry/replay [v3.5 blocker]: old-ingress+new-service → rejected AT NEXT (ingress sig mismatch), new-ingress+old-service → rejected AT HONO (service sig mismatch), new+new → accepted**. No dual-key / key-id / overlap path for either secret. Also rotate the shared OpenAI key (gateway) + re-verify.
- OpenAI cost ceiling: gateway enforces the concrete caps (§4.2) — 10/min + 100/day request bounds + **hard daily TOKEN ceiling** with atomic reserve/reconcile, kill-closed on breach + alerts; counters survive restart.

## 9. OPEN GATES (operator/Cipher — gate the BUILD/deploy, not the design)
1. **GATE — operator Slack member id** → `SLACK_ALLOWED_USERS` + the Slack gateway's authorized-event identity. **Pending.**
2. **Model / API path / daily TOKEN ceiling — PINNED per §4.2 (the artifact is authoritative):** exact snapshot **`gpt-4o-mini-2024-07-18`** (NOT the `gpt-4o-mini` alias) · `POST /v1/chat/completions` · 4,096 in / 1,024 out / concurrency 1 · **hard daily TOKEN ceiling = 50,000 tokens/UTC-day**. Only an **UPWARD** change to the daily number is operator-gated (raise from observed legitimate usage); no operator confirmation is required to ship at these pinned values.
3. **GATE — empirically-minimal Slack `*.slack.com` gateway host suffix set** that satisfies Bolt reconnect — determined **empirically** during build, pinned to the minimum for the listener-A allowlist. (Residual: suffix, not exact-host — documented §3.) Pending.
4. Squid vs small-Go-proxy final call — Squid preferred pending its SBOM/CVE pass; Go path only after adversarial review (§3).
5. **GATE — custom BROKER + OPENAI-GATEWAY artifacts require pinned source review + SBOM / dependency / CVE / provenance** before go-live (these are bespoke artifacts holding BOTH HMAC secrets — `HERMES_BOT_INGRESS_SECRET` + `HERMES_BOT_SIGNING_SECRET` (broker) — and the shared OpenAI key (gateway); no unreviewed image ships). Pending.
6. **GATE — concrete state-volume + log CONTENTS + retention + rotation must be DEFINED** (exactly what the `0700` state volume stores, what each component's logs contain, retention window, rotation policy) — metadata-only, no PII/secret retained (§7). Pending.

**Note (NOT a gate):** the shared-"General"-OpenAI-key residual (§4 threat-model note) is **already operator-accepted — do not reopen.** **academicOutcome is SETTLED — INCLUDED:** the operator explicitly accepted it crossing OpenAI + Slack (GLaDOS-confirmed 2026-07-30); it is NOT an open gate and appears in the broker re-projection (§5) / Lane 1 §4.

## 10. Build order (after Cipher passes BOTH lane specs + operator gates land + Rex's Lane 1 v3.6 is live on xerox)
Squid proxy (**three listeners A/B/C**, each bound to its own internal segment) → OpenAI-gateway (holds shared key, validate-only surface, concrete token ceiling; on `hermes-llm`+`openai-proxy`, NO ext-net) → broker (POST-only, signs BOTH the Lane 1 §2.0 ingress envelope + the §2.1 service envelope to live /api/bot/*, re-projects Lane 1 v3.6 fields incl. `activeThisSemester`) → Hermes container (tool→broker, `OPENAI_BASE_URL`→gateway **directly** + dummy key) → wire Slack (DM-only manifest, via listener A) → run the §6a probe set (**5-segment / 3-listener** network/listener negatives incl. all-three-listener wrong-listener probes + Hermes-can't-reach-OpenAI-except-via-gateway + **Squid-reverse-reach negatives (blocker 9)** + DNS-config/query-count + parser/SSRF/rebinding + **broker/gateway pre-parse hardening (blocker 11)** + key-absent + gateway-credential-handling + token-ceiling + signed-request + **two-HMAC blocker-4 five-test set** + **four-component** non-root/caps/read-only/seccomp/AppArmor/secret-absence/resolver/PII + zero-PII + Slack negatives + HMAC-rotation drill **(both keys)**) → send Cipher the evidence → go-live on her PASS.
