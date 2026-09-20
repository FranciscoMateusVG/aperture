import test from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  assertActivationMatchesPending,
  assertAuthorizedEpic,
  invokeTeamControl,
  parsePendingList,
} from "../dist/team-control.js";

const requestId = "11111111-1111-4111-8111-111111111111";
const pending = parsePendingList({
  action: "list_pending",
  result: [{
    snapshot: { team: "alpha", project: "project:aperture", creation_request_id: requestId },
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
