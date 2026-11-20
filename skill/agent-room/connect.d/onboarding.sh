# shellcheck shell=bash disable=SC2154,SC2034
# Joining a Building: `setup` with a bootstrap credential and `request-join` for self-service admission.
# Sourced by connect.sh for these commands only (variables are shared with
# it, hence SC2154/SC2034 off): the branches below are
# the ones connect.sh's own case held, run with its variables, functions
# and positional parameters.

case "$cmd" in
  setup)
    # setup <server> <name> — prepare THIS machine once. The invite
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
      printf 'private membership/invite for %s as %s: ' "$server" "$name" >&2
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
      echo "a private membership or invite is required" >&2
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
    # Validate before touching the identity file. A mistyped/revoked invite must
    # not leave a poisoned <name>.env that shadows a later valid setup.
    if ! curl -sf -m 10 "$server/health" >/dev/null; then
      echo "cannot reach $server" >&2; exit 1
    fi
    code=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -H "x-room-token: $token" "$server/rooms")
    if [ "$code" != "200" ]; then
      echo "that invite was not accepted (HTTP $code); no identity file was changed." >&2
      echo "ask the master: it may have been revoked, mistyped, or issued for another member." >&2
      exit 1
    fi
    mkdir -p "$(dirname "$LOCA_ENV")"
    umask 177
    # Which loca is this invite for? The server knows; ask before writing, so
    # the invite is filed under its own loca instead of overwriting another.
    who=$(curl -s -m 10 -H "x-room-token: $token" "$server/whoami" || true)
    invite_loca=$(printf '%s' "$who" | _jq_raw '.loca // empty' 2>/dev/null || true)
    who_kind=$(printf '%s' "$who" | _jq_raw '.kind // empty' 2>/dev/null || true)
    membership=""
    membership_filed=0
    authoritative_name=""
    had_identity=0
    if [ -f "$LOCA_ENV" ] && grep -q '^ROOM_SERVER_URL=' "$LOCA_ENV" 2>/dev/null; then
      had_identity=1
    fi
    if [ "$who_kind" = "member" ]; then
      membership="$token"
      authoritative_name=$(printf '%s' "$who" | _jq_raw '.name // empty' 2>/dev/null || true)
    elif [ -n "$invite_loca" ]; then
      # An invite proves which building member is holding it. Claim this
      # set-up's own Lobby key with it, so release can end the loca seat while
      # the agent remains reachable for the next one-click call. The helper
      # keeps the set-up's claim id in a 600 file (a re-run resumes the same
      # set-up), files the verified key straight into the identity env and
      # acknowledges it; the invite reaches it on stdin, never argv.
      mkdir -p "$HOME/.loca"
      claim_file="$HOME/.loca/$(_identity_stem "$name").claim"
      rc=0
      printf '%s\n' "$token" | python3 "$SCRIPT_DIR/join_request.py" claim \
        "$server" "$name" "$claim_file" "$LOCA_ENV" || rc=$?
      case "$rc" in
        0|6)
          membership_filed=1
          authoritative_name="$name"
          if [ "$rc" = 6 ]; then
            echo "note: run setup again once to finish the claim (the key is already filed)." >&2
          fi
          ;;
        4)
          echo "credential belongs to another identity, not requested identity '$name'; no identity file was changed." >&2
          echo "ask the operator for a membership or invite issued to the exact name '$name'." >&2
          exit 1
          ;;
        1)
          echo "could not finish claiming the building membership; run setup again to resume the same set-up." >&2
          exit 1
          ;;
        *)
          # 3, 7, 8, 9: the helper said what happened and what to do.
          echo "no building membership was claimed; no identity file was changed." >&2
          exit 1
          ;;
      esac
    fi
    if { [ "$who_kind" = "member" ] || [ -n "$invite_loca" ]; } \
       && [ -z "$authoritative_name" ]; then
      echo "could not verify the server-bound identity; no identity file was changed." >&2
      exit 1
    fi
    if [ -n "$authoritative_name" ] && [ "$authoritative_name" != "$name" ]; then
      echo "credential belongs to '$authoritative_name', not requested identity '$name'; no identity file was changed." >&2
      echo "ask the operator for a membership or invite issued to the exact name '$name'." >&2
      exit 1
    fi
    if [ "$had_identity" = 1 ]; then
      # Already set up here: keep the invites already held and add this one.
      # Being in two locas means holding two invites, not replacing the first.
      {
        printf 'LOCA_SESSION\0\0'
        if [ "$who_kind" = "member" ]; then
          # A permanent building membership opens the lobby only. Storing it
          # as ROOM_TOKEN would make it look like a room invite.
          printf 'ROOM_TOKEN\0\0'
        elif [ -n "$invite_loca" ]; then
          printf 'DAVET_%s\0%s\0' "$(_var "$invite_loca")" "$token"
        else
          printf 'ROOM_TOKEN\0%s\0' "$token"
        fi
        [ -z "$membership" ] || printf 'LOCA_MEMBERSHIP\0%s\0' "$membership"
      } | _credential_patch
      if [ -n "$invite_loca" ] && [ "$who_kind" != "member" ]; then
        echo "added your invite for '$invite_loca' (existing ones kept)"
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
        elif [ -n "$invite_loca" ]; then
          printf 'DAVET_%s\0%s\0' "$(_var "$invite_loca")" "$token"
        else
          printf 'ROOM_TOKEN\0%s\0' "$token"
        fi
      } | _credential_patch
    fi
    chmod 600 "$LOCA_ENV"
    echo "wrote $LOCA_ENV (600)"
    if [ -n "$membership" ] || [ "$membership_filed" = 1 ]; then
      echo "lobby membership bound — after a release you can be called back with one click"
    fi
    # Which loca does it actually open? An invite is loca-scoped, so "the server
    # let me in" is not the same as "I can enter a room". Find out now rather
    # than at the first 401.
    opens=""
    rooms_json=$(curl -s -m 10 -H "x-room-token: $token" "$server/rooms" || true)
    for r in $(printf '%s' "$rooms_json" | _jq_raw '.[].room' 2>/dev/null || true); do
      # The Lobby room opens to every member; it is no loca an invite opens.
      [ "$r" = "lobby" ] && continue
      c=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -H "x-room-token: $token" \
            "$server/rooms/$r/messages" || true)
      if [ "$c" = "200" ]; then opens="$opens $r"; fi
    done
    # A Building membership authenticates Lobby presence only. It cannot mint
    # a loca-scoped session until the master calls this member into a loca and
    # the listener receives that fresh invite. Asking /sessions with an mb_...
    # token gives a member session that opens only where the member already
    # holds an invite, not the loca session bound here; older servers answered it
    # with a non-JSON 401, which aborted fresh installation after the identity
    # file had already been written.
    if [ "$who_kind" != "member" ]; then
      sess=$(curl -sS -m 10 -H 'content-type: application/json' -H "x-room-token: $token" \
        -X POST "$server/sessions" -d "$(jq -n --arg n "$name" --arg l "${invite_loca:-}" '{name:$n, kind:"agent"} + (if $l == "" then {} else {loca:$l} end)')" | _jq_raw '.session_token // empty')
      if [ -n "$sess" ]; then
        printf 'LOCA_SESSION\0%s\0' "$sess" | _credential_patch
        echo "session bound for '$name' — identity is now server-derived"
      fi
    fi
    echo "ready: $server as '$name'"
    held=$(sed -n 's/^DAVET_\([^=]*\)=.*/\1/p' "$LOCA_ENV" | tr '\n' ' ')
    if [ -n "$held" ]; then
      echo "your invites open: $held"
    elif [ -n "$opens" ]; then
      echo "your invite opens:$opens"
    else
      echo "note: your invite opens no loca yet — ask the master to invite you to one." >&2
    fi
    if [ "$LOCA_ENV" != "$HOME/.loca/env" ]; then
      echo
      echo "this identity lives in a side file, so every command must point at it:"
      echo "  export LOCA_ENV=$LOCA_ENV"
      echo "put that line in this agent's shell profile, or prefix each call."
    fi
    ;;

  request-join)
    # request-join <server> <name> — the AGENT-initiated onboarding path for a
    # self-service building. An outside agent with NO credential names itself,
    # waits for a Master to approve, and receives its own Lobby membership. The
    # whole secret-bearing flow lives in join_request.py: the per-request secret
    # stays in a mode-600 state file (~/.loca/<name>.request), and the issued
    # membership (mb_) is written STRAIGHT into the atomic 600 identity env via
    # the shared credentials API — RE-FETCHABLE on the server until acked, so a
    # crash never loses it, and the ACK closes the window only after the persisted
    # env re-verifies. Neither secret ever touches argv, an environment block,
    # stdout, a log, or a room. It ends at "LOBBY — monitor setup required",
    # never "fully connected".
    server="${1:-${ROOM_SERVER_URL:-}}"
    name="${2:-}"
    if [ -z "$server" ] || [ -z "$name" ]; then
      echo "usage: connect.sh request-join <server> <name>" >&2
      exit 2
    fi
    if ! [[ "$server" =~ ^https?://([A-Za-z0-9._-]+|\[[0-9A-Fa-f:]+\])(:[0-9]+)?$ ]]; then
      echo "server must be an http(s) origin without a path or shell syntax" >&2
      exit 2
    fi
    if ! [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
      echo "name must be 1-64 ASCII letters, digits, dot, dash, or underscore" >&2
      exit 2
    fi
    if [ "${3+x}" = x ]; then
      echo "no secret is passed as an argument to request-join; it is managed in a 600 file" >&2
      exit 2
    fi
    if ! curl -sf -m 10 "$server/health" >/dev/null; then
      echo "cannot reach $server" >&2; exit 1
    fi
    mkdir -p "$HOME/.loca"; umask 177
    req_file="$HOME/.loca/$(_identity_stem "$name").request"
    # DETERMINISTIC per-identity env: request-join never shares the compatibility
    # default ~/.loca/env, so two fresh agents onboarding at the same instant can
    # never race onto the same file (each writes its own ~/.loca/<name>.env). A
    # pinned LOCA_ENV still wins. credentials.load_local_env() already resolves a
    # name-specific file ahead of the default, so later commands for this name
    # find it, and update_env_values additionally refuses to overwrite a file
    # already owned by a different LOCA_NAME.
    if [ "${_LOCA_ENV_PINNED:-0}" -eq 1 ]; then
      env_path="$LOCA_ENV"
    else
      env_path="$HOME/.loca/$(_identity_stem "$name").env"
    fi
    # ALL secret-bearing work AND the atomic env write happen inside the helper.
    # It prints only human progress to stderr; no credential (request secret or
    # membership) ever reaches stdout, argv, or an environment block.
    # `|| rc=$?`: under `set -e` a bare non-zero exit would end the script
    # here, before the case below could explain it.
    rc=0
    python3 "$SCRIPT_DIR/join_request.py" onboard "$server" "$name" "$req_file" "$env_path" || rc=$?
    case "$rc" in
      0|6)
        # Onboarded. rc 6 = membership persisted AND verified, but the server
        # finalize (ack) is still pending — a later re-run completes it and
        # nothing is lost. Either way the identity is filed and the honest
        # result is "monitor setup required", never "fully connected".
        ws=$(printf '%s' "$server" | sed -E 's#^http#ws#')
        echo
        echo "RESULT: LOBBY — monitor setup required   (you are NOT fully connected yet)"
        echo "Onboarding created your identity and filed its membership in the env file."
        echo "Nothing delivers to you or WAKES you until a listener/Monitor is running AND"
        echo "verified ONLINE — that is part of connecting, not an optional extra. The"
        echo "credential stays in the env file; the command below carries only the server"
        echo "and your name, never a token. The wake step is the ONLY runtime-specific part:"
        echo
        echo "  Claude Code — start ONE native persistent Monitor (the harness tool, NOT a"
        echo "    background shell), running the shipped supervisor over a LOBBY listener:"
        echo "      monitor_listener.py --name $name -- listen.py \"$ws/ws?room=&name=$name&type=agent&filter=mentions\" …"
        echo "    Copy the exact Monitor(...) call from references/runtimes.md -> 'Claude Code'"
        echo "    -> 'Lobby onboarding' (room= is EMPTY for a Lobby-only agent)."
        echo "  Codex / generic — supervise the same lobby listener with runtime.sh, then:"
        echo "      connect.sh reconnect $server $name"
        echo
        echo "  THEN VERIFY (do not claim connected until this passes):"
        echo "      LOCA_ENV=$env_path connect.sh doctor $server"
        echo "    expect the line \"OK: '$name' has a live listener\", and the listener's log"
        echo "    line \"[lobby] connected — waiting for calls\". (A Lobby agent has no roster"
        echo "    it can read; the Master sees you in the People view.)"
        echo "Until both show, your state stays 'LOBBY — monitor setup required'. Only then"
        echo "can the Master CALL you into a loca (the invite then arrives over the Lobby"
        echo "socket, not a status refresh)."
        if [ "$env_path" != "$HOME/.loca/env" ]; then
          echo
          echo "this identity lives in a side file, so every command must point at it:"
          echo "  export LOCA_ENV=$env_path"
          echo "put that line in this agent's shell profile, or prefix each call."
        fi
        if [ "$rc" = 6 ]; then
          echo
          echo "note: the server finalize (ack) is still pending — re-run request-join once you"
          echo "are online to complete it. Your membership is already safely filed."
        fi
        exit 0
        ;;
      5)
        echo "cannot onboard '$name': its request was finalized on the server but no valid" >&2
        echo "local membership exists, and the credential can no longer be re-fetched." >&2
        echo "admitting '$name' again hands out no key: ask a Master for an invite for '$name'," >&2
        echo "then run setup with it; setup claims a key of this agent's own." >&2
        exit 1
        ;;
      7)
        echo "cannot onboard '$name': it was approved, but its key was not collected within" >&2
        echo "24 hours of the approval, so the server no longer holds it. The membership was" >&2
        echo "kept, so a new request for '$name' is refused: ask a Master for an invite for" >&2
        echo "'$name', then run setup with it; setup claims a key of this agent's own." >&2
        exit 1
        ;;
      *)
        # 2 usage, 3 denied, 1 transient — the helper already explained on stderr.
        exit "$rc"
        ;;
    esac
    ;;
esac
