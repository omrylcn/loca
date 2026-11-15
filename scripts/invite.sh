#!/usr/bin/env bash
# invite.sh — issue an invite: the right to enter ONE loca, and no other.
#
# Only the master runs this. It needs the admin token, and the admin token
# comes from the building itself (.env) — nobody hands it out, nobody mints it.
#
#   ./scripts/invite.sh <loca> <name> [agent|user]    issue an invite (member only)
#   ./scripts/invite.sh --admit <loca> <name> [kind]  admit to building, THEN invite
#   ./scripts/invite.sh --list <loca>                 who holds one
#   ./scripts/invite.sh --revoke <loca> <invite_id>    end an invite (ivid_…, from --list)
#   ./scripts/invite.sh --reissue <loca> <name>       re-send: a fresh invite for
#                                                     the seat they already hold
#
# An invite seats an EXISTING member — it never creates identity. For someone
# new, --admit runs the two acts openly, in order: admit, then invite.
#
# --reissue ends the member's current invite for that loca and issues a fresh
# one in its place; the seat stays theirs. The fresh invite also goes out over
# their Lobby line, so a listening agent moves to it without anyone handing it
# over.
#
# Server:  $LOCA_SERVER (default http://127.0.0.1:8787)
# Master:  $ADMIN_TOKEN, else /opt/loca/.env, else ./.env
#
# Never paste an invite into a room. Hand it over privately — rooms are
# permanent and every member reads them.

set -euo pipefail

SERVER="${LOCA_SERVER:-http://127.0.0.1:8787}"

# Admin token from the environment, else from a known .env. Never passed on the
# command line, so it stays out of `ps` output and shell history.
if [ -z "${ADMIN_TOKEN:-}" ]; then
  for f in /opt/loca/.env ./.env; do
    [ -f "$f" ] || continue
    ADMIN_TOKEN=$(sed -n 's/^ADMIN_TOKEN=//p' "$f" | tr -d '"' | head -1)
    [ -n "$ADMIN_TOKEN" ] && break
  done
fi
if [ -z "${ADMIN_TOKEN:-}" ]; then
  echo "no master key found." >&2
  echo "  export ADMIN_TOKEN=... or define ADMIN_TOKEN in .env" >&2
  exit 1
fi

api() { curl -sS -m 10 -H "x-admin-token: $ADMIN_TOKEN" "$@"; }

case "${1:-}" in
  --list)
    loca="${2:?usage: invite.sh --list <loca>}"
    api "$SERVER/rooms/$loca/invites" \
      | jq -r '.[] | "\(.name)\t\(.kind)\t\(.id)"' \
      | { printf 'who\tkind\tdavet\n'; cat; } | column -t -s $'\t'
    ;;

  --revoke)
    loca="${2:?usage: invite.sh --revoke <loca> <invite_id>}"
    reference="${3:?invite management id required}"
    api -X DELETE "$SERVER/rooms/$loca/invites/$reference"
    echo
    ;;

  --reissue)
    loca="${2:?usage: invite.sh --reissue <loca> <name>}"
    name="${3:?name required}"
    payload=$(jq -n --arg n "$name" '{name:$n, reissue:true}')
    resp=$(api -X POST -H 'content-type: application/json' -d "$payload" "$SERVER/rooms/$loca/call")
    # An older server ignores the flag and answers as to a call.
    if [ "$(printf '%s' "$resp" | jq -r '.already // empty' 2>/dev/null || true)" = "true" ]; then
      echo "this server cannot re-send an invite yet; $name's invite was not changed." >&2
      exit 1
    fi
    token=$(printf '%s' "$resp" | jq -r '.token // empty' 2>/dev/null || true)
    if [ -z "$token" ]; then
      echo "could not re-send: $resp" >&2
      exit 1
    fi
    printf '\n  %s has a fresh invite for the "%s" loca; the previous one no longer works.\n\n  %s\n\n' "$name" "$loca" "$token"
    printf '  It was also sent over their Lobby line: a listening agent moves to it by itself.\n'
    printf '  Otherwise hand it over PRIVATELY — never into a room.\n\n'
    ;;

  --admit)
    # The open two-step for an outsider: admit (the founding act, its own
    # record), then invite. Two requests, two audit events — never one hiding
    # inside the other.
    loca="${2:?usage: invite.sh --admit <loca> <name> [agent|user]}"
    name="${3:?name required}"
    kind="${4:-agent}"
    payload=$(jq -n --arg n "$name" --arg k "$kind" '{name:$n, kind:$k}')
    m=$(api -X POST -H 'content-type: application/json' -d "$payload" "$SERVER/members")
    mtok=$(printf '%s' "$m" | jq -r '.token // empty' 2>/dev/null || true)
    if [ "$(printf '%s' "$m" | jq -r '.already // false' 2>/dev/null || true)" = "true" ]; then
      printf '  1) %s already belongs to the building\n' "$name"
    elif [ -n "$mtok" ]; then
      printf '  1) %s admitted to the building (%s…)\n' "$name" "${mtok:0:10}"
    else
      echo "could not admit $name: $m" >&2
      exit 1
    fi
    exec "$0" "$loca" "$name" "$kind"
    ;;

  ""|-h|--help)
    sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
    ;;

  *)
    loca="$1"
    name="${2:?usage: invite.sh <loca> <name> [agent|user]}"
    kind="${3:-agent}"
    payload=$(jq -n --arg n "$name" --arg k "$kind" '{name:$n, kind:$k}')
    resp=$(api -X POST -H 'content-type: application/json' -d "$payload" "$SERVER/rooms/$loca/invites")
    token=$(printf '%s' "$resp" | jq -r '.token // empty')
    if [ -z "$token" ]; then
      case "$resp" in
        *"does not belong"*)
          echo "not a building member — an invite seats an existing member." >&2
          echo "  admit them first: $0 --admit $loca $name $kind" >&2
          ;;
        *) echo "could not issue an invite: $resp" >&2 ;;
      esac
      exit 1
    fi
    printf '\n  %s is invited to the "%s" loca.\n\n  %s\n\n' "$name" "$loca" "$token"
    printf '  Hand this over PRIVATELY — never into a room.\n'
    printf '  They then run:\n\n    connect.sh setup %s %s\n\n' "$SERVER" "$name"
    printf '  and paste it when asked. It opens "%s" and nothing else.\n' "$loca"
    printf '  To end it, list invites and use its management id: %s --revoke %s <invite_id>\n\n' "$0" "$loca"
    ;;
esac
