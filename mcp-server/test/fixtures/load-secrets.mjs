// Loader for memory-secrets.json: re-joins each secret's `text_parts` fragments into the runtime
// `text` the redaction tests exercise. Fragments exist only so the file at rest never contains a
// contiguous detector-matching literal (see the fixture's _comment). Everything else passes through.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export function loadSecretsFixture(path = join(dirname(fileURLToPath(import.meta.url)), "memory-secrets.json")) {
  const raw = JSON.parse(readFileSync(path, "utf8"));
  const secrets = {};
  for (const [name, v] of Object.entries(raw.secrets)) {
    if (!Array.isArray(v.text_parts)) throw new Error(`fixture secret ${name}: text_parts missing — the fixture must not carry contiguous literals`);
    const { text_parts, ...rest } = v;
    secrets[name] = { ...rest, text: text_parts.join("") };
  }
  return { ...raw, secrets };
}
