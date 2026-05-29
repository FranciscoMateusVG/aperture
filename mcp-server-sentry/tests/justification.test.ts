// R2 — justification shape tests for Cipher's wiring contract.

import { describe, test, expect } from "vitest";
import { checkJustification } from "../src/gates.js";

describe("checkJustification (R2)", () => {
  test("rejects missing justification", () => {
    const r = checkJustification({});
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/requires a "justification"/);
  });

  test("rejects non-string justification", () => {
    const r = checkJustification({ justification: 42 });
    expect(r.ok).toBe(false);
  });

  test("rejects justification under 20 chars", () => {
    const r = checkJustification({ justification: "too short" });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/too short/);
  });

  test("rejects justification over 500 chars", () => {
    const r = checkJustification({ justification: "x".repeat(501) });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/too long/);
  });

  test("accepts justification at minimum length (20)", () => {
    const r = checkJustification({ justification: "x".repeat(20) });
    expect(r.ok).toBe(true);
  });

  test("accepts justification at maximum length (500)", () => {
    const r = checkJustification({ justification: "x".repeat(500) });
    expect(r.ok).toBe(true);
  });

  test("trims whitespace before length check", () => {
    const r = checkJustification({ justification: "   " + "x".repeat(15) + "   " });
    expect(r.ok).toBe(false);
  });

  test("accepts a realistic justification", () => {
    const r = checkJustification({
      justification:
        "Investigating CSP report attached to issue ABC-123 — operator approved offline.",
    });
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.text).toMatch(/Investigating CSP/);
  });
});
