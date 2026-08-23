#!/usr/bin/env bash
# admit.sh — admit someone to the BUILDING. The founding act: it creates the
# identity every davet later refers to. Heavy, rare, done from a terminal.
#
#   ./scripts/admit.sh <name> [agent|user]    admit to the building
#   ./scripts/admit.sh --list                 who belongs
#   ./scripts/admit.sh --revoke <mb_token>    end a membership
#
# Admitting gives no seat anywhere. To seat them: invite.sh <loca> <name>.
#
# Server:  $LOCA_SERVER (default http://127.0.0.1:8787)
# Master:  $ADMIN_TOKEN, else /opt/loca/.env, else ./.env

set -euo pipefail

SERVER="${LOCA_SERVER:-http://127.0.0.1:8787}"

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
    api "$SERVER/members" \
      | jq -r '.[] | "\(.name)\t\(.kind)\t\(.admitted_by)\t\(.id)"' \
      | { printf 'who\tkind\tadmitted by\tmembership\n'; cat; } | column -t -s $'\t'
    ;;

  --revoke)
    reference="${2:?usage: admit.sh --revoke <membership_id>}"
    api -X DELETE "$SERVER/members/$reference"
    echo
    ;;

  ""|-h|--help)
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    ;;

  *)
    name="$1"
    kind="${2:-agent}"
    payload=$(jq -n --arg n "$name" --arg k "$kind" '{name:$n, kind:$k}')
    resp=$(api -X POST -H 'content-type: application/json' -d "$payload" "$SERVER/members")
    token=$(printf '%s' "$resp" | jq -r '.token // empty')
    if [ -z "$token" ]; then
      echo "could not admit: $resp" >&2
      exit 1
    fi
    printf '\n  %s now belongs to the building (%s…).\n\n' "$name" "${token:0:12}"
    printf '  No seat yet — to call them into a loca:\n\n    invite.sh <loca> %s\n\n' "$name"
    ;;
esac
