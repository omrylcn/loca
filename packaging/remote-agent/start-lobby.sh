#!/usr/bin/env bash
set -euo pipefail

find_skill() {
  local candidate
  for candidate in "$HOME/.codex/skills/loca" "$HOME/.claude/skills/loca"; do
    [ -x "$candidate/runtime.sh" ] && { printf '%s' "$candidate"; return 0; }
  done
  return 1
}

SKILL_DIR=$(find_skill) || {
  echo "loca skill not installed" >&2
  exit 3
}
exec "$SKILL_DIR/runtime.sh" start "$@"
