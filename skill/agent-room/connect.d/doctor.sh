# shellcheck shell=bash disable=SC2154,SC2034
# `doctor`: the interactive report on this identity's credentials, listener and runtime.
# Sourced by connect.sh for these commands only (variables are shared with
# it, hence SC2154/SC2034 off): the branches below are
# the ones connect.sh's own case held, run with its variables, functions
# and positional parameters.

case "$cmd" in
  doctor)
    # doctor <server> [--fix]
    # Health check for the CLIENT side — the pains we actually hit: leaked
    # listener/bot processes, duplicate (room,name) connections shadowing each
    # other, a stale process squatting on the server port. --fix kills the
    # flagged duplicates (keeps the newest per room+name).
    # Every probe is best-effort: a failed probe is itself report data and must
    # not let global `set -e` truncate the remaining diagnostics.
    set +e
    server="${1:-http://127.0.0.1:8787}"; fix="${2:-}"
    echo "── server ──"
    if h=$(curl -sS -m 5 "$server/health" 2>/dev/null); then
      echo "  ok: $h"
      _kit_notice "$(printf '%s' "$h" | _jq_raw '.version // empty' 2>/dev/null)"
    else
      echo "  UNREACHABLE: $server"
      port=$(printf '%s' "$server" | sed -E 's#.*:([0-9]+).*#\1#')
      holder=$(ss -tlnp 2>/dev/null | grep ":$port " | head -1)
      [ -n "$holder" ] && echo "  port $port holder: $holder"
    fi
    # Are we set up for THIS server? The question every agent actually asks
    # before joining, and the one setup answers.
    echo "── identity ──"
    # Look where setup actually writes. With $LOCA_ENV pinned, that file is
    # the whole question: an agent checks its own identity and must not open
    # another identity's file or present its key. Without it (an operator
    # looking at the machine), every ~/.loca/*.env, because one machine can
    # host several agents (a prod box running cyber and debug side by side).
    found=0
    seen=""
    invited_names_file="/tmp/loca-doctor-invited.$$"
    listener_names_file="/tmp/loca-doctor-listeners.$$"
    : > "$invited_names_file"
    : > "$listener_names_file"
    if [ "$_LOCA_ENV_PINNED" -eq 1 ] && [ -n "${LOCA_ENV:-}" ]; then
      identity_files=("$LOCA_ENV")
    else
      identity_files=("$HOME"/.loca/env "$HOME"/.loca/*.env)
    fi
    for f in "${identity_files[@]}"; do
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
        identity=$(curl -sS -m 5 -H "x-room-token: $membership" -w $'\n%{http_code}' \
          "$server/whoami" 2>/dev/null || true)
        identity_code=$(printf '%s' "$identity" | tail -n1)
        identity=$(printf '%s' "$identity" | sed '$d')
        if [ "$identity_code" = "200" ] \
          && [ "$(printf '%s' "$identity" | _jq_raw '.kind // empty' 2>/dev/null)" = "member" ]; then
          # Only a complete loca list may decide what is stale: the field must
          # be there, be a list, and hold loca names. A missing or malformed
          # `locas` is not "seated nowhere"; reading it as an empty list would
          # let --fix delete every kept invite, and a server that keeps only
          # invite hashes cannot send them again.
          locas_complete=$(printf '%s' "$identity" | _jq_raw \
            'if (.locas | type) == "array" and (.locas | all(type == "string" and length > 0)) then "yes" else "no" end' \
            2>/dev/null || true)
          if [ "$locas_complete" != "yes" ]; then
            echo "  UNVERIFIED: '${env_name:-?}' — membership valid, but the server's loca list was missing or malformed; nothing was checked or removed ($f)"
            continue
          fi
          # A valid membership needs a live listener whether or not it is
          # seated: the Lobby connection is what a call reaches.
          [ -z "$env_name" ] || printf '%s\n' "$env_name" >> "$invited_names_file"
          mapfile -t invited_locas < <(printf '%s' "$identity" | _jq_raw '.locas[]?' 2>/dev/null)
          mapfile -t needs_fresh < <(_needs_fresh_davet "$f")
          verified=(); stale=(); refresh=(); unlisted=(); expected_keys=" "
          for loca in "${invited_locas[@]}"; do
            key="DAVET_$(_var "$loca")"
            expected_keys+="$key "
            token=$(_credential_value "$f" "$key" 2>/dev/null || true)
            if _invite_is_live "$server" "$loca" "$token"; then
              verified+=("$loca")
            elif _in_list "$loca" ${needs_fresh[@]+"${needs_fresh[@]}"}; then
              refresh+=("$loca")
            else
              stale+=("$loca")
            fi
          done
          while IFS= read -r key; do
            [ -n "$key" ] || continue
            case "$expected_keys" in
              *" $key "*) ;;
              *) stale+=("${key#DAVET_}"); unlisted+=("${key#DAVET_}") ;;
            esac
          done < <(sed -n 's/^\(DAVET_[A-Za-z0-9_]*\)=.*/\1/p' "$f")
          if [ "${#refresh[@]}" -gt 0 ]; then
            refresh_list=$(IFS=,; printf '%s' "${refresh[*]}")
            echo "  NEEDS FRESH DAVET: '${env_name:-?}' — ${refresh_list}: no kept invite fits the seat; it needs a fresh invite — ask the Master to re-send ($f)"
          fi
          if [ "${#verified[@]}" -gt 0 ]; then
            verified_list=$(IFS=,; printf '%s' "${verified[*]}")
            echo "  INVITED (davet verified): '${env_name:-?}' — ${verified_list} ($f)"
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
            echo "  STALE: '${env_name:-?}' — revoked, expired, missing, or replaced invite credentials: $stale_list ($f)"
          elif [ "${#refresh[@]}" -eq 0 ]; then
            echo "  LOBBY: '${env_name:-?}' — membership valid, no active loca invite ($f)"
          fi
          # Only a loca the server no longer lists is certainly gone. A seated
          # loca's invite that did not verify may have met a network fault, and
          # a server that keeps only invite hashes cannot send it again.
          if [ "$fix" = "--fix" ] && [ "${#unlisted[@]}" -gt 0 ]; then
            {
              for loca in "${unlisted[@]}"; do
                printf 'DAVET_%s\0\0' "$(_var "$loca")"
              done
              printf 'LOCA_SESSION\0\0'
            } | _credential_patch_file "$f"
            echo "  fixed: removed local invite/session cache for locas the server no longer lists; invites for seated locas are kept"
          fi
          continue
        fi
        if [ "$identity_code" = "403" ] && [ "$identity" = "membership suspended" ]; then
          echo "  SUSPENDED: '${env_name:-?}' — the server says the Master suspended this membership; it resumes when the Master resumes it, and a running listener reconnects by itself ($f)"
          continue
        fi
        if _membership_suspended "$f"; then
          echo "  SUSPENDED: '${env_name:-?}' — the listener was told the Master suspended this membership; it resumes when the Master resumes it, and a running listener reconnects by itself ($f)"
          continue
        fi
      fi
      if curl -sfS -m 5 -H "X-Session-Token: $sess" "$server/rooms" >/dev/null 2>&1; then
        echo "  ready: '${env_name:-?}' — invites: ${locas:-none} ($f)"
      else
        echo "  '${env_name:-?}' — session stale, renews on next use ($f)"
      fi
    done
    if [ "$found" = 0 ]; then
      echo "  no identity for $server — run: connect.sh setup $server <name>"
      echo "  (an invite comes from the master, privately — never from a room)"
    fi
    echo "── loca client processes (all servers) ──"
    found=0
    process_probe_available=1
    if ! command -v pgrep >/dev/null 2>&1 || ! command -v ps >/dev/null 2>&1; then
      process_probe_available=0
      echo "  UNKNOWN: process inspection unavailable (pgrep/ps missing); listener state was not checked"
    fi
    # pid \t age \t cmd  for every listener/bot/adapter. Leave out doctor's
    # own process and its subshells (same pid or same exact command line),
    # never a line merely because a name in it holds the word "doctor".
    doctor_cmd=$(ps -ww -o args= -p $$ 2>/dev/null || true)
    while [ "$process_probe_available" = 1 ] && IFS= read -r line; do
      pid="${line%% *}"
      cmd="${line#* }"
      age=$(ps -o etime= -p "$pid" 2>/dev/null | tr -d ' ')
      # room+name extracted from the ws url in the args. A Lobby listener's
      # room= is empty: it still serves every loca the Lobby seats it in.
      key=$(printf '%s' "$cmd" | grep -oE 'room=[^& ]*&name=[^& "]+' | head -1)
      echo "  pid=$pid age=$age ${key:-?} :: $(printf '%s' "$cmd" | cut -c1-90)"
      listener_name=$(printf '%s' "$key" | sed -n 's/.*&name=//p')
      [ -z "$listener_name" ] || printf '%s\n' "$listener_name" >> "$listener_names_file"
      found=1
      echo "$pid $key" >> /tmp/loca-doctor.$$
    done < <(
      if [ "$process_probe_available" = 1 ]; then
        pgrep -af 'listen\.py|bot\.py|agentd\.py' |
          DOCTOR_PID=$$ DOCTOR_CMD="$doctor_cmd" awk '{
            cmd = $0; sub(/^[0-9]+ /, "", cmd)
            if ($1 == ENVIRON["DOCTOR_PID"] || cmd == ENVIRON["DOCTOR_CMD"]) next
            print
          }' |
          grep -v 'runtime_agent\.py' |
          grep -v 'monitor_listener\.py' |
          grep -vE '^[0-9]+ +(/bin/)?(ba)?sh '
      fi
    )
    [ "$process_probe_available" = 1 ] && [ "$found" = 0 ] && echo "  none"
    echo "── listener coverage ──"
    coverage=0
    while IFS= read -r invited_name; do
      [ -n "$invited_name" ] || continue
      coverage=1
      if [ "$process_probe_available" = 0 ]; then
        echo "  UNKNOWN: '$invited_name' listener coverage could not be inspected on this platform"
      elif grep -Fxq "$invited_name" "$listener_names_file"; then
        echo "  OK: '$invited_name' has a live listener"
      else
        echo "  MISSING LISTENER: '$invited_name' has a valid membership but no live listener; calls, delivery, presence and session renewal cannot reach it"
      fi
    done < <(sort -u "$invited_names_file")
    [ "$coverage" = 1 ] || echo "  no member identities to check"
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
      [ "$process_probe_available" = 1 ] || exit 0
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
    ) || true
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
    # `doctor` is an interactive report. Findings are rendered above but do
    # not inherit an incidental non-zero status from the final probe under
    # `set -o pipefail`. A future CI gate must be an explicit --fail-on-* mode.
    true
    ;;
esac
