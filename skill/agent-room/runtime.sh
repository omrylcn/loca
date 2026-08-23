#!/usr/bin/env bash
set -euo pipefail

SKILL_DIR=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

usage() {
  cat >&2 <<'EOF'
Usage:
  runtime.sh start NAME [--runtime auto|codex|codex-v2|codex-v1|hook|manual] [--thread-id ID] [--replace-thread] [--hook COMMAND] [--env FILE] [--only-direct] [--codex-sandbox inherit|workspace-write|danger-full-access] [--relay-mode shadow|live]
  runtime.sh status NAME [--env FILE]
  runtime.sh stop NAME
EOF
}

safe_name() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9_-' '_'
}

service_name() {
  printf 'loca-listener-%s.service' "$(safe_name "$1")"
}

user_systemd_available() {
  command -v systemctl >/dev/null 2>&1 \
    && systemctl --user show-environment >/dev/null 2>&1
}

identity_file() {
  local name="$1" explicit="${2:-}" safe candidate default_name
  safe=$(safe_name "$name")
  candidate="${explicit:-$HOME/.loca/$safe.env}"
  if [ ! -f "$candidate" ] && [ -z "$explicit" ] && [ -f "$HOME/.loca/env" ]; then
    default_name=$(sed -n 's/^LOCA_NAME=//p' "$HOME/.loca/env" | head -1)
    [ "$default_name" = "$name" ] && candidate="$HOME/.loca/env"
  fi
  printf '%s' "$candidate"
}

stop_runtime() {
  local name="$1" safe pid_file pid cmd unit unit_file launcher
  safe=$(safe_name "$name")
  pid_file="$HOME/.loca/run/$safe.pid"
  unit=$(service_name "$name")
  unit_file="$HOME/.config/systemd/user/$unit"
  launcher="$HOME/.loca/run/$safe-service.sh"
  if user_systemd_available \
    && systemctl --user cat "$unit" >/dev/null 2>&1; then
    systemctl --user disable --now "$unit" >/dev/null 2>&1 || true
    systemctl --user reset-failed "$unit" >/dev/null 2>&1 || true
    rm -f "$unit_file" "$launcher"
    systemctl --user daemon-reload
    rm -f "$pid_file" "$HOME/.loca/run/$safe.runtime" "$HOME/.loca/run/$safe.codex-thread" "$HOME/.loca/run/$safe.delivery" "$HOME/.loca/run/$safe.codex-sandbox" "$HOME/.loca/run/$safe.relay-mode" "$HOME/.loca/run/$safe.health.json" "$HOME/.loca/run/$safe.v2-shadow.health.json"
    echo "stopped: $name"
    return 0
  fi
  [ -f "$pid_file" ] || { echo "not running: $name"; return 0; }
  pid=$(sed -n '1p' "$pid_file")
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$pid_file" "$HOME/.loca/run/$safe.runtime" "$HOME/.loca/run/$safe.codex-thread" "$HOME/.loca/run/$safe.delivery" "$HOME/.loca/run/$safe.codex-sandbox" "$HOME/.loca/run/$safe.relay-mode" "$HOME/.loca/run/$safe.health.json" "$HOME/.loca/run/$safe.v2-shadow.health.json"
    echo "stale pid file cleared: $name"
    return 0
  fi
  cmd=$(ps -o args= -p "$pid" 2>/dev/null || true)
  case "$cmd" in *runtime_agent.py*"$name"*|*listen.py*"$name"*) ;; *)
    echo "refusing to stop pid=$pid: it is not the '$name' loca listener" >&2
    return 4
  esac
  kill "$pid"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "listener did not stop cleanly: pid=$pid" >&2
    return 1
  fi
  rm -f "$pid_file" "$HOME/.loca/run/$safe.runtime" "$HOME/.loca/run/$safe.codex-thread" "$HOME/.loca/run/$safe.delivery" "$HOME/.loca/run/$safe.codex-sandbox" "$HOME/.loca/run/$safe.relay-mode" "$HOME/.loca/run/$safe.health.json" "$HOME/.loca/run/$safe.v2-shadow.health.json"
  echo "stopped: $name"
}

start_runtime() {
  local name="${1:-}" runtime="auto" thread_id="${CODEX_THREAD_ID:-}" only_direct=false codex_sandbox=inherit replace_thread=false relay_mode=""
  local hook="" shadow_hook="" v2_hook="" v2_health="" explicit_env="${LOCA_ENV:-}" safe env_file server env_name binds_thread=false
  local run_dir log_dir msg_dir cursor_dir inbox_dir worker_cursor_dir pid_file runtime_file thread_file delivery_file sandbox_file relay_file health_file shadow_health_file state_db
  local log_file msg_file cursor_file inbox_file worker_cursor_file old_pid old_cmd old_runtime old_delivery old_sandbox old_relay delivery enc_name ws url pid unit supervisor codex_bin
  local unit_dir unit_file launcher launcher_quoted start_dir
  local -a args supervisor_args

  [ -n "$name" ] || { usage; return 2; }
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --runtime) runtime="${2:-}"; shift 2 ;;
      --thread-id) thread_id="${2:-}"; shift 2 ;;
      --replace-thread) replace_thread=true; shift ;;
      --hook) hook="${2:-}"; shift 2 ;;
      --env) explicit_env="${2:-}"; shift 2 ;;
      --only-direct) only_direct=true; shift ;;
      --codex-sandbox) codex_sandbox="${2:-}"; shift 2 ;;
      --relay-mode) relay_mode="${2:-}"; shift 2 ;;
      *) echo "unknown option: $1" >&2; usage; return 2 ;;
    esac
  done
  case "$runtime" in auto|codex|codex-v2|codex-v1|hook|manual) ;; *)
    echo "--runtime must be auto, codex, codex-v2, codex-v1, hook, or manual" >&2
    return 2
  esac
  case "$codex_sandbox" in inherit|workspace-write|danger-full-access) ;; *)
    echo "--codex-sandbox must be inherit, workspace-write, or danger-full-access" >&2
    return 2
  esac
  case "$relay_mode" in ""|shadow|live) ;; *)
    echo "--relay-mode must be shadow or live" >&2
    return 2
  esac
  if [ "$runtime" = auto ]; then
    if [ -n "$thread_id" ]; then runtime=codex-v2
    elif [ -n "$hook" ]; then runtime=hook
    else runtime=manual
    fi
  fi
  # `codex` is the stable public runtime name. Adapter v2 owns the reply relay,
  # so a room reply must be accepted before its lifecycle can advance. The old
  # per-delivery v1 adapter remains available only as an explicit rollback.
  [ "$runtime" != codex ] || runtime=codex-v2
  if [ -z "$relay_mode" ]; then
    if [ "$runtime" = codex-v2 ]; then relay_mode=live
    else relay_mode=shadow
    fi
  fi
  [ "$runtime" != hook ] || [ -n "$hook" ] || {
    echo "--runtime hook requires --hook COMMAND" >&2
    return 2
  }
  [ "$runtime" != codex-v1 ] || [ -n "$thread_id" ] || {
    echo "legacy Codex v1 thread id missing; invoke \$loca inside the Codex thread" >&2
    return 2
  }
  if [ "$runtime" = codex-v2 ] && [ "$relay_mode" = shadow ] && [ -z "$thread_id" ]; then
    echo "Codex v2 shadow requires --thread-id so the existing v1 responder stays live" >&2
    return 2
  fi
  if [ "$runtime" = codex-v1 ] || { [ "$runtime" = codex-v2 ] && [ "$relay_mode" = shadow ]; }; then
    binds_thread=true
  fi

  safe=$(safe_name "$name")
  env_file=$(identity_file "$name" "$explicit_env")
  [ -f "$env_file" ] || {
    echo "identity not found: $env_file (run connect.sh setup first)" >&2
    return 3
  }
  chmod 600 "$env_file"
  server=$(sed -n 's/^ROOM_SERVER_URL=//p' "$env_file" | head -1)
  env_name=$(sed -n 's/^LOCA_NAME=//p' "$env_file" | head -1)
  [ "$env_name" = "$name" ] || {
    echo "identity mismatch: $env_file belongs to '$env_name', not '$name'" >&2
    return 3
  }
  case "$server" in http://*|https://*) ;; *)
    echo "invalid ROOM_SERVER_URL in $env_file" >&2
    return 3
  esac

  run_dir="$HOME/.loca/run"
  log_dir="$HOME/.loca/logs"
  msg_dir="$HOME/.loca/messages"
  cursor_dir="$HOME/.loca/cursors"
  inbox_dir="$HOME/.loca/inbox"
  worker_cursor_dir="$HOME/.loca/worker-cursors"
  mkdir -p "$run_dir" "$log_dir" "$msg_dir" "$cursor_dir" "$inbox_dir" "$worker_cursor_dir"
  chmod 700 "$HOME/.loca" "$run_dir" "$log_dir" "$msg_dir" "$cursor_dir" "$inbox_dir" "$worker_cursor_dir"
  pid_file="$run_dir/$safe.pid"
  runtime_file="$run_dir/$safe.runtime"
  thread_file="$run_dir/$safe.codex-thread"
  delivery_file="$run_dir/$safe.delivery"
  sandbox_file="$run_dir/$safe.codex-sandbox"
  relay_file="$run_dir/$safe.relay-mode"
  health_file="$run_dir/$safe.health.json"
  shadow_health_file="$run_dir/$safe.v2-shadow.health.json"
  log_file="$log_dir/$safe.listener.log"
  msg_file="$msg_dir/$safe.jsonl"
  cursor_file="$cursor_dir/$safe.json"
  inbox_file="$inbox_dir/$safe.jsonl"
  worker_cursor_file="$worker_cursor_dir/$safe.json"
  state_db="$HOME/.loca/runtime-v2.sqlite3"
  if [ "$runtime" = codex-v2 ]; then
    # Never feed a historical v1 queue into a new v2 canary. The v2 adapter
    # owns a separate durable source and resumes it by SQLite cursor.
    inbox_file="$inbox_dir/$safe.v2.jsonl"
    if [ "$relay_mode" = shadow ]; then
      # Dual shadow needs a fresh shared feed and an independent v1 cursor;
      # reusing the legacy cursor with a different file can skip or replay data.
      inbox_file="$inbox_dir/$safe.v2-shadow.jsonl"
      worker_cursor_file="$worker_cursor_dir/$safe.v2-shadow.json"
    fi
  fi
  start_dir="${LOCA_WORKDIR:-$PWD}"
  [ -d "$start_dir" ] || {
    echo "runtime working directory does not exist: $start_dir" >&2
    return 3
  }
  unit=$(service_name "$name")
  unit_dir="$HOME/.config/systemd/user"
  unit_file="$unit_dir/$unit"
  launcher="$run_dir/$safe-service.sh"
  delivery=mentions
  case "$name" in loca-dev|loca-care) only_direct=true ;; esac
  [ "$only_direct" = false ] || delivery=direct

  if user_systemd_available && systemctl --user is-active --quiet "$unit"; then
    if ! systemctl --user is-enabled --quiet "$unit"; then
      # Upgrade an older transient systemd-run unit to the reboot-persistent
      # user service defined below.
      stop_runtime "$name" >/dev/null
    else
    old_runtime=$(sed -n '1p' "$runtime_file" 2>/dev/null || true)
    old_delivery=$(sed -n '1p' "$delivery_file" 2>/dev/null || echo mentions)
    old_sandbox=$(sed -n '1p' "$sandbox_file" 2>/dev/null || echo inherit)
    old_relay=$(sed -n '1p' "$relay_file" 2>/dev/null || echo shadow)
    if [ "$runtime" = manual ] && [ "$old_runtime" != manual ] && [ -n "$old_runtime" ]; then
      pid=$(systemctl --user show "$unit" -p MainPID --value)
      printf '%s\n' "$pid" > "$pid_file"
      echo "preserved existing runtime=$old_runtime (pid=$pid); stop it explicitly before switching to manual"
      return 0
    fi
    if [ "$old_runtime" = "$runtime" ] && [ "$old_delivery" = "$delivery" ] && [ "$old_sandbox" = "$codex_sandbox" ] && [ "$old_relay" = "$relay_mode" ]; then
      if [ "$binds_thread" = true ]; then
        old_thread=$(sed -n '1p' "$thread_file" 2>/dev/null || true)
        if [ -n "$old_thread" ] && [ "$old_thread" != "$thread_id" ] && [ "$replace_thread" != true ]; then
          echo "refusing implicit Codex thread rebind: runtime is already bound to $old_thread" >&2
          echo "stop it first, or repeat with --replace-thread for an intentional takeover" >&2
          return 5
        fi
        if [ "$old_thread" != "$thread_id" ]; then
          stop_runtime "$name" >/dev/null
        else
          printf '%s\n' "$thread_id" > "$thread_file"
          chmod 600 "$thread_file"
        fi
      fi
      if [ "$binds_thread" = true ] && [ "$old_thread" != "$thread_id" ]; then
        : # intentional replacement falls through to a clean new supervisor
      else
      pid=$(systemctl --user show "$unit" -p MainPID --value)
      printf '%s\n' "$pid" > "$pid_file"
      echo "already running: pid=$pid runtime=$runtime delivery=$delivery supervisor=systemd"
      LOCA_ENV="$env_file" "$SKILL_DIR/connect.sh" status "$server" "$name"
      return 0
      fi
    fi
    stop_runtime "$name" >/dev/null
    fi
  elif [ -f "$pid_file" ]; then
    old_pid=$(sed -n '1p' "$pid_file")
    if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
      old_cmd=$(ps -o args= -p "$old_pid" 2>/dev/null || true)
      case "$old_cmd" in *runtime_agent.py*"$name"*|*listen.py*"$name"*)
        old_runtime=$(sed -n '1p' "$runtime_file" 2>/dev/null || true)
        old_delivery=$(sed -n '1p' "$delivery_file" 2>/dev/null || echo mentions)
        old_sandbox=$(sed -n '1p' "$sandbox_file" 2>/dev/null || echo inherit)
        old_relay=$(sed -n '1p' "$relay_file" 2>/dev/null || echo shadow)
        if [ "$runtime" = manual ] && [ "$old_runtime" != manual ] && [ -n "$old_runtime" ]; then
          echo "preserved existing runtime=$old_runtime (pid=$old_pid); stop it explicitly before switching to manual"
          return 0
        fi
        if [ "$old_runtime" = "$runtime" ] && [ "$old_delivery" = "$delivery" ] && [ "$old_sandbox" = "$codex_sandbox" ] && [ "$old_relay" = "$relay_mode" ]; then
          if [ "$binds_thread" = true ]; then
            old_thread=$(sed -n '1p' "$thread_file" 2>/dev/null || true)
            if [ -n "$old_thread" ] && [ "$old_thread" != "$thread_id" ] && [ "$replace_thread" != true ]; then
              echo "refusing implicit Codex thread rebind: runtime is already bound to $old_thread" >&2
              echo "stop it first, or repeat with --replace-thread for an intentional takeover" >&2
              return 5
            fi
            if [ "$old_thread" != "$thread_id" ]; then
              stop_runtime "$name" >/dev/null
              old_pid=""
            else
              printf '%s\n' "$thread_id" > "$thread_file"
              chmod 600 "$thread_file"
            fi
          fi
          if [ "$binds_thread" = true ] && [ -z "$old_pid" ]; then
            :
          else
          echo "already running: pid=$old_pid runtime=$runtime delivery=$delivery"
          LOCA_ENV="$env_file" "$SKILL_DIR/connect.sh" status "$server" "$name"
          return 0
          fi
        fi
        stop_runtime "$name" >/dev/null
        ;;
      esac
    fi
  fi

  if [ "$runtime" = codex-v1 ] || [ "$runtime" = codex-v2 ]; then
    # A user systemd service does not inherit an IDE shell's PATH. Resolve the
    # executable while the Codex shell is still available.
    codex_bin="${CODEX_BIN:-$(command -v codex 2>/dev/null || true)}"
    if [ -z "$codex_bin" ] || [ ! -x "$codex_bin" ]; then
      echo "codex executable not found; set CODEX_BIN before starting the runtime" >&2
      return 3
    fi
  fi

  # The legacy adapter resumes the interactive thread that invoked $loca.
  # v2 deliberately creates separate room-scoped headless threads instead.
  if [ "$runtime" = codex-v1 ]; then
    printf '%s\n' "$thread_id" > "$thread_file"
    chmod 600 "$thread_file"
    hook="$(printf '%q ' python3 "$SKILL_DIR/nudge.py" codex \
      --thread-file "$thread_file" --codex-bin "$codex_bin")"
    if [ "$codex_sandbox" != inherit ]; then
      hook+="$(printf '%q ' --sandbox-policy "$codex_sandbox")"
    fi
  elif [ "$runtime" = codex-v2 ]; then
    v2_health="$health_file"
    [ "$relay_mode" != shadow ] || v2_health="$shadow_health_file"
    v2_hook="exec $(printf '%q ' python3 "$SKILL_DIR/codex_adapter_v2.py" \
      --inbox "$inbox_file" --state-db "$state_db" --identity "$name" \
      --workdir "$start_dir" --codex-bin "$codex_bin" \
      --connect-sh "$SKILL_DIR/connect.sh" --health-file "$v2_health" \
      --relay-mode "$relay_mode")"
    if [ "$codex_sandbox" != inherit ]; then
      v2_hook+="$(printf '%q ' --sandbox "$codex_sandbox")"
    fi
    if [ "$relay_mode" = shadow ]; then
      printf '%s\n' "$thread_id" > "$thread_file"
      chmod 600 "$thread_file"
      hook="$(printf '%q ' python3 "$SKILL_DIR/nudge.py" codex \
        --thread-file "$thread_file" --codex-bin "$codex_bin")"
      if [ "$codex_sandbox" != inherit ]; then
        hook+="$(printf '%q ' --sandbox-policy "$codex_sandbox")"
      fi
      shadow_hook="$v2_hook"
    else
      hook="$v2_hook"
    fi
  fi

  enc_name=$(python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$name")
  ws=$(printf '%s' "$server" | sed -E 's#^http#ws#')
  url="$ws/ws?room=__lobby__&name=$enc_name&type=agent&filter=mentions"
  args=("$SKILL_DIR/listen.py" "$url" "$msg_file" "--skip-own" "$name" "--cursor" "$cursor_file" "--turn-log" "$inbox_file")
  # Direct-only runtimes ignore @all announcements. This is agent-agnostic:
  # operators can give any role a low-noise, low-latency wake channel.
  [ "$only_direct" = false ] || args+=("--only-direct" "$name")
  # The listener only writes durable envelopes. v1 uses a per-delivery
  # consumer; v2 runs one persistent adapter with an independent ingestion
  # ledger. Neither adapter is attached directly to listener callbacks.
  supervisor_args=(
    "$SKILL_DIR/runtime_agent.py"
    "--inbox" "$inbox_file"
    "--worker-cursor" "$worker_cursor_file"
    "--health-file" "$health_file"
  )
  # A productive Codex turn may run builds/tests for well over five minutes.
  # nudge.py applies a five-minute *inactivity* timeout; this outer cap only
  # kills a truly wedged adapter after two hours. Other hooks keep the tighter
  # default because their liveness protocol is unknown.
  if [ "$runtime" = codex-v1 ] || { [ "$runtime" = codex-v2 ] && [ "$relay_mode" = shadow ]; }; then
    supervisor_args+=(
    "--consumer-timeout-seconds" "7200"
    "--preempt-direct-user"
    )
  fi
  if [ "$runtime" = codex-v2 ] && [ "$relay_mode" = shadow ]; then
    supervisor_args+=(
      "--exec" "$hook"
      "--shadow-persistent-exec" "$shadow_hook"
      "--shadow-ready-file" "$shadow_health_file"
    )
  elif [ "$runtime" = codex-v2 ]; then
    supervisor_args+=(
      "--persistent-exec" "$hook"
      "--persistent-ready-file" "$health_file"
    )
  elif [ -n "$hook" ]; then
    supervisor_args+=("--exec" "$hook")
  fi
  supervisor_args+=("--" "python3" "${args[@]}")

  : >> "$log_file"
  if user_systemd_available; then
    mkdir -p "$unit_dir"
    chmod 700 "$unit_dir"
    start_dir="${LOCA_WORKDIR:-$PWD}"
    [ -d "$start_dir" ] || {
      echo "runtime working directory does not exist: $start_dir" >&2
      return 3
    }
    {
      printf '#!/usr/bin/env bash\n'
      printf 'set -euo pipefail\n'
      printf 'cd %q\n' "$start_dir"
      printf 'export LOCA_ENV=%q\n' "$env_file"
      printf 'exec'
      printf ' %q' python3 "${supervisor_args[@]}"
      printf ' >>%q 2>&1\n' "$log_file"
    } > "$launcher"
    chmod 700 "$launcher"
    launcher_quoted=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$launcher")
    {
      printf '[Unit]\n'
      printf 'Description=Loca runtime for %s\n' "$name"
      printf 'After=network-online.target\n'
      printf 'Wants=network-online.target\n\n'
      printf '[Service]\n'
      printf 'Type=simple\n'
      printf 'ExecStart=%s\n' "$launcher_quoted"
      printf 'Restart=on-failure\n'
      printf 'RestartSec=2s\n'
      printf 'NoNewPrivileges=true\n\n'
      printf '[Install]\n'
      printf 'WantedBy=default.target\n'
    } > "$unit_file"
    chmod 600 "$unit_file"
    systemctl --user daemon-reload
    systemctl --user reset-failed "$unit" >/dev/null 2>&1 || true
    systemctl --user enable --now "$unit_file" >/dev/null
    supervisor=systemd-enabled
    pid=0
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      pid=$(systemctl --user show "$unit" -p MainPID --value 2>/dev/null || echo 0)
      [ "${pid:-0}" -gt 0 ] && break
      sleep 0.2
    done
  else
    LOCA_ENV="$env_file" nohup python3 "${supervisor_args[@]}" >>"$log_file" 2>&1 &
    pid=$!
    supervisor=shell
  fi
  [ "${pid:-0}" -gt 0 ] || {
    echo "listener did not start; inspect $log_file" >&2
    return 1
  }
  printf '%s\n' "$pid" > "$pid_file"
  printf '%s\n' "$runtime" > "$runtime_file"
  printf '%s\n' "$delivery" > "$delivery_file"
  printf '%s\n' "$codex_sandbox" > "$sandbox_file"
  printf '%s\n' "$relay_mode" > "$relay_file"
  chmod 600 "$pid_file" "$runtime_file" "$delivery_file" "$sandbox_file" "$relay_file" "$log_file"

  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || {
      echo "listener exited; inspect $log_file" >&2
      tail -20 "$log_file" >&2 || true
      return 1
    }
    grep -q '\[lobby\] connected' "$log_file" && break
    sleep 0.2
  done

  echo "listener running: pid=$pid runtime=$runtime delivery=$delivery supervisor=$supervisor"
  if [ "$supervisor" = systemd-enabled ] \
    && command -v loginctl >/dev/null 2>&1 \
    && [ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || echo no)" != "yes" ]; then
    echo "reboot note: user linger is disabled; an admin may run:"
    echo "  sudo loginctl enable-linger $(id -un)"
    echo "without linger the enabled service resumes at the next login, not before"
  fi
  LOCA_ENV="$env_file" "$SKILL_DIR/connect.sh" status "$server" "$name"
  echo "messages: $msg_file"
  echo "turn inbox: $inbox_file"
  if [ "$runtime" = codex-v2 ]; then
    echo "attention ledger: $state_db"
  else
    echo "worker cursor: $worker_cursor_file"
  fi
  echo "log: $log_file"
}

status_runtime() {
  local name="${1:-}" explicit_env="${LOCA_ENV:-}" safe env_file server pid_file pid unit
  local running=false runtime relay_mode health_file shadow_health_file log_file last_transport wake ack reply duplicate_count health_age updated_ms now_ms
  local reply_pending oldest_reply_ms reply_age adapter_version
  local server_version skill_version encoded_name inbox_display
  [ -n "$name" ] || { usage; return 2; }
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --env) explicit_env="${2:-}"; shift 2 ;;
      *) echo "unknown option: $1" >&2; usage; return 2 ;;
    esac
  done
  safe=$(safe_name "$name")
  env_file=$(identity_file "$name" "$explicit_env")
  [ -f "$env_file" ] || { echo "identity not found: $env_file" >&2; return 3; }
  server=$(sed -n 's/^ROOM_SERVER_URL=//p' "$env_file" | head -1)
  pid_file="$HOME/.loca/run/$safe.pid"
  unit=$(service_name "$name")
  health_file="$HOME/.loca/run/$safe.health.json"
  shadow_health_file="$HOME/.loca/run/$safe.v2-shadow.health.json"
  log_file="$HOME/.loca/logs/$safe.listener.log"
  runtime=$(sed -n '1p' "$HOME/.loca/run/$safe.runtime" 2>/dev/null || echo manual)
  relay_mode=$(sed -n '1p' "$HOME/.loca/run/$safe.relay-mode" 2>/dev/null || echo shadow)
  inbox_display="$HOME/.loca/inbox/$safe.jsonl"
  if [ "$runtime" = codex-v2 ]; then
    inbox_display="$HOME/.loca/inbox/$safe.v2.jsonl"
    [ "$relay_mode" != shadow ] || inbox_display="$HOME/.loca/inbox/$safe.v2-shadow.jsonl"
  fi
  echo "identity: OK (name=$name server=$server)"
  if user_systemd_available && systemctl --user is-active --quiet "$unit"; then
    pid=$(systemctl --user show "$unit" -p MainPID --value)
    echo "process: running (pid=$pid supervisor=systemd)"
    running=true
  elif [ -f "$pid_file" ]; then
    pid=$(sed -n '1p' "$pid_file")
    if kill -0 "$pid" 2>/dev/null; then
      echo "process: running (pid=$pid)"
      running=true
    else echo "process: stopped (stale pid=$pid)"
    fi
  else
    echo "process: stopped"
  fi
  echo "runtime: $runtime"
  [ "$runtime" != codex-v2 ] || echo "relay mode: $relay_mode"
  case "$runtime" in codex|codex-v1)
    echo "reply contract: DEGRADED (legacy v1 cannot prove Loca accepted the reply)"
    ;;
  esac
  echo "delivery: $(sed -n '1p' "$HOME/.loca/run/$safe.delivery" 2>/dev/null || echo mentions)"
  echo "codex sandbox: $(sed -n '1p' "$HOME/.loca/run/$safe.codex-sandbox" 2>/dev/null || echo inherit)"
  echo "turn inbox: $inbox_display"
  if [ "$runtime" = codex-v2 ]; then
    echo "attention ledger: $HOME/.loca/runtime-v2.sqlite3"
  else
    echo "worker cursor: $HOME/.loca/worker-cursors/$safe.json"
  fi
  if [ "$running" = true ]; then
    echo "delivery health: OK (durable listener supervised)"
  else
    echo "delivery health: DEGRADED (listener stopped)"
  fi
  last_transport=$(grep -E '\] listening ->|\[lobby\] connected|reconnect after error|membership rejected' "$log_file" 2>/dev/null | tail -1 || true)
  case "$last_transport" in
    *"listening ->"*|*"[lobby] connected"*)
      [ "$running" = true ] && echo "presence health: OK ($last_transport)" \
        || echo "presence health: DEGRADED (last connected, process stopped)"
      ;;
    "") echo "presence health: UNVERIFIED (no connection record yet)" ;;
    *) echo "presence health: DEGRADED ($last_transport)" ;;
  esac
  if [ -f "$health_file" ]; then
    wake=$(jq -r '.wake // "UNVERIFIED"' "$health_file" 2>/dev/null || echo UNVERIFIED)
    ack=$(jq -r '.ack // "UNVERIFIED"' "$health_file" 2>/dev/null || echo UNVERIFIED)
    reply=$(jq -r '.reply // "UNVERIFIED"' "$health_file" 2>/dev/null || echo UNVERIFIED)
    reply_pending=$(jq -r '.reply_required_pending // 0' "$health_file" 2>/dev/null || echo 0)
    oldest_reply_ms=$(jq -r '.oldest_reply_stored_at_ms // 0' "$health_file" 2>/dev/null || echo 0)
    updated_ms=$(jq -r '.updated_at_ms // 0' "$health_file" 2>/dev/null || echo 0)
    now_ms=$(date +%s%3N)
    health_age=$(( (now_ms - updated_ms) / 1000 ))
    if [ "$oldest_reply_ms" -gt 0 ]; then
      reply_age=$(( (now_ms - oldest_reply_ms) / 1000 ))
    else
      reply_age=0
    fi
    if [ "$health_age" -gt 360 ] && { [ "$wake" = RUNNING ] || [ "$ack" = PENDING ]; }; then
      echo "agent health: DEGRADED (wake/ACK made no progress for ${health_age}s)"
    elif [ "$reply_pending" -gt 0 ] && [ "$reply_age" -gt 60 ]; then
      echo "agent health: DEGRADED ($reply_pending required reply/replies pending for ${reply_age}s)"
    elif [ "$wake" = FAILED ] || [ "$wake" = RESTARTING ]; then
      echo "agent health: DEGRADED (wake=$wake)"
    elif [ "$reply_pending" -gt 0 ]; then
      echo "agent health: IN_PROGRESS ($reply_pending required reply/replies pending)"
    else
      echo "agent health: OK (transport and wake state are separate)"
    fi
    echo "wake health: $wake"
    echo "reply health: $reply"
    [ "$runtime" != codex-v2 ] || echo "reply required pending: $reply_pending"
    echo "ack health: $ack"
    echo "health age: ${health_age}s"
  elif [ "$runtime" = manual ]; then
    echo "wake health: DEGRADED (manual runtime has no automatic wake)"
    echo "reply health: UNVERIFIED"
    echo "ack health: PENDING"
  else
    echo "wake health: UNVERIFIED (no consumer health record yet)"
    echo "reply health: UNVERIFIED"
    echo "ack health: UNVERIFIED"
  fi
  if [ "$runtime" = codex-v2 ] && [ "$relay_mode" = shadow ]; then
    if [ -f "$shadow_health_file" ]; then
      echo "v2 shadow ingestion: $(jq -r '.ingestion // "UNVERIFIED"' "$shadow_health_file" 2>/dev/null || echo UNVERIFIED)"
      echo "v2 shadow accepted: $(jq -r '.last_accepted_attention_id // "none"' "$shadow_health_file" 2>/dev/null || echo none)"
      echo "v2 shadow relay: DISABLED (v1 is the sole responder)"
    else
      echo "v2 shadow health: UNVERIFIED (no shadow health record yet)"
    fi
  fi
  encoded_name=$(python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$name")
  duplicate_count=$(python3 - "$encoded_name" <<'PY'
import pathlib
import sys

needle = f"name={sys.argv[1]}"
count = 0
for command_file in pathlib.Path("/proc").glob("[0-9]*/cmdline"):
    try:
        argv = command_file.read_bytes().split(b"\0")
    except OSError:
        continue
    decoded = [part.decode(errors="replace") for part in argv if part]
    if len(decoded) > 1 and pathlib.Path(decoded[1]).name == "listen.py":
        if any(needle in part for part in decoded):
            count += 1
print(count)
PY
)
  if [ "$duplicate_count" -le 1 ]; then
    echo "duplicate: NONE"
  else
    echo "duplicate: DEGRADED ($duplicate_count listeners for this identity)"
  fi
  server_version=$(curl -fsS -m 5 "$server/health" 2>/dev/null | jq -r '.version // "unknown"' 2>/dev/null || echo unknown)
  skill_version=$(tr -d '[:space:]' < "$SKILL_DIR/VERSION" 2>/dev/null || true)
  adapter_version=1
  [ "$runtime" != codex-v2 ] || adapter_version=2
  echo "version: server=${server_version:-unknown} skill=${skill_version:-unknown} adapter=$adapter_version"
  LOCA_ENV="$env_file" "$SKILL_DIR/connect.sh" status "$server" "$name"
  if [ -f "$log_file" ]; then
    echo "── last listener lines ──"
    tail -10 "$log_file"
  fi
}

command="${1:-}"
[ -n "$command" ] || { usage; exit 2; }
shift
case "$command" in
  start) start_runtime "$@" ;;
  status) status_runtime "$@" ;;
  stop)
    [ "$#" -eq 1 ] || { usage; exit 2; }
    stop_runtime "$1"
    ;;
  *) usage; exit 2 ;;
esac
