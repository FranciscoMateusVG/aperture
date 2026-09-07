/**
 * ONE fixed-purpose staging provisioning action for the Xerox Quiz project.
 * aperture-ztid5. NOT a generic request broker: every endpoint, method, order,
 * identifier and body field below is fixed at authoring time. There is no
 * caller-supplied path, body, host or action.
 *
 * Gate 1 is project.one: it binds identity (exact projectId AND organizationId)
 * and selects exactly one staging environment by the server's own declared
 * environment name. Zero or multiple matches is a hard stop -- never a guess,
 * and never a fallback to production. Two further fixed read-only endpoints,
 * compose.one and domain.byComposeId, establish the state project.one cannot
 * prove; their procedure names and field allowlists were read from the running
 * Dokploy v0.30.2 build. Every addressable state is read BEFORE the mutations.
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
import { openSync, fstatSync, lstatSync, readSync, writeSync, closeSync, fsyncSync,
         mkdirSync, mkdtempSync, rmSync, linkSync } from 'node:fs';
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
  ESCROW_CORRUPT: 'E_ESCROW_CORRUPT', ESCROW_WRITE: 'E_ESCROW_WRITE',
  ESCROW_MISSING: 'E_ESCROW_MISSING', ADOPT_AMBIGUOUS: 'E_ADOPT_AMBIGUOUS',
  IDENTITY_UNPROVEN: 'E_IDENTITY_UNPROVEN',
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

// ── Gate 1: identity + staging environment binding ───────────────────────
// Refuses any production identifier appearing anywhere in the projection.
function assertNoProdIds(values) {
  for (const v of values) {
    if (typeof v === 'string' && PROD_DENYLIST.includes(v)) throw new Fail(E.PROD_ID_REFUSED);
  }
  return true;
}

// Projects ONLY identifiers and the environment's server-declared name. Never
// reads, returns or inspects environment VALUES (the environment row carries
// its own `env` column holding secrets).
//
// The `compose` relation name and its {composeId, appName} columns were read
// from the RUNNING Dokploy v0.30.2 build, not assumed. A missing or non-array
// relation is a HARD STOP, never an empty list: treating unknown state as
// "absent" is what would let this create a duplicate over a live service.
function projectStagingBinding(body) {
  if (!body || typeof body !== 'object') throw new Fail(E.BAD_SHAPE);
  if (body.projectId !== QUIZ_PROJECT_ID) throw new Fail(E.PROJECT_MISMATCH);
  if (body.organizationId !== QUIZ_ORG_ID) throw new Fail(E.ORG_MISMATCH);

  const envs = body.environments;
  if (!Array.isArray(envs) || envs.length === 0) throw new Fail(E.BAD_SHAPE);

  const projected = envs.map((e) => {
    if (!e || typeof e !== 'object') throw new Fail(E.BAD_SHAPE);
    const id = e.environmentId;
    const name = e.name;
    if (typeof id !== 'string' || id.length === 0) throw new Fail(E.BAD_SHAPE);
    if (typeof name !== 'string' || name.length === 0) throw new Fail(E.BAD_SHAPE);
    if (!Array.isArray(e.compose)) throw new Fail(E.BAD_SHAPE);  // never default to []
    const composes = e.compose.map((c) => {
      if (!c || typeof c !== 'object') throw new Fail(E.BAD_SHAPE);
      if (typeof c.composeId !== 'string' || c.composeId.length === 0) throw new Fail(E.BAD_SHAPE);
      if (typeof c.appName !== 'string' || c.appName.length === 0) throw new Fail(E.BAD_SHAPE);
      return { composeId: c.composeId, appName: c.appName };
    });
    return { environmentId: id, name, composes };
  });

  assertNoProdIds(projected.flatMap((e) =>
    [e.environmentId, ...e.composes.flatMap((c) => [c.composeId, c.appName])]));

  const matches = projected.filter((e) => e.name.toLowerCase() === STAGING_ENV_NAME);
  if (matches.length === 0) throw new Fail(E.NO_STAGING_ENV);
  if (matches.length > 1) throw new Fail(E.MULTI_STAGING_ENV);
  return { environmentId: matches[0].environmentId,
           environmentCount: projected.length,
           composes: matches[0].composes };
}

// Selects the remote object to act on, WITHOUT guessing.
//  - a durable binding from a previous run wins, matched by exact composeId;
//  - otherwise the staging environment must contain NO service whose appName
//    is ours or is prefixed with ours. Dokploy appends a random suffix to
//    appName on create, so a prefix hit means a prior run already created one
//    and we lost the binding -- ambiguous, therefore a stop, never a create.
function selectTarget(binding, priorBinding) {
  if (priorBinding) {
    const hits = binding.composes.filter((c) => c.composeId === priorBinding.composeId);
    if (hits.length !== 1) throw new Fail(E.COMPOSE_CONFLICT);
    return { composeId: hits[0].composeId, create: false, adopt: false };
  }
  const collide = binding.composes.filter((c) => c.appName === COMPOSE_APPNAME
    || c.appName.startsWith(COMPOSE_APPNAME + '-'));
  if (collide.length === 0) return { composeId: null, create: true, adopt: false };
  // RECOVERY, narrowly scoped: compose.create is a remote side effect that
  // precedes saveBinding, so a crash in that window leaves a suffixed orphan
  // and no local binding. Exactly one candidate may be ADOPTED, and only after
  // compose.one proves it sits in the selected staging environment and still
  // has an allowed shape. More than one candidate is unresolvable.
  if (collide.length !== 1) throw new Fail(E.ADOPT_AMBIGUOUS);
  return { composeId: collide[0].composeId, create: false, adopt: true };
}

// An adoptable row is either an untouched create stub (source fields still
// unset) or already exactly our target. Anything else is someone else's object.
function assertAdoptable(st) {
  const stub = !st.repository && !st.owner && !st.branch;
  if (stub || composeMatchesTarget(st)) return true;
  throw new Fail(E.COMPOSE_CONFLICT);
}

// The compose and environment rows both carry an `env` column holding secret
// values. These projections read an explicit ALLOWLIST of structural fields and
// never touch `env`, so no secret can reach a comparison, a log or an error.
const COMPOSE_FIELDS = Object.freeze(
  ['composeId', 'appName', 'environmentId', 'sourceType', 'repository', 'owner',
   'branch', 'composePath']);

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

// OWNERSHIP proof, distinct from configuration match. A finite prod denylist is
// defense-in-depth, not evidence that a returned id belongs to us: this binds
// the row to the exact id requested AND to the staging environment selected at
// gate 1, so an arbitrary non-denylisted id can never be mutated.
function assertComposeIdentity(st, expectedComposeId, expectedEnvironmentId) {
  if (st.composeId !== expectedComposeId) throw new Fail(E.IDENTITY_UNPROVEN);
  if (st.environmentId !== expectedEnvironmentId) throw new Fail(E.IDENTITY_UNPROVEN);
  if (typeof st.appName !== 'string' || st.appName.length === 0) {
    throw new Fail(E.IDENTITY_UNPROVEN);
  }
  // Dokploy may append a random suffix to appName at create time.
  if (st.appName !== COMPOSE_APPNAME && !st.appName.startsWith(COMPOSE_APPNAME + '-')) {
    throw new Fail(E.IDENTITY_UNPROVEN);
  }
  return true;
}

// Exact configuration match => verified no-op. Partial or conflicting stops.
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

// 'create' (no domain yet), 'noop' (exactly ours and enabled), else throws.
// Never returns 'create' for unknown state.
function classifyDomainState(rows, expectedComposeId) {
  if (typeof expectedComposeId !== 'string' || expectedComposeId.length === 0) {
    throw new Fail(E.IDENTITY_UNPROVEN);
  }
  const projected = projectDomainRows(rows);
  // Every row returned for our composeId query must actually BE ours.
  for (const d of projected) {
    if (d.composeId !== expectedComposeId) throw new Fail(E.IDENTITY_UNPROVEN);
  }
  if (projected.length === 0) return 'create';
  if (projected.length === 1 && domainMatchesTarget(projected[0])
      && projected[0].enabled === true) {
    return 'noop';                       // a DISABLED row is not a no-op
  }
  throw new Fail(E.DOMAIN_CONFLICT);
}

// ── Synthetic staging secrets: escrowed BEFORE the first mutation ───────
// A retry must never regenerate. Regenerating after a remote side effect
// would rotate auth material and strand the existing Postgres volumes, so the
// set is created once, published atomically, and strictly reused thereafter.
const ESCROW_PATH = join(TEMP_PARENT, 'quiz-staging-fixture.env');
const BINDING_PATH = join(TEMP_PARENT, 'quiz-staging-binding.json');
const ESCROW_KEYS = Object.freeze([
  'QUIZ_DB_PASSWORD', 'SECRET_KEY', 'ADMIN_USERNAME', 'ADMIN_PASSWORD',
  'STAGING_HONO_IMAGE', 'STAGING_HONO_DB_PASSWORD', 'STAGING_BETTER_AUTH_SECRET',
]);

function generateStagingEnv() {
  const pw = () => randomBytes(24).toString('base64url');
  return {
    QUIZ_DB_PASSWORD: pw(), SECRET_KEY: pw(),
    ADMIN_USERNAME: 'staging-admin@incluir.test', ADMIN_PASSWORD: pw(),
    STAGING_HONO_IMAGE: PINNED_HONO_IMAGE_ID,
    STAGING_HONO_DB_PASSWORD: pw(), STAGING_BETTER_AUTH_SECRET: pw(),
    // TRUSTED_PROXY_CIDRS stays absent: empty is deny-all until the exact
    // Traefik peer /32 or /128 has been OBSERVED on the running service.
  };
}
function envToBlock(env) {
  return ESCROW_KEYS.map((k) => `${k}=${env[k]}`).join('\n');
}
const MAX_ESCROW_VALUE = 512;
const MAX_ESCROW_BYTES = 8192;

// Strict exactly-once grammar. Duplicate keys are a HARD ERROR: a second
// SECRET_KEY line previously overwrote the first, so an appended line could
// silently substitute a secret.
function parseEscrowBlock(text) {
  if (typeof text !== 'string' || text.length === 0 || text.length > MAX_ESCROW_BYTES) {
    throw new Fail(E.ESCROW_CORRUPT);
  }
  const lines = text.split('\n');
  if (lines[lines.length - 1] !== '') throw new Fail(E.ESCROW_CORRUPT); // exactly one trailing LF
  lines.pop();
  if (lines.length !== ESCROW_KEYS.length) throw new Fail(E.ESCROW_CORRUPT);
  const out = Object.create(null);
  for (const line of lines) {
    const eq = line.indexOf('=');
    if (eq <= 0) throw new Fail(E.ESCROW_CORRUPT);
    const k = line.slice(0, eq);
    const v = line.slice(eq + 1);
    if (!ESCROW_KEYS.includes(k)) throw new Fail(E.ESCROW_CORRUPT);
    if (Object.prototype.hasOwnProperty.call(out, k)) throw new Fail(E.ESCROW_CORRUPT); // duplicate
    if (v.length === 0 || v.length > MAX_ESCROW_VALUE) throw new Fail(E.ESCROW_CORRUPT);
    if (!/^[\x20-\x7e]+$/.test(v)) throw new Fail(E.ESCROW_CORRUPT);  // no control bytes
    out[k] = v;
  }
  for (const k of ESCROW_KEYS) {
    if (!Object.prototype.hasOwnProperty.call(out, k)) throw new Fail(E.ESCROW_CORRUPT);
  }
  if (out.STAGING_HONO_IMAGE !== PINNED_HONO_IMAGE_ID) throw new Fail(E.IMAGE_NOT_PINNED);
  return out;
}

function readOwnedFile(path, code) {
  let fd;
  try { fd = openSync(path, FS.O_RDONLY | FS.O_NOFOLLOW); }
  catch (e) { if (e && e.code === 'ENOENT') return null; throw new Fail(code); }
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0
        || st.nlink !== 1 || st.size === 0 || st.size > MAX_TOKEN_FILE_BYTES) {
      throw new Fail(code);
    }
    const buf = Buffer.allocUnsafe(st.size);
    if (readSync(fd, buf, 0, st.size, 0) !== st.size) throw new Fail(code);
    try { return new TextDecoder('utf-8', { fatal: true }).decode(buf); }
    catch { throw new Fail(code); }
  } finally { try { closeSync(fd); } catch { /* cleanup only */ } }
}
// Durable atomic no-overwrite publication. The escrow is the state that
// recovery depends on, so it is fully written, fsynced, hard-linked into place
// (link fails if the destination exists, so a concurrent run cannot clobber an
// authoritative escrow), the DIRECTORY is fsynced so the link itself survives a
// crash, and the result is reopened and compared before it is trusted.
function publishFile(path, contents, code) {
  const buf = Buffer.from(contents, 'utf8');
  if (buf.length === 0 || buf.length > MAX_ESCROW_BYTES) throw new Fail(code);
  const tmp = `${path}.tmp-${randomBytes(8).toString('hex')}`;
  let fd;
  try {
    fd = openSync(tmp, FS.O_WRONLY | FS.O_CREAT | FS.O_EXCL | FS.O_NOFOLLOW, 0o600);
    let off = 0;
    while (off < buf.length) off += writeSync(fd, buf, off, buf.length - off);
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0
        || st.size !== buf.length) {
      throw new Fail(code);
    }
    fsyncSync(fd);                       // contents durable before it is named
  } catch { try { if (fd !== undefined) closeSync(fd); } catch { /* cleanup */ }
           try { rmSync(tmp, { force: true }); } catch { /* cleanup */ }
           throw new Fail(code); }
  try { closeSync(fd); } catch { /* cleanup only */ }
  try {
    linkSync(tmp, path);                 // fails if it already exists
    let dfd;
    try { dfd = openSync(TEMP_PARENT, FS.O_RDONLY); fsyncSync(dfd); }  // link durable
    finally { try { if (dfd !== undefined) closeSync(dfd); } catch { /* cleanup */ } }
  } catch { try { rmSync(tmp, { force: true }); } catch { /* cleanup */ }
           throw new Fail(code); }
  finally { try { rmSync(tmp, { force: true }); } catch { /* cleanup only */ } }
  // Reopen and verify what actually landed; never trust the write path alone.
  const back = readOwnedFile(path, code);
  if (back !== contents) throw new Fail(code);
  return back;
}

// The escrow DECISION is exported as a pure composition seam with NO defaults:
// every input must be injected, so an importer can exercise the logic but can
// never reach the real file. The live reader/publisher below stay private and
// are referenced only by main(), so no exported call can discover or return
// the real secret set.
function escrowDecision({ existingText, generateFn, publishFn }) {
  if (typeof generateFn !== 'function' || typeof publishFn !== 'function') {
    throw new Fail(E.BAD_SHAPE);
  }
  if (existingText !== null && existingText !== undefined) {
    return { env: parseEscrowBlock(existingText), reused: true };
  }
  const env = generateFn();
  const back = publishFn(envToBlock(env) + '\n');
  if (back === null || back === undefined) throw new Fail(E.ESCROW_WRITE);
  return { env: parseEscrowBlock(back), reused: false };   // trust only what landed
}

// PRIVATE. Never exported, never a default parameter of an exported function.
function loadOrCreateEscrow() {
  assertOwnedDir(TEMP_PARENT, E.TOKEN_PERMS);
  return escrowDecision({
    existingText: readOwnedFile(ESCROW_PATH, E.ESCROW_CORRUPT),
    generateFn: generateStagingEnv,
    publishFn: (text) => publishFile(ESCROW_PATH, text, E.ESCROW_WRITE),
  });
}

// Durable record of the remote object this run bound to, so a retry rebinds
// by exact composeId instead of guessing from a suffixed appName.
function loadBinding() {
  const t = readOwnedFile(BINDING_PATH, E.ESCROW_CORRUPT);
  if (t === null) return null;
  let v;
  try { v = JSON.parse(t); } catch { throw new Fail(E.ESCROW_CORRUPT); }
  if (!v || typeof v !== 'object' || Object.keys(v).length !== 1
      || typeof v.composeId !== 'string' || v.composeId.length === 0
      || v.composeId.length > 200 || !/^[\x21-\x7e]+$/.test(v.composeId)) {
    throw new Fail(E.ESCROW_CORRUPT);
  }
  assertNoProdIds([v.composeId]);
  return v;
}
function saveBinding(composeId) {
  if (typeof composeId !== 'string' || composeId.length === 0 || composeId.length > 200
      || !/^[\x21-\x7e]+$/.test(composeId)) {
    throw new Fail(E.ESCROW_WRITE);
  }
  assertOwnedDir(TEMP_PARENT, E.TOKEN_PERMS);
  publishFile(BINDING_PATH, JSON.stringify({ composeId }) + '\n', E.ESCROW_WRITE);
  return BINDING_PATH;
}

export { projectComposeState, composeMatchesTarget, assertComposeIdentity, selectTarget,
         assertAdoptable, escrowDecision, projectDomainRows,
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
  escrowFn, loadBindingFn, saveBindingFn,
  deadlineMs = ACTION_DEADLINE_MS,
}) {
  // No live defaults: every secret-capable seam must be supplied by the caller,
  // and only main() supplies the private live ones. An importer therefore
  // cannot reach the real escrow through this function.
  for (const fn of [assertContextFn, readTokenFn, openForwardFn, requestFn,
                    escrowFn, loadBindingFn, saveBindingFn]) {
    if (typeof fn !== 'function') throw new Fail(E.BAD_SHAPE);
  }
  assertContextFn();
  const token = readTokenFn();

  // Secrets are escrowed BEFORE any remote side effect and strictly reused on
  // every later invocation. Nothing below may regenerate them.
  const escrow = escrowFn();
  const env = escrow.env;
  if (env.STAGING_HONO_IMAGE !== PINNED_HONO_IMAGE_ID) throw new Fail(E.IMAGE_NOT_PINNED);
  if ('TRUSTED_PROXY_CIDRS' in env) throw new Fail(E.BAD_SHAPE);
  const priorBinding = loadBindingFn();
  // Coupling: a pre-existing remote object may only be touched with the SAME
  // escrow that created it. Binding present with a freshly generated escrow
  // means the escrow was lost, and proceeding would rotate the live stack's
  // DB and auth secrets.
  if (priorBinding && !escrow.reused) throw new Fail(E.ESCROW_MISSING);

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

      // ── READ PHASE ──────────────────────────────────────────────────
      // Gate 1: identity + exactly one staging environment.
      const binding = projectStagingBinding(
        await call(`${P_PROJECT_ONE}?projectId=${QUIZ_PROJECT_ID}`, 'GET', undefined));
      const target = selectTarget(binding, priorBinding);

      let composeId = target.composeId;
      const created = target.create;
      // Same coupling rule for an adopted orphan: it was created by a previous
      // run, so its secrets must be the escrowed ones, not a fresh set.
      if (!created && !escrow.reused) throw new Fail(E.ESCROW_MISSING);

      // Creating the stub is the only mutation allowed before the remaining
      // reads, and it is unavoidable: the object must exist to be read.
      if (created) {
        composeId = extractComposeId(
          await call(P_COMPOSE_CREATE, 'POST', bodyComposeCreate(binding.environmentId)));
        // Durably record the binding BEFORE anything else, so a crash here
        // still lets a retry rebind by exact id instead of creating again.
        saveBindingFn(composeId);
      }

      // Rebind the row we will mutate, on BOTH paths, and prove ownership.
      const st = projectComposeState(
        await call(`${P_COMPOSE_ONE}?composeId=${composeId}`, 'GET', undefined));
      assertComposeIdentity(st, composeId, binding.environmentId);
      if (target.adopt) {
        // Adopting a crash orphan: allowed shapes only, then persist the
        // binding BEFORE any mutation so the window cannot reopen.
        assertAdoptable(st);
        saveBindingFn(composeId);
      } else if (!created && !composeMatchesTarget(st)) {
        // A previously bound service must still be exactly our target.
        throw new Fail(E.COMPOSE_CONFLICT);
      }

      // Domain state is read BEFORE the update mutations, so a conflict on an
      // existing service is discovered with ZERO POSTs issued.
      const domainAction = classifyDomainState(
        await call(`${P_DOMAIN_BY_COMPOSE}?composeId=${composeId}`, 'GET', undefined),
        composeId);

      // ── MUTATION PHASE ──────────────────────────────────────────────
      await call(P_COMPOSE_UPDATE, 'POST', bodyComposeSource(composeId));
      await call(P_COMPOSE_UPDATE, 'POST', bodyComposeEnv(composeId, envToBlock(env)));
      if (domainAction === 'create') {
        await call(P_DOMAIN_CREATE, 'POST', bodyDomainCreate(composeId));
      }
      await call(P_COMPOSE_DEPLOY, 'POST', bodyComposeDeploy(composeId));

      return { composeId, environmentId: binding.environmentId,
               credPath: ESCROW_PATH, escrowReused: escrow.reused,
               reusedExisting: !created, domainAction };
    })();
    work.catch(() => { /* a late loser must not raise an unhandled rejection */ });
    return await Promise.race([work, budget]);
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
      // The private live seams are supplied HERE and nowhere else.
      escrowFn: loadOrCreateEscrow,
      loadBindingFn: loadBinding,
      saveBindingFn: saveBinding,
    });
    // Fixed receipt plus the minimum non-secret IDs evidencing the binding.
    process.stdout.write(`${OK_RECEIPT}\n`);
    process.stdout.write(`environmentId=${r.environmentId}\n`);
    process.stdout.write(`composeId=${r.composeId}\n`);
    process.stdout.write(`fixture_file=${r.credPath}\n`);
    process.stdout.write(`escrow_reused=${r.escrowReused}\n`);
    process.stdout.write(`domain_action=${r.domainAction}\n`);
  } catch (err) {
    process.stdout.write((err instanceof Fail ? err.code : E.INTERNAL) + '\n');
    process.exitCode = 1;
  }
}

export { orchestrate, bodyComposeCreate, bodyComposeSource, bodyComposeEnv,
         bodyDomainCreate, bodyComposeDeploy, extractComposeId,
         parseEscrowBlock, ESCROW_KEYS };

if (import.meta.url === `file://${process.argv[1]}`) { await main(); }
