import test from "node:test";
import assert from "node:assert/strict";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  authorizeMessage,
  isValidSeatName,
  loadSeatRegistry,
} from "../dist/seat-registry.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const NAME_CASES = JSON.parse(
  readFileSync(resolve(HERE, "../../tests/fixtures/seat-name-cases.json"), "utf8"),
);

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "aperture-v4-registry-"));
  const agents = join(root, "agents");
  const teams = join(root, "teams");
  mkdirSync(agents);
  mkdirSync(teams);
  return { root, agents, teams, close: () => rmSync(root, { recursive: true, force: true }) };
}

function agent(root, name, { enabled = true, role = "worker", team = false } = {}) {
  const dir = join(root, name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "manifest.json"), JSON.stringify({ name, model: "codex/test", role, enabled }));
  if (team) {
    writeFileSync(join(dir, "TEAM"), "");
    writeFileSync(join(dir, ".complete"), "");
  }
}

function team(root, name, { project = "project:aperture", lead, seats, grants = [], state = "active" }) {
  const dir = join(root, name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, "state.json"), JSON.stringify({ state, generation: 1 }));
  writeFileSync(join(dir, "team.json"), JSON.stringify({ team: name, project, lead, seats, grants }));
}

test("canonical name fixture matches TypeScript registry", () => {
  for (const name of NAME_CASES.valid) assert.equal(isValidSeatName(name), true, name);
  for (const name of NAME_CASES.invalid) assert.equal(isValidSeatName(name), false, name);
});

test("registry preserves legacy and exposes only complete active unambiguous team seats", () => {
  const f = fixture();
  try {
    agent(f.agents, "rex", { role: "backend" });
    agent(f.agents, "disabled", { enabled: false });
    agent(f.agents, "p1-a-lead", { team: true });
    agent(f.agents, "pending-lead", { team: true });
    team(f.teams, "p1-a", {
      lead: "p1-a-lead",
      seats: [{ name: "p1-a-lead", role: "lead" }],
    });
    team(f.teams, "pending", {
      lead: "pending-lead",
      seats: [{ name: "pending-lead", role: "lead" }],
      state: "pending",
    });

    let registry = loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams });
    assert.deepEqual([...registry.seats.keys()].sort(), ["p1-a-lead", "rex"]);
    assert.equal(registry.seats.get("p1-a-lead").isLead, true);
    assert.equal(registry.seats.get("p1-a-lead").role, "lead");

    writeFileSync(join(f.teams, "p1-a", "journal.json"), "{}");
    registry = loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams });
    assert.equal(registry.seats.has("p1-a-lead"), false, "journal makes the seat invisible");
  } finally {
    f.close();
  }
});

test("fresh snapshots observe activation and archive without a process restart", () => {
  const f = fixture();
  try {
    agent(f.agents, "p1-a-lead", { team: true });
    team(f.teams, "p1-a", {
      lead: "p1-a-lead",
      seats: [{ name: "p1-a-lead", role: "lead" }],
      state: "pending",
    });
    assert.equal(loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams }).seats.has("p1-a-lead"), false);
    writeFileSync(join(f.teams, "p1-a", "state.json"), JSON.stringify({ state: "active", generation: 2 }));
    assert.equal(loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams }).seats.has("p1-a-lead"), true);
    writeFileSync(join(f.teams, "p1-a", "state.json"), JSON.stringify({ state: "archived", generation: 3 }));
    assert.equal(loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams }).seats.has("p1-a-lead"), false);
  } finally {
    f.close();
  }
});

test("unsafe symlinks and ambiguous memberships fail closed", () => {
  const f = fixture();
  try {
    const outside = join(f.root, "outside");
    agent(outside, "evil");
    symlinkSync(join(outside, "evil"), join(f.agents, "evil"));
    agent(f.agents, "shared-lead", { team: true });
    for (const name of ["p1-a", "p1-b"]) {
      team(f.teams, name, {
        lead: "shared-lead",
        seats: [{ name: "shared-lead", role: "lead" }],
      });
    }
    agent(f.agents, "missing-marker", { team: false });
    team(f.teams, "p1-c", {
      lead: "missing-marker",
      seats: [{ name: "missing-marker", role: "lead" }],
    });
    agent(f.agents, "journal-link", { team: true });
    team(f.teams, "p1-d", {
      lead: "journal-link",
      seats: [{ name: "journal-link", role: "lead" }],
    });
    symlinkSync(join(f.root, "does-not-exist"), join(f.teams, "p1-d", "journal.json"));
    const registry = loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams });
    assert.equal(registry.seats.has("evil"), false);
    assert.equal(registry.seats.has("shared-lead"), false);
    assert.equal(registry.seats.has("missing-marker"), false, "team membership cannot downgrade to legacy");
    assert.equal(registry.seats.has("journal-link"), false, "even a broken journal symlink fails closed");
  } finally {
    f.close();
  }
});

test("routing uses registry truth, explicit grants, and unknown-recipient precedence", () => {
  const f = fixture();
  try {
    for (const name of ["glados", "wheatley", "peppy", "rex", "izzy"]) agent(f.agents, name);
    const teamSeats = [
      ["p1-a-lead", "lead"], ["p1-a-backend", "backend"],
      ["p1-b-lead", "lead"], ["p1-b-qa", "qa"],
      ["p2-c-lead", "lead"],
    ];
    for (const [name] of teamSeats) agent(f.agents, name, { team: true });
    team(f.teams, "p1-a", {
      lead: "p1-a-lead",
      seats: teamSeats.slice(0, 2).map(([name, role]) => ({ name, role })),
      grants: [{ from: "p1-a", to: "izzy", scope: "message", by: "glados", at: "2026-09-20T00:00:00Z" }],
    });
    team(f.teams, "p1-b", {
      lead: "p1-b-lead",
      seats: teamSeats.slice(2, 4).map(([name, role]) => ({ name, role })),
    });
    team(f.teams, "p2-c", {
      project: "project:other",
      lead: "p2-c-lead",
      seats: [{ name: "p2-c-lead", role: "lead" }],
    });
    const registry = loadSeatRegistry({ agentsRoot: f.agents, teamsRoot: f.teams });
    assert.equal(authorizeMessage(registry, "rex", "izzy").allowed, true, "legacy compatibility group");
    assert.equal(authorizeMessage(registry, "p1-a-backend", "p1-a-lead").allowed, true, "same team");
    assert.equal(authorizeMessage(registry, "p1-a-lead", "p1-b-lead").allowed, true, "same-project leads");
    assert.deepEqual(authorizeMessage(registry, "p1-a-backend", "p1-b-lead"), {
      allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "nonlead",
    });
    assert.deepEqual(authorizeMessage(registry, "p1-a-backend", "p9-nope"), {
      allowed: false, code: "E_UNKNOWN_RECIPIENT", reason: "unknown_recipient",
    });
    assert.deepEqual(authorizeMessage(registry, "p1-a-lead", "p2-c-lead"), {
      allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "cross_project",
    });
    assert.equal(authorizeMessage(registry, "p1-a-backend", "izzy").allowed, true, "explicit QA grant");
    assert.deepEqual(authorizeMessage(registry, "p1-b-qa", "izzy"), {
      allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "no_grant",
    }, "QA role alone grants nothing");
    assert.equal(authorizeMessage(registry, "p1-a-backend", "glados").allowed, true, "trio path");
    assert.deepEqual(authorizeMessage(registry, "unknown", "glados"), {
      allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "unknown_sender",
    }, "a trio-shaped target never rescues an unknown sender");
    assert.deepEqual(authorizeMessage(registry, "unknown", "also-unknown"), {
      allowed: false, code: "E_UNKNOWN_RECIPIENT", reason: "unknown_recipient",
    }, "unknown-recipient precedence is stable even when the sender is also invalid");
  } finally {
    f.close();
  }
});
