#!/bin/bash
# aperture/skills/setup.sh
# Creates symlinks for Aperture skills in ~/.claude/skills/

SKILLS_DIR="$HOME/.claude/skills"
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$SKILLS_DIR"

for skill_dir in "$REPO_DIR"/aperture/*/; do
  skill_name=$(basename "$skill_dir")
  target="$SKILLS_DIR/aperture-$skill_name"

  [ -L "$target" ] && rm "$target"
  ln -s "$skill_dir" "$target"
  echo "✅ Linked: aperture-$skill_name"
done
