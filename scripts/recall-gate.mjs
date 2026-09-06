#!/usr/bin/env node
/**
 * recall-gate — the §7 retrieval gate for the context diet (aperture-trgpo).
 *
 * Runs the golden query set (mcp-server/test/fixtures/memory-golden.json) against the REAL
 * memory bank through the real index (dist/memory-index.js), using the sidecar seed unless a
 * per-machine sidecar exists, and reports recall@5 overall and per kind. Conflict queries must
 * rank the CURRENT key and must NOT rank the retracted one — those pass at 100% or the gate fails.
 *
 *   just recall-gate            table + exit 1 on breach
 *   just recall-gate --json     machine-readable
 *
 * Thresholds: recall@5 ≥ 0.90 overall; conflict subset 100%; warm recall < 50 ms median.
 */
import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const distDir = process.env.APERTURE_MCP_DIST ?? resolve(repo, "mcp-server", "dist");
const goldenPath = process.env.APERTURE_GOLDEN ?? resolve(repo, "mcp-server", "test", "fixtures", "memory-golden.json");
const json = process.argv.includes("--json");
const K = 5;
const MIN_RECALL = 0.9;
const MAX_WARM_MS = 50;

const fail = (msg) => { console.error(`recall-gate: ${msg}`); process.exit(2); };
if (!existsSync(resolve(distDir, "memory-index.js"))) fail("dist/memory-index.js missing — run just build-mcp");
if (!existsSync(goldenPath)) fail(`golden set missing: ${goldenPath}`);

const mod = await import(resolve(distDir, "memory-index.js"));
const golden = JSON.parse(readFileSync(goldenPath, "utf8"));
if (!Array.isArray(golden) || golden.length === 0) fail("golden set is empty");

const t0 = performance.now();
const index = await mod.buildIndex();
const buildMs = performance.now() - t0;

const rows = [];
const timings = [];
for (const g of golden) {
  const q = { query: g.query, k: K, include_superseded: false };
  const t1 = performance.now();
  const r = mod.recall(index, q);
  timings.push(performance.now() - t1);
  const keys = r.items.map((i) => i.key);
  const hit = keys.includes(g.expect_key);
  const leaked = g.must_not_rank ? keys.includes(g.must_not_rank) : false;
  rows.push({ kind: g.kind ?? "lexical", query: g.query, expect: g.expect_key, hit, leaked, rank: hit ? keys.indexOf(g.expect_key) + 1 : null, top: keys[0] ?? null });
}

const byKind = {};
for (const r of rows) {
  const b = (byKind[r.kind] ??= { n: 0, hit: 0, leaked: 0 });
  b.n++; if (r.hit) b.hit++; if (r.leaked) b.leaked++;
}
const overall = rows.filter((r) => r.hit).length / rows.length;
const conflicts = rows.filter((r) => r.kind === "conflict");
const conflictPass = conflicts.length === 0 ? 1 : conflicts.filter((r) => r.hit && !r.leaked).length / conflicts.length;
timings.sort((a, b) => a - b);
const medianMs = timings[Math.floor(timings.length / 2)] ?? 0;

const breaches = [];
if (overall < MIN_RECALL) breaches.push(`recall@${K} ${overall.toFixed(3)} < ${MIN_RECALL}`);
if (conflictPass < 1) breaches.push(`conflict subset ${(conflictPass * 100).toFixed(0)}% < 100%`);
if (medianMs > MAX_WARM_MS) breaches.push(`warm recall median ${medianMs.toFixed(1)} ms > ${MAX_WARM_MS}`);

if (json) {
  console.log(JSON.stringify({ k: K, overall, conflictPass, medianMs, buildMs, byKind, rows, breaches, index_built_at: index.builtAt }, null, 1));
} else {
  console.log(`recall-gate — ${rows.length} golden queries, k=${K}, index built ${index.builtAt} (${buildMs.toFixed(0)} ms), warm median ${medianMs.toFixed(1)} ms`);
  console.log(`\nkind         n  recall@5  leaked`);
  for (const [k, b] of Object.entries(byKind)) console.log(`${k.padEnd(11)}${String(b.n).padStart(3)}  ${(b.hit / b.n).toFixed(3).padStart(8)}  ${String(b.leaked).padStart(6)}`);
  console.log(`${"overall".padEnd(11)}${String(rows.length).padStart(3)}  ${overall.toFixed(3).padStart(8)}`);
  const misses = rows.filter((r) => !r.hit || r.leaked);
  if (misses.length) {
    console.log(`\nmisses (${misses.length}):`);
    for (const m of misses) console.log(`  [${m.kind}] "${m.query}" → expected ${m.expect}${m.leaked ? " (RETRACTED KEY LEAKED)" : ""}; top=${m.top}`);
  }
  console.log(`\nRESULT: ${breaches.length ? "FAIL — " + breaches.join("; ") : "PASS"}`);
}
process.exit(breaches.length ? 1 : 0);
