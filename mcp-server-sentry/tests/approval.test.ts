// R1 — approval flow tests for Cipher's wiring contract.
//
// We exercise the request-id generator and the body-formatter directly
// (pure functions). The end-to-end bd round-trip is integration-tested
// in Izzy's aperture-echr (separate task, blocked on this PR). This unit
// suite covers the deterministic shape pieces.

import { describe, test, expect } from "vitest";
import { newRequestId } from "../src/approval.js";

describe("newRequestId", () => {
  test("generates unique IDs", () => {
    const ids = new Set<string>();
    for (let i = 0; i < 100; i++) ids.add(newRequestId());
    expect(ids.size).toBe(100);
  });

  test("uses 'appr-' prefix", () => {
    expect(newRequestId()).toMatch(/^appr-[a-f0-9]{8}$/);
  });

  test("IDs are short (suitable for grep in operator reply)", () => {
    expect(newRequestId().length).toBeLessThanOrEqual(16);
  });
});

describe("approval module contract", () => {
  test("exports requestApproval, setWrapLayerAgentName, newRequestId", async () => {
    const mod = await import("../src/approval.js");
    expect(typeof mod.requestApproval).toBe("function");
    expect(typeof mod.setWrapLayerAgentName).toBe("function");
    expect(typeof mod.newRequestId).toBe("function");
  });
});
