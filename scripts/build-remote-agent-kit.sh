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
# Reuse the credential-safe canonical bundles. A second hand-maintained copy
# list previously omitted join_request.py (and other shipped runtime helpers).
python3 "$ROOT/scripts/build_skill_bundles.py" "$STAGE/skill-bundles" >/dev/null
unzip -q "$STAGE/skill-bundles/loca.zip" -d "$KIT"
unzip -q "$STAGE/skill-bundles/loca-care.zip" -d "$KIT"

find "$KIT" -type d -exec chmod 755 {} +
find "$KIT" -type f -exec chmod 644 {} +
chmod 700 "$KIT/install.sh" "$KIT/start-lobby.sh" \
  "$KIT/stop-lobby.sh" "$KIT/status.sh" "$KIT/loca/connect.sh" \
  "$KIT/loca/runtime.sh" "$KIT/loca/codex_rpc.py"
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
