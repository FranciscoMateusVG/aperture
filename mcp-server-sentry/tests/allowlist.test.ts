// R3 + R4 — allowlist load + project-param coverage tests for Cipher's
// wiring contract (aperture-ttzz).
//
// R3 — fail-closed: missing config, unreadable config, empty project list,
//      malformed YAML → loadAllowlist returns null → caller refuses ALL
//      tool calls.
//
// R4 — every project-identifying param-name shape (10+ variants) plus the
//      defensive shape regex must DENY unauthorised values.

import { describe, test, expect, afterEach } from "vitest";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import {
  loadAllowlist,
  checkProjectAllowlist,
  PROJECT_PARAM_NAMES,
  classifyTool,
} from "../src/gates.js";

function withTempAllowlist(content: string | null, fn: () => void): void {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "sentry-mcp-allowlist-"));
  const tmpFile = path.join(tmpDir, "allowlist.yaml");
  if (content !== null) {
    fs.writeFileSync(tmpFile, content);
  }
  const prev = process.env.SENTRY_MCP_ALLOWLIST_PATH;
  process.env.SENTRY_MCP_ALLOWLIST_PATH = tmpFile;
  try {
    fn();
  } finally {
    if (prev === undefined) delete process.env.SENTRY_MCP_ALLOWLIST_PATH;
    else process.env.SENTRY_MCP_ALLOWLIST_PATH = prev;
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

describe("loadAllowlist (R3 fail-closed)", () => {
  test("returns null when config file is missing", () => {
    const prev = process.env.SENTRY_MCP_ALLOWLIST_PATH;
    process.env.SENTRY_MCP_ALLOWLIST_PATH = "/nonexistent/path/never/exists.yaml";
    try {
      expect(loadAllowlist()).toBeNull();
    } finally {
      if (prev === undefined) delete process.env.SENTRY_MCP_ALLOWLIST_PATH;
      else process.env.SENTRY_MCP_ALLOWLIST_PATH = prev;
    }
  });

  test("returns null when config file is empty", () => {
    withTempAllowlist("", () => {
      expect(loadAllowlist()).toBeNull();
    });
  });

  test("returns null when project_allowlist is empty array", () => {
    const yaml = `project_allowlist: []
agent_default_on: [cipher, rex]
agent_opt_in: []
`;
    withTempAllowlist(yaml, () => {
      expect(loadAllowlist()).toBeNull();
    });
  });

  test("returns null when project_allowlist key is missing", () => {
    const yaml = `agent_default_on: [cipher]
agent_opt_in: []
`;
    withTempAllowlist(yaml, () => {
      expect(loadAllowlist()).toBeNull();
    });
  });

  test("returns null on malformed YAML", () => {
    withTempAllowlist("project_allowlist: [\n  incluir\n  no_closing", () => {
      expect(loadAllowlist()).toBeNull();
    });
  });

  test("loads valid config", () => {
    const yaml = `project_allowlist: [incluir, eunenem-v2, fit]
agent_default_on: [cipher, rex, peppy]
agent_opt_in: [atlas]
`;
    withTempAllowlist(yaml, () => {
      const got = loadAllowlist();
      expect(got).not.toBeNull();
      expect(got!.project_allowlist).toEqual(["incluir", "eunenem-v2", "fit"]);
      expect(got!.agent_default_on).toContain("rex");
      expect(got!.agent_opt_in).toContain("atlas");
    });
  });
});

describe("checkProjectAllowlist (R4 param coverage)", () => {
  const allow = {
    project_allowlist: ["incluir", "eunenem-v2", "fit"],
    agent_default_on: [],
    agent_opt_in: [],
  };

  test.each(PROJECT_PARAM_NAMES)(
    "denies unauthorised value via param name %s",
    (paramName) => {
      const result = checkProjectAllowlist({ [paramName]: "evil-project" }, allow);
      expect(result.ok).toBe(false);
    },
  );

  test.each(PROJECT_PARAM_NAMES)(
    "allows authorised value via param name %s",
    (paramName) => {
      // 'projects' is plural — supply array; others — scalar.
      const value = paramName === "projects" ? ["incluir"] : "incluir";
      const result = checkProjectAllowlist({ [paramName]: value }, allow);
      expect(result.ok).toBe(true);
    },
  );

  test("denies any param that LOOKS like a project param but is not known", () => {
    // Defensive shape regex catches future param-name drift.
    const variants = [
      "project_uuid",
      "projectids",
      "projectIds",
      "organizationIds",
    ];
    for (const v of variants) {
      const result = checkProjectAllowlist({ [v]: "incluir" }, allow);
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.reason).toMatch(/Unknown project-identifying param/);
      }
    }
  });

  test("ignores irrelevant params", () => {
    const result = checkProjectAllowlist({ query: "is:unresolved", limit: 10 }, allow);
    expect(result.ok).toBe(true);
  });

  test("array param: rejects if ANY value is unauthorised", () => {
    const result = checkProjectAllowlist(
      { projects: ["incluir", "evil-project"] },
      allow,
    );
    expect(result.ok).toBe(false);
  });
});

describe("classifyTool", () => {
  test("known mutation tools classify as mutation", () => {
    expect(classifyTool("autofix_issue")).toBe("mutation");
    expect(classifyTool("update_issue")).toBe("mutation");
    expect(classifyTool("delete_issue")).toBe("mutation");
  });

  test("known attachment tool classifies as attachment", () => {
    expect(classifyTool("get_event_attachment")).toBe("attachment");
  });

  test("read tools classify as read", () => {
    expect(classifyTool("search_issues")).toBe("read");
    expect(classifyTool("get_issue")).toBe("read");
    expect(classifyTool("search_docs")).toBe("read");
  });

  test("unknown mutation-shaped tool defaults to mutation", () => {
    expect(classifyTool("create_workflow")).toBe("mutation");
    expect(classifyTool("disable_alert")).toBe("mutation");
  });

  test("unknown read tool defaults to read", () => {
    expect(classifyTool("query_metrics")).toBe("read");
  });
});
