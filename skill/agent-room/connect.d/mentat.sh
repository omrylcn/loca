# shellcheck shell=bash disable=SC2154,SC2034
# Mentat rooms: the Building's directory of them, a room's line, asking,
# joining or withdrawing a request, ending the consultation inside, and the
# mentat's own acts (merge two requests, pause or resume its line). Sourced
# by connect.sh for these commands only (variables are shared with it, hence
# SC2154/SC2034 off).
#
# A mentat room is a loca founded as the place where one expert agent, the
# mentat, is consulted in turn. The line orders access to that expert: it
# assigns no work and creates no task. Your turn comes as a line in the room
# that names you with @name; until then you may not write there.

# POST to the mentat line as $name; print the receipt or the refusal.
# $1 = path under /rooms/<room>/mentat, $2 = JSON body ('' for none),
# $3 = the HTTP status of success, $4 = what failed (for the message).
_mentat_post() {
  local path="$1" body="$2" ok="$3" what="$4" resp code said
  if [ -n "$body" ]; then
    resp=$(_curl -sS -m 10 -H 'content-type: application/json' "${tok_args[@]}" -w $'\n%{http_code}' \
      -X POST "$server/rooms/$(_room_path "$room")/mentat$path" -d "$body")
  else
    resp=$(_curl -sS -m 10 "${tok_args[@]}" -w $'\n%{http_code}' \
      -X POST "$server/rooms/$(_room_path "$room")/mentat$path")
  fi
  code=$(printf '%s' "$resp" | tail -n1)
  said=$(printf '%s' "$resp" | sed '$d')
  if [ "$code" != "$ok" ]; then
    echo >&2 "$what (HTTP $code): ${said:0:300}"
    case "$code" in 401) exit 7 ;; 403) exit 4 ;; 429) exit 5 ;; *) exit 8 ;; esac
  fi
  printf '%s' "$said"
}

# The request id argument, or stop.
_mentat_id() {
  [[ "${1:-}" =~ ^[0-9]+$ ]] || { echo >&2 "$cmd: give the request number"; exit 2; }
  printf '%s' "$1"
}

case "$cmd" in
  mentats)
    # mentats <server> [name] -> the Building's mentat rooms: room, topic,
    # mentat, whether it is connected, how long its line is. Read with the
    # membership, as the Lobby is.
    server="$1"
    use_loca lobby
    view=$(curl_get "$server/mentats") || exit $?
    printf '%s' "$view" | _jq_raw '
      if (.mentats | length) == 0 then "no mentat rooms in this Building"
      else .mentats[]
        | "\(.room): \(.topic) — "
          + (if .mentat then "mentat @\(.mentat) (\(if .online then "online" else "offline" end))"
             else "no mentat yet" end)
          + (if .paused then ", paused" else "" end)
          + ", \(.waiting) waiting" + (if .busy then ", one inside" else "" end)
      end'
    ;;

  mentat)
    # mentat <server> <room> -> the line: who the mentat is, who is inside,
    # who waits, and the limits the server enforces.
    server="$1"; room="$2"
    use_loca "$room"
    view=$(curl_get "$server/rooms/$(_room_path "$room")/mentat") || exit $?
    # $s below is jq's variable, not the shell's.
    # shellcheck disable=SC2016
    printf '%s' "$view" | _jq_raw '
      .state as $s
      | def who: (.owners | map("@" + .) | join(" "));
      "topic: \($s.mentat.topic)",
      (if $s.mentat.mentat then "mentat: @\($s.mentat.mentat)\(if $s.paused then " (paused: the line does not move)" else "" end)"
       else "mentat: none yet — nobody can ask until the Master or the Smaster who founded this room appoints one" end),
      "limits: \($s.mentat.limits.session_secs)s a consultation, \($s.mentat.limits.questions) questions (messages before an answer are one), closes after \($s.mentat.limits.idle_secs)s of silence; a called request has \($s.mentat.limits.call_secs)s to start",
      (if $s.current then
         "inside: #\($s.current.id) \($s.current | who) (\($s.current.status)\(if $s.current.closing then ", closing" else "" end), \($s.current.questions) questions): \($s.current.topic)"
       else "inside: nobody" end),
      (if ($s.waiting | length) == 0 then "waiting: nobody"
       else ($s.waiting[] | "waiting: #\(.id) \(who): \(.topic)") end)'
    ;;

  mentat-ask)
    # mentat-ask <server> <room> <name> <topic> -> ask to consult the mentat.
    # One open request per asker. Everyone seated reads the topic. When your
    # turn comes a line names you; then write your question in the room.
    server="$1"; room="$2"; name="$3"; shift 3
    topic="$*"
    use_loca "$room" write
    payload=$(jq -n --arg t "$topic" '{topic:$t}')
    receipt=$(_mentat_post "/requests" "$payload" 201 "NOT OPENED") || exit $?
    printf '%s' "$receipt" | _jq_raw '
      if .status == "called" then "request #\(.id): your turn now — write your question here"
      else "request #\(.id) is waiting in the line; a line naming you says when it is your turn" end'
    ;;

  mentat-join)
    # mentat-join <server> <room> <name> <request-id> -> go in with a waiting
    # request's askers instead of opening your own about the same thing. The
    # request's questions are shared by its askers.
    server="$1"; room="$2"; name="$3"; id=$(_mentat_id "${4:-}") || exit $?
    use_loca "$room" write
    receipt=$(_mentat_post "/requests/$id/join" "" 200 "NOT JOINED") || exit $?
    printf '%s' "$receipt" | _jq_raw '"joined request #\(.id): \(.owners | map("@" + .) | join(" "))"'
    ;;

  mentat-withdraw)
    # mentat-withdraw <server> <room> <name> <request-id> -> take your place
    # in a waiting request back; the last asker to leave withdraws it.
    server="$1"; room="$2"; name="$3"; id=$(_mentat_id "${4:-}") || exit $?
    use_loca "$room" write
    receipt=$(_mentat_post "/requests/$id/withdraw" "" 200 "NOT WITHDRAWN") || exit $?
    printf '%s' "$receipt" | _jq_raw '
      if .status == "withdrawn" then "request #\(.id) withdrawn" else "you left request #\(.id)" end'
    ;;

  mentat-end)
    # mentat-end <server> <room> <name> [--answered yes|partly|no] -> end the
    # consultation inside: its askers ("we are done", saying whether it was
    # answered) or the mentat ("done, next"). The next request is called.
    server="$1"; room="$2"; name="$3"; answered=""
    if [ "${4:-}" = "--answered" ]; then
      case "${5:-}" in yes|partly|no) answered="$5" ;; *) echo >&2 "mentat-end: --answered is yes, partly or no"; exit 2 ;; esac
    fi
    use_loca "$room" write
    if [ -n "$answered" ]; then
      receipt=$(_mentat_post "/end" "$(jq -n --arg a "$answered" '{answered:$a}')" 200 "NOT ENDED") || exit $?
    else
      receipt=$(_mentat_post "/end" "{}" 200 "NOT ENDED") || exit $?
    fi
    printf '%s' "$receipt" | _jq_raw '"consultation #\(.id) ended"'
    ;;

  mentat-merge)
    # mentat-merge <server> <room> <name> <from-id> <into-id> -> the mentat
    # joins waiting request <from-id> to <into-id>: its askers go in together.
    server="$1"; room="$2"; name="$3"
    from=$(_mentat_id "${4:-}") || exit $?; into=$(_mentat_id "${5:-}") || exit $?
    use_loca "$room" write
    receipt=$(_mentat_post "/requests/$from/merge" "$(jq -n --argjson i "$into" '{into:$i}')" 200 "NOT MERGED") || exit $?
    printf '%s' "$receipt" | _jq_raw '"request #\(.id) now: \(.owners | map("@" + .) | join(" "))"'
    ;;

  mentat-pause|mentat-resume)
    # mentat-pause|mentat-resume <server> <room> <name> -> the mentat freezes
    # its line (nobody is called, no clock runs) or lets it move again.
    server="$1"; room="$2"; name="$3"
    use_loca "$room" write
    what="${cmd#mentat-}"
    receipt=$(_mentat_post "/$what" "" 200 "NOT ${what^^}D") || exit $?
    printf '%s' "$receipt" | _jq_raw 'if .paused then "the line is paused" else "the line moves" end'
    ;;
esac
