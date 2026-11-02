#!/usr/bin/env node
// aperture-trust — mark directories as trusted in Claude Code's config, so a
// Claude team member launched there doesn't stop at "Do you trust the files in
// this folder?" with nobody at the keyboard.
//
// Usage: aperture-trust --config <path to .claude.json> <dir> [<dir>...]
// Sets projects[<dir>].hasTrustDialogAccepted = true and leaves everything
// else as it was (key order, other projects, formatting style). Writes via a
// temp file + rename. A missing or unreadable config is left alone (exit 1).
// Needs Node 22+. No dependencies.

import { readFileSync, writeFileSync, renameSync, statSync, chmodSync } from "node:fs";

const argv = process.argv.slice(2);
const i = argv.indexOf("--config");
if (i < 0 || !argv[i + 1]) {
  process.stderr.write("aperture-trust: --config <path> is required\n");
  process.exit(2);
}
const config = argv[i + 1];
const dirs = argv.filter((_, j) => j !== i && j !== i + 1);

let data;
try {
  data = JSON.parse(readFileSync(config, "utf8"));
} catch (err) {
  process.stderr.write(`aperture-trust: cannot read ${config}: ${err.message}\n`);
  process.exit(1);
}

data.projects ??= {};
let changed = false;
for (const dir of dirs) {
  const entry = (data.projects[dir] ??= {});
  if (entry.hasTrustDialogAccepted !== true) {
    entry.hasTrustDialogAccepted = true;
    changed = true;
  }
}

if (changed) {
  const tmp = `${config}.aperture-${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(data, null, 2));
  chmodSync(tmp, statSync(config).mode & 0o777);
  renameSync(tmp, config);
}
process.stdout.write(changed ? "trusted\n" : "already trusted\n");
