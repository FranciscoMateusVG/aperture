import test from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { runInNewContext } from 'node:vm';
import { stopSeatSchema, parseStopSeatReady } from '../dist/team-stop.js';
import { invokeTeamControl, teamControlWatchdogMs } from '../dist/team-control.js';

const shared = name => JSON.parse(readFileSync(new URL(`../../tests/fixtures/team-stop-${name}.json`, import.meta.url), 'utf8'));
const nativeRequest = shared('request');
const nativeResponse = shared('response');
const input = nativeRequest.input;
const ready = () => ({ action: 'stop_seat', result: {
  team: input.team, seat: input.seat, generation: input.expected_generation,
  phase: 'ready', checkpoint_recovery: 'valid', owner_state: 'active', blockers: [],
} });

test('shared Rust serde request and response preserve the exact native wire contract', () => {
  assert.deepEqual(nativeRequest, { action: 'stop_seat', input: stopSeatSchema.parse(input) });
  assert.deepEqual(nativeResponse, ready());
  assert.deepEqual(parseStopSeatReady(nativeResponse, input), nativeResponse);
});

test('stop selectors reject authority, force/discard, malformed names and unsafe generations', () => {
  assert.deepEqual(stopSeatSchema.parse(input), input);
  for (const extra of [{ actor: 'glados' }, { checkpoint: { validation: 'ok' } }, { force: true },
    { discard: true }, { timeout: 1 }, { selection: {} }, { pid: 123 }, { token_id: 'synthetic' },
    { expected_generation: 0 }, { expected_generation: -1 }, { expected_generation: 1.5 },
    { expected_generation: Number.MAX_SAFE_INTEGER + 1 }, { expected_generation: Infinity },
    { expected_generation: '1' }, { team: '' }, { team: 'A' }, { team: 'a'.repeat(17) },
    { team: '../mural' }, { seat: 'a'.repeat(32) }, { seat: 'worker\n' }, { seat: 'a/b' }]) {
    assert.equal(stopSeatSchema.safeParse({ ...input, ...extra }).success, false);
  }
  assert.equal(stopSeatSchema.safeParse({ ...input, team: 'a'.repeat(16), seat: 'a'.repeat(31) }).success, true);
});

test('ready receipt requires exact echoes and never claims archive, replacement or zero process count', () => {
  assert.deepEqual(parseStopSeatReady(ready(), input), ready());
  for (const mutate of [r => r.action = 'archive', r => r.result.team = 'other',
    r => r.result.seat = 'other', r => r.result.generation++, r => r.result.generation = 0,
    r => r.result.phase = 'started', r => r.result.phase = 'archived',
    r => r.result.checkpoint_recovery = 'stale', r => r.result.checkpoint_recovery = 'none',
    r => r.result.owner_state = 'stale', r => r.result.blockers.push('uncertain'),
    r => r.result.process_count = 0, r => r.result.owner = {}, r => r.result.pid = 123,
    r => r.result.generation = Number.MAX_SAFE_INTEGER + 1, r => delete r.result.checkpoint_recovery,
    r => r.approved = true]) {
    const value = ready(); mutate(value);
    assert.throws(() => parseStopSeatReady(value, input), /E_CONTROL_UNKNOWN/);
  }
  for (const value of [null, [], {}, 'ready']) assert.throws(() => parseStopSeatReady(value, input), /E_CONTROL_UNKNOWN/);
});

// Evaluate only the actual emitted registration expression with test dependencies.
// This proves its guard/one-call wiring, not the native token authentication itself.
function stopHandler(denied, invoke) {
  const source = readFileSync(new URL('../dist/index.js', import.meta.url), 'utf8');
  const start = source.indexOf('server.tool("team_stop_seat",');
  const end = source.indexOf('\nserver.tool(', start + 1);
  assert.ok(start >= 0 && end > start);
  let callback;
  runInNewContext(source.slice(start, end), {
    server: { tool(name, description, schema, fn) {
      assert.equal(name, 'team_stop_seat');
      assert.equal(schema.input, stopSeatSchema);
      assert.match(description, /separate action/);
      callback = fn;
    } },
    gladosControlDenied: () => denied,
    stopSeatSchema, parseStopSeatReady, invokeTeamControl: invoke,
  });
  return callback;
}

test('real MCP registration calls GLaDOS guard before parsing or transport and invokes stop once', async () => {
  let calls = 0;
  const denied = { isError: true, content: [{ type: 'text', text: 'E_CONTROL_UNAUTHORIZED' }] };
  const unauthorized = stopHandler(denied, async () => { calls++; });
  assert.equal(await unauthorized({ input: { actor: 'glados' } }), denied);
  assert.equal(calls, 0);
  const handler = stopHandler(null, async request => {
    calls++;
    assert.equal(JSON.stringify(request), JSON.stringify({ action: 'stop_seat', input }));
    return ready();
  });
  assert.equal((await handler({ input: { ...input, force: true } })).isError, true);
  assert.equal(calls, 0);
  const result = await handler({ input });
  assert.deepEqual(JSON.parse(result.content[0].text), ready());
  assert.equal(calls, 1);
});

test('malformed receipt or UNKNOWN from native transport does not retry or fabricate readiness', async () => {
  for (const failure of ['receipt', 'timeout']) {
    let calls = 0;
    const handler = stopHandler(null, async () => {
      calls++;
      if (failure === 'timeout') throw new Error('E_CONTROL_UNKNOWN: explicit reconciliation required');
      const value = ready(); value.result.generation++; return value;
    });
    const result = await handler({ input });
    assert.equal(result.isError, true);
    assert.match(result.content[0].text, /E_CONTROL_UNKNOWN/);
    assert.equal(calls, 1);
  }
});

test('stop uses stdin-only native request and preserves the factual same-generation ready response', async () => {
  const root = mkdtempSync(join(tmpdir(), 'aperture-stop-wire-'));
  const bin = join(root, 'control');
  const requestFile = join(root, 'request');
  writeFileSync(bin, `#!/bin/sh\n[ "$#" -eq 0 ] || exit 70\nIFS= read -r request\nprintf '%s' "$request" > '${requestFile}'\nprintf '%s\\n' '${JSON.stringify(ready())}'\n`);
  chmodSync(bin, 0o700);
  const old = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  try {
    const value = await invokeTeamControl({ action: 'stop_seat', input });
    assert.deepEqual(JSON.parse(readFileSync(requestFile, 'utf8')), { action: 'stop_seat', input });
    assert.deepEqual(parseStopSeatReady(value, input), ready());
  } finally {
    if (old === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = old;
    rmSync(root, { recursive: true, force: true });
  }
});

test('stop watchdog is fixed180s and kills the isolated fixture group with UNKNOWN after admission', { timeout: 5000 }, async t => {
  assert.equal(teamControlWatchdogMs('stop_seat'), 180_000);
  assert.equal(teamControlWatchdogMs('bootstrap_seat'), 180_000);
  assert.equal(teamControlWatchdogMs('replace'), 180_000);
  assert.equal(teamControlWatchdogMs('list_teams'), 15_000);
  const root = mkdtempSync(join(tmpdir(), 'aperture-stop-timeout-'));
  const bin = join(root, 'control');
  const admitted = join(root, 'admitted');
  const escaped = join(root, 'escaped');
  // A surviving grandchild would publish escaped after the parent watchdog.
  writeFileSync(bin, `#!/bin/sh\n[ "$#" -eq 0 ] || exit 70\nIFS= read -r request\nprintf '%s' "$request" | grep -q '"action":"stop_seat"' || exit 71\n(sleep 0.3; : > '${escaped}') &\nprintf '%s' "$$" > '${admitted}'\nwait\n`);
  chmodSync(bin, 0o700);
  const old = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  t.mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const pending = invokeTeamControl({ action: 'stop_seat', input });
    for (let i = 0; i < 100 && !existsSync(admitted); i++) await delay(5);
    assert.equal(existsSync(admitted), true);
    t.mock.timers.tick(180_000);
    await assert.rejects(pending, /E_CONTROL_UNKNOWN: team control outcome is incomplete/);
    await delay(400);
    assert.equal(existsSync(escaped), false, 'isolated child group must not continue past watchdog');
  } finally {
    t.mock.timers.reset();
    if (old === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = old;
    rmSync(root, { recursive: true, force: true });
  }
});
