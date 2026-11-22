# shellcheck shell=bash disable=SC2154,SC2034
# Work: goals, tasks and the Loca Board, attentions and waits.
# Sourced by connect.sh for these commands only (variables are shared with
# it, hence SC2154/SC2034 off): the branches below are
# the ones connect.sh's own case held, run with its variables, functions
# and positional parameters.

case "$cmd" in
  goals)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$(_room_path "$room")/goals"
    ;;

  # ---- tasks (the Loca Board, `board`): declared work, read and moved by its holder ----
  tasks)
    # tasks <server> <room>                 -> every task, as the server lists them
    # tasks <server> <room> --mine <name>   -> the unfinished tasks given to <name>
    server="$1"; room="$2"; mine=""
    if [ "${3:-}" = "--mine" ]; then
      mine="${4:-}"
      [ -n "$mine" ] || { echo "tasks: --mine needs your name" >&2; exit 2; }
    elif [ -n "${3:-}" ]; then
      echo "tasks: unexpected argument '$3' (use --mine <name>)" >&2; exit 2
    fi
    use_loca "$room"
    list=$(curl_get "$server/rooms/$(_room_path "$room")/tasks") || exit $?
    if [ -z "$mine" ]; then
      printf '%s' "$list"
    else
      # Names are compared without letter case. The server, not this filter,
      # decides who may move a task: it follows the person it was given to.
      printf '%s' "$list" | python3 -c '
import json, sys
mine = sys.argv[1].casefold()
tasks = [t for t in json.load(sys.stdin)
         if str(t.get("assigned_to") or "").casefold() == mine
         and t.get("status") in ("open", "taken", "review")]
print(json.dumps(tasks, ensure_ascii=False))' "$mine"
    fi
    ;;

  task)
    server="$1"; room="$2"; id="${3:-}"
    [[ "$id" =~ ^[0-9]+$ ]] || { echo "task: give the task number" >&2; exit 2; }
    use_loca "$room"
    list=$(curl_get "$server/rooms/$(_room_path "$room")/tasks") || exit $?
    _task_by_id "$list" "$id" || exit $?
    ;;

  task-take|task-done)
    server="$1"; room="$2"; name="$3"; id="${4:-}"
    [[ "$id" =~ ^[0-9]+$ ]] || { echo "$cmd: give the task number" >&2; exit 2; }
    use_loca "$room"
    if [ "$cmd" = task-take ]; then
      # Claim it only when nobody holds it (as the web's Start button does);
      # otherwise ask to start it and let the server say whose it is.
      list=$(curl_get "$server/rooms/$(_room_path "$room")/tasks") || exit $?
      task=$(_task_by_id "$list" "$id") || exit $?
      if [ -z "$(printf '%s' "$task" | _jq_raw '.assigned_to // empty')" ]; then
        payload=$(jq -n --arg by "$name" '{status:"taken", assigned_to:$by, by:$by}')
      else
        payload=$(jq -n --arg by "$name" '{status:"taken", by:$by}')
      fi
    else
      # On the Loca Board the server takes "done" from its holder as a request
      # for review; the note says what changed and how to check it.
      payload=$(jq -n --arg by "$name" --arg note "${5:-}" \
        '{status:"done", by:$by} + (if $note == "" then {} else {note:$note} end)')
    fi
    _task_write -X PATCH "$server/rooms/$(_room_path "$room")/tasks/$id" -d "$payload" || exit $?
    ;;

  # The Lead's two task acts (the Operator may use them too). The server
  # decides who may: anyone else gets 403 and nothing changes.
  task-new)
    # task-new <server> <room> <name> "<title>" [--assign <who>]
    server="$1"; room="$2"; name="$3"; title="${4:-}"; assignee=""
    [ -n "${title//[[:space:]]/}" ] || { echo "task-new: give the task's title" >&2; exit 2; }
    if [ "${5:-}" = "--assign" ]; then
      assignee="${6:-}"
      [ -n "$assignee" ] || { echo "task-new: --assign needs a name" >&2; exit 2; }
      [ -z "${7:-}" ] || { echo "task-new: unexpected argument '$7'" >&2; exit 2; }
    elif [ -n "${5:-}" ]; then
      echo "task-new: unexpected argument '$5' (use --assign <who>)" >&2; exit 2
    fi
    use_loca "$room"
    payload=$(jq -n --arg by "$name" --arg title "$title" --arg who "$assignee" \
      '{title:$title, by:$by} + (if $who == "" then {} else {assigned_to:$who} end)')
    _task_write -X POST "$server/rooms/$(_room_path "$room")/tasks" -d "$payload" || exit $?
    ;;

  task-assign)
    # task-assign <server> <room> <name> <id> <who>
    server="$1"; room="$2"; name="$3"; id="${4:-}"; assignee="${5:-}"
    [[ "$id" =~ ^[0-9]+$ ]] || { echo "task-assign: give the task number" >&2; exit 2; }
    [ -n "$assignee" ] || { echo "task-assign: give who the task is for" >&2; exit 2; }
    use_loca "$room"
    payload=$(jq -n --arg by "$name" --arg who "$assignee" '{assigned_to:$who, by:$by}')
    _task_write -X PATCH "$server/rooms/$(_room_path "$room")/tasks/$id" -d "$payload" || exit $?
    ;;

  # The Loca Board's review: go (review -> done) or no-go with a reason
  # (review -> taken). The Lead (never on its own task) or the Operator.
  task-go|task-nogo)
    server="$1"; room="$2"; name="$3"; id="${4:-}"; reason="${5:-}"
    [[ "$id" =~ ^[0-9]+$ ]] || { echo "$cmd: give the task number" >&2; exit 2; }
    if [ "$cmd" = task-go ]; then
      payload=$(jq -n --arg by "$name" '{status:"done", by:$by}')
    else
      [ -n "${reason//[[:space:]]/}" ] || { echo "task-nogo: give the reason" >&2; exit 2; }
      payload=$(jq -n --arg by "$name" --arg reason "$reason" '{status:"taken", reason:$reason, by:$by}')
    fi
    use_loca "$room"
    _task_write -X PATCH "$server/rooms/$(_room_path "$room")/tasks/$id" -d "$payload" || exit $?
    ;;

  attentions)
    if [ "${LOCA_INTERNAL_ATTENTION:-0}" != "1" ]; then
      echo "internal attention ledger is not a product workflow; set LOCA_INTERNAL_ATTENTION=1 only for compatibility diagnostics" >&2
      exit 64
    fi
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$(_room_path "$room")/attentions"
    ;;

  attention-claim)
    if [ "${LOCA_INTERNAL_ATTENTION:-0}" != "1" ]; then
      echo "internal attention lifecycle is adapter-owned; set LOCA_INTERNAL_ATTENTION=1 only for compatibility diagnostics" >&2
      exit 64
    fi
    server="$1"; room="$2"; name="$3"; attention_id="$4"
    use_loca "$room"
    payload=$(jq -n --arg by "$name" '{by:$by}')
    encoded_id=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$attention_id")
    curl_json -X POST "$server/rooms/$(_room_path "$room")/attentions/$encoded_id/claim" -d "$payload"
    ;;

  attention-resolve)
    server="$1"; room="$2"; name="$3"; attention_id="$4"
    use_loca "$room"
    payload=$(jq -n --arg by "$name" '{by:$by}')
    encoded_id=$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$attention_id")
    curl_json -X POST "$server/rooms/$(_room_path "$room")/attentions/$encoded_id/resolve" -d "$payload"
    ;;

  brief)
    # brief <server> <room> -> the loca's state in a few lines, for its Lead
    # or operator to report from: the Goal, what waits for a decision, what
    # is in progress, who is blocked on whom, what landed. A section whose
    # extension is off says so; nothing here changes anything.
    server="$1"; room="$2"
    use_loca "$room"
    brief_dir=$(mktemp -d)
    trap 'rm -rf "${brief_dir:?}"' EXIT
    for part in tasks waits goals journal; do
      curl_get "$server/rooms/$(_room_path "$room")/$part" >"$brief_dir/$part.json" 2>/dev/null \
        || : >"$brief_dir/$part.json"
    done
    python3 "$SCRIPT_DIR/brief.py" "$room" "$brief_dir/tasks.json" "$brief_dir/waits.json" \
      "$brief_dir/goals.json" "$brief_dir/journal.json"
    ;;

  waits)
    server="$1"; room="$2"
    use_loca "$room"
    curl_get "$server/rooms/$(_room_path "$room")/waits"
    ;;

  wait)
    # Explicit state, never inferred from ordinary conversation.
    server="$1"; room="$2"; name="$3"; waiting_for="$4"; shift 4
    reason="$*"
    use_loca "$room"
    payload=$(jq -n --arg by "$name" --arg who "$waiting_for" --arg why "$reason" \
      '{by:$by, waiting_for:$who, reason:$why}')
    curl_json -X POST "$server/rooms/$(_room_path "$room")/waits" -d "$payload"
    ;;

  wait-clear)
    server="$1"; room="$2"; name="$3"
    use_loca "$room"
    payload=$(jq -n --arg by "$name" '{by:$by}')
    curl_json -X DELETE "$server/rooms/$(_room_path "$room")/waits/$name" -d "$payload"
    ;;
esac
