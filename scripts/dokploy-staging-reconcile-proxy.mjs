#!/usr/bin/env node
/**
 * One fixed staging reconciliation: preserve the existing Quiz compose env,
 * set only TRUSTED_PROXY_CIDRS=10.0.1.8/32, then deploy that same compose.
 *
 * Bead: aperture-ztid5. Design and code gate: Cipher. NOT RUN against any live
 * host; requires his exact-head PASS plus a GLaDOS execution dispatch.
 *
 * WHAT THIS IS
 *   A single fixed-purpose, NO-ARGUMENT action. Reads one pinned local file,
 *   opens a strict SSH Unix-socket forward, binds the exact project,
 *   environment and compose with two reads, then (only when needed) performs
 *   one exact env update and one deploy. It prints one constant.
 *
 * WHAT THIS IS NOT
 *   No Infisical code of any kind — no auth, no retrieval, no constants. No
 *   list, search, retry, fallback, provisioning, secret generation, domain
 *   change, or caller-selected endpoint. The token file is READ ONLY and never
 *   modified. The only mutations target the one pinned staging compose.
 *
 * WHY THE SSH FORWARD
 *   xerox Dokploy is also reachable on a PUBLIC PLAIN-HTTP port. Sending an API
 *   key there would put it in clear text on the wire. This reaches only the
 *   host's own 127.0.0.1:3000 through an authenticated forward, and the pinned
 *   address is the tailnet one.
 *
 * ZEROIZATION — stated, not implied
 *   The token exists transiently in this process's memory. It is never written,
 *   logged, echoed, fingerprinted, measured, or attached to an error. Node
 *   cannot guarantee zeroization: it may persist in the allocator or OS memory
 *   after the reference drops, and this process cannot scrub it.
 */

import { openSync, fstatSync, lstatSync, readSync, closeSync, mkdirSync, mkdtempSync,
         rmSync, constants as FS } from 'node:fs';
import { userInfo } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import http from 'node:http';

// ─── COMPILED CONSTANTS. No argument, env var, or override reaches any of it.
const ACCOUNT = userInfo();
const HOME = ACCOUNT.homedir;                       // pwd-derived, not $HOME
const TOKEN_PARENT = join(HOME, 'Downloads');
const TOKEN_PATH = join(TOKEN_PARENT, 'secret copy.txt');
const TOKEN_KEY = 'DOKPLOY_TOKEN_INCLUIR_XEROX';

// Temp parent lives under an owned, already-verified directory — NOT
// os.tmpdir(), which follows an ambient TMPDIR and could be redirected.
const TEMP_PARENT = join(HOME, '.config', 'aperture');

// Pinned SSH context. Structurally verified: ubuntu@100.85.254.44 is the
// tailnet address, exactly one known_hosts entry exists, and id_ed25519 is the
// only default private key present.
const SSH_BIN = '/usr/bin/ssh';
const SSH_USER = 'ubuntu';
const SSH_HOST = '100.85.254.44';
const SSH_IDENTITY = join(HOME, '.ssh', 'id_ed25519');
const SSH_KNOWN_HOSTS = join(HOME, '.ssh', 'known_hosts');
const REMOTE_ADDR = '127.0.0.1:3000';

const QUIZ_PROJECT_ID = 'w4FraIVPC0PfP2fZxVtaT';
const QUIZ_ORG_ID = 'GME9CAd599FWcInMNTZ2F';
const STAGING_ENV_ID = 'awjX2FNbBoxgiIqRLnuMN';
const STAGING_ENV_NAME = 'staging';
const COMPOSE_ID = 'YxWNm8CuV70dXA1kt8KM7';
const COMPOSE_APPNAME = 'quiz-incluir-staging-4400a18b9519d0bb-v2vvhj';
const REPO_OWNER = 'FranciscoMateusVG';
const REPO_NAME = 'quiz-incluir';
const INFRA_BRANCH = 'aperture-ztid5-staging';
const COMPOSE_PATH = './docker-compose.staging.yml';
const TRUST_KEY = 'TRUSTED_PROXY_CIDRS';
const TRUST_VALUE = '10.0.1.8/32';
const P_PROJECT_ONE = '/api/project.one';
const P_COMPOSE_ONE = '/api/compose.one';
const P_COMPOSE_UPDATE = '/api/compose.update';
const P_COMPOSE_DEPLOY = '/api/compose.deploy';

const OK_RECEIPT = 'XEROX_QUIZ_PROXY_RECONCILED';

const MAX_TOKEN_FILE_BYTES = 8192;
const MAX_TOKEN_LEN = 4096;
const MAX_BODY_BYTES = 1_000_000;
const MAX_ENV_BYTES = 65_536;
const REQUEST_TIMEOUT_MS = 10_000;
const ACTION_DEADLINE_MS = 60_000;
const SOCKET_READY_MS = 15_000;

class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}
const E = {
  BAD_ACTION: 'E_BAD_ACTION',
  UNSAFE_RUNTIME: 'E_UNSAFE_RUNTIME',
  TOKEN_MISSING: 'E_TOKEN_MISSING',
  TOKEN_PERMS: 'E_TOKEN_PERMS',
  TOKEN_ENCODING: 'E_TOKEN_ENCODING',
  TOKEN_PARSE: 'E_TOKEN_PARSE',
  TOKEN_INVALID: 'E_TOKEN_INVALID',
  SSH_CONTEXT: 'E_SSH_CONTEXT',
  FORWARD_FAILED: 'E_FORWARD_FAILED',
  NETWORK: 'E_NETWORK',
  AUTH_REJECTED: 'E_AUTH_REJECTED',
  REDIRECT_REFUSED: 'E_REDIRECT_REFUSED',
  UPSTREAM_STATUS: 'E_UPSTREAM_STATUS',
  BODY_TOO_LARGE: 'E_BODY_TOO_LARGE',
  BAD_ENCODING: 'E_BAD_ENCODING',
  BAD_SHAPE: 'E_BAD_SHAPE',
  PROJECT_MISMATCH: 'E_PROJECT_MISMATCH',
  ORG_MISMATCH: 'E_ORG_MISMATCH',
  TARGET_MISMATCH: 'E_TARGET_MISMATCH',
  ENV_CORRUPT: 'E_ENV_CORRUPT',
  DEADLINE: 'E_DEADLINE',
  INTERNAL: 'E_INTERNAL',
};

// ─── Runtime refusal. Detects unsafe invocation; does NOT defeat code already
// imported before this runs.
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

// ─── Token file: lstat, then open O_NOFOLLOW and re-verify the SAME fd.
// Pure ownership policy for a directory we are about to trust. Exported so the
// policy is provable without touching the filesystem. A symlink fails here
// because lstat of a symlink is not a directory.
function classifyOwnedDir(st) {
  return Boolean(st) && st.isDirectory() && st.uid === ACCOUNT.uid
    && (st.mode & 0o077) === 0;
}

function assertOwnedDir(path, code) {
  let st;
  try { st = lstatSync(path); } catch { throw new Fail(code); }
  if (!classifyOwnedDir(st)) throw new Fail(code);
  return true;
}

function readToken() {
  // The fixed source parent must be validated too: the file checks below are
  // worthless if a writable parent lets someone swap the file underneath.
  // Fail closed on drift rather than trusting a once-observed state.
  assertOwnedDir(TOKEN_PARENT, E.TOKEN_PERMS);
  let pre;
  try { pre = lstatSync(TOKEN_PATH); } catch { throw new Fail(E.TOKEN_MISSING); }
  if (pre.isSymbolicLink()) throw new Fail(E.TOKEN_PERMS);

  let fd;
  try { fd = openSync(TOKEN_PATH, FS.O_RDONLY | FS.O_NOFOLLOW); }
  catch { throw new Fail(E.TOKEN_MISSING); }
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0
        || st.nlink !== 1 || st.size === 0 || st.size > MAX_TOKEN_FILE_BYTES) {
      throw new Fail(E.TOKEN_PERMS);
    }
    if (st.ino !== pre.ino || st.dev !== pre.dev) throw new Fail(E.TOKEN_PERMS);
    const buf = Buffer.allocUnsafe(st.size);
    if (readSync(fd, buf, 0, st.size, 0) !== st.size) throw new Fail(E.TOKEN_PERMS);
    let text;
    try { text = new TextDecoder('utf-8', { fatal: true }).decode(buf); }
    catch { throw new Fail(E.TOKEN_ENCODING); }
    return parseTokenFile(text);
  } finally {
    try { closeSync(fd); } catch { /* cleanup only */ }
  }
}

// Pure. EXACTLY one non-blank line, exactly the pinned key, nothing else.
// A BOM, a comment, a second entry, a duplicate or trailing data all fail.
function parseTokenFile(text) {
  if (typeof text !== 'string') throw new Fail(E.TOKEN_PARSE);
  if (text.charCodeAt(0) === 0xfeff) throw new Fail(E.TOKEN_PARSE);   // BOM
  const lines = text.split('\n').filter((l) => l.trim() !== '');
  if (lines.length !== 1) throw new Fail(E.TOKEN_PARSE);
  const line = lines[0];
  if (line.startsWith('#')) throw new Fail(E.TOKEN_PARSE);
  const prefix = TOKEN_KEY + '=';
  if (!line.startsWith(prefix)) throw new Fail(E.TOKEN_PARSE);
  const value = line.slice(prefix.length);
  if (value.length === 0 || value.length > MAX_TOKEN_LEN) throw new Fail(E.TOKEN_INVALID);
  for (const ch of value) {
    const cp = ch.codePointAt(0);
    if (cp < 0x21 || cp > 0x7e) throw new Fail(E.TOKEN_INVALID);
  }
  return value;
}

// ─── Pure response handling.
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

// Absolute wall clock. node:http `timeout` is socket INACTIVITY only, so a
// drip response would never trip it. Settles once; the deadline always wins.
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

function safeId(value) {
  return typeof value === 'string' && value.length > 0 && value.length <= 128
    && /^[A-Za-z0-9._:@+-]+$/.test(value);
}

// Project only the structural target. Environment-level env values are never
// read. The exact compose must occur once and only inside the pinned staging
// environment.
function projectTargetFromProject(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.projectId !== QUIZ_PROJECT_ID) throw new Fail(E.PROJECT_MISMATCH);
  if (body.organizationId !== QUIZ_ORG_ID) throw new Fail(E.ORG_MISMATCH);
  if (!Array.isArray(body.environments)) throw new Fail(E.BAD_SHAPE);
  const hits = [];
  for (const env of body.environments) {
    if (!env || typeof env !== 'object' || !safeId(env.environmentId)
        || typeof env.name !== 'string' || typeof env.isDefault !== 'boolean'
        || !Array.isArray(env.compose)) throw new Fail(E.BAD_SHAPE);
    for (const row of env.compose) {
      if (!row || typeof row !== 'object' || !safeId(row.composeId)
          || !safeId(row.appName)) throw new Fail(E.BAD_SHAPE);
      if (row.composeId === COMPOSE_ID) hits.push({ env, row });
    }
  }
  if (hits.length !== 1) throw new Fail(E.TARGET_MISMATCH);
  const { env, row } = hits[0];
  if (env.environmentId !== STAGING_ENV_ID
      || env.name.toLowerCase() !== STAGING_ENV_NAME || env.isDefault !== false
      || row.appName !== COMPOSE_APPNAME) throw new Fail(E.TARGET_MISMATCH);
  return true;
}

// compose.one necessarily carries the value-bearing env field. It remains
// process-local and is never reflected in an error or receipt.
function projectComposeEnv(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.composeId !== COMPOSE_ID || body.environmentId !== STAGING_ENV_ID
      || body.appName !== COMPOSE_APPNAME || body.sourceType !== 'github'
      || body.repository !== REPO_NAME || body.owner !== REPO_OWNER
      || body.branch !== INFRA_BRANCH || body.composePath !== COMPOSE_PATH) {
    throw new Fail(E.TARGET_MISMATCH);
  }
  if (typeof body.env !== 'string' || Buffer.byteLength(body.env, 'utf8') > MAX_ENV_BYTES) {
    throw new Fail(E.ENV_CORRUPT);
  }
  return body.env;
}

function parseEnvBlock(raw) {
  if (typeof raw !== 'string' || raw.length === 0
      || Buffer.byteLength(raw, 'utf8') > MAX_ENV_BYTES || raw.includes('\r')) {
    throw new Fail(E.ENV_CORRUPT);
  }
  const trailingLf = raw.endsWith('\n');
  const lines = raw.split('\n');
  if (trailingLf) lines.pop();
  if (lines.length === 0 || lines.some((line) => line.length === 0)) {
    throw new Fail(E.ENV_CORRUPT);
  }
  const seen = new Set();
  const parsed = lines.map((line) => {
    const eq = line.indexOf('=');
    if (eq <= 0) throw new Fail(E.ENV_CORRUPT);
    const key = line.slice(0, eq);
    const value = line.slice(eq + 1);
    if (!/^[A-Z][A-Z0-9_]*$/.test(key) || seen.has(key)) {
      throw new Fail(E.ENV_CORRUPT);
    }
    for (const ch of value) {
      const cp = ch.codePointAt(0);
      if (cp < 0x20 || cp > 0x7e) throw new Fail(E.ENV_CORRUPT);
    }
    seen.add(key);
    return { key, value, raw: line };
  });
  return { parsed, trailingLf };
}

function reconcileProxyEnv(raw) {
  const { parsed, trailingLf } = parseEnvBlock(raw);
  const target = parsed.find((line) => line.key === TRUST_KEY);
  if (target?.value === TRUST_VALUE) return { env: raw, changed: false };
  const lines = parsed.map((line) => line.key === TRUST_KEY
    ? `${TRUST_KEY}=${TRUST_VALUE}` : line.raw);
  if (!target) lines.push(`${TRUST_KEY}=${TRUST_VALUE}`);
  const env = lines.join('\n') + (trailingLf ? '\n' : '');
  if (Buffer.byteLength(env, 'utf8') > MAX_ENV_BYTES) throw new Fail(E.ENV_CORRUPT);
  return { env, changed: true };
}

// ─── SSH context validated STRUCTURALLY before the token is ever read, so a
// broken identity or known_hosts fails before a secret is in memory.
function assertSshContext() {
  for (const [path, wantMode] of [[SSH_IDENTITY, 0o077], [SSH_KNOWN_HOSTS, 0o022]]) {
    let st;
    try { st = lstatSync(path); } catch { throw new Fail(E.SSH_CONTEXT); }
    if (!st.isFile() || st.uid !== ACCOUNT.uid) throw new Fail(E.SSH_CONTEXT);
    if ((st.mode & wantMode) !== 0) throw new Fail(E.SSH_CONTEXT);
    if (st.size === 0) throw new Fail(E.SSH_CONTEXT);
  }
  return true;
}

// Strict, pinned, and not overridable from ssh_config: an explicit user@host,
// an explicit identity with IdentitiesOnly, an explicit known_hosts file, and
// strict host-key checking that can only verify — never prompt or accept-new.
const SSH_ARGS = [
  // ClearAllForwardings is deliberately ABSENT. Setting it to yes also clears
  // the -L below (verified on this host's OpenSSH_9.9p2 via `ssh -G`: with it,
  // no localforward is emitted at all), so ssh would authenticate and then
  // never create the socket. -F none is the actual control against
  // config-injected forwardings: no config file is read in the first place.
  // -F none: read NO config files. Without it, mutable per-user/system
  // ssh_config can still inject ProxyJump/ProxyCommand/HostName/control paths,
  // so the pins below would not actually pin the connection.
  '-F', 'none',
  '-N',
  '-o', 'BatchMode=yes',
  '-o', 'StrictHostKeyChecking=yes',
  '-o', 'PasswordAuthentication=no',
  '-o', 'KbdInteractiveAuthentication=no',
  '-o', 'PermitLocalCommand=no',
  '-o', 'ExitOnForwardFailure=yes',
  '-o', 'IdentitiesOnly=yes',
  '-o', `UserKnownHostsFile=${SSH_KNOWN_HOSTS}`,
  '-i', SSH_IDENTITY,
];

// Readiness is decided by lstat on the socket ONLY. No HTTP probe: project.one
// must be the SOLE request that ever crosses this forward.
function openForward() {
  // No pre-delete of a predictable path: destroying a path before proving this
  // run owns it is a destructive act on someone else's file. mkdirSync is
  // idempotent and does NOT repair perms on an existing dir, so the parent is
  // verified explicitly, then mkdtemp yields a fresh 0700 dir owned by this run.
  mkdirSync(TEMP_PARENT, { recursive: true, mode: 0o700 });
  assertOwnedDir(TEMP_PARENT, E.FORWARD_FAILED);
  const dir = mkdtempSync(join(TEMP_PARENT, 'dkscope-'));
  if (!classifyOwnedDir(lstatSync(dir))) {
    rmSync(dir, { recursive: true, force: true });
    throw new Fail(E.FORWARD_FAILED);
  }
  const sock = join(dir, 's');

  const child = spawn(SSH_BIN,
    [...SSH_ARGS, '-L', `${sock}:${REMOTE_ADDR}`, `${SSH_USER}@${SSH_HOST}`],
    // Empty env and no stdio: the token cannot reach argv, env or stdin.
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
      let st = null;
      try { st = lstatSync(sock); } catch { st = null; }
      if (st && st.isSocket() && st.uid === ACCOUNT.uid) { finish(resolve, sock); return; }
      if (Date.now() - started > SOCKET_READY_MS) { finish(reject, new Fail(E.FORWARD_FAILED)); return; }
      setTimeout(poll, 100);
    };
    poll();
  });

  return { ready, cleanup };
}

// Only four fixed request shapes exist. There is no caller-supplied path,
// method, host, compose, project or environment.
function requestSpec(kind, envText) {
  if (kind === 'project') return {
    method: 'GET', path: `${P_PROJECT_ONE}?projectId=${QUIZ_PROJECT_ID}`,
  };
  if (kind === 'compose') return {
    method: 'GET', path: `${P_COMPOSE_ONE}?composeId=${COMPOSE_ID}`,
  };
  if (kind === 'update') return {
    method: 'POST', path: P_COMPOSE_UPDATE,
    body: { composeId: COMPOSE_ID, env: envText },
  };
  if (kind === 'deploy') return {
    method: 'POST', path: P_COMPOSE_DEPLOY, body: { composeId: COMPOSE_ID },
  };
  throw new Fail(E.INTERNAL);
}

function buildRequestOptions({ socketPath, token, spec, bodyJson }) {
  const headers = { accept: 'application/json', 'x-api-key': token };
  if (bodyJson !== undefined) {
    headers['content-type'] = 'application/json';
    headers['content-length'] = Buffer.byteLength(bodyJson).toString();
  }
  return { socketPath, method: spec.method, path: spec.path, headers,
           timeout: REQUEST_TIMEOUT_MS };
}

function requestOnce({ socketPath, token, spec }) {
  const bodyJson = spec.body === undefined ? undefined : JSON.stringify(spec.body);
  let active = null;
  return withAbsoluteDeadline({
    ms: REQUEST_TIMEOUT_MS,
    abort: () => { try { active?.destroy(); } catch { /* best effort */ } },
    start: (ok, bad) => {
      const agent = new http.Agent({ keepAlive: false, maxSockets: 1 });
      const req = http.request(
        { ...buildRequestOptions({ socketPath, token, spec, bodyJson }), agent },
        (res) => {
          try { classifyStatus(res.statusCode); }
          catch (e) { res.destroy(); bad(e); return; }
          consumeBoundedStream(res, MAX_BODY_BYTES).then(
            (text) => { try { ok(parseBounded(text)); } catch (e) { bad(e); } }, bad);
        });
      active = req;
      req.on('timeout', () => { req.destroy(); bad(new Fail(E.NETWORK)); });
      req.on('error', () => bad(new Fail(E.NETWORK)));
      if (bodyJson !== undefined) req.write(bodyJson);
      req.end();
    },
  });
}

async function orchestrate({ readTokenFn, assertContextFn, openForwardFn, requestFn,
                             deadlineMs = ACTION_DEADLINE_MS }) {
  for (const fn of [readTokenFn, assertContextFn, openForwardFn, requestFn]) {
    if (typeof fn !== 'function') throw new Fail(E.BAD_SHAPE);
  }
  assertContextFn();
  const token = readTokenFn();
  const fwd = openForwardFn();
  let timer = null;
  try {
    const budget = new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Fail(E.DEADLINE)), deadlineMs);
      timer.unref?.();
    });
    const work = (async () => {
      const socketPath = await fwd.ready;
      const call = (spec) => requestFn({ socketPath, token, spec });
      projectTargetFromProject(await call(requestSpec('project')));
      const current = projectComposeEnv(await call(requestSpec('compose')));
      const next = reconcileProxyEnv(current);
      if (next.changed) {
        await call(requestSpec('update', next.env));
        await call(requestSpec('deploy'));
      }
    })();
    work.catch(() => { /* late loser remains contained */ });
    await Promise.race([work, budget]);
  } finally {
    if (timer) clearTimeout(timer);
    fwd.cleanup();
  }
  return OK_RECEIPT;
}

async function main() {
  if (process.argv.slice(2).length !== 0) {
    process.stdout.write(E.BAD_ACTION + '\n');
    process.exitCode = 2;
    return;
  }
  try {
    assertSafeRuntime();
    const receipt = await orchestrate({
      assertContextFn: assertSshContext, readTokenFn: readToken,
      openForwardFn: openForward, requestFn: requestOnce,
    });
    process.stdout.write(receipt + '\n');
  } catch (err) {
    process.stdout.write((err instanceof Fail ? err.code : E.INTERNAL) + '\n');
    process.exitCode = 1;
  }
}

const isDirectRun = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isDirectRun) await main();

export {
  Fail, E, OK_RECEIPT, TOKEN_KEY, TRUST_KEY, TRUST_VALUE,
  QUIZ_PROJECT_ID, QUIZ_ORG_ID, STAGING_ENV_ID, COMPOSE_ID, COMPOSE_APPNAME,
  parseTokenFile, classifyStatus, parseBounded, consumeBoundedStream,
  withAbsoluteDeadline, classifyRuntime, assertSafeRuntime, classifyOwnedDir,
  requestSpec, buildRequestOptions, projectTargetFromProject, projectComposeEnv,
  parseEnvBlock, reconcileProxyEnv, orchestrate, SSH_ARGS,
};
