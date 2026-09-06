/**
 * memory-index — indexed lazy retrieval over the BEADS memory bank (aperture-trgpo, v3.2).
 *
 * Spec: docs/superpowers/specs/2026-09-06-context-diet-design.md (v1.1, master af184d7).
 *
 * The bank (`bd memories --json`, 200 entries, ~373 KiB) used to be injected whole into
 * every agent's boot prompt (and again at every PreCompact). This module builds a ~25 KiB
 * INDEX (one line per live entry) plus on-demand `recall` / `recall_full` / `recall_stats`,
 * and renders the two injection modes (`boot`, `precompact`).
 *
 * Invariants (binding — GLaDOS review + operator GO 2026-09-06):
 *   - NON-DESTRUCTIVE: this module never writes to the bank. Metadata lives in a sidecar.
 *   - FAIL-CLOSED SANITIZATION: `redact()` is applied to every output surface — index lines,
 *     gists, recall items, recall_full bodies, and the cache file at write time. The cache
 *     never holds unredacted text.
 *   - SECRET-TAGGED RECORDS ARE EXCLUDED ENTIRELY: sidecar `tags: ["secret"]` (or a redact()
 *     hit that covers the whole body) removes the entry from index, recall, recall_full and
 *     stats. `include_superseded` reveals superseded NON-secret entries only.
 *   - BOOT FALLBACK KEEPS STANDING RULES: if the bank/index cannot be built, `renderFallback`
 *     still emits the last-good standing block (from `STANDING_CACHE`) — never the full bank.
 *
 * CLI (used by scripts/aperture-prime.sh):
 *   node dist/memory-index.js --mode boot|precompact [--sidecar <path>] [--cache <path>]
 *     prints the rendered block to stdout; exit 0 always (fallback text on failure).
 *   node dist/memory-index.js --stats        prints recall_stats JSON.
 */

import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { runBd } from "./beads.js";

// ── paths / knobs ─────────────────────────────────────────────────────────

export const RUN_DIR = process.env.APERTURE_RUN_DIR ?? resolve(homedir(), ".aperture", "run");
/** Sidecar metadata, keyed by memory key. Repo seed lives at docs/memory-meta.seed.json. */
export const SIDECAR_PATH = process.env.APERTURE_MEMORY_META ?? resolve(homedir(), ".aperture", "memory-meta.json");
/** Redacted index cache, keyed by sha256(bank JSON + sidecar JSON). */
export const CACHE_PATH = process.env.APERTURE_MEMORY_CACHE ?? join(RUN_DIR, "memory-index.json");
/** Last-good standing block, used by the boot fallback when indexing fails. */
export const STANDING_CACHE = process.env.APERTURE_STANDING_CACHE ?? join(RUN_DIR, "standing.md");

export const GIST_MAX_WORDS = 10;
export const STANDING_BLOCK_MAX_BYTES = 16 * 1024;
/** Hard limit for a reviewed standing statement; validated in loadSidecar, never truncated. */
export const STANDING_TEXT_MAX_BYTES = 1200;
export const RECALL_FULL_MAX_BYTES = 8 * 1024;
export const RECALL_K_MAX = 10;
export const STALE_AFTER_DAYS = 90;

// ── types (the contract) ──────────────────────────────────────────────────

export interface MemoryMeta {
  project?: string;
  tags?: string[];
  entities?: string[];
  /** Constitutional decision: inlined in full in boot + precompact, boosted in recall. */
  standing?: boolean;
  /** Keys this entry replaces. Superseded keys are hidden from index and recall by default. */
  supersedes?: string[];
  /** ISO date (YYYY-MM-DD). Age source #1; else a date in the key; else Dolt first-seen. */
  updated?: string;
  /** Reviewed, COMPLETE statement of a standing rule (every clause, exception and condition),
   *  ≤ STANDING_TEXT_MAX_BYTES UTF-8. Rendered verbatim in the resident standing block. Longer
   *  → loadSidecar REJECTS the sidecar (never truncates). Absent → the full memory body is
   *  rendered instead (marked unreviewed) so a missing statement never drops a restriction. */
  standing_text?: string;
}
export type Sidecar = Record<string, MemoryMeta>;

export interface MemoryEntry {
  key: string;
  /** REDACTED body. The raw body never leaves loadBank()/buildIndex() internals. */
  body: string;
  bytesTotal: number;
  meta: MemoryMeta;
  ageDays: number | null;
  /** true → excluded from every surface. */
  secret: boolean;
  /** Set when another live entry lists this key in `supersedes`. */
  supersededBy: string | null;
  /** redact() found and replaced at least one span in the body. */
  redacted: boolean;
}

export interface IndexLine {
  key: string;
  project: string | null;
  tags: string[];
  /** ≤ GIST_MAX_WORDS words, redacted; "[redacted gist]" if a secret span fell inside it. */
  gist: string;
  ageDays: number | null;
  standing: boolean;
}

export interface MemoryIndex {
  builtAt: string;
  /** sha256(bank JSON + sidecar JSON) — the cache key. */
  hash: string;
  entries: MemoryEntry[];
  lines: IndexLine[];
  /** Standing entries, stable key order. `text` = reviewed standing_text (reviewed:true) or the full
   *  redacted body (reviewed:false). Always complete — never an excerpt. */
  standing: Array<{ key: string; body: string; text?: string; reviewed?: boolean }>;
  /** Keys excluded as secret (count only is ever exposed). */
  secretCount: number;
  supersededCount: number;
}

export interface RecallQuery {
  query: string;
  k?: number; // ≤ RECALL_K_MAX, default 5
  offset?: number;
  project?: string;
  tags?: string[];
  /** Reveal superseded NON-secret entries. Never reveals secret-tagged entries. */
  include_superseded?: boolean;
}
export interface RecallItem {
  key: string;
  gist: string;
  score: number;
  ageDays: number | null;
  tags: string[];
  standing: boolean;
  supersededBy: string | null;
}
export interface RecallResult {
  items: RecallItem[];
  total: number;
  next_offset: number | null;
  index_built_at: string;
}
export interface RecallFullResult {
  key: string;
  /** Redacted; truncated to max_bytes with a trailing notice when bytesTotal exceeds it. */
  body: string;
  bytesTotal: number;
  truncated: boolean;
  tags: string[];
  supersedes: string[];
  supersededBy: string | null;
}
export interface RecallStats {
  total: number;
  live: number;
  byProject: Record<string, number>;
  byTag: Record<string, number>;
  standing: number;
  superseded: number;
  secretExcluded: number;
  redactedSpans: number;
  index_built_at: string;
  cache_age_seconds: number | null;
}

export type RenderMode = "boot" | "precompact";

// ── redaction (ONE function, every surface) ───────────────────────────────

const REDACTED = "[REDACTED]";
/** Share of a body's characters that redact() must remove for the entry to count as secret. */
const SECRET_COVERAGE = 0.8;

type Rule = { re: RegExp; sub: (m: RegExpExecArray) => string | null };

/** `label=value` → `label=[REDACTED]`; skip when the value is already redacted (no double count). */
function keepLabel(labelGroup: number, sepGroup: number, valueGroup: number): (m: RegExpExecArray) => string | null {
  return (m) => (m[valueGroup].startsWith(REDACTED) ? null : `${m[labelGroup]}${m[sepGroup]}${REDACTED}`);
}

/**
 * Base64 alphabet WITHOUT `_`/`-`: snake/kebab identifiers (env-var names, bead ids, slugs)
 * must stay retrievable. URL-safe tokens carrying `_`/`-` are covered by the vendor + label
 * rules above; a bare one longer than 32 chars between separators is still caught.
 */
const B64_CLASS = "A-Za-z0-9+/=";
const HEX_ID_LENGTHS = new Set([40, 64]);

/** Standalone-run classifier: null = leave alone (identifier / path-ish), REDACTED otherwise. */
function classifyRun(s: string): string | null {
  if (!/[A-Za-z]/.test(s) || !/[0-9]/.test(s)) return null;
  const segments = s.split(/[+/=]+/).filter(Boolean);
  // `sha256:<64hex>` / `WHEEL=<64hex>` / bare hashes — git/sha identifiers we WANT retrievable.
  if (segments.some((seg) => HEX_ID_LENGTHS.has(seg.length) && /^[0-9a-fA-F]+$/.test(seg))) return null;
  // `a/b/c=1/d` — a path or enum list of short words, not a token.
  if (segments.length >= 3 && segments.every((seg) => seg.length < 12)) return null;
  return REDACTED;
}

/**
 * Ordered, fail-closed. Each rule is applied in turn over the whole text; a `sub` returning
 * null leaves the match untouched and uncounted (used to avoid re-redacting "[REDACTED]").
 */
const RULES: Rule[] = [
  // PEM private keys — terminated block, then an unterminated one (still redact to the end).
  { re: /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----/g, sub: () => REDACTED },
  { re: /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*$/g, sub: () => REDACTED },
  // Vendor token shapes.
  { re: /\bsk-[A-Za-z0-9_-]{16,}/g, sub: () => REDACTED },
  { re: /\b(?:ghp_|gho_|github_pat_)[A-Za-z0-9_]{20,}/g, sub: () => REDACTED },
  { re: /\bxox[abp]-[A-Za-z0-9-]{10,}/g, sub: () => REDACTED },
  { re: /\bAKIA[A-Z0-9]{16}\b/g, sub: () => REDACTED },
  { re: /\b(Bearer)(\s+)([A-Za-z0-9._-]{16,})/g, sub: keepLabel(1, 2, 3) },
  // Labelled credentials: keep the label, redact the value.
  { re: /\b(BEADS_DOLT_PASSWORD)(\s*=\s*)(\S+)/g, sub: keepLabel(1, 2, 3) },
  {
    re: /\b(password|passwd|pwd|secret|token|api[_-]?key|client[_-]?secret)(["']?\s*[:=]\s*["']?)(\S{6,})/gi,
    sub: keepLabel(1, 2, 3),
  },
  // mempalace credential drawers — the path IS the secret's address.
  { re: /\b(?:peppy|wing_[A-Za-z0-9_-]*)\/secrets(?:\/[A-Za-z0-9_.-]+)*/g, sub: () => REDACTED },
  { re: /\bmempalace_get_drawer\([^)\n]*\)?/g, sub: () => REDACTED },
  // Standalone base64/hex runs ≥ 32 with both letters and digits. Exactly-40 / exactly-64 hex
  // are git/sha identifiers we WANT retrievable; 7-hex short shas never reach 32 chars.
  {
    re: new RegExp(`(?<![${B64_CLASS}])[${B64_CLASS}]{32,}(?![${B64_CLASS}])`, "g"),
    sub: (m) => classifyRun(m[0]),
  },
];

function applyRule(text: string, rule: Rule): { text: string; spans: number } {
  let out = "";
  let last = 0;
  let spans = 0;
  rule.re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = rule.re.exec(text)) !== null) {
    const rep = rule.sub(m);
    if (m[0].length === 0) {
      rule.re.lastIndex++;
      continue;
    }
    if (rep === null) continue;
    out += text.slice(last, m.index) + rep;
    last = m.index + m[0].length;
    spans++;
  }
  if (spans === 0) return { text, spans: 0 };
  return { text: out + text.slice(last), spans };
}

/**
 * Replace secret-bearing spans with "[REDACTED]". Patterns (fail-closed — prefer a false
 * positive over a leak): API keys/tokens (sk-…, ghp_…, xox…, AKIA…, Bearer …), PEM blocks,
 * `password=`/`secret=`/`token=` values, mempalace/credential drawer paths, base64 runs ≥ 32.
 * Returns the text and whether anything was replaced. Pure; no I/O.
 */
export function redact(text: string): { text: string; hit: boolean; spans: number } {
  let cur = text;
  let spans = 0;
  for (const rule of RULES) {
    const r = applyRule(cur, rule);
    cur = r.text;
    spans += r.spans;
  }
  return { text: cur, hit: spans > 0, spans };
}

/** Fraction of the original characters that redact() removed (0 for an empty body). */
function redactionCoverage(original: string, redactedText: string, spans: number): number {
  if (original.length === 0) return 0;
  const survivors = redactedText.length - spans * REDACTED.length;
  return (original.length - survivors) / original.length;
}

// ── loading ───────────────────────────────────────────────────────────────

const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

function isStringArray(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((x) => typeof x === "string");
}

function validateMeta(key: string, raw: unknown): MemoryMeta {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error(`sidecar: entry "${key}" is not an object`);
  }
  const r = raw as Record<string, unknown>;
  const meta: MemoryMeta = {};
  if (r.project !== undefined) {
    if (typeof r.project !== "string") throw new Error(`sidecar: "${key}".project must be a string`);
    meta.project = r.project;
  }
  for (const f of ["tags", "entities", "supersedes"] as const) {
    if (r[f] !== undefined) {
      if (!isStringArray(r[f])) throw new Error(`sidecar: "${key}".${f} must be an array of strings`);
      meta[f] = r[f] as string[];
    }
  }
  if (r.standing !== undefined) {
    if (typeof r.standing !== "boolean") throw new Error(`sidecar: "${key}".standing must be a boolean`);
    meta.standing = r.standing;
  }
  if (r.updated !== undefined) {
    if (typeof r.updated !== "string" || !ISO_DATE.test(r.updated) || !parseDate(r.updated)) {
      throw new Error(`sidecar: "${key}".updated must be YYYY-MM-DD`);
    }
    meta.updated = r.updated;
  }
  if (r.standing_text !== undefined) {
    if (typeof r.standing_text !== "string" || r.standing_text.trim().length === 0) {
      throw new Error(`sidecar: "${key}".standing_text must be a non-empty string`);
    }
    const n = Buffer.byteLength(r.standing_text, "utf8");
    if (n > STANDING_TEXT_MAX_BYTES) {
      throw new Error(`sidecar: "${key}".standing_text is ${n} bytes > ${STANDING_TEXT_MAX_BYTES} — shorten by review, never truncate`);
    }
    meta.standing_text = r.standing_text;
  }
  return meta;
}

function parseSidecar(json: string, origin: string): Sidecar {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch (e) {
    throw new Error(`sidecar ${origin}: invalid JSON (${(e as Error).message})`);
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`sidecar ${origin}: top level must be an object keyed by memory key`);
  }
  const out: Sidecar = {};
  for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) out[k] = validateMeta(k, v);
  return out;
}

/** Read + validate the sidecar. Missing file → {}. Malformed → throws (fail closed at build). */
/** Repo-tracked seed used when the per-machine sidecar has not been installed yet. Resolved
 *  relative to this module (dist/ → ../../docs), so it works from the worktree and the main checkout. */
export const SIDECAR_SEED_PATH = resolve(dirname(new URL(import.meta.url).pathname), "..", "..", "docs", "memory-meta.seed.json");

export function loadSidecar(path: string = SIDECAR_PATH): Sidecar {
  if (path === SIDECAR_PATH && !existsSync(path) && existsSync(SIDECAR_SEED_PATH)) path = SIDECAR_SEED_PATH;
  if (!existsSync(path)) return {};
  return parseSidecar(readFileSync(path, "utf8"), path);
}

/** `bd memories --json` → { key: body }. Raw bodies — INTERNAL ONLY, never returned to callers. */
export async function loadBank(): Promise<Record<string, string>> {
  const raw = await runBd(["memories", "--json"]);
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (e) {
    throw new Error(`bd memories --json: invalid JSON (${(e as Error).message})`);
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("bd memories --json: expected a {key: body} object");
  }
  const bank: Record<string, string> = {};
  for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
    bank[k] = typeof v === "string" ? v : JSON.stringify(v);
  }
  return bank;
}

// ── ages ──────────────────────────────────────────────────────────────────

function parseDate(iso: string): Date | null {
  const d = new Date(`${iso}T00:00:00Z`);
  return Number.isNaN(d.getTime()) ? null : d;
}

function daysBetween(iso: string, now: Date): number | null {
  const d = parseDate(iso);
  if (!d) return null;
  return Math.max(0, Math.floor((now.getTime() - d.getTime()) / 86_400_000));
}

const DATE_IN_KEY = /\d{4}-\d{2}-\d{2}/;

/** Age source order: sidecar `updated` → first YYYY-MM-DD in the key → firstSeen(key). */
function entryDate(key: string, meta: MemoryMeta, firstSeen: (key: string) => string | null): string | null {
  if (meta.updated) return meta.updated;
  const inKey = DATE_IN_KEY.exec(key);
  if (inKey && parseDate(inKey[0])) return inKey[0];
  const fs = firstSeen(key);
  return fs && parseDate(fs) ? fs : null;
}

const DOLT_ARGS = [
  "--host", "127.0.0.1", "--port", "3307", "--user", "beads", "--password", "", "--no-tls",
  "--use-db", "beads_aperture", "sql", "-q",
  "SELECT `key`, MIN(commit_date) AS first_seen FROM dolt_history_config WHERE `key` LIKE 'kv.memory.%' GROUP BY `key`",
  "-r", "json",
];
const DOLT_TIMEOUT_MS = 5000;

/**
 * One Dolt round-trip per cold build: first commit date of every `kv.memory.<key>` row. Any
 * failure (no dolt, server down, timeout, odd output) → every age null + ONE stderr line.
 */
async function doltFirstSeen(): Promise<(key: string) => string | null> {
  const dates = new Map<string, string>();
  try {
    const stdout = await new Promise<string>((res, rej) => {
      execFile(
        "dolt",
        DOLT_ARGS,
        { timeout: DOLT_TIMEOUT_MS, env: { ...process.env, PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}` } },
        (err, out, stderr) => (err ? rej(new Error(`${err.message}${stderr ? ` | ${stderr.trim()}` : ""}`.slice(0, 300))) : res(out)),
      );
    });
    const parsed = JSON.parse(stdout) as { rows?: Array<Record<string, unknown>> };
    for (const row of parsed.rows ?? []) {
      const k = row.key;
      const d = row.first_seen ?? row["MIN(commit_date)"];
      if (typeof k !== "string" || typeof d !== "string") continue;
      const iso = d.slice(0, 10);
      if (ISO_DATE.test(iso)) dates.set(k.replace(/^kv\.memory\./, ""), iso);
    }
  } catch (e) {
    process.stderr.write(`memory-index: dolt first-seen lookup failed, ages unknown: ${(e as Error).message}\n`);
    return () => null;
  }
  return (key) => dates.get(key) ?? null;
}

// ── building / caching ────────────────────────────────────────────────────

export interface BuildOptions {
  sidecarPath?: string;
  cachePath?: string;
  /** Injected for tests: bank + sidecar instead of bd/disk. */
  bank?: Record<string, string>;
  sidecar?: Sidecar;
  /** Injected for tests / Dolt-less builds: first-seen ISO date per key. */
  firstSeen?: (key: string) => string | null;
  now?: Date;
}

/** On-disk cache = the index plus the per-entry date so ages can be recomputed on a hit. */
interface CacheFile extends MemoryIndex {
  dates: Record<string, string | null>;
}

function writeAtomic(file: string, content: string): void {
  mkdirSync(dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  writeFileSync(tmp, content, { mode: 0o600 });
  renameSync(tmp, file);
}

function isIndexShape(v: unknown): v is CacheFile {
  if (v === null || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  return (
    typeof o.builtAt === "string" &&
    typeof o.hash === "string" &&
    Array.isArray(o.entries) &&
    Array.isArray(o.lines) &&
    Array.isArray(o.standing) &&
    typeof o.secretCount === "number" &&
    typeof o.supersededCount === "number" &&
    o.dates !== null &&
    typeof o.dates === "object" &&
    o.entries.every((e) => e && typeof e === "object" && typeof (e as MemoryEntry).key === "string" && typeof (e as MemoryEntry).body === "string")
  );
}

function readCache(path: string, hash: string, now: Date): MemoryIndex | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return null;
  }
  if (!isIndexShape(parsed) || parsed.hash !== hash) return null;
  const { dates, ...index } = parsed;
  const age = (key: string): number | null => {
    const d = dates[key];
    return d ? daysBetween(d, now) : null;
  };
  for (const e of index.entries) e.ageDays = age(e.key);
  for (const l of index.lines) l.ageDays = age(l.key);
  return index;
}

/** First sentence/clause of a redacted body, ≤ GIST_MAX_WORDS words; "[redacted gist]" if the clause held a secret span. */
function makeGist(body: string): string {
  const cleaned = body.replace(/^[\s#>*\-•]+/, "").trim();
  const cut = /[.!?;](?:\s|$)|\n|\s[—–]\s/.exec(cleaned);
  const clause = (cut ? cleaned.slice(0, cut.index + (/[.!?]/.test(cut[0][0]) ? 1 : 0)) : cleaned).trim();
  if (clause.includes(REDACTED)) return "[redacted gist]";
  const words = clause.split(/\s+/).filter(Boolean);
  if (words.length <= GIST_MAX_WORDS) return words.join(" ");
  return `${words.slice(0, GIST_MAX_WORDS).join(" ")}…`;
}

/**
 * Build the index (or load it from cache when sha256(bank+sidecar) matches). Applies redact()
 * to every body BEFORE anything is stored; writes the cache (redacted) atomically; refreshes
 * STANDING_CACHE with the standing block. Throws on bd failure — callers decide the fallback.
 */
export async function buildIndex(opts: BuildOptions = {}): Promise<MemoryIndex> {
  const now = opts.now ?? new Date();
  const cachePath = opts.cachePath ?? CACHE_PATH;
  // Injected sidecars (tests, programmatic callers) get the SAME validation as the file path —
  // standing_text length/shape rejection must not be bypassable.
  const sidecar: Sidecar = opts.sidecar
    ? Object.fromEntries(Object.entries(opts.sidecar).map(([k, v]) => [k, validateMeta(k, v)]))
    : loadSidecar(opts.sidecarPath ?? SIDECAR_PATH);
  const bank = opts.bank ?? (await loadBank());

  const bankJson = JSON.stringify(bank);
  const sidecarJson = JSON.stringify(sidecar);
  const hash = computeHash(bankJson, sidecarJson);

  const cached = readCache(cachePath, hash, now);
  if (cached) return cached;

  const keys = Object.keys(bank).sort();
  const metaOf = (k: string): MemoryMeta => sidecar[k] ?? {};

  // Pass 1: redact + secret classification (raw bodies die here).
  const redactedBody = new Map<string, { body: string; spans: number; secret: boolean; bytesTotal: number }>();
  for (const k of keys) {
    const raw = bank[k];
    const r = redact(raw);
    const tagged = (metaOf(k).tags ?? []).includes("secret");
    const secret = tagged || (r.hit && redactionCoverage(raw, r.text, r.spans) >= SECRET_COVERAGE);
    redactedBody.set(k, { body: r.text, spans: r.spans, secret, bytesTotal: Buffer.byteLength(raw, "utf8") });
  }

  // Pass 2: supersession — first NON-secret live key whose `supersedes` lists the entry.
  const supersededBy = new Map<string, string>();
  for (const k of keys) {
    if (redactedBody.get(k)!.secret) continue;
    for (const s of metaOf(k).supersedes ?? []) {
      if (s !== k && bank[s] !== undefined && !supersededBy.has(s)) supersededBy.set(s, k);
    }
  }

  // Pass 3: ages — Dolt is consulted only when some key has no other date source.
  const needsFirstSeen = keys.some((k) => !metaOf(k).updated && !DATE_IN_KEY.test(k));
  const firstSeen = opts.firstSeen ?? (needsFirstSeen ? await doltFirstSeen() : () => null);
  const dates: Record<string, string | null> = {};

  const entries: MemoryEntry[] = [];
  const lines: IndexLine[] = [];
  const standing: MemoryIndex["standing"] = [];
  let secretCount = 0;
  let supersededCount = 0;

  for (const k of keys) {
    const rb = redactedBody.get(k)!;
    if (rb.secret) {
      secretCount++;
      continue;
    }
    const meta = metaOf(k);
    const date = entryDate(k, meta, firstSeen);
    dates[k] = date;
    const ageDays = date ? daysBetween(date, now) : null;
    const sup = supersededBy.get(k) ?? null;
    if (sup) supersededCount++;
    entries.push({ key: k, body: rb.body, bytesTotal: rb.bytesTotal, meta, ageDays, secret: false, supersededBy: sup, redacted: rb.spans > 0 });
    if (sup) continue;
    lines.push({
      key: k,
      project: meta.project ?? null,
      tags: meta.tags ?? [],
      gist: makeGist(rb.body),
      ageDays,
      standing: meta.standing === true,
    });
    if (meta.standing === true) {
      // Reviewed statement (redacted like everything else) or the full body — never an excerpt.
      const reviewed = typeof meta.standing_text === "string";
      const text = reviewed ? redact(meta.standing_text as string).text : rb.body;
      standing.push({ key: k, body: rb.body, text, reviewed });
    }
  }

  const index: MemoryIndex = { builtAt: now.toISOString(), hash, entries, lines, standing, secretCount, supersededCount };

  // Cache + standing block: redacted text only, atomic, 0600.
  const cacheFile: CacheFile = { ...index, dates };
  writeAtomic(cachePath, JSON.stringify(cacheFile));
  writeAtomic(STANDING_CACHE, renderStandingBlock(index));
  return index;
}

export function computeHash(bankJson: string, sidecarJson: string): string {
  return createHash("sha256").update(bankJson).update("\n--sidecar--\n").update(sidecarJson).digest("hex");
}

// ── rendering (injection seams) ───────────────────────────────────────────

const LEGEND = "Use recall(query) for ranked gists and recall_full(key) for a body; the full bank is never injected.";

/** "## Standing decisions (N)" + full redacted bodies, capped at STANDING_BLOCK_MAX_BYTES with a notice. */
/** "## Standing decisions (N)" — each entry COMPLETE: the reviewed standing_text verbatim, or (unreviewed)
 *  the full body. No per-entry truncation ever (GLaDOS hold #4); the whole block is capped at
 *  STANDING_BLOCK_MAX_BYTES by dropping WHOLE trailing entries with a notice naming them. */
function renderStandingBlock(index: MemoryIndex): string {
  // GLaDOS hold #4b: NEVER silently drop a standing entry. Every designated statement is rendered
  // in full; if the block exceeds STANDING_BLOCK_MAX_BYTES the release gates (context-budget /
  // retention-gate) FAIL visibly on size — omission is not an option.
  const total = index.standing.length;
  let out = `## Standing decisions (${total})\n`;
  for (const s of index.standing) {
    const text = (s.text ?? s.body).trim().replace(/\s+/g, " ");
    const tag = s.reviewed ? "" : " [unreviewed — full memory body; reviewed statement pending]";
    out += `- **${s.key}**${tag} — ${text}\n`;
  }
  const bytes = Buffer.byteLength(out, "utf8");
  if (bytes > STANDING_BLOCK_MAX_BYTES) {
    out += `[STANDING BLOCK OVER BUDGET: ${bytes} > ${STANDING_BLOCK_MAX_BYTES} bytes — all ${total} entries are still rendered above; the release gate must fail]\n`;
  }
  return out;
}

/** boot: `- key · tags · gist · Nd`; precompact: `- key · tags · Nd` (no gist — precompact must fit ≤ 30 KiB
 *  alongside the standing block; the gists were already in context before compaction). Project is NOT
 *  repeated per line — lines are grouped under `### project: <name>` headings by renderIndex. */
function renderLine(l: IndexLine, mode: RenderMode): string {
  const parts = [l.key, l.tags.length ? l.tags.join(",") : "-"];
  if (mode === "boot") parts.push(l.gist);
  if (l.ageDays !== null) parts.push(`${l.ageDays}d`);
  return `- ${parts.join(" · ")}`;
}

/** Group index lines by project (sorted, "-" last) so the project name costs one heading, not 200 columns. */
function renderGrouped(lines: IndexLine[], mode: RenderMode): string {
  const groups = new Map<string, IndexLine[]>();
  for (const l of lines) {
    const p = l.project ?? "-";
    if (!groups.has(p)) groups.set(p, []);
    groups.get(p)!.push(l);
  }
  const names = [...groups.keys()].sort((a, b) => (a === "-" ? 1 : b === "-" ? -1 : a.localeCompare(b)));
  return names
    .map((p) => `### project: ${p} (${groups.get(p)!.length})
${groups.get(p)!.map((l) => renderLine(l, mode)).join("\n")}`)
    .join("\n");
}

/**
 * boot       = "## Standing decisions" (full, ≤ STANDING_BLOCK_MAX_BYTES) + "## Memory index" lines WITH gists
 * precompact = same standing block + compact lines (key · tags · age, no gist) so standing + index ≤ 30 KiB
 *              (the bd workflow preamble is added by aperture-prime.sh only in boot mode)
 * Every line is already redacted. Includes a one-line legend telling the agent to use recall/recall_full.
 */
export function renderIndex(index: MemoryIndex, mode: RenderMode): string {
  const header = `<!-- memory-index mode=${mode} built=${index.builtAt} hash=${index.hash.slice(0, 12)} -->\n`;
  const standingBlock = renderStandingBlock(index);
  const lines = renderGrouped(index.lines, mode);
  const indexBlock =
    `## Memory index (${index.lines.length} live, ${index.supersededCount} superseded hidden, ${index.secretCount} secret excluded)\n` +
    `${lines}${lines ? "\n" : ""}${LEGEND}\n`;
  return `${header}${standingBlock}\n${indexBlock}`;
}

/**
 * Used when buildIndex() throws. Emits the last-good standing block from STANDING_CACHE (if any)
 * plus exactly one line: "[memory index unavailable: <reason> — use recall/recall_full]".
 * NEVER the bank.
 */
export function renderFallback(reason: string, standingCachePath: string = STANDING_CACHE): string {
  let standing = "";
  try {
    if (existsSync(standingCachePath)) standing = redact(readFileSync(standingCachePath, "utf8")).text.trimEnd();
  } catch {
    standing = "";
  }
  const oneLine = `[memory index unavailable: ${reason.replace(/\s+/g, " ").trim()} — use recall/recall_full]`;
  return standing ? `${standing}\n\n${oneLine}\n` : `${oneLine}\n`;
}

// ── retrieval ─────────────────────────────────────────────────────────────

const BM25_K1 = 1.2;
const BM25_B = 0.75;
const STANDING_BOOST = 1.5;
const STALE_DEMOTION = 0.7;

/** Ids that earn an exact-match rank: bead ids and `#NNN` PR/issue numbers. */
const IDENT_RE = /aperture-[a-z0-9]{5}\b|#\d+\b/g;

/** Lowercase; alphanumeric parts ≥ 2 chars; hyphen/underscore-joined identifiers kept whole as well. */
function tokenize(text: string): string[] {
  const lower = text.toLowerCase();
  const out: string[] = [];
  for (const part of lower.split(/[^a-z0-9]+/)) if (part.length >= 2) out.push(part);
  for (const whole of lower.match(/[a-z0-9]+(?:[-_][a-z0-9]+)+/g) ?? []) out.push(whole);
  return out;
}

function identifiers(text: string): Set<string> {
  return new Set(text.toLowerCase().match(IDENT_RE) ?? []);
}

interface PreparedDoc {
  entry: MemoryEntry;
  tf: Map<string, number>;
  len: number;
  idents: Set<string>;
  gist: string;
}
interface Prepared {
  docs: PreparedDoc[];
  df: Map<string, number>;
  avgLen: number;
  gistByKey: Map<string, string>;
}

const prepared = new WeakMap<MemoryIndex, Prepared>();

function prepare(index: MemoryIndex): Prepared {
  const hit = prepared.get(index);
  if (hit) return hit;
  const gistByKey = new Map(index.lines.map((l) => [l.key, l.gist]));
  const df = new Map<string, number>();
  const docs: PreparedDoc[] = index.entries.map((entry) => {
    const text = `${entry.key} ${entry.body}`;
    const tf = new Map<string, number>();
    const toks = tokenize(text);
    for (const t of toks) tf.set(t, (tf.get(t) ?? 0) + 1);
    for (const t of tf.keys()) df.set(t, (df.get(t) ?? 0) + 1);
    return { entry, tf, len: toks.length, idents: identifiers(text), gist: gistByKey.get(entry.key) ?? makeGist(entry.body) };
  });
  const avgLen = docs.length ? docs.reduce((a, d) => a + d.len, 0) / docs.length : 1;
  const p = { docs, df, avgLen: avgLen || 1, gistByKey };
  prepared.set(index, p);
  return p;
}

function bm25(doc: PreparedDoc, terms: string[], p: Prepared): number {
  const N = p.docs.length;
  let score = 0;
  for (const t of terms) {
    const tf = doc.tf.get(t);
    if (!tf) continue;
    const n = p.df.get(t) ?? 0;
    const idf = Math.log(1 + (N - n + 0.5) / (n + 0.5));
    score += idf * ((tf * (BM25_K1 + 1)) / (tf + BM25_K1 * (1 - BM25_B + (BM25_B * doc.len) / p.avgLen)));
  }
  return score;
}

/** BM25 over key+redacted body; standing +boost; > STALE_AFTER_DAYS demoted; superseded hidden unless asked; secret never. */
export function recall(index: MemoryIndex, q: RecallQuery): RecallResult {
  const p = prepare(index);
  const k = Math.min(RECALL_K_MAX, Math.max(1, Math.floor(q.k ?? 5)));
  const offset = Math.max(0, Math.floor(q.offset ?? 0));
  const queryTerms = [...new Set(tokenize(q.query))];
  const queryIdents = identifiers(q.query);
  const queryWhole = q.query.trim().toLowerCase();
  const wantTags = (q.tags ?? []).map((t) => t.toLowerCase());
  const project = q.project?.toLowerCase();

  const ranked: Array<{ item: RecallItem; exact: boolean }> = [];
  for (const doc of p.docs) {
    const e = doc.entry;
    if (e.supersededBy && !q.include_superseded) continue;
    const tags = (e.meta.tags ?? []).map((t) => t.toLowerCase());
    if (project !== undefined && (e.meta.project ?? "").toLowerCase() !== project) continue;
    if (wantTags.length && !wantTags.every((t) => tags.includes(t))) continue;

    const keyLower = e.key.toLowerCase();
    const exact = queryWhole === keyLower || queryTerms.includes(keyLower) || [...queryIdents].some((id) => doc.idents.has(id));
    let score = bm25(doc, queryTerms, p);
    if (score <= 0 && !exact) continue;
    if (e.meta.standing === true) score *= STANDING_BOOST;
    if (e.ageDays !== null && e.ageDays > STALE_AFTER_DAYS) score *= STALE_DEMOTION;
    ranked.push({
      exact,
      item: {
        key: e.key,
        gist: doc.gist,
        score: Number(score.toFixed(4)),
        ageDays: e.ageDays,
        tags: e.meta.tags ?? [],
        standing: e.meta.standing === true,
        supersededBy: e.supersededBy,
      },
    });
  }
  ranked.sort((a, b) => {
    if (a.exact !== b.exact) return a.exact ? -1 : 1;
    if (b.item.score !== a.item.score) return b.item.score - a.item.score;
    return a.item.key.localeCompare(b.item.key);
  });
  const total = ranked.length;
  const items = ranked.slice(offset, offset + k).map((r) => r.item);
  return { items, total, next_offset: offset + k < total ? offset + k : null, index_built_at: index.builtAt };
}

/** Cut a string to at most `max` UTF-8 bytes without splitting a code point. */
function truncateBytes(s: string, max: number): string {
  const buf = Buffer.from(s, "utf8");
  if (buf.length <= max) return s;
  let end = max;
  while (end > 0 && (buf[end] & 0xc0) === 0x80) end--;
  return buf.subarray(0, end).toString("utf8");
}

/** Full redacted body, truncated to maxBytes with notice. Unknown/secret key → null. */
export function recallFull(index: MemoryIndex, key: string, maxBytes: number = RECALL_FULL_MAX_BYTES): RecallFullResult | null {
  const e = index.entries.find((x) => x.key === key);
  if (!e || e.secret) return null;
  const cap = Math.min(RECALL_FULL_MAX_BYTES, Math.max(1, Math.floor(maxBytes)));
  const fullBytes = Buffer.byteLength(e.body, "utf8");
  const truncated = fullBytes > cap;
  const shown = truncated ? truncateBytes(e.body, cap) : e.body;
  const body = truncated ? `${shown}\n[truncated: ${Buffer.byteLength(shown, "utf8")} of ${fullBytes} bytes]` : shown;
  return {
    key: e.key,
    body,
    bytesTotal: fullBytes,
    truncated,
    tags: e.meta.tags ?? [],
    supersedes: e.meta.supersedes ?? [],
    supersededBy: e.supersededBy,
  };
}

/** Sanitised counts only — never bodies, never secret keys. */
export function recallStats(index: MemoryIndex, cachePath: string = CACHE_PATH): RecallStats {
  const byProject: Record<string, number> = {};
  const byTag: Record<string, number> = {};
  let standing = 0;
  let superseded = 0;
  let redactedSpans = 0;
  for (const e of index.entries) {
    const proj = e.meta.project ?? "(none)";
    byProject[proj] = (byProject[proj] ?? 0) + 1;
    for (const t of e.meta.tags ?? []) byTag[t] = (byTag[t] ?? 0) + 1;
    if (e.meta.standing === true) standing++;
    if (e.supersededBy) superseded++;
    if (e.redacted) redactedSpans += e.body.split(REDACTED).length - 1;
  }
  let cacheAge: number | null = null;
  try {
    cacheAge = Math.max(0, Math.floor((Date.now() - statSync(cachePath).mtimeMs) / 1000));
  } catch {
    cacheAge = null;
  }
  return {
    total: index.entries.length + index.secretCount,
    live: index.entries.length - superseded,
    byProject,
    byTag,
    standing,
    superseded,
    secretExcluded: index.secretCount,
    redactedSpans,
    index_built_at: index.builtAt,
    cache_age_seconds: cacheAge,
  };
}

// ── CLI ───────────────────────────────────────────────────────────────────

function parseArgs(argv: string[]): { mode: RenderMode | null; stats: boolean; sidecar?: string; cache?: string } {
  const out: { mode: RenderMode | null; stats: boolean; sidecar?: string; cache?: string } = { mode: null, stats: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--stats") out.stats = true;
    else if (a === "--mode") out.mode = argv[++i] === "precompact" ? "precompact" : "boot";
    else if (a.startsWith("--mode=")) out.mode = a.slice(7) === "precompact" ? "precompact" : "boot";
    else if (a === "--sidecar") out.sidecar = argv[++i];
    else if (a.startsWith("--sidecar=")) out.sidecar = a.slice(10);
    else if (a === "--cache") out.cache = argv[++i];
    else if (a.startsWith("--cache=")) out.cache = a.slice(8);
  }
  return out;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const opts: BuildOptions = { sidecarPath: args.sidecar, cachePath: args.cache };
  if (args.stats) {
    try {
      const index = await buildIndex(opts);
      process.stdout.write(`${JSON.stringify(recallStats(index, args.cache ?? CACHE_PATH), null, 2)}\n`);
    } catch (e) {
      process.stdout.write(`${JSON.stringify({ error: (e as Error).message })}\n`);
    }
    return;
  }
  const mode: RenderMode = args.mode ?? "boot";
  try {
    const index = await buildIndex(opts);
    process.stdout.write(renderIndex(index, mode));
  } catch (e) {
    process.stdout.write(renderFallback((e as Error).message));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main()
    .catch((e) => process.stdout.write(renderFallback(String((e as Error)?.message ?? e))))
    .finally(() => process.exit(0));
}
