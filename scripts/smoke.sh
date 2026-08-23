#!/usr/bin/env bash
# End-to-end smoke test: boots a fresh room-server on a temp DB and exercises
# every REST surface (auth, messages, notes, modes, rate limit, settings,
# persistence, reply, health/epoch) with curl, asserting status codes.
#
# Usage:  ./scripts/smoke.sh
# Requires: cargo, curl, jq. Exits non-zero on the first failed assertion.

set -uo pipefail
cd "$(dirname "$0")/.." || exit

PORT="${PORT:-$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')}"
DB="$(mktemp /tmp/agent-room-smoke-XXXX.db)"
LOG="$(mktemp /tmp/agent-room-smoke-XXXX.log)"
CARE_ENV="$(mktemp /tmp/loca-care-smoke-XXXX.env)"
ADMIN="adm-$RANDOM"
ROOM="join-$RANDOM"
B="http://127.0.0.1:$PORT"
FAILED=0
SRV_PID=""

cleanup() {
  [ -n "$SRV_PID" ] && kill "$SRV_PID" 2>/dev/null
  rm -f "$DB" "$DB"-wal "$DB"-shm
  rm -f "$LOG"
  rm -f "$CARE_ENV"
}
trap cleanup EXIT

assert() {  # assert <label> <expected> <actual>
  if [ "$2" = "$3" ]; then
    printf '  \033[32mok\033[0m   %s\n' "$1"
  else
    printf '  \033[31mFAIL\033[0m %s  (expected %s, got %s)\n' "$1" "$2" "$3"
    FAILED=$((FAILED + 1))
  fi
}

# code <method> <path> <data|-> [extra curl args...]   ("-" = no body)
code() {
  local method="$1" path="$2" data="$3"; shift 3
  if [ "$data" = "-" ]; then
    curl -s -o /dev/null -w '%{http_code}' -X "$method" "$B$path" "$@"
  else
    curl -s -o /dev/null -w '%{http_code}' -X "$method" "$B$path" \
      -H 'content-type: application/json' -d "$data" "$@"
  fi
}

wait_server() {
  for _ in $(seq 1 50); do
    if ! kill -0 "$SRV_PID" 2>/dev/null; then
      printf 'server exited during startup:\n' >&2
      tail -40 "$LOG" >&2
      return 1
    fi
    curl -s "$B/health" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  printf 'server did not become healthy on %s\n' "$B" >&2
  tail -40 "$LOG" >&2
  return 1
}

echo "building…"
cargo build -q 2>/dev/null || { echo "build failed"; exit 1; }

echo "booting server (port $PORT, admin+room tokens, rate 3/60s)…"
ADMIN_TOKEN="$ADMIN" ROOM_TOKEN="$ROOM" DB_PATH="$DB" \
  RATE_LIMIT=3 RATE_WINDOW_SECS=60 PORT="$PORT" RUST_LOG=warn \
  ./target/debug/room-server >"$LOG" 2>&1 &
SRV_PID=$!
wait_server || exit 1

AH="x-admin-token: $ADMIN"
RH="x-room-token: $ROOM"
M='{"sender":"backend","sender_type":"agent","text":"hi"}'

echo "── health / auth ──"
assert "web shell is served"              "200"  "$(code GET / -)"
assert "health advertises needs_token" "true" "$(curl -s "$B/health" | jq -r '.needs_token')"
assert "health advertises build version" "true" "$(curl -s "$B/health" | jq -r '.version | test("^[0-9]+\\.[0-9]+\\.[0-9]+")')"
assert "epoch present"                 "true" "$(curl -s "$B/health" | jq 'has("epoch")')"
assert "post without token -> 401"     "401"  "$(code POST /rooms/general/messages "$M")"
assert "post with room token -> 201"   "201"  "$(code POST /rooms/general/messages "$M" -H "$RH")"
assert "post with admin token -> 201"  "201"  "$(code POST /rooms/general/messages "$M" -H "$AH")"
# WS upgrade without a token must be rejected before the handshake (401).
WSUP=(-H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZQ==') # gitleaks:allow — RFC 6455 public example
assert "ws needs token (401)"          "401"  "$(curl -s -o /dev/null -w '%{http_code}' "${WSUP[@]}" "$B/ws?room=general&name=x")"
assert "ws with token header (101)"    "101"  "$(curl -s -o /dev/null -w '%{http_code}' -m 1 "${WSUP[@]}" -H "Sec-WebSocket-Protocol: loca.v1, loca.room.$ROOM" "$B/ws?room=general&name=x")"

echo "── membership / davet / session / lobby ──"
MEMBER_JSON=$(curl -sS -X POST "$B/members" -H "$AH" -H 'content-type: application/json' \
  -d '{"name":"smoke-agent","kind":"agent"}')
MEMBERSHIP=$(jq -r '.token // empty' <<<"$MEMBER_JSON")
assert "admit member -> token" "true" "$([[ "$MEMBERSHIP" == mb_* ]] && echo true || echo false)"
assert "list members -> 200" "200" "$(code GET /members - -H "$AH")"
MEMBERSHIP_ID=$(curl -sS -H "$AH" "$B/members" | jq -r '.[] | select(.name=="smoke-agent") | .id')
assert "member list redacts token" "true" \
  "$(curl -sS -H "$AH" "$B/members" | jq '[.[] | has("token")] | all(. == false)')"
assert "membership management id listed" "true" "$([[ "$MEMBERSHIP_ID" == mbid_* ]] && echo true || echo false)"
assert "residents include member" "true" "$(curl -sS -H "$AH" "$B/residents" | jq '[.[].name] | index("smoke-agent") != null')"
CARE_MEMBER_JSON=$(curl -sS -X POST "$B/members" -H "$AH" -H 'content-type: application/json' \
  -d '{"name":"loca-care","kind":"agent"}')
CARE_MEMBERSHIP=$(jq -r '.token // empty' <<<"$CARE_MEMBER_JSON")
assert "admit caretaker -> token" "true" "$([[ "$CARE_MEMBERSHIP" == mb_* ]] && echo true || echo false)"
assert "ordinary member cannot audit Building -> 403" "403" \
  "$(code GET /care/residents - -H "x-room-token: $MEMBERSHIP")"
assert "caretaker audits Building -> 200" "200" \
  "$(code GET /care/residents - -H "x-room-token: $CARE_MEMBERSHIP")"
assert "caretaker audit contains no credential" "true" \
  "$(curl -sS -H "x-room-token: $CARE_MEMBERSHIP" "$B/care/residents" | jq '[.[] | has("token")] | all(. == false)')"
chmod 600 "$CARE_ENV"
printf 'ROOM_SERVER_URL=%s\nLOCA_NAME=loca-care\nLOCA_MEMBERSHIP=%s\n' \
  "$B" "$CARE_MEMBERSHIP" >"$CARE_ENV"
CARE_AUDIT=$(LOCA_ENV="$CARE_ENV" python3 skill/loca-care/scripts/audit.py --format json)
assert "loca-care skill audits exact Building" "true" \
  "$(jq '(.total == 2) and (.away == 2) and ([.residents[].name] | index("loca-care") != null)' <<<"$CARE_AUDIT")"

DAVET_JSON=$(curl -sS -X POST "$B/rooms/general/invites" -H "$AH" -H 'content-type: application/json' \
  -d '{"name":"smoke-agent"}')
DAVET=$(jq -r '.token // empty' <<<"$DAVET_JSON")
assert "invite member -> token" "true" "$([[ "$DAVET" == dv_* ]] && echo true || echo false)"
assert "list invites -> 200"  "200" "$(code GET /rooms/general/invites - -H "$AH")"
assert "invite list redacts token" "true" \
  "$(curl -sS -H "$AH" "$B/rooms/general/invites" | jq '[.[] | has("token")] | all(. == false)')"
assert "whoami resolves davet" "davet" "$(curl -sS -H "x-room-token: $DAVET" "$B/whoami" | jq -r '.kind')"

CLAIM=$(curl -sS -X POST -H "x-room-token: $DAVET" "$B/membership/claim")
assert "davet claims permanent membership" "$MEMBERSHIP" "$(jq -r '.membership_token' <<<"$CLAIM")"
assert "membership cannot read private loca -> 401" "401" \
  "$(code GET /rooms/general/messages - -H "x-room-token: $MEMBERSHIP")"
assert "lobby WS accepts membership (101)" "101" \
  "$(curl -s -o /dev/null -w '%{http_code}' -m 1 "${WSUP[@]}" -H "Sec-WebSocket-Protocol: loca.v1, loca.membership.$MEMBERSHIP" "$B/lobby/ws")"

SESSION_JSON=$(curl -sS -X POST "$B/sessions" \
  -H 'content-type: application/json' -H "x-room-token: $DAVET" \
  -d '{"name":"wrong-name","kind":"agent"}')
SESSION=$(jq -r '.session_token' <<<"$SESSION_JSON")
assert "davet session minted" "true" "$([[ "$SESSION" == st_* ]] && echo true || echo false)"
assert "session identity comes from davet" "smoke-agent" "$(jq -r '.name' <<<"$SESSION_JSON")"
assert "session opens REST door" "200" \
  "$(code GET /rooms/general/messages - -H "x-session-token: $SESSION")"
assert "session opens WS door (101)" "101" \
  "$(curl -s -o /dev/null -w '%{http_code}' -m 1 "${WSUP[@]}" -H "Sec-WebSocket-Protocol: loca.v1, loca.session.$SESSION" "$B/ws?room=general&name=wrong&type=agent")"
assert "logout revokes session -> 204" "204" \
  "$(code DELETE /sessions - -H "x-session-token: $SESSION")"
assert "revoked session refused -> 401" "401" \
  "$(code GET /rooms/general/messages - -H "x-session-token: $SESSION")"

assert "self-release -> 204" "204" \
  "$(code POST /rooms/general/release - -H "x-room-token: $DAVET")"
assert "released davet refused -> 401" "401" \
  "$(code GET /rooms/general/messages - -H "x-room-token: $DAVET")"
CALL_JSON=$(curl -sS -X POST "$B/rooms/general/call" -H "$AH" \
  -H 'content-type: application/json' -d '{"name":"smoke-agent"}')
RECALLED_DAVET=$(jq -r '.token' <<<"$CALL_JSON")
assert "one-click recall mints fresh davet" "true" \
  "$([[ "$RECALLED_DAVET" == dv_* && "$RECALLED_DAVET" != "$DAVET" ]] && echo true || echo false)"
assert "recalled davet opens loca" "200" \
  "$(code GET /rooms/general/messages - -H "x-room-token: $RECALLED_DAVET")"
assert "room members endpoint -> 200" "200" "$(code GET /rooms/general/members - -H "$AH")"

echo "── master pairing / delegated authority ──"
PAIRING=$(curl -sS -X POST "$B/pairings?ttl_hours=1" -H "$AH" | jq -r '.pairing_code')
assert "one-use pairing minted" "true" "$([[ "$PAIRING" == pair_* ]] && echo true || echo false)"
ADMIN_SESSION_JSON=$(curl -sS -X POST "$B/sessions" \
  -H 'content-type: application/json' -H "x-pairing-code: $PAIRING" \
  -d '{"name":"smoke-master","kind":"user"}')
ADMIN_SESSION=$(jq -r '.session_token' <<<"$ADMIN_SESSION_JSON")
assert "pairing creates admin session" "true" "$(jq -r '.admin' <<<"$ADMIN_SESSION_JSON")"
assert "pairing is one-use -> 401" "401" \
  "$(code POST /sessions '{"name":"intruder","kind":"user"}' -H "x-pairing-code: $PAIRING")"

SMASTER_JSON=$(curl -sS -X POST "$B/smasters" -H "$AH" \
  -H 'content-type: application/json' -d '{"name":"smoke-smaster"}')
SMASTER=$(jq -r '.token' <<<"$SMASTER_JSON")
assert "create smaster -> token" "true" "$([[ "$SMASTER" == sm_* ]] && echo true || echo false)"
assert "list smasters -> 200" "200" "$(code GET /smasters - -H "$AH")"
SMASTER_ID=$(curl -sS -H "$AH" "$B/smasters" | jq -r '.[] | select(.name=="smoke-smaster") | .id')
assert "smaster list redacts token" "true" \
  "$(curl -sS -H "$AH" "$B/smasters" | jq '[.[] | has("token")] | all(. == false)')"
assert "revoke smaster by id -> 200" "200" "$(code DELETE "/smasters/$SMASTER_ID" - -H "$AH")"

echo "── notes CRUD + auth ──"
assert "note create no token -> 401" "401" "$(code POST /rooms/general/notes '{"key":"api","by":"backend"}')"
assert "note create -> 201"          "201" "$(code POST /rooms/general/notes '{"key":"api","title":"API","body":"v1","by":"backend"}' -H "$RH")"
assert "note dup -> 409"             "409" "$(code POST /rooms/general/notes '{"key":"api","by":"x"}' -H "$RH")"
assert "note update -> 200"          "200" "$(code PUT /rooms/general/notes/api '{"body":"v2","by":"backend"}' -H "$RH")"
assert "note update missing -> 404"  "404" "$(code PUT /rooms/general/notes/none '{"by":"x"}' -H "$RH")"
assert "notes list -> 200"            "200" "$(code GET /rooms/general/notes - -H "$RH")"
assert "note read -> v2"              "v2"  "$(curl -sS -H "$RH" "$B/rooms/general/notes/api" | jq -r '.body')"
assert "note history retained"        "true" "$(curl -sS -H "$RH" "$B/rooms/general/notes/api/history" | jq 'length > 0')"
assert "memory search finds note"     "true" "$(curl -sS -H "$RH" "$B/rooms/general/search?q=v2" | jq '.notes | length > 0')"
assert "note delete -> 204"          "204" "$(code DELETE /rooms/general/notes/api - -H "$RH")"

echo "── chat modes (admin, hard enforce) ──"
assert "get mode -> 200"                 "200" "$(code GET /rooms/general/mode - -H "$RH")"
assert "set mode no admin -> 401"       "401" "$(code PUT /rooms/general/mode '{"mode":{"mode":"paused"}}' -H "$RH")"
assert "set paused -> 200"              "200" "$(code PUT /rooms/general/mode '{"mode":{"mode":"paused"}}' -H "$AH")"
assert "post while paused -> 403"       "403" "$(code POST /rooms/general/messages "$M" -H "$RH")"
assert "admin posts while paused ->201" "201" "$(code POST /rooms/general/messages '{"sender":"admin","sender_type":"user","text":"a"}' -H "$AH")"
assert "back to free -> 200"            "200" "$(code PUT /rooms/general/mode '{"mode":{"mode":"free"}}' -H "$AH")"
assert "name lead -> 200"               "200" "$(code POST /rooms/general/lead '{"lead":"smoke-agent"}' -H "$AH")"

echo "── moderation / declared work ──"
assert "mute member -> 200"   "200" "$(code POST /rooms/general/moderate '{"action":"mute","name":"smoke-agent"}' -H "$AH")"
assert "moderation state -> muted" "true" \
  "$(curl -sS -H "$AH" "$B/rooms/general/moderate" | jq '.muted | index("smoke-agent") != null')"
assert "unmute member -> 200" "200" "$(code POST /rooms/general/moderate '{"action":"unmute","name":"smoke-agent"}' -H "$AH")"

assert "journal append -> 201" "201" \
  "$(code POST /rooms/general/journal '{"text":"smoke deploy complete","by":"smoke-agent","by_type":"agent"}' -H "$RH")"
assert "journal list contains entry" "true" \
  "$(curl -sS -H "$RH" "$B/rooms/general/journal" | jq '[.[].text] | index("smoke deploy complete") != null')"

TASK_JSON=$(curl -sS -X POST "$B/rooms/general/tasks" -H "$AH" \
  -H 'content-type: application/json' \
  -d '{"title":"smoke implementation","by":"operator","assigned_to":"smoke-agent"}')
TASK_ID=$(jq -r '.id' <<<"$TASK_JSON")
assert "task declared -> id" "true" "$([[ "$TASK_ID" =~ ^[0-9]+$ ]] && echo true || echo false)"
assert "task list contains task" "true" \
  "$(curl -sS -H "$AH" "$B/rooms/general/tasks" | jq --argjson id "$TASK_ID" '[.[].id] | index($id) != null')"

TASK2_JSON=$(curl -sS -X POST "$B/rooms/general/tasks" -H "$AH" \
  -H 'content-type: application/json' \
  -d '{"title":"smoke review","by":"operator","assigned_to":"reviewer"}')
TASK2_ID=$(jq -r '.id' <<<"$TASK2_JSON")
GOAL_JSON=$(curl -sS -X POST "$B/rooms/general/goals" -H "$AH" \
  -H 'content-type: application/json' \
  -d "{\"outcome\":\"public release ready\",\"completion\":\"all_tasks\",\"task_ids\":[$TASK_ID,$TASK2_ID],\"by\":\"operator\"}")
GOAL_ID=$(jq -r '.id' <<<"$GOAL_JSON")
assert "goal declared -> active" "active" "$(jq -r '.status' <<<"$GOAL_JSON")"
assert "second active goal refused -> 409" "409" \
  "$(code POST /rooms/general/goals '{"outcome":"competing goal","by":"operator"}' -H "$AH")"
assert "first linked task completed -> 200" "200" \
  "$(code PATCH "/rooms/general/tasks/$TASK_ID" '{"status":"done","by":"operator"}' -H "$AH")"
assert "goal remains active with open task" "active" \
  "$(curl -sS -H "$AH" "$B/rooms/general/goals" | jq -r --argjson id "$GOAL_ID" '.[] | select(.id==$id) | .status')"
assert "second linked task completed -> 200" "200" \
  "$(code PATCH "/rooms/general/tasks/$TASK2_ID" '{"status":"done","by":"operator"}' -H "$AH")"
assert "all linked tasks achieve goal" "achieved" \
  "$(curl -sS -H "$AH" "$B/rooms/general/goals" | jq -r --argjson id "$GOAL_ID" '.[] | select(.id==$id) | .status')"

assert "explicit wait declared -> 201" "201" \
  "$(code POST /rooms/general/waits '{"by":"smoke-agent","waiting_for":"reviewer","reason":"review needed"}' -H "$AH")"
assert "wait list contains edge" "reviewer" \
  "$(curl -sS -H "$AH" "$B/rooms/general/waits" | jq -r '.[] | select(.waiter=="smoke-agent") | .waiting_for')"
assert "operator clears wait -> 204" "204" \
  "$(code DELETE /rooms/general/waits/smoke-agent '{"by":"operator"}' -H "$AH")"
assert "wait edge removed" "0" \
  "$(curl -sS -H "$AH" "$B/rooms/general/waits" | jq '[.[] | select(.waiter=="smoke-agent")] | length')"

echo "── reply threading ──"
RID=$(curl -s -X POST "$B/rooms/general/messages" -H "$RH" -H 'content-type: application/json' -d '{"sender":"a","sender_type":"agent","text":"root"}' | jq '.id')
curl -s -o /dev/null -X POST "$B/rooms/general/messages" -H "$RH" -H 'content-type: application/json' -d "{\"sender\":\"b\",\"sender_type\":\"agent\",\"text\":\"re\",\"reply_to\":$RID}"
assert "reply carries reply_to" "$RID" "$(curl -s -H "$RH" "$B/rooms/general/messages" | jq '.[] | select(.text=="re") | .reply_to')"

echo "── exactly-once delivery ──"
IDEM='{"sender":"retry-agent","sender_type":"agent","text":"exactly once","op_id":"smoke-op-1"}'
ID1=$(curl -sS -X POST "$B/rooms/general/messages" -H "$RH" -H 'content-type: application/json' -d "$IDEM" | jq '.id')
ID2=$(curl -sS -X POST "$B/rooms/general/messages" -H "$RH" -H 'content-type: application/json' -d "$IDEM" | jq '.id')
assert "same op_id returns same message" "$ID1" "$ID2"
assert "same op_id stored once" "1" \
  "$(curl -sS -H "$RH" "$B/rooms/general/messages" | jq '[.[] | select(.sender=="retry-agent" and .text=="exactly once")] | length')"

echo "── rate limit (3/60s) + tunable ──"
S='{"sender":"spammer","sender_type":"agent","text":"x"}'
code POST /rooms/general/messages "$S" -H "$RH" >/dev/null
code POST /rooms/general/messages "$S" -H "$RH" >/dev/null
code POST /rooms/general/messages "$S" -H "$RH" >/dev/null
assert "4th post rate-limited -> 429" "429" "$(code POST /rooms/general/messages "$S" -H "$RH")"
assert "settings no admin -> 401"     "401" "$(code PUT /rooms/general/settings '{"rate_limit":100}' -H "$RH")"
assert "admin saves delivery settings -> 200" "200" \
  "$(code PUT /rooms/general/settings \
    '{"rate_limit":100,"turn_max_messages":4,"turn_idle_ms":5000,"turn_max_wait_ms":15000,"care_wait_secs":120,"care_cooldown_secs":300,"care_max_attempts":2,"care_context_messages":8}' \
    -H "$AH")"
assert "get settings -> raised limit" "100" "$(curl -sS -H "$RH" "$B/rooms/general/settings" | jq '.rate_limit')"
assert "get settings -> 4-message packet" "4:5000:15000" \
  "$(curl -sS -H "$RH" "$B/rooms/general/settings" | jq -r '[.turn_max_messages,.turn_idle_ms,.turn_max_wait_ms] | join(":")')"
assert "get settings -> bounded care" "120:300:2:8" \
  "$(curl -sS -H "$RH" "$B/rooms/general/settings" | jq -r '[.care_wait_secs,.care_cooldown_secs,.care_max_attempts,.care_context_messages] | join(":")')"
assert "spammer posts again -> 201"   "201" "$(code POST /rooms/general/messages "$S" -H "$RH")"

echo "── new room creation ──"
assert "post to new room -> 201" "201"  "$(code POST /rooms/project-x/messages '{"sender":"web","sender_type":"agent","text":"hello"}' -H "$RH")"
assert "new room listed"         "true" "$(curl -s -H "$RH" "$B/rooms" | jq '[.[].room] | index("project-x") != null')"
assert "seed disposable room -> 201" "201" \
  "$(code POST /rooms/disposable/messages '{"sender":"web","sender_type":"agent","text":"bye"}' -H "$AH")"
assert "archive disposable room -> 200" "200" \
  "$(code PUT /rooms/disposable/settings '{"archived":true}' -H "$AH")"
assert "seal archived room -> 204" "204" "$(code DELETE /rooms/disposable - -H "$AH")"

echo "── revocation cleanup ──"
assert "revoke recalled davet -> 200" "200" \
  "$(code DELETE "/rooms/general/invites/$RECALLED_DAVET" - -H "$AH")"
assert "recalled davet is closed -> 401" "401" \
  "$(code GET /rooms/general/messages - -H "x-room-token: $RECALLED_DAVET")"
assert "revoke member by id -> 200" "200" "$(code DELETE "/members/$MEMBERSHIP_ID" - -H "$AH")"
assert "logout admin session -> 204" "204" \
  "$(code DELETE /sessions - -H "x-session-token: $ADMIN_SESSION")"

echo "── persistence across restart ──"
EPOCH1=$(curl -s "$B/health" | jq '.epoch')
kill "$SRV_PID"
wait "$SRV_PID" 2>/dev/null
ADMIN_TOKEN="$ADMIN" ROOM_TOKEN="$ROOM" DB_PATH="$DB" RATE_LIMIT=3 RATE_WINDOW_SECS=60 PORT="$PORT" RUST_LOG=warn \
  ./target/debug/room-server >"$LOG" 2>&1 &
SRV_PID=$!
wait_server || exit 1
EPOCH2=$(curl -s "$B/health" | jq '.epoch')
assert "epoch changed after restart" "true" "$([ "$EPOCH2" != "$EPOCH1" ] && echo true || echo false)"
assert "messages survived restart"   "true" "$(curl -s -H "$RH" "$B/rooms/general/messages" | jq 'length > 0')"
assert "project-x survived restart"  "true" "$(curl -s -H "$RH" "$B/rooms" | jq '[.[].room] | index("project-x") != null')"
assert "achieved goal survived restart" "achieved" \
  "$(curl -sS -H "$AH" "$B/rooms/general/goals" | jq -r --argjson id "$GOAL_ID" '.[] | select(.id==$id) | .status')"

echo
if [ "$FAILED" -eq 0 ]; then
  printf '\033[32mALL SMOKE CHECKS PASSED\033[0m\n'
else
  printf '\033[31m%d SMOKE CHECK(S) FAILED\033[0m\n' "$FAILED"
  exit 1
fi
