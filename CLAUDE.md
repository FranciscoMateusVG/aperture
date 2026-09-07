# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

## Shared operating rules

Read `AGENTS.md` and `.claude/skills/constitution/SKILL.md` for current task, QA, communication and worktree rules. GLaDOS alone discovers/files tasks after operator acknowledgment; specialists fetch assigned beads only. PR-open closure is subject to the bead's explicit acceptance. No blanket follow-up filing, stash clearing, branch deletion or forced push at session end.

Use BEADS for tracking and persistent knowledge, not a parallel markdown TODO or MEMORY.md bank. Existing skill details remain invocable; do not treat older generated boilerplate as permission to create tasks, expand tests or bypass review.



## Build & Test

```bash
just setup           # build the ~/.claude/aperture/ runtime tree from canonical sources
just build-mcp       # compile the MCP server (mcp-server/dist/index.js)
pnpm tauri build     # release build the Tauri app
just check-setup     # verify runtime tree is sane
just status          # full preflight (skills + MCP + BEADS + Docker + agents)
```

## Architecture Overview

Aperture is a Tauri desktop app + tmux + per-agent Claude/Codex CLI sessions, glued by an MCP server (`aperture-bus`).

- **Frontend** — `src/` Vite + vanilla TS launcher (agent cards, model picker, version footer).
- **Tauri backend** — `src-tauri/src/` Rust. Key files: `agent_loader.rs` (loads agents from `~/.claude/aperture/`), `agents.rs` (start/stop/inject_skills), `poller.rs` (BEADS message delivery), `tmux.rs`, `lib.rs` (entry + Tauri commands).
- **MCP server** — `mcp-server/src/` Node TS. Per-agent stdio MCP exposing send_message, BEADS task tools, identity. Filtering and projection on `query_tasks` / `search_tasks`.
- **Agent registry** — `agents/<name>/{manifest.json, skills.txt}` is the canonical source. `prompts/<name>.md` holds the system prompt. `.claude/skills/<skill>/SKILL.md` holds shared skill bodies.
- **Runtime tree** — `~/.claude/aperture/` is symlinked from the repo by `just setup`. Aperture only reads from this tree at boot; the repo is source of truth.

## Conventions & Patterns

- **Agent lanes** — see `AGENTS.md`. Cross-agent delegation flows through GLaDOS via BEADS.
- **BEADS first** — every task tracked, every message persisted via `send_message` (which writes a BEADS row). Operator alerts via `send_message(to: "operator")` light an attention badge but do NOT deliver text — agents reply in the terminal.
- **Project labels mandatory** — every BEADS task carries one `project:<name>` label. No exceptions.
- **Folder-driven agents** — adding/disabling/renaming an agent = editing `agents/<name>/` and re-running `just setup`. No Rust recompile.
