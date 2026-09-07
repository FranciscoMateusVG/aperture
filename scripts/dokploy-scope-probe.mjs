#!/usr/bin/env node
/**
 * dokploy-scope-probe — ONE read-only question: does the operator-supplied
 * xerox Dokploy token carry the Quiz organization?
 *
 * Bead: aperture-ztid5. Design and code gate: Cipher. NOT RUN against any live
 * host; requires his exact-head PASS plus a GLaDOS execution dispatch.
 *
 * WHAT THIS IS
 *   A single fixed-purpose, NO-ARGUMENT action. Reads one pinned local file,
 *   opens a strict SSH Unix-socket forward, makes EXACTLY ONE read-only HTTP
 *   request, and prints one constant.
 *
 * WHAT THIS IS NOT
 *   No Infisical code of any kind — no auth, no retrieval, no constants. No
 *   POST capability exists anywhere in this file. No list, search, retry,
 *   fallback, mutation, or second endpoint. The token file is READ ONLY and
 *   never modified.
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
const P_PROJECT_ONE = '/api/project.one';

const OK_RECEIPT = 'XEROX_QUIZ_SCOPE_CONFIRMED';

const MAX_TOKEN_FILE_BYTES = 8192;
const MAX_TOKEN_LEN = 4096;
const MAX_BODY_BYTES = 1_000_000;
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

function assertQuizScope(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.projectId !== QUIZ_PROJECT_ID) throw new Fail(E.PROJECT_MISMATCH);
  if (body.organizationId !== QUIZ_ORG_ID) throw new Fail(E.ORG_MISMATCH);
  return true;
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
  '-o', 'ClearAllForwardings=yes',
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

// ─── The ONE request. Private agent; the ambient globalAgent is never used, so
// a patched global cannot observe this traffic even with a stock constructor.
// Pure: the exact wire shape of the ONE request. Exported so tests can assert
// the real path and header set. The seam alone cannot prove this — it observes
// {token} before any header exists.
function buildRequestOptions({ socketPath, token, timeoutMs }) {
  return {
    socketPath,
    method: 'GET',
    path: `${P_PROJECT_ONE}?projectId=${QUIZ_PROJECT_ID}`,
    headers: { accept: 'application/json', 'x-api-key': token },
    timeout: timeoutMs,
  };
}

function requestProjectOne({ socketPath, token, timeoutMs }) {
  let active = null;
  return withAbsoluteDeadline({
    ms: timeoutMs,
    abort: () => { try { if (active) active.destroy(); } catch { /* best effort */ } },
    start: (ok, bad) => {
      const agent = new http.Agent({ keepAlive: false, maxSockets: 1 });
      const req = http.request({
        ...buildRequestOptions({ socketPath, token, timeoutMs }),
        agent,
      }, (res) => {
        try { classifyStatus(res.statusCode); }
        catch (e) { res.destroy(); bad(e); return; }
        consumeBoundedStream(res, MAX_BODY_BYTES).then(
          (text) => { try { ok(parseBounded(text)); } catch (e) { bad(e); } }, bad);
      });
      active = req;
      req.on('timeout', () => { req.destroy(); bad(new Fail(E.NETWORK)); });
      req.on('error', () => bad(new Fail(E.NETWORK)));   // cause never surfaced
      req.end();
    },
  });
}

// ─── Composed orchestration with INJECTED seams, so the exact call sequence is
// provable in tests. None of the injected functions can discover the real
// reader, the real host, or the real transport — they are supplied by the
// caller, and the live path below supplies the private ones.
async function orchestrate({ readTokenFn, assertContextFn, openForwardFn, requestFn,
                             deadlineMs = ACTION_DEADLINE_MS }) {
  assertContextFn();
  const token = readTokenFn();
  const fwd = openForwardFn();
  let timer = null;
  try {
    // The action deadline lives HERE, not in a process.exit timer: exiting the
    // process skips this finally and can strand the ssh child and its socket
    // dir. Losing the race rejects normally, so cleanup still runs exactly once.
    const budget = new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Fail(E.DEADLINE)), deadlineMs);
      timer.unref?.();
    });
    const work = (async () => {
      const socketPath = await fwd.ready;
      const body = await requestFn({ socketPath, token, timeoutMs: REQUEST_TIMEOUT_MS });
      assertQuizScope(body);
    })();
    work.catch(() => { /* a late loser must not raise an unhandled rejection */ });
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
      assertContextFn: assertSshContext,
      readTokenFn: readToken,
      openForwardFn: openForward,
      requestFn: requestProjectOne,
    });
    process.stdout.write(receipt + '\n');
  } catch (err) {
    process.stdout.write((err instanceof Fail ? err.code : E.INTERNAL) + '\n');
    process.exitCode = 1;
  }
}

const isDirectRun = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isDirectRun) main();

export {
  parseTokenFile, classifyStatus, parseBounded, consumeBoundedStream,
  withAbsoluteDeadline, classifyRuntime, assertSafeRuntime, assertQuizScope,
  E, Fail, OK_RECEIPT, QUIZ_PROJECT_ID, QUIZ_ORG_ID, TOKEN_KEY, P_PROJECT_ONE,
  classifyOwnedDir, buildRequestOptions,
  orchestrate, SSH_ARGS,
};
