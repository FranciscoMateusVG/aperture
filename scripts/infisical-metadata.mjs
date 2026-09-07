#!/usr/bin/env node
/**
 * infisical-metadata — fixed-purpose, non-model Infisical METADATA reader.
 *
 * Bead: aperture-a4ph5. Design gate: Cipher (aperture-5nxd8 / aperture-wisp-k2t2zp).
 *
 * WHAT THIS IS
 *   Authenticates to the pocketsoftware Infisical instance with the EXISTING
 *   banked `peppy-admin` Universal Auth machine identity, then emits ONLY
 *   project / environment / secret-KEY-NAME metadata as a single bounded JSON
 *   receipt.
 *
 * WHAT THIS IS NOT
 *   Not a secret reader. Not a generic HTTP client. Not an injector. There is
 *   no `inject` action, not even a stub — adding one is a new design review.
 *
 * THE VALUE BOUNDARY (state it honestly)
 *   Infisical v0.146 list-secrets responses DO contain `secretValue`. Values and
 *   the Bearer token therefore exist transiently inside THIS process. They are
 *   never emitted, logged, persisted, fingerprinted, or placed in model-visible
 *   output. The receipt says exactly that — it does NOT claim "no values were
 *   retrieved", which would be false.
 *
 * HARD RULES ENCODED BELOW
 *   - Credential source is a HARDCODED path. No argument, env var, or override.
 *   - No subprocess, no shell, no curl, no dynamic import, no eval.
 *   - Closed network graph: literal host/port/method/path. No redirects, no
 *     proxy, no retries, no fallback URL.
 *   - Upstream status codes, bodies, and headers are NEVER reflected outward.
 *     Errors are stable machine codes only.
 *   - Caught errors are never stringified into output.
 */

import { openSync, fstatSync, lstatSync, readSync, closeSync, constants as FS } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

// ─── Fixed configuration. Nothing here is user-supplied. ────────────────────
const CRED_PATH = join(homedir(), '.config', 'aperture', 'infisical-peppy-admin.env');
const CRED_DIR = join(homedir(), '.config', 'aperture');

const HOST = '100.102.73.112';
const PORT = 3005;
const ORIGIN = `http://${HOST}:${PORT}`;

// The complete, closed set of endpoints this tool may ever contact.
const EP_LOGIN = { method: 'POST', path: '/api/v1/auth/universal-auth/login' };
const EP_WORKSPACES = { method: 'GET', path: '/api/v1/workspace' };
const EP_SECRETS = { method: 'GET', path: '/api/v3/secrets/raw' };

const ONLY_ACTION = 'list-metadata';

const MAX_CRED_BYTES = 4096;
const MAX_VALUE_LEN = 512;      // per credential field
const MAX_BODY_BYTES = 2_000_000;
const MAX_ITEMS = 500;          // projects, envs, or secrets per collection
const MAX_STR = 256;            // any projected identifier/name
// NOTE: with Node's global fetch and no undici dependency we can enforce an
// OVERALL deadline but NOT a separate connect timeout. Documented rather than
// faked: adding a dependency purely for that would widen the supply chain more
// than it buys. Deadline kept tight.
const OVERALL_TIMEOUT_MS = 15000;

// ─── Stable error codes. No upstream detail ever rides along. ───────────────
class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}
const E = {
  BAD_ACTION: 'E_BAD_ACTION',
  CRED_DIR_PERMS: 'E_CRED_DIR_PERMS',
  CRED_MISSING: 'E_CRED_MISSING',
  CRED_NOT_REGULAR: 'E_CRED_NOT_REGULAR',
  CRED_SYMLINK: 'E_CRED_SYMLINK',
  CRED_PERMS: 'E_CRED_PERMS',
  CRED_OWNER: 'E_CRED_OWNER',
  CRED_LINKS: 'E_CRED_LINKS',
  CRED_SIZE: 'E_CRED_SIZE',
  CRED_RACE: 'E_CRED_RACE',
  CRED_PARSE: 'E_CRED_PARSE',
  NETWORK: 'E_NETWORK',
  AUTH_REJECTED: 'E_AUTH_REJECTED',
  REDIRECT_REFUSED: 'E_REDIRECT_REFUSED',
  UPSTREAM_STATUS: 'E_UPSTREAM_STATUS',
  BODY_TOO_LARGE: 'E_BODY_TOO_LARGE',
  BAD_SHAPE: 'E_BAD_SHAPE',
};

// ─── Credential file: open once, validate the SAME file descriptor. ─────────
// lstat first (catch symlink), open with O_NOFOLLOW, then fstat and require
// inode+device to match the lstat — this closes the swap-after-check race.
function readCredentials() {
  let dirSt;
  try { dirSt = lstatSync(CRED_DIR); } catch { throw new Fail(E.CRED_MISSING); }
  if (!dirSt.isDirectory()) throw new Fail(E.CRED_DIR_PERMS);
  if (dirSt.uid !== process.getuid()) throw new Fail(E.CRED_DIR_PERMS);
  if ((dirSt.mode & 0o077) !== 0) throw new Fail(E.CRED_DIR_PERMS); // no group/other

  let pre;
  try { pre = lstatSync(CRED_PATH); } catch { throw new Fail(E.CRED_MISSING); }
  if (pre.isSymbolicLink()) throw new Fail(E.CRED_SYMLINK);

  let fd;
  try {
    fd = openSync(CRED_PATH, FS.O_RDONLY | FS.O_NOFOLLOW);
  } catch (err) {
    if (err && err.code === 'ELOOP') throw new Fail(E.CRED_SYMLINK);
    throw new Fail(E.CRED_MISSING);
  }

  try {
    const st = fstatSync(fd);
    if (!st.isFile()) throw new Fail(E.CRED_NOT_REGULAR);
    if (st.uid !== process.getuid()) throw new Fail(E.CRED_OWNER);
    if ((st.mode & 0o077) !== 0) throw new Fail(E.CRED_PERMS);
    if (st.nlink !== 1) throw new Fail(E.CRED_LINKS);
    if (st.size === 0 || st.size > MAX_CRED_BYTES) throw new Fail(E.CRED_SIZE);
    // The file we opened must be the file we lstat'd.
    if (st.ino !== pre.ino || st.dev !== pre.dev) throw new Fail(E.CRED_RACE);

    const buf = Buffer.allocUnsafe(st.size);
    const n = readSync(fd, buf, 0, st.size, 0);
    if (n !== st.size) throw new Fail(E.CRED_SIZE);
    return parseCredentials(buf.toString('utf8'));
  } finally {
    try { closeSync(fd); } catch { /* fd cleanup only */ }
  }
}

// Exactly two keys. Duplicates, extras, empty and oversize all rejected.
function parseCredentials(text) {
  const want = new Set(['INFISICAL_CLIENT_ID', 'INFISICAL_CLIENT_SECRET']);
  const got = new Map();
  for (const rawLine of text.split('\n')) {
    const line = rawLine.trim();
    if (line === '' || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq <= 0) throw new Fail(E.CRED_PARSE);
    const key = line.slice(0, eq).trim();
    const val = line.slice(eq + 1).trim();
    if (!want.has(key)) throw new Fail(E.CRED_PARSE);       // extras rejected
    if (got.has(key)) throw new Fail(E.CRED_PARSE);         // duplicates rejected
    if (val.length === 0 || val.length > MAX_VALUE_LEN) throw new Fail(E.CRED_PARSE);
    got.set(key, val);
  }
  if (got.size !== want.size) throw new Fail(E.CRED_PARSE);
  return { clientId: got.get('INFISICAL_CLIENT_ID'), clientSecret: got.get('INFISICAL_CLIENT_SECRET') };
}

// ─── Pure response handling. Extracted so every failure mode is testable
// WITHOUT standing up a server and WITHOUT exposing a generic HTTP client:
// neither function takes a URL, method or header.
function classifyStatus(status) {
  if (status >= 300 && status < 400) throw new Fail(E.REDIRECT_REFUSED);
  if (status === 401 || status === 403) throw new Fail(E.AUTH_REJECTED);
  if (status !== 200) throw new Fail(E.UPSTREAM_STATUS);
  return true;
}

function parseBounded(text) {
  if (typeof text !== 'string') throw new Fail(E.BAD_SHAPE);
  if (text.length > MAX_BODY_BYTES) throw new Fail(E.BODY_TOO_LARGE);
  try { return JSON.parse(text); } catch { throw new Fail(E.BAD_SHAPE); }
}

// ─── Network: global fetch only. Node's fetch ignores HTTP(S)_PROXY unless a
// dispatcher opts in, and we install none — so there is no proxy path. No
// retries and no fallback URL exist anywhere in this file.
async function call(ep, { query, body, token }) {
  let url = ORIGIN + ep.path;
  if (query) {
    const qs = new URLSearchParams(query).toString();
    url += '?' + qs;
  }
  const headers = { accept: 'application/json' };
  if (token) headers.authorization = `Bearer ${token}`;
  if (body) headers['content-type'] = 'application/json';

  let res;
  try {
    res = await fetch(url, {
      method: ep.method,
      headers,
      body: body ? JSON.stringify(body) : undefined,
      redirect: 'manual',
      signal: AbortSignal.timeout(OVERALL_TIMEOUT_MS),
    });
  } catch {
    // Never surface the underlying error object.
    throw new Fail(E.NETWORK);
  }

  classifyStatus(res.status);

  const len = Number(res.headers.get('content-length') ?? '0');
  if (Number.isFinite(len) && len > MAX_BODY_BYTES) throw new Fail(E.BODY_TOO_LARGE);

  let text;
  try { text = await res.text(); } catch { throw new Fail(E.NETWORK); }
  return parseBounded(text);
  // `text` (which may carry secretValue) goes out of scope here and is never
  // returned, logged, or attached to an error.
}

// ─── Projection helpers: validate shape, then copy ONLY safe fields. ────────
function safeStr(v) {
  if (typeof v !== 'string') throw new Fail(E.BAD_SHAPE);
  if (v.length > MAX_STR) throw new Fail(E.BAD_SHAPE);
  // Escape C0 controls (incl. newline, CR, ESC), DEL and C1 (0x80-0x9F) so a
  // hostile name cannot forge receipt structure or drive the operator's
  // terminal. Explicit code-point checks, never a regex holding literal
  // control bytes.
  let out = '';
  for (const ch of v) {
    const cp = ch.codePointAt(0);
    if (cp < 0x20 || cp === 0x7f || (cp >= 0x80 && cp <= 0x9f)) {
      out += '\\x' + cp.toString(16).padStart(2, '0');
    } else {
      out += ch;
    }
  }
  return out;
}

function boundedArray(v) {
  if (!Array.isArray(v)) throw new Fail(E.BAD_SHAPE);
  if (v.length > MAX_ITEMS) throw new Fail(E.BAD_SHAPE);
  return v;
}

// Pure projections — the leak-critical code paths, callable in tests.
// projectSecretNames is the one that matters: its INPUT contains secretValue,
// its OUTPUT must contain only key names.
function projectSecretNames(secretsBody) {
  const out = [];
  for (const sec of boundedArray(secretsBody?.secrets ?? [])) {
    if (!sec || typeof sec !== 'object') throw new Fail(E.BAD_SHAPE);
    out.push(safeStr(sec.secretKey));
  }
  return out;
}

function projectWorkspaceMeta(ws) {
  if (!ws || typeof ws !== 'object') throw new Fail(E.BAD_SHAPE);
  return {
    id: safeStr(ws.id ?? ws._id),
    name: safeStr(ws.name),
    slug: safeStr(ws.slug ?? ''),
    environmentSlugs: boundedArray(ws.environments ?? []).map((e) => {
      if (!e || typeof e !== 'object') throw new Fail(E.BAD_SHAPE);
      return safeStr(e.slug);
    }),
  };
}

// ─── The one action. ────────────────────────────────────────────────────────
async function listMetadata() {
  const { clientId, clientSecret } = readCredentials();

  const auth = await call(EP_LOGIN, { body: { clientId, clientSecret } });
  if (!auth || typeof auth.accessToken !== 'string' || auth.accessToken.length === 0) {
    throw new Fail(E.BAD_SHAPE);
  }
  const token = auth.accessToken; // secret: memory only, never emitted

  const wsBody = await call(EP_WORKSPACES, { token });
  const workspaces = boundedArray(wsBody?.workspaces ?? wsBody);

  const projects = [];
  let secretNameTotal = 0;

  for (const ws of workspaces) {
    if (!ws || typeof ws !== 'object') throw new Fail(E.BAD_SHAPE);
    const id = safeStr(ws.id ?? ws._id);
    const project = { id, name: safeStr(ws.name), slug: safeStr(ws.slug ?? ''), environments: [] };

    for (const env of boundedArray(ws.environments ?? [])) {
      if (!env || typeof env !== 'object') throw new Fail(E.BAD_SHAPE);
      const envSlug = safeStr(env.slug);
      const entry = { slug: envSlug, name: safeStr(env.name ?? env.slug), secretNames: [] };

      const sBody = await call(EP_SECRETS, {
        token,
        query: { workspaceId: id, environment: envSlug, secretPath: '/' },
      });
      // ONLY key names are projected. secretValue is never touched.
      entry.secretNames = projectSecretNames(sBody);
      secretNameTotal += entry.secretNames.length;
      entry.secretCount = entry.secretNames.length;
      project.environments.push(entry);
    }
    projects.push(project);
  }

  return {
    ok: true,
    action: ONLY_ACTION,
    host: `${HOST}:${PORT}`,
    identity: 'peppy-admin (logical reference; no value read)',
    projectCount: projects.length,
    secretNameCount: secretNameTotal,
    projects,
    valueBoundary:
      'Infisical returned value-bearing responses; zero values were emitted, ' +
      'logged, persisted, fingerprinted, or placed in model-visible output.',
  };
}

// ─── Entry point. Action is validated BEFORE any file or network access. ────
async function main() {
  const args = process.argv.slice(2);
  // Exactly one argument, byte-exact. Case mutations and extras are rejected.
  if (args.length !== 1 || args[0] !== ONLY_ACTION) {
    process.stdout.write(JSON.stringify({ ok: false, error: E.BAD_ACTION }) + '\n');
    process.exitCode = 2;
    return;
  }

  try {
    const receipt = await listMetadata();
    process.stdout.write(JSON.stringify(receipt) + '\n');
  } catch (err) {
    // Only our own stable code is ever emitted. Never the caught object.
    const code = err instanceof Fail ? err.code : 'E_INTERNAL';
    process.stdout.write(JSON.stringify({ ok: false, error: code }) + '\n');
    process.exitCode = 1;
  }
}

const isDirectRun =
  process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isDirectRun) main();

export {
  parseCredentials, safeStr, readCredentials, classifyStatus, parseBounded,
  projectSecretNames, projectWorkspaceMeta, E, Fail, ONLY_ACTION, CRED_PATH, CRED_DIR,
};
