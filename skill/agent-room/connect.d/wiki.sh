# shellcheck shell=bash disable=SC2154,SC2034
# Loca Wiki commands: reading pages, reviews, maintenance leases and context requests.
# Sourced by connect.sh for these commands only (variables are shared with
# it, hence SC2154/SC2034 off): the branches below are
# the ones connect.sh's own case held, run with its variables, functions
# and positional parameters.

case "$cmd" in
  # ---- Loca Wiki ----
  wiki)
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    _wiki_request "$server/rooms/$(_room_path "$room")/wiki"
    ;;

  wiki-review)
    server="$1"; room="$2"; name="$3"; review_file="$4"
    use_loca "$room"
    # Review content is data, not a shell expression. A local file avoids a
    # large quoted payload on argv and leaves credentials in their boundary.
    payload=$(jq -ce 'select(type == "object" and (.pages | type == "array"))' "$review_file") || exit 2
    _wiki_request -X POST "$server/rooms/$(_room_path "$room")/wiki/review" -d "$payload"
    ;;

  wiki-remove)
    server="$1"; room="$2"; name="$3"; slug="$4"; removal_file="$5"
    use_loca "$room"
    if [[ ! "$slug" =~ ^[a-z0-9-]{1,80}$ ]]; then
      echo "Invalid Wiki page slug" >&2; exit 2
    fi
    payload=$(jq -ce 'select(type == "object" and (.expected_revision | type == "number") and (.reason | type == "string"))' "$removal_file") || exit 2
    _wiki_request -X DELETE "$server/rooms/$(_room_path "$room")/wiki/pages/$slug" -d "$payload"
    ;;

  wiki-page)
    server="$1"; room="$2"; name="$3"; slug="$4"
    use_loca "$room"
    if [[ ! "$slug" =~ ^[a-z0-9-]{1,80}$ ]]; then
      echo "Invalid Wiki page slug" >&2; exit 2
    fi
    snapshot=$(_wiki_request "$server/rooms/$(_room_path "$room")/wiki/pages/$slug") || exit $?
    printf '%s' "$snapshot" | jq -ce --arg room "$room" --arg slug "$slug" '
      select(.room == $room and .page.slug == $slug)' || {
        echo "Wiki page unavailable; no context was supplied" >&2; exit 2;
      }
    ;;

  wiki-claim)
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    _wiki_request -X POST "$server/rooms/$(_room_path "$room")/wiki/maintenance/claim" -d '{}'
    ;;

  wiki-finish)
    server="$1"; room="$2"; name="$3"; review_file="$4"
    use_loca "$room"
    payload=$(jq -ce 'select(type == "object" and (.lease | type == "object") and (.review.pages | type == "array"))' "$review_file") || exit 2
    _wiki_request -X POST "$server/rooms/$(_room_path "$room")/wiki/maintenance/finish" -d "$payload"
    ;;

  wiki-renew)
    server="$1"; room="$2"; name="$3"; lease_file="$4"
    use_loca "$room"
    payload=$(jq -ce 'select(type == "object" and (.batch_id | type == "number") and (.epoch | type == "number"))' "$lease_file") || exit 2
    _wiki_request -X POST "$server/rooms/$(_room_path "$room")/wiki/maintenance/renew" -d "$payload"
    ;;

  wiki-context)
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    _wiki_request "$server/rooms/$(_room_path "$room")/wiki/context"
    ;;

  wiki-requests)
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    _wiki_request "$server/rooms/$(_room_path "$room")/wiki/context/requests"
    ;;

  wiki-ready|wiki-request|wiki-accept|wiki-offer)
    server="$1"; room="$2"; name="$3"; json_file="$4"
    use_loca "$room"
    payload=$(jq -ce 'select(type == "object")' "$json_file") || exit 2
    case "$cmd" in
      wiki-ready) wiki_path=runtime ;;
      wiki-request) wiki_path=context ;;
      wiki-accept) wiki_path=context/accept ;;
      wiki-offer) wiki_path=context/offer ;;
    esac
    _wiki_request -X POST "$server/rooms/$(_room_path "$room")/wiki/$wiki_path" -d "$payload"
    ;;
esac
