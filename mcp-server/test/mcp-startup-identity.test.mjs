import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVER = resolve(HERE, "..", "dist", "index.js");

test("invalid AGENT_NAME exits before mailbox or queue filesystem effects", () => {
  const root = mkdtempSync(join(tmpdir(), "aperture-invalid-principal-"));
  const home = join(root, "home");
  const agents = join(root, "agents");
  const teams = join(root, "teams");
  const mailbox = join(root, "mailbox");
  const escaped = join(root, "escaped");
  mkdirSync(home);
  mkdirSync(agents);
  mkdirSync(teams);
  try {
    const result = spawnSync(process.execPath, [SERVER], {
      env: {
        PATH: process.env.PATH,
        HOME: home,
        AGENT_NAME: "../escaped",
        APERTURE_AGENTS_DIR: agents,
        APERTURE_TEAMS_DIR: teams,
        APERTURE_MAILBOX: mailbox,
        APERTURE_HUB_TOKEN_DIR: join(root, "tokens"),
        APERTURE_HUB_TOKEN_FILE: join(root, "tokens", "../escaped.token"),
      },
      encoding: "utf8",
      timeout: 2_000,
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /not an authenticated enabled registry principal/);
    assert.equal(existsSync(mailbox), false, "mailbox root was not created");
    assert.equal(existsSync(join(home, ".aperture")), false, "send queue root was not created");
    assert.equal(existsSync(escaped), false, "path traversal target was not created");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
