#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUT_DIR="$ROOT/dist"
ARCHIVE="$OUT_DIR/loca-remote-agent.zip"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
KIT="$STAGE/loca-remote-agent"

mkdir -p "$KIT/loca" "$KIT/loca-care/agents" "$KIT/loca-care/scripts" "$OUT_DIR"
cp "$ROOT/packaging/remote-agent/README.md" "$KIT/"
"$ROOT/scripts/version.sh" > "$KIT/VERSION"
cp "$ROOT/packaging/remote-agent/install.sh" "$KIT/"
cp "$ROOT/packaging/remote-agent/start-lobby.sh" "$KIT/"
cp "$ROOT/packaging/remote-agent/stop-lobby.sh" "$KIT/"
cp "$ROOT/packaging/remote-agent/status.sh" "$KIT/"
cp "$ROOT/skill/agent-room/SKILL.md" "$KIT/loca/"
cp "$ROOT/skill/agent-room/VERSION" "$KIT/loca/"
cp "$ROOT/skill/agent-room/connect.sh" "$KIT/loca/"
cp "$ROOT/skill/agent-room/credentials.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/listen.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/monitor_listener.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/bot.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/nudge.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/orchestrator_queue.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/attention_store.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/codex_adapter_v2.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/runtime.sh" "$KIT/loca/"
cp "$ROOT/skill/agent-room/runtime_agent.py" "$KIT/loca/"
cp "$ROOT/skill/agent-room/runtime_consumer.py" "$KIT/loca/"
mkdir -p "$KIT/loca/agents" "$KIT/loca/references"
cp "$ROOT/skill/agent-room/agents/openai.yaml" "$KIT/loca/agents/"
cp "$ROOT/skill/agent-room/references/runtimes.md" "$KIT/loca/references/"
cp "$ROOT/skill/agent-room/references/codex-orchestrator.md" "$KIT/loca/references/"
cp "$ROOT/skill/agent-room/references/adapter-protocol-v1.md" "$KIT/loca/references/"
cp "$ROOT/skill/agent-room/references/adapter-protocol-v2.md" "$KIT/loca/references/"
cp "$ROOT/skill/loca-care/SKILL.md" "$KIT/loca-care/"
cp "$ROOT/skill/loca-care/VERSION" "$KIT/loca-care/"
cp "$ROOT/skill/loca-care/agents/openai.yaml" "$KIT/loca-care/agents/"
cp "$ROOT/skill/loca-care/scripts/audit.py" "$KIT/loca-care/scripts/"

find "$KIT" -type d -exec chmod 755 {} +
find "$KIT" -type f -exec chmod 644 {} +
chmod 700 "$KIT/install.sh" "$KIT/start-lobby.sh" \
  "$KIT/stop-lobby.sh" "$KIT/status.sh" "$KIT/loca/connect.sh" \
  "$KIT/loca/runtime.sh" "$KIT/loca/nudge.py"
chmod 700 "$KIT/loca/orchestrator_queue.py"
chmod 700 "$KIT/loca/attention_store.py" "$KIT/loca/codex_adapter_v2.py"
chmod 700 "$KIT/loca/monitor_listener.py"
chmod 700 "$KIT/loca/runtime_agent.py" "$KIT/loca/runtime_consumer.py"
chmod 700 "$KIT/loca-care/scripts/audit.py"
CHECKSUM_FILE="$STAGE/SHA256SUMS.tmp"
(
  cd "$KIT"
  find . -type f ! -name SHA256SUMS -print0 |
    sort -z |
    xargs -0 sha256sum > "$CHECKSUM_FILE"
)
mv "$CHECKSUM_FILE" "$KIT/SHA256SUMS"
chmod 644 "$KIT/SHA256SUMS"

# ZIP stores file mtimes and traversal order. Normalize both so the same Git
# source produces the same release artifact across runs and time zones.
# ZIP's DOS timestamp floor is 1980-01-01. A fixed default keeps the artifact
# content-addressable even when an unrelated documentation-only commit changes
# HEAD's timestamp. Release automation may override this standard variable.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-315532800}
case "$SOURCE_DATE_EPOCH" in
  ''|*[!0-9]*)
    echo "SOURCE_DATE_EPOCH must be an integer Unix timestamp" >&2
    exit 2
    ;;
esac
find "$KIT" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
rm -f "$ARCHIVE"
(
  cd "$STAGE"
  mapfile -d '' -t entries < <(
    find loca-remote-agent -type f -print0 | LC_ALL=C sort -z
  )
  TZ=UTC zip -qX "$ARCHIVE" "${entries[@]}"
)
echo "$ARCHIVE"
