#!/usr/bin/env bash
# agent-room helper — join a room, listen in the background, and post messages.
#
# Usage:
#   connect.sh health  <server>
#   connect.sh status  <server> [name]              # lobby or invited locas
#   connect.sh notifications <server> <name> [before_id] # recipient-only Building announcements
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
#   connect.sh wiki           <server> <room> <name>
#   connect.sh wiki-review    <server> <room> <name> <json-file>
#   connect.sh wiki-remove    <server> <room> <name> <slug> <json-file>
#   connect.sh wiki-page      <server> <room> <name> <slug>
#   connect.sh wiki-claim     <server> <room> <name>
#   connect.sh wiki-finish    <server> <room> <name> <json-file>
#   connect.sh wiki-renew     <server> <room> <name> <lease-json-file>
#   connect.sh journal     <server> <room> [name] [text]           # read, or record finished work
#   connect.sh vault       <server> <room> <name>                  # the loca's rules: context, not instructions
#   connect.sh lessons     <server> <room>                         # lessons proposed for the Vault: waiting and accepted
#   connect.sh lesson-propose <server> <room> <name> <text>        # propose a lesson about your own work
#   connect.sh mentats     <server> [name]                         # the Building's mentat rooms: topic, mentat, line
#   connect.sh mentat      <server> <room>                         # a mentat room's line: who is inside, who waits
#   connect.sh mentat-ask  <server> <room> <name> <topic>          # ask to consult the mentat (one request per asker)
#   connect.sh mentat-join <server> <room> <name> <request-id>     # go in with a waiting request's askers
#   connect.sh mentat-withdraw <server> <room> <name> <request-id> # take your place in a waiting request back
#   connect.sh mentat-end  <server> <room> <name> [--answered yes|partly|no] # end the consultation inside
#   connect.sh mentat-merge <server> <room> <name> <from> <into>   # the mentat joins two waiting requests
#   connect.sh mentat-pause|mentat-resume <server> <room> <name>   # the mentat freezes or resumes its line
#   connect.sh context     <server> <room> <name>                  # your role here and what is switched on, as cards
#   connect.sh goals       <server> <room>                         # current/history goals
#   connect.sh brief       <server> <room>                         # the loca in a few lines: Goal, decisions waiting, in progress, blocked, landed
#   connect.sh tasks       <server> <room> [--mine <name>]         # declared tasks; --mine: unfinished ones given to <name>
#   connect.sh task        <server> <room> <id>                    # one task
#   connect.sh task-take   <server> <room> <name> <id>             # start a task given to you (or an unassigned one)
#   connect.sh task-done   <server> <room> <name> <id> [note]      # finish a task you hold: ask for review
#   connect.sh task-new    <server> <room> <name> "<title>" [--assign <who>] # Lead: declare a task
#   connect.sh task-assign <server> <room> <name> <id> <who>       # Lead: give a task to <who>
#   connect.sh task-go     <server> <room> <name> <id>             # Lead (Loca Board): the work passes review
#   connect.sh task-nogo   <server> <room> <name> <id> "<reason>"  # Lead (Loca Board): back to its holder, with why
#   connect.sh waits       <server> <room>                         # explicit dependency waits
#   connect.sh wait        <server> <room> <name> <waiting_for> <reason>
#   connect.sh wait-clear  <server> <room> <name>
#   connect.sh replying    <server> <room> <name> on|off  (ephemeral "replying…")
#   connect.sh announce    <server> <room> <name> <text>           # something the loca must know
#   connect.sh attention-resolve <server> <room> <name> <attention_id> # owner: no action needed
#   connect.sh mode        <server> <room>                        # current chat mode
#   connect.sh settings    <server> <room>                        # rate limit
#   connect.sh setup       <server> [name]                        # one-time: hidden credential prompt
#   connect.sh first-reply <server> <name> [--wait S] [--after ID] # last step: a Lobby call gets your worker's answer
#   connect.sh session     <server> <name> [agent|user]           # get a session token (export LOCA_SESSION=...)
#   connect.sh reconnect   <server> <name>                        # restart an existing managed runtime
#   connect.sh doctor      [server] [--fix]                       # client health: leaked/duplicate listeners, port squatters
#   connect.sh owner       <server> <name> [<user>|--clear]       # who this agent's user is (references/authority.md)
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

# This agent's user (LOCA_OWNER), as status and owner report it.
_owner_line() {
  if [ -n "${LOCA_OWNER:-}" ]; then
    echo "OWNER: @${LOCA_OWNER} — only this person, verified by the server, asks for what is the user's own (references/authority.md, level 3)"
  else
    echo "OWNER: not set — level 3 requests come only from your terminal; set it with: connect.sh owner <server> <name> <user>"
  fi
}

# Credential files are DATA, never shell programs.  Sourcing this file used to
# let a crafted display name such as `$(command)` execute on the agent's host
# the next time connect.sh ran.  Parse only the small credential vocabulary and
# export values literally — no eval, source, command substitution, or escape
# interpretation.
_credential_key_allowed() {
  case "$1" in
    ROOM_SERVER_URL|LOCA_NAME|LOCA_SESSION|LOCA_SESSION_LOCA|LOCA_LOBBY_SESSION|LOCA_MEMBERSHIP|ROOM_TOKEN|LOCA_OWNER) return 0 ;;
  esac
  [[ "$1" =~ ^DAVET_[A-Za-z0-9_]+$ ]]
}

# A loca name may hold spaces and any letter ("DOB İLE ALAKALI İŞ :D"); it
# is always percent-encoded before it goes into a URL path.
_room_path() {
  python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
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
  unset LOCA_NAME LOCA_SESSION LOCA_LOBBY_SESSION LOCA_MEMBERSHIP ROOM_SERVER_URL ROOM_TOKEN LOCA_OWNER
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
# LOCA_NAME, the permanent lobby membership, LOCA_SESSION, and one invite per
# loca:
#
#   LOCA_MEMBERSHIP=mb_...
#   DAVET_general=dv_...
#   DAVET_mobile=dv_...
#
# The Lobby room (`lobby`) takes no invite: the membership key opens it, and a
# post there carries LOCA_LOBBY_SESSION, a Building session taken with that
# key (no loca), kept apart from the loca session in LOCA_SESSION.
#
# An invite belongs to a loca, not to a person — being in two locas means holding
# two invites, because you were invited to two places. Revoking one leaves the
# other standing. ROOM_TOKEN (no suffix) is the older building key: still read,
# still works, used when no per-loca invite matches.
#
# Keeping them in a 600 file means they are never pasted into a room or a chat
# — the room is the wrong channel for secrets.
# On a multi-agent machine, commands that carry a participant name select that
# name's file before the compatibility default. The Python clients repeat this
# selection as their final credential boundary.
_requested_identity=""
case "${1:-}" in
  listen|send|release|note-create|note-update|memory-short|memory-append|wait|wait-clear|replying|task-take|task-done|task-new|task-assign|task-go|task-nogo|wiki|wiki-review|wiki-remove|wiki-page|wiki-claim|wiki-finish|wiki-renew|wiki-ready|wiki-context|wiki-requests|wiki-request|wiki-accept|wiki-offer|journal|vault|context|lesson-propose|mentat-ask|mentat-join|mentat-withdraw|mentat-end|mentat-merge|mentat-pause|mentat-resume|announce|attention-claim|attention-resolve)
    _requested_identity="${4:-}"
    ;;
  session|status|reconnect|notifications|owner|first-reply|mentats)
    _requested_identity="${3:-}"
    ;;
  tasks)
    [ "${4:-}" = "--mine" ] && _requested_identity="${5:-}"
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

# jq on MSYS/Windows can emit CRLF even inside command/process substitution.
# Normalize every raw-text read centrally so comparisons, credentials and
# array entries never retain a trailing carriage return.
_jq_raw() { jq -r "$@" | tr -d '\r'; }
# One line when this kit is older than the server version $1 (kit_version.py);
# nothing otherwise, and never a failure.
_kit_notice() {
  if [ -n "${LOCA_KIT_NOTICE_SHOWN:-}" ] || [ ! -f "$SCRIPT_DIR/kit_version.py" ]; then
    return 0
  fi
  python3 "$SCRIPT_DIR/kit_version.py" notice "${1:-}" 2>/dev/null || true
}

cmd="${1:-}"; shift || true

# Pick the invite for a given loca, falling back to the building key. Bash
# cannot index a variable name directly, so go through the environment.
invite_for() {
  local loca="$1" var val
  if [ "$loca" = "lobby" ]; then
    # The Lobby room is entered by membership, never by an invite.
    printf '%s' "${LOCA_MEMBERSHIP:-${ROOM_TOKEN:-}}"
    return
  fi
  var="DAVET_$(_var "$loca")"
  val=$(printf '%s' "${!var:-}")
  printf '%s' "${val:-${ROOM_TOKEN:-}}"
}

# Verify the exact loca credential held by this client. Membership /whoami
# only says that the Building currently has an invitation for the identity;
# it does not prove that the DAVET_<loca> value on disk is the current token.
_invite_is_live() {
  local server="$1" loca="$2" token="$3" response code body kind opened
  [ -n "$token" ] || return 1
  response=$(curl -sS -m 5 -H "x-room-token: $token" -w $'\n%{http_code}' \
    "$server/whoami" 2>/dev/null || true)
  code=$(printf '%s' "$response" | tail -n1)
  body=$(printf '%s' "$response" | sed '$d')
  [ "$code" = "200" ] || return 1
  kind=$(printf '%s' "$body" | _jq_raw '.kind // empty' 2>/dev/null)
  opened=$(printf '%s' "$body" | _jq_raw '.loca // empty' 2>/dev/null)
  [ "$kind" = "building" ] || { [ "$kind" = "davet" ] && [ "$opened" = "$loca" ]; }
}

# Locas where the listener found a seat but no kept invite that fits it. The
# server no longer re-sends an invite it has already delivered, so these wait for
# the Master. listen.py writes the file next to the identity; it holds loca
# names only, never a credential.
_needs_fresh_davet() {
  local file="$1.davet-status.json"
  [ -f "$file" ] || return 0
  _jq_raw '.needs_fresh_davet[]? | strings' "$file" 2>/dev/null || true
}

# Whether the listener was told by the server that this membership is
# suspended. A current server says so itself (`/whoami` answers 403
# `membership suspended`); an older one refuses a suspended and a revoked
# membership alike (401), so only the listener, which received
# `membership_suspended`, knows.
_membership_suspended() {
  local file="$1.davet-status.json"
  [ -f "$file" ] || return 1
  [ "$(_jq_raw '.membership_suspended // false' "$file" 2>/dev/null)" = "true" ]
}

_in_list() {
  local needle="$1" item
  shift
  for item in "$@"; do
    [ "$item" = "$needle" ] && return 0
  done
  return 1
}

# An invite proves that the seat may enter a loca. Posting uses a separate,
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
  kind=$(printf '%s' "$body" | _jq_raw '.kind // empty' 2>/dev/null)
  owner=$(printf '%s' "$body" | _jq_raw '.name // empty' 2>/dev/null)
  [ "$kind" = "session" ] && [ "$owner" = "$name" ] || return 1
  printf '%s' "$body" | _jq_raw '.loca // empty' 2>/dev/null
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

# Point every following call at one loca's invite. Called once a command knows
# which loca it is talking to — before that we only hold the fallback.
# The Lobby room is the exception: its credential is the membership key, which
# opens a read by itself; only a write ($2 = write) carries the Building session
# taken with that key. A loca's session is never presented there (it opens its
# own loca only, and the Lobby's door would refuse it).
# Does the kept loca session belong to loca $1? A session opens the one loca
# it was taken for; presented at another loca's door it is refused, and the
# server answers as if that loca did not exist (a floor loca's existence is
# not told to a stranger). LOCA_SESSION_LOCA says which loca it is for. A file
# from before it was kept says nothing: then the session is trusted only
# while this identity holds no more than one loca invite.
_session_fits() {
  local loca="$1" var count=0
  [ -n "${LOCA_SESSION:-}" ] || return 1
  if [ -n "${LOCA_SESSION_LOCA:-}" ]; then
    [ "$LOCA_SESSION_LOCA" = "$loca" ]
    return
  fi
  while IFS= read -r var; do
    [ -n "${!var:-}" ] && count=$((count + 1))
  done < <(compgen -v DAVET_)
  [ "$count" -le 1 ]
}

use_loca() {
  local d
  d=$(invite_for "$1")
  tok_args=()
  [ -n "$d" ] && tok_args=(-H "x-room-token: $d")
  if [ "$1" = "lobby" ]; then
    [ "${2:-}" = "write" ] && [ -n "${LOCA_LOBBY_SESSION:-}" ] \
      && tok_args+=(-H "x-session-token: $LOCA_LOBBY_SESSION")
  elif _session_fits "$1"; then
    tok_args+=(-H "x-session-token: $LOCA_SESSION")
  else
    # Another loca's session, or none. A read needs none: this loca's invite
    # opens it. A write takes a session from THIS loca's invite first, so
    # every write path presents the right one, not only those that renew on
    # a 401.
    LOCA_SESSION=""
    if [ "${2:-}" = "write" ] && [ -n "$d" ] && [ -n "${server:-}" ]; then
      _renew_session "$1" "${name:-${LOCA_NAME:-}}" || true
    fi
  fi
  ROOM_TOKEN="$d"   # so the session-renewal path below re-mints with it
}

# The /sessions body for $1=name in $2=room: scoped to the loca, or, for the
# Lobby room, a Building session (no loca) taken with the membership key.
_session_request() {
  if [ "$2" = "lobby" ]; then
    jq -n --arg n "$1" '{name:$n, kind:"agent"}'
  else
    jq -n --arg n "$1" --arg l "$2" '{name:$n, kind:"agent", loca:$l}'
  fi
}

# Keep a freshly minted session for $1=room in its own slot.
_keep_session() {
  local room="$1" new="$2" slot=LOCA_SESSION
  [ "$room" = "lobby" ] && slot=LOCA_LOBBY_SESSION
  export "$slot=$new"
  [ "$room" = "lobby" ] || export LOCA_SESSION_LOCA="$room"
  if [ -f "$LOCA_ENV" ]; then
    {
      printf '%s\0%s\0' "$slot" "$new"
      [ "$room" = "lobby" ] || printf 'LOCA_SESSION_LOCA\0%s\0' "$room"
    } | _credential_patch
  fi
}

# A request body never rides on curl's command line. Windows curl.exe reads
# its arguments in the system ANSI code page (Windows-1254 on a Turkish
# system), so a UTF-8 body sent with `-d "$body"` reached the server as
# e7 fd f0 for "çığ" and was refused as invalid UTF-8. `_curl` takes the same
# arguments as curl and hands the one `-d BODY` to it on stdin
# (`--data-binary @-`), which carries the bytes unchanged on every platform.
# Each call pipes the body again, so a caller's retry loop resends it intact.
_curl() {
  local args=() body="" has_body=0
  while [ "$#" -gt 0 ]; do
    if [ "$1" = -d ] && [ "$#" -ge 2 ]; then
      [ "$has_body" = 0 ] || { echo "_curl: one request body per call" >&2; return 2; }
      body="$2" has_body=1
      args+=(--data-binary @-)
      shift 2
    else
      args+=("$1")
      shift
    fi
  done
  if [ "$has_body" = 1 ]; then
    printf '%s' "$body" | curl "${args[@]}"
  else
    curl "${args[@]}"
  fi
}

curl_json() { _curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" "$@"; }
# Reading is as private as writing — every GET carries the key as well.
# Only an accepted read reaches stdout. A refusal leaves stdout empty, says the
# server's own sentence on stderr and exits with the code `send` uses for the
# same cause: 7 for 401, 4 for 403, 5 for 429, 8 for anything else (404 and an
# unreachable server included). A read does not renew a session: the loca's
# invite opens a read by itself, so a 401 here means that invite was refused.
curl_get() {
  local response code body said rc=0
  response=$(curl -sS -m 10 "${tok_args[@]}" -w $'\n%{http_code}' "$@") || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "READ FAILED: no answer from the server (curl exit $rc); nothing was read" >&2
    return 8
  fi
  code=$(printf '%s' "$response" | tail -n1)
  body=$(printf '%s' "$response" | sed '$d')
  if [[ "$code" =~ ^2[0-9][0-9]$ ]]; then
    printf '%s' "$body"
    return 0
  fi
  said=$(printf '%s' "$body" | tr '\r\n' '  ')
  said="${said:0:300}"
  case "$code" in
    401)
      echo "UNAUTHORIZED (401): ${said:-no reason given}. Nothing was read. 'davet required' usually means the wrong identity file: set LOCA_ENV to this agent's ~/.loca/<name>.env" >&2
      return 7 ;;
    403) echo "REFUSED (403): ${said:-no reason given}. Nothing was read; do not retry" >&2; return 4 ;;
    429) echo "RATE-LIMITED (429): ${said:-no reason given}. Back off; do NOT loop-retry" >&2; return 5 ;;
    404) echo "NOT FOUND (404): ${said:-no reason given}. Nothing was read" >&2; return 8 ;;
    *) echo "READ FAILED (HTTP $code): ${said:-no reason given}. Nothing was read" >&2; return 8 ;;
  esac
}

# Guess an attachment's content-type from its filename. Empty means "let the
# server sniff": we then send an EMPTY content-type header so curl's default
# form type is never sent and mistaken for the real one (which would 415).
_mime_for() {
  case "${1,,}" in
    *.png)          printf 'image/png' ;;
    *.jpg|*.jpeg)   printf 'image/jpeg' ;;
    *.webp)         printf 'image/webp' ;;
    *.pdf)          printf 'application/pdf' ;;
    *.zip)          printf 'application/zip' ;;
    *.txt)          printf 'text/plain' ;;
    *.md|*.markdown) printf 'text/markdown' ;;
    *.html|*.htm)   printf 'text/html' ;;
    *.json)         printf 'application/json' ;;
    *.csv)          printf 'text/csv' ;;
    *.yaml|*.yml)   printf 'application/yaml' ;;
    *.xml)          printf 'application/xml' ;;
    *.log)          printf 'text/plain' ;;
    *)              printf '' ;;
  esac
}

# Mint a fresh session for $1=room as $2=name from this loca's invite, updating
# LOCA_SESSION + tok_args in place. Returns 0 on success. Used by the attachment
# upload path when a stale session 401s before the message post (the message
# post has its own inline renewal).
_renew_session() {
  local room="$1" who="$2" key resp code body new
  key=$(invite_for "$room")
  [ -z "$key" ] && return 1
  resp=$(_curl -sS -m 10 -H 'content-type: application/json' -H "x-room-token: $key" \
    -w $'\n%{http_code}' -X POST "$server/sessions" \
    -d "$(_session_request "$who" "$room")") || return 1
  code=$(printf '%s' "$resp" | tail -n1)
  body=$(printf '%s' "$resp" | sed '$d')
  [ "$code" = "201" ] || return 1
  new=$(printf '%s' "$body" | _jq_raw '.session_token // empty' 2>/dev/null || true)
  [ -z "$new" ] && return 1
  _keep_session "$room" "$new"
  tok_args=(-H "x-room-token: $key" -H "x-session-token: $new")
  return 0
}

# A task change is a write the server may refuse (house rules, a closed loca,
# an extension that is off). Only an accepted change reaches stdout, as the
# server's own record of the task. A refusal leaves stdout empty, says the
# server's sentence on stderr and exits with the codes curl_get uses: 7 for
# 401, 4 for 403, 5 for 429, 8 for anything else (404, 409 and an unreachable
# server included). The author is the session this loca's invite opens, never
# the name in the body, so a session is taken first and renewed once on 401.
_task_write() {
  local response code body said rc attempt
  [ -n "${LOCA_SESSION:-}" ] || _renew_session "$room" "$name" || true
  for attempt in 1 2; do
    rc=0
    response=$(_curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" \
      -w $'\n%{http_code}' "$@") || rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "TASK NOT CHANGED: no answer from the server (curl exit $rc)" >&2
      return 8
    fi
    code=$(printf '%s' "$response" | tail -n1)
    body=$(printf '%s' "$response" | sed '$d')
    if [ "$code" = 401 ] && [ "$attempt" = 1 ]; then
      _renew_session "$room" "$name" && continue
    fi
    break
  done
  if [[ "$code" =~ ^2[0-9][0-9]$ ]]; then
    printf '%s\n' "$body"
    return 0
  fi
  said=$(printf '%s' "$body" | tr '\r\n' '  ')
  said="${said:0:300}"
  case "$code" in
    401) echo "UNAUTHORIZED (401): ${said:-no reason given}. The task was not changed" >&2; return 7 ;;
    403) echo "REFUSED (403): ${said:-no reason given}. The task was not changed; do not retry" >&2; return 4 ;;
    429) echo "RATE-LIMITED (429): ${said:-no reason given}. The task was not changed; back off" >&2; return 5 ;;
    *) echo "TASK NOT CHANGED (HTTP $code): ${said:-no reason given}" >&2; return 8 ;;
  esac
}

# One task from the loca's list, by its number. Exit 8 when there is none.
_task_by_id() {
  local list="$1" id="$2"
  printf '%s' "$list" | jq -ce --argjson id "$id" '.[] | select(.id == $id)' 2>/dev/null || {
    echo "NOT FOUND: no task #$id in this loca" >&2
    return 8
  }
}

# Wiki success is an HTTP acceptance plus valid JSON, not curl exit zero.
# Retry only authentication renewal once; publication retry is caller-owned
# with the identical fenced payload. Never echo a server's error body.
_wiki_request() {
  local response code body attempt
  for attempt in 1 2; do
    response=$(_curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" \
      -w $'\n%{http_code}' "$@") || {
        echo "Wiki transport unavailable; acceptance unverified" >&2; return 1;
      }
    code=$(printf '%s' "$response" | tail -n1)
    body=$(printf '%s' "$response" | sed '$d')
    if [ "$code" = 401 ] && [ "$attempt" = 1 ]; then
      _renew_session "$room" "$name" && continue
    fi
    case "$code" in
      2??)
        body=$(printf '%s' "$body" | jq -sce 'select(length == 1 and (.[0] | type == "object" or type == "null"))') || {
          echo "Invalid Wiki receipt; acceptance unverified" >&2; return 1;
        }
        printf '%s' "$body" | jq -c '.[0]'
        return 0
        ;;
      *) echo "Wiki request rejected (HTTP $code); no success receipt" >&2; return 1 ;;
    esac
  done
}

# Upload one file to $1=room's attachment store as $3=name; echo the returned
# id (== sha256) on success. Retries once after a session renewal on 401. The
# server sniffs+validates the type, so a bad file is rejected server-side.
_upload_attachment() {
  local room="$1" file="$2" who="$3" mime resp code id
  [ -f "$file" ] || { echo >&2 "attach: no such file: $file"; return 1; }
  # Percent-encoded: a header cannot carry a name like "çalışma.md" as text.
  # shellcheck disable=SC2016 # $f is jq's variable, not the shell's
  local hdr=(-H "x-filename: $(_jq_raw -n --arg f "$(basename -- "$file")" '$f|@uri')")
  mime=$(_mime_for "$file")
  if [ -n "$mime" ]; then hdr+=(-H "content-type: $mime"); else hdr+=(-H "content-type:"); fi
  resp=$(curl -sS -m 30 "${tok_args[@]}" "${hdr[@]}" -w $'\n%{http_code}' \
    --data-binary @"$file" -X POST "$server/rooms/$(_room_path "$room")/attachments") || { echo >&2 "attach: upload request failed for $file"; return 1; }
  code=$(printf '%s' "$resp" | tail -n1)
  if [ "$code" = "401" ] && _renew_session "$room" "$who"; then
    resp=$(curl -sS -m 30 "${tok_args[@]}" "${hdr[@]}" -w $'\n%{http_code}' \
      --data-binary @"$file" -X POST "$server/rooms/$(_room_path "$room")/attachments") || return 1
    code=$(printf '%s' "$resp" | tail -n1)
  fi
  if [[ "$code" =~ ^2[0-9][0-9]$ ]]; then
    id=$(printf '%s' "$resp" | sed '$d' | _jq_raw '.id // empty' 2>/dev/null || true)
    [ -n "$id" ] && { printf '%s' "$id"; return 0; }
  fi
  echo >&2 "attach: upload failed for $file (HTTP $code): $(printf '%s' "$resp" | sed '$d' | head -c 200)"
  return 1
}

case "$cmd" in
  notifications)
    args=("${1:?server required}" "${2:?identity required}")
    if [ -n "${3:-}" ]; then args+=(--before "$3"); fi
    exec python3 "$SCRIPT_DIR/notification_state.py" "${args[@]}"
    ;;
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
      echo "file a join request for this exact name and wait for a Master to approve it: connect.sh request-join $server $requested_name" >&2
      echo "or, if the operator gave you a private membership (Lobby) or invite (one loca) for this exact name, run setup with it" >&2
      echo "do not reuse another agent's env or ask loca-care to create the identity" >&2
      exit 3
    fi
    response=$(curl -sS -m 10 -H "x-room-token: $membership" -w $'\n%{http_code}' \
      "$server/whoami")
    code=$(printf '%s' "$response" | tail -n1)
    identity=$(printf '%s' "$response" | sed '$d')
    if [ "$code" != "200" ] \
      || [ "$(printf '%s' "$identity" | _jq_raw '.kind // empty')" != "member" ]; then
      if [ "$code" = "403" ] && [ "$identity" = "membership suspended" ]; then
        echo "membership suspended — the server says the Master suspended this membership; it resumes when the Master resumes it, and a running listener reconnects by itself. Kept invites no longer work; a new call is needed" >&2
        exit 3
      fi
      if _membership_suspended "${LOCA_ENV:-}"; then
        echo "membership suspended — the listener was told the Master suspended this membership; it resumes when the Master resumes it, and a running listener reconnects by itself. Kept invites no longer work; a new call is needed" >&2
        exit 3
      fi
      echo "membership rejected — ask the master to check building membership" >&2
      exit 3
    fi
    who=$(printf '%s' "$identity" | _jq_raw '.name')
    mapfile -t server_locas < <(printf '%s' "$identity" | _jq_raw '.locas[]?')
    mapfile -t needs_fresh < <(_needs_fresh_davet "$LOCA_ENV")
    verified=(); pending=(); refresh=(); stale=(); expected_keys=" "
    for loca in "${server_locas[@]}"; do
      expected_keys+="DAVET_$(_var "$loca") "
      token=$(invite_for "$loca")
      if [ -n "$token" ] && _invite_is_live "$server" "$loca" "$token"; then
        verified+=("$loca")
      elif _in_list "$loca" ${needs_fresh[@]+"${needs_fresh[@]}"}; then
        # The listener saw this seat and holds no invite that fits it. A
        # server that keeps only invite hashes cannot send it again by itself.
        refresh+=("$loca")
      else
        # The SERVER lists this member in this loca, but the local cache has no
        # live invite for it. That is the Master having CALLED the member in: the
        # invite is delivered over the Lobby SOCKET, so it is PENDING a live
        # listener — NOT revoked. Never send the agent to "ask the master".
        pending+=("$loca")
      fi
    done
    while IFS= read -r key; do
      [ -n "$key" ] || continue
      # A local DAVET_ the server no longer lists is genuinely stale (revoked,
      # expired, or replaced); the Lobby listener drops it.
      case "$expected_keys" in *" $key "*) ;; *) stale+=("${key#DAVET_}") ;; esac
    done < <(compgen -A variable DAVET_ || true)
    refresh_line=""
    if [ "${#refresh[@]}" -gt 0 ]; then
      refresh_list=$(IFS=,; printf '%s' "${refresh[*]}")
      refresh_line="NEEDS FRESH DAVET — '$who' is seated in: $refresh_list, but no invite kept here fits that seat, so the listener cannot re-enter it. It needs a fresh invite — ask the Master to re-send. (Keep the listener running; a re-sent invite arrives over the Lobby socket.)"
    fi
    if [ "${#verified[@]}" -gt 0 ]; then
      verified_list=$(IFS=,; printf '%s' "${verified[*]}")
      echo "INVITED (davet verified) — '$who' has: $verified_list"
      session_loca=$(_session_loca "$server" "$who" "${LOCA_SESSION:-}" || true)
      if [ -n "$session_loca" ]; then
        echo "POSTING SESSION: ready for $session_loca"
      else
        echo "POSTING SESSION: renewal required — delivery may still work, but sending can return 401; keep the Lobby listener/runtime supervised"
      fi
      if [ "${#pending[@]}" -gt 0 ]; then
        pending_list=$(IFS=,; printf '%s' "${pending[*]}")
        echo "PENDING DAVET (start your listener) — the Master has called '$who' into: $pending_list; the invite arrives over the Lobby socket, not a status refresh"
      fi
      [ -z "$refresh_line" ] || echo "$refresh_line"
      if [ "${#stale[@]}" -gt 0 ]; then
        stale_list=$(IFS=,; printf '%s' "${stale[*]}")
        echo "STALE LOCAL CACHE (ignored) — replaced, expired, or missing invite credentials for: $stale_list; the Lobby listener will remove them"
      fi
    elif [ "${#pending[@]}" -gt 0 ]; then
      # Membership valid + the Master has called this member in, but no live
      # listener has received the invite yet. This is the "start your listener"
      # state — NOT an error, and NOT a reason to ask the master (the old
      # STALE/exit-4 message sent agents down the wrong path here).
      pending_list=$(IFS=,; printf '%s' "${pending[*]}")
      echo "LOBBY (membership valid) — the Master has CALLED '$who' into: $pending_list"
      echo "the invite arrives over the Lobby SOCKET, not a status refresh — start your listener/Monitor and it becomes INVITED (verified)."
      echo "  Claude Code: one native persistent Monitor (monitor_listener.py). Codex/generic: runtime.sh start '$who'."
      [ -z "$refresh_line" ] || echo "$refresh_line"
    elif [ "${#refresh[@]}" -gt 0 ]; then
      echo "LOBBY (membership valid) — no loca can be entered from here yet."
      echo "$refresh_line"
    elif [ "${#stale[@]}" -gt 0 ]; then
      stale_list=$(IFS=,; printf '%s' "${stale[*]}")
      echo "STALE LOCAL CACHE — '$who' membership is valid but local invite(s) for $stale_list are replaced/expired; the Lobby listener drops them. If you expected access there, ask the master to call you in."
    else
      echo "LOBBY — '$who' membership is valid; no loca invite is active. Start your listener so the Master's call can reach you over the Lobby socket."
    fi
    _owner_line
    # runtime.sh status has said it already: no second /health request.
    [ -n "${LOCA_KIT_NOTICE_SHOWN:-}" ] \
      || _kit_notice "$(curl -sS -m 5 "$server/health" 2>/dev/null | _jq_raw '.version // empty' 2>/dev/null || true)"
    ;;

  owner)
    # Who this agent's user is (references/authority.md): their exact Loca
    # @name. Only that person, verified by the server, asks for what is the
    # user's own (level 3); everyone at the table works on the loca's
    # project. Shown, set, or cleared here; never a credential.
    server="$1"; name="${2:?name required}"; user="${3:-}"
    if [ ! -f "$LOCA_ENV" ] || [ "$(_credential_value "$LOCA_ENV" LOCA_NAME || true)" != "$name" ]; then
      echo "no local identity for '$name' — set its user after setup or request-join" >&2
      exit 3
    fi
    if [ -z "$user" ]; then
      _owner_line
      exit 0
    fi
    if [ "$user" = "--clear" ]; then
      printf '%s\0%s\0' LOCA_OWNER "" | _credential_patch
      LOCA_OWNER=""
      _owner_line
      exit 0
    fi
    user="${user#@}"
    if ! [[ "$user" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
      echo "a user is a Loca @name: 1-64 ASCII letters, digits, dot, dash, or underscore, starting with a letter or digit" >&2
      exit 2
    fi
    if [ "$user" = "$name" ]; then
      echo "an agent is not its own user" >&2
      exit 2
    fi
    printf '%s\0%s\0' LOCA_OWNER "$user" | _credential_patch
    LOCA_OWNER="$user"
    _owner_line
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
    curl_get "$server/rooms/$(_room_path "$room")/members"
    ;;

  since)
    server="$1"; room="$2"; since="${3:-0}"
    use_loca "$room"
    curl_get "$server/rooms/$(_room_path "$room")/messages?since=$since"
    ;;

  wiki|wiki-review|wiki-remove|wiki-page|wiki-claim|wiki-finish|wiki-renew|wiki-context|wiki-requests|wiki-ready|wiki-request|wiki-accept|wiki-offer)
    # shellcheck disable=SC1091 # connect.d/wiki.sh is checked on its own
    . "$SCRIPT_DIR/connect.d/wiki.sh"
    ;;

  memory|memory-short|memory-append|memory-entries|notes|note-get|note-create|note-update|journal|vault|context|lessons|lesson-propose)
    # shellcheck disable=SC1091 # connect.d/content.sh is checked on its own
    . "$SCRIPT_DIR/connect.d/content.sh"
    ;;
  mentats|mentat|mentat-ask|mentat-join|mentat-withdraw|mentat-end|mentat-merge|mentat-pause|mentat-resume)
    # shellcheck disable=SC1091 # connect.d/mentat.sh is checked on its own
    . "$SCRIPT_DIR/connect.d/mentat.sh"
    ;;

  goals|brief|tasks|task|task-take|task-done|task-new|task-assign|task-go|task-nogo|attentions|attention-claim|attention-resolve|waits|wait|wait-clear)
    # shellcheck disable=SC1091 # connect.d/work.sh is checked on its own
    . "$SCRIPT_DIR/connect.d/work.sh"
    ;;

  replying)
    # replying <server> <room> <name> on|off — say this agent is (or no longer
    # is) writing a reply. Ephemeral: the room shows "<name> is typing…" for a
    # few seconds, so a runtime repeats "on" while its model turn runs. Prints
    # the HTTP status; a refusal is not an error worth retrying.
    server="$1"; room="$2"; name="$3"; state="${4:-on}"
    case "$state" in
      on) on=true ;;
      off) on=false ;;
      *) echo >&2 "replying: the last argument is on or off"; exit 2 ;;
    esac
    use_loca "$room" write
    code=$(_curl -sS -m 5 -o /dev/null -w '%{http_code}' -H 'content-type: application/json' \
      "${tok_args[@]}" -X POST "$server/rooms/$(_room_path "$room")/typing" -d "{\"on\":$on}") || code=000
    printf '%s\n' "$code"
    [[ "$code" =~ ^2[0-9][0-9]$ ]] || exit 8
    ;;

  announce)
    # announce <server> <room> <name> <text> — say something the loca must know
    server="$1"; room="$2"; name="$3"; shift 3
    use_loca "$room"
    text="$*"
    op_id="${LOCA_OP_ID:-announce-$name-$(date +%s%N)-$$-$RANDOM}"
    payload=$(jq -n --arg s "$name" --arg t "$text" --arg o "$op_id" \
      '{sender:$s, sender_type:"agent", target:"all", text:$t, kind:"announce", op_id:$o}')
    curl_json -X POST "$server/rooms/$(_room_path "$room")/messages" -d "$payload"
    ;;

  mode)
    # mode <server> <room>   -> current chat mode (who may talk right now)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$(_room_path "$room")/mode"
    ;;

  settings)
    # settings <server> <room>   -> rate limit etc. (how fast you may post)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$(_room_path "$room")/settings"
    ;;

  send)
    server="$1"; room="$2"; name="$3"; target="$4"; shift 4
    # Split the trailing args into text words and --attach <file> pairs. The
    # flag may appear anywhere; this ONE invocation shape is identical for
    # Claude Code and Codex (RFC: one path for both runtimes).
    attach_files=()
    text_parts=()
    while [ $# -gt 0 ]; do
      if [ "$1" = "--attach" ]; then
        shift
        # A dangling --attach (no path after it, e.g. it was the last arg) is a
        # user error, not an empty attach — fail loudly rather than silently
        # sending an attachment-less message.
        if [ -z "${1:-}" ]; then
          echo >&2 "send: --attach requires a file path"
          exit 2
        fi
        attach_files+=("$1")
        shift
      else
        text_parts+=("$1")
        shift
      fi
    done
    text="${text_parts[*]}"
    # Validate attachments on the CLIENT before any upload, so a bad request
    # never leaves an orphan pending blob on the server.
    if [ "${#attach_files[@]}" -gt 0 ]; then
      if [ "${#attach_files[@]}" -gt 4 ]; then
        echo >&2 "send: at most 4 attachments per message (got ${#attach_files[@]})"
        exit 2
      fi
      for f in "${attach_files[@]}"; do
        # The filename rides in the x-filename header; a CR/LF or other control
        # char is a header-injection vector. Reject at the client boundary (the
        # server also sanitizes the stored name, but we never emit a bad header).
        case "$(basename -- "$f")" in
          *[[:cntrl:]]*)
            echo >&2 "send: attachment filename has control characters: $f"
            exit 2 ;;
        esac
      done
    fi
    use_loca "$room" write
    # Upload each attachment first, collecting its server-issued id (== sha256).
    # A failed upload aborts the send — we never post a message citing a file
    # that isn't stored.
    attach_ids=()
    if [ "${#attach_files[@]}" -gt 0 ]; then
      for f in "${attach_files[@]}"; do
        aid=$(_upload_attachment "$room" "$f" "$name") || exit 6
        attach_ids+=("$aid")
      done
    fi
    op_id="${LOCA_OP_ID:-send-$name-$(date +%s%N)-$$-$RANDOM}"
    reply_to="${LOCA_REPLY_TO:-}"
    if [ "$target" = "-" ] || [ -z "$target" ]; then
      body=$(jq -n --arg s "$name" --arg t "$text" --arg o "$op_id" \
        '{sender:$s, sender_type:"agent", text:$t, op_id:$o}')
    else
      body=$(jq -n --arg s "$name" --arg g "$target" --arg t "$text" --arg o "$op_id" \
        '{sender:$s, sender_type:"agent", target:$g, text:$t, op_id:$o}')
    fi
    if [ "${#attach_ids[@]}" -gt 0 ]; then
      ids_json=$(printf '%s\n' "${attach_ids[@]}" | jq -R . | jq -s .)
      body=$(printf '%s' "$body" | jq --argjson a "$ids_json" '. + {attachments:$a}')
    fi
    if [[ "$reply_to" =~ ^[0-9]+$ ]]; then
      body=$(printf '%s' "$body" | jq --argjson reply_to "$reply_to" \
        '. + {reply_to:$reply_to}')
    fi
    # Append the HTTP status so the caller sees a 403 (mode-gated) rejection.
    resp=$(_curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" -w $'\n%{http_code}' \
      -X POST "$server/rooms/$(_room_path "$room")/messages" -d "$body")
    code=$(printf '%s' "$resp" | tail -n1)
    # Sessions live in the server's memory: a restart invalidates them. A lobby
    # call also delivers a fresh INVITE but deliberately no session. In both
    # cases mint one from THIS loca's invite and retry once. Requiring an old
    # LOCA_SESSION here deadlocked every first post after lobby -> call:
    # the server required a session, while the client refused to mint one
    # because it did not already have one.
    renew_key=$(invite_for "$room")
    if [ "$code" = "401" ] && [ -n "$renew_key" ]; then
      # Renew as whoever this call is for — not as whatever LOCA_NAME happens to
      # hold. On a box running two agents, an inherited LOCA_NAME from the other
      # one silently mints a session under their name.
      renew_as="${LOCA_NAME:-$name}"
      [ -n "$name" ] && renew_as="$name"
      renew_resp=$(_curl -sS -m 10 -H 'content-type: application/json' -H "x-room-token: $renew_key" \
        -w $'\n%{http_code}' \
        -X POST "$server/sessions" -d "$(_session_request "$renew_as" "$room")" \
      )
      renew_code=$(printf '%s' "$renew_resp" | tail -n1)
      renew_body=$(printf '%s' "$renew_resp" | sed '$d')
      new=""
      if [ "$renew_code" = "201" ]; then
        new=$(printf '%s' "$renew_body" | _jq_raw '.session_token // empty' 2>/dev/null || true)
      elif [ "$room" = "lobby" ]; then
        echo >&2 "session renewal rejected (HTTP $renew_code): the Building refused this membership key for the Lobby room"
      else
        echo >&2 "session renewal rejected (HTTP $renew_code): reconnect through Lobby to refresh this loca's invite"
      fi
      if [ -n "$new" ]; then
        _keep_session "$room" "$new"
        # Many things end a session (its 12 hours, a newer session for the
        # same key, a restart, a first post after a call); do not guess which.
        echo >&2 "session renewed"
        tok_args=(-H "x-room-token: $renew_key" -H "x-session-token: $new")
        resp=$(_curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" -w $'\n%{http_code}' \
          -X POST "$server/rooms/$(_room_path "$room")/messages" -d "$body")
        code=$(printf '%s' "$resp" | tail -n1)
      fi
    fi
    printf '%s' "$resp" | sed '$d'
    if [ "$code" = "403" ]; then
      # The server's sentence says which refusal it is (paused, restricted,
      # round-robin, muted, banned, closed, suspended, extension off).
      said=$(printf '%s' "$resp" | sed '$d' | tr '\r\n' '  ')
      said="${said:0:300}"
      echo >&2 "REJECTED (403): ${said:-no reason given} — not posted; do not loop-retry. For a chat mode, run 'mode' to see who may talk."
      exit 4
    elif [ "$code" = "429" ]; then
      echo >&2 "RATE-LIMITED (429): you are posting too fast. Back off; do NOT loop-retry."
      exit 5
    elif [ "$code" = "401" ]; then
      echo >&2 "UNAUTHORIZED (401): stale session/invite; reconnect through Lobby before retrying"
      exit 7
    elif [ "$code" = "409" ]; then
      # The server cannot prove this op_id's earlier use was ours; the same
      # op_id will never post. Only a new operation can.
      echo >&2 "NOT POSTED (409): the server cannot tell whether this operation id was already used by you. Read recent messages with 'since'; if your words are not there, send them again with a new LOCA_OP_ID"
      exit 8
    elif ! [[ "$code" =~ ^2[0-9][0-9]$ ]]; then
      echo >&2 "SEND FAILED (HTTP $code): message was not accepted; retain the same operation id for a retry"
      exit 8
    fi
    response_body=$(printf '%s' "$resp" | sed '$d')
    if ! printf '%s' "$response_body" | jq -e --arg room "$room" --arg sender "$name" \
      'type == "object" and (.id | type == "number" and . > 0 and . == floor) and .room == $room and .sender == $sender' >/dev/null 2>&1; then
      echo >&2 "SEND UNVERIFIED: server did not return the accepted message identity; retain the same operation id"
      exit 8
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
    # release <server> <room> <name> — "my work is done". End THIS identity's
    # invite/seat without punishment; building membership remains in lobby.
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    code=$(curl -sS -o /dev/null -w '%{http_code}' -m 10 \
      -H 'content-type: application/json' "${tok_args[@]}" \
      -X POST "$server/rooms/$(_room_path "$room")/release")
    if [ "$code" != "204" ]; then
      echo "release failed (HTTP $code): your own invite/session for '$room' is required" >&2
      exit 7
    fi
    # The server ended both invite and session. Remove their stale local copies
    # so the next command cannot keep presenting a dead credential.
    if [ -f "$LOCA_ENV" ]; then
      {
        printf 'DAVET_%s\0\0' "$(_var "$room")"
        printf 'LOCA_SESSION\0\0'
        printf 'LOCA_SESSION_LOCA\0\0'
      } | _credential_patch
    fi
    unset LOCA_SESSION LOCA_SESSION_LOCA
    printf "released '%s' from '%s'; building membership stays in lobby\n" "$name" "$room"
    ;;

  listen)
    server="$1"; room="$2"; name="$3"
    log="${ROOM_LOG:-.agent-room/$room.jsonl}"
    mkdir -p "$(dirname "$log")"
    ws=$(printf '%s' "$server" | sed -E 's#^http#ws#')
    # filter=msg -> server sends only real chat messages (no typing/members noise).
    url="$ws/ws?room=$(_room_path "$room")&name=$(_room_path "$name")&type=agent&filter=msg"
    # Keep credentials out of argv/process listings. listen.py selects the
    # name-specific env, validates its server origin, then adds the loca's invite
    # and session internally.
    echo "listening on $server room=$room name=$name -> $log" >&2
    exec python3 "$SCRIPT_DIR/listen.py" "$url" "$log"
    ;;

  setup|request-join|first-reply)
    # shellcheck disable=SC1091 # connect.d/onboarding.sh is checked on its own
    . "$SCRIPT_DIR/connect.d/onboarding.sh"
    ;;

  stop)
    # stop <server> <name> [--kill|--dry-run] — stop ONLY this identity's
    # listener/Monitor on this server. NEVER a broad pattern kill: a
    # `pkill -f listen.py` would take out every agent's listener on the machine
    # (and a pattern can even match the caller's own shell — that is how a smoke
    # cleanup once killed its own process). This targets only processes whose ws
    # URL names EXACTLY <name> on <server>'s host, and re-checks each PID's kernel
    # start-time immediately before signalling, so a recycled PID can never make
    # us kill a bystander. Every other identity's listener is left untouched.
    server="${1:-${ROOM_SERVER_URL:-}}"
    name="${2:-${LOCA_NAME:-}}"
    opt="${3:-}"
    if [ -z "$server" ] || [ -z "$name" ]; then
      echo "usage: connect.sh stop <server> <name> [--kill|--dry-run]" >&2
      exit 2
    fi
    if ! [[ "$server" =~ ^https?://([A-Za-z0-9._-]+|\[[0-9A-Fa-f:]+\])(:[0-9]+)?$ ]]; then
      echo "server must be an http(s) origin without a path or shell syntax" >&2
      exit 2
    fi
    if ! [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
      echo "name must be 1-64 ASCII letters, digits, dot, dash, or underscore, starting with a letter or digit" >&2
      exit 2
    fi
    host=$(printf '%s' "$server" | sed -E 's#^https?://##; s#/.*##')
    extra=()
    case "$opt" in
      "") ;;
      --kill) extra=(--signal KILL) ;;
      --dry-run) extra=(--dry-run) ;;
      *) echo "unknown option: $opt (use --kill or --dry-run)" >&2; exit 2 ;;
    esac
    python3 "$SCRIPT_DIR/stop_listener.py" "$host" "$name" "${extra[@]}"
    exit $?
    ;;

  session)
    # session <server> <name> [agent|user]  -> prints a session token.
    # Usage:  export LOCA_SESSION=$(connect.sh session $SERVER myname agent)
    server="$1"; name="$2"; kind="${3:-agent}"
    payload=$(jq -n --arg n "$name" --arg k "$kind" '{name:$n, kind:$k}')
    curl_json -X POST "$server/sessions" -d "$payload" | _jq_raw '.session_token'
    ;;

  doctor)
    # shellcheck disable=SC1091 # connect.d/doctor.sh is checked on its own
    . "$SCRIPT_DIR/connect.d/doctor.sh"
    ;;

  *)
    echo "unknown command: $cmd (see header of $0)" >&2
    exit 2
    ;;
esac
