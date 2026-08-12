# Plan: Hermes local file storage + Slack retrieval (mcp-for-claude)

**Author:** Wheatley (planning) · **Executor:** Peppy · **Date:** 2026-08-09
**Target:** `mini:~/projects/mcp-for-claude` (stdio MCP, wired into openclaw `mcp.servers.workspace`). NOT git — Mini is source-of-truth; deploy = `npm run build` + `openclaw mcp reload`.
**Requirement (operator):** files stored locally on the Mac Mini; retrieval delivered to him over Slack via Hermes. NO Google Drive, NO Google re-consent. (Places tool + Drive both dropped by operator — this supersedes the round-2 Drive section.)

---

## TL;DR
- **Storage root:** `~/.openclaw/media/hermes-storage/` — under openclaw's media root (so Slack outbound can attach from it) and **verified GC-safe** (no openclaw TTL/sweep touches it).
- **Slack delivery mechanism (verified):** the retrieval tool returns the file's **local path as `mediaUrl`/`mediaUrls`** in its reply payload; openclaw's `normalizeOutboundReplyPayload` + Slack adapter deliver it **inline, single agent turn**. No `message send` CLI side-channel needed.
- **Two prereqs (operator/infra):** (1) add `files:write` (+ `files:read` for inbound) to the Hermes Slack app + reinstall + re-wire token (Peppy's confirmed gating fact); (2) fold `hermes-storage/` into the off-host backup pattern (currently unprotected).
- **One build-time verify gate:** confirm end-to-end that an mcp-for-claude tool result can set `mediaUrl` directly (vs. the agent having to echo the path into its reply) before relying on zero agent glue.

---

## 1. The delivery mechanism (verified live — the design hinge)
openclaw's outbound reply contract (`node_modules/openclaw/dist/reply-payload-*.d.ts` + docs `/nodes/images`):
- `OutboundReplyPayload = { text?, mediaUrl?, mediaUrls?: string[] }`.
- `normalizeOutboundReplyPayload()` extracts these from **"loose tool or agent payload objects"** → a tool's returned payload carrying `mediaUrl` is normalized and handed to the channel adapter.
- Media inputs are **local paths**: `MediaPayloadInput = { path, contentType? }`; `buildMediaPayload()` builds the Slack send fields from a path. `mediaRoot = ~/.openclaw/media`.
- `openclaw message send --media <path>` exists but is the **ops/debug side-channel**, NOT the agent's normal path. Do not build the retrieval tool around it.

**Design consequence:** the retrieval tool is NOT a "send" action — it's a **read that returns a media reply payload**. Given a stored file, it returns `{ text?: "<caption>", mediaUrl: "<abs path under hermes-storage>" }`; the agent's turn carries that; Slack renders it inline. Single turn, no CLI.

⚠️ **BUILD-TIME VERIFY GATE (do first):** the `.d.ts` says tool payloads are normalized, but exercise it end-to-end before trusting zero-glue: register a trivial tool that returns `{mediaUrl: <path to the existing ~/.openclaw/media/hermes-storage-test.txt>}`, call it in a Slack turn, confirm the file arrives inline. If the tool-result field ISN'T auto-normalized, the fallback is the agent echoing the returned path into its own reply payload (still single-turn, just needs one line of agent glue) — SDK refs: docs `/tools/media-overview`, `/plugins/sdk-overview`. Confirm which before wiring the full surface.

## 2. Storage design
- **Root:** `~/.openclaw/media/hermes-storage/` (GC-safe — §4). Files live here so the Slack outbound adapter (allow-listed to `~/.openclaw/media`) can attach them directly; a path outside the allowed roots is refused by openclaw.
- **Layout:** `hermes-storage/files/<id>.<ext>` for blobs + `hermes-storage/index.json` metadata sidecar: `{ id, originalName, contentType, size, createdAt, tags?[], description? }` per file. `id` is a generated stable key (avoids filename collisions; original name preserved in metadata for list/search/display).
- **Why an index:** list/search need metadata without stat-walking; the index is the source of truth for the query tools, the blob dir is the bytes.

## 3. Tool surface (mcp-for-claude — new `storage.tool.ts` + `storage.repository.ts`)
Registered on the SAME `workspace` MCP server (openclaw launches `build/index.js`; after adding tools run `openclaw mcp reload`). Pull-based, on-demand.
1. **`storage-save`** — input: bytes / local path / inline content + optional originalName, contentType, tags, description → writes blob under `files/`, appends to `index.json`, returns `{id, originalName, size}`.
2. **`storage-list`** — optional filter (tag, contentType, recent N) → array of metadata records (no bytes).
3. **`storage-search`** — query over originalName / tags / description → matching metadata records.
4. **`storage-read`** — id → file metadata + content (for the agent to INSPECT the file itself, e.g. read a stored text/doc; not the Slack-delivery path).
5. **`storage-get-to-slack`** (the retrieval-and-deliver tool) — id → returns a reply payload `{ text?: caption, mediaUrl: <abs path> }` (or `mediaUrls` for multiple ids). This is the §1 mechanism; the Slack adapter delivers inline. **Shape depends on the §1 build-time verify** — if tool-result mediaUrl auto-normalizes, return it directly; if not, return the path + a flag the agent echoes into its reply.
6. *(optional, Phase 2)* **`storage-save-from-slack`** — inbound: operator drops a file in Slack → openclaw's `download-file` capability (needs `files:read`) → `storage-save`. Gate behind the `files:read` scope; primary requirement is outbound, so this is opt-in.

**Path-safety:** every tool resolves ids to paths INSIDE `hermes-storage/` and rejects traversal (no `..`, must canonicalize under the root) — the store must never attach or delete a file outside its own dir even though the media root is broader.

## 4. GC-safety (verified — no action needed, just don't break it)
Confirmed live: openclaw has NO TTL/retention/cleanup/sweep of `~/.openclaw/media` (config grep clean; no cron/launchd media-GC job; the only media `unlink`s target temp files + conversation-history trimming, not the media tree). Inbound web media uses a separate `media/inbound/` temp flow. A persistent `hermes-storage/` subdir is safe indefinitely. (Don't name the store `inbound/` or collide with that flow.)

## 5. Prereq A (operator/Peppy) — Slack `files:write` scope
Peppy's confirmed gating fact: the Hermes Slack bot token is MISSING `files:write` → file sends 403 `missing_scope`. **Blocker for the entire delivery path.** Operator adds `files:write` (+ `files:read` if Phase-2 inbound is wanted) to the Hermes Slack app → reinstall → Peppy re-wires the new token. Independent of the MCP build — do it in parallel. Same Slack-console pattern as the earlier `message.im` event-sub fix.

## 6. Prereq B (infra) — backup `hermes-storage/`
Verified: `~/.openclaw/` is currently **NOT backed up** — no cron/rsync touches it (incluir/pocketsoftware watchtowers ignore it), Time Machine has no destination configured, and `openclaw backup` is manual + its scope (config/creds/sessions/workspaces) may **exclude** `media/`. So stored files would be **unprotected**.
**Recommendation:** fold `~/.openclaw/media/hermes-storage/` into the existing watchtower → OCI off-host rsync pattern already used for incluir/pocketsoftware (durable, off-host). Do NOT rely on `openclaw backup` (manual + may skip media) or Time Machine (no destination). Operator/Peppy infra call — flag now so stored files aren't a single-disk-failure from gone.

## 7. Sequencing
- **Track 1 (Peppy, parallel now):** Slack `files:write` scope-add + token re-wire (Prereq A). Gates delivery.
- **Track 2 (build):** (0) §1 build-time verify gate → (1) `storage.repository.ts` + `storage.tool.ts` (save/list/search/read/get-to-slack) → register + `openclaw mcp reload` → (2) end-to-end test: save a file, `storage-get-to-slack`, confirm inline Slack delivery (needs Track 1 done).
- **Track 3 (infra):** backup fold-in (Prereq B) — before the store holds anything the operator cares about losing.
- Phase 2 (optional): inbound `storage-save-from-slack` once `files:read` is granted.

## Open items for operator
1. Approve `files:write` (+ `files:read` for inbound file-drops?) on the Hermes Slack app + reinstall.
2. Approve folding `hermes-storage/` into the off-host backup (recommended — else stored files are unprotected).

## Integration facts (for the builder)
- openclaw → MCP wiring: `~/.openclaw/openclaw.json` `mcp.servers.workspace` launches `…/mcp-for-claude/build/index.js` (env `…/.env`). New tools register in `build/index.js`; `openclaw mcp reload` picks them up next turn.
- Reply-payload contract: `node_modules/openclaw/dist/reply-payload-*.d.ts` (`mediaUrl`/`mediaUrls`, `normalizeOutboundReplyPayload`, `MediaPayloadInput.path`).
- Media root (allow-listed for outbound): `~/.openclaw/media` — store MUST live under it.
