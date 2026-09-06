# Context Diet v1 — indexed lazy memory + constitutional core (GLaDOS first, fleet-wide by construction)

**Bead:** aperture-trgpo (v3.2 scope). **Author:** Wheatley. **Status:** v1.2 — v1.1 + QA-remediation corrections (aperture-3kavd HOLD #1–#5: credential procedure, bead-id matcher, Codex token-path assembly, honest lexical-v1 retrieval scope with separate @3/@5 gating). Operator-approved lexical v1 2026-09-06. Units: KiB throughout; tokens ≈ bytes ÷ 4.

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
- **Retrieval is lexical in v1** (BM25 over key+body + exact-id rank). Queries with **zero shared content tokens** are **unsupported** — the gate reports them, never counts them as passed. Semantic/embedding retrieval is v4 scope (operator decision 2026-09-06: keyword/ID-based v1, "ok so go on").
- **Golden set (34 = 30 gated + 4 reported-only):** 10 exact identifiers (bead ids, PR numbers, hostnames, env-var names), 9 retraction/old-vs-current conflicts (expected answer is the *current* key; the superseded one must not rank), 6 lexical, 5 `paraphrase-lexical` (low-overlap paraphrases; measured 5–7 shared tokens with the target, mostly stopwords + 1–2 content words — **not** zero-overlap, and labelled so in the fixture), and 4 `zero-overlap` (measured 0 shared content tokens, len ≥ 3, stopwords removed) that are **reported as unsupported and not gated**.
- **Two truths, gated separately:** the explicit `recall` tool (k=5) and the automatic hook (k=3, what the user actually sees). Thresholds: recall@3 ≥ 0.90 and recall@5 ≥ 0.90 over gated kinds; conflicts 100% at both k. The identifier and conflict subsets are additionally run through the **real hook binary** (`dist/memory-recall.js`, top-3): identifier ≥ 0.90, conflicts 100%.
- **Measured at 87c2d87+ (2026-09-06, real bank):** recall@5 = 30/30 (1.000); recall@3 = 29/30 (0.967 — one `paraphrase-lexical` query ranks 4th); hook truth: identifier 10/10, conflict 9/9; zero-overlap 0/4 (unsupported, as expected). 29/30 at k=3 is the honest number, not "perfect".

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
| recall@3 (hook truth) and recall@5 (tool truth) on gated golden kinds / conflict subset at both k | ≥ 0.90 and ≥ 0.90 / 100% (measured: 0.967 / 1.000 / 100%) |
| hook truth — real `dist/memory-recall.js` top-3 on identifier / conflict subsets | ≥ 0.90 / 100% (measured 10/10, 9/9) |
| zero-overlap queries (semantic) | **unsupported in v1** — reported by the gate, never gated (measured 0/4) |
| standing-rule retention — named obligation coverage (see §7a) | (a) all **13 enumerated DECISION rows** (DECISION-1, DECISION-1b, DECISION-2, DECISION-3, DECISION-4, DECISION-5, DECISION-6, DECISION-7, DECISION-8, DECISION-9, DECISION-10, DECISION-11, DECISION-12) present verbatim in resident `orchestrator-core/SKILL.md §0`, checked by id; (b) all **13 designated standing memory statements** (sidecar `standing:true`, listed by key in the gate output) present in both `boot` and `precompact` renders. Two disjoint sets; the gate prints every id/key it verified, never a bare count. Measured: 13/13 and 13/13 boot + 13/13 precompact |
| secret exclusion | seeded fixtures absent from all 5 surfaces + cache |
| latency | cold < 1.5 s, warm recall < 50 ms |
| non-destruction | `bd memories --json` hash unchanged by any tooling step |

### 7a. Retention obligations — reconciliation of "25" (aperture-3kavd catch, 2026-09-06)

**What the 25 was.** v1 of this spec (`feaf0c2`) wrote "25 named constitutional rules" in §7 *before* `DECISIONS.md` existed, while the same v1 §1 already sized the reconciled rule set at **13 overlap clusters**. No commit in the branch history enumerates more than 13 decisions (there is no `DECISION-13` or higher anywhere). The 25 was an **unenumerated draft estimate** carried into v1.1, not a list anything was measured against — so nothing that was ever enumerated has been dropped. This section replaces the estimate with the enumerated set.

**The enumerated set = the 13 DECISION rows** (`DECISION-1, 1b, 2 … 12` in `orchestrator-core/DECISIONS.md`), GLaDOS-reviewed before merge per §1. Coverage of the source skills whose rules were reconciled (derived from each row's *Supersedes* / *Source* columns):

| source skill / prompt | reconciled by |
|---|---|
| `cost-proportional-orchestration` | DECISION-1, DECISION-11, DECISION-12 |
| `agent-liveness` | DECISION-1, DECISION-1b, DECISION-2, DECISION-4, DECISION-5, DECISION-6, DECISION-8, DECISION-9 |
| `watch-protocol` | DECISION-1, DECISION-1b, DECISION-2, DECISION-3, DECISION-4, DECISION-5, DECISION-6, DECISION-7, DECISION-8, DECISION-9, DECISION-10 |
| `glados-loop` | DECISION-2, DECISION-8, DECISION-10, DECISION-12 |
| `specialist-delegation` | DECISION-2 |
| `prompts/glados.md` | DECISION-3, DECISION-4, DECISION-6, DECISION-7, DECISION-11 |
| `beads` | DECISION-3, DECISION-12 |
| `communicate` | DECISION-6 |

Rows whose *Supersedes* column says "None"/"Nothing superseded" (DECISION-5, DECISION-9) are harmonisations that pin an agreed reading; they are obligations too and are gated identically. The other resident core clauses (procedures in `references/procedures.md`, precedents in `references/precedents.md`) are **not** binding decisions and are not gated.

**Separate set: the 13 standing memory statements** (sidecar `standing:true`, with reviewed `standing_text`). These are bank memories rendered resident by `aperture-prime.sh` in both modes; they are enumerated by key in the gate output and are unrelated to the DECISION rows (no overlap in either direction).

**Gate.** `just retention-gate` prints each DECISION id it verified and each standing key it found in both renders, and additionally asserts that the DECISION ids in `DECISIONS.md` equal the ids named in this spec (so spec and code cannot drift silently again). Substantive acceptance was not lowered: the obligations that exist are all still required verbatim; the only change is that the requirement now names them.

## Files
`mcp-server/src/{memory-index,memory-recall}.ts` (+tests), `index.ts` (3 tools), `scripts/aperture-prime.sh`, `scripts/context-budget.sh` + `justfile`, `.claude/settings.json`, `src-tauri/src/agents.rs::inject_bd_memory`, `.claude/skills/orchestrator-core/{SKILL.md,DECISIONS.md,references/}`, `agents/glados/{skills,resident}.txt`, `prompts/glados.md`, `docs/memory-meta.seed.json`, `test/fixtures/memory-golden.json`.

## Migration (each step measured, reversible, additive until step 3)
1. Index + `recall`/`recall_full`/`recall_stats` shipped **additively**; nothing injected changes. Golden set + secret fixtures green.
2. Sidecar backfill PR (reviewed); hermes summaries as new entries; non-destruction gate.
3. Flip the two seams to `aperture-prime.sh boot|precompact`; measure; rollback = one-line hook revert.
4. `orchestrator-core` + `DECISIONS.md` (GLaDOS review) + resident split; retention gate.
5. Fleet-wide by construction (same two seams); per-agent resident sets unchanged.

**Out of scope (v4):** embeddings / mempalace as transcript memory, Codex auto-inject, cross-agent memory policy.
