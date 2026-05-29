// R6 — token-redaction unit tests for Cipher's wiring contract (aperture-ttzz).
//
// Asserts that no log line, audit body, error message, or stderr emission
// from any wrap-layer code path ever contains:
//   - The literal token verbatim
//   - The first 8 chars of the token in isolation
//   - The literal "Bearer " (case-insensitive) followed by anything
//   - An Authorization header value in any form
//
// The harness injects a known synthetic token via createRedactor() and
// pushes every input that the wrap layer could conceivably emit through
// redact() / redactObject(), then greps the outputs.

import { describe, test, expect } from "vitest";
import { createRedactor } from "../src/redact.js";

const SYNTHETIC_TOKEN = "sntry_test_TOKEN_DEADBEEF_0123456789abcdef";

describe("redact (R6)", () => {
  const r = createRedactor(SYNTHETIC_TOKEN);

  test("redacts the full token verbatim", () => {
    const input = `request: GET /api/foo with secret=${SYNTHETIC_TOKEN}&x=y`;
    const out = r.redact(input);
    expect(out).not.toContain(SYNTHETIC_TOKEN);
    expect(out).toContain("[REDACTED]");
  });

  test("redacts the first 8-character prefix of the token", () => {
    const prefix = SYNTHETIC_TOKEN.slice(0, 8);
    const input = `truncated dump: ${prefix} ...rest stripped`;
    const out = r.redact(input);
    expect(out).not.toContain(prefix);
  });

  test("redacts Bearer headers regardless of token match", () => {
    const input = `Authorization: Bearer arbitrary-token-not-ours_xyz123`;
    const out = r.redact(input);
    expect(out).toMatch(/Bearer \[REDACTED\]/);
    expect(out).not.toContain("arbitrary-token-not-ours_xyz123");
  });

  test("redacts Bearer with case variants", () => {
    expect(r.redact("bearer some_value")).toContain("[REDACTED]");
    expect(r.redact("BEARER some_value")).toContain("[REDACTED]");
    expect(r.redact("bEaReR some_value")).toContain("[REDACTED]");
  });

  test("redacts Authorization headers in JSON form", () => {
    const input = JSON.stringify({ headers: { authorization: SYNTHETIC_TOKEN } });
    const out = r.redact(input);
    expect(out).not.toContain(SYNTHETIC_TOKEN);
  });

  test("deep-redacts an object via redactObject", () => {
    const obj = {
      tool: "search_issues",
      params: { project: "incluir", auth: SYNTHETIC_TOKEN },
      nested: {
        headers: { Authorization: `Bearer ${SYNTHETIC_TOKEN}` },
      },
    };
    const out = r.redactObject(obj);
    const serialised = JSON.stringify(out);
    expect(serialised).not.toContain(SYNTHETIC_TOKEN);
    expect(serialised).not.toContain(SYNTHETIC_TOKEN.slice(0, 8));
  });

  test("no-token redactor still strips Bearer patterns", () => {
    const empty = createRedactor(null);
    const out = empty.redact("Authorization: Bearer foo-bar-baz");
    expect(out).toMatch(/Bearer \[REDACTED\]/);
  });

  test("handles empty / non-string input without throw", () => {
    expect(r.redact("")).toBe("");
    expect(r.redactObject(null)).toBeNull();
    expect(r.redactObject(undefined)).toBeUndefined();
  });
});
