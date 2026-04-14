# Extending Aperture: Non-MCP Agent Models

> **Document Purpose:** Step-by-step guide for onboarding new AI model types (OpenAI GPT, Anthropic Sonnet, etc.) that cannot natively call MCP tools into the Aperture orchestration system.
>
> **Reference:** Extends codex-beads-bridge.md Section 8 (Extensibility). This guide assumes you have read the Codex harness architecture in that document and understand the `@@BEADS@@` command block pattern.
>
> **Status:** Live — template tested with Codex. Adding a new model = following this checklist.

---

## Overview: The Non-MCP Problem

Aperture's coordination layer (BEADS) depends on two communication mechanisms:

| Mechanism | Claude Code | Non-MCP Models |
|-----------|-------------|-----------------|
| **Poller tmux injection** | ✅ Reads output as context | ❌ Ignored by model's own REPL |
| **MCP tool calls** | ✅ Native support | ❌ Not available / not configured |

Result: **Non-MCP models are communication-isolated.** They cannot call BEADS operations (send messages, update tasks, store artifacts) and cannot receive messages via the standard delivery path.

The **BEADS Proxy Harness** solves this by intercepting the model's output, parsing structured command blocks, and executing BEADS operations on the model's behalf.

---

## The Extensible Pattern

The Codex harness (`src-tauri/src/codex_harness.rs`) is **model-agnostic**. It uses:

1. **A model-independent command syntax** (`@@BEADS@@` blocks) — identical for all model types
2. **A model-independent parser** (`src-tauri/src/beads_parser.rs`) — reusable without modification
3. **A model-independent executor** (inside `codex_harness.rs`) — pure BEADS CLI calls

What DOES change per model:

1. **Model prefix** — how to detect the model type (e.g., `codex/` vs. `gpt4o/` vs. `claude/` inside Aperture)
2. **API launcher** — how to spawn the model (Codex CLI vs. OpenAI CLI vs. curl against an API endpoint)
3. **Configuration** — where to write system prompts, auth tokens, model parameters, etc.
4. **Output monitoring** — how to capture the model's output (tmux scraping works for CLI models; API-based models need response interception)

---

## 5-Step Extensibility Checklist

### Step 1: Define the Model Prefix and Update Model Validation

In `src-tauri/src/agents.rs` line 400, add your model type to the validation list:

**Current (Codex example):**
```rust
let valid = matches!(model.as_str(), "opus" | "sonnet" | "haiku") || model.starts_with("codex/");
if !valid {
    return Err(format!("Invalid model '{}'. Must be opus/sonnet/haiku or codex/<model>", model));
}
```

**Extended (adding GPT-4o):**
```rust
let valid = matches!(model.as_str(), "opus" | "sonnet" | "haiku") 
    || model.starts_with("codex/")
    || model.starts_with("gpt4o/");
if !valid {
    return Err(format!("Invalid model '{}'. Must be opus/sonnet/haiku, codex/<model>, or gpt4o/<model>", model));
}
```

### Step 2: Create a Harness Module for Your Model Type

Create `src-tauri/src/<model>_harness.rs`. Follow the Codex harness template:

```rust
//! <Model> BEADS Bridge — Harness
//!
//! Three jobs:
//! 1. Pre-prompt injection — query BEADS for unread messages at agent startup
//! 2. Message buffering — accumulate messages for running agents (alternative to tmux injection)
//! 3. Output monitoring — scan model output for @@BEADS@@ blocks and execute them

use crate::beads_parser::{parse_beads_blocks, BeadsCommand};
use std::fs;

// Copy the entire structure from codex_harness.rs:
// - Helper functions for BEADS CLI (bd_path, beads_dir, etc.)
// - pub fn inject_pending_messages(agent_name: &str, prompt: String) -> String
// - pub fn buffer_pending_message(agent_name: &str, formatted: &str)
// - pub fn start_output_monitor(window_id: String, agent_name: String)
// - execute_command(cmd: &BeadsCommand, agent_name: &str)
// 
// Only change:
// - The model-specific paths/dirs if needed (e.g., gpt4o vs. codex home)
// - The output monitoring logic if the model doesn't expose tmux
// - Any model-specific parsing (if needed; most models output plain text)
```

**Key insight:** The parser and executor are **identical across models**. Copy-paste the Codex harness, then modify only the parts specific to your model's deployment (paths, launcher style, output format).

### Step 3: Wire the Harness into `agents.rs`

In `start_agent()`, add a branch parallel to the Codex branch (lines 62-114):

**Pattern:**
```rust
let launcher_script = if agent.model.starts_with("codex/") {
    // ... existing Codex logic ...
} else if agent.model.starts_with("gpt4o/") {
    let bare_model = agent.model.trim_start_matches("gpt4o/");
    let gpt4o_home = format!("/tmp/aperture-gpt4o-{}", name);
    
    // Create home directory
    fs::create_dir_all(&gpt4o_home).map_err(|e| e.to_string())?;
    
    // Read and prepare prompt
    let prompt_content = fs::read_to_string(&agent.prompt_file)
        .map_err(|e| format!("Failed to read prompt file '{}': {}", agent.prompt_file, e))?;
    let prompt_content = inject_skills(prompt_content, &project_dir);
    // Inject pending BEADS messages
    let prompt_content = gpt4o_harness::inject_pending_messages(&name, prompt_content);
    let prompt_dest = format!("{}/prompt.md", gpt4o_home);
    fs::write(&prompt_dest, &prompt_content).map_err(|e| e.to_string())?;
    
    // Write configuration (model-specific format; OpenAI might use JSON vs. TOML)
    let config_path = format!("{}/config.json", gpt4o_home);
    let config = serde_json::json!({
        "model": bare_model,
        "prompt_file": prompt_dest,
        "api_key": std::env::var("OPENAI_API_KEY").unwrap_or_default(),
        // ... other OpenAI-specific config ...
    });
    fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap())
        .map_err(|e| e.to_string())?;
    
    // Launcher script (model-specific invocation)
    format!(
        r#"#!/bin/bash
export GPT4O_HOME="{gpt4o_home}"
export OPENAI_API_KEY="$OPENAI_API_KEY"
exec openai-cli agent --config {config_path} --model {bare_model}
"#,
        gpt4o_home = gpt4o_home,
        config_path = config_path,
        bare_model = bare_model,
    )
} else {
    // ... existing Claude agent logic ...
}
```

Then, after launching, start the harness (lines 144-146):

```rust
if agent.model.starts_with("codex/") {
    codex_harness::start_output_monitor(window_id.clone(), name.clone());
} else if agent.model.starts_with("gpt4o/") {
    gpt4o_harness::start_output_monitor(window_id.clone(), name.clone());
}
```

### Step 4: Update the Module Exports

In `src-tauri/src/lib.rs`, add your new harness module:

```rust
mod codex_harness;
mod gpt4o_harness;  // ← add this
mod beads_parser;
// ... rest of modules
```

### Step 5: Update Agent Skill — Document Model-Specific Behavior

Create `.claude/skills/gpt4o-comms/SKILL.md` (parallel to `codex-comms/SKILL.md`).

Copy the structure from `codex-comms/SKILL.md`, but add model-specific notes:

```markdown
# GPT-4o Agent Communication Protocol

> **This skill applies to you if you are running as a GPT-4o agent** 
> (your model identifier starts with `gpt4o/`).

... (rest identical to codex-comms, with model-specific tweaks) ...

## Model-Specific Notes

- **System prompt interpretation:** GPT-4o follows OpenAI-style instructions (use "You are..." instead of Anthropic's preferred format)
- **Output format:** Plain text response (no special markdown mode)
- **@@BEADS@@ blocks:** Identical syntax; GPT-4o should emit them in regular text (not in code fences)
- **Token limits:** Be conservative with prompt padding — OpenAI's context is smaller than Claude's
```

---

## Implementation Path: Example — Adding OpenAI GPT-4o

### Phase 1: Harness Implementation

**Files to create:**
- `src-tauri/src/gpt4o_harness.rs` — copy `codex_harness.rs`, adapt paths/output monitoring
- `.claude/skills/gpt4o-comms/SKILL.md` — copy `codex-comms/SKILL.md`, add OpenAI-specific notes

**Files to modify:**
- `src-tauri/src/agents.rs` — add `gpt4o/` branch (lines 62-114, 144-146), update validation (line 400)
- `src-tauri/src/lib.rs` — add `mod gpt4o_harness;`

**Test coverage:**
- Parser: reuse existing `src-tauri/tests/beads_parser_test.rs` (no changes — parser is model-agnostic)
- Harness: create `src-tauri/tests/gpt4o_harness_test.rs` (test output monitoring, message injection)
- E2E: spawn a GPT-4o agent, emit `@@BEADS@@` blocks, verify BEADS operations succeed

### Phase 2: Prompt Engineering

GPT-4o may have different instruction-following characteristics than Codex. Test the `@@BEADS@@` syntax in isolation:

```bash
# Test block emission
echo "Respond with: @@BEADS send_message to:glados message:\"test\"@@" \
  | openai api chat.completions.create \
    --model gpt-4o \
    --system-prompt "Emit @@BEADS blocks for BEADS operations."
```

If GPT-4o hallucinates or reformats blocks, update the system prompt or the parser's error handling.

### Phase 3: Integration

1. **Merge harness code** — PR with `gpt4o_harness.rs` + agent.rs changes
2. **Deploy and test** — spawn a real GPT-4o agent, run end-to-end BEADS workflow
3. **Document findings** — update `gpt4o-comms/SKILL.md` with any quirks discovered during testing
4. **Validation** — model validation in agents.rs should accept `gpt4o/<model>` from that point forward

---

## Testing Your Model Integration

### Manual Verification Checklist

- [ ] **Model launches** — tmux window shows agent REPL
- [ ] **Messages inject** — pre-prompt injection works; agent sees startup messages
- [ ] **Blocks parse** — agent emits `@@BEADS@@` blocks; parser extracts commands
- [ ] **BEADS executes** — harness runs `bd` CLI calls; observer logs show "OK" status
- [ ] **Round-trip** — send agent a BEADS message → agent receives it → agent responds with block → block executes → loop closes

### Automated Tests

Create `src-tauri/tests/<model>_harness_test.rs`:

```rust
#[test]
fn test_output_monitor_parses_blocks() {
    let output = "Some response text\n@@BEADS send_message to:test message:\"hi\"@@\nMore text";
    let commands = parse_beads_blocks(output);
    assert_eq!(commands.len(), 1);
    // Verify parsed correctly
}

#[test]
fn test_pre_prompt_injection() {
    let prompt = "Original prompt";
    let messages = vec![/* test message */];
    let injected = inject_pending_messages_with_messages("agent", prompt, messages);
    assert!(injected.contains("BEADS MESSAGES"));
    assert!(injected.contains("Original prompt"));
}
```

---

## Common Patterns & Gotchas

### Pattern 1: Models with Different Output Formats

If your model outputs structured JSON (not plain text), adapt the output monitoring:

```rust
// Instead of scanning raw tmux pane text:
let output = tmux::tmux_capture_pane(&window_id)?;

// Parse JSON response, then scan for blocks in the text field:
let json: serde_json::Value = serde_json::from_str(&output)?;
let text = json["choices"][0]["message"]["content"].as_str().unwrap_or("");
let commands = parse_beads_blocks(text);
```

### Pattern 2: API-Based Models (No tmux)

If your model is accessed via an API (not a CLI), you can't use tmux scraping. Instead:

1. Intercept API responses in a middleware / client wrapper
2. Scan the response text for `@@BEADS@@` blocks
3. Execute blocks, then return the response to the model

Example (pseudo-code):

```rust
fn call_openai_api(messages: Vec<Message>) -> Result<String> {
    let response = openai::create_chat_completion(messages)?;
    let text = &response.choices[0].message.content;
    
    // Scan for and execute BEADS blocks
    let commands = parse_beads_blocks(text);
    for cmd in commands {
        execute_command(&cmd, agent_name);
    }
    
    Ok(text.to_string())
}
```

### Pattern 3: Models That Reformatted Blocks

Some models may reformat `@@BEADS@@` blocks (add spaces, wrap in code fences, etc.). Adapt the parser regex:

```rust
// Current: @@BEADS ... @@
// Robust: @@ ?BEADS ?... ?@@ (allow optional spaces around markers)
let regex = r"@{2}\s*BEADS\s+(.*?)\s*@{2}";
```

---

## Architecture Decision: When to Fork the Harness vs. Reuse

| Scenario | Recommendation |
|----------|-----------------|
| Model outputs plain text, has CLI, uses tmux | **Reuse codex_harness** — minimal changes |
| Model outputs JSON, uses API, no tmux | **Fork and adapt** — response interception replaces output monitoring |
| Model follows different instruction style | **Reuse harness, fork the skill** — only agent-facing docs change |
| Model cannot reliably emit structured blocks | **Escalate to GLaDOS** — may need different coordination syntax |

---

## Rollout Checklist — Pre-Merge

- [ ] Model harness module created and tested
- [ ] `agents.rs` updated with model prefix detection and launch branch
- [ ] `lib.rs` module export added
- [ ] Model skill created (`.claude/skills/<model>-comms/SKILL.md`)
- [ ] Unit tests pass (parser, harness, specific blocks)
- [ ] E2E test: spawn agent → emit block → verify BEADS operation
- [ ] Model validation updated (agents.rs line 400)
- [ ] Documentation: README updated with supported models
- [ ] **Knowledge artifact stored in BEADS** — findings from testing, any quirks discovered

---

## Example: Full Commit Message (When Adding a New Model)

```
feat(aperture): add support for OpenAI GPT-4o via BEADS harness

- Create gpt4o_harness.rs: pre-prompt injection, message buffering, output monitoring
- Wire gpt4o/ model prefix to harness in agents.rs
- Add gpt4o-comms skill for agent-facing protocol docs
- Update model validation to accept gpt4o/<model>
- Add integration tests: output parsing, BEADS round-trip

Model uses identical @@BEADS@@ syntax as Codex.
Parser and executor unchanged — harness is model-agnostic.

Tested:
- Agent launch and REPL startup
- Pre-prompt message injection
- @@BEADS send_message, update_task, store_artifact, close_task blocks
- Round-trip: message → agent → block → BEADS operation → confirmation

Closes: src-tauri-kvg
```

---

## Future: Making This Fully Pluggable

Right now, adding a model type requires code changes. To make it fully pluggable:

1. **Model registry** — TOML/JSON file listing available models and their harness types
2. **Trait-based harness** — define a `NonMcpHarness` trait; all models implement it
3. **Runtime loading** — Tauri backend loads harness modules dynamically based on model prefix

This would allow new models to be added without recompiling. **Defer this until Aperture has 3+ non-MCP model types.**

---

## Appendix: Key Files Reference

| File | Purpose | When Modifying |
|------|---------|-----------------|
| `src-tauri/src/agents.rs` | Agent launcher | Adding model prefix, launch logic, harness wiring |
| `src-tauri/src/<model>_harness.rs` | Message delivery & execution | Model-specific output monitoring, paths |
| `src-tauri/src/beads_parser.rs` | `@@BEADS@@` block parsing | Extending command syntax (rare — versioned) |
| `.claude/skills/<model>-comms/SKILL.md` | Agent-facing protocol | Model-specific instruction style, quirks |
| `docs/codex-beads-bridge.md` | Architecture & @@BEADS@@ spec | Reference only — do not modify for each model |

---

## Version

| Field | Value |
|-------|-------|
| Document | Extending Non-MCP Agents |
| Version | 1.0.0 |
| Updated | 2026-04-13 |
| Owner | atlas |
| Reference | Task `src-tauri-kvg` |
