#!/usr/bin/env node
/**
 * One-shot runner for the EXISTING seed-e2e.ts at the pinned monorepo commit.
 * It runs locally through two strict SSH loopback forwards whose remote targets
 * are validated as the isolated staging DB and Hono containers. Seed stdout
 * and stderr are discarded at the spawn boundary because the pinned script
 * prints its DATABASE_URL and disposable fixture credentials.
 */
import { closeSync, constants as FS, fstatSync, lstatSync, mkdirSync, mkdtempSync,
         openSync, readFileSync, readSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { userInfo } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { spawn, execFileSync } from 'node:child_process';
import net from 'node:net';

const ACCOUNT = userInfo();
const HOME = ACCOUNT.homedir;
const TEMP_PARENT = join(HOME, '.config', 'aperture');
const ESCROW_PATH = join(TEMP_PARENT, 'quiz-staging-fixture.env');
const MONOREPO = join(HOME, 'projects', 'monorepo-incluir');
const APP_MODULES = join(MONOREPO, 'apps', 'hono-app', 'node_modules');
const TSX_CLI = join(APP_MODULES, 'tsx', 'dist', 'cli.mjs');

const SOURCE_REV = '9cb605fc2cc63930a9fdf4f73ccec3ec05c6daad';
const SOURCE_PATH = 'apps/hono-app/scripts/seed-e2e.ts';
const SOURCE_SHA256 = '9b690ca203df6d26a43afee9eccf3316eef5d0fe6f4ffca49a08f564ccd926e8';
const TSX_SHA256 = '8729ecfb90d9d568939e4190e6f1d3317c946583b7d37a776e0c23a21c021cf8';
const NODE_VERSION = 'v24.15.0';

const NETWORK = 'quiz-incluir-staging-4400a18b9519d0bb-v2vvhj_quiz-staging-net';
const DB_CONTAINER = 'quiz-staging-hono-db';
const HONO_CONTAINER = 'quiz-staging-hono';
const HONO_IMAGE_ID = 'sha256:a88663c43ac045af03ed27d7d14199d476f5c8ae024d2128e3af56b6287079a7';
const DB_NAME = 'incluir';
const DB_USER = 'e2e';
const TABLE_COUNT = 67;
const LOCAL_DB_PORT = 45432;
const LOCAL_HONO_PORT = 45433;

const SSH_BIN = '/usr/bin/ssh';
const SSH_USER = 'ubuntu';
const SSH_HOST = '100.85.254.44';
const SSH_IDENTITY = join(HOME, '.ssh', 'id_ed25519');
const SSH_KNOWN_HOSTS = join(HOME, '.ssh', 'known_hosts');
const SSH_BASE = [
  '-F', 'none',
  '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=yes',
  '-o', 'PasswordAuthentication=no', '-o', 'KbdInteractiveAuthentication=no',
  '-o', 'PermitLocalCommand=no', '-o', 'ExitOnForwardFailure=yes',
  '-o', 'IdentitiesOnly=yes', '-o', `UserKnownHostsFile=${SSH_KNOWN_HOSTS}`,
  '-i', SSH_IDENTITY,
];

const ESCROW_KEYS = Object.freeze([
  'QUIZ_DB_PASSWORD', 'SECRET_KEY', 'ADMIN_USERNAME', 'ADMIN_PASSWORD',
  'STAGING_HONO_IMAGE', 'STAGING_HONO_DB_PASSWORD', 'STAGING_BETTER_AUTH_SECRET',
]);
const OK_RECEIPT = 'QUIZ_STAGING_SYNTHETIC_SEED_COMPLETE';
const E = Object.freeze({
  BAD_ACTION: 'E_BAD_ACTION', UNSAFE_RUNTIME: 'E_UNSAFE_RUNTIME',
  LOCAL_CONTEXT: 'E_LOCAL_CONTEXT', SSH_CONTEXT: 'E_SSH_CONTEXT',
  TOPOLOGY: 'E_TOPOLOGY', ESCROW: 'E_ESCROW',
  FORWARD: 'E_FORWARD', DB_BINDING: 'E_DB_BINDING',
  SOURCE: 'E_SOURCE', SEED: 'E_SEED', DEADLINE: 'E_DEADLINE',
  INTERNAL: 'E_INTERNAL',
});
class Fail extends Error {
  constructor(code) { super(code); this.code = code; }
}

function classifyRuntime({ env, execArgv, nodeVersion }) {
  const e = env ?? {};
  if (e.NODE_OPTIONS || e.NODE_DEBUG || e.NODE_DEBUG_NATIVE || e.NODE_USE_ENV_PROXY
      || (execArgv ?? []).length !== 0 || nodeVersion !== NODE_VERSION) {
    throw new Fail(E.UNSAFE_RUNTIME);
  }
  return true;
}
function assertSafeRuntime() {
  return classifyRuntime({ env: process.env, execArgv: process.execArgv,
                           nodeVersion: process.version });
}
function ownedDir(st) {
  return Boolean(st) && st.isDirectory() && st.uid === ACCOUNT.uid && (st.mode & 0o077) === 0;
}
function ownedSourceDir(st) {
  return Boolean(st) && st.isDirectory() && st.uid === ACCOUNT.uid && (st.mode & 0o022) === 0;
}
function assertOwnedDir(path, code) {
  let st; try { st = lstatSync(path); } catch { throw new Fail(code); }
  if (!ownedDir(st)) throw new Fail(code);
}
function assertOwnedFile(path, mask, code) {
  let st; try { st = lstatSync(path); } catch { throw new Fail(code); }
  if (!st.isFile() || st.isSymbolicLink() || st.uid !== ACCOUNT.uid
      || (st.mode & mask) !== 0 || st.size === 0 || st.nlink !== 1) throw new Fail(code);
  return st;
}

function parseEscrow(text) {
  if (typeof text !== 'string' || text.length === 0 || text.length > 8192) {
    throw new Fail(E.ESCROW);
  }
  const lines = text.split('\n');
  if (lines.at(-1) === '') lines.pop();
  if (lines.length !== ESCROW_KEYS.length) throw new Fail(E.ESCROW);
  const values = Object.create(null);
  for (const line of lines) {
    const eq = line.indexOf('=');
    if (eq <= 0) throw new Fail(E.ESCROW);
    const key = line.slice(0, eq); const value = line.slice(eq + 1);
    if (!ESCROW_KEYS.includes(key) || Object.hasOwn(values, key)
        || value.length === 0 || value.length > 512 || !/^[\x20-\x7e]+$/.test(value)) {
      throw new Fail(E.ESCROW);
    }
    values[key] = value;
  }
  for (const key of ESCROW_KEYS) if (!Object.hasOwn(values, key)) throw new Fail(E.ESCROW);
  if (values.STAGING_HONO_IMAGE !== HONO_IMAGE_ID) throw new Fail(E.ESCROW);
  return values.STAGING_HONO_DB_PASSWORD;
}
function readEscrowPassword() {
  assertOwnedDir(TEMP_PARENT, E.ESCROW);
  const pre = assertOwnedFile(ESCROW_PATH, 0o077, E.ESCROW);
  if (pre.size > 8192) throw new Fail(E.ESCROW);
  let fd;
  try { fd = openSync(ESCROW_PATH, FS.O_RDONLY | FS.O_NOFOLLOW); }
  catch { throw new Fail(E.ESCROW); }
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.uid !== ACCOUNT.uid || (st.mode & 0o077) !== 0
        || st.nlink !== 1 || st.size !== pre.size || st.ino !== pre.ino || st.dev !== pre.dev) {
      throw new Fail(E.ESCROW);
    }
    const buf = Buffer.allocUnsafe(st.size);
    if (readSync(fd, buf, 0, st.size, 0) !== st.size) throw new Fail(E.ESCROW);
    let text; try { text = new TextDecoder('utf-8', { fatal: true }).decode(buf); }
    catch { throw new Fail(E.ESCROW); }
    return parseEscrow(text);
  } finally { try { closeSync(fd); } catch { /* cleanup only */ } }
}

function assertLocalContext() {
  // Source is non-secret and conventionally 0755, but must remain owned by the
  // executor and not group/other-writable.
  for (const path of [MONOREPO, join(MONOREPO, 'apps'),
    join(MONOREPO, 'apps', 'hono-app'), APP_MODULES]) {
    let st; try { st = lstatSync(path); } catch { throw new Fail(E.LOCAL_CONTEXT); }
    if (!ownedSourceDir(st)) throw new Fail(E.LOCAL_CONTEXT);
  }
  assertOwnedFile(TSX_CLI, 0o022, E.LOCAL_CONTEXT);
  const hash = createHash('sha256').update(readFileSync(TSX_CLI)).digest('hex');
  if (hash !== TSX_SHA256) throw new Fail(E.LOCAL_CONTEXT);
  const versions = { bcrypt: '5.1.1', kysely: '0.28.16', pg: '8.20.0', tsx: '4.21.0' };
  for (const [pkg, version] of Object.entries(versions)) {
    let body;
    try { body = JSON.parse(readFileSync(join(APP_MODULES, pkg, 'package.json'), 'utf8')); }
    catch { throw new Fail(E.LOCAL_CONTEXT); }
    if (body.version !== version) throw new Fail(E.LOCAL_CONTEXT);
  }
}
function assertSshContext() {
  assertOwnedFile(SSH_IDENTITY, 0o077, E.SSH_CONTEXT);
  assertOwnedFile(SSH_KNOWN_HOSTS, 0o022, E.SSH_CONTEXT);
}

const REMOTE_TOPOLOGY_PROGRAM = [
  'import json,subprocess',
  'def one(n):',
  ' x=json.loads(subprocess.check_output(["/usr/bin/docker","inspect",n],stderr=subprocess.DEVNULL))[0]',
  ' return {"running":x["State"]["Running"],"image":x["Image"],"networks":{k:v["IPAddress"] for k,v in x["NetworkSettings"]["Networks"].items()}}',
  `print(json.dumps({"db":one("${DB_CONTAINER}"),"hono":one("${HONO_CONTAINER}")},separators=(',',':')))`,
].join('\n');

function parseTopology(text) {
  if (typeof text !== 'string' || text.length === 0 || text.length > 8192) {
    throw new Fail(E.TOPOLOGY);
  }
  let body; try { body = JSON.parse(text); } catch { throw new Fail(E.TOPOLOGY); }
  for (const key of ['db', 'hono']) {
    const row = body?.[key];
    if (!row || row.running !== true || !row.networks || typeof row.networks !== 'object'
        || Array.isArray(row.networks) || Object.keys(row.networks).length !== 1
        || !Object.hasOwn(row.networks, NETWORK)) throw new Fail(E.TOPOLOGY);
    const ip = row.networks[NETWORK];
    if (typeof ip !== 'string' || !/^192\.168\.128\.(?:[1-9]|[1-9][0-9]|1[0-9]{2}|2[0-4][0-9]|25[0-4])$/.test(ip)) {
      throw new Fail(E.TOPOLOGY);
    }
  }
  if (body.hono.image !== HONO_IMAGE_ID
      || body.db.networks[NETWORK] === body.hono.networks[NETWORK]) throw new Fail(E.TOPOLOGY);
  return { dbIp: body.db.networks[NETWORK], honoIp: body.hono.networks[NETWORK] };
}
function readTopology() {
  let output;
  try {
    // OpenSSH joins remote command argv through a shell. Encode the fixed,
    // non-secret program so whitespace/quotes cannot change its parse.
    const encoded = Buffer.from(REMOTE_TOPOLOGY_PROGRAM, 'utf8').toString('base64');
    const remote = `/usr/bin/python3 -c "import base64;exec(base64.b64decode('${encoded}'))"`;
    output = execFileSync(SSH_BIN,
      [...SSH_BASE, `${SSH_USER}@${SSH_HOST}`, remote],
      { encoding: 'utf8', timeout: 15_000, maxBuffer: 8192,
        stdio: ['ignore', 'pipe', 'ignore'], env: {} });
  } catch { throw new Fail(E.TOPOLOGY); }
  return parseTopology(output);
}

function buildForwardArgs({ dbIp, honoIp }) {
  return [...SSH_BASE, '-N',
    '-L', `127.0.0.1:${LOCAL_DB_PORT}:${dbIp}:5432`,
    '-L', `127.0.0.1:${LOCAL_HONO_PORT}:${honoIp}:3003`,
    `${SSH_USER}@${SSH_HOST}`];
}
function waitPort(port, deadline) {
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const socket = net.createConnection({ host: '127.0.0.1', port });
      socket.setTimeout(500);
      socket.once('connect', () => { socket.destroy(); resolve(); });
      const failed = () => {
        socket.destroy();
        if (Date.now() >= deadline) reject(new Fail(E.FORWARD));
        else setTimeout(attempt, 100);
      };
      socket.once('error', failed); socket.once('timeout', failed);
    };
    attempt();
  });
}
function openForward(topology) {
  const child = spawn(SSH_BIN, buildForwardArgs(topology),
    { stdio: ['ignore', 'ignore', 'ignore'], env: {} });
  let stopped = false;
  const exited = new Promise((_, reject) => {
    child.once('error', () => reject(new Fail(E.FORWARD)));
    child.once('exit', () => { if (!stopped) reject(new Fail(E.FORWARD)); });
  });
  const ports = Promise.all([
    waitPort(LOCAL_DB_PORT, Date.now() + 15_000),
    waitPort(LOCAL_HONO_PORT, Date.now() + 15_000),
  ]);
  const ready = Promise.race([ports, exited]);
  const cleanup = () => { stopped = true; try { child.kill('SIGTERM'); } catch { /* cleanup */ } };
  return { ready, cleanup };
}

function loadPinnedSeed() {
  let source;
  try {
    source = execFileSync('/usr/bin/git',
      ['-C', MONOREPO, 'show', `${SOURCE_REV}:${SOURCE_PATH}`],
      { encoding: 'utf8', timeout: 10_000, maxBuffer: 2_000_000,
        stdio: ['ignore', 'pipe', 'ignore'], env: {} });
  } catch { throw new Fail(E.SOURCE); }
  if (createHash('sha256').update(source).digest('hex') !== SOURCE_SHA256) {
    throw new Fail(E.SOURCE);
  }
  return source;
}

function createRunTree(source) {
  mkdirSync(TEMP_PARENT, { recursive: true, mode: 0o700 });
  assertOwnedDir(TEMP_PARENT, E.LOCAL_CONTEXT);
  const dir = mkdtempSync(join(TEMP_PARENT, 'quiz-seed-'));
  if (!ownedDir(lstatSync(dir))) throw new Fail(E.LOCAL_CONTEXT);
  const app = join(dir, 'apps', 'hono-app');
  const scripts = join(app, 'scripts');
  mkdirSync(scripts, { recursive: true, mode: 0o700 });
  writeFileSync(join(app, 'package.json'), '{"type":"module"}\n', { mode: 0o600, flag: 'wx' });
  writeFileSync(join(scripts, 'seed-e2e.ts'), source, { mode: 0o600, flag: 'wx' });
  symlinkSync(APP_MODULES, join(app, 'node_modules'), 'dir');
  const verify = [
    "import pg from 'pg';",
    "const c=new pg.Client({connectionString:process.env.DATABASE_URL,connectionTimeoutMillis:5000,query_timeout:5000});",
    "try{await c.connect();const r=await c.query(\"select current_database() db,current_user usr,(select count(*)::int from information_schema.tables where table_schema='public' and table_type='BASE TABLE') tables\");const x=r.rows[0];if(x.db!=='incluir'||x.usr!=='e2e'||x.tables!==67)process.exitCode=1;}catch{process.exitCode=1;}finally{await c.end().catch(()=>{});}",
  ].join('\n');
  writeFileSync(join(app, 'verify-db.mjs'), verify, { mode: 0o600, flag: 'wx' });
  return {
    dir, app, seed: join(scripts, 'seed-e2e.ts'), verify: join(app, 'verify-db.mjs'),
    cleanup: () => { try { rmSync(dir, { recursive: true, force: true }); } catch { /* cleanup */ } },
  };
}

function runHidden(executable, args, env, timeoutMs, code) {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {
      cwd: dirname(args.at(-1)), env,
      // Load-bearing: the pinned seed prints its DB URL, password and CPFs.
      stdio: ['ignore', 'ignore', 'ignore'],
    });
    let settled = false; let timer = null;
    const finish = (ok) => {
      if (settled) return; settled = true; clearTimeout(timer);
      ok ? resolve() : reject(new Fail(code));
    };
    child.once('error', () => finish(false));
    child.once('exit', (status, signal) => finish(status === 0 && signal === null));
    timer = setTimeout(() => {
      try { child.kill('SIGKILL'); } catch { /* best effort */ }
      finish(false);
    }, timeoutMs);
  });
}
function childEnvironment(password) {
  if (typeof password !== 'string' || !/^[\x20-\x7e]{1,512}$/.test(password)) {
    throw new Fail(E.ESCROW);
  }
  return {
    DATABASE_URL: `postgresql://${DB_USER}:${encodeURIComponent(password)}@127.0.0.1:${LOCAL_DB_PORT}/${DB_NAME}`,
    HONO_URL: `http://127.0.0.1:${LOCAL_HONO_PORT}`,
  };
}

async function orchestrate({ assertLocalFn, assertSshFn, topologyFn, escrowFn,
                             forwardFn, sourceFn, treeFn, runVerifyFn, runSeedFn }) {
  for (const fn of [assertLocalFn, assertSshFn, topologyFn, escrowFn, forwardFn,
                    sourceFn, treeFn, runVerifyFn, runSeedFn]) {
    if (typeof fn !== 'function') throw new Fail(E.INTERNAL);
  }
  assertLocalFn(); assertSshFn();
  const topology = topologyFn();       // target proof BEFORE secret read
  const password = escrowFn();
  const source = sourceFn();
  const tree = treeFn(source);
  const fwd = forwardFn(topology);
  try {
    await fwd.ready;
    const env = childEnvironment(password);
    await runVerifyFn(tree, env);
    await runSeedFn(tree, env);
  } finally {
    fwd.cleanup();
    tree.cleanup();
  }
  return OK_RECEIPT;
}

async function main() {
  if (process.argv.slice(2).length !== 0) {
    process.stdout.write(E.BAD_ACTION + '\n'); process.exitCode = 2; return;
  }
  try {
    assertSafeRuntime();
    const receipt = await orchestrate({
      assertLocalFn: assertLocalContext,
      assertSshFn: assertSshContext,
      topologyFn: readTopology,
      escrowFn: readEscrowPassword,
      forwardFn: openForward,
      sourceFn: loadPinnedSeed,
      treeFn: createRunTree,
      runVerifyFn: (tree, env) => runHidden(process.execPath, [tree.verify], env, 15_000, E.DB_BINDING),
      runSeedFn: (tree, env) => runHidden(process.execPath, [TSX_CLI, tree.seed], env, 300_000, E.SEED),
    });
    process.stdout.write(receipt + '\n');
  } catch (err) {
    process.stdout.write((err instanceof Fail ? err.code : E.INTERNAL) + '\n');
    process.exitCode = 1;
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) await main();

export {
  Fail, E, OK_RECEIPT, NETWORK, DB_CONTAINER, HONO_CONTAINER, HONO_IMAGE_ID,
  SOURCE_REV, SOURCE_PATH, SOURCE_SHA256, TSX_SHA256, NODE_VERSION,
  parseEscrow, parseTopology, buildForwardArgs, childEnvironment, classifyRuntime,
  orchestrate, SSH_BASE, ESCROW_KEYS,
};
