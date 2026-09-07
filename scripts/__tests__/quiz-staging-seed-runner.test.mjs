import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  Fail, E, OK_RECEIPT, NETWORK, DB_CONTAINER, HONO_CONTAINER, HONO_IMAGE_ID,
  SOURCE_REV, SOURCE_PATH, SOURCE_SHA256, TSX_SHA256, NODE_VERSION,
  parseEscrow, parseTopology, buildForwardArgs, childEnvironment, classifyRuntime,
  orchestrate, SSH_BASE, ESCROW_KEYS,
} from '../quiz-staging-seed-runner.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = readFileSync(join(HERE, '..', 'quiz-staging-seed-runner.mjs'), 'utf8');
const SECRET = 'CANARY_DB_PASSWORD_e31a';
const TOKENISH = 'CANARY_SEED_OUTPUT_9f20';
const escrow = Object.fromEntries(ESCROW_KEYS.map((key) => [key, key === 'STAGING_HONO_IMAGE'
  ? HONO_IMAGE_ID : key === 'STAGING_HONO_DB_PASSWORD' ? SECRET : 'fixed-value']));
const escrowText = Object.entries(escrow).map(([k, v]) => `${k}=${v}`).join('\n');
const topologyText = JSON.stringify({
  db: { running: true, image: 'sha256:db', networks: { [NETWORK]: '192.168.128.2' } },
  hono: { running: true, image: HONO_IMAGE_ID, networks: { [NETWORK]: '192.168.128.5' } },
});
function surface(err) {
  let json = ''; try { json = JSON.stringify(err); } catch { /* ignore */ }
  return [err?.code, err?.message, err?.stack, json].join('|');
}

test('pins exact existing seed source and toolchain artifacts', () => {
  assert.equal(SOURCE_REV, '9cb605fc2cc63930a9fdf4f73ccec3ec05c6daad');
  assert.equal(SOURCE_PATH, 'apps/hono-app/scripts/seed-e2e.ts');
  assert.equal(SOURCE_SHA256, '9b690ca203df6d26a43afee9eccf3316eef5d0fe6f4ffca49a08f564ccd926e8');
  assert.equal(TSX_SHA256, '8729ecfb90d9d568939e4190e6f1d3317c946583b7d37a776e0c23a21c021cf8');
  assert.equal(NODE_VERSION, process.version);
});

test('escrow parser accepts exact keys and returns only staging DB password', () => {
  assert.equal(parseEscrow(escrowText), SECRET);
  assert.equal(parseEscrow(escrowText + '\n'), SECRET);
});

test('escrow parser rejects missing, duplicate, extra, malformed and wrong image', () => {
  const lines = escrowText.split('\n');
  const bad = [
    lines.slice(1).join('\n'),
    escrowText + `\n${ESCROW_KEYS[0]}=duplicate`,
    escrowText + '\nEXTRA=x',
    escrowText.replace('=', ':'),
    escrowText.replace(HONO_IMAGE_ID, 'sha256:wrong'),
    escrowText.replace(SECRET, SECRET + '\u0000'),
  ];
  for (const text of bad) assert.throws(() => parseEscrow(text), (e) => e.code === E.ESCROW);
});

test('topology binds both running containers exclusively to staging network', () => {
  assert.deepEqual(parseTopology(topologyText),
    { dbIp: '192.168.128.2', honoIp: '192.168.128.5' });
});

test('topology rejects shared/prod network, stopped target, wrong image and duplicate IP', () => {
  const base = JSON.parse(topologyText);
  const variants = [
    { ...base, db: { ...base.db, networks: { [NETWORK]: '192.168.128.2', dokploy: '10.0.1.2' } } },
    { ...base, db: { ...base.db, running: false } },
    { ...base, hono: { ...base.hono, image: 'sha256:wrong' } },
    { ...base, hono: { ...base.hono, networks: { [NETWORK]: '192.168.128.2' } } },
    { ...base, hono: { ...base.hono, networks: { [NETWORK]: '10.0.1.220' } } },
  ];
  for (const body of variants) {
    assert.throws(() => parseTopology(JSON.stringify(body)), (e) => e.code === E.TOPOLOGY);
  }
});

test('forward args pin tailnet SSH and validated isolated IPs only', () => {
  const args = buildForwardArgs(parseTopology(topologyText));
  assert.ok(args.includes('127.0.0.1:45432:192.168.128.2:5432'));
  assert.ok(args.includes('127.0.0.1:45433:192.168.128.5:3003'));
  assert.ok(args.includes('ubuntu@100.85.254.44'));
  assert.ok(args.includes('-F') && args.includes('none'));
  assert.ok(args.includes('StrictHostKeyChecking=yes'));
  assert.ok(args.includes('IdentitiesOnly=yes'));
  assert.equal(args.filter((a) => a === '-L').length, 2);
});

test('child env points only to loopback forwards and exact DB identity', () => {
  const env = childEnvironment(SECRET);
  assert.deepEqual(Object.keys(env).sort(), ['DATABASE_URL', 'HONO_URL']);
  assert.equal(env.HONO_URL, 'http://127.0.0.1:45433');
  assert.ok(env.DATABASE_URL.startsWith('postgresql://e2e:'));
  assert.ok(env.DATABASE_URL.endsWith('@127.0.0.1:45432/incluir'));
  assert.ok(!env.DATABASE_URL.includes('192.168.'));
});

function harness({ topology = parseTopology(topologyText), failAt = null } = {}) {
  const calls = []; let fwdClean = 0; let treeClean = 0;
  const step = (name, value) => {
    calls.push(name);
    if (failAt === name) throw new Fail(E.SEED);
    return value;
  };
  return {
    calls, fwdClean: () => fwdClean, treeClean: () => treeClean,
    args: {
      assertLocalFn: () => step('local'),
      assertSshFn: () => step('ssh'),
      topologyFn: () => step('topology', topology),
      escrowFn: () => step('escrow', SECRET),
      sourceFn: () => step('source', 'PINNED_SOURCE'),
      treeFn: () => step('tree', { marker: 'tree',
        cleanup: () => { calls.push('tree-clean'); treeClean += 1; } }),
      forwardFn: (t) => step('forward', {
        ready: Promise.resolve(step('forward-ready', t)),
        cleanup: () => { calls.push('forward-clean'); fwdClean += 1; },
      }),
      runVerifyFn: (_tree, env) => step('verify', env),
      runSeedFn: (_tree, env) => step('seed', env),
    },
  };
}

test('exact composition proves target before reading secret, verifies DB before seed', async () => {
  const h = harness();
  assert.equal(await orchestrate(h.args), OK_RECEIPT);
  assert.deepEqual(h.calls.map((x) => typeof x === 'string' ? x : x),
    ['local', 'ssh', 'topology', 'escrow', 'source', 'tree', 'forward-ready',
      'forward', 'verify', 'seed', 'forward-clean', 'tree-clean']);
  assert.ok(h.calls.indexOf('topology') < h.calls.indexOf('escrow'));
  assert.ok(h.calls.indexOf('verify') < h.calls.indexOf('seed'));
  assert.equal(h.fwdClean(), 1); assert.equal(h.treeClean(), 1);
});

test('topology failure stops before secret read or forward', async () => {
  const h = harness({ failAt: 'topology' });
  await assert.rejects(() => orchestrate(h.args), Fail);
  assert.deepEqual(h.calls, ['local', 'ssh', 'topology']);
});

test('DB verification failure stops seed and cleans both resources', async () => {
  const h = harness({ failAt: 'verify' });
  await assert.rejects(() => orchestrate(h.args), Fail);
  assert.ok(!h.calls.includes('seed'));
  assert.equal(h.fwdClean(), 1); assert.equal(h.treeClean(), 1);
});

test('synchronous forward construction failure still removes the private run tree', async () => {
  const h = harness({ failAt: 'forward' });
  await assert.rejects(() => orchestrate(h.args), Fail);
  assert.equal(h.fwdClean(), 0, 'no forward handle existed');
  assert.equal(h.treeClean(), 1, 'created run tree must not survive a forward failure');
});

test('seed failure is not retried and resources clean exactly once', async () => {
  const h = harness({ failAt: 'seed' });
  await assert.rejects(() => orchestrate(h.args), Fail);
  assert.equal(h.calls.filter((x) => x === 'seed').length, 1);
  assert.equal(h.fwdClean(), 1); assert.equal(h.treeClean(), 1);
});

test('credential-bearing canaries never reach receipt or failure surfaces', async () => {
  const h = harness({ failAt: 'seed' });
  const err = await orchestrate(h.args).then(() => null, (e) => e);
  assert.ok(!surface(err).includes(SECRET));
  assert.ok(!surface(err).includes(TOKENISH));
  assert.ok(!OK_RECEIPT.includes(SECRET));
});

test('runtime accepts exact clean executor and refuses instrumentation/version drift', () => {
  const clean = { env: {}, execArgv: [], nodeVersion: NODE_VERSION };
  assert.equal(classifyRuntime(clean), true);
  for (const state of [
    { ...clean, env: { NODE_OPTIONS: '--inspect' } },
    { ...clean, env: { NODE_DEBUG: 'http' } },
    { ...clean, execArgv: ['--import=x'] },
    { ...clean, nodeVersion: 'v20.20.2' },
  ]) assert.throws(() => classifyRuntime(state), (e) => e.code === E.UNSAFE_RUNTIME);
});

test('SSH uses no config file and no permissive host verification', () => {
  assert.ok(SSH_BASE.includes('-F') && SSH_BASE.includes('none'));
  for (const required of ['BatchMode=yes', 'StrictHostKeyChecking=yes',
    'PasswordAuthentication=no', 'KbdInteractiveAuthentication=no',
    'PermitLocalCommand=no', 'ExitOnForwardFailure=yes', 'IdentitiesOnly=yes']) {
    assert.ok(SSH_BASE.includes(required));
  }
  for (const bad of ['StrictHostKeyChecking=no', 'accept-new', 'UserKnownHostsFile=/dev/null']) {
    assert.ok(!SSH_BASE.includes(bad));
  }
});

test('seed subprocess boundary suppresses stdin/stdout/stderr and has no automatic retry', () => {
  const run = SRC.slice(SRC.indexOf('function runHidden'), SRC.indexOf('function childEnvironment'));
  assert.ok(run.includes("stdio: ['ignore', 'ignore', 'ignore']"));
  assert.ok(!run.includes('pipe'));
  const main = SRC.slice(SRC.indexOf('async function main'));
  assert.equal((main.match(/runSeedFn:/g) || []).length, 1);
});

test('no production endpoint, public DB publish, provider call or replacement seed exists', () => {
  for (const banned of ['10.0.1.220', '167.234.234.41', 'api.openai.com',
    'OPENAI_API_KEY', 'docker run', '--publish', '-p 5432']) {
    assert.ok(!SRC.includes(banned), 'contains ' + banned);
  }
  assert.ok(SRC.includes("git',\n      ['-C', MONOREPO, 'show'"));
  assert.ok(!SRC.includes('writeFileSync(join(scripts, \'replacement'));
});

test('operational source/escrow/SSH/process functions remain private', async () => {
  const mod = await import('../quiz-staging-seed-runner.mjs');
  for (const name of ['readEscrowPassword', 'readTopology', 'openForward',
    'loadPinnedSeed', 'createRunTree', 'runHidden', 'main', 'ESCROW_PATH']) {
    assert.ok(!(name in mod), name + ' escaped');
  }
});

test('receipt is constant and non-oracular', () => {
  assert.equal(OK_RECEIPT, 'QUIZ_STAGING_SYNTHETIC_SEED_COMPLETE');
  for (const bad of ['createHash', 'SOURCE_SHA256', 'TSX_SHA256']) {
    // Hashing is allowed only for public source/tool integrity; receipt stays constant.
    assert.ok(!OK_RECEIPT.includes(bad));
  }
});
