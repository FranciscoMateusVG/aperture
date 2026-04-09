# Aperture Infrastructure Commands
# Usage: just <command>

set shell := ["bash", "-cu"]

# Default: show available commands
default:
    @just --list

# ============== Skills ==============

# Set up skills symlink (idempotent — safe to re-run)
setup-skills:
    @echo "🔗 Setting up Aperture skills symlink..."
    ln -sfn "$(pwd)/skills/aperture" ~/.claude/skills/aperture
    @echo "✅ Symlink created: ~/.claude/skills/aperture -> $(pwd)/skills/aperture"
    @just check-skills

# Verify skills symlink is healthy
check-skills:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "🔍 Checking Aperture skills..."
    link=~/.claude/skills/aperture
    if [ -L "$link" ]; then
        target=$(readlink "$link")
        if [ -d "$target" ]; then
            count=$(ls -1 "$target"/*.md 2>/dev/null | wc -l | tr -d ' ')
            echo "  ✅ Symlink: $link -> $target"
            echo "  📄 Skills found: $count"
            for f in "$target"/*.md; do
                name=$(basename "$f" .md)
                echo "     • aperture:$name"
            done
        else
            echo "  ❌ Symlink exists but target directory missing: $target"
            echo "     Run: just setup-skills"
            exit 1
        fi
    else
        echo "  ❌ No symlink at $link"
        echo "     Run: just setup-skills"
        exit 1
    fi

# ============== Status ==============

# Full system health check
status:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "📊 Aperture System Status"
    echo "========================="
    echo ""
    echo "Skills:"
    just check-skills
    echo ""
    echo "Docker:"
    docker info > /dev/null 2>&1 && echo "  ✅ Docker is running" || echo "  ❌ Docker is not running"
    echo ""
    echo "Agents:"
    for agent in glados wheatley peppy izzy; do
        if pgrep -f "name $agent" > /dev/null 2>&1; then
            echo "  ✅ $agent is running"
        else
            echo "  ⚪ $agent is not running"
        fi
    done
