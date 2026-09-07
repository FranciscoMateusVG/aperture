/**
 * ONE fixed-purpose staging provisioning action for the Xerox Quiz project.
 * aperture-ztid5. NOT a generic request broker: every endpoint, method, order,
 * identifier and body field below is fixed at authoring time. There is no
 * caller-supplied path, body, host or action.
 *
 * Gate 1 is the SOLE read: project.one. It binds identity (exact projectId AND
 * organizationId) and selects exactly one staging environment by the server's
 * own declared environment name. Zero or multiple matches is a hard stop --
 * never a guess, and never a fallback to production.
 *
 * Mutations run only after that binding AND after the pinned Hono image and
 * the pushed infra revision are both known. Production identifiers are refused
 * structurally, not by convention.
 *
 * Controls carried unchanged from the Cipher-PASSed probe (d5b65f0): literal
 * token source with no-follow/owner/mode/nlink checks and bounded printable
 * value, verified parent directories, mkdtemp with no pre-delete, pinned SSH
 * host/user/key/known_hosts with -F none and no ClearAllForwardings, private
 * HTTP agent on one socket, absolute deadlines and response caps, no retries,
 * no redirects, no fallback endpoint, no token/body/error reflection, and
 * unconditional cleanup.
 *
 * Output is fixed codes plus the minimum non-secret identifiers needed to
 * evidence the staging binding. Never a secret, hash, length, header, env
 * value or raw response body.
 */
import { openSync, fstatSync, lstatSync, readSync, closeSync, mkdirSync, mkdtempSync,
         rmSync, writeFileSync } from 'node:fs';
import { constants as FS } from 'node:fs';
import { userInfo } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import http from 'node:http';

const HOME = userInfo().homedir;
const ACCOUNT = { uid: userInfo().uid };

// ── Pinned source and transport (identical to the reviewed probe) ────────
const TOKEN_PARENT = join(HOME, 'Downloads');
const TOKEN_PATH = join(TOKEN_PARENT, 'secret copy.txt');
const TOKEN_KEY = 'DOKPLOY_TOKEN_INCLUIR_XEROX';
const TEMP_PARENT = join(HOME, '.config', 'aperture');
const SSH_BIN = '/usr/bin/ssh';
const SSH_USER = 'ubuntu';
const SSH_HOST = '100.85.254.44';
const SSH_IDENTITY = join(HOME, '.ssh', 'id_ed25519');
const SSH_KNOWN_HOSTS = join(HOME, '.ssh', 'known_hosts');
const REMOTE_ADDR = '127.0.0.1:3000';

// ── Fixed staging target ────────────────────────────────────────────────
const QUIZ_PROJECT_ID = 'w4FraIVPC0PfP2fZxVtaT';
const QUIZ_ORG_ID = 'GME9CAd599FWcInMNTZ2F';
const STAGING_ENV_NAME = 'staging';           // server-declared discriminator
const COMPOSE_NAME = 'Quiz Staging';
const COMPOSE_APPNAME = 'quiz-incluir-staging';
const COMPOSE_PATH = './docker-compose.staging.yml';
const REPO_OWNER = 'FranciscoMateusVG';
const REPO_NAME = 'quiz-incluir';
const INFRA_BRANCH = 'aperture-ztid5-staging';
const INFRA_REV = '26d6a9eb25b8653356d1aae4659382a62d479c41';
const GITHUB_ID = 'TOmazYpTr8Wz21abongPE';
const DOMAIN_HOST = 'staging-quiz.programaincluir.org';
const DOMAIN_PORT = 8000;
const DOMAIN_SERVICE = 'quiz-incluir-backend-staging'; // exact compose service key
const PINNED_HONO_IMAGE = 'quiz-staging-hono:9cb605fc';
// Immutable identity of the image built on xerox from monorepo revision
// 9cb605fc2cc63930a9fdf4f73ccec3ec05c6daad. Recorded so the packet pins WHAT
// runs, not merely a mutable tag that could later point elsewhere.
const PINNED_HONO_IMAGE_ID =
  'sha256:a88663c43ac045af03ed27d7d14199d476f5c8ae024d2128e3af56b6287079a7';
const PINNED_HONO_SOURCE_REV = '9cb605fc2cc63930a9fdf4f73ccec3ec05c6daad';

// Production identifiers that must NEVER appear in a request or a response we
// act on. Structural refusal, not a naming convention.
const PROD_DENYLIST = Object.freeze([
  '_A6rI-GEm9oF8ysIojm0O', 'bPiJP-GUPhNbIsOEN_HmW', 'biqK8MbgAXtrJH24k5zTg',
  '27vJsrYScdmCcKf1qVh6Y', 'uIBU4__1Jw3RGp6WSzz6y', '4sHHtg1XwERiDc6o2labm',
  'compose-override-solid-state-port-349ude', 'quiz-incluir-e17b8a-w3hpak',
  'prod-main-app-main-apps-wfjeox',
]);

const P_PROJECT_ONE = '/api/project.one';
const P_COMPOSE_CREATE = '/api/compose.create';
const P_COMPOSE_UPDATE = '/api/compose.update';
const P_DOMAIN_CREATE = '/api/domain.create';
const P_COMPOSE_DEPLOY = '/api/compose.deploy';
// Minimum additional read-only endpoints, confirmed present in the RUNNING
// Dokploy v0.30.2 build (procedure names grepped from /app/.next/server), not
// assumed. Needed because project.one cannot establish domain state.
const P_COMPOSE_ONE = '/api/compose.one';
const P_DOMAIN_BY_COMPOSE = '/api/domain.byComposeId';

const MAX_TOKEN_FILE_BYTES = 8192;
const MAX_TOKEN_LEN = 4096;
const MAX_BODY_BYTES = 1_000_000;
const REQUEST_TIMEOUT_MS = 15_000;
const ACTION_DEADLINE_MS = 180_000;
const SOCKET_READY_MS = 15_000;

const E = Object.freeze({
  BAD_ACTION: 'E_BAD_ACTION', UNSAFE_RUNTIME: 'E_UNSAFE_RUNTIME',
  TOKEN_MISSING: 'E_TOKEN_MISSING', TOKEN_PERMS: 'E_TOKEN_PERMS',
  TOKEN_ENCODING: 'E_TOKEN_ENCODING', TOKEN_SHAPE: 'E_TOKEN_SHAPE',
  SSH_CONTEXT: 'E_SSH_CONTEXT', FORWARD_FAILED: 'E_FORWARD_FAILED',
  NETWORK: 'E_NETWORK', DEADLINE: 'E_DEADLINE',
  AUTH_REJECTED: 'E_AUTH_REJECTED', REDIRECT_REFUSED: 'E_REDIRECT_REFUSED',
  UPSTREAM_STATUS: 'E_UPSTREAM_STATUS', BODY_TOO_LARGE: 'E_BODY_TOO_LARGE',
  BAD_ENCODING: 'E_BAD_ENCODING', BAD_SHAPE: 'E_BAD_SHAPE',
  PROJECT_MISMATCH: 'E_PROJECT_MISMATCH', ORG_MISMATCH: 'E_ORG_MISMATCH',
  NO_STAGING_ENV: 'E_NO_STAGING_ENV', MULTI_STAGING_ENV: 'E_MULTI_STAGING_ENV',
  PROD_ID_REFUSED: 'E_PROD_ID_REFUSED', IMAGE_NOT_PINNED: 'E_IMAGE_NOT_PINNED',
  COMPOSE_CONFLICT: 'E_COMPOSE_CONFLICT', DOMAIN_CONFLICT: 'E_DOMAIN_CONFLICT',
  INTERNAL: 'E_INTERNAL',
});
const OK_RECEIPT = 'XEROX_QUIZ_STAGING_PROVISIONED';

class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}

// ── Runtime guard ───────────────────────────────────────────────────────
function classifyRuntime(st) {
  const env = st.env || {};
  if (env.NODE_OPTIONS || env.NODE_DEBUG || env.NODE_DEBUG_NATIVE || env.NODE_USE_ENV_PROXY) {
    throw new Fail(E.UNSAFE_RUNTIME);
  }
  if ((st.execArgv || []).some((a) => /^--(inspect|require|import|experimental-loader)/.test(a))) {
    throw new Fail(E.UNSAFE_RUNTIME);
  }
  if (!st.globalAgentIsStock) throw new Fail(E.UNSAFE_RUNTIME);
  return true;
}
function assertSafeRuntime() {
  return classifyRuntime({
    env: process.env, execArgv: process.execArgv,
    globalAgentIsStock: http.globalAgent && http.globalAgent.keepAlive === false,
  });
}

// ── Pinned token read (unchanged policy from the PASSed probe) ──────────
function classifyOwnedDir(st) {
  return Boolean(st) && st.isDirectory() && st.uid === ACCOUNT.uid && (st.mode & 0o077) === 0;
}
function assertOwnedDir(path, code) {
  let st;
  try { st = lstatSync(path); } catch { throw new Fail(code); }
  if (!classifyOwnedDir(st)) throw new Fail(code);
  return true;
}
function parseTokenFile(text) {
  if (typeof text !== 'string' || text.length === 0) throw new Fail(E.TOKEN_SHAPE);
  if (text.charCodeAt(0) === 0xfeff) throw new Fail(E.TOKEN_SHAPE);
  const lines = text.split('\n').filter((l, i, a) => !(i === a.length - 1 && l === ''));
  if (lines.length !== 1) throw new Fail(E.TOKEN_SHAPE);
  const eq = lines[0].indexOf('=');
  if (eq <= 0) throw new Fail(E.TOKEN_SHAPE);
  if (lines[0].slice(0, eq) !== TOKEN_KEY) throw new Fail(E.TOKEN_SHAPE);
  const value = lines[0].slice(eq + 1);
  if (value.length === 0 || value.length > MAX_TOKEN_LEN) throw new Fail(E.TOKEN_SHAPE);
  if (!/^[\x21-\x7e]+$/.test(value)) throw new Fail(E.TOKEN_SHAPE);
  return value;
}
function readToken() {
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
  } finally { try { closeSync(fd); } catch { /* cleanup only */ } }
}
export { classifyOwnedDir, parseTokenFile, classifyRuntime, assertSafeRuntime, Fail, E,
         OK_RECEIPT, QUIZ_PROJECT_ID, QUIZ_ORG_ID, PROD_DENYLIST, STAGING_ENV_NAME };


// ── SSH context + single forward (policy identical to the PASSed probe) ──
function assertSshContext() {
  for (const p of [SSH_IDENTITY, SSH_KNOWN_HOSTS]) {
    let st;
    try { st = lstatSync(p); } catch { throw new Fail(E.SSH_CONTEXT); }
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0) {
      throw new Fail(E.SSH_CONTEXT);
    }
  }
  return true;
}

const SSH_ARGS = [
  // ClearAllForwardings stays ABSENT: =yes also clears the -L below (verified
  // on this host via ssh -G), so ssh would authenticate and never create the
  // socket. -F none is the real control against config-injected forwardings.
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

function openForward() {
  mkdirSync(TEMP_PARENT, { recursive: true, mode: 0o700 });
  assertOwnedDir(TEMP_PARENT, E.FORWARD_FAILED);
  const dir = mkdtempSync(join(TEMP_PARENT, 'dkprov-'));
  if (!classifyOwnedDir(lstatSync(dir))) {
    rmSync(dir, { recursive: true, force: true });
    throw new Fail(E.FORWARD_FAILED);
  }
  const sock = join(dir, 's');
  const child = spawn(SSH_BIN,
    [...SSH_ARGS, '-L', `${sock}:${REMOTE_ADDR}`, `${SSH_USER}@${SSH_HOST}`],
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

// ── Bounded response handling ───────────────────────────────────────────
function classifyStatus(status) {
  if (status === 401 || status === 403) throw new Fail(E.AUTH_REJECTED);
  if (status >= 300 && status < 400) throw new Fail(E.REDIRECT_REFUSED);
  if (status < 200 || status >= 300) throw new Fail(E.UPSTREAM_STATUS); // never reflect the code
  return true;
}
function parseBounded(text) {
  if (typeof text !== 'string' || text.length > MAX_BODY_BYTES) throw new Fail(E.BODY_TOO_LARGE);
  try { return JSON.parse(text); } catch { throw new Fail(E.BAD_SHAPE); } // never echo the body
}
function consumeBoundedStream(res, cap) {
  return new Promise((resolve, reject) => {
    const chunks = []; let total = 0; let done = false;
    const fail = (e) => { if (done) return; done = true; res.destroy(); reject(e); };
    res.on('data', (c) => {
      total += c.length;
      if (total > cap) { fail(new Fail(E.BODY_TOO_LARGE)); return; }
      chunks.push(c);
    });
    res.on('error', () => fail(new Fail(E.NETWORK)));
    res.on('end', () => {
      if (done) return; done = true;
      try { resolve(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks))); }
      catch { reject(new Fail(E.BAD_ENCODING)); }
    });
  });
}
function withAbsoluteDeadline({ start, abort, ms }) {
  return new Promise((resolve, reject) => {
    let settled = false; let timer = null;
    const settle = (fn, arg) => {
      if (settled) return; settled = true;
      if (timer) { clearTimeout(timer); timer = null; }
      fn(arg);
    };
    timer = setTimeout(() => {
      if (settled) return; settled = true;
      if (timer) { clearTimeout(timer); timer = null; }
      try { if (abort) abort(); } catch { /* must not mask the deadline */ }
      reject(new Fail(E.DEADLINE));
    }, ms);
    try { start((v) => settle(resolve, v), (e) => settle(reject, e)); }
    catch (e) { settle(reject, e); }
  });
}

// Pure: the exact wire shape of every call. Method and path come from the
// fixed table above; there is no caller-supplied path.
function buildRequestOptions({ socketPath, token, path, method, bodyJson }) {
  const headers = { accept: 'application/json', 'x-api-key': token };
  if (bodyJson !== undefined) {
    headers['content-type'] = 'application/json';
    headers['content-length'] = Buffer.byteLength(bodyJson).toString();
  }
  return { socketPath, method, path, headers, timeout: REQUEST_TIMEOUT_MS };
}

function requestOnce({ socketPath, token, path, method, body }) {
  const bodyJson = body === undefined ? undefined : JSON.stringify(body);
  let active = null;
  return withAbsoluteDeadline({
    ms: REQUEST_TIMEOUT_MS,
    abort: () => { try { if (active) active.destroy(); } catch { /* best effort */ } },
    start: (ok, bad) => {
      const agent = new http.Agent({ keepAlive: false, maxSockets: 1 });
      const req = http.request(
        { ...buildRequestOptions({ socketPath, token, path, method, bodyJson }), agent },
        (res) => {
          try { classifyStatus(res.statusCode); }
          catch (e) { res.destroy(); bad(e); return; }
          consumeBoundedStream(res, MAX_BODY_BYTES).then(
            (text) => { try { ok(parseBounded(text)); } catch (e) { bad(e); } }, bad);
        });
      active = req;
      req.on('timeout', () => { req.destroy(); bad(new Fail(E.NETWORK)); });
      req.on('error', () => bad(new Fail(E.NETWORK)));  // cause never surfaced
      if (bodyJson !== undefined) req.write(bodyJson);
      req.end();
    },
  });
}

// ── Gate 1: identity + staging binding, the SOLE read ───────────────────
// Refuses any production identifier appearing anywhere in the projection.
function assertNoProdIds(values) {
  for (const v of values) {
    if (typeof v === 'string' && PROD_DENYLIST.includes(v)) throw new Fail(E.PROD_ID_REFUSED);
  }
  return true;
}

// Projects ONLY identifiers and the environment's server-declared name. Never
// reads, returns or inspects environment VALUES.
function projectStagingBinding(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.projectId !== QUIZ_PROJECT_ID) throw new Fail(E.PROJECT_MISMATCH);
  if (body.organizationId !== QUIZ_ORG_ID) throw new Fail(E.ORG_MISMATCH);

  const envs = body.environments;
  if (!Array.isArray(envs)) throw new Fail(E.BAD_SHAPE);

  const projected = envs.map((e) => {
    if (!e || typeof e !== 'object') throw new Fail(E.BAD_SHAPE);
    const id = e.environmentId;
    const name = e.name;
    if (typeof id !== 'string' || typeof name !== 'string') throw new Fail(E.BAD_SHAPE);
    const composes = Array.isArray(e.compose) ? e.compose : [];
    return {
      environmentId: id,
      name,
      composes: composes.map((c) => ({
        composeId: typeof c?.composeId === 'string' ? c.composeId : null,
        appName: typeof c?.appName === 'string' ? c.appName : null,
      })),
    };
  });

  assertNoProdIds(projected.flatMap((e) =>
    [e.environmentId, ...e.composes.flatMap((c) => [c.composeId, c.appName])]));

  const matches = projected.filter((e) => e.name.toLowerCase() === STAGING_ENV_NAME);
  // Zero or multiple is a hard stop. Never guess, never fall back to production.
  if (matches.length === 0) throw new Fail(E.NO_STAGING_ENV);
  if (matches.length > 1) throw new Fail(E.MULTI_STAGING_ENV);

  const target = matches[0];
  const existing = target.composes.find((c) => c.appName === COMPOSE_APPNAME
    || (c.appName && c.appName.startsWith(COMPOSE_APPNAME + '-')));
  return {
    environmentId: target.environmentId,
    environmentCount: projected.length,
    existingComposeId: existing ? existing.composeId : null,
  };
}

// The compose and environment rows both carry an `env` column holding secret
// values. These projections read an explicit ALLOWLIST of structural fields and
// never touch `env`, so no secret can reach a comparison, a log or an error.
const COMPOSE_FIELDS = Object.freeze(
  ['composeId', 'appName', 'sourceType', 'repository', 'owner', 'branch', 'composePath']);

function projectComposeState(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  const out = {};
  for (const k of COMPOSE_FIELDS) {
    const v = body[k];
    if (v !== null && v !== undefined && typeof v !== 'string') throw new Fail(E.BAD_SHAPE);
    out[k] = v ?? null;
  }
  assertNoProdIds([out.composeId, out.appName]);
  return out;
}

// Exact-match => verified no-op. Anything partial or conflicting stops.
function composeMatchesTarget(st) {
  return st.sourceType === 'github' && st.repository === REPO_NAME
    && st.owner === REPO_OWNER && st.branch === INFRA_BRANCH
    && st.composePath === COMPOSE_PATH;
}

const DOMAIN_FIELDS = Object.freeze(
  ['domainId', 'host', 'port', 'https', 'path', 'serviceName', 'certificateType',
   'domainType', 'composeId', 'enabled']);

function projectDomainRows(rows) {
  if (!Array.isArray(rows)) throw new Fail(E.BAD_SHAPE);
  return rows.map((r) => {
    if (!r || typeof r !== 'object') throw new Fail(E.BAD_SHAPE);
    const out = {};
    for (const k of DOMAIN_FIELDS) out[k] = r[k] ?? null;
    assertNoProdIds([out.composeId]);
    return out;
  });
}

function domainMatchesTarget(d) {
  return d.host === DOMAIN_HOST && d.port === DOMAIN_PORT
    && d.serviceName === DOMAIN_SERVICE && d.https === true
    && d.path === '/' && d.certificateType === 'letsencrypt'
    && d.domainType === 'compose';
}

// Returns 'create' (no domain yet), 'noop' (exactly our domain already), or
// throws on conflict. Never returns 'create' on unknown state.
function classifyDomainState(rows) {
  const projected = projectDomainRows(rows);
  if (projected.length === 0) return 'create';
  const ours = projected.filter((d) => d.host === DOMAIN_HOST);
  if (ours.length === 1 && projected.length === 1 && domainMatchesTarget(ours[0])) return 'noop';
  throw new Fail(E.DOMAIN_CONFLICT);
}

// ── Synthetic staging secrets: generated here, NEVER printed ────────────
// Delivered to the operator by writing one 0600 file; the value never enters
// stdout, stderr, an error, or this process's own logs.
function generateStagingEnv() {
  const pw = () => randomBytes(24).toString('base64url');
  return {
    QUIZ_DB_PASSWORD: pw(),
    SECRET_KEY: pw(),
    ADMIN_USERNAME: 'staging-admin@incluir.test',
    ADMIN_PASSWORD: pw(),
    STAGING_HONO_IMAGE: PINNED_HONO_IMAGE,
    STAGING_HONO_DB_PASSWORD: pw(),
    STAGING_BETTER_AUTH_SECRET: pw(),
    // TRUSTED_PROXY_CIDRS deliberately UNSET: empty is deny-all. It is set only
    // after the exact Traefik peer /32 or /128 has been OBSERVED on the running
    // service. A range such as 10.0.1.0/24 is forbidden by contract.
  };
}
function envToBlock(env) {
  return Object.entries(env).map(([k, v]) => `${k}=${v}`).join('\n');
}
function writeCredentialFile(env) {
  assertOwnedDir(TEMP_PARENT, E.TOKEN_PERMS);
  const path = join(TEMP_PARENT, 'quiz-staging-fixture.env');
  writeFileSync(path, envToBlock(env) + '\n', { mode: 0o600, flag: 'wx' });
  return path;  // path only; never the contents
}
export { projectComposeState, composeMatchesTarget, projectDomainRows,
         domainMatchesTarget, classifyDomainState, COMPOSE_FIELDS, DOMAIN_FIELDS,
         P_COMPOSE_ONE, P_DOMAIN_BY_COMPOSE,
         assertSshContext, SSH_ARGS, buildRequestOptions, projectStagingBinding,
         assertNoProdIds, classifyStatus, parseBounded, consumeBoundedStream,
         withAbsoluteDeadline, envToBlock, generateStagingEnv,
         COMPOSE_APPNAME, DOMAIN_HOST, DOMAIN_PORT, DOMAIN_SERVICE, INFRA_BRANCH,
         INFRA_REV, PINNED_HONO_IMAGE, PINNED_HONO_IMAGE_ID, PINNED_HONO_SOURCE_REV,
         P_PROJECT_ONE, P_COMPOSE_CREATE,
         P_COMPOSE_UPDATE, P_DOMAIN_CREATE, P_COMPOSE_DEPLOY };


// ── Fixed mutation bodies. Every field is pinned here; nothing is caller-fed.
function bodyComposeCreate(environmentId) {
  return {
    name: COMPOSE_NAME, appName: COMPOSE_APPNAME, environmentId,
    composeType: 'docker-compose', composePath: COMPOSE_PATH, sourceType: 'github',
  };
}
function bodyComposeSource(composeId) {
  return {
    composeId, repository: REPO_NAME, owner: REPO_OWNER, branch: INFRA_BRANCH,
    githubId: GITHUB_ID, sourceType: 'github', composePath: COMPOSE_PATH,
  };
}
function bodyComposeEnv(composeId, envBlock) {
  return { composeId, env: envBlock };
}
function bodyDomainCreate(composeId) {
  return {
    composeId, host: DOMAIN_HOST, https: true, port: DOMAIN_PORT,
    serviceName: DOMAIN_SERVICE, certificateType: 'letsencrypt',
    path: '/', domainType: 'compose',
  };
}
function bodyComposeDeploy(composeId) {
  return { composeId };
}

function extractComposeId(body) {
  const id = body && typeof body === 'object'
    ? (typeof body.composeId === 'string' ? body.composeId : null) : null;
  if (!id) throw new Fail(E.BAD_SHAPE);
  assertNoProdIds([id]);
  return id;
}

// ── The single fixed-purpose action, with injected seams for testing ─────
// Order is: read gate -> [create] -> source -> env -> domain -> deploy.
// No mutation may precede the identity and staging-environment binding.
async function orchestrate({
  assertContextFn, readTokenFn, openForwardFn, requestFn,
  envFn = generateStagingEnv, writeCredsFn = writeCredentialFile,
  deadlineMs = ACTION_DEADLINE_MS,
}) {
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
      const call = (path, method, body) =>
        requestFn({ socketPath, token, path, method, body });

      // GATE 1 — the sole read. Binds identity and exactly one staging env.
      const binding = projectStagingBinding(
        await call(`${P_PROJECT_ONE}?projectId=${QUIZ_PROJECT_ID}`, 'GET', undefined));

      // Idempotency preflight from AUTHORITATIVE state, before any mutation.
      let composeId = binding.existingComposeId;
      const created = composeId === null;
      if (!created) {
        // A service already exists: prove it is exactly ours before touching it.
        const st = projectComposeState(
          await call(`${P_COMPOSE_ONE}?composeId=${composeId}`, 'GET', undefined));
        if (!composeMatchesTarget(st)) throw new Fail(E.COMPOSE_CONFLICT);
      } else {
        composeId = extractComposeId(
          await call(P_COMPOSE_CREATE, 'POST', bodyComposeCreate(binding.environmentId)));
      }
      assertNoProdIds([composeId]);

      await call(P_COMPOSE_UPDATE, 'POST', bodyComposeSource(composeId));

      const env = envFn();
      if (env.STAGING_HONO_IMAGE !== PINNED_HONO_IMAGE) throw new Fail(E.IMAGE_NOT_PINNED);
      if ('TRUSTED_PROXY_CIDRS' in env) throw new Fail(E.BAD_SHAPE); // must stay unset
      await call(P_COMPOSE_UPDATE, 'POST', bodyComposeEnv(composeId, envToBlock(env)));

      // Domain state from the authoritative read, never inferred.
      const domainAction = classifyDomainState(
        await call(`${P_DOMAIN_BY_COMPOSE}?composeId=${composeId}`, 'GET', undefined));
      if (domainAction === 'create') {
        await call(P_DOMAIN_CREATE, 'POST', bodyDomainCreate(composeId));
      }

      await call(P_COMPOSE_DEPLOY, 'POST', bodyComposeDeploy(composeId));

      const credPath = writeCredsFn(env);
      return { composeId, environmentId: binding.environmentId, credPath,
               reusedExisting: !created, domainAction };
    })();
    work.catch(() => { /* a late loser must not raise an unhandled rejection */ });
    const out = await Promise.race([work, budget]);
    return out;
  } finally {
    if (timer) clearTimeout(timer);
    fwd.cleanup();   // unconditional
  }
}

async function main() {
  if (process.argv.slice(2).length !== 0) {
    process.stdout.write(E.BAD_ACTION + '\n');
    process.exitCode = 2;
    return;
  }
  try {
    assertSafeRuntime();
    const r = await orchestrate({
      assertContextFn: assertSshContext,
      readTokenFn: readToken,
      openForwardFn: openForward,
      requestFn: requestOnce,
    });
    // Fixed receipt plus the minimum non-secret IDs evidencing the binding.
    process.stdout.write(`${OK_RECEIPT}\n`);
    process.stdout.write(`environmentId=${r.environmentId}\n`);
    process.stdout.write(`composeId=${r.composeId}\n`);
    process.stdout.write(`fixture_file=${r.credPath}\n`);
  } catch (err) {
    process.stdout.write((err instanceof Fail ? err.code : E.INTERNAL) + '\n');
    process.exitCode = 1;
  }
}

export { orchestrate, bodyComposeCreate, bodyComposeSource, bodyComposeEnv,
         bodyDomainCreate, bodyComposeDeploy, extractComposeId };

if (import.meta.url === `file://${process.argv[1]}`) { await main(); }
