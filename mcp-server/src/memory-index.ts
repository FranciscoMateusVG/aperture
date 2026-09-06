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

import { createHash } from "node:crypto";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

// ── paths / knobs ─────────────────────────────────────────────────────────

export const RUN_DIR = process.env.APERTURE_RUN_DIR ?? resolve(homedir(), ".aperture", "run");
/** Sidecar metadata, keyed by memory key. Repo seed lives at docs/memory-meta.seed.json. */
export const SIDECAR_PATH = process.env.APERTURE_MEMORY_META ?? resolve(homedir(), ".aperture", "memory-meta.json");
/** Redacted index cache, keyed by sha256(bank JSON + sidecar JSON). */
export const CACHE_PATH = process.env.APERTURE_MEMORY_CACHE ?? join(RUN_DIR, "memory-index.json");
/** Last-good standing block, used by the boot fallback when indexing fails. */
export const STANDING_CACHE = process.env.APERTURE_STANDING_CACHE ?? join(RUN_DIR, "standing.md");

export const GIST_MAX_WORDS = 12;
export const STANDING_BLOCK_MAX_BYTES = 8 * 1024;
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
  /** Standing entries in full (redacted), in stable key order. */
  standing: Array<{ key: string; body: string }>;
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

/**
 * Replace secret-bearing spans with "[REDACTED]". Patterns (fail-closed — prefer a false
 * positive over a leak): API keys/tokens (sk-…, ghp_…, xox…, AKIA…, Bearer …), PEM blocks,
 * `password=`/`secret=`/`token=` values, mempalace/credential drawer paths, base64 runs ≥ 32.
 * Returns the text and whether anything was replaced. Pure; no I/O.
 */
export function redact(text: string): { text: string; hit: boolean; spans: number } {
  throw new Error("TODO(memory-index worker): implement redact()");
}

// ── loading ───────────────────────────────────────────────────────────────

/** Read + validate the sidecar. Missing file → {}. Malformed → throws (fail closed at build). */
export function loadSidecar(path: string = SIDECAR_PATH): Sidecar {
  throw new Error("TODO(memory-index worker): implement loadSidecar()");
}

/** `bd memories --json` → { key: body }. Raw bodies — INTERNAL ONLY, never returned to callers. */
export async function loadBank(): Promise<Record<string, string>> {
  throw new Error("TODO(memory-index worker): implement loadBank() via beads.runBd");
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

/**
 * Build the index (or load it from cache when sha256(bank+sidecar) matches). Applies redact()
 * to every body BEFORE anything is stored; writes the cache (redacted) atomically; refreshes
 * STANDING_CACHE with the standing block. Throws on bd failure — callers decide the fallback.
 */
export async function buildIndex(opts: BuildOptions = {}): Promise<MemoryIndex> {
  throw new Error("TODO(memory-index worker): implement buildIndex()");
}

export function computeHash(bankJson: string, sidecarJson: string): string {
  return createHash("sha256").update(bankJson).update("\n--sidecar--\n").update(sidecarJson).digest("hex");
}

// ── rendering (injection seams) ───────────────────────────────────────────

/**
 * boot       = "## Standing decisions" (full, ≤ STANDING_BLOCK_MAX_BYTES) + "## Memory index (N live)" lines
 * precompact = same two blocks (the bd workflow preamble is added by aperture-prime.sh only in boot mode)
 * Every line is already redacted. Includes a one-line legend telling the agent to use recall/recall_full.
 */
export function renderIndex(index: MemoryIndex, mode: RenderMode): string {
  throw new Error("TODO(memory-index worker): implement renderIndex()");
}

/**
 * Used when buildIndex() throws. Emits the last-good standing block from STANDING_CACHE (if any)
 * plus exactly one line: "[memory index unavailable: <reason> — use recall/recall_full]".
 * NEVER the bank.
 */
export function renderFallback(reason: string, standingCachePath: string = STANDING_CACHE): string {
  throw new Error("TODO(memory-index worker): implement renderFallback()");
}

// ── retrieval ─────────────────────────────────────────────────────────────

/** BM25 over key+redacted body; standing +boost; > STALE_AFTER_DAYS demoted; superseded hidden unless asked; secret never. */
export function recall(index: MemoryIndex, q: RecallQuery): RecallResult {
  throw new Error("TODO(memory-index worker): implement recall()");
}

/** Full redacted body, truncated to maxBytes with notice. Unknown/secret key → null. */
export function recallFull(index: MemoryIndex, key: string, maxBytes: number = RECALL_FULL_MAX_BYTES): RecallFullResult | null {
  throw new Error("TODO(memory-index worker): implement recallFull()");
}

/** Sanitised counts only — never bodies, never secret keys. */
export function recallStats(index: MemoryIndex, cachePath: string = CACHE_PATH): RecallStats {
  throw new Error("TODO(memory-index worker): implement recallStats()");
}

// ── CLI ───────────────────────────────────────────────────────────────────
// Implemented by the memory-index worker at the bottom of this file:
//   if (import.meta.url === pathToFileURL(process.argv[1]).href) { … parse --mode/--stats … }
// Prints renderIndex() or renderFallback(); always exits 0.
