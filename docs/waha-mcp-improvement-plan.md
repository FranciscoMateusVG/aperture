# Plan: WAHA/WhatsApp MCP improvement (pull-based) — mcp-for-claude

**Author:** Wheatley (planning) · **Executor:** Peppy · **Date:** 2026-08-08
**Target:** `mini:~/projects/mcp-for-claude` (Node/TS stdio MCP, NOT git — source-of-truth is the Mini dir; deploy = `npm run build` then restart openclaw). **WAHA:** v2026.7.2 NOWEB @ localhost:3002.
**Operator constraint (hard):** pull/polling only — NO webhook/push architecture. Improvement (D) is OUT.

> **STATUS: ALL 6 PHASES SHIPPED + deployed + end-to-end verified (2026-08-08, Peppy).** This doc has been updated post-ship: each phase carries a **⚠️ SHIPPED-CORRECTION** note where the live 2026.7.2 build diverged from pre-build research (the Phase-0 verify-first gate caught a real divergence at every phase). The doc now reflects shipped reality, not just the original plan.

## 0. Pre-flight verification (do FIRST, before coding any phase)
Research was against WAHA docs, not the exact 2026.7.2 build. Before each phase, confirm request/response shapes against the LIVE ground truth:
```
curl -s localhost:3002/swagger/openapi.json | jq '.paths | keys'   # confirm endpoint paths
# spot a real message object to see ack/media/id shapes on THIS build:
curl -s -H "X-Api-Key: $KEY" "localhost:3002/api/{session}/chats/all/messages?limit=5&downloadMedia=false" | jq '.[0]'
```
This is cheap and prevents spec*-vs-build drift (the ack field name, media object shape, and send-response id shape all had version churn upstream).

---

## Phase A — ack-based unread detection (TOP PRIORITY, the live correctness fix, standalone-shippable)

**Problem (confirmed in code):** unread is filtered on `c.unreadCount > 0` at `get-unread-messages.use-case.ts:32` and `get-whatsapp-chats.use-case.ts:31`; raw read at `waha.repository.ts:419` (`chat.unreadCount ?? 0`). NOWEB returns `unreadCount` null/0 for live-arrived messages → "0 unread" while genuinely-unread messages sit at ack=DEVICE. **No `ack` field is read anywhere in the codebase.**

**Confirmed API facts:**
- ack enum: `ERROR=-1, PENDING=0, SERVER=1, DEVICE=2, READ=3, PLAYED=4`.
- NOWEB reliably populates ack on incoming since WAHA 2025.4 (`DEVICE=2` is the resting value for a genuinely-unread incoming message).
- **Unread rule: `fromMe === false && ack < 3`** (READ=3 and PLAYED=4 both mean the operator has seen it).
- Messages endpoint supports `chatId=all` on NOWEB (one call across all chats — the efficient polling primitive) + server-side `filter.fromMe=false`, `filter.timestamp.gte/lte`. `filter.ack` is exact-equality (no `<` operator), so fetch `filter.fromMe=false` and threshold `ack < 3` client-side.

**Changes:**
1. `waha.repository.ts` — add `ack: number` (and `ackName?: string` if present on this build) to the `WahaMessage` type. **Also fix the misleading comment at :58**: `hasMedia` is currently commented "Whether message has been read" — it's the media flag; correct the comment so the ack work isn't sabotaged by it.
2. New repo method `getUnreadMessages(session, {messagesPerChat})` keeping messages with `fromMe=false && ack < 3`. **⚠️ SHIPPED-CORRECTION (Phase A, verified live on 2026.7.2 — Peppy):** the efficient `GET /chats/all/messages` single-pull is NOT viable yet — on this build it returns `chatId: null` AND keys DIRECT chats by the sender's `@lid` (not the `@c.us` chat id), so DMs mis-group and unread DMs get silently DROPPED (groups were fine — `from`=group JID). **Use the per-chat-iteration path: loop chats from `/chats/overview`, pull per-chat messages (chatId known from the loop = correct for DMs+groups), classify `ack<3`.** ~240ms over 50 chats on localhost — acceptable. The efficient `chats/all` path becomes viable only AFTER Phase D adds `@lid→@c.us` resolution; revisit then. Also EXCLUDE `status@broadcast` (WhatsApp status/stories ride the same stream and would inflate the count).
3. `get-unread-messages.use-case.ts` — replace the `unreadCount > 0` filter with the ack-based classification. Unread count per chat = number of `fromMe=false && ack<3` messages, not the WAHA field.
4. `get-whatsapp-chats.use-case.ts` `unreadOnly` (:31) — same: a chat is "unread" iff it has ≥1 `fromMe=false && ack<3` message. If keeping this cheap, compute from the same all-messages pull rather than per-chat round-trips.
5. Leave `unreadCount` in the raw type but STOP deciding on it; optionally surface it as advisory only.

**Acceptance:** with 2 genuinely-unread live messages (fromMe=false, ack=2) sitting in a chat whose `unreadCount` is null/0, `get-unread-messages` reports them; a chat where the operator has read everything reports 0. Prove against the live pain case from today. **Passive-read invariant (see Phase F) already holds here — reading via GET marks nothing.**

---

## Phase B — store-first pairing + headless QR (correctness for history + openclaw usability)

**Problem:** no store-config call anywhere → search-groups/contacts silently empty when store is off (today's bite). And `get-whatsapp-qr` writes `/tmp/whatsapp-qr.png` and shells `open` (`waha.repository.ts:377-383`) — useless when openclaw drives headless.

**Confirmed API facts:**
- Store is **per-session** config (no global env in current docs): `config.noweb.store.enabled` (default false) + `config.noweb.store.fullSync` (default false; false≈3mo history, true≈1yr).
- **Ordering constraint (critical): WhatsApp only pushes history on a FRESH device link.** Docs warn changing store values after QR scan can LOSE history. So store must be set BEFORE pairing; enabling on an already-linked session means unlink + re-pair.
- Read current config: `GET /api/sessions/{session}` → `config.noweb.store`, `status`. Set: `PUT /api/sessions/{session}` (or `POST /api/sessions` on create) then `POST /api/sessions/{session}/start`, THEN `GET /api/{session}/auth/qr`.

**Changes:**
1. New repo helpers: `getSessionConfig(session)` (GET sessions/{s}), `ensureStoreEnabled(session, {fullSync})` (check-and-set with the ordering guard).
2. `get-whatsapp-qr` / `whatsapp-start-session` flow → check-and-set store BEFORE returning/generating QR:
   - If session not linked: PUT store.enabled=true (+fullSync per config), start, then QR.
   - If already WORKING with store disabled: DO NOT silently toggle (history-loss risk) — return a clear warning that re-pairing is required to backfill history, and only toggle on an explicit force/re-pair path.
3. **Headless QR:** stop shelling `open`; return the QR as inline base64 (already available in the response per the map) so openclaw renders it. Drop the /tmp png write (or keep as optional debug only).

**Acceptance:** a fresh pair enables store before QR; post-link, search-contacts/groups return data; QR tool returns base64 inline with no `open` shell-out; an already-linked-store-off session yields a warning, not silent history loss.

---

## Phase C — media understanding (on-demand, reuse the MCP's OpenAI wiring)

**Problem:** `downloadMedia` hardcoded false at `waha.repository.ts:210`; media rendered as literal `[media]`; no download tool. A real group media message today was uninterpretable.

**Confirmed API facts:**
- Media object on a message: `media.url` (e.g. `http://localhost:3002/api/files/<id>.<ext>`), `media.mimetype`, `media.filename`, `media.error`; top-level `hasMedia`.
- `?downloadMedia=true` populates `media.url`; **`hasMedia:true` with `media:null` can happen** (not downloaded) → fetch single message on demand: `GET /api/{session}/chats/{chatId}/messages/{messageId}?downloadMedia=true`.
- **The `/api/files/...` URL requires the API key** (`X-Api-Key`) — the MCP must attach it when pulling bytes.
- Branch on `mimetype` prefix: `audio/*` (voice notes, opus/ogg) → transcription; `image/*` → vision/OCR. No rich `type` enum on the pull object — use mimetype.
- Caveat: media may expire per `WHATSAPP_FILES_LIFETIME` → fetch promptly at read time.

**Changes:**
1. New tool `get-whatsapp-media` (or extend get-whatsapp-messages with an opt-in `interpretMedia`): given chatId+messageId, fetch with `downloadMedia=true`, pull bytes from `media.url` WITH the API key.
2. Route by mimetype through the existing OpenAI wiring: `audio/*` → speech-to-text (transcription); `image/*` → vision description + OCR text. Return the interpreted text inline where `[media]` used to render.
3. On-demand only (pull-based) — no auto-download-everything loop; interpret when the operator/agent asks about that message.

**Acceptance:** a voice note returns a transcript; an image returns a description + any OCR'd text; a document returns filename+mimetype (interpretation optional). API key correctly attached to file fetch (no 401).
**Model choice:** consult the `openai-models` skill for current STT + vision model ids before pinning — do NOT default to whisper-1/gpt-4o from memory.
**⚠️ SHIPPED-CORRECTION (Phase C, verified live — Peppy):** `media.url` on this build points at an INTERNAL host (the WAHA container's own address), not a client-reachable one — the MCP must fetch bytes server-side from that internal URL (with the X-Api-Key attached) and pass the bytes to OpenAI, NOT hand the URL to any external caller. Shipped path: real image/webp → gpt-5.5 vision → correct description ("a gray-brown tabby cat on light blue bedding"). Model id used: gpt-5.5 (per openai-models at ship time — confirm current before re-pinning).

---

## Phase D — sender name resolution (incl. @lid)

**Problem:** chat/contact/group names resolve, but **message sender `msg.from` is shown RAW** at `get-unread:61` → group senders are phone-ids, not names.

**Confirmed API facts:**
- Contacts: `GET /api/contacts/all?session=…` (bulk — build a JID→name cache); fields `name` → `pushname` → `number` (prefer in that order).
- Groups: `GET /api/{session}/groups` / `/groups/{id}` → `subject` is the display name.
- **No batch-by-id endpoint** → hydrate a local cache from contacts/all + groups, resolve per-message from cache, refresh periodically.
- **`@lid` identities:** some 2026-build senders arrive as `@lid` not `@c.us` — resolve via `GET /api/{session}/lids`, `/lids/{lid}`, `/lids/pn/{phone}`, or names won't resolve.

**Changes:**
1. Name-cache module: hydrate from contacts/all + groups (+ lids map), TTL refresh. 
2. Resolve `msg.from` for display everywhere raw JIDs surface (get-unread:61 + wherever sender is rendered), with `@lid`→phone→name fallback chain, ending at the raw number if unresolved.

**Acceptance:** group message senders show contact/push names, not phone-ids; `@lid` senders resolve; unknowns degrade to the number, never crash.
**⚠️ SHIPPED-CORRECTION (Phase D, verified live — Peppy):** two divergences from pre-build research: (1) `contacts/all` itself RETURNS ids as `@lid` on this build, so the cache must key/normalize on `@lid` and carry the `@lid→phone` map from `/lids`, not assume `@c.us` keys; (2) a GROUP message's sender is NOT on `msg.from` (that's the group JID) — it's in the message's participant field (`msg.participant`/`author`-style), so resolve the group-sender from THAT, not `from`. Shipped result: senders render as "Marcio Gonçalves", groups as "PD Pamps 👣Só Amor ❤", with the @lid→phone→name chain working. NOTE: this @lid resolution is the unlock for Phase A's efficient `chats/all` path (see Phase A shipped-correction) — once the @lid→@c.us map exists, the single-pull DM mis-keying can be resolved and `chats/all` becomes viable.

---

## Phase E — real send receipts

**Problem:** sent msg id IS captured (`SendMessageResult.id`), but ack/delivery status is not; and today a send returned an EMPTY id.

**Confirmed API facts:**
- `POST /api/sendText` returns the sent message object incl. id — but id SHAPE varies across builds (top-level string vs `id._serialized` vs `id.id`); group sends + NOWEB timeouts are the common empty-id causes.
- Poll delivery via the same messages endpoint filtered to that chat, match on id, read numeric ack `SERVER(1)→DEVICE(2)→READ(3)`.

**Changes:**
1. Parse the send-response id DEFENSIVELY: accept top-level string `id`, `id._serialized`, `id.id`; if empty, fall back to matching the just-sent message by `chatId + fromMe=true + body + newest timestamp`.
2. Only send when `GET /api/sessions/{session}` reports `status: WORKING` (avoids the empty-id timeout case); generous NOWEB send timeout.
3. Optional `get-send-status(chatId, messageId)` to poll the ack of a sent message on demand (pull-based).

**Acceptance:** send returns a non-empty id (or the fallback recovers it); a follow-up status check reports the ack progression; sends are gated on session WORKING.
**⚠️ SHIPPED-CORRECTION (Phase E, verified live — Peppy):** the empty-id root cause on this build is concrete: the id is returned at **`key.id`** (nested), not top-level and not `id._serialized`/`id.id`. Defensive parse must include `key.id`. With that path added the send returns a non-empty id; WORKING-gate + `get-send-status` shipped as specced.

---

## Phase F — passive read mode (CROSS-CUTTING INVARIANT, applies to all phases)

**Operator requirement:** the bot polls + reads content WITHOUT (a) appearing online and (b) clearing the unread badge on the operator's own phone.

**Confirmed API facts (the three levers):**
- **(a) Not online:** `config.noweb.markOnline:false` at session config (default true) — BUT sends/some requests can still flip online for a window, so ALSO set env `WAHA_PRESENCE_AUTO_ONLINE=False` (and never POST presence, never send typing).
- **(b) Don't clear the badge:** the mark-read endpoints (`POST /api/{session}/chats/{chatId}/messages/read`, legacy `POST /api/sendSeen`) send a read receipt that **syncs across the operator's devices → CLEARS his phone badge and bumps ack to READ=3.** For a read-only assistant these must NEVER be called.
- **(c) Reading is passive:** `GET .../messages` sends no read receipt — fetching/downloading marks nothing seen.

**Changes / invariant:**
1. Session config sets `markOnline:false`; document the `WAHA_PRESENCE_AUTO_ONLINE=False` env requirement on the WAHA instance (Peppy's infra side).
2. **HARD RULE across the whole MCP: never call `messages/read` or `sendSeen`, never POST presence, never send typing.** If a "mark read" tool is ever wanted it must be explicit, operator-invoked, and clearly labeled as badge-clearing — not a side effect of reading.
3. Add a code-level guard/comment at the wahaFetch layer noting these endpoints are forbidden by the passive-read requirement.

**Acceptance:** after the bot polls + reads + interprets media across chats, the operator's phone still shows the unread badges, and the operator never saw the account go online.

---

## Sequencing & deploy
- **A ships first, alone** (the live correctness fix; independent of B–F). Then B (unblocks history/search), then C/D/E in any order, with F's invariant enforced throughout.
- Each phase: verify shapes against live swagger (§0) → change → `npm run build` on the Mini → restart openclaw → manual verify against the real pain case.
- Not git: keep the `.env.bak`/`openclaw.json.bak` discipline; source-of-truth is the Mini dir.

## Open items for Peppy / operator
- Confirm `chatId=all` works on this exact 2026.7.2 build (§0) — it's the efficient Phase-A primitive; per-chat iteration is the fallback.
- `fullSync` true vs false for the store (≈1yr vs ≈3mo history) — operator preference.
- `WAHA_PRESENCE_AUTO_ONLINE=False` must be set on the WAHA container (infra) for Phase F to hold — Peppy's lever.
