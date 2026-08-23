#!/usr/bin/env bash
# invite.sh — issue a davet: the right to enter ONE loca, and no other.
#
# Only the master runs this. It needs the admin token, and the admin token
# comes from the building itself (.env) — nobody hands it out, nobody mints it.
#
#   ./scripts/invite.sh <loca> <name> [agent|user]    issue a davet (member only)
#   ./scripts/invite.sh --admit <loca> <name> [kind]  admit to building, THEN invite
#   ./scripts/invite.sh --list <loca>                 who holds one
#   ./scripts/invite.sh --revoke <loca> <token>       end a davet
#
# A davet seats an EXISTING member — it never creates identity. For someone
# new, --admit runs the two acts openly, in order: admit, then invite.
#
# Server:  $LOCA_SERVER (default http://127.0.0.1:8787)
# Master:  $ADMIN_TOKEN, else /opt/loca/.env, else ./.env
#
# Never paste a davet into a room. Hand it over privately — rooms are
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
    loca="${2:?usage: invite.sh --revoke <loca> <davet_id>}"
    reference="${3:?davet management id required}"
    api -X DELETE "$SERVER/rooms/$loca/invites/$reference"
    echo
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
    mtok=$(printf '%s' "$m" | jq -r '.token // empty')
    if [ -z "$mtok" ]; then
      echo "could not admit $name: $m" >&2
      exit 1
    fi
    printf '  1) %s admitted to the building (%s…)\n' "$name" "${mtok:0:10}"
    exec "$0" "$loca" "$name" "$kind"
    ;;

  ""|-h|--help)
    sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
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
          echo "not a building member — a davet seats an existing member." >&2
          echo "  admit them first: $0 --admit $loca $name $kind" >&2
          ;;
        *) echo "could not issue a davet: $resp" >&2 ;;
      esac
      exit 1
    fi
    printf '\n  %s is invited to the "%s" loca.\n\n  %s\n\n' "$name" "$loca" "$token"
    printf '  Hand this over PRIVATELY — never into a room.\n'
    printf '  They then run:\n\n    connect.sh setup %s %s\n\n' "$SERVER" "$name"
    printf '  and paste it when asked. It opens "%s" and nothing else.\n' "$loca"
    printf '  To end it, list davets and use its management id: %s --revoke %s <davet_id>\n\n' "$0" "$loca"
    ;;
esac
