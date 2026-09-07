#!/usr/bin/env node
/**
 * infisical-metadata — fixed-purpose, non-model Infisical METADATA reader.
 *
 * Bead: aperture-a4ph5. Design + code gate: Cipher (aperture-5nxd8).
 * Revision 2 applies Cipher's exact-code verdict on e8f2e14.
 *
 * WHAT THIS IS
 *   Authenticates with the EXISTING banked `peppy-admin` Universal Auth machine
 *   identity and emits ONLY project / environment / secret-KEY-NAME metadata as
 *   a single bounded JSON receipt.
 *
 * WHAT THIS IS NOT
 *   Not a secret reader. Not a generic HTTP client. Not an injector. There is no
 *   `inject` action, not even a stub. The credential reader, the credential path
 *   and the live action are deliberately NOT exported: importing this module
 *   must not hand a caller a plaintext credential reader or a live caller.
 *
 * THE VALUE BOUNDARY (stated honestly)
 *   Infisical v0.146 list-secrets responses DO contain `secretValue`. Values and
 *   the Bearer token exist transiently inside THIS process. They are never
 *   emitted, logged, persisted, fingerprinted, or placed in model-visible
 *   output. The receipt says exactly that — it does NOT claim none were
 *   retrieved, which would be false.
 *
 * TRANSPORT
 *   node:http with a literal hostname/port/path. node:http carries no proxy
 *   semantics, so ambient proxy configuration cannot relay the token — unlike
 *   global fetch, which on Node 24 can honour ambient proxy settings via
 *   NODE_OPTIONS=--use-env-proxy. A defensive guard also rejects that runtime
 *   state before the credential is read.
 */

import { openSync, fstatSync, lstatSync, readSync, closeSync, constants as FS } from 'node:fs';
import { userInfo } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import http from 'node:http';

// ─── Fixed configuration. Nothing here is caller-supplied. ──────────────────
// Cipher HIGH-1: os.homedir() follows ambient HOME. Use the OS account home.
const ACCOUNT = userInfo();
const CRED_DIR = join(ACCOUNT.homedir, '.config', 'aperture');
const CRED_PATH = join(CRED_DIR, 'infisical-peppy-admin.env');

const HOST = '100.102.73.112';
const PORT = 3005;

const P_LOGIN = '/api/v1/auth/universal-auth/login';
const P_WORKSPACES = '/api/v1/workspace';
const P_SECRETS = '/api/v3/secrets/raw';

const ONLY_ACTION = 'list-metadata';

// Cipher MEDIUM-6: global ceilings, not per-collection caps that multiply out.
const MAX_CRED_BYTES = 4096;
const MAX_CRED_VALUE_LEN = 512;
const MAX_BODY_BYTES = 1000000;      // per response, enforced while streaming
const MAX_PROJECTS = 50;
const MAX_ENVS_PER_PROJECT = 20;
const MAX_REQUESTS = 120;            // hard ceiling on total HTTP calls
const MAX_NAMES_TOTAL = 5000;        // across the whole action
const MAX_STR = 256;
const MAX_RECEIPT_BYTES = 512000;    // serialized receipt ceiling
const REQUEST_TIMEOUT_MS = 10000;
const ACTION_DEADLINE_MS = 60000;    // one deadline for the whole action

// ─── Stable error codes. No upstream detail ever rides along. ───────────────
class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}
const E = {
  BAD_ACTION: 'E_BAD_ACTION',
  UNSAFE_RUNTIME: 'E_UNSAFE_RUNTIME',
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
  BAD_ENCODING: 'E_BAD_ENCODING',
  BAD_SHAPE: 'E_BAD_SHAPE',
  LIMIT_EXCEEDED: 'E_LIMIT_EXCEEDED',
  DEADLINE: 'E_DEADLINE',
  RECEIPT_TOO_LARGE: 'E_RECEIPT_TOO_LARGE',
};

// ─── Cipher HIGH-2: reject proxy-enabling runtime state before any secret read.
function assertSafeRuntime() {
  const opts = String(process.env.NODE_OPTIONS ?? '');
  if (/--use-env-proxy\b/.test(opts)) throw new Fail(E.UNSAFE_RUNTIME);
  if (process.env.NODE_USE_ENV_PROXY) throw new Fail(E.UNSAFE_RUNTIME);
  if (http.globalAgent && http.globalAgent.constructor !== http.Agent) {
    throw new Fail(E.UNSAFE_RUNTIME);
  }
}

// ─── Credential file: open once, validate the SAME descriptor. PRIVATE. ─────
function readCredentials() {
  let dirSt;
  try { dirSt = lstatSync(CRED_DIR); } catch { throw new Fail(E.CRED_MISSING); }
  if (!dirSt.isDirectory()) throw new Fail(E.CRED_DIR_PERMS);
  if (dirSt.uid !== ACCOUNT.uid) throw new Fail(E.CRED_DIR_PERMS);
  if ((dirSt.mode & 0o077) !== 0) throw new Fail(E.CRED_DIR_PERMS);

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
    if (st.uid !== ACCOUNT.uid) throw new Fail(E.CRED_OWNER);
    if ((st.mode & 0o077) !== 0) throw new Fail(E.CRED_PERMS);
    if (st.nlink !== 1) throw new Fail(E.CRED_LINKS);
    if (st.size === 0 || st.size > MAX_CRED_BYTES) throw new Fail(E.CRED_SIZE);
    if (st.ino !== pre.ino || st.dev !== pre.dev) throw new Fail(E.CRED_RACE);

    const buf = Buffer.allocUnsafe(st.size);
    const n = readSync(fd, buf, 0, st.size, 0);
    if (n !== st.size) throw new Fail(E.CRED_SIZE);
    return parseCredentials(buf.toString('utf8'));
  } finally {
    try { closeSync(fd); } catch { /* fd cleanup only */ }
  }
}

// Pure. Exported for tests: takes text, never touches the filesystem.
function parseCredentials(text) {
  const want = ['INFISICAL_CLIENT_ID', 'INFISICAL_CLIENT_SECRET'];
  const got = new Map();
  for (const rawLine of String(text).split('\n')) {
    const line = rawLine.trim();
    if (line === '' || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq <= 0) throw new Fail(E.CRED_PARSE);
    const key = line.slice(0, eq).trim();
    const val = line.slice(eq + 1).trim();
    if (!want.includes(key)) throw new Fail(E.CRED_PARSE);
    if (got.has(key)) throw new Fail(E.CRED_PARSE);
    if (val.length === 0 || val.length > MAX_CRED_VALUE_LEN) throw new Fail(E.CRED_PARSE);
    got.set(key, val);
  }
  if (got.size !== want.length) throw new Fail(E.CRED_PARSE);
  return { clientId: got.get(want[0]), clientSecret: got.get(want[1]) };
}

// ─── Pure response handling. No URL, method or header is caller-controllable.
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

// Cipher LOW-MEDIUM-8: C0/C1/DEL plus Unicode line/paragraph separators and
// bidi/format controls, any of which can visually forge or reorder a one-line
// receipt even though they are not C0.
function isForbiddenCp(cp) {
  if (cp < 0x20 || cp === 0x7f) return true;              // C0 + DEL
  if (cp >= 0x80 && cp <= 0x9f) return true;              // C1
  if (cp === 0x2028 || cp === 0x2029) return true;        // LS, PS
  if (cp >= 0x200b && cp <= 0x200f) return true;          // ZWSP..RLM
  if (cp >= 0x202a && cp <= 0x202e) return true;          // bidi embedding
  if (cp >= 0x2066 && cp <= 0x2069) return true;          // bidi isolates
  if (cp === 0x061c) return true;                         // ALM
  if (cp === 0xfeff) return true;                         // BOM / ZWNBSP
  return false;
}

function safeStr(v) {
  if (typeof v !== 'string') throw new Fail(E.BAD_SHAPE);
  if (v.length === 0 || v.length > MAX_STR) throw new Fail(E.BAD_SHAPE);
  let out = '';
  for (const ch of v) {
    const cp = ch.codePointAt(0);
    out += isForbiddenCp(cp) ? '\\u' + cp.toString(16).padStart(4, '0') : ch;
  }
  return out;
}

// Cipher MEDIUM-7: exact envelopes. A missing array is a shape error, never an
// empty result — empty metadata must not be a silent fallback.
function requireArray(obj, field, max) {
  if (!obj || typeof obj !== 'object') throw new Fail(E.BAD_SHAPE);
  const v = obj[field];
  if (!Array.isArray(v)) throw new Fail(E.BAD_SHAPE);
  if (v.length > max) throw new Fail(E.LIMIT_EXCEEDED);
  return v;
}

function projectSecretNames(secretsBody) {
  const out = [];
  for (const sec of requireArray(secretsBody, 'secrets', MAX_NAMES_TOTAL)) {
    if (!sec || typeof sec !== 'object') throw new Fail(E.BAD_SHAPE);
    // ONLY the key name is projected; the value field is never touched.
    out.push(safeStr(sec.secretKey));
  }
  return out;
}

function projectWorkspaceMeta(ws) {
  if (!ws || typeof ws !== 'object') throw new Fail(E.BAD_SHAPE);
  const id = Object.prototype.hasOwnProperty.call(ws, 'id') ? ws.id : ws._id;
  return {
    id: safeStr(id),
    name: safeStr(ws.name),
    slug: safeStr(ws.slug),
    environmentSlugs: requireArray(ws, 'environments', MAX_ENVS_PER_PROJECT).map((e) => {
      if (!e || typeof e !== 'object') throw new Fail(E.BAD_SHAPE);
      return safeStr(e.slug);
    }),
  };
}

// ─── Budget: one action deadline plus global request/name ceilings. ─────────
function makeBudget() { return { requests: 0, names: 0, startedAt: Date.now() }; }

function chargeRequest(budget) {
  if (Date.now() - budget.startedAt > ACTION_DEADLINE_MS) throw new Fail(E.DEADLINE);
  budget.requests += 1;
  if (budget.requests > MAX_REQUESTS) throw new Fail(E.LIMIT_EXCEEDED);
}

// ─── Transport: node:http, literal host/port/path, streamed with a byte cap. ─
function request({ method, path, token, body }) {
  return new Promise((resolve, reject) => {
    const payload = body ? Buffer.from(JSON.stringify(body), 'utf8') : null;
    const headers = { accept: 'application/json' };
    if (token) headers.authorization = 'Bearer ' + token;
    if (payload) {
      headers['content-type'] = 'application/json';
      headers['content-length'] = String(payload.length);
    }

    const req = http.request(
      { host: HOST, port: PORT, method, path, headers, timeout: REQUEST_TIMEOUT_MS },
      (res) => {
        try { classifyStatus(res.statusCode); }
        catch (e) { res.destroy(); reject(e); return; }

        // Cipher MEDIUM-5: stream and abort above the cap; never buffer first.
        const chunks = [];
        let total = 0;
        res.on('data', (c) => {
          total += c.length;
          if (total > MAX_BODY_BYTES) { res.destroy(); reject(new Fail(E.BODY_TOO_LARGE)); return; }
          chunks.push(c);
        });
        res.on('error', () => reject(new Fail(E.NETWORK)));
        res.on('end', () => {
          let text;
          try {
            // Fatal decode: reject invalid UTF-8 rather than substituting U+FFFD.
            text = new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks));
          } catch { reject(new Fail(E.BAD_ENCODING)); return; }
          try { resolve(parseBounded(text)); } catch (e) { reject(e); }
        });
      },
    );

    req.on('timeout', () => { req.destroy(); reject(new Fail(E.NETWORK)); });
    req.on('error', () => reject(new Fail(E.NETWORK)));   // never surface the cause
    if (payload) req.write(payload);
    req.end();
  });
}

// ─── The one action. PRIVATE — not exported, so importing this module cannot
// trigger a credential read or a live call.
async function listMetadata() {
  assertSafeRuntime();
  const budget = makeBudget();
  const { clientId, clientSecret } = readCredentials();

  chargeRequest(budget);
  const auth = await request({ method: 'POST', path: P_LOGIN, body: { clientId, clientSecret } });
  if (!auth || typeof auth !== 'object' || typeof auth.accessToken !== 'string'
      || auth.accessToken.length === 0) {
    throw new Fail(E.BAD_SHAPE);
  }
  const token = auth.accessToken;   // secret: memory only, never emitted

  chargeRequest(budget);
  const wsBody = await request({ method: 'GET', path: P_WORKSPACES, token });
  const workspaces = requireArray(wsBody, 'workspaces', MAX_PROJECTS);

  const projects = [];
  for (const ws of workspaces) {
    const meta = projectWorkspaceMeta(ws);
    const envs = [];
    for (const slug of meta.environmentSlugs) {
      chargeRequest(budget);
      const q = new URLSearchParams({
        workspaceId: meta.id, environment: slug, secretPath: '/',
      }).toString();
      const sBody = await request({ method: 'GET', path: P_SECRETS + '?' + q, token });
      const names = projectSecretNames(sBody);
      budget.names += names.length;
      if (budget.names > MAX_NAMES_TOTAL) throw new Fail(E.LIMIT_EXCEEDED);
      envs.push({ slug, secretCount: names.length, secretNames: names });
    }
    projects.push({ id: meta.id, name: meta.name, slug: meta.slug, environments: envs });
  }

  return {
    ok: true,
    action: ONLY_ACTION,
    host: HOST + ':' + PORT,
    // Cipher LOW-9: this path returns no verifiable identity id, so this is a
    // LOCATOR, not a verification. The wording says so rather than implying it.
    identityLocator: 'peppy-admin (logical reference; NOT verified from a server field)',
    projectCount: projects.length,
    secretNameCount: budget.names,
    requestCount: budget.requests,
    projects,
    valueBoundary:
      'Infisical returned value-bearing responses; zero values were emitted, '
      + 'logged, persisted, fingerprinted, or placed in model-visible output.',
  };
}

// ─── Entry point. Action validated BEFORE any runtime, file or network access.
async function main() {
  const args = process.argv.slice(2);
  if (args.length !== 1 || args[0] !== ONLY_ACTION) {
    process.stdout.write(JSON.stringify({ ok: false, error: E.BAD_ACTION }) + '\n');
    process.exitCode = 2;
    return;
  }
  try {
    const receipt = await listMetadata();
    const line = JSON.stringify(receipt);
    if (line.length > MAX_RECEIPT_BYTES) throw new Fail(E.RECEIPT_TOO_LARGE);
    process.stdout.write(line + '\n');
  } catch (err) {
    const code = err instanceof Fail ? err.code : 'E_INTERNAL';
    process.stdout.write(JSON.stringify({ ok: false, error: code }) + '\n');
    process.exitCode = 1;
  }
}

const isDirectRun = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isDirectRun) main();

// Cipher HIGH-3: export ONLY pure, non-secret validators. readCredentials,
// CRED_PATH, CRED_DIR, request() and listMetadata() are deliberately private —
// importing this module must not yield a credential reader or a live caller.
export {
  parseCredentials, safeStr, isForbiddenCp, classifyStatus, parseBounded,
  requireArray, projectSecretNames, projectWorkspaceMeta,
  E, Fail, ONLY_ACTION,
};
