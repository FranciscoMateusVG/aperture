# Context Diet v1 — indexed lazy memory + constitutional core (GLaDOS first, fleet-wide by construction)

**Bead:** aperture-trgpo (v3.2 scope). **Author:** Wheatley. **Status:** design, pending GLaDOS review + operator yes.

## The numbers (measured 2026-09-06)

GLaDOS boot on the Codex path (`/tmp/aperture-codex-glados/prompt.md`) = **535 KB**: prompt 15 KB + 10 resident skills 146 KB + `bd prime` memory bank **382 KB** (200 entries, median 1.7 KB). The Claude path gets the same 382 KB at `SessionStart` **and again at `PreCompact`**. The bank is 2.4× the whole system prompt; one family (`hermes-lane-2-*`, 41 entries, all 2026-07-31) is 31% of it, and entries are stored as `key`+`body` only — **no timestamps, no metadata** (Dolt KV row `kv.memory.<key>`; dates only via `dolt_history_config`); 0 of 200 have ever been forgotten; the orchestration quartet (89 KB) is 48% rules, 25% narrative, with 15–17 KB duplicated across files and one contradiction (who presses Ctrl-C on a hung specialist). Boot inbox: 0 today; only matters after outages (already capped at 200).

**Grep-receipt (existing infra):** mempalace has a real hybrid ranker (local MiniLM + BM25), but its corpus is Peppy's credential drawers + dead diaries, it isn't wired for Codex, and Cipher's `credential-drawer-plaintext-read-ban` applies. `bd memories <kw>` is single-term substring, no ranking/tags/limit. `bd prime` has no flag to omit the bank (only `--mcp`, 41 KB, or a `.beads/PRIME.md` override). Codex's native `memories` feature is off and per-CODEX_HOME. **Conclusion:** reuse the *pattern* (name+description resident, body on demand — the Codex skills catalog), not the palace; 200 entries need BM25, not embeddings.

## Design

### 1. Constitutional core (resident ≤ 45 KB, from 161 KB)
- New skill `orchestrator-core` = the quartet + `subagents` **rules only**, deduped (13 overlap clusters identified), contradiction resolved toward `agent-liveness` (real hang → surface to operator; API drop → BEADS resend). Procedures → `references/procedures.md`; narratives → `references/precedents.md`; the four originals stay invocable lazily (Claude native discovery / Codex catalog).
- `prompts/glados.md`: drop the ~5.5 KB that restates skills; fix "always create BEADS tasks" → the §0 operator-ack gate.
- `agents/glados/resident.txt` (Codex path) and trimmed `skills.txt` (Claude path) = `beads`, `communicate`, `team`, `orchestrator-core`.

### 2. Memory index (~25 KB resident instead of 382 KB)
Metadata rides **in-content conventions** (schema is key+body), backfilled once and lint-enforced:
```
[meta] project=aperture tags=comms,hub entities=ws-hub,hub-client supersedes=old-key standing=true
```
`mcp-server/src/memory-index.ts` reads `bd memories --json` → one line per live entry: `key · project · tags · 12-word gist · age`. Age = `[meta] updated=` if present, else a date in the key, else first-seen from `dolt_history_config` (queried once per index build, cached in `~/.aperture/run/memory-index.json`). Excluded from the index: superseded keys, secret-bearing content (regex: tokens, `BEGIN … KEY`, `password=`, drawer paths), and `tags=secret`.

### 3. Retrieval
- **Explicit:** MCP tools `recall(query, k=5, project?, tags?)` → ranked gists + keys (BM25 over key+content, standing entries boosted, >90-day entries demoted, superseded hidden); `recall_full(key)` → full body. Both reject secret-tagged entries.
- **Automatic (Claude path, v1):** `UserPromptSubmit` hook already exists for busy/idle; extend the same script to print top-3 gists for the prompt text + active bead id (hook stdout is injected as context). Codex path v1: explicit only; bridge auto-inject is a v2 item.
- **Full bank:** never injected at boot; `recall_full` per key, `bd memories --json` for audits.

### 4. Injection seams (one script, two paths)
`scripts/aperture-prime.sh` = `bd prime --mcp` + memory index. Replaces bare `bd prime` in `.claude/settings.json` (`SessionStart`, `PreCompact`) and in `agents.rs::inject_bd_memory` for Codex. No other injection changes.

### 5. Staleness & secrets
`supersedes=` hides the old key from index and recall (fetchable with `include_superseded=true`); `standing=true` pins constitutional decisions; age demotes, never deletes; secret regex + tag exclusion tested with seeded fixtures, exclusion list reviewed by Cipher.

### 6. Gates (all scripted, run in CI + `just context-budget`)
| gate | target |
|---|---|
| fresh-session boot bytes (Codex prompt.md; Claude hook output) | ≤ 120 KB (from 535 KB); PreCompact re-injection ≤ 30 KB |
| recall@5 on a 30-query golden set mined from recent bead history | ≥ 0.90 |
| standing-rule retention | 25 named constitutional rules present verbatim in the resident core (grep test) |
| secret exclusion | seeded fake-secret memory never appears in index or recall |
| recall latency | < 200 ms |

## Files
`mcp-server/src/memory-index.ts` (+test), `index.ts` (tools), `presence-hint.ts` (auto-recall branch), `scripts/aperture-prime.sh`, `.claude/settings.json`, `src-tauri/src/agents.rs::inject_bd_memory`, `.claude/skills/orchestrator-core/`, `agents/glados/{skills,resident}.txt`, `prompts/glados.md`, `scripts/context-budget.sh` + `justfile`.

## Migration (each step measurable, reversible)
1. Ship index + `recall` tools **additively**; nothing injected changes. Measure recall on the golden set.
2. Backfill `[meta]` on the 200 memories (subagent, mechanical) + lint; consolidate the 41 `hermes-lane-2-*` entries into ≤3 (`supersedes=` the rest) and `bd forget` the junk `list` entry — the bank's first real pruning.
3. Flip the two injection seams to `aperture-prime.sh`; measure boot bytes; PreCompact drops to index-only.
4. `orchestrator-core` merge + GLaDOS resident split; retention gate.
5. Fleet-wide by construction: the same two seams served every agent; per-agent resident sets unchanged.

**Out of scope (v4):** embeddings/mempalace as a transcript memory, Codex auto-inject, cross-agent memory sharing policies.
