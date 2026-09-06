# Context Diet v1 — indexed lazy memory + constitutional core (GLaDOS first, fleet-wide by construction)

**Bead:** aperture-trgpo (v3.2 scope). **Author:** Wheatley. **Status:** v1.1 — revised per GLaDOS review (6 corrections); pending GLaDOS re-review + operator yes. Units: KiB throughout; tokens ≈ bytes ÷ 4.

## The numbers (measured 2026-09-06)

GLaDOS boot on the Codex path (`/tmp/aperture-codex-glados/prompt.md`) = **535.5 KiB (≈137k tokens)**: prompt 14.5 KiB + 10 resident skills 143.3 KiB + `bd prime` **377.8 KiB** (workflow preamble 4.6 KiB + 200 memories 373.2 KiB, median 1.6 KiB). The Claude path gets the same 377.8 KiB at `SessionStart` **and again at every `PreCompact`**. Memories are `key`+`body` only — no timestamps, no metadata (Dolt KV row `kv.memory.<key>`; dates only via `dolt_history_config`); `hermes-lane-2-*` (41 entries, 2026-07-31) is 31% of the bank; 0 of 200 ever forgotten. The orchestration quartet (86.8 KiB) is 48% rules, 25% narrative, 15–17 KiB duplicated, with one contradiction (who presses Ctrl-C on a hung specialist). Boot inbox: 0 today; matters only after outages (capped at 200).

**Grep-receipt:** mempalace's hybrid ranker exists (MiniLM + BM25) but its corpus is credential drawers + dead diaries, it isn't wired for Codex, and Cipher's `credential-drawer-plaintext-read-ban` applies. `bd memories <kw>` is unranked substring. `bd prime` cannot omit the bank (`--mcp` = 40.3 KiB, or a `.beads/PRIME.md` override). Codex's native `memories` feature is off, per-CODEX_HOME. **Conclusion:** reuse the *pattern* (name+description resident, body on demand), not the palace; 200 entries need BM25, not embeddings.

## Design

### 1. Constitutional core (resident ≤ 45 KiB, from 157.8 KiB)
- New skill `orchestrator-core` = quartet + `subagents` **rules only**, deduped (13 overlap clusters). Procedures → `references/procedures.md`; narratives → `references/precedents.md`; the originals stay lazily invocable.
- **Named decisions, explicit overrides (GLaDOS #4).** Each reconciled rule is written as `DECISION-n: <rule> — supersedes <skill §>, source <bead/date>`. Where an operator instruction post-dates a skill (stall interruption; /compact at 60%), **the operator instruction wins and the older skill text is marked superseded in place** — never silently dropped. A `DECISIONS.md` table in the skill lists every override; GLaDOS reviews it before merge.
- `prompts/glados.md`: drop the ~5.4 KiB restating skills; fix "always create BEADS tasks" → the §0 operator-ack gate.
- `agents/glados/resident.txt` (Codex) and trimmed `skills.txt` (Claude) = `beads`, `communicate`, `team`, `orchestrator-core`.

### 2. Memory index — non-destructive (GLaDOS #5)
The bank is **never rewritten or forgotten by tooling**. Metadata lives in a **sidecar** `~/.aperture/memory-meta.json` (repo-tracked seed at `docs/memory-meta.seed.json`), keyed by memory key:
```json
"hub-self-heal-pr-46": {"project":"aperture","tags":["comms","hub"],"entities":["ws-hub"],"standing":false,"supersedes":[],"updated":"2026-07-19"}
```
Backfill = a reviewed PR editing the sidecar only. Hermes consolidation = **new** summary entries (`bd remember`, reviewed text) whose sidecar `supersedes` lists the 41 originals; originals untouched; retrieval tests assert the summaries win and the originals stay fetchable with `include_superseded`. The junk `list` entry is hidden via sidecar, not forgotten.

`mcp-server/src/memory-index.ts` builds one line per live entry: `key · project · tags · ≤12-word gist · age`. Age = sidecar `updated`, else a date in the key, else Dolt first-seen (one query per cold build).

### 3. Secret exclusion — one function, every surface (GLaDOS #1)
`redact()` = regex set (API keys/tokens, `BEGIN … KEY`, `password=`/`secret=`, drawer paths, base64 ≥ 32) **plus** sidecar `tags:secret`. Applied identically to: index lines, gists (a gist that would contain a redacted span is replaced by `[redacted gist]`), `recall` results, `recall_full` bodies, and the cache file at write time (the cache never holds unredacted text). Untagged legacy entries are covered by the regex path. **No surface ever emits raw `bd memories --json`**; audits get `recall_stats` → sanitised counts + keys. Seeded fake-secret fixtures test all five surfaces. Cipher reviews the regex set.

### 4. Retrieval schemas (GLaDOS #6)
- `recall({query, k≤10 (default 5), offset=0, project?, tags?, include_superseded=false})` → `{items:[{key, gist, score, age_days, tags, superseded_by?}], total, next_offset|null, index_built_at}`. BM25 over key+body; `standing` +boost, >90-day −demotion, superseded hidden unless asked.
- `recall_full({key, max_bytes≤8192})` → `{key, body (redacted, truncated with notice), bytes_total, tags, supersedes, superseded_by}`.
- `recall_stats()` → counts by project/tag, superseded count, redacted count, cache age. No bodies.
- **Cache:** `~/.aperture/run/memory-index.json` keyed by sha256(`bd memories --json` + sidecar). Every call re-hashes (~20 ms); mismatch → rebuild. Sidecar edit or new memory therefore invalidates immediately. Targets: cold build < 1.5 s, warm `recall` < 50 ms, `recall_full` < 20 ms.
- **Golden set (30):** ≥ 8 exact identifiers (bead ids, PR numbers, hostnames, env-var names), ≥ 6 retraction/old-vs-current conflicts (expected answer is the *current* key; the superseded one must not rank), ≥ 4 paraphrases with no lexical overlap, remainder lexical. recall@5 ≥ 0.90; conflicts pass at 100%.

### 5. Injection seams — two explicit modes, no full-bank fallback (GLaDOS #2)
`scripts/aperture-prime.sh <boot|precompact>`:
- **boot** = `bd prime` **with the `## Persistent Memories` section stripped** (4.6 KiB, not `--mcp`'s 40.3 KiB) + **standing decisions inlined in full** (sidecar `standing:true`, cap 8 KiB) + index (~25 KiB) → **≤ 40 KiB**.
- **precompact** = standing decisions + index → **≤ 30 KiB**.
- On any failure (bd down, index build error): emit the preamble + one line `[memory index unavailable: <reason> — use recall/recall_full]`. **Never the full bank.**
Replaces bare `bd prime` in `.claude/settings.json` (`SessionStart` → boot, `PreCompact` → precompact) and in `agents.rs::inject_bd_memory` (boot). Standing rules are therefore **resident** (§5) and also boosted (§4) — not one or the other.

### 6. Automatic recall — separate from presence (GLaDOS #3)
A **separate** hook script `dist/memory-recall.js` on `UserPromptSubmit` prints top-3 gists for the prompt text + active bead id (hook stdout becomes context). `presence-hint.js` is untouched; the two run as independent hook entries, so neither's failure or latency affects the other. Recall failure prints `[recall unavailable: <reason>]` rather than silence, so a missing hint is visible, never mistaken for "nothing relevant". Codex v1: explicit tools only.

### 7. Gates (`just context-budget`, CI)
| gate | target |
|---|---|
| assembled boot prompt: Codex `prompt.md` bytes; Claude = system prompt + boot hook output | ≤ 120 KiB (from 535.5) + token estimate printed |
| PreCompact re-injection | ≤ 30 KiB |
| recall@5 on the golden set / conflict subset | ≥ 0.90 / 100% |
| standing-rule retention | 25 named DECISION rules present verbatim in resident output (grep test) |
| secret exclusion | seeded fixtures absent from all 5 surfaces + cache |
| latency | cold < 1.5 s, warm recall < 50 ms |
| non-destruction | `bd memories --json` hash unchanged by any tooling step |

## Files
`mcp-server/src/{memory-index,memory-recall}.ts` (+tests), `index.ts` (3 tools), `scripts/aperture-prime.sh`, `scripts/context-budget.sh` + `justfile`, `.claude/settings.json`, `src-tauri/src/agents.rs::inject_bd_memory`, `.claude/skills/orchestrator-core/{SKILL.md,DECISIONS.md,references/}`, `agents/glados/{skills,resident}.txt`, `prompts/glados.md`, `docs/memory-meta.seed.json`, `test/fixtures/memory-golden.json`.

## Migration (each step measured, reversible, additive until step 3)
1. Index + `recall`/`recall_full`/`recall_stats` shipped **additively**; nothing injected changes. Golden set + secret fixtures green.
2. Sidecar backfill PR (reviewed); hermes summaries as new entries; non-destruction gate.
3. Flip the two seams to `aperture-prime.sh boot|precompact`; measure; rollback = one-line hook revert.
4. `orchestrator-core` + `DECISIONS.md` (GLaDOS review) + resident split; retention gate.
5. Fleet-wide by construction (same two seams); per-agent resident sets unchanged.

**Out of scope (v4):** embeddings / mempalace as transcript memory, Codex auto-inject, cross-agent memory policy.
