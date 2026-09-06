/**
 * memory-recall — automatic top-3 memory recall for a Claude agent's prompt.
 *
 * Claude Code runs this as a `UserPromptSubmit` hook (aperture-trgpo, context
 * diet §6), as a SEPARATE entry from presence-hint.js so the two cannot affect
 * each other's failure or latency:
 *
 *   UserPromptSubmit → node <repo>/mcp-server/dist/memory-recall.js
 *
 * Claude Code pipes the hook payload on stdin:
 *   { "hook_event_name": "UserPromptSubmit", "prompt": "<user text>", "session_id": …, "cwd": … }
 * and whatever this script prints on stdout becomes model context for the turn.
 *
 * Output (stdout), ≤ OUTPUT_MAX_BYTES:
 *   [memory recall] top matches for this prompt — use recall_full(key) for detail:
 *   - <key> · <gist> (<age>)
 *   - …                                    (≤ 3 lines)
 * Zero matches → NOTHING is printed (an empty hint is not context).
 * Failure/timeout → exactly `[recall unavailable: <reason>]` — visible, never
 * silent, so a missing hint is never mistaken for "nothing relevant".
 *
 * Query = prompt text + APERTURE_ACTIVE_BEAD (if set) + any `aperture-xxxx`
 * ids found in the prompt, so bead-scoped memories surface for a bare
 * "continue with the bead" prompt.
 *
 * Hooks run SYNCHRONOUSLY inside the agent's session, so this MUST be fast
 * and MUST NEVER fail the session:
 *   - ALWAYS exits 0, on every path.
 *   - Hard timeout APERTURE_RECALL_TIMEOUT_MS (default 1500 ms) covering
 *     stdin + index build + ranking; on expiry prints the unavailable line.
 *   - stdin is read in full, but guarded: if it has not ended after
 *     STDIN_GRACE_MS the script proceeds with whatever was read.
 *
 * No-op rules (exit 0, no output):
 *   - APERTURE_HUB_TOKEN_FILE unset → not an Aperture-launched agent.
 *   - prompt shorter than MIN_PROMPT_CHARS (nothing to rank on).
 *   - prompt starts with "/" (slash command — /compact, /clear, /loop …).
 *
 * Everything printed has already been through redact() in memory-index.ts;
 * secret-tagged memories never reach this script.
 */

import { BEAD_ID_RE, buildIndex, recall } from "./memory-index.js";

const DEFAULT_TIMEOUT_MS = 1500;
const STDIN_GRACE_MS = 300;
const MIN_PROMPT_CHARS = 12;
const TOP_K = 3;
const OUTPUT_MAX_BYTES = 600;
const QUERY_MAX_CHARS = 2000;
const HEADER = "[memory recall] top matches for this prompt — use recall_full(key) for detail:";

let settled = false;

/** Print (optionally) and leave with exit 0. Exactly once. */
function finish(out?: string): void {
  if (settled) return;
  settled = true;
  clearTimeout(hardTimer);
  if (!out) {
    process.exit(0);
  }
  // Pipes are async on macOS: wait for the flush before exiting, with a backstop.
  const bail = setTimeout(() => process.exit(0), 200);
  process.stdout.write(out, () => {
    clearTimeout(bail);
    process.exit(0);
  });
}

function unavailable(reason: string): void {
  const oneLine = reason.replace(/\s+/g, " ").trim().slice(0, 200);
  finish(`[recall unavailable: ${oneLine}]\n`);
}

const envTimeout = Number(process.env.APERTURE_RECALL_TIMEOUT_MS);
const timeoutMs = Number.isFinite(envTimeout) && envTimeout > 0 ? envTimeout : DEFAULT_TIMEOUT_MS;
const hardTimer = setTimeout(() => unavailable(`timed out after ${timeoutMs}ms`), timeoutMs);

if (!process.env.APERTURE_HUB_TOKEN_FILE) {
  // Not an Aperture-launched agent: silent no-op.
  finish();
}

/** Read all of stdin; resolve early with what was read if it has not ended by the grace period. */
function readStdin(): Promise<string> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    let done = false;
    const settle = () => {
      if (done) return;
      done = true;
      clearTimeout(grace);
      resolve(Buffer.concat(chunks).toString("utf8"));
    };
    const grace = setTimeout(settle, STDIN_GRACE_MS);
    process.stdin.on("data", (c: Buffer) => chunks.push(c));
    process.stdin.on("end", settle);
    process.stdin.on("error", settle);
    process.stdin.resume();
  });
}

function extractPrompt(raw: string): string {
  try {
    const payload = JSON.parse(raw);
    return typeof payload?.prompt === "string" ? payload.prompt : "";
  } catch {
    return "";
  }
}

function formatAge(ageDays: number | null): string {
  return ageDays === null ? "age?" : `${ageDays}d`;
}

/** Header + up to TOP_K lines, trimmed to OUTPUT_MAX_BYTES (whole lines only). */
function render(items: Array<{ key: string; gist: string; ageDays: number | null }>): string {
  const lines = [HEADER];
  let bytes = Buffer.byteLength(HEADER) + 1;
  for (const it of items.slice(0, TOP_K)) {
    const line = `- ${it.key} · ${it.gist} (${formatAge(it.ageDays)})`;
    const len = Buffer.byteLength(line) + 1;
    if (bytes + len > OUTPUT_MAX_BYTES) break;
    lines.push(line);
    bytes += len;
  }
  return lines.length > 1 ? lines.join("\n") + "\n" : "";
}

async function main(): Promise<void> {
  const prompt = extractPrompt(await readStdin()).trim();
  if (prompt.length < MIN_PROMPT_CHARS || prompt.startsWith("/")) {
    finish();
    return;
  }
  const terms = [prompt.slice(0, QUERY_MAX_CHARS)];
  const activeBead = process.env.APERTURE_ACTIVE_BEAD?.trim();
  if (activeBead) terms.push(activeBead);
  for (const id of new Set(prompt.toLowerCase().match(BEAD_ID_RE) ?? [])) terms.push(id);

  const idx = await buildIndex();
  const r = recall(idx, { query: terms.join(" "), k: TOP_K });
  finish(render(r.items));
}

main().catch((e: unknown) => {
  unavailable(e instanceof Error ? e.message : String(e));
});
