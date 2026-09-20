import test from "node:test";
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import {
  assertActivationMatchesPending,
  assertAuthorizedEpic,
  invokeTeamControl,
  parsePendingList,
  teamControlWatchdogMs,
} from "../dist/team-control.js";

const requestId = "11111111-1111-4111-8111-111111111111";
const pending = parsePendingList({
  action: "list_pending",
  result: [{
    snapshot: { team: "alpha", project: "project:aperture", repo: "aperture", creation_request_id: requestId },
    state: { state: "pending", generation: 0 },
  }],
});

test("pending selector and epic authorization are exact", () => {
  const input = {
    team: "alpha",
    expected_generation: 0,
    creation_request_id: requestId,
    epic_id: "aperture-4rsnc",
  };
  assert.equal(assertActivationMatchesPending(pending, input).snapshot.team, "alpha");
  assert.doesNotThrow(() => assertAuthorizedEpic(JSON.stringify({
    id: "aperture-4rsnc",
    issue_type: "epic",
    status: "in_progress",
    labels: ["project:aperture"],
  }), input.epic_id, "project:aperture"));
  assert.throws(() => assertActivationMatchesPending(pending, { ...input, expected_generation: 1 }), /E_CONTROL_STALE/);
  assert.throws(() => assertAuthorizedEpic(JSON.stringify({
    id: "aperture-4rsnc",
    issue_type: "task",
    status: "in_progress",
    labels: ["project:aperture"],
  }), input.epic_id, "project:aperture"), /E_CONTROL_EPIC_INVALID/);
  assert.throws(() => assertAuthorizedEpic(JSON.stringify({
    id: "aperture-4rsnc",
    issue_type: "epic",
    status: "in_progress",
    labels: ["project:incluir"],
  }), input.epic_id, "project:aperture"), /E_CONTROL_EPIC_INVALID/);
});

test("control runner uses stdin with zero argv authority and bounded JSON output", async () => {
  const root = mkdtempSync(join(tmpdir(), "aperture-team-control-"));
  const bin = join(root, "control");
  writeFileSync(bin, `#!/bin/sh
[ "$#" -eq 0 ] || exit 70
IFS= read -r request
printf '%s' "$request" | grep -q '"action":"list_pending"' || exit 71
printf '%s\n' '{"action":"list_pending","result":[]}'
`);
  chmodSync(bin, 0o700);
  const old = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  try {
    assert.deepEqual(await invokeTeamControl({ action: "list_pending" }), {
      action: "list_pending",
      result: [],
    });
  } finally {
    if (old === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = old;
    rmSync(root, { recursive: true, force: true });
  }
});

test("control watchdog is fixed by action and replacement timeout is unknown after admission", async (t) => {
  assert.equal(teamControlWatchdogMs("list_pending"), 15_000);
  assert.equal(teamControlWatchdogMs("approve"), 15_000);
  assert.equal(teamControlWatchdogMs("cancel"), 15_000);
  assert.equal(teamControlWatchdogMs("checkpoint"), 15_000);
  assert.equal(teamControlWatchdogMs("inspect_remote"), 15_000);
  assert.equal(teamControlWatchdogMs("resolve_remote"), 15_000);
  assert.equal(teamControlWatchdogMs("replace"), 180_000);
  assert.equal(teamControlWatchdogMs("archive"), 90_000);
  assert.equal(teamControlWatchdogMs("rollback_archive"), 90_000);

  const root = mkdtempSync(join(tmpdir(), "aperture-team-control-timeout-"));
  const bin = join(root, "control");
  const admitted = join(root, "admitted");
  writeFileSync(bin, `#!/bin/sh
[ "$#" -eq 0 ] || exit 70
IFS= read -r request
printf '%s' "$request" | grep -q '"action":"replace"' || exit 71
: > '${admitted}'
sleep 999
`);
  chmodSync(bin, 0o700);
  const old = process.env.APERTURE_TEAM_CONTROL_BIN;
  process.env.APERTURE_TEAM_CONTROL_BIN = bin;
  t.mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const pending = invokeTeamControl({
      action: "replace",
      input: {
        target_seat: "alpha-worker",
        expected_generation: 1,
        selection: { harness: "codex", model: "gpt-6-astra", reasoning: "high" },
      },
    });
    for (let i = 0; i < 100 && !existsSync(admitted); i += 1) {
      await delay(5);
    }
    assert.equal(existsSync(admitted), true, "fixture child must cross its admission marker");
    t.mock.timers.tick(180_000);
    await assert.rejects(pending, /E_CONTROL_UNKNOWN: team control outcome is incomplete/);
  } finally {
    t.mock.timers.reset();
    if (old === undefined) delete process.env.APERTURE_TEAM_CONTROL_BIN;
    else process.env.APERTURE_TEAM_CONTROL_BIN = old;
    rmSync(root, { recursive: true, force: true });
  }
});
