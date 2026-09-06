// aperture-trgpo — unit + CLI pins for the memory index (dist/memory-index.js).
//
// Everything in-process uses INJECTED bank/sidecar/firstSeen (BuildOptions) — no bd, no
// dolt. The two CLI tests spawn `node dist/memory-index.js` with BD_PATH pointed at a stub
// whose every key carries a date, so the Dolt first-seen path is never entered.
//
// Pins (spec §3 secret exclusion on all five surfaces + cache; §4 retrieval; §5 fallback):
//   1. redact() catches every fixture pattern class, keeps labels, leaves identifiers alone
//   2. tags:secret → absent from lines / standing / recall / recallFull / stats keys / cache
//   3. untagged PEM body → excluded by the ≥ 80 % coverage rule
//   4. partial secret in prose → redacted in cache, gist, recall item, recallFull
//   5. superseded hidden by default, revealed with include_superseded; superseded+secret never
//   6. standing entries in full in renderIndex and in STANDING_CACHE (both modes)
//   7. renderFallback = cached standing block + the one line; never bank text
//   8. cache hit when hash unchanged; rebuild when the sidecar changes
//   9. recall ranking: exact key first, #NNN first, conflict → successor, pagination, stale
//  10. recallFull truncation notice
//  11. CLI: stub bank → exit 0 + both headers; failing stub → exit 0 + fallback, no bank text
//
// Run: node --test test/memory-index.test.mjs   (from mcp-server/, after pnpm build)

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadSecretsFixture } from "./fixtures/load-secrets.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const DIST = resolve(HERE, "..", "dist", "memory-index.js");
const FIXTURES = loadSecretsFixture(join(HERE, "fixtures", "memory-secrets.json"));
const SECRETS = FIXTURES.secrets;
const MARKERS = [...Object.values(SECRETS).map((s) => s.marker), FIXTURES.tagged_marker];

// Module constants read env at import time — sandbox BEFORE the dynamic import.
const TMP = mkdtempSync(join(tmpdir(), "memory-index-test-"));
process.env.APERTURE_RUN_DIR = join(TMP, "run");
process.env.APERTURE_MEMORY_META = join(TMP, "no-such-sidecar.json");
process.env.APERTURE_MEMORY_CACHE = join(TMP, "default-cache.json");
delete process.env.APERTURE_STANDING_CACHE;
const mi = await import(DIST);
const { BEAD_ID_RE, redact, buildIndex, renderIndex, renderFallback, recall, recallFull, recallStats, STANDING_CACHE, RECALL_FULL_MAX_BYTES } = mi;

process.on("exit", () => rmSync(TMP, { recursive: true, force: true }));

const NOW = new Date("2026-09-06T12:00:00Z");
let cacheN = 0;
const freshCache = () => join(TMP, `cache-${++cacheN}.json`);

/** Assert none of the seeded markers appears in `text`. */
function assertNoMarkers(text, surface) {
  for (const m of MARKERS) assert.ok(!text.includes(m), `${surface} leaked marker ${JSON.stringify(m)}`);
}

// ── fixture bank ────────────────────────────────────────────────────────────

const BANK = {
  "deploy-order-2026-09-01":
    "Deploy order matters: run migrations before the app restarts. PR #57 shipped the guard in the deploy pipeline. Deploy twice if unsure.",
  "hub-self-heal-pr-46": "Hub self-heal (ws-hub) reconnects after a hub crash; see aperture-3x136 for the launch-path starvation fix.",
  "new-deploy-rule-2026-08-20": "Deploy order rule (current): migrations run in CI before the deploy, never at container start.",
  "old-deploy-rule-2026-05-01": "Deploy order rule (old): run migrations at container start during the deploy.",
  "old-secret-rule-2026-05-02": `Old secret rule body with ${FIXTURES.tagged_marker} inside.`,
  "vault-creds-2026-08-01": `Where the vault creds live: ${FIXTURES.tagged_marker} (plain prose, no regex hit).`,
  "legacy-pem-key-2026-06-01": SECRETS.pem.text,
  "gateway-token-note-2026-08-15": `Gateway ${SECRETS.partial_token.text} is rotated weekly; ask Peppy before the deploy.`,
  "standing-compact-at-60-2026-07-19": "DECISION-1: /compact at 60% context — operator instruction 2026-07-19 supersedes the 80% figure in agent-liveness.",
  "standing-no-full-bank-2026-09-06": "DECISION-2: the full memory bank is never injected; use recall and recall_full.",
  "stale-tmux-note-2026-01-01": "Tmux pane housekeeping: kill stale panes after the deploy window closes.",
  "fresh-tmux-note-2026-09-01": "Tmux pane housekeeping: kill stale panes after the deploy window closes.",
  "deploy-extra-a-2026-08-01": "Deploy note A about the deploy checklist.",
  "deploy-extra-b-2026-08-02": "Deploy note B about the deploy checklist.",
  "undated-note": "An undated note whose age comes from firstSeen.",
};

const SIDECAR = {
  "deploy-order-2026-09-01": { project: "aperture", tags: ["deploy", "ci"], updated: "2026-09-01" },
  "hub-self-heal-pr-46": { project: "aperture", tags: ["comms", "hub"], entities: ["ws-hub"], updated: "2026-07-19" },
  "new-deploy-rule-2026-08-20": { project: "incluir", tags: ["deploy"], supersedes: ["old-deploy-rule-2026-05-01", "old-secret-rule-2026-05-02"] },
  "old-deploy-rule-2026-05-01": { project: "incluir", tags: ["deploy"] },
  "old-secret-rule-2026-05-02": { project: "incluir", tags: ["secret", "deploy"] },
  "vault-creds-2026-08-01": { project: "aperture", tags: ["secret"] },
  "standing-compact-at-60-2026-07-19": { project: "aperture", tags: ["decision"], standing: true },
  "standing-no-full-bank-2026-09-06": { project: "aperture", tags: ["decision"], standing: true },
};

const firstSeen = (key) => (key === "undated-note" ? "2026-03-01" : null);

async function build(overrides = {}) {
  return buildIndex({ bank: BANK, sidecar: SIDECAR, firstSeen, now: NOW, cachePath: freshCache(), ...overrides });
}

// ── 1. redact() ─────────────────────────────────────────────────────────────

test("redact() catches every fixture pattern class and keeps the label", () => {
  for (const [name, s] of Object.entries(SECRETS)) {
    const r = redact(`prefix ${s.text} suffix`);
    assert.equal(r.hit, true, `${name}: no hit`);
    assert.ok(r.spans >= 1, `${name}: spans`);
    assert.ok(!r.text.includes(s.marker), `${name}: marker survived → ${r.text}`);
    assert.ok(r.text.includes("[REDACTED]"), `${name}: no [REDACTED] token`);
    assert.ok(r.text.startsWith("prefix ") && r.text.endsWith(" suffix"), `${name}: context damaged → ${r.text}`);
    if (s.label) assert.ok(r.text.includes(s.label), `${name}: label ${s.label} dropped → ${r.text}`);
  }
});

test("redact() leaves identifiers, hashes, env-var names and prose alone", () => {
  for (const safe of FIXTURES.safe) {
    const r = redact(safe);
    assert.equal(r.hit, false, `false positive on ${JSON.stringify(safe)} → ${r.text}`);
    assert.equal(r.text, safe);
  }
  assert.equal(redact("").hit, false);
});

test("redact() is idempotent and never double-counts an already-redacted value", () => {
  const once = redact(`password=${SECRETS.password_label.marker} and ${SECRETS.openai_key.text}`);
  assert.equal(once.spans, 2);
  const twice = redact(once.text);
  assert.equal(twice.hit, false);
  assert.equal(twice.text, once.text);
});

// ── 2. tags:secret excluded from every surface + the cache ──────────────────

test("secret-tagged entry is absent from lines, standing, recall, recallFull, stats keys and the cache file", async () => {
  const cachePath = freshCache();
  const idx = await build({ cachePath });
  const keys = (arr) => arr.map((x) => x.key);
  assert.ok(!keys(idx.lines).includes("vault-creds-2026-08-01"));
  assert.ok(!keys(idx.entries).includes("vault-creds-2026-08-01"));
  assert.ok(!keys(idx.standing).includes("vault-creds-2026-08-01"));
  assert.equal(recallFull(idx, "vault-creds-2026-08-01"), null);
  const r = recall(idx, { query: "vault creds live" });
  assert.ok(!keys(r.items).includes("vault-creds-2026-08-01"));
  const stats = recallStats(idx, cachePath);
  assert.equal(stats.secretExcluded, 3, "vault-creds + old-secret-rule (tagged) + legacy-pem (coverage)");
  assert.ok(!JSON.stringify(stats).includes("vault-creds"));
  assert.ok(!JSON.stringify(stats).includes("secret-rule"));
  assert.equal(stats.total, Object.keys(BANK).length);

  const onDisk = readFileSync(cachePath, "utf8");
  assert.ok(!onDisk.includes("vault-creds-2026-08-01"), "secret key leaked into cache");
  assertNoMarkers(onDisk, "cache file");
  assertNoMarkers(JSON.stringify(idx), "index object");
  assert.equal(statSync(cachePath).mode & 0o777, 0o600);
  assert.ok(!existsSync(`${cachePath}.${process.pid}.tmp`), "tmp file left behind");
});

// ── 3. ≥ 80 % coverage rule ─────────────────────────────────────────────────

test("untagged entry whose body is a PEM block is excluded by the ≥80% rule", async () => {
  const cachePath = freshCache();
  const idx = await build({ cachePath });
  assert.ok(!idx.entries.some((e) => e.key === "legacy-pem-key-2026-06-01"));
  assert.ok(!idx.lines.some((l) => l.key === "legacy-pem-key-2026-06-01"));
  assert.equal(recallFull(idx, "legacy-pem-key-2026-06-01"), null);
  assert.ok(!readFileSync(cachePath, "utf8").includes(SECRETS.pem.marker));
});

// ── 4. partial secret in prose ──────────────────────────────────────────────

test("partial secret (token=… inside prose) is redacted in cache, gist, recall item and recallFull", async () => {
  const cachePath = freshCache();
  const idx = await build({ cachePath });
  const key = "gateway-token-note-2026-08-15";
  const entry = idx.entries.find((e) => e.key === key);
  assert.ok(entry, "partial-secret entry must stay live");
  assert.equal(entry.redacted, true);
  assert.equal(entry.secret, false);
  assert.ok(entry.body.includes("token=[REDACTED]"));
  assert.ok(!entry.body.includes(SECRETS.partial_token.marker));

  const line = idx.lines.find((l) => l.key === key);
  assert.equal(line.gist, "[redacted gist]", "gist span held a secret → placeholder");

  const r = recall(idx, { query: "gateway rotated weekly" });
  const item = r.items.find((i) => i.key === key);
  assert.ok(item, "recall must return the live partial-secret entry");
  assert.equal(item.gist, "[redacted gist]");

  const full = recallFull(idx, key);
  assert.ok(full.body.includes("token=[REDACTED]"));
  assert.ok(!full.body.includes(SECRETS.partial_token.marker));

  assert.ok(!readFileSync(cachePath, "utf8").includes(SECRETS.partial_token.marker));
  assert.ok(!renderIndex(idx, "boot").includes(SECRETS.partial_token.marker));
});

// ── 5. supersession ─────────────────────────────────────────────────────────

test("superseded entry hidden by default, revealed with include_superseded; superseded+secret never", async () => {
  const idx = await build();
  const old = "old-deploy-rule-2026-05-01";
  assert.ok(!idx.lines.some((l) => l.key === old), "superseded entry must not be in lines");
  const entry = idx.entries.find((e) => e.key === old);
  assert.equal(entry.supersededBy, "new-deploy-rule-2026-08-20");
  assert.equal(idx.supersededCount, 1, "only the non-secret superseded entry counts");

  const hidden = recall(idx, { query: "container start migrations" });
  assert.ok(!hidden.items.some((i) => i.key === old));
  const shown = recall(idx, { query: "container start migrations", include_superseded: true });
  const item = shown.items.find((i) => i.key === old);
  assert.ok(item, "include_superseded reveals it");
  assert.equal(item.supersededBy, "new-deploy-rule-2026-08-20");
  assert.equal(recallFull(idx, old).supersededBy, "new-deploy-rule-2026-08-20");
  assert.deepEqual(recallFull(idx, "new-deploy-rule-2026-08-20").supersedes, [old, "old-secret-rule-2026-05-02"]);

  const revealed = recall(idx, { query: "old secret rule body", include_superseded: true });
  assert.ok(!revealed.items.some((i) => i.key === "old-secret-rule-2026-05-02"), "superseded+secret leaked");
  assert.equal(recallFull(idx, "old-secret-rule-2026-05-02"), null);
  assert.ok(!JSON.stringify(idx).includes(FIXTURES.tagged_marker));
});

// ── 6. standing block ───────────────────────────────────────────────────────

test("standing entries appear in full in renderIndex (both modes) and in STANDING_CACHE", async () => {
  const idx = await build();
  assert.deepEqual(
    idx.standing.map((s) => s.key),
    ["standing-compact-at-60-2026-07-19", "standing-no-full-bank-2026-09-06"],
  );
  for (const mode of ["boot", "precompact"]) {
    const out = renderIndex(idx, mode);
    assert.ok(out.includes(`mode=${mode}`), "mode in header comment");
    assert.ok(out.includes("## Standing decisions (2)"));
    assert.ok(out.includes(BANK["standing-compact-at-60-2026-07-19"]));
    assert.ok(out.includes(BANK["standing-no-full-bank-2026-09-06"]));
    assert.ok(out.includes("## Memory index (11 live, 1 superseded hidden, 3 secret excluded)"), out.split("\n").find((l) => l.startsWith("## Memory")));
    assert.ok(out.includes("Use recall(query) for ranked gists and recall_full(key) for a body; the full bank is never injected."));
    assert.ok(out.includes("### project: aperture ("), "lines grouped under a project heading");
    if (mode === "boot") {
      assert.ok(out.includes("- deploy-order-2026-09-01 · deploy,ci · Deploy order matters: run migrations before the app restarts. · 5d"));
      assert.ok(out.includes("- undated-note · - · An undated note whose age comes from firstSeen. · 189d"));
    } else {
      assert.ok(out.includes("- deploy-order-2026-09-01 · deploy,ci · 5d"), "precompact lines carry no gist");
      assert.ok(!out.includes("Deploy order matters"), "precompact must not carry gists");
    }
    assert.ok(!out.includes("old-deploy-rule"), "superseded hidden from render");
    assertNoMarkers(out, `renderIndex(${mode})`);
  }
  const cached = readFileSync(STANDING_CACHE, "utf8");
  assert.ok(cached.startsWith("## Standing decisions (2)"));
  assert.ok(cached.includes(BANK["standing-compact-at-60-2026-07-19"]));
  assert.equal(statSync(STANDING_CACHE).mode & 0o777, 0o600);
});

test("standing entries are COMPLETE (reviewed standing_text verbatim, else full body marked unreviewed); >1200 B standing_text is rejected; over-budget block renders everything and flags itself", async () => {
  const big = "x".repeat(6000);
  const reviewed = "RULE: never inject the bank at boot. Exception: operator-run plain sessions get the bd preamble only. Pending: Cipher regex review.";
  const idx = await build({
    bank: { ...BANK, "standing-big-a-2026-09-01": `A ${big}`, "standing-reviewed-2026-09-02": `long narrative ${big}` },
    sidecar: { ...SIDECAR, "standing-big-a-2026-09-01": { standing: true }, "standing-reviewed-2026-09-02": { standing: true, standing_text: reviewed } },
  });
  const out = renderIndex(idx, "boot");
  const block = out.slice(out.indexOf("## Standing decisions"), out.indexOf("## Memory index"));
  assert.ok(block.includes(`- **standing-reviewed-2026-09-02** — ${reviewed}`), "reviewed statement rendered verbatim, complete");
  assert.ok(!block.includes("standing-reviewed-2026-09-02** [unreviewed"), "reviewed entry not marked unreviewed");
  assert.ok(block.includes("standing-big-a-2026-09-01** [unreviewed — full memory body; reviewed statement pending] — A " + big), "unreviewed entry carries its FULL body, never an excerpt");
  assert.ok(!block.includes("…"), "no excerpt ellipsis anywhere in the standing block");
  // >600 B standing_text → sidecar REJECTED (never truncated)
  await assert.rejects(
    build({ bank: BANK, sidecar: { ...SIDECAR, "standing-compact-at-60-2026-07-19": { standing: true, standing_text: "y".repeat(1301) } } }),
    /standing_text is 1301 bytes > 1300 — shorten by review, never truncate/,
  );
  // over-budget: NOTHING is dropped — every entry still rendered, plus a visible over-budget marker
  const many = {}; const manyMeta = {};
  for (let i = 0; i < 40; i++) { const k = `standing-rule-${String(i).padStart(2, "0")}-2026-09-01`; many[k] = `Rule ${i}: ${"y".repeat(500)}`; manyMeta[k] = { standing: true }; }
  const idx2 = await build({ bank: { ...BANK, ...many }, sidecar: { ...SIDECAR, ...manyMeta } });
  const out2 = renderIndex(idx2, "precompact");
  for (let i = 0; i < 40; i++) assert.ok(out2.includes(`- **standing-rule-${String(i).padStart(2, "0")}-2026-09-01**`), `entry ${i} must still be rendered`);
  assert.match(out2, /\[STANDING BLOCK OVER BUDGET: \d+ > 16384 bytes — all 42 entries are still rendered above; the release gate must fail\]/);
  assert.ok(!/…/.test(out2), "no partial rule text");
});

test("renderFallback emits the cached standing block + exactly one unavailable line, never bank text", async () => {
  await build();
  const out = renderFallback("bd invocation failed after 12ms | exit: 1");
  assert.ok(out.startsWith("## Standing decisions (2)"));
  assert.ok(out.includes(BANK["standing-no-full-bank-2026-09-06"]));
  const lines = out.trimEnd().split("\n");
  assert.equal(lines.at(-1), "[memory index unavailable: bd invocation failed after 12ms | exit: 1 — use recall/recall_full]");
  assert.equal(lines.filter((l) => l.startsWith("[memory index unavailable")).length, 1);
  for (const key of Object.keys(BANK)) if (!key.startsWith("standing-")) assert.ok(!out.includes(key), `bank key ${key} leaked`);
  assertNoMarkers(out, "fallback");

  const bare = renderFallback("no cache", join(TMP, "missing-standing.md"));
  assert.equal(bare, "[memory index unavailable: no cache — use recall/recall_full]\n");
});

test("renderFallback re-redacts a tampered standing cache", () => {
  const p = join(TMP, "tampered-standing.md");
  writeFileSync(p, `## Standing decisions (1)\n\n### x\n${SECRETS.openai_key.text}\n`);
  const out = renderFallback("x", p);
  assert.ok(!out.includes(SECRETS.openai_key.marker));
  assert.ok(out.includes("[REDACTED]"));
});

// ── 8. cache ────────────────────────────────────────────────────────────────

test("cache hit when hash unchanged (second build reads the file); rebuild when the sidecar changes", async () => {
  const cachePath = freshCache();
  const t1 = new Date("2026-09-06T10:00:00Z");
  const t2 = new Date("2026-09-06T11:00:00Z");
  const t3 = new Date("2026-09-06T12:00:00Z");
  const first = await build({ cachePath, now: t1 });
  assert.equal(first.builtAt, t1.toISOString());
  const mtime1 = statSync(cachePath).mtimeMs;

  const second = await build({ cachePath, now: t2 });
  assert.equal(second.builtAt, t1.toISOString(), "builtAt must come from the cache file, not the clock");
  assert.equal(second.hash, first.hash);
  assert.equal(statSync(cachePath).mtimeMs, mtime1, "cache hit must not rewrite the file");
  assert.deepEqual(second.lines, first.lines);
  assert.deepEqual(second.entries.map((e) => e.key), first.entries.map((e) => e.key));

  // Ages are recomputed against `now` on a hit (same day here → equal).
  assert.equal(second.entries.find((e) => e.key === "undated-note").ageDays, 189);

  const third = await build({ cachePath, now: t3, sidecar: { ...SIDECAR, "deploy-extra-a-2026-08-01": { project: "aperture" } } });
  assert.notEqual(third.hash, first.hash);
  assert.equal(third.builtAt, t3.toISOString(), "sidecar change → rebuild");
  assert.ok(statSync(cachePath).mtimeMs > mtime1);
  assert.equal(third.lines.find((l) => l.key === "deploy-extra-a-2026-08-01").project, "aperture");
});

test("a corrupt cache file is ignored and rebuilt", async () => {
  const cachePath = freshCache();
  writeFileSync(cachePath, "{not json");
  const idx = await build({ cachePath });
  assert.equal(idx.builtAt, NOW.toISOString());
  assert.doesNotThrow(() => JSON.parse(readFileSync(cachePath, "utf8")));
});

test("malformed sidecar throws; missing sidecar → {}", () => {
  const bad = join(TMP, "bad-sidecar.json");
  writeFileSync(bad, JSON.stringify({ k: { tags: "not-an-array" } }));
  assert.throws(() => mi.loadSidecar(bad), /tags must be an array of strings/);
  writeFileSync(bad, JSON.stringify({ k: { updated: "yesterday" } }));
  assert.throws(() => mi.loadSidecar(bad), /updated must be YYYY-MM-DD/);
  writeFileSync(bad, "[]");
  assert.throws(() => mi.loadSidecar(bad), /top level must be an object/);
  assert.deepEqual(mi.loadSidecar(join(TMP, "nope.json")), {});
  writeFileSync(bad, JSON.stringify({ "not-in-bank": { project: "x", supersedes: ["a"], standing: false, updated: "2026-01-02" } }));
  assert.deepEqual(mi.loadSidecar(bad), { "not-in-bank": { project: "x", supersedes: ["a"], standing: false, updated: "2026-01-02" } });
});

// ── 9. recall ranking ───────────────────────────────────────────────────────

test("recall ranks an exact key match first", async () => {
  const idx = await build();
  const r = recall(idx, { query: "hub-self-heal-pr-46" });
  assert.equal(r.items[0].key, "hub-self-heal-pr-46");
  assert.equal(r.index_built_at, idx.builtAt);
  const r2 = recall(idx, { query: "aperture-3x136 hub" });
  assert.equal(r2.items[0].key, "hub-self-heal-pr-46", "bead-id identifier match ranks first");
});

test("bead ids: 4/5/6-char suffixes all earn exact rank; a longer hyphenated id never partial-matches (aperture-3kavd HOLD #2)", async () => {
  // Isolated bank so fixture totals elsewhere stay untouched. Distractors share MORE terms than the id docs,
  // so only the exact-id rank can put the id doc first.
  const bank = {
    "pane-keystroke-2026-05-23": "Orchestrator pane keystroke capability validated via tmux send-keys (bead aperture-0hv9).",
    "hub-launch-path-2026-07-19": "GUI launch-path PATH starvation, fixed in PR #46 (bead aperture-3x136).",
    "codex-bind-order-2026-09-05": "Codex bind order: hello before bind, retry once (bead aperture-oeb6q).",
    "wisp-thread-2026-09-06": "Message thread aperture-wisp-174klx discussed pane keystroke capability and orchestrator liveness.",
    "distractor-a-2026-09-01": "Orchestrator pane keystroke capability liveness notes about tmux panes and orchestrator capability.",
    "distractor-b-2026-09-02": "More orchestrator pane keystroke capability liveness notes, tmux panes, orchestrator, capability, keystroke.",
  };
  const idx = await buildIndex({ bank, sidecar: {}, firstSeen, now: NOW, cachePath: freshCache() });
  const first = (q) => recall(idx, { query: q, k: 5 }).items[0]?.key;
  assert.equal(first("orchestrator pane keystroke capability aperture-0hv9"), "pane-keystroke-2026-05-23", "4-char id must rank first");
  assert.equal(first("orchestrator pane keystroke capability aperture-3x136"), "hub-launch-path-2026-07-19", "5-char id must rank first");
  assert.equal(first("orchestrator pane keystroke capability aperture-oeb6q"), "codex-bind-order-2026-09-05", "6-char id must rank first");
  // Partial-longer-id rejection: the wisp doc's aperture-wisp-174klx must not register as bead id aperture-wisp,
  // so a query for "aperture-wisp" gets no exact hit at all (and a 4-char query never matches the wisp doc by id).
  assert.deepEqual("aperture-wisp-174klx".match(BEAD_ID_RE), null, "longer hyphenated id yields no bead-id match");
  // "aperture-wisp" still matches the wisp doc LEXICALLY (token `wisp`) — that is fine. What must not happen is an
  // EXACT hit: with both ids in the query, the 4-char exact doc must beat the wisp doc; if the prefix counted as an
  // exact bead id, both would be exact and BM25 tie-break (rare `wisp` term) would put the wisp doc first.
  assert.equal(first("aperture-wisp aperture-0hv9"), "pane-keystroke-2026-05-23", "prefix of a longer id must not be an exact hit");
  assert.deepEqual("see aperture-abcdefg now".match(BEAD_ID_RE), null, "7-char suffix is not a bead id");
  assert.deepEqual("xaperture-0hv9".match(BEAD_ID_RE), null, "left boundary");
  assert.deepEqual("(aperture-0hv9), aperture-3x136. aperture-oeb6q!".match(BEAD_ID_RE), ["aperture-0hv9", "aperture-3x136", "aperture-oeb6q"]);
  assert.equal(recall(idx, { query: "PR #46" }).items[0].key, "hub-launch-path-2026-07-19", "#NNN exact rank still works");
});

test("recall ranks a #NNN identifier match first even when other docs share more terms", async () => {
  const idx = await build();
  const r = recall(idx, { query: "deploy checklist note PR #57" });
  assert.equal(r.items[0].key, "deploy-order-2026-09-01");
  const bare = recall(idx, { query: "#57" });
  assert.equal(bare.items[0].key, "deploy-order-2026-09-01");
  assert.equal(bare.total, 1);
});

test("recall hides a superseded conflict in favour of its successor", async () => {
  const idx = await build();
  const r = recall(idx, { query: "deploy order rule migrations", project: "incluir" });
  assert.equal(r.items[0].key, "new-deploy-rule-2026-08-20");
  assert.ok(!r.items.some((i) => i.key === "old-deploy-rule-2026-05-01"));
  assert.ok(r.items.every((i) => i.key !== "deploy-order-2026-09-01"), "project filter applied");
  const tagged = recall(idx, { query: "deploy", tags: ["ci"] });
  assert.deepEqual(tagged.items.map((i) => i.key), ["deploy-order-2026-09-01"]);
});

test("recall paginates with k + offset and reports total / next_offset", async () => {
  const idx = await build();
  const all = recall(idx, { query: "deploy", k: 10 });
  assert.ok(all.total >= 5, `expected ≥5 deploy matches, got ${all.total}`);
  assert.equal(all.items.length, all.total);
  assert.equal(all.next_offset, null);
  const p1 = recall(idx, { query: "deploy", k: 2 });
  assert.equal(p1.items.length, 2);
  assert.equal(p1.next_offset, 2);
  const p2 = recall(idx, { query: "deploy", k: 2, offset: 2 });
  assert.deepEqual(p2.items.map((i) => i.key), all.items.slice(2, 4).map((i) => i.key));
  assert.equal(p2.next_offset, all.total > 4 ? 4 : null);
  assert.equal(p2.total, all.total);
  const clamped = recall(idx, { query: "deploy", k: 999 });
  assert.ok(clamped.items.length <= 10);
  assert.equal(recall(idx, { query: "zzzznomatch" }).total, 0);
});

test("recall demotes stale entries (> STALE_AFTER_DAYS) and boosts standing", async () => {
  const idx = await build();
  const r = recall(idx, { query: "tmux pane housekeeping" });
  assert.equal(r.items[0].key, "fresh-tmux-note-2026-09-01");
  assert.equal(r.items[1].key, "stale-tmux-note-2026-01-01");
  assert.ok(r.items[0].score > r.items[1].score);
  assert.equal(r.items[1].ageDays, 248);

  const s = recall(idx, { query: "recall full bank injected" });
  assert.equal(s.items[0].key, "standing-no-full-bank-2026-09-06");
  assert.equal(s.items[0].standing, true);
});

// ── 10. recallFull ──────────────────────────────────────────────────────────

test("recallFull truncates at maxBytes with a notice and clamps to RECALL_FULL_MAX_BYTES", async () => {
  const idx = await build();
  const key = "deploy-order-2026-09-01";
  const full = recallFull(idx, key);
  assert.equal(full.truncated, false);
  assert.equal(full.body, BANK[key]);
  assert.equal(full.bytesTotal, Buffer.byteLength(BANK[key]));
  assert.deepEqual(full.tags, ["deploy", "ci"]);

  const cut = recallFull(idx, key, 40);
  assert.equal(cut.truncated, true);
  assert.ok(cut.body.startsWith(BANK[key].slice(0, 40)));
  assert.ok(cut.body.endsWith(`\n[truncated: 40 of ${full.bytesTotal} bytes]`), cut.body);
  assert.equal(recallFull(idx, "no-such-key"), null);

  const big = await build({ bank: { "big-2026-09-01": "y".repeat(20000) }, sidecar: {} });
  const b = recallFull(big, "big-2026-09-01", 1_000_000);
  assert.equal(b.truncated, true);
  assert.ok(b.body.endsWith(`\n[truncated: ${RECALL_FULL_MAX_BYTES} of 20000 bytes]`));
});

test("recallStats reports counts only", async () => {
  const cachePath = freshCache();
  const idx = await build({ cachePath });
  const s = recallStats(idx, cachePath);
  assert.equal(s.total, 15);
  assert.equal(s.live, 11);
  assert.equal(s.superseded, 1);
  assert.equal(s.secretExcluded, 3);
  assert.equal(s.standing, 2);
  assert.equal(s.redactedSpans, 1);
  assert.equal(s.byProject.aperture, 4);
  assert.equal(s.byTag.deploy, 3, "deploy-order + new + old(superseded); the secret one is excluded");
  assert.equal(typeof s.cache_age_seconds, "number");
  assertNoMarkers(JSON.stringify(s), "stats");
});

// ── 11. CLI ─────────────────────────────────────────────────────────────────

function cliEnv(stubBody) {
  const dir = mkdtempSync(join(TMP, "cli-"));
  mkdirSync(join(dir, "run"));
  writeFileSync(join(dir, "meta.json"), JSON.stringify({ "standing-rule-2026-09-01": { standing: true, project: "aperture" } }));
  const stub = join(dir, "bd");
  writeFileSync(stub, `#!/bin/sh\n${stubBody}\n`);
  chmodSync(stub, 0o755);
  return {
    dir,
    env: {
      ...process.env,
      APERTURE_MEMORY_META: join(dir, "meta.json"),
      APERTURE_MEMORY_CACHE: join(dir, "cache.json"),
      APERTURE_RUN_DIR: join(dir, "run"),
      APERTURE_STANDING_CACHE: join(dir, "run", "standing.md"),
      BD_PATH: stub,
    },
  };
}

const STUB_BANK = {
  "standing-rule-2026-09-01": "DECISION-9: stub standing rule body.",
  "stub-note-2026-09-02": `A stub note about the ws-hub with ${SECRETS.partial_token.text} inside.`,
  "stub-secret-2026-09-03": SECRETS.pem.text,
};
const STUB_OK = `if [ "$1" = "memories" ] && [ "$2" = "--json" ]; then cat <<'EOF'\n${JSON.stringify(STUB_BANK)}\nEOF\nexit 0; fi\necho "stub: unexpected argv $*" >&2; exit 2`;

test("CLI --mode boot with a stub bank → exit 0, both headers, redacted, cache + standing written", () => {
  const { dir, env } = cliEnv(STUB_OK);
  const res = spawnSync(process.execPath, [DIST, "--mode", "boot"], { env, encoding: "utf8" });
  assert.equal(res.status, 0, res.stderr);
  assert.ok(res.stdout.includes("<!-- memory-index mode=boot"));
  assert.ok(res.stdout.includes("## Standing decisions (1)"));
  assert.ok(res.stdout.includes("DECISION-9: stub standing rule body."));
  assert.ok(res.stdout.includes("## Memory index (2 live, 0 superseded hidden, 1 secret excluded)"), res.stdout);
  assert.ok(res.stdout.includes("- stub-note-2026-09-02 · - · [redacted gist] · "));
  assertNoMarkers(res.stdout, "CLI boot stdout");
  assert.ok(existsSync(join(dir, "cache.json")));
  assertNoMarkers(readFileSync(join(dir, "cache.json"), "utf8"), "CLI cache");
  assert.ok(readFileSync(join(dir, "run", "standing.md"), "utf8").includes("DECISION-9"));

  const pre = spawnSync(process.execPath, [DIST, "--mode=precompact", "--sidecar", join(dir, "meta.json"), "--cache", join(dir, "cache.json")], { env, encoding: "utf8" });
  assert.equal(pre.status, 0);
  assert.ok(pre.stdout.includes("mode=precompact"));
  assert.ok(pre.stdout.includes("## Standing decisions (1)") && pre.stdout.includes("## Memory index (2 live"));

  const stats = spawnSync(process.execPath, [DIST, "--stats"], { env, encoding: "utf8" });
  assert.equal(stats.status, 0);
  const parsed = JSON.parse(stats.stdout);
  assert.equal(parsed.total, 3);
  assert.equal(parsed.secretExcluded, 1);
  assert.equal(parsed.standing, 1);
});

test("CLI --mode boot with a failing bd → exit 0, fallback line + last-good standing block, no bank text", () => {
  const { dir, env } = cliEnv(STUB_OK);
  // Warm the standing cache first, then break bd.
  assert.equal(spawnSync(process.execPath, [DIST, "--mode", "boot"], { env, encoding: "utf8" }).status, 0);
  writeFileSync(join(dir, "bd"), `#!/bin/sh\necho "bd: simulated outage" >&2\nexit 1\n`);
  rmSync(join(dir, "cache.json"), { force: true });

  const res = spawnSync(process.execPath, [DIST, "--mode", "boot"], { env, encoding: "utf8" });
  assert.equal(res.status, 0, res.stderr);
  assert.match(res.stdout, /^\[memory index unavailable: .*simulated outage.* — use recall\/recall_full\]$/m);
  assert.ok(res.stdout.includes("## Standing decisions (1)"), "last-good standing block retained");
  assert.ok(res.stdout.includes("DECISION-9"));
  assert.ok(!res.stdout.includes("## Memory index"));
  assert.ok(!res.stdout.includes("stub-note"), "bank text leaked into fallback");
  assert.ok(!res.stdout.includes("ws-hub"));
  assertNoMarkers(res.stdout, "CLI fallback stdout");

  const stats = spawnSync(process.execPath, [DIST, "--stats"], { env, encoding: "utf8" });
  assert.equal(stats.status, 0);
  assert.ok(JSON.parse(stats.stdout).error.includes("simulated outage"));

  // A cold start with no standing cache at all still exits 0 with just the one line.
  const cold = cliEnv(`echo "bd: down" >&2; exit 1`);
  const r2 = spawnSync(process.execPath, [DIST, "--mode", "precompact"], { env: cold.env, encoding: "utf8" });
  assert.equal(r2.status, 0);
  assert.equal(r2.stdout.trim().split("\n").length, 1);
  assert.match(r2.stdout, /^\[memory index unavailable: /);
});

test("dist build is present (sanity for the CLI tests)", () => {
  assert.ok(existsSync(DIST));
  execFileSync(process.execPath, ["--check", DIST]);
});
