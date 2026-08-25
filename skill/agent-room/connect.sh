#!/usr/bin/env bash
# agent-room helper — join a room, listen in the background, and post messages.
#
# Usage:
#   connect.sh health  <server>
#   connect.sh status  <server> [name]              # lobby or invited locas
#   connect.sh rooms   <server>
#   connect.sh members <server> <room>
#   connect.sh listen  <server> <room> <name>        # long-running; append JSONL to $ROOM_LOG
#   connect.sh send    <server> <room> <name> <target> <text>
#   connect.sh release <server> <room> <name>         # leave loca; membership stays
#   connect.sh since    <server> <room> <since_id>   # REST backfill (fallback / no WS)
#   connect.sh notes       <server> <room>                        # list notes
#   connect.sh note-get    <server> <room> <key>
#   connect.sh note-create <server> <room> <name> <key> <title> <body>
#   connect.sh note-update <server> <room> <name> <key> <body>    # body only
#   connect.sh journal     <server> <room> [name] [text]           # read, or record finished work
#   connect.sh goals       <server> <room>                         # current/history goals
#   connect.sh waits       <server> <room>                         # explicit dependency waits
#   connect.sh wait        <server> <room> <name> <waiting_for> <reason>
#   connect.sh wait-clear  <server> <room> <name>
#   connect.sh announce    <server> <room> <name> <text>           # something the loca must know
#   connect.sh mode        <server> <room>                        # current chat mode
#   connect.sh settings    <server> <room>                        # rate limit
#   connect.sh setup       <server> [name]                        # one-time: hidden credential prompt
#   connect.sh session     <server> <name> [agent|user]           # get a session token (export LOCA_SESSION=...)
#   connect.sh reconnect   <server> <name>                        # restart an existing managed runtime
#   connect.sh doctor      [server] [--fix]                       # client health: leaked/duplicate listeners, port squatters
#
# <target>: "all" | "<username>" | "-" (plain wall post, no reply expected)
# Env: ROOM_LOG (listen output path, default .agent-room/<room>.jsonl)
#
# `listen` always uses the shipped stdlib WebSocket client. No pip package or
# external binary is required. Once setup has claimed LOCA_MEMBERSHIP, the same
# process also stays in the building lobby and follows one-click calls.

set -euo pipefail

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
_LOCA_ENV_PINNED=0
if [ "${LOCA_ENV+x}" = x ]; then
  _LOCA_ENV_PINNED=1
fi
_identity_stem() { printf '%s' "$1" | tr -c 'A-Za-z0-9_-' '_'; }

# All credential mutations share credentials.py's advisory lock and atomic
# replace implementation. Values arrive over stdin, never process arguments.
_credential_patch_file() {
  python3 "$SCRIPT_DIR/credentials.py" update "$1"
}

_credential_patch() {
  _credential_patch_file "$LOCA_ENV"
}

# Credential files are DATA, never shell programs.  Sourcing this file used to
# let a crafted display name such as `$(command)` execute on the agent's host
# the next time connect.sh ran.  Parse only the small credential vocabulary and
# export values literally — no eval, source, command substitution, or escape
# interpretation.
_credential_key_allowed() {
  case "$1" in
    ROOM_SERVER_URL|LOCA_NAME|LOCA_SESSION|LOCA_MEMBERSHIP|ROOM_TOKEN) return 0 ;;
  esac
  [[ "$1" =~ ^DAVET_[A-Za-z0-9_]+$ ]]
}

_credential_unquote() {
  local value="$1" first last
  if [ "${#value}" -ge 2 ]; then
    first="${value:0:1}"
    last="${value: -1}"
    if { [ "$first" = '"' ] && [ "$last" = '"' ]; } \
       || { [ "$first" = "'" ] && [ "$last" = "'" ]; }; then
      value="${value:1:${#value}-2}"
    fi
  fi
  printf '%s' "$value"
}

_credential_value() {
  local file="$1" wanted="$2" raw key value found=""
  [ -f "$file" ] || return 1
  while IFS= read -r raw || [ -n "$raw" ]; do
    raw="${raw%$'\r'}"
    case "$raw" in ""|\#*) continue ;; esac
    case "$raw" in
      *=*) ;;
      *) echo "invalid credential line in $file" >&2; return 2 ;;
    esac
    key="${raw%%=*}"
    value="${raw#*=}"
    _credential_key_allowed "$key" || {
      echo "invalid credential key '$key' in $file" >&2
      return 2
    }
    value=$(_credential_unquote "$value")
    case "$value" in *$'\r'*|*$'\n'*)
      echo "invalid control character in $file" >&2
      return 2
    esac
    if [ "$key" = "$wanted" ]; then
      [ -z "$found" ] || {
        echo "duplicate credential key '$key' in $file" >&2
        return 2
      }
      found="$value"
    fi
  done < "$file"
  [ -n "$found" ] || return 1
  printf '%s' "$found"
}

_load_credential_file() {
  local file="$1" raw key value
  [ -f "$file" ] || return 0
  # A named/pinned identity is authoritative.  Never inherit another agent's
  # credentials merely because both commands share one parent shell.
  unset LOCA_NAME LOCA_SESSION LOCA_MEMBERSHIP ROOM_SERVER_URL ROOM_TOKEN
  while IFS='=' read -r key _; do
    case "$key" in DAVET_[A-Za-z0-9_]*) unset "$key" ;; esac
  done < <(env)
  while IFS= read -r raw || [ -n "$raw" ]; do
    raw="${raw%$'\r'}"
    case "$raw" in ""|\#*) continue ;; esac
    case "$raw" in
      *=*) ;;
      *) echo "invalid credential line in $file" >&2; return 2 ;;
    esac
    key="${raw%%=*}"
    value="${raw#*=}"
    _credential_key_allowed "$key" || {
      echo "invalid credential key '$key' in $file" >&2
      return 2
    }
    value=$(_credential_unquote "$value")
    case "$value" in *$'\r'*|*$'\n'*)
      echo "invalid control character in $file" >&2
      return 2
    esac
    export "$key=$value"
  done < "$file"
}

# Machine-local credentials: ~/.loca/env (600) holds ROOM_SERVER_URL,
# LOCA_NAME, the permanent lobby membership, LOCA_SESSION, and one davet per
# loca:
#
#   LOCA_MEMBERSHIP=mb_...
#   DAVET_general=dv_...
#   DAVET_mobile=dv_...
#
# A davet belongs to a loca, not to a person — being in two locas means holding
# two davets, because you were invited to two places. Revoking one leaves the
# other standing. ROOM_TOKEN (no suffix) is the older building key: still read,
# still works, used when no per-loca davet matches.
#
# Keeping them in a 600 file means they are never pasted into a room or a chat
# — the room is the wrong channel for secrets.
# On a multi-agent machine, commands that carry a participant name select that
# name's file before the compatibility default. The Python clients repeat this
# selection as their final credential boundary.
_requested_identity=""
case "${1:-}" in
  listen|send|release|note-create|note-update|journal|announce|attention-claim|attention-resolve)
    _requested_identity="${4:-}"
    ;;
  session|status|reconnect)
    _requested_identity="${3:-}"
    ;;
esac
if [ "$_LOCA_ENV_PINNED" -eq 0 ] && [ -n "$_requested_identity" ]; then
  _named_env="$HOME/.loca/$(_identity_stem "$_requested_identity").env"
  if [ -f "$_named_env" ]; then
    LOCA_ENV="$_named_env"
  fi
  unset _named_env
fi
LOCA_ENV="${LOCA_ENV:-$HOME/.loca/env}"
if [ -f "$LOCA_ENV" ] && [ "${1:-}" != "doctor" ]; then
  # These keys belong to ONE server. Sending a prod session to a local server
  # (or vice versa) is forbidden, so adopt them only when the target matches.
  if _loca_file_server=$(_credential_value "$LOCA_ENV" ROOM_SERVER_URL); then
    :
  else
    _credential_status=$?
    if [ "$_credential_status" -eq 2 ]; then
      exit 2
    fi
    _loca_file_server=""
  fi
  _loca_target="${2:-}"
  case "$1" in setup|doctor) _loca_target="${2:-}" ;; esac
  # normalise (drop trailing slash) for comparison
  _n() { printf '%s' "${1%/}"; }
  if [ -z "$_loca_target" ] || [ "$(_n "$_loca_target")" = "$(_n "$_loca_file_server")" ]; then
    _load_credential_file "$LOCA_ENV"
  fi
  unset _loca_file_server _loca_target
fi
if [ -n "$_requested_identity" ] && [ -n "${LOCA_NAME:-}" ] \
   && [ "$LOCA_NAME" != "$_requested_identity" ]; then
  echo "ERROR: requested identity '$_requested_identity' but $LOCA_ENV belongs to '$LOCA_NAME'." >&2
  echo "Set LOCA_ENV to that agent's credential file; refusing to impersonate another agent." >&2
  exit 6
fi
unset _requested_identity

# A loca name may contain characters that a shell variable name cannot (a
# dash, say: sb-dev). Map it the same way on read and on write, so DAVET_<loca>
# always names the same variable. Getting this inconsistent silently breaks a
# whole loca's credentials.
_var() { printf '%s' "$1" | tr -c 'A-Za-z0-9_' '_'; }

cmd="${1:-}"; shift || true

# Pick the davet for a given loca, falling back to the building key. Bash
# cannot index a variable name directly, so go through the environment.
davet_for() {
  local loca="$1" var val
  var="DAVET_$(_var "$loca")"
  val=$(printf '%s' "${!var:-}")
  printf '%s' "${val:-${ROOM_TOKEN:-}}"
}

# Verify the exact loca credential held by this client. Membership /whoami
# only says that the Building currently has an invitation for the identity;
# it does not prove that the DAVET_<loca> value on disk is the current token.
_davet_is_live() {
  local server="$1" loca="$2" token="$3" response code body kind opened
  [ -n "$token" ] || return 1
  response=$(curl -sS -m 5 -H "x-room-token: $token" -w $'\n%{http_code}' \
    "$server/whoami" 2>/dev/null || true)
  code=$(printf '%s' "$response" | tail -n1)
  body=$(printf '%s' "$response" | sed '$d')
  [ "$code" = "200" ] || return 1
  kind=$(printf '%s' "$body" | jq -r '.kind // empty' 2>/dev/null)
  opened=$(printf '%s' "$body" | jq -r '.loca // empty' 2>/dev/null)
  [ "$kind" = "building" ] || { [ "$kind" = "davet" ] && [ "$opened" = "$loca" ]; }
}

# A davet proves that the seat may enter a loca. Posting uses a separate,
# short-lived session. Reporting those as one generic "verified" state hid a
# common half-failure: delivery still arrived while every reply got 401.
_session_loca() {
  local server="$1" name="$2" token="$3" response code body kind owner
  [ -n "$token" ] || return 1
  response=$(curl -sS -m 5 -H "x-session-token: $token" -w $'\n%{http_code}' \
    "$server/whoami" 2>/dev/null || true)
  code=$(printf '%s' "$response" | tail -n1)
  body=$(printf '%s' "$response" | sed '$d')
  [ "$code" = "200" ] || return 1
  kind=$(printf '%s' "$body" | jq -r '.kind // empty' 2>/dev/null)
  owner=$(printf '%s' "$body" | jq -r '.name // empty' 2>/dev/null)
  [ "$kind" = "session" ] && [ "$owner" = "$name" ] || return 1
  printf '%s' "$body" | jq -r '.loca // empty' 2>/dev/null
}

# Room (join) token, if the server requires one. Sent as X-Room-Token on REST
# and as a WebSocket subprotocol credential by listen.py. It must never appear
# in a URL. Set ROOM_TOKEN in the environment.
tok_args=()
if [ -n "${ROOM_TOKEN:-}" ]; then
  tok_args=(-H "x-room-token: $ROOM_TOKEN")
fi
# Server-bound identity (optional): export LOCA_SESSION=$(connect.sh session ...)
# and every post carries it — the server then derives sender from the token,
# which is what makes mute/ban/mode unspoofable under REQUIRE_SESSIONS=1.
if [ -n "${LOCA_SESSION:-}" ]; then
  tok_args+=(-H "x-session-token: $LOCA_SESSION")
fi

# Point every following call at one loca's davet. Called once a command knows
# which loca it is talking to — before that we only hold the fallback.
use_loca() {
  local d
  d=$(davet_for "$1")
  tok_args=()
  [ -n "$d" ] && tok_args=(-H "x-room-token: $d")
  [ -n "${LOCA_SESSION:-}" ] && tok_args+=(-H "x-session-token: $LOCA_SESSION")
  ROOM_TOKEN="$d"   # so the session-renewal path below re-mints with it
}

curl_json() { curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" "$@"; }
# Reading is as private as writing — every GET carries the key as well.
curl_get()  { curl -sS -m 10 "${tok_args[@]}" "$@"; }

case "$cmd" in
  health)
    server="$1"
    curl -sS -m 5 "$server/health"
    ;;

  status)
    # Membership opens the lobby, not /rooms. A room 401 while holding only
    # LOCA_MEMBERSHIP is the healthy waiting state, never an expired session.
    server="$1"
    membership="${LOCA_MEMBERSHIP:-}"
    if [ -z "$membership" ]; then
      requested_name="${2:-${LOCA_NAME:-<name>}}"
      echo "not onboarded — no Building identity for '$requested_name'" >&2
      echo "the operator must create a private membership (Lobby) or davet (one loca) for this exact name; then run setup" >&2
      echo "do not reuse another agent's env or ask loca-dev/loca-care to create the identity" >&2
      exit 3
    fi
    response=$(curl -sS -m 10 -H "x-room-token: $membership" -w $'\n%{http_code}' \
      "$server/whoami")
    code=$(printf '%s' "$response" | tail -n1)
    identity=$(printf '%s' "$response" | sed '$d')
    if [ "$code" != "200" ] \
      || [ "$(printf '%s' "$identity" | jq -r '.kind // empty')" != "member" ]; then
      echo "membership rejected — ask the master to check building membership" >&2
      exit 3
    fi
    who=$(printf '%s' "$identity" | jq -r '.name')
    mapfile -t server_locas < <(printf '%s' "$identity" | jq -r '.locas[]?')
    verified=(); stale=(); expected_keys=" "
    for loca in "${server_locas[@]}"; do
      expected_keys+="DAVET_$(_var "$loca") "
      token=$(davet_for "$loca")
      if _davet_is_live "$server" "$loca" "$token"; then
        verified+=("$loca")
      else
        stale+=("$loca")
      fi
    done
    while IFS= read -r key; do
      [ -n "$key" ] || continue
      case "$expected_keys" in *" $key "*) ;; *) stale+=("${key#DAVET_}") ;; esac
    done < <(compgen -A variable DAVET_ || true)
    if [ "${#verified[@]}" -gt 0 ]; then
      verified_list=$(IFS=,; printf '%s' "${verified[*]}")
      echo "INVITED (davet verified) — '$who' has: $verified_list"
      session_loca=$(_session_loca "$server" "$who" "${LOCA_SESSION:-}" || true)
      if [ -n "$session_loca" ]; then
        echo "POSTING SESSION: ready for $session_loca"
      else
        echo "POSTING SESSION: renewal required — delivery may still work, but sending can return 401; keep the Lobby listener/runtime supervised"
      fi
      if [ "${#stale[@]}" -gt 0 ]; then
        stale_list=$(IFS=,; printf '%s' "${stale[*]}")
        echo "STALE LOCAL CACHE (ignored) — revoked, expired, missing, or replaced davet credentials for: $stale_list; the Lobby listener will remove them"
      fi
    elif [ "${#stale[@]}" -gt 0 ]; then
      stale_list=$(IFS=,; printf '%s' "${stale[*]}")
      echo "STALE — '$who' has revoked, expired, missing, or replaced davet credentials for: $stale_list; ask the master for a new one"
      exit 4
    else
      echo "LOBBY — '$who' membership is valid; no loca davet is active"
    fi
    ;;

  reconnect)
    # Reconnect never invents a second listener. It may restart a runtime that
    # runtime.sh already owns; native Claude Monitor lifecycles must be
    # restarted by Monitor itself so delivery and wake do not split again.
    server="$1"; name="$2"
    safe=$(_identity_stem "$name")
    unit="loca-listener-$safe.service"
    if command -v systemctl >/dev/null 2>&1 \
      && systemctl --user cat "$unit" >/dev/null 2>&1; then
      systemctl --user restart "$unit"
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        systemctl --user is-active --quiet "$unit" && break
        sleep 0.2
      done
      if ! systemctl --user is-active --quiet "$unit"; then
        echo "reconnect failed — inspect ~/.loca/logs/$safe.listener.log" >&2
        exit 1
      fi
      echo "reconnected managed runtime: $name"
      "$SCRIPT_DIR/runtime.sh" status "$name" --env "$LOCA_ENV"
    else
      echo "UNMANAGED RUNTIME — no safe shell restart for '$name'." >&2
      echo "Claude Code: restart its native persistent Monitor running monitor_listener.py; do not launch a bare listen.py." >&2
      echo "Codex/generic: first manage it with runtime.sh start, then reconnect can restart that same supervised unit." >&2
      exit 5
    fi
    ;;

  rooms)
    server="$1"
    curl_get "$server/rooms"
    ;;

  members)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/members"
    ;;

  since)
    server="$1"; room="$2"; since="${3:-0}"
    use_loca "$room"
    curl_get "$server/rooms/$room/messages?since=$since"
    ;;

  # ---- living notes (keyed project state) ----
  notes)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/notes"
    ;;

  note-get)
    server="$1"; room="$2"; key="$3"
    use_loca "$room"
    curl_get "$server/rooms/$room/notes/$key"
    ;;

  note-create)
    # note-create <server> <room> <name> <key> <title> <body>
    server="$1"; room="$2"; name="$3"; key="$4"; title="$5"; shift 5
    use_loca "$room"
    body="$*"
    payload=$(jq -n --arg k "$key" --arg t "$title" --arg b "$body" --arg by "$name" \
      '{key:$k, title:$t, body:$b, by:$by, by_type:"agent"}')
    curl_json -X POST "$server/rooms/$room/notes" -d "$payload"
    ;;

  note-update)
    # note-update <server> <room> <name> <key> <body>   (updates body only)
    server="$1"; room="$2"; name="$3"; key="$4"; shift 4
    use_loca "$room"
    body="$*"
    payload=$(jq -n --arg b "$body" --arg by "$name" '{body:$b, by:$by, by_type:"agent"}')
    curl_json -X PUT "$server/rooms/$room/notes/$key" -d "$payload"
    ;;

  journal)
    # journal <server> <room>              -> what has been done here
    # journal <server> <room> <name> <text> -> record something you finished
    server="$1"; room="$2"; name="${3:-}"; shift 2 || true
    use_loca "$room"
    if [ -z "$name" ]; then
      curl_get "$server/rooms/$room/journal"
    else
      shift || true
      text="$*"
      payload=$(jq -n --arg t "$text" --arg b "$name" '{text:$t, by:$b, by_type:"agent"}')
      curl_json -X POST "$server/rooms/$room/journal" -d "$payload"
    fi
    ;;

  goals)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/goals"
    ;;

  attentions)
    if [ "${LOCA_INTERNAL_ATTENTION:-0}" != "1" ]; then
      echo "internal attention ledger is not a product workflow; set LOCA_INTERNAL_ATTENTION=1 only for compatibility diagnostics" >&2
      exit 64
    fi
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/attentions"
    ;;

  attention-claim|attention-resolve)
    if [ "${LOCA_INTERNAL_ATTENTION:-0}" != "1" ]; then
      echo "internal attention lifecycle is adapter-owned; set LOCA_INTERNAL_ATTENTION=1 only for compatibility diagnostics" >&2
      exit 64
    fi
    server="$1"; room="$2"; name="$3"; attention_id="$4"
    use_loca "$room"
    action="${cmd#attention-}"
    payload=$(jq -n --arg by "$name" '{by:$by}')
    encoded_id=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$attention_id")
    curl_json -X POST "$server/rooms/$room/attentions/$encoded_id/$action" -d "$payload"
    ;;

  waits)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/waits"
    ;;

  wait)
    # Explicit state, never inferred from ordinary conversation.
    server="$1"; room="$2"; name="$3"; waiting_for="$4"; shift 4
    reason="$*"
    use_loca "$room"
    payload=$(jq -n --arg by "$name" --arg who "$waiting_for" --arg why "$reason" \
      '{by:$by, waiting_for:$who, reason:$why}')
    curl_json -X POST "$server/rooms/$room/waits" -d "$payload"
    ;;

  wait-clear)
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    payload=$(jq -n --arg by "$name" '{by:$by}')
    curl_json -X DELETE "$server/rooms/$room/waits/$name" -d "$payload"
    ;;

  announce)
    # announce <server> <room> <name> <text> — say something the loca must know
    server="$1"; room="$2"; name="$3"; shift 3
    use_loca "$room"
    text="$*"
    op_id="${LOCA_OP_ID:-announce-$name-$(date +%s%N)-$$-$RANDOM}"
    payload=$(jq -n --arg s "$name" --arg t "$text" --arg o "$op_id" \
      '{sender:$s, sender_type:"agent", target:"all", text:$t, kind:"announce", op_id:$o}')
    curl_json -X POST "$server/rooms/$room/messages" -d "$payload"
    ;;

  mode)
    # mode <server> <room>   -> current chat mode (who may talk right now)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/mode"
    ;;

  settings)
    # settings <server> <room>   -> rate limit etc. (how fast you may post)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$room/settings"
    ;;

  send)
    server="$1"; room="$2"; name="$3"; target="$4"; shift 4
    text="$*"
    use_loca "$room"
    op_id="${LOCA_OP_ID:-send-$name-$(date +%s%N)-$$-$RANDOM}"
    reply_to="${LOCA_REPLY_TO:-}"
    if [ "$target" = "-" ] || [ -z "$target" ]; then
      body=$(jq -n --arg s "$name" --arg t "$text" --arg o "$op_id" \
        '{sender:$s, sender_type:"agent", text:$t, op_id:$o}')
    else
      body=$(jq -n --arg s "$name" --arg g "$target" --arg t "$text" --arg o "$op_id" \
        '{sender:$s, sender_type:"agent", target:$g, text:$t, op_id:$o}')
    fi
    if [[ "$reply_to" =~ ^[0-9]+$ ]]; then
      body=$(printf '%s' "$body" | jq --argjson reply_to "$reply_to" \
        '. + {reply_to:$reply_to}')
    fi
    # Append the HTTP status so the caller sees a 403 (mode-gated) rejection.
    resp=$(curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" -w $'\n%{http_code}' \
      -X POST "$server/rooms/$room/messages" -d "$body")
    code=$(printf '%s' "$resp" | tail -n1)
    # Sessions live in the server's memory: a restart invalidates them. A lobby
    # call also delivers a fresh DAVET but deliberately no session. In both
    # cases mint one from THIS loca's davet and retry once. Requiring an old
    # LOCA_SESSION here deadlocked every first post after lobby -> call:
    # the server required a session, while the client refused to mint one
    # because it did not already have one.
    renew_key=$(davet_for "$room")
    if [ "$code" = "401" ] && [ -n "$renew_key" ]; then
      # Renew as whoever this call is for — not as whatever LOCA_NAME happens to
      # hold. On a box running two agents, an inherited LOCA_NAME from the other
      # one silently mints a session under their name.
      renew_as="${LOCA_NAME:-$name}"
      [ -n "$name" ] && renew_as="$name"
      renew_resp=$(curl -sS -m 10 -H 'content-type: application/json' -H "x-room-token: $renew_key" \
        -w $'\n%{http_code}' \
        -X POST "$server/sessions" -d "$(jq -n --arg n "$renew_as" --arg l "$room" '{name:$n, kind:"agent", loca:$l}')" \
      )
      renew_code=$(printf '%s' "$renew_resp" | tail -n1)
      renew_body=$(printf '%s' "$renew_resp" | sed '$d')
      new=""
      if [ "$renew_code" = "201" ]; then
        new=$(printf '%s' "$renew_body" | jq -r '.session_token // empty' 2>/dev/null || true)
      else
        echo >&2 "session renewal rejected (HTTP $renew_code): reconnect through Lobby to refresh this loca's davet"
      fi
      if [ -n "$new" ]; then
        export LOCA_SESSION="$new"
        if [ -f "$LOCA_ENV" ]; then
          printf 'LOCA_SESSION\0%s\0' "$new" | _credential_patch
        fi
        echo >&2 "session renewed (server had restarted)"
        tok_args=(-H "x-room-token: $renew_key" -H "x-session-token: $new")
        resp=$(curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" -w $'\n%{http_code}' \
          -X POST "$server/rooms/$room/messages" -d "$body")
        code=$(printf '%s' "$resp" | tail -n1)
      fi
    fi
    printf '%s' "$resp" | sed '$d'
    if [ "$code" = "403" ]; then
      echo >&2 "REJECTED (403): chat mode forbids you right now. Run 'mode' to see who may talk; wait or ask the admin."
      exit 4
    elif [ "$code" = "429" ]; then
      echo >&2 "RATE-LIMITED (429): you are posting too fast. Back off; do NOT loop-retry."
      exit 5
    elif [ "$code" = "401" ]; then
      echo >&2 "UNAUTHORIZED (401): stale session/davet; reconnect through Lobby before retrying"
      exit 7
    fi
    # The durable runtime queue ACKs a delivery when the agent's reply is
    # actually accepted by Loca, not when a potentially long Codex turn later
    # finishes. The receipt path is a private supervisor-created capability
    # inherited only by the active adapter/tool process.
    if [[ "$code" =~ ^2[0-9][0-9]$ ]] && [ -n "${LOCA_REPLY_RECEIPT:-}" ]; then
      receipt_dir=$(dirname -- "$LOCA_REPLY_RECEIPT")
      if [ -d "$receipt_dir" ]; then
        receipt_tmp="$LOCA_REPLY_RECEIPT.$$.$RANDOM.tmp"
        umask 077
        printf '%s\n' "$op_id" > "$receipt_tmp"
        chmod 600 "$receipt_tmp"
        mv -f -- "$receipt_tmp" "$LOCA_REPLY_RECEIPT"
      fi
    fi
    ;;

  release)
    # release <server> <room> <name> — "işim bitti". End THIS identity's
    # davet/seat without punishment; building membership remains in lobby.
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    code=$(curl -sS -o /dev/null -w '%{http_code}' -m 10 \
      -H 'content-type: application/json' "${tok_args[@]}" \
      -X POST "$server/rooms/$room/release")
    if [ "$code" != "204" ]; then
      echo "release failed (HTTP $code): your own davet/session for '$room' is required" >&2
      exit 7
    fi
    # The server ended both davet and session. Remove their stale local copies
    # so the next command cannot keep presenting a dead credential.
    if [ -f "$LOCA_ENV" ]; then
      {
        printf 'DAVET_%s\0\0' "$(_var "$room")"
        printf 'LOCA_SESSION\0\0'
      } | _credential_patch
    fi
    unset LOCA_SESSION
    printf "released '%s' from '%s'; building membership stays in lobby\n" "$name" "$room"
    ;;

  listen)
    server="$1"; room="$2"; name="$3"
    log="${ROOM_LOG:-.agent-room/$room.jsonl}"
    mkdir -p "$(dirname "$log")"
    ws=$(printf '%s' "$server" | sed -E 's#^http#ws#')
    # filter=msg -> server sends only real chat messages (no typing/members noise).
    url="$ws/ws?room=$room&name=$name&type=agent&filter=msg"
    # Keep credentials out of argv/process listings. listen.py selects the
    # name-specific env, validates its server origin, then adds the loca's davet
    # and session internally.
    echo "listening on $server room=$room name=$name -> $log" >&2
    exec python3 "$SCRIPT_DIR/listen.py" "$url" "$log"
    ;;

  setup)
    # setup <server> <name> — prepare THIS machine once. The davet
    # opens one loca; the master issues it (scripts/invite.sh) and hands it
    # over privately. It is read from stdin, never argv/process listings.
    # Writes ~/.loca/env (600), takes a session, verifies access.
    server="${1:-${ROOM_SERVER_URL:-}}"
    name="${2:-$(hostname -s)}"
    if [ -z "$server" ]; then
      echo "server is required; pass the self-hosted or hosted http(s) origin explicitly" >&2
      echo "example: connect.sh setup https://loca.example agent-name" >&2
      exit 2
    fi
    if ! [[ "$server" =~ ^https?://([A-Za-z0-9._-]+|\[[0-9A-Fa-f:]+\])(:[0-9]+)?$ ]]; then
      echo "server must be an http(s) origin without a path, credentials, or shell syntax" >&2
      exit 2
    fi
    if ! [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
      echo "agent name must be 1-64 ASCII letters, digits, dot, dash, or underscore" >&2
      exit 2
    fi
    if [ "${3+x}" = x ]; then
      echo "bootstrap credential must not be passed as an argument; run setup and use its hidden prompt" >&2
      exit 2
    fi
    token=""
    if [ -t 0 ]; then
      printf 'private membership/davet for %s as %s: ' "$server" "$name" >&2
      IFS= read -r -s token
      printf '\n' >&2
    else
      IFS= read -r token
    fi
    # PowerShell and other Windows producers terminate stdin lines with CRLF.
    # `read` removes LF only; CR is transport syntax, never part of a Loca
    # credential, and would otherwise produce a misleading HTTP 400.
    token=${token%$'\r'}
    if [ -z "$token" ]; then
      echo "a private membership or davet is required" >&2
      exit 2
    fi
    # One machine can host several agents (a prod box running a debug agent and
    # a cyber agent, say). They must not share one identity, so unless the
    # caller pinned LOCA_ENV, each name gets its own file.
    if [ "$_LOCA_ENV_PINNED" -eq 0 ] && [ -f "$HOME/.loca/env" ]; then
      existing=$(sed -n 's/^LOCA_NAME=//p' "$HOME/.loca/env" | tr -d '"' | head -1)
      if [ -n "$existing" ] && [ "$existing" != "$name" ]; then
        LOCA_ENV="$HOME/.loca/$(_identity_stem "$name").env"
        echo "note: default identity is '$existing'; using separate file $LOCA_ENV for '$name'"
      fi
    fi
    # Validate before touching the identity file. A mistyped/revoked davet must
    # not leave a poisoned <name>.env that shadows a later valid setup.
    if ! curl -sf -m 10 "$server/health" >/dev/null; then
      echo "cannot reach $server" >&2; exit 1
    fi
    code=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -H "x-room-token: $token" "$server/rooms")
    if [ "$code" != "200" ]; then
      echo "that davet was not accepted (HTTP $code); no identity file was changed." >&2
      echo "ask the master: it may have been revoked, mistyped, or issued for another member." >&2
      exit 1
    fi
    mkdir -p "$(dirname "$LOCA_ENV")"
    umask 177
    # Which loca is this davet for? The server knows; ask before writing, so
    # the davet is filed under its own loca instead of overwriting another.
    who=$(curl -s -m 10 -H "x-room-token: $token" "$server/whoami" || true)
    davet_loca=$(printf '%s' "$who" | jq -r '.loca // empty' 2>/dev/null || true)
    who_kind=$(printf '%s' "$who" | jq -r '.kind // empty' 2>/dev/null || true)
    membership=""
    authoritative_name=""
    if [ "$who_kind" = "member" ]; then
      membership="$token"
      authoritative_name=$(printf '%s' "$who" | jq -r '.name // empty' 2>/dev/null || true)
    elif [ -n "$davet_loca" ]; then
      # A davet proves which building member is holding it. Claim that
      # permanent lobby identity once, so release can end the loca seat while
      # the agent remains reachable for the next one-click call.
      claim=$(curl -sS -m 10 -H 'content-type: application/json' \
        -H "x-room-token: $token" -X POST "$server/membership/claim" || true)
      membership=$(printf '%s' "$claim" | jq -r '.membership_token // empty' 2>/dev/null || true)
      authoritative_name=$(printf '%s' "$claim" | jq -r '.name // empty' 2>/dev/null || true)
      if [ -z "$membership" ]; then
        echo "could not claim building membership; no identity file was changed." >&2
        exit 1
      fi
    fi
    if { [ "$who_kind" = "member" ] || [ -n "$davet_loca" ]; } \
       && [ -z "$authoritative_name" ]; then
      echo "could not verify the server-bound identity; no identity file was changed." >&2
      exit 1
    fi
    if [ -n "$authoritative_name" ] && [ "$authoritative_name" != "$name" ]; then
      echo "credential belongs to '$authoritative_name', not requested identity '$name'; no identity file was changed." >&2
      echo "ask the operator for a membership or davet issued to the exact name '$name'." >&2
      exit 1
    fi
    if [ -f "$LOCA_ENV" ] && grep -q '^ROOM_SERVER_URL=' "$LOCA_ENV" 2>/dev/null; then
      # Already set up here: keep the davets already held and add this one.
      # Being in two locas means holding two davets, not replacing the first.
      {
        printf 'LOCA_SESSION\0\0'
        if [ "$who_kind" = "member" ]; then
          # A permanent building membership opens the lobby only. Storing it
          # as ROOM_TOKEN would make it look like a room davet.
          printf 'ROOM_TOKEN\0\0'
        elif [ -n "$davet_loca" ]; then
          printf 'DAVET_%s\0%s\0' "$(_var "$davet_loca")" "$token"
        else
          printf 'ROOM_TOKEN\0%s\0' "$token"
        fi
        [ -z "$membership" ] || printf 'LOCA_MEMBERSHIP\0%s\0' "$membership"
      } | _credential_patch
      if [ -n "$davet_loca" ] && [ "$who_kind" != "member" ]; then
        echo "added your davet for '$davet_loca' (existing ones kept)"
      fi
    else
      {
        printf 'ROOM_SERVER_URL\0%s\0' "$server"
        printf 'LOCA_NAME\0%s\0' "$name"
        if [ -n "$membership" ]; then
          printf 'LOCA_MEMBERSHIP\0%s\0' "$membership"
        fi
        if [ "$who_kind" = "member" ]; then
          :
        elif [ -n "$davet_loca" ]; then
          printf 'DAVET_%s\0%s\0' "$(_var "$davet_loca")" "$token"
        else
          printf 'ROOM_TOKEN\0%s\0' "$token"
        fi
      } | _credential_patch
    fi
    chmod 600 "$LOCA_ENV"
    echo "wrote $LOCA_ENV (600)"
    if [ -n "$membership" ]; then
      echo "lobby membership bound — release sonrası tek tıkla geri çağrılabilirsin"
    fi
    # Which loca does it actually open? A davet is loca-scoped, so "the server
    # let me in" is not the same as "I can enter a room". Find out now rather
    # than at the first 401.
    opens=""
    rooms_json=$(curl -s -m 10 -H "x-room-token: $token" "$server/rooms" || true)
    for r in $(printf '%s' "$rooms_json" | jq -r '.[].room' 2>/dev/null || true); do
      c=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -H "x-room-token: $token" \
            "$server/rooms/$r/messages" || true)
      if [ "$c" = "200" ]; then opens="$opens $r"; fi
    done
    # A Building membership authenticates Lobby presence only. It cannot mint
    # a loca-scoped session until the master calls this member into a loca and
    # the listener receives that fresh davet. Asking /sessions with an mb_...
    # token returns a non-JSON 401 on a closed server and used to abort fresh
    # installation after the identity file had already been written.
    if [ "$who_kind" != "member" ]; then
      sess=$(curl -sS -m 10 -H 'content-type: application/json' -H "x-room-token: $token" \
        -X POST "$server/sessions" -d "$(jq -n --arg n "$name" --arg l "${davet_loca:-}" '{name:$n, kind:"agent"} + (if $l == "" then {} else {loca:$l} end)')" | jq -r '.session_token // empty')
      if [ -n "$sess" ]; then
        printf 'LOCA_SESSION\0%s\0' "$sess" | _credential_patch
        echo "session bound for '$name' — identity is now server-derived"
      fi
    fi
    echo "ready: $server as '$name'"
    held=$(sed -n 's/^DAVET_\([^=]*\)=.*/\1/p' "$LOCA_ENV" | tr '\n' ' ')
    if [ -n "$held" ]; then
      echo "your davets open: $held"
    elif [ -n "$opens" ]; then
      echo "your davet opens:$opens"
    else
      echo "note: your davet opens no loca yet — ask the master to invite you to one." >&2
    fi
    if [ "$LOCA_ENV" != "$HOME/.loca/env" ]; then
      echo
      echo "this identity lives in a side file, so every command must point at it:"
      echo "  export LOCA_ENV=$LOCA_ENV"
      echo "put that line in this agent's shell profile, or prefix each call."
    fi
    ;;

  session)
    # session <server> <name> [agent|user]  -> prints a session token.
    # Usage:  export LOCA_SESSION=$(connect.sh session $SERVER myname agent)
    server="$1"; name="$2"; kind="${3:-agent}"
    payload=$(jq -n --arg n "$name" --arg k "$kind" '{name:$n, kind:$k}')
    curl_json -X POST "$server/sessions" -d "$payload" | jq -r '.session_token'
    ;;

  doctor)
    # doctor <server> [--fix]
    # Health check for the CLIENT side — the pains we actually hit: leaked
    # listener/bot processes, duplicate (room,name) connections shadowing each
    # other, a stale process squatting on the server port. --fix kills the
    # flagged duplicates (keeps the newest per room+name).
    server="${1:-http://127.0.0.1:8787}"; fix="${2:-}"
    echo "── server ──"
    if h=$(curl -sS -m 5 "$server/health" 2>/dev/null); then
      echo "  ok: $h"
    else
      echo "  UNREACHABLE: $server"
      port=$(printf '%s' "$server" | sed -E 's#.*:([0-9]+).*#\1#')
      holder=$(ss -tlnp 2>/dev/null | grep ":$port " | head -1)
      [ -n "$holder" ] && echo "  port $port holder: $holder"
    fi
    # Are we set up for THIS server? The question every agent actually asks
    # before joining, and the one setup answers.
    echo "── identity ──"
    # Look where setup actually writes: $LOCA_ENV if pinned, else every
    # ~/.loca/*.env, because one machine can host several agents (a prod box
    # running cyber and debug side by side).
    found=0
    seen=""
    invited_names_file="/tmp/loca-doctor-invited.$$"
    listener_names_file="/tmp/loca-doctor-listeners.$$"
    : > "$invited_names_file"
    : > "$listener_names_file"
    for f in ${LOCA_ENV:+"$LOCA_ENV"} "$HOME"/.loca/env "$HOME"/.loca/*.env; do
      [ -f "$f" ] || continue
      # ~/.loca/env matches both the explicit path and the glob; report once.
      case " $seen " in *" $f "*) continue ;; esac
      seen="$seen $f"
      if env_server=$(_credential_value "$f" ROOM_SERVER_URL 2>&1); then
        :
      else
        credential_status=$?
        if [ "$credential_status" -eq 2 ]; then
          echo "  INVALID: $env_server"
        fi
        continue
      fi
      [ -n "$env_server" ] || continue
      [ "${env_server%/}" = "${server%/}" ] || continue
      found=1
      env_name=$(sed -n 's/^LOCA_NAME=//p' "$f" | tr -d '"' | head -1)
      locas=$(sed -n 's/^DAVET_\([^=]*\)=.*/\1/p' "$f" | tr '\n' ' ')
      sess=$(sed -n 's/^LOCA_SESSION=//p' "$f" | tr -d '"' | head -1)
      membership=$(sed -n 's/^LOCA_MEMBERSHIP=//p' "$f" | tr -d '"' | head -1)
      if [ -n "$membership" ]; then
        identity=$(curl -sfS -m 5 -H "x-room-token: $membership" "$server/whoami" 2>/dev/null || true)
        if [ "$(printf '%s' "$identity" | jq -r '.kind // empty' 2>/dev/null)" = "member" ]; then
          mapfile -t invited_locas < <(printf '%s' "$identity" | jq -r '.locas[]?' 2>/dev/null)
          verified=(); stale=(); expected_keys=" "
          for loca in "${invited_locas[@]}"; do
            key="DAVET_$(_var "$loca")"
            expected_keys+="$key "
            token=$(_credential_value "$f" "$key" 2>/dev/null || true)
            if _davet_is_live "$server" "$loca" "$token"; then
              verified+=("$loca")
            else
              stale+=("$loca")
            fi
          done
          while IFS= read -r key; do
            [ -n "$key" ] || continue
            case "$expected_keys" in *" $key "*) ;; *) stale+=("${key#DAVET_}") ;; esac
          done < <(sed -n 's/^\(DAVET_[A-Za-z0-9_]*\)=.*/\1/p' "$f")
          if [ "${#verified[@]}" -gt 0 ]; then
            verified_list=$(IFS=,; printf '%s' "${verified[*]}")
            echo "  INVITED (davet verified): '${env_name:-?}' — ${verified_list} ($f)"
            [ -z "$env_name" ] || printf '%s\n' "$env_name" >> "$invited_names_file"
            session_loca=$(_session_loca "$server" "$env_name" "$sess" || true)
            if [ -n "$session_loca" ]; then
              echo "  POSTING SESSION: ready for $session_loca ($f)"
            else
              echo "  POSTING SESSION: renewal required; sends may return 401 ($f)"
            fi
            if [ "${#stale[@]}" -gt 0 ]; then
              stale_list=$(IFS=,; printf '%s' "${stale[*]}")
              echo "  STALE LOCAL CACHE (ignored): '${env_name:-?}' — ${stale_list} ($f)"
            fi
          elif [ "${#stale[@]}" -gt 0 ]; then
            stale_list=$(IFS=,; printf '%s' "${stale[*]}")
            echo "  STALE: '${env_name:-?}' — revoked, expired, missing, or replaced davet credentials: $stale_list ($f)"
          else
            echo "  LOBBY: '${env_name:-?}' — membership valid, no active loca davet ($f)"
          fi
          if [ "$fix" = "--fix" ] && [ "${#stale[@]}" -gt 0 ]; then
            {
              for loca in "${stale[@]}"; do
                printf 'DAVET_%s\0\0' "$(_var "$loca")"
              done
              printf 'LOCA_SESSION\0\0'
            } | _credential_patch_file "$f"
            echo "  fixed: removed stale local davet/session cache; the Lobby listener will replay live calls"
          fi
          continue
        fi
      fi
      if curl -sfS -m 5 -H "X-Session-Token: $sess" "$server/rooms" >/dev/null 2>&1; then
        echo "  ready: '${env_name:-?}' — davets: ${locas:-none} ($f)"
      else
        echo "  '${env_name:-?}' — session stale, renews on next use ($f)"
      fi
    done
    if [ "$found" = 0 ]; then
      echo "  no identity for $server — run: connect.sh setup $server <name>"
      echo "  (a davet comes from the master, privately — never from a room)"
    fi
    echo "── loca client processes (all servers) ──"
    found=0
    # pid \t age \t cmd  for every listener/bot/adapter
    while IFS= read -r line; do
      pid="${line%% *}"
      cmd="${line#* }"
      age=$(ps -o etime= -p "$pid" 2>/dev/null | tr -d ' ')
      # room+name extracted from the ws url in the args
      key=$(printf '%s' "$cmd" | grep -oE 'room=[^& ]+&name=[^& "]+' | head -1)
      echo "  pid=$pid age=$age ${key:-?} :: $(printf '%s' "$cmd" | cut -c1-90)"
      listener_name=$(printf '%s' "$key" | sed -n 's/.*&name=//p')
      [ -z "$listener_name" ] || printf '%s\n' "$listener_name" >> "$listener_names_file"
      found=1
      echo "$pid $key" >> /tmp/loca-doctor.$$
    done < <(
      pgrep -af 'listen\.py|bot\.py|agentd\.py' |
        grep -v doctor |
        grep -v 'runtime_agent\.py' |
        grep -v 'monitor_listener\.py' |
        grep -vE '^[0-9]+ +(/bin/)?(ba)?sh '
    )
    [ "$found" = 0 ] && echo "  none"
    echo "── listener coverage ──"
    coverage=0
    while IFS= read -r invited_name; do
      [ -n "$invited_name" ] || continue
      coverage=1
      if grep -Fxq "$invited_name" "$listener_names_file"; then
        echo "  OK: '$invited_name' has a live listener"
      else
        echo "  MISSING LISTENER: '$invited_name' has a verified davet but no live listener; delivery/presence/session renewal can silently split"
      fi
    done < <(sort -u "$invited_names_file")
    [ "$coverage" = 1 ] || echo "  no invited identities to check"
    rm -f "$invited_names_file" "$listener_names_file"
    echo "── duplicates (same room+name — the older one is a ghost) ──"
    dup=0
    if [ -f /tmp/loca-doctor.$$ ]; then
      while IFS= read -r key; do
        pids=$(grep -F " $key" /tmp/loca-doctor.$$ | awk '{print $1}')
        n=$(printf '%s\n' "$pids" | wc -l)
        if [ "$n" -gt 1 ]; then
          dup=1
          keep=$(printf '%s\n' "$pids" | tail -1)
          kill_list=$(printf '%s\n' "$pids" | head -n -1 | tr '\n' ' ')
          echo "  $key -> $n processes (keep newest pid=$keep, stale: $kill_list)"
          if [ "$fix" = "--fix" ]; then
            for p2 in $kill_list; do kill "$p2" 2>/dev/null && echo "    killed $p2"; done
          fi
        fi
      done < <(awk '{ $1=""; sub(/^ /,""); print }' /tmp/loca-doctor.$$ | sort -u | grep -v '^$')
      rm -f /tmp/loca-doctor.$$
    fi
    [ "$dup" = 0 ] && echo "  none"
    if [ "$dup" = 1 ] && [ "$fix" != "--fix" ]; then
      echo "run with --fix to kill the stale ones:  $0 doctor $server --fix"
    fi
    echo "── wake bridges ──"
    # Legacy Claude setups split delivery and wake across a project-local
    # JSONL plus tail/grep. Delivery looked healthy, but native Monitor did
    # not own the WebSocket and could miss, delay, or filter the model turn.
    # A process/roster check cannot call that healthy, so report both halves.
    legacy_wake_bridges=$(
      ps -eo pid=,comm=,args= 2>/dev/null |
        awk '
          {
            line=tolower($0)
            filtered_grep=(($2 ~ /^(grep|ugrep)$/ || $3 ~ /^(grep|ugrep)$/) &&
              line ~ /target/ && line ~ /all/)
            project_tail=(($2 == "tail" || $3 == "tail") &&
              line ~ /tail[[:space:]].*-f/ &&
              line ~ /\.agent-room\/[^[:space:]]*\.jsonl/)
            project_listener=($2 ~ /^python/ && line ~ /listen\.py/ &&
              line !~ /monitor_listener\.py/ &&
              line ~ /\.agent-room\/[^[:space:]]*\.jsonl/)
            if (filtered_grep || project_tail || project_listener) {
              sub(/^[[:space:]]+/, "")
              print
            }
          }
        '
    )
    if [ -n "$legacy_wake_bridges" ]; then
      echo "  DEGRADED: legacy project-local JSONL/tail wake bridge found"
      printf '%s\n' "$legacy_wake_bridges" |
        while IFS= read -r wake_line; do
          echo "    $wake_line"
        done
      echo "  Claude Code must run monitor_listener.py inside one native persistent Monitor."
      echo "  Its only child owns listen.py; do not put a project-local file, tail, or grep after it."
    else
      echo "  no legacy project-local JSONL/tail bridge detected"
    fi
    ;;

  *)
    echo "unknown command: $cmd (see header of $0)" >&2
    exit 2
    ;;
esac
