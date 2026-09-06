#!/usr/bin/env node
/**
 * recall-gate — the §7 retrieval gate for the context diet (aperture-trgpo / aperture-3kavd HOLD #4/#5).
 *
 * Runs the golden query set (mcp-server/test/fixtures/memory-golden.json) against the REAL memory bank
 * through the real index (dist/memory-index.js) and reports TWO truths separately:
 *   - recall@5  — what the explicit `recall` tool returns (default k=5)
 *   - recall@3  — what the automatic UserPromptSubmit hook actually shows (TOP_K=3)
 * and, for the exact-identifier + conflict subsets, ALSO drives the real hook binary
 * (dist/memory-recall.js) so "the hook surfaces the right memory" is measured on the hook, not inferred.
 *
 * Gated kinds: identifier, conflict, lexical, paraphrase-lexical.
 *   recall@3 ≥ 0.90 AND recall@5 ≥ 0.90 over gated kinds; conflict subset 100% at both k (current key ranks,
 *   retracted key does not); hook-truth: identifier ≥ 0.90, conflict 100%; warm recall median < 50 ms.
 * Reported, NEVER gated: kind "zero-overlap" — v1 retrieval is lexical (BM25 + exact ids); queries with zero
 *   shared content tokens are UNSUPPORTED and are printed as such so the limitation stays visible.
 *
 *   just recall-gate            table + exit 1 on breach
 *   just recall-gate --json     machine-readable
 */
import { readFileSync, existsSync, mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve, dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const distDir = process.env.APERTURE_MCP_DIST ?? resolve(repo, "mcp-server", "dist");
const goldenPath = process.env.APERTURE_GOLDEN ?? resolve(repo, "mcp-server", "test", "fixtures", "memory-golden.json");
const json = process.argv.includes("--json");
const KS = [3, 5];
const HOOK_K = 3;
const MIN_RECALL = 0.9;
const MAX_WARM_MS = 50;
const GATED = new Set(["identifier", "conflict", "lexical", "paraphrase-lexical"]);
const HOOK_KINDS = new Set(["identifier", "conflict"]);
const HOOK_MIN_PROMPT_CHARS = 12; // the hook ignores shorter prompts by design

const fail = (msg) => { console.error(`recall-gate: ${msg}`); process.exit(2); };
if (!existsSync(resolve(distDir, "memory-index.js"))) fail("dist/memory-index.js missing — run just build-mcp");
if (!existsSync(resolve(distDir, "memory-recall.js"))) fail("dist/memory-recall.js missing — run just build-mcp");
if (!existsSync(goldenPath)) fail(`golden set missing: ${goldenPath}`);

const mod = await import(resolve(distDir, "memory-index.js"));
const golden = JSON.parse(readFileSync(goldenPath, "utf8"));
if (!Array.isArray(golden) || golden.length === 0) fail("golden set is empty");
for (const g of golden) if (g.kind === "paraphrase") fail(`golden entry still labelled "paraphrase" (relabel paraphrase-lexical or zero-overlap): ${g.query}`);

const t0 = performance.now();
const index = await mod.buildIndex();
const buildMs = performance.now() - t0;

// ── index truth at k=3 and k=5 ──
const rows = [];
const timings = [];
for (const g of golden) {
  const kind = g.kind ?? "lexical";
  const row = { kind, query: g.query, expect: g.expect_key, gated: GATED.has(kind), at: {} };
  for (const K of KS) {
    const t1 = performance.now();
    const r = mod.recall(index, { query: g.query, k: K, include_superseded: false });
    if (K === 5) timings.push(performance.now() - t1);
    const keys = r.items.map((i) => i.key);
    row.at[K] = { hit: keys.includes(g.expect_key), leaked: g.must_not_rank ? keys.includes(g.must_not_rank) : false, rank: keys.indexOf(g.expect_key) + 1 || null, top: keys[0] ?? null };
  }
  rows.push(row);
}
const gatedRows = rows.filter((r) => r.gated);
const recallAt = (K, rs) => (rs.length ? rs.filter((r) => r.at[K].hit).length / rs.length : 1);
const conflictPassAt = (K) => { const c = gatedRows.filter((r) => r.kind === "conflict"); return c.length ? c.filter((r) => r.at[K].hit && !r.at[K].leaked).length / c.length : 1; };
const byKind = {};
for (const r of rows) {
  const b = (byKind[r.kind] ??= { n: 0, gated: r.gated, hit3: 0, hit5: 0, leaked3: 0, leaked5: 0 });
  b.n++; if (r.at[3].hit) b.hit3++; if (r.at[5].hit) b.hit5++; if (r.at[3].leaked) b.leaked3++; if (r.at[5].leaked) b.leaked5++;
}
timings.sort((a, b) => a - b);
const medianMs = timings[Math.floor(timings.length / 2)] ?? 0;

// ── hook truth: drive the real UserPromptSubmit hook for identifier + conflict queries ──
const tmp = mkdtempSync(join(tmpdir(), "recall-gate-hook-"));
const tokenFile = join(tmp, "gate.token");
writeFileSync(tokenFile, "recall-gate-dummy-token-not-a-secret", { mode: 0o600 });
const hookRows = [];
for (const g of golden.filter((g) => HOOK_KINDS.has(g.kind))) {
  if (g.query.length < HOOK_MIN_PROMPT_CHARS) { hookRows.push({ kind: g.kind, query: g.query, expect: g.expect_key, skipped: "prompt shorter than the hook minimum" }); continue; }
  const payload = JSON.stringify({ hook_event_name: "UserPromptSubmit", prompt: g.query });
  const r = spawnSync(process.execPath, [resolve(distDir, "memory-recall.js")], {
    input: payload, encoding: "utf8", timeout: 10000,
    env: { ...process.env, APERTURE_HUB_TOKEN_FILE: tokenFile, APERTURE_ACTIVE_BEAD: "" },
  });
  const keys = (r.stdout || "").split("\n").filter((l) => l.startsWith("- ")).map((l) => l.slice(2).split(" · ")[0]).slice(0, HOOK_K);
  hookRows.push({ kind: g.kind, query: g.query, expect: g.expect_key, hit: keys.includes(g.expect_key), leaked: g.must_not_rank ? keys.includes(g.must_not_rank) : false, keys, exit: r.status });
}
rmSync(tmp, { recursive: true, force: true });
const hookIdent = hookRows.filter((r) => r.kind === "identifier" && !r.skipped);
const hookConf = hookRows.filter((r) => r.kind === "conflict" && !r.skipped);
const hookIdentRecall = hookIdent.length ? hookIdent.filter((r) => r.hit).length / hookIdent.length : 1;
const hookConfPass = hookConf.length ? hookConf.filter((r) => r.hit && !r.leaked).length / hookConf.length : 1;
const hookSkipped = hookRows.filter((r) => r.skipped).length;

// ── unsupported (reported only) ──
const zero = rows.filter((r) => r.kind === "zero-overlap");
const zeroHit5 = zero.filter((r) => r.at[5].hit).length;

// ── verdict ──
const breaches = [];
for (const K of KS) if (recallAt(K, gatedRows) < MIN_RECALL) breaches.push(`recall@${K} ${recallAt(K, gatedRows).toFixed(3)} < ${MIN_RECALL} (gated kinds)`);
for (const K of KS) if (conflictPassAt(K) < 1) breaches.push(`conflict subset @${K} ${(conflictPassAt(K) * 100).toFixed(0)}% < 100%`);
if (hookIdentRecall < MIN_RECALL) breaches.push(`hook identifier recall@${HOOK_K} ${hookIdentRecall.toFixed(3)} < ${MIN_RECALL}`);
if (hookConfPass < 1) breaches.push(`hook conflict subset @${HOOK_K} ${(hookConfPass * 100).toFixed(0)}% < 100%`);
if (medianMs > MAX_WARM_MS) breaches.push(`warm recall median ${medianMs.toFixed(1)} ms > ${MAX_WARM_MS}`);

if (json) {
  console.log(JSON.stringify({ ks: KS, hook_k: HOOK_K, gated_kinds: [...GATED], recall: { at3: recallAt(3, gatedRows), at5: recallAt(5, gatedRows) }, conflictPass: { at3: conflictPassAt(3), at5: conflictPassAt(5) }, hook: { identifier_recall: hookIdentRecall, identifier_n: hookIdent.length, conflict_pass: hookConfPass, conflict_n: hookConf.length, skipped: hookSkipped, rows: hookRows }, unsupported: { kind: "zero-overlap", n: zero.length, hit5: zeroHit5, rows: zero }, medianMs, buildMs, byKind, rows, breaches, index_built_at: index.builtAt }, null, 1));
} else {
  console.log(`recall-gate — ${golden.length} golden queries (${gatedRows.length} gated, ${zero.length} reported-only), index built ${index.built_at ?? index.builtAt} (${buildMs.toFixed(0)} ms), warm median ${medianMs.toFixed(1)} ms`);
  console.log(`\nkind                 n  gated  recall@3  recall@5  leaked@3  leaked@5`);
  for (const [k, b] of Object.entries(byKind)) console.log(`${k.padEnd(19)}${String(b.n).padStart(3)}  ${b.gated ? "yes  " : "no   "}  ${(b.hit3 / b.n).toFixed(3).padStart(8)}  ${(b.hit5 / b.n).toFixed(3).padStart(8)}  ${String(b.leaked3).padStart(8)}  ${String(b.leaked5).padStart(8)}`);
  console.log(`${"gated overall".padEnd(19)}${String(gatedRows.length).padStart(3)}  yes    ${recallAt(3, gatedRows).toFixed(3).padStart(8)}  ${recallAt(5, gatedRows).toFixed(3).padStart(8)}   (thresholds: @3 ≥ ${MIN_RECALL}, @5 ≥ ${MIN_RECALL}, conflicts 100% at both)`);
  console.log(`\nhook truth (real dist/memory-recall.js, top-${HOOK_K}): identifier ${hookIdent.filter((r) => r.hit).length}/${hookIdent.length} = ${hookIdentRecall.toFixed(3)}; conflict ${hookConf.filter((r) => r.hit && !r.leaked).length}/${hookConf.length} = ${(hookConfPass * 100).toFixed(0)}%${hookSkipped ? `; skipped ${hookSkipped} (prompt < ${HOOK_MIN_PROMPT_CHARS} chars, hook ignores by design)` : ""}`);
  for (const r of hookRows.filter((r) => !r.skipped && (!r.hit || r.leaked))) console.log(`  HOOK MISS [${r.kind}] "${r.query}" → expected ${r.expect}; hook showed ${r.keys.join(", ") || "(nothing)"}${r.leaked ? " (RETRACTED KEY SHOWN)" : ""}`);
  console.log(`\nUNSUPPORTED in v1 lexical retrieval (reported, not gated): zero-overlap ${zeroHit5}/${zero.length} hit @5`);
  for (const r of zero) console.log(`  [zero-overlap] "${r.query}" → ${r.expect}: ${r.at[5].hit ? "hit (rank " + r.at[5].rank + ")" : "miss"}`);
  const misses = gatedRows.filter((r) => !r.at[3].hit || !r.at[5].hit || r.at[3].leaked || r.at[5].leaked);
  if (misses.length) {
    console.log(`\ngated misses (${misses.length}):`);
    for (const m of misses) console.log(`  [${m.kind}] "${m.query}" → expected ${m.expect}; @3 ${m.at[3].hit ? "hit" : "MISS"} (rank ${m.at[3].rank ?? ">3"}), @5 ${m.at[5].hit ? "hit" : "MISS"} (rank ${m.at[5].rank ?? ">5"})${m.at[3].leaked || m.at[5].leaked ? " (RETRACTED KEY LEAKED)" : ""}`);
  }
  console.log(`\nRESULT: ${breaches.length ? "FAIL — " + breaches.join("; ") : "PASS"}`);
}
process.exit(breaches.length ? 1 : 0);
