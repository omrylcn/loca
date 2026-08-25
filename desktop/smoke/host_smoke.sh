#!/usr/bin/env bash
# Desktop Host smoke test — the committed regression fence for the standalone
# (bundled-server) flavor's provisioning + Add-agent contracts. It boots
# room-server with the EXACT env desktop/src-tauri/src/main.rs `standalone::spawn`
# uses (closed-door: REQUIRE_INVITE + REQUIRE_SESSIONS, iye reserved), then
# asserts the behaviors the Host relies on. These were previously verified only
# by hand; this script makes them a repeatable fence (run before folding the
# desktop branch).
#
# Covered (the new three-layer / Master behaviors, which the general smoke.sh +
# server suite did NOT yet fence):
#   1. Master seam — LOCA_MASTER_NAME names the /sessions admin session (a real
#      Master), overriding the POST body name; admin=true. (provision_master_session)
#   2. Lobby davet — POST /members returns an mb_ token (Building admission ->
#      Lobby). (add_agent layer 2)
#   3. loca_exists source — GET /rooms lists the reserved home loca iye, so the
#      Add-agent existence pre-check has a real source. (add_agent loca_exists)
#   4. Phantom-loca TRIPWIRE — POST /rooms/<nonexistent>/invites STILL returns
#      200 + a dv_. That is exactly why add_agent pre-checks existence before
#      issuing a Loca davet; without the pre-check a typo'd name would seat the
#      agent in a phantom loca. If a server-side guard ever lands, flip this to
#      expect a rejection (the failing assert is the reminder).
#   5. Reserved iye — an arbitrary agent cannot be seated in iye (non-2xx).
#
# Usage:  ./desktop/smoke/host_smoke.sh
# Requires: cargo, curl, jq, python3. Exits non-zero on the first failed assert.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit   # repo root (the desktop branch is self-contained)

MASTER_NAME="SmokeMaster"
PORT="${PORT:-$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')}"
DB="$(mktemp /tmp/loca-host-smoke-XXXX.db)"
LOG="$(mktemp /tmp/loca-host-smoke-XXXX.log)"
ADMIN="adm-smoke-$RANDOM"
B="http://127.0.0.1:$PORT"
FAILED=0
SRV_PID=""

cleanup() {
  [ -n "$SRV_PID" ] && kill "$SRV_PID" 2>/dev/null
  rm -f "$DB" "$DB"-wal "$DB"-shm "$LOG"
}
trap cleanup EXIT

assert() {  # assert <label> <expected> <actual>
  if [ "$2" = "$3" ]; then printf '  \033[32mok\033[0m   %s\n' "$1"
  else printf '  \033[31mFAIL\033[0m %s  (expected %s, got %s)\n' "$1" "$2" "$3"; FAILED=$((FAILED + 1)); fi
}

echo "building room-server (desktop branch is self-contained)..."
cargo build -p server --bin room-server 2>>"$LOG" || { echo "build failed, see $LOG"; exit 1; }
BIN="target/debug/room-server"

echo "booting Host server (LOCA_MASTER_NAME=$MASTER_NAME, closed-door) on :$PORT"
LOCA_MASTER_NAME="$MASTER_NAME" BIND_ADDR=127.0.0.1 PORT="$PORT" DB_PATH="$DB" ADMIN_TOKEN="$ADMIN" \
  REQUIRE_INVITE=1 REQUIRE_SESSIONS=1 LOCA_AGENT_ROOM=iye RESERVED_LOCA=iye \
  "$BIN" >>"$LOG" 2>&1 &
SRV_PID=$!
for _ in $(seq 1 40); do curl -sf "$B/health" >/dev/null 2>&1 && break; sleep 0.25; done
curl -sf "$B/health" >/dev/null 2>&1 || { echo "server never became ready, see $LOG"; exit 1; }

# 1) Master seam: the admin session is named after LOCA_MASTER_NAME (a real
#    Master), NOT the body name "you" and NOT the legacy "operator"; admin=true.
SESS=$(curl -s -X POST "$B/sessions" -H "x-admin-token: $ADMIN" -H "content-type: application/json" -d '{"name":"you","kind":"user"}')
assert "master session name = LOCA_MASTER_NAME" "$MASTER_NAME" "$(echo "$SESS" | jq -r .name)"
assert "master session admin=true"              "true"         "$(echo "$SESS" | jq -r .admin)"

# 2) Lobby davet: POST /members returns an mb_ token (Building -> Lobby).
MBTOK=$(curl -s -X POST "$B/members" -H "x-admin-token: $ADMIN" -H "content-type: application/json" -d '{"name":"smokebot","kind":"agent"}' | jq -r .token)
assert "lobby davet is an mb_ token" "yes" "$(case "$MBTOK" in mb_*) echo yes;; *) echo "no($MBTOK)";; esac)"

# 3) loca_exists source: GET /rooms lists the reserved home loca iye.
assert "GET /rooms lists iye" "iye" "$(curl -s "$B/rooms" -H "x-admin-token: $ADMIN" | jq -r '.[].room' | grep -x iye)"

# 4) Phantom-loca TRIPWIRE: invite to a nonexistent loca STILL returns 200
#    (this is why add_agent pre-checks existence; flip if a server guard lands).
GHOST=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/rooms/ghost-$RANDOM/invites" -H "x-admin-token: $ADMIN" -H "content-type: application/json" -d '{"name":"smokebot","kind":"agent"}')
assert "phantom-loca invite still 200 (add_agent guards this)" "200" "$GHOST"

# 5) Reserved iye: an arbitrary agent cannot be seated in the home loca (non-2xx).
IYE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$B/rooms/iye/invites" -H "x-admin-token: $ADMIN" -H "content-type: application/json" -d '{"name":"smokebot","kind":"agent"}')
assert "iye rejects an arbitrary agent" "reject" "$([ "${IYE:0:1}" = "2" ] && echo accept || echo reject)"

echo
if [ "$FAILED" -eq 0 ]; then echo "host smoke: ALL GREEN"; else echo "host smoke: $FAILED FAILED"; exit 1; fi
