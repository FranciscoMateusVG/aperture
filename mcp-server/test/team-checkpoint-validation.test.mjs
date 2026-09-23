import test from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { runInNewContext } from 'node:vm';
import { validateCheckpointSchema, parseCheckpointValidation } from '../dist/team-checkpoint-validation.js';
import { invokeTeamControl, teamControlWatchdogMs } from '../dist/team-control.js';

const shared = name => JSON.parse(readFileSync(new URL(`../../tests/fixtures/team-checkpoint-validation-${name}.json`, import.meta.url), 'utf8'));
const nativeRequest = shared('request');
const nativeResponse = shared('response');
const input = nativeRequest.input;
const ready = () => ({ action: 'validate_checkpoint', result: {
  team: input.team, seat: input.seat, generation: input.expected_generation,
  seq: input.seq, validation: 'ok',
} });

test('shared Rust serde request and response match the selector and verdict contract', () => {
  assert.deepEqual(nativeRequest, { action: 'validate_checkpoint', input: validateCheckpointSchema.parse(input) });
  assert.deepEqual(nativeResponse, ready());
  assert.deepEqual(parseCheckpointValidation(nativeResponse, input), nativeResponse);
});

test('validation selectors reject authority, force/discard, malformed names and unsafe generations', () => {
  assert.deepEqual(validateCheckpointSchema.parse(input), input);
  for (const extra of [{ actor: 'glados' }, { checkpoint: { validation: 'ok' } }, { force: true },
    { discard: true }, { timeout: 1 }, { selection: {} }, { pid: 123 }, { token_id: 'synthetic' },
    { expected_generation: 0 }, { expected_generation: -1 }, { expected_generation: 1.5 },
    { expected_generation: Number.MAX_SAFE_INTEGER + 1 }, { expected_generation: Infinity },
    { expected_generation: '1' }, { team: '' }, { team: 'A' }, { team: 'a'.repeat(17) },
    { team: '../mural' }, { seat: 'a'.repeat(32) }, { seat: 'worker\n' }, { seat: 'a/b' }]) {
    assert.equal(validateCheckpointSchema.safeParse({ ...input, ...extra }).success, false);
  }
  assert.equal(validateCheckpointSchema.safeParse({ ...input, team: 'a'.repeat(16), seat: 'a'.repeat(31) }).success, true);
});

test('validation preserves all verdicts and requires exact selector echoes without claiming a stop', () => {
  for (const validation of ['ok', 'divergent', 'rejected']) {
    const value = ready(); value.result.validation = validation;
    assert.deepEqual(parseCheckpointValidation(value, input), value);
  }
  for (const mutate of [r => r.action = 'stop_seat', r => r.result.team = 'other',
    r => r.result.seat = 'other', r => r.result.generation++, r => r.result.seq++,
    r => r.result.seq = 0, r => r.result.seq = Number.MAX_SAFE_INTEGER + 1,
    r => r.result.generation = 0, r => r.result.validation = 'pending',
    r => r.result.validation = 'valid', r => r.result.validation = true,
    r => r.result.phase = 'ready', r => r.result.owner_state = 'active',
    r => r.result.blockers = [], r => r.result.process_count = 0,
    r => delete r.result.seq, r => delete r.result.validation, r => r.approved = true]) {
    const value = ready(); mutate(value);
    assert.throws(() => parseCheckpointValidation(value, input), /E_CONTROL_UNKNOWN/);
  }
  for (const value of [null, [], {}, 'ok']) assert.throws(() => parseCheckpointValidation(value, input), /E_CONTROL_UNKNOWN/);
});

test('sequence is a positive safe integer and caller proof is always rejected', () => {
  for (const seq of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, Infinity, NaN, '2', null, undefined]) {
    assert.equal(validateCheckpointSchema.safeParse({ ...input, seq }).success, false);
  }
  for (const extra of [{ path: '/tmp/repo' }, { result: 'ok' }, { validation: 'ok' },
    { evidence: {} }, { repository: 'repo' }, { seq: 1, actor: 'glados' }]) {
    assert.equal(validateCheckpointSchema.safeParse({ ...input, ...extra }).success, false);
  }
  assert.equal(validateCheckpointSchema.safeParse({ ...input, seq: Number.MAX_SAFE_INTEGER,
    expected_generation: Number.MAX_SAFE_INTEGER }).success, true);
});

// Evaluate only the actual emitted registration expression with test dependencies.
// This proves its guard/one-call wiring, not the native token authentication itself.
function validationHandler(denied, invoke) {
  const source = readFileSync(new URL('../dist/index.js', import.meta.url), 'utf8');
  const start = source.indexOf('server.tool("team_validate_checkpoint",');
  const end = source.indexOf('\nserver.tool(', start + 1);
  assert.ok(start >= 0 && end > start);
  let callback;
  runInNewContext(source.slice(start, end), {
    server: { tool(name, description, schema, fn) {
      assert.equal(name, 'team_validate_checkpoint');
      assert.equal(schema.input, validateCheckpointSchema);
      assert.match(description, /separate action/);
      callback = fn;
    } },
    gladosControlDenied: () => denied,
    validateCheckpointSchema, parseCheckpointValidation, invokeTeamControl: invoke,
  });
  return callback;
}

test('real MCP registration calls GLaDOS guard before parsing or transport and invokes validation once', async () => {
  let calls = 0;
  const denied = { isError: true, content: [{ type: 'text', text: 'E_CONTROL_UNAUTHORIZED' }] };
  const unauthorized = validationHandler(denied, async () => { calls++; });
  assert.equal(await unauthorized({ input: { actor: 'glados' } }), denied);
  assert.equal(calls, 0);
  const handler = validationHandler(null, async request => {
    calls++;
    assert.equal(JSON.stringify(request), JSON.stringify({ action: 'validate_checkpoint', input }));
    return ready();
  });
  assert.equal((await handler({ input: { ...input, force: true } })).isError, true);
  assert.equal(calls, 0);
  const result = await handler({ input });
  assert.deepEqual(JSON.parse(result.content[0].text), ready());
  assert.equal(calls, 1);
});

test('ok, divergent and rejected return factual validation without invoking StopSeat', async () => {
  for (const validation of ['ok', 'divergent', 'rejected']) {
    const requests = [];
    const handler = validationHandler(null, async request => {
      requests.push(request.action);
      const value = ready(); value.result.validation = validation; return value;
    });
    const result = await handler({ input });
    assert.equal(result.isError, undefined);
    assert.equal(JSON.parse(result.content[0].text).result.validation, validation);
    assert.deepEqual(requests, ['validate_checkpoint']);
  }
});

test('malformed receipt or UNKNOWN from native transport does not retry or fabricate readiness', async () => {
  for (const failure of ['receipt', 'timeout']) {
    let calls = 0;
    const handler = validationHandler(null, async () => {
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

test('validation uses stdin-only native request and preserves the exact native verdict', async () => {
  const root = mkdtempSync(join(tmpdir(), 'aperture-validation-wire-'));
  const bin = join(root, 'control');
  const requestFile = join(root, 'request');
  writeFileSync(bin, `#!/bin/sh\n[ "$#" -eq 0 ] || exit 70\nIFS= read -r request\nprintf '%s' "$request" > '${requestFile}'\nprintf '%s\\n' '${JSON.stringify(ready())}'\n`);
  chmodSync(bin, 0o700);
  const old = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  try {
    const value = await invokeTeamControl({ action: 'validate_checkpoint', input });
    assert.deepEqual(JSON.parse(readFileSync(requestFile, 'utf8')), { action: 'validate_checkpoint', input });
    assert.deepEqual(parseCheckpointValidation(value, input), ready());
  } finally {
    if (old === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = old;
    rmSync(root, { recursive: true, force: true });
  }
});

test('validation watchdog is fixed180s and kills the isolated fixture group with UNKNOWN after admission', { timeout: 10000 }, async t => {
  assert.equal(teamControlWatchdogMs('validate_checkpoint'), 180_000);
  assert.equal(teamControlWatchdogMs('bootstrap_seat'), 180_000);
  assert.equal(teamControlWatchdogMs('replace'), 180_000);
  assert.equal(teamControlWatchdogMs('list_teams'), 15_000);
  const root = mkdtempSync(join(tmpdir(), 'aperture-validation-timeout-'));
  const bin = join(root, 'control');
  const admitted = join(root, 'admitted');
  const escaped = join(root, 'escaped');
  // A surviving grandchild would publish escaped after the parent watchdog.
  writeFileSync(bin, `#!/bin/sh\n[ "$#" -eq 0 ] || exit 70\nIFS= read -r request\nprintf '%s' "$request" | grep -q '"action":"validate_checkpoint"' || exit 71\n(sleep 1; : > '${escaped}') &\nprintf '%s' "$$" > '${admitted}'\nwait\n`);
  chmodSync(bin, 0o700);
  const old = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  t.mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const pending = invokeTeamControl({ action: 'validate_checkpoint', input });
    for (let i = 0; i < 400 && !existsSync(admitted); i++) await delay(5);
    assert.equal(existsSync(admitted), true);
    t.mock.timers.tick(180_000);
    await assert.rejects(pending, /E_CONTROL_UNKNOWN: team control outcome is incomplete/);
    await delay(1100);
    assert.equal(existsSync(escaped), false, 'isolated child group must not continue past watchdog');
  } finally {
    t.mock.timers.reset();
    if (old === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = old;
    rmSync(root, { recursive: true, force: true });
  }
});
