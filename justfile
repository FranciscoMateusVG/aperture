# Aperture Infrastructure Commands
# Usage: just <command>

set shell := ["bash", "-cu"]

# Default: show available commands
default:
    @just --list

# ============== Setup (runtime tree) ==============

# Build the ~/.claude/aperture/ runtime tree from the repo's canonical sources.
# Each agent gets a folder with manifest.json + prompt.md + skills/. All entries
# are symlinks back into the repo, so editing here updates the runtime in place.
# Re-run this whenever you add/remove an agent, skill, or change a manifest.
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    REPO="$(pwd)"
    ROOT="$HOME/.claude/aperture"
    SHARED="$ROOT/shared"

    echo "🔧 Building Aperture runtime tree at $ROOT"
    echo

    # 1) Wipe any stale agent dirs (NOT shared/, which we rebuild below).
    if [ -d "$ROOT" ]; then
        for d in "$ROOT"/*/; do
            [ -d "$d" ] || continue
            name=$(basename "$d")
            [ "$name" = "shared" ] && continue
            rm -rf "$d"
        done
    fi
    mkdir -p "$SHARED"

    # 2) Repopulate shared/ with one symlink per skill in .claude/skills/.
    echo "── Shared skills"
    rm -rf "$SHARED"
    mkdir -p "$SHARED"
    count=0
    for skill_dir in "$REPO"/.claude/skills/*/; do
        [ -d "$skill_dir" ] || continue
        name=$(basename "$skill_dir")
        ln -sfn "$skill_dir" "$SHARED/$name"
        echo "  • $name"
        count=$((count + 1))
    done
    echo "  ✅ $count skills linked into shared/"
    echo

    # 3) For each agent in agents/<name>/, build the runtime folder.
    echo "── Agents"
    agent_count=0
    for agent_dir in "$REPO"/agents/*/; do
        [ -d "$agent_dir" ] || continue
        name=$(basename "$agent_dir")
        manifest="$agent_dir/manifest.json"
        skills_txt="$agent_dir/skills.txt"
        prompt_src="$REPO/prompts/$name.md"

        if [ ! -f "$manifest" ]; then
            echo "  ⚠️  $name: missing manifest.json — skipped"
            continue
        fi
        if [ ! -f "$prompt_src" ]; then
            echo "  ⚠️  $name: missing prompts/$name.md — skipped"
            continue
        fi

        agent_root="$ROOT/$name"
        mkdir -p "$agent_root/skills"
        ln -sfn "$manifest" "$agent_root/manifest.json"
        ln -sfn "$prompt_src" "$agent_root/prompt.md"

        # Wire up each requested skill as a symlink into shared/.
        skill_list=""
        if [ -f "$skills_txt" ]; then
            while IFS= read -r line; do
                skill=$(echo "$line" | sed 's/#.*//' | xargs || true)
                [ -z "$skill" ] && continue
                if [ ! -e "$SHARED/$skill" ]; then
                    echo "  ⚠️  $name → unknown skill '$skill' (not in .claude/skills/)"
                    continue
                fi
                ln -sfn "$SHARED/$skill" "$agent_root/skills/$skill"
                skill_list="$skill_list $skill"
            done < "$skills_txt"
        fi
        echo "  • $name:$skill_list"
        agent_count=$((agent_count + 1))
    done
    echo "  ✅ $agent_count agents wired"
    echo
    echo "Done. Aperture will load from $ROOT on next launch."

# Verify the runtime tree is sane.
check-setup:
    #!/usr/bin/env bash
    set -euo pipefail
    ROOT="$HOME/.claude/aperture"
    if [ ! -d "$ROOT" ]; then
        echo "  ❌ $ROOT does not exist — run: just setup"
        exit 1
    fi
    echo "🔍 Checking $ROOT"
    fail=0
    for agent_dir in "$ROOT"/*/; do
        name=$(basename "$agent_dir")
        [ "$name" = "shared" ] && continue
        if [ ! -e "$agent_dir/manifest.json" ]; then
            echo "  ❌ $name: manifest.json missing or broken symlink"
            fail=1
        elif [ ! -e "$agent_dir/prompt.md" ]; then
            echo "  ❌ $name: prompt.md missing or broken symlink"
            fail=1
        else
            skill_count=$(find "$agent_dir/skills" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')
            echo "  ✅ $name ($skill_count skills)"
        fi
    done
    [ "$fail" = "0" ] && echo "  ✅ runtime tree OK" || exit 1

# ============== BEADS ==============

# Run BEADS diagnostics
doctor:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "🩺 BEADS Diagnostics"
    echo "===================="
    echo ""
    echo "CLI:"
    if command -v bd > /dev/null 2>&1; then
        echo "  ✅ bd $(bd --version 2>&1 | head -1)"
    else
        echo "  ❌ bd not found — install with: brew install beads"
        exit 1
    fi
    if command -v dolt > /dev/null 2>&1; then
        echo "  ✅ dolt $(dolt version 2>&1 | head -1)"
    else
        echo "  ❌ dolt not found — install with: brew install dolt"
        exit 1
    fi
    echo ""
    echo "Database:"
    export BEADS_DIR=~/.aperture/.beads
    export BD_ACTOR=operator
    if [ -d "$BEADS_DIR" ]; then
        echo "  ✅ BEADS directory exists: $BEADS_DIR"
    else
        echo "  ❌ BEADS directory missing — run: mkdir -p ~/.aperture/.beads"
        exit 1
    fi
    bd dolt status 2>&1 | sed 's/^/  /' || true
    echo ""
    echo "Connection test:"
    if bd list --json > /dev/null 2>&1; then
        echo "  ✅ BEADS responding"
    else
        echo "  ❌ BEADS not responding — run: cd ~/.aperture && BEADS_DIR=~/.aperture/.beads bd bootstrap"
    fi

# ============== MCP Server ==============

# Build the MCP server
build-mcp:
    @echo "🔨 Building MCP server..."
    cd mcp-server && pnpm install && pnpm build
    @echo "✅ MCP server built"

# Build the Sentry MCP wrap server (aperture-ttzz)
build-mcp-sentry:
    @echo "🔨 Building Sentry MCP wrap server..."
    cd mcp-server-sentry && pnpm install && pnpm build
    @echo "✅ Sentry MCP wrap server built"

# Run the Sentry MCP wrap server's test suite
test-mcp-sentry:
    @echo "🧪 Running Sentry MCP wrap server tests..."
    cd mcp-server-sentry && pnpm test
    @echo "✅ Sentry MCP wrap tests passed"

# Build BOTH MCP servers — used by `just setup-all`.
build-mcp-all: build-mcp build-mcp-sentry

# Check MCP server config
check-mcp:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "🔍 Checking MCP server..."
    mcp_path=$(cat .claude/settings.json | grep -o '"command": "[^"]*"' | head -1 | sed 's/"command": "//;s/"//')
    if [ -f "$mcp_path" ]; then
        echo "  ✅ MCP server script exists: $mcp_path"
    else
        echo "  ❌ MCP server script not found: $mcp_path"
        echo "     Update .claude/settings.json with the correct path to mcp-server/start.sh"
        exit 1
    fi
    if [ -f "mcp-server/dist/index.js" ]; then
        echo "  ✅ MCP server compiled"
    else
        echo "  ❌ MCP server not compiled — run: just build-mcp"
        exit 1
    fi

# ============== Status ==============

# Full system health check (pre-flight)
status:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "📊 Aperture System Status"
    echo "========================="
    echo ""
    echo "── Runtime tree ──"
    just check-setup
    echo ""
    echo "── MCP Server ──"
    just check-mcp
    echo ""
    echo "── BEADS ──"
    just doctor
    echo ""
    echo "── Docker ──"
    docker info > /dev/null 2>&1 && echo "  ✅ Docker is running" || echo "  ⚠️  Docker is not running (needed for deploys)"
    echo ""
    echo "── Agents ──"
    for agent in glados wheatley peppy izzy vance rex; do
        if pgrep -f "name $agent" > /dev/null 2>&1; then
            echo "  ✅ $agent is running"
        else
            echo "  ⚪ $agent is not running"
        fi
    done
    echo ""
    echo "========================="
    echo "Run 'just status' before each session to catch issues early."
