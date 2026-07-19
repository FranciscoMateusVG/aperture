// R5 — target_user_id cap + PII-stripping for audit emission tests.

import { describe, test, expect } from "vitest";
import { extractTargetUserIds, makeParamsSafe } from "../src/audit.js";

describe("extractTargetUserIds (R5)", () => {
  test("caps at 10, marks truncated, records full count", () => {
    const events = Array.from({ length: 50 }, (_, i) => ({
      user: { id: `u-${i.toString().padStart(3, "0")}` },
    }));
    const response = { events };

    const result = extractTargetUserIds(response);
    expect(result.user_ids).toHaveLength(10);
    expect(result.truncated).toBe(true);
    expect(result.total).toBe(50);
  });

  test("does not truncate when under cap", () => {
    const events = [
      { user: { id: "u-1" } },
      { user: { id: "u-2" } },
      { user: { id: "u-3" } },
    ];
    const result = extractTargetUserIds({ events });
    expect(result.user_ids).toEqual(["u-1", "u-2", "u-3"]);
    expect(result.truncated).toBe(false);
    expect(result.total).toBe(3);
  });

  test("dedupes user IDs", () => {
    const events = [
      { user: { id: "u-1" } },
      { user: { id: "u-1" } },
      { user: { id: "u-2" } },
    ];
    const result = extractTargetUserIds({ events });
    expect(result.total).toBe(2);
  });

  test("handles userId / user_id field variants", () => {
    const events = [
      { userId: "u-a" },
      { user_id: "u-b" },
      { user: { id: "u-c" } },
    ];
    const result = extractTargetUserIds({ events });
    expect(new Set(result.user_ids)).toEqual(new Set(["u-a", "u-b", "u-c"]));
  });

  test("does NOT capture email / username / name fields", () => {
    const events = [
      { user: { id: "u-1", email: "alice@example.com", username: "alice", name: "Alice" } },
    ];
    const result = extractTargetUserIds({ events });
    // Only u-1 should appear; no PII fields anywhere in result.
    expect(result.user_ids).toEqual(["u-1"]);
    // Sanity: dump the result and confirm no email-like string sneaks in.
    const dump = JSON.stringify(result);
    expect(dump).not.toMatch(/alice@example\.com/);
    expect(dump).not.toMatch(/"alice"/);
    expect(dump).not.toMatch(/"Alice"/);
  });

  test("handles numeric user IDs (Sentry sometimes returns ints)", () => {
    const events = [{ user: { id: 42 } }, { userId: 7 }];
    const result = extractTargetUserIds({ events });
    expect(new Set(result.user_ids)).toEqual(new Set(["42", "7"]));
  });

  test("nested traces / issues / arbitrary structure", () => {
    const response = {
      issues: [
        {
          id: "issue-1",
          last_seen_event: { user: { id: "u-deep" } },
        },
      ],
    };
    const result = extractTargetUserIds(response);
    expect(result.user_ids).toContain("u-deep");
  });

  test("empty / null / non-object inputs return empty", () => {
    expect(extractTargetUserIds(null).user_ids).toEqual([]);
    expect(extractTargetUserIds(undefined).user_ids).toEqual([]);
    expect(extractTargetUserIds("string").user_ids).toEqual([]);
    expect(extractTargetUserIds(42).user_ids).toEqual([]);
  });
});

describe("makeParamsSafe", () => {
  test("strips auth/token/api_key fields", () => {
    const safe = makeParamsSafe({
      project: "incluir",
      auth: "secret",
      token: "secret2",
      api_key: "secret3",
      accessToken: "secret4",
      authorization: "Bearer secret5",
    });
    expect(safe).toHaveProperty("project", "incluir");
    expect(JSON.stringify(safe)).not.toContain("secret");
  });

  test("strips justification text (R2 — never log)", () => {
    const safe = makeParamsSafe({
      project: "incluir",
      justification: "I need this attachment for forensics",
    });
    expect(safe).not.toHaveProperty("justification");
    expect(JSON.stringify(safe)).not.toContain("forensics");
  });
});
