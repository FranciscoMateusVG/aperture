#!/usr/bin/env node
/**
 * Fixed production maintenance cutover for the Quiz auth-only release.
 *
 * Bead: aperture-ztid5. Design and code gate: Cipher. NOT RUN against any live
 * host; requires his exact-head PASS plus a GLaDOS execution dispatch.
 *
 * WHAT THIS IS
 *   A fixed three-phase action: prepare, publish, or restore-metadata. Every
 *   phase is pinned to one production project, compose, domain and release
 *   revision. A private state file captures the supported API's decrypted env
 *   and original structural rows before the first mutation.
 *
 * WHAT THIS IS NOT
 *   No Infisical code of any kind — no auth, no retrieval, no constants. No
 *   list, search, retry, fallback, provisioning, secret generation, or
 *   caller-selected endpoint. It does not rebuild the old deployment: host-
 *   local rollback uses the separately captured exact image/inspect artifact.
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

import { openSync, fstatSync, lstatSync, readSync, closeSync, writeSync,
         fsyncSync, linkSync, mkdirSync, mkdtempSync, rmSync,
         constants as FS } from 'node:fs';
import { userInfo } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
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
const PROD_ENV_ID = '2R_tl5CmivjIDXqjdIsbF';
const PROD_ENV_NAME = 'production';
const COMPOSE_ID = 'eAVrq4KRr2EUx0f7sYAgH';
const COMPOSE_APPNAME = 'quiz-incluir-e17b8a-w3hpak';
const REPO_OWNER = 'FranciscoMateusVG';
const REPO_NAME = 'quiz-incluir';
const RELEASE_BRANCH = 'aperture-ztid5-prod-auth-cutover';
const RELEASE_REV = 'a988bb2b53251cb395b9e774b3ccc922a41e0752';
const GITHUB_ID = 'TOmazYpTr8Wz21abongPE';
const COMPOSE_PATH = './docker-compose.prod.yml';
const DOMAIN_ID = '8imJoHOm8KjT4uk8hH8EG';
const DOMAIN_HOST = 'quiz.programaincluir.org';
const OLD_DOMAIN_SERVICE = 'quiz-incluir-frontend-e17b8a';
const NEW_DOMAIN_SERVICE = 'quiz-incluir-backend-e17b8a';
const OLD_DOMAIN_PORT = 8080;
const NEW_DOMAIN_PORT = 8000;
const AUTH_KEY = 'MONOREPO_AUTH_URL';
const AUTH_VALUE = 'http://hono-app:3003';
const TRUST_KEY = 'TRUSTED_PROXY_CIDRS';
const P_PROJECT_ONE = '/api/project.one';
const P_COMPOSE_ONE = '/api/compose.one';
const P_COMPOSE_UPDATE = '/api/compose.update';
const P_COMPOSE_DEPLOY = '/api/compose.deploy';
const P_DOMAIN_ONE = '/api/domain.one';
const P_DOMAIN_UPDATE = '/api/domain.update';

const OK_PREPARE = 'QUIZ_PROD_PRIVATE_DEPLOY_TRIGGERED';
const OK_PUBLISH = 'QUIZ_PROD_PUBLIC_DEPLOY_TRIGGERED';
const OK_RESTORE = 'QUIZ_PROD_METADATA_RESTORE_TRIGGERED';
const STATE_PATH = join(TEMP_PARENT, 'quiz-prod-auth-cutover-state.json');

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
  DOMAIN_MISMATCH: 'E_DOMAIN_MISMATCH',
  STATE_MISSING: 'E_STATE_MISSING',
  STATE_CORRUPT: 'E_STATE_CORRUPT',
  STATE_EXISTS: 'E_STATE_EXISTS',
  RELEASE_REV_MISMATCH: 'E_RELEASE_REV_MISMATCH',
  PHASE_MISMATCH: 'E_PHASE_MISMATCH',
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
// read. The exact compose must occur once and only inside production.
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
  if (env.environmentId !== PROD_ENV_ID
      || env.name.toLowerCase() !== PROD_ENV_NAME
      || row.appName !== COMPOSE_APPNAME) throw new Fail(E.TARGET_MISMATCH);
  return true;
}

// compose.one necessarily carries the value-bearing env field. It remains
// process-local and is never reflected in an error or receipt.
function projectCompose(body, allowedBranches = ['main', RELEASE_BRANCH]) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.composeId !== COMPOSE_ID || body.environmentId !== PROD_ENV_ID
      || body.appName !== COMPOSE_APPNAME || body.sourceType !== 'github'
      || body.repository !== REPO_NAME || body.owner !== REPO_OWNER
      || !allowedBranches.includes(body.branch) || body.composePath !== COMPOSE_PATH) {
    throw new Fail(E.TARGET_MISMATCH);
  }
  if (typeof body.autoDeploy !== 'boolean') throw new Fail(E.BAD_SHAPE);
  if (typeof body.env !== 'string' || Buffer.byteLength(body.env, 'utf8') > MAX_ENV_BYTES) {
    throw new Fail(E.ENV_CORRUPT);
  }
  return { env: body.env, branch: body.branch };
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

function reconcilePrepareEnv(raw) {
  const { parsed, trailingLf } = parseEnvBlock(raw);
  const auth = parsed.find((line) => line.key === AUTH_KEY);
  const trust = parsed.find((line) => line.key === TRUST_KEY);
  const lines = parsed.map((line) => {
    if (line.key === AUTH_KEY) return `${AUTH_KEY}=${AUTH_VALUE}`;
    if (line.key === TRUST_KEY) return `${TRUST_KEY}=`;
    return line.raw;
  });
  if (!auth) lines.push(`${AUTH_KEY}=${AUTH_VALUE}`);
  if (!trust) lines.push(`${TRUST_KEY}=`);
  const env = lines.join('\n') + (trailingLf ? '\n' : '');
  if (Buffer.byteLength(env, 'utf8') > MAX_ENV_BYTES) throw new Fail(E.ENV_CORRUPT);
  return { env, changed: env !== raw };
}

const DOMAIN_FIELDS = Object.freeze([
  'domainId', 'host', 'path', 'port', 'customEntrypoint', 'https',
  'certificateType', 'customCertResolver', 'serviceName', 'domainType',
  'internalPath', 'stripPath', 'middlewares', 'forwardAuthEnabled', 'enabled',
  'composeId',
]);

function projectDomain(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  const out = {};
  for (const k of DOMAIN_FIELDS) out[k] = body[k] ?? null;
  if (out.domainId !== DOMAIN_ID || out.composeId !== COMPOSE_ID
      || out.host !== DOMAIN_HOST || out.https !== true || out.path !== '/'
      || out.certificateType !== 'letsencrypt' || out.domainType !== 'compose') {
    throw new Fail(E.DOMAIN_MISMATCH);
  }
  if (![OLD_DOMAIN_SERVICE, NEW_DOMAIN_SERVICE].includes(out.serviceName)
      || ![OLD_DOMAIN_PORT, NEW_DOMAIN_PORT].includes(out.port)
      || typeof out.enabled !== 'boolean' || !Array.isArray(out.middlewares)
      || typeof out.stripPath !== 'boolean'
      || typeof out.forwardAuthEnabled !== 'boolean') {
    throw new Fail(E.DOMAIN_MISMATCH);
  }
  return out;
}

function assertOldPublishedDomain(d) {
  if (d.serviceName !== OLD_DOMAIN_SERVICE || d.port !== OLD_DOMAIN_PORT
      || d.enabled !== true) throw new Fail(E.PHASE_MISMATCH);
  return true;
}

function assertPreparedDomain(d) {
  if (d.serviceName !== OLD_DOMAIN_SERVICE || d.port !== OLD_DOMAIN_PORT
      || d.enabled !== false) throw new Fail(E.PHASE_MISMATCH);
  return true;
}

function assertPublishedDomain(d) {
  if (d.serviceName !== NEW_DOMAIN_SERVICE || d.port !== NEW_DOMAIN_PORT
      || d.enabled !== true) throw new Fail(E.PHASE_MISMATCH);
  return true;
}

function fullDomainUpdate(d, { serviceName, port, enabled }) {
  const body = { domainId: DOMAIN_ID };
  for (const k of DOMAIN_FIELDS) {
    if (k === 'domainId' || k === 'composeId') continue;
    body[k] = d[k];
  }
  body.serviceName = serviceName;
  body.port = port;
  body.enabled = enabled;
  return body;
}

function bodyComposeSource() {
  return { composeId: COMPOSE_ID, repository: REPO_NAME, owner: REPO_OWNER,
    branch: RELEASE_BRANCH, githubId: GITHUB_ID, sourceType: 'github',
    composePath: COMPOSE_PATH, autoDeploy: false };
}

const MAX_STATE_BYTES = 131_072;
const COMPOSE_STATE_FIELDS = Object.freeze([
  'composeId', 'environmentId', 'appName', 'sourceType', 'repository', 'owner',
  'branch', 'composePath', 'githubId', 'composeType', 'autoDeploy', 'env',
]);

function composeSnapshot(body) {
  const checked = projectCompose(body);
  const out = {};
  for (const k of COMPOSE_STATE_FIELDS) out[k] = body[k] ?? null;
  out.env = checked.env;
  return out;
}

function serializeState(compose, domain) {
  const body = JSON.stringify({ version: 1, compose: composeSnapshot(compose),
    domain: projectDomain(domain) }) + '\n';
  if (Buffer.byteLength(body, 'utf8') > MAX_STATE_BYTES) throw new Fail(E.STATE_CORRUPT);
  return body;
}

function parseState(text) {
  if (typeof text !== 'string' || Buffer.byteLength(text, 'utf8') > MAX_STATE_BYTES) {
    throw new Fail(E.STATE_CORRUPT);
  }
  let body;
  try { body = JSON.parse(text); } catch { throw new Fail(E.STATE_CORRUPT); }
  if (!body || body.version !== 1 || !body.compose || !body.domain) {
    throw new Fail(E.STATE_CORRUPT);
  }
  try {
    const compose = composeSnapshot(body.compose);
    const domain = projectDomain(body.domain);
    assertOldPublishedDomain(domain);
    return { compose, domain };
  } catch { throw new Fail(E.STATE_CORRUPT); }
}

function readOwnedState() {
  assertOwnedDir(TEMP_PARENT, E.STATE_CORRUPT);
  let fd;
  try { fd = openSync(STATE_PATH, FS.O_RDONLY | FS.O_NOFOLLOW); }
  catch (err) {
    if (err?.code === 'ENOENT') return null;
    throw new Fail(E.STATE_CORRUPT);
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0
        || st.nlink !== 1 || st.size === 0 || st.size > MAX_STATE_BYTES) {
      throw new Fail(E.STATE_CORRUPT);
    }
    const buf = Buffer.allocUnsafe(st.size);
    if (readSync(fd, buf, 0, st.size, 0) !== st.size) throw new Fail(E.STATE_CORRUPT);
    try { return new TextDecoder('utf-8', { fatal: true }).decode(buf); }
    catch { throw new Fail(E.STATE_CORRUPT); }
  } finally { try { closeSync(fd); } catch { /* cleanup */ } }
}

function publishState(text) {
  if (readOwnedState() !== null) throw new Fail(E.STATE_EXISTS);
  const buf = Buffer.from(text, 'utf8');
  const tmp = `${STATE_PATH}.tmp-${randomBytes(8).toString('hex')}`;
  let fd;
  try {
    fd = openSync(tmp, FS.O_WRONLY | FS.O_CREAT | FS.O_EXCL | FS.O_NOFOLLOW, 0o600);
    let off = 0;
    while (off < buf.length) off += writeSync(fd, buf, off, buf.length - off);
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0
        || st.nlink !== 1 || st.size !== buf.length) throw new Fail(E.STATE_CORRUPT);
    fsyncSync(fd);
    closeSync(fd); fd = undefined;
    linkSync(tmp, STATE_PATH); // no overwrite
    let dfd;
    try { dfd = openSync(TEMP_PARENT, FS.O_RDONLY); fsyncSync(dfd); }
    finally { try { if (dfd !== undefined) closeSync(dfd); } catch { /* cleanup */ } }
  } catch (err) {
    try { if (fd !== undefined) closeSync(fd); } catch { /* cleanup */ }
    throw err instanceof Fail ? err : new Fail(E.STATE_CORRUPT);
  } finally { try { rmSync(tmp, { force: true }); } catch { /* cleanup */ } }
  const back = readOwnedState();
  if (back !== text) throw new Fail(E.STATE_CORRUPT);
  return true;
}

function assertReleaseRevision(lsRemoteOutput) {
  if (typeof lsRemoteOutput !== 'string' || lsRemoteOutput.length > 4096) {
    throw new Fail(E.RELEASE_REV_MISMATCH);
  }
  const lines = lsRemoteOutput.split('\n').filter((x) => x.trim() !== '');
  if (lines.length !== 1) throw new Fail(E.RELEASE_REV_MISMATCH);
  const m = /^([0-9a-f]{40})\s+refs\/heads\/(.+)$/.exec(lines[0].trim());
  if (!m || m[1] !== RELEASE_REV || m[2] !== RELEASE_BRANCH) {
    throw new Fail(E.RELEASE_REV_MISMATCH);
  }
  return true;
}

function readReleaseRevision() {
  return execFileSync('/usr/bin/git',
    ['ls-remote', `https://github.com/${REPO_OWNER}/${REPO_NAME}`,
      `refs/heads/${RELEASE_BRANCH}`],
    { encoding: 'utf8', timeout: 30_000, maxBuffer: 4096,
      stdio: ['ignore', 'pipe', 'ignore'] });
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

// Only these fixed request shapes exist. No caller supplies a path, target or
// body; `payload` is produced only by the validated state transitions below.
function requestSpec(kind, payload) {
  if (kind === 'project') return {
    method: 'GET', path: `${P_PROJECT_ONE}?projectId=${QUIZ_PROJECT_ID}`,
  };
  if (kind === 'compose') return {
    method: 'GET', path: `${P_COMPOSE_ONE}?composeId=${COMPOSE_ID}`,
  };
  if (kind === 'domain') return {
    method: 'GET', path: `${P_DOMAIN_ONE}?domainId=${DOMAIN_ID}`,
  };
  if (kind === 'source') return {
    method: 'POST', path: P_COMPOSE_UPDATE, body: bodyComposeSource(),
  };
  if (kind === 'env') return {
    method: 'POST', path: P_COMPOSE_UPDATE,
    body: { composeId: COMPOSE_ID, env: payload },
  };
  if (kind === 'domain-update') return {
    method: 'POST', path: P_DOMAIN_UPDATE, body: payload,
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

async function orchestrate({ phase, readTokenFn, assertContextFn, openForwardFn,
                             requestFn, readStateFn, publishStateFn, revisionFn,
                             deadlineMs = ACTION_DEADLINE_MS }) {
  if (!['prepare', 'publish', 'restore-metadata'].includes(phase)) {
    throw new Fail(E.BAD_ACTION);
  }
  for (const fn of [readTokenFn, assertContextFn, openForwardFn, requestFn,
                    readStateFn, publishStateFn, revisionFn]) {
    if (typeof fn !== 'function') throw new Fail(E.BAD_SHAPE);
  }
  // Rollback availability must not depend on the candidate branch continuing
  // to exist. The pin gates forward movement only; restore uses the captured
  // original metadata and the separately prepared exact-image host artifact.
  if (phase !== 'restore-metadata') assertReleaseRevision(revisionFn());
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
      const composeBody = await call(requestSpec('compose'));
      const current = projectCompose(composeBody);
      const domain = projectDomain(await call(requestSpec('domain')));
      const storedText = readStateFn();

      if (phase === 'prepare') {
        let state;
        if (storedText === null) {
          assertOldPublishedDomain(domain);
          const snapshot = serializeState(composeBody, domain);
          publishStateFn(snapshot); // durable before the first remote mutation
          state = parseState(snapshot);
        } else {
          state = parseState(storedText);
        }
        const expected = reconcilePrepareEnv(state.compose.env).env;
        const originalSource = current.branch === state.compose.branch
          && composeBody.autoDeploy === state.compose.autoDeploy;
        const releaseSource = current.branch === RELEASE_BRANCH
          && composeBody.autoDeploy === false;
        const originalEnv = current.env === state.compose.env;
        const preparedEnv = current.env === expected;
        if (domain.enabled === true) {
          assertOldPublishedDomain(domain);
          // Before the first mutation, the supported API must still match the
          // snapshot we are about to rely on. Never overwrite later credential
          // changes from a stale local state file.
          if (!originalSource || !originalEnv) throw new Fail(E.PHASE_MISMATCH);
        } else {
          assertPreparedDomain(domain);
          // Legitimate crash-resume points are narrowly enumerated: domain
          // disabled before source update; source updated before env; or both
          // updated before the queued deploy. Any other drift stops.
          const validResume = (originalSource && originalEnv)
            || (releaseSource && originalEnv)
            || (releaseSource && preparedEnv);
          if (!validResume) {
            throw new Fail(E.PHASE_MISMATCH);
          }
        }

        // Disable first so the exact pre-build service-name validator skips
        // the old frontend key, which is absent from the candidate compose.
        if (domain.enabled) {
          await call(requestSpec('domain-update', fullDomainUpdate(domain, {
            serviceName: OLD_DOMAIN_SERVICE, port: OLD_DOMAIN_PORT, enabled: false,
          })));
        }
        if (!releaseSource) await call(requestSpec('source'));
        if (!preparedEnv) {
          await call(requestSpec('env', expected));
        }
        await call(requestSpec('deploy'));
      } else if (phase === 'publish') {
        if (storedText === null) throw new Fail(E.STATE_MISSING);
        const state = parseState(storedText);
        projectCompose(composeBody, [RELEASE_BRANCH]);
        if (composeBody.autoDeploy !== false) throw new Fail(E.PHASE_MISMATCH);
        if (domain.enabled === false) assertPreparedDomain(domain);
        else assertPublishedDomain(domain);
        if (current.env !== reconcilePrepareEnv(state.compose.env).env) {
          throw new Fail(E.PHASE_MISMATCH);
        }
        if (!domain.enabled) {
          await call(requestSpec('domain-update', fullDomainUpdate(domain, {
            serviceName: NEW_DOMAIN_SERVICE, port: NEW_DOMAIN_PORT, enabled: true,
          })));
        }
        await call(requestSpec('deploy')); // labels are baked only by deploy
      } else {
        if (storedText === null) throw new Fail(E.STATE_MISSING);
        const state = parseState(storedText);
        await call(requestSpec('env', state.compose.env));
        await call({ method: 'POST', path: P_COMPOSE_UPDATE, body: {
          composeId: COMPOSE_ID, repository: state.compose.repository,
          owner: state.compose.owner, branch: state.compose.branch,
          githubId: GITHUB_ID, sourceType: state.compose.sourceType,
          composePath: state.compose.composePath,
          autoDeploy: state.compose.autoDeploy,
        } });
        await call(requestSpec('domain-update', fullDomainUpdate(state.domain, {
          serviceName: state.domain.serviceName, port: state.domain.port,
          enabled: state.domain.enabled,
        })));
      }
    })();
    work.catch(() => { /* late loser remains contained */ });
    await Promise.race([work, budget]);
  } finally {
    if (timer) clearTimeout(timer);
    fwd.cleanup();
  }
  return phase === 'prepare' ? OK_PREPARE
    : phase === 'publish' ? OK_PUBLISH : OK_RESTORE;
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length !== 1 || !['prepare', 'publish', 'restore-metadata'].includes(args[0])) {
    process.stdout.write(E.BAD_ACTION + '\n');
    process.exitCode = 2;
    return;
  }
  try {
    assertSafeRuntime();
    mkdirSync(TEMP_PARENT, { recursive: true, mode: 0o700 });
    assertOwnedDir(TEMP_PARENT, E.STATE_CORRUPT);
    const receipt = await orchestrate({ phase: args[0],
      assertContextFn: assertSshContext, readTokenFn: readToken,
      openForwardFn: openForward, requestFn: requestOnce,
      readStateFn: readOwnedState, publishStateFn: publishState,
      revisionFn: readReleaseRevision,
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
  Fail, E, OK_PREPARE, OK_PUBLISH, OK_RESTORE, TOKEN_KEY, AUTH_KEY, AUTH_VALUE,
  TRUST_KEY, QUIZ_PROJECT_ID, QUIZ_ORG_ID, PROD_ENV_ID, COMPOSE_ID, COMPOSE_APPNAME,
  parseTokenFile, classifyStatus, parseBounded, consumeBoundedStream,
  withAbsoluteDeadline, classifyRuntime, assertSafeRuntime, classifyOwnedDir,
  requestSpec, buildRequestOptions, projectTargetFromProject, projectCompose,
  parseEnvBlock, reconcilePrepareEnv, projectDomain, fullDomainUpdate,
  assertOldPublishedDomain, assertPreparedDomain, assertPublishedDomain,
  serializeState, parseState,
  composeSnapshot, assertReleaseRevision, bodyComposeSource,
  orchestrate, SSH_ARGS, RELEASE_BRANCH, RELEASE_REV, DOMAIN_ID,
};
