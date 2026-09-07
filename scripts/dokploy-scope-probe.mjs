#!/usr/bin/env node
/**
 * dokploy-scope-probe — ONE read-only check: does the existing
 * DOKPLOY_TOKEN_INCLUIR_XEROX carry the Quiz organization on the XEROX
 * Dokploy installation?
 *
 * Bead: aperture-ztid5. Design: Cipher (approved), implementation gated on his
 * exact-code review plus a fresh execution dispatch. NOTHING here has been run
 * against a live host.
 *
 * WHAT THIS IS
 *   A single fixed-purpose, NO-ARGUMENT action. It fetches exactly one pinned
 *   secret, uses it transiently as an API key over a protected local socket,
 *   makes exactly one read-only call, and prints one constant.
 *
 * WHAT THIS IS NOT
 *   Not a general helper. Not a credential mover — nothing is written to disk.
 *   Not a Dokploy client: `project.one` for one pinned project id is the only
 *   endpoint reachable, and no mutation method exists in this file.
 *
 * WHY THE SSH FORWARD (Cipher, and it matters)
 *   xerox Dokploy is also reachable on a PUBLIC PLAIN-HTTP port. Sending an API
 *   key there would put the credential on the wire in clear text across the
 *   internet. This connects ONLY through an authenticated SSH local
 *   Unix-socket forward to the host's own 127.0.0.1:3000, so the key never
 *   leaves an encrypted channel and never touches the public endpoint.
 *
 * VALUE BOUNDARY / ZEROIZATION — stated, not implied
 *   The secret exists transiently in this process's memory only. It is never
 *   written to disk, never placed in argv or environment, never logged, never
 *   fingerprinted, and never attached to an error. Node cannot guarantee
 *   zeroization: the value may persist in the allocator or OS memory after the
 *   reference is dropped, and this process cannot scrub it. The process exits
 *   immediately after the single check.
 */

import { openSync, fstatSync, lstatSync, readSync, closeSync, mkdtempSync,
         rmSync, constants as FS } from 'node:fs';
import { userInfo, tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import http from 'node:http';

// ─── COMPILED CONSTANTS. Nothing here is caller-supplied. ─────────────────
const ACCOUNT = userInfo();
const CRED_DIR = join(ACCOUNT.homedir, '.config', 'aperture');
const CRED_PATH = join(CRED_DIR, 'infisical-peppy-admin.env');

const INFISICAL_HOST = '100.102.73.112';
const INFISICAL_PORT = 3005;
const P_LOGIN = '/api/v1/auth/universal-auth/login';
// Legacy single-secret retrieve. Pinned name, workspace, environment, path.
const SECRET_NAME = 'DOKPLOY_TOKEN_INCLUIR_XEROX';
const WORKSPACE_ID = 'b4a65c24-dd50-4e93-b323-41d472e7cf46';
const ENVIRONMENT = 'prod';
const SECRET_PATH = '/';

// xerox Dokploy, reached ONLY over the SSH forward. Never the public endpoint.
const SSH_BIN = '/usr/bin/ssh';
const SSH_TARGET = 'xerox';
const REMOTE_ADDR = '127.0.0.1:3000';
const QUIZ_PROJECT_ID = 'w4FraIVPC0PfP2fZxVtaT';
const QUIZ_ORG_ID = 'GME9CAd599FWcInMNTZ2F';
const P_PROJECT_ONE = '/api/project.one';

const OK_RECEIPT = 'XEROX_QUIZ_SCOPE_CONFIRMED';

const MAX_CRED_BYTES = 4096;
const MAX_CRED_VALUE_LEN = 512;
const MAX_BODY_BYTES = 1_000_000;
const MAX_SECRET_LEN = 4096;
const REQUEST_TIMEOUT_MS = 10_000;
const ACTION_DEADLINE_MS = 60_000;
const FORWARD_READY_MS = 15_000;

class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}
const E = {
  BAD_ACTION: 'E_BAD_ACTION',
  UNSAFE_RUNTIME: 'E_UNSAFE_RUNTIME',
  CRED_MISSING: 'E_CRED_MISSING',
  CRED_PERMS: 'E_CRED_PERMS',
  CRED_PARSE: 'E_CRED_PARSE',
  CRED_ENCODING: 'E_CRED_ENCODING',
  NETWORK: 'E_NETWORK',
  AUTH_REJECTED: 'E_AUTH_REJECTED',
  REDIRECT_REFUSED: 'E_REDIRECT_REFUSED',
  UPSTREAM_STATUS: 'E_UPSTREAM_STATUS',
  BODY_TOO_LARGE: 'E_BODY_TOO_LARGE',
  BAD_ENCODING: 'E_BAD_ENCODING',
  BAD_SHAPE: 'E_BAD_SHAPE',
  SECRET_INVALID: 'E_SECRET_INVALID',
  FORWARD_FAILED: 'E_FORWARD_FAILED',
  PROJECT_MISMATCH: 'E_PROJECT_MISMATCH',
  ORG_MISMATCH: 'E_ORG_MISMATCH',
  DEADLINE: 'E_DEADLINE',
  INTERNAL: 'E_INTERNAL',
};

// ─── Runtime refusal, before any secret is read. Detects unsafe invocation;
// does NOT defeat code already imported.
// Pure classifier so every case is testable without mutating this process.
// Note this refuses a non-empty execArgv, which means the probe legitimately
// refuses to run under `node --test` — the guard is doing its job.
function classifyRuntime({ env, execArgv, globalAgentIsStock }) {
  const e = env ?? {};
  if (String(e.NODE_OPTIONS ?? '') !== '') throw new Fail(E.UNSAFE_RUNTIME);
  if (Array.isArray(execArgv) && execArgv.length > 0) throw new Fail(E.UNSAFE_RUNTIME);
  if (e.NODE_DEBUG) throw new Fail(E.UNSAFE_RUNTIME);
  if (e.NODE_DEBUG_NATIVE) throw new Fail(E.UNSAFE_RUNTIME);
  if (e.NODE_USE_ENV_PROXY) throw new Fail(E.UNSAFE_RUNTIME);
  if (globalAgentIsStock === false) throw new Fail(E.UNSAFE_RUNTIME);
  return true;
}

function assertSafeRuntime() {
  return classifyRuntime({
    env: process.env,
    execArgv: process.execArgv,
    globalAgentIsStock: !http.globalAgent || http.globalAgent.constructor === http.Agent,
  });
}

// ─── Credential file. Same guards as the approved helper; its reader is
// deliberately NOT exported there, so the checks are mirrored, not imported.
function readCredentials() {
  let dirSt;
  try { dirSt = lstatSync(CRED_DIR); } catch { throw new Fail(E.CRED_MISSING); }
  if (!dirSt.isDirectory() || dirSt.uid !== ACCOUNT.uid || (dirSt.mode & 0o077)) {
    throw new Fail(E.CRED_PERMS);
  }
  let pre;
  try { pre = lstatSync(CRED_PATH); } catch { throw new Fail(E.CRED_MISSING); }
  if (pre.isSymbolicLink()) throw new Fail(E.CRED_PERMS);

  let fd;
  try { fd = openSync(CRED_PATH, FS.O_RDONLY | FS.O_NOFOLLOW); }
  catch { throw new Fail(E.CRED_MISSING); }
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077)
        || st.nlink !== 1 || st.size === 0 || st.size > MAX_CRED_BYTES) {
      throw new Fail(E.CRED_PERMS);
    }
    if (st.ino !== pre.ino || st.dev !== pre.dev) throw new Fail(E.CRED_PERMS);
    const buf = Buffer.allocUnsafe(st.size);
    if (readSync(fd, buf, 0, st.size, 0) !== st.size) throw new Fail(E.CRED_PERMS);
    let text;
    try { text = new TextDecoder('utf-8', { fatal: true }).decode(buf); }
    catch { throw new Fail(E.CRED_ENCODING); }
    return parseCredentials(text);
  } finally {
    try { closeSync(fd); } catch { /* cleanup only */ }
  }
}

function parseCredentials(text) {
  const want = ['INFISICAL_CLIENT_ID', 'INFISICAL_CLIENT_SECRET'];
  const got = new Map();
  for (const raw of String(text).split('\n')) {
    const line = raw.trim();
    if (line === '' || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq <= 0) throw new Fail(E.CRED_PARSE);
    const key = line.slice(0, eq).trim();
    const val = line.slice(eq + 1).trim();
    if (!want.includes(key) || got.has(key)) throw new Fail(E.CRED_PARSE);
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

function consumeBoundedStream(readable, maxBytes) {
  return new Promise((resolve, reject) => {
    const chunks = []; let total = 0; let settled = false;
    const fail = (f) => {
      if (settled) return; settled = true;
      try { readable.destroy(); } catch { /* best effort */ }
      reject(f);
    };
    readable.on('data', (c) => {
      if (settled) return;
      total += c.length;
      if (total > maxBytes) { fail(new Fail(E.BODY_TOO_LARGE)); return; }
      chunks.push(c);
    });
    readable.on('error', () => fail(new Fail(E.NETWORK)));
    readable.on('end', () => {
      if (settled) return; settled = true;
      try { resolve(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks))); }
      catch { reject(new Fail(E.BAD_ENCODING)); }
    });
  });
}

// Absolute wall-clock bound. node:http `timeout` is socket INACTIVITY only, so
// a drip response would never trip it. Settles once; timer always cleared.
function withAbsoluteDeadline({ start, abort, ms }) {
  return new Promise((resolve, reject) => {
    let settled = false; let timer = null;
    const settle = (fn, arg) => {
      if (settled) return; settled = true;
      if (timer) { clearTimeout(timer); timer = null; }
      fn(arg);
    };
    timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      if (timer) { clearTimeout(timer); timer = null; }
      try { if (abort) abort(); } catch { /* must not mask the deadline */ }
      reject(new Fail(E.DEADLINE));
    }, ms);
    try { start((v) => settle(resolve, v), (e) => settle(reject, e)); }
    catch (e) { settle(reject, e); }
  });
}

function makeBudget() { return { startedAt: Date.now() }; }
function remainingMs(b) { return ACTION_DEADLINE_MS - (Date.now() - b.startedAt); }
function charge(b) {
  const left = remainingMs(b);
  if (left <= 0) throw new Fail(E.DEADLINE);
  return Math.min(REQUEST_TIMEOUT_MS, left);
}

// ─── One request primitive. `where` selects a PINNED destination; it is not a
// URL and cannot address an arbitrary host.
function request({ where, method, path, headers, body, timeoutMs, socketPath }) {
  let active = null;
  return withAbsoluteDeadline({
    ms: timeoutMs,
    abort: () => { try { if (active) active.destroy(); } catch { /* best effort */ } },
    start: (ok, bad) => {
      const payload = body ? Buffer.from(JSON.stringify(body), 'utf8') : null;
      const hdrs = { accept: 'application/json', ...headers };
      if (payload) {
        hdrs['content-type'] = 'application/json';
        hdrs['content-length'] = String(payload.length);
      }
      const opts = socketPath
        ? { socketPath, method, path, headers: hdrs, timeout: timeoutMs }
        : { host: where.host, port: where.port, method, path, headers: hdrs, timeout: timeoutMs };
      const req = http.request(opts, (res) => {
        try { classifyStatus(res.statusCode); }
        catch (e) { res.destroy(); bad(e); return; }
        consumeBoundedStream(res, MAX_BODY_BYTES).then(
          (text) => { try { ok(parseBounded(text)); } catch (e) { bad(e); } }, bad);
      });
      active = req;
      req.on('timeout', () => { req.destroy(); bad(new Fail(E.NETWORK)); });
      req.on('error', () => bad(new Fail(E.NETWORK)));
      if (payload) req.write(payload);
      req.end();
    },
  });
}

// ─── SSH local Unix-socket forward to xerox's OWN 127.0.0.1:3000.
// The alias `xerox` resolves to the TAILNET address, so this never touches the
// public plain-HTTP endpoint. Strict host-key checking is REQUIRED, not
// relaxed: the host key is already known, so it verifies rather than prompts.
// The secret is never in this child's argv, env or stdin.
const SSH_ARGS = [
  '-N',
  '-o', 'BatchMode=yes',
  '-o', 'StrictHostKeyChecking=yes',
  '-o', 'PasswordAuthentication=no',
  '-o', 'KbdInteractiveAuthentication=no',
  '-o', 'PermitLocalCommand=no',
  '-o', 'ClearAllForwardings=yes',
  '-o', 'ExitOnForwardFailure=yes',
];

function startForward() {
  // Owned 0700 directory for the socket; removed on every exit path.
  const dir = mkdtempSync(join(tmpdir(), 'dkscope-'));
  const sock = join(dir, 's');
  const child = spawn(SSH_BIN,
    [...SSH_ARGS, '-L', `${sock}:${REMOTE_ADDR}`, SSH_TARGET],
    // Empty env: nothing of ours, and certainly no secret, reaches the child.
    { stdio: ['ignore', 'ignore', 'ignore'], env: {} });

  const cleanup = () => {
    try { child.kill('SIGTERM'); } catch { /* best effort */ }
    try { rmSync(dir, { recursive: true, force: true }); } catch { /* best effort */ }
  };

  const ready = new Promise((resolve, reject) => {
    let done = false;
    const finish = (fn, arg) => { if (done) return; done = true; fn(arg); };
    child.on('error', () => finish(reject, new Fail(E.FORWARD_FAILED)));
    child.on('exit', () => finish(reject, new Fail(E.FORWARD_FAILED)));
    const started = Date.now();
    const poll = () => {
      if (done) return;
      if (Date.now() - started > FORWARD_READY_MS) { finish(reject, new Fail(E.FORWARD_FAILED)); return; }
      const probe = http.request({ socketPath: sock, method: 'GET', path: '/', timeout: 1000 }, (res) => {
        res.destroy(); finish(resolve, sock);
      });
      probe.on('error', () => setTimeout(poll, 250));
      probe.on('timeout', () => { probe.destroy(); setTimeout(poll, 250); });
      probe.end();
    };
    poll();
  });

  return { ready, cleanup, sock };
}

// ─── Fetch EXACTLY ONE pinned secret. No list, no search, no fallback, no
// import expansion, no reference expansion.
async function fetchPinnedSecret(budget) {
  const { clientId, clientSecret } = readCredentials();
  const auth = await request({
    where: { host: INFISICAL_HOST, port: INFISICAL_PORT },
    method: 'POST', path: P_LOGIN,
    body: { clientId, clientSecret }, timeoutMs: charge(budget),
  });
  if (!auth || typeof auth.accessToken !== 'string' || auth.accessToken.length === 0) {
    throw new Fail(E.BAD_SHAPE);
  }
  const q = new URLSearchParams({
    workspaceId: WORKSPACE_ID,
    environment: ENVIRONMENT,
    secretPath: SECRET_PATH,
    type: 'shared',
    expandSecretReferences: 'false',
    include_imports: 'false',
  }).toString();
  const body = await request({
    where: { host: INFISICAL_HOST, port: INFISICAL_PORT },
    method: 'GET',
    path: `/api/v3/secrets/raw/${SECRET_NAME}?${q}`,
    headers: { authorization: `Bearer ${auth.accessToken}` },
    timeoutMs: charge(budget),
  });
  return extractSecretValue(body);
}

// Pure: validates the envelope and returns the value. Never logs it.
function extractSecretValue(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  const sec = body.secret;
  if (!sec || typeof sec !== 'object') throw new Fail(E.BAD_SHAPE);
  if (sec.secretKey !== SECRET_NAME) throw new Fail(E.BAD_SHAPE);
  const v = sec.secretValue;
  if (typeof v !== 'string' || v.length === 0 || v.length > MAX_SECRET_LEN) {
    throw new Fail(E.SECRET_INVALID);
  }
  for (const ch of v) {
    const cp = ch.codePointAt(0);
    if (cp < 0x20 || cp === 0x7f) throw new Fail(E.SECRET_INVALID);
  }
  return v;
}

// Pure: the only thing we assert about the Dokploy response.
function assertQuizScope(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.projectId !== QUIZ_PROJECT_ID) throw new Fail(E.PROJECT_MISMATCH);
  if (body.organizationId !== QUIZ_ORG_ID) throw new Fail(E.ORG_MISMATCH);
  return true;
}

// ─── The single action. PRIVATE — importing this module cannot run it.
async function probe() {
  assertSafeRuntime();
  const budget = makeBudget();
  const token = await fetchPinnedSecret(budget);
  const fwd = startForward();
  try {
    const sock = await fwd.ready;
    const body = await request({
      socketPath: sock, method: 'GET',
      path: `${P_PROJECT_ONE}?projectId=${QUIZ_PROJECT_ID}`,
      headers: { 'x-api-key': token },
      timeoutMs: charge(budget),
    });
    assertQuizScope(body);
  } finally {
    fwd.cleanup();
  }
  return OK_RECEIPT;
}

async function main() {
  // No-argument action: any argument at all is refused before anything runs.
  if (process.argv.slice(2).length !== 0) {
    process.stdout.write(E.BAD_ACTION + '\n');
    process.exitCode = 2;
    return;
  }
  try {
    process.stdout.write((await probe()) + '\n');
  } catch (err) {
    // Only our stable code. Never the caught object, never an upstream body.
    process.stdout.write((err instanceof Fail ? err.code : E.INTERNAL) + '\n');
    process.exitCode = 1;
  }
}

const isDirectRun = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isDirectRun) main();

export {
  parseCredentials, classifyStatus, parseBounded, consumeBoundedStream,
  withAbsoluteDeadline, makeBudget, remainingMs, charge,
  E, Fail, OK_RECEIPT, QUIZ_PROJECT_ID, QUIZ_ORG_ID,
  SECRET_NAME, WORKSPACE_ID, ENVIRONMENT, SECRET_PATH, P_PROJECT_ONE,
  extractSecretValue, assertQuizScope, assertSafeRuntime, classifyRuntime, SSH_ARGS,
};
