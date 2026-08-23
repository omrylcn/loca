#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
SERVER=""
NAME=""
TARGET="both"
INSTALL_DEPS=0
START_AFTER=1
HOSTED=0
MODE=install

usage() {
  cat <<'EOF'
Usage: ./install.sh --name NAME [options]
       ./install.sh --upgrade-only [--target claude|codex|both]
       ./install.sh --rollback [--target claude|codex|both]

Options:
  --server URL       Explicit self-hosted Loca server origin
  --hosted           Use the official invite-only hosted Loca server
  --target TARGET    claude | codex | both (default: both)
  --upgrade-only     Upgrade skill files; keep identities and listeners intact
  --rollback         Restore the most recent skill backup
  --install-deps     Install missing python3/curl/jq with the system package manager
  --no-start         Install and onboard, but do not start the lobby listener
  -h, --help         Show this help

The membership/davet is read from a hidden prompt; it is never accepted as a
command-line argument. Choose exactly one of --server or --hosted; the
installer never contacts a remote host by default. Upgrade and rollback are
local-only and never ask for or change credentials.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --name) NAME="${2:-}"; shift 2 ;;
    --server)
      [ "$HOSTED" -eq 0 ] || {
        echo "--server and --hosted cannot be used together" >&2
        exit 2
      }
      SERVER="${2:-}"
      shift 2
      ;;
    --hosted)
      [ -z "$SERVER" ] || {
        echo "--server and --hosted cannot be used together" >&2
        exit 2
      }
      HOSTED=1
      SERVER="https://loca.speakbetter.tech"
      shift
      ;;
    --target) TARGET="${2:-}"; shift 2 ;;
    --upgrade-only)
      [ "$MODE" = install ] || {
        echo "--upgrade-only and --rollback are mutually exclusive" >&2
        exit 2
      }
      MODE=upgrade
      shift
      ;;
    --rollback)
      [ "$MODE" = install ] || {
        echo "--upgrade-only and --rollback are mutually exclusive" >&2
        exit 2
      }
      MODE=rollback
      shift
      ;;
    --install-deps) INSTALL_DEPS=1; shift ;;
    --no-start) START_AFTER=0; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$TARGET" in claude|codex|both) ;; *)
  echo "--target must be claude, codex, or both" >&2
  exit 2
esac

if [ ! -d "$ROOT/loca" ] || [ ! -f "$ROOT/loca/SKILL.md" ] \
  || [ ! -d "$ROOT/loca-care" ] || [ ! -f "$ROOT/loca-care/SKILL.md" ]; then
  echo "package is incomplete: loca and loca-care skills are required" >&2
  exit 2
fi

install_one_skill() {
  local parent="$1" skill="$2" target new backup backup_root stamp
  parent="${parent%/}"
  target="$parent/$skill"
  new="$parent/.$skill.new.$$"
  backup_root=$(backup_root_for "$parent")
  stamp=$(date -u +%Y%m%dT%H%M%SZ)-$$
  mkdir -p "$parent"
  mkdir -p "$backup_root"
  migrate_legacy_backups "$parent" "$backup_root" "$skill"
  rm -rf "$new"
  cp -a "$ROOT/$skill" "$new"
  if [ -e "$target" ] || [ -L "$target" ]; then
    backup="$backup_root/$skill.backup.$stamp"
    mv "$target" "$backup"
    echo "existing skill backed up: $backup"
  fi
  mv "$new" "$target"
  if [ "$skill" = loca ]; then
    chmod 700 "$target/connect.sh" "$target/runtime.sh" "$target/nudge.py" \
      "$target/orchestrator_queue.py" "$target/runtime_agent.py" \
      "$target/runtime_consumer.py" "$target/monitor_listener.py" \
      "$target/attention_store.py" "$target/codex_adapter_v2.py"
  else
    chmod 700 "$target/scripts/audit.py"
  fi
  echo "skill installed: $target"
}

install_skill() {
  install_one_skill "$1" loca
  install_one_skill "$1" loca-care
}

backup_root_for() {
  local parent="$1" runtime
  runtime=$(basename "$(dirname "$parent")")
  runtime="${runtime#.}"
  printf '%s\n' "$HOME/.loca/skill-backups/$runtime"
}

migrate_legacy_backups() {
  local parent="$1" backup_root="$2" skill="$3" legacy destination
  mkdir -p "$backup_root"
  for legacy in "$parent"/"$skill".backup.* "$parent"/"$skill".replaced.*; do
    [ -e "$legacy" ] || [ -L "$legacy" ] || continue
    destination="$backup_root/$(basename "$legacy")"
    if [ -e "$destination" ] || [ -L "$destination" ]; then
      destination="$destination.migrated.$$"
    fi
    mv "$legacy" "$destination"
    echo "legacy skill backup moved out of discovery path: $destination"
  done
}

rollback_one_skill() {
  local parent="$1" skill="$2" target backup backup_root failed stamp
  parent="${parent%/}"
  target="$parent/$skill"
  backup_root=$(backup_root_for "$parent")
  migrate_legacy_backups "$parent" "$backup_root" "$skill"
  backup=$(
    find "$backup_root" -maxdepth 1 -type d -name "$skill.backup.*" \
      -printf '%f\n' 2>/dev/null |
      LC_ALL=C sort |
      tail -1
  )
  if [ -z "$backup" ]; then
    if [ "$skill" = loca-care ] && { [ -e "$target" ] || [ -L "$target" ]; }; then
      stamp=$(date -u +%Y%m%dT%H%M%SZ)-$$
      failed="$backup_root/$skill.replaced.$stamp"
      mv "$target" "$failed"
      echo "new $skill skill removed by rollback: $failed"
      return 0
    fi
    echo "no $skill backup found under $backup_root" >&2
    return 3
  fi
  backup="$backup_root/$backup"
  stamp=$(date -u +%Y%m%dT%H%M%SZ)-$$
  failed="$backup_root/$skill.replaced.$stamp"
  if [ -e "$target" ] || [ -L "$target" ]; then
    mv "$target" "$failed"
  fi
  if ! mv "$backup" "$target"; then
    [ ! -e "$failed" ] || mv "$failed" "$target"
    echo "rollback failed; current skill restored" >&2
    return 1
  fi
  echo "skill rolled back: $target"
  [ ! -e "$failed" ] || echo "replaced version retained: $failed"
}

rollback_skill() {
  local parent="$1"
  rollback_one_skill "$parent" loca-care
  rollback_one_skill "$parent" loca
}

for_each_target() {
  local action="$1"
  case "$TARGET" in
    claude) "$action" "$HOME/.claude/skills" ;;
    codex) "$action" "$HOME/.codex/skills" ;;
    both)
      "$action" "$HOME/.claude/skills"
      "$action" "$HOME/.codex/skills"
      ;;
  esac
}

if [ "$MODE" = upgrade ]; then
  for_each_target install_skill
  echo "skill upgrade complete; credentials and running listeners were not changed"
  echo "restart the selected runtime adapter to load the new code"
  exit 0
fi
if [ "$MODE" = rollback ]; then
  for_each_target rollback_skill
  echo "skill rollback complete; credentials and running listeners were not changed"
  echo "restart the selected runtime adapter to load the restored code"
  exit 0
fi

if [ -z "$SERVER" ]; then
  echo "choose a server explicitly: --server https://loca.example.com or --hosted" >&2
  exit 2
fi
if [ -z "$NAME" ]; then
  printf 'agent name: ' >&2
  read -r NAME
fi
if ! [[ "$NAME" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$ ]]; then
  echo "agent name must be 1-64 ASCII letters, digits, dot, dash, or underscore" >&2
  exit 2
fi
SERVER="${SERVER%/}"
if ! [[ "$SERVER" =~ ^https?://([A-Za-z0-9._-]+|\[[0-9A-Fa-f:]+\])(:[0-9]+)?$ ]]; then
  echo "--server must be an http(s) origin without a path or credentials" >&2
  exit 2
fi

missing=""
for cmd in python3 curl jq; do
  command -v "$cmd" >/dev/null 2>&1 || missing="$missing $cmd"
done

install_dependencies() {
  run_root() {
    if [ "$(id -u)" -eq 0 ]; then
      "$@"
    elif command -v sudo >/dev/null 2>&1; then
      sudo "$@"
    else
      echo "root or sudo is required to install dependencies" >&2
      exit 2
    fi
  }
  if command -v apt-get >/dev/null 2>&1; then
    run_root apt-get update
    run_root apt-get install -y python3 curl jq
  elif command -v dnf >/dev/null 2>&1; then
    run_root dnf install -y python3 curl jq
  elif command -v yum >/dev/null 2>&1; then
    run_root yum install -y python3 curl jq
  elif command -v apk >/dev/null 2>&1; then
    run_root apk add python3 curl jq
  else
    echo "unsupported package manager; install manually:$missing" >&2
    exit 2
  fi
}

if [ -n "$missing" ]; then
  if [ "$INSTALL_DEPS" -eq 1 ]; then
    install_dependencies
  else
    echo "missing dependencies:$missing" >&2
    echo "re-run with --install-deps or install them manually" >&2
    exit 2
  fi
fi

printf 'membership or davet for %s (input hidden): ' "$SERVER" >&2
IFS= read -r -s BOOTSTRAP_TOKEN
printf '\n' >&2
case "$BOOTSTRAP_TOKEN" in mb_*|dv_*) ;; *)
  echo "expected an mb_... membership or dv_... davet; nothing was changed" >&2
  exit 2
esac
if ! curl -fsS -m 10 "$SERVER/health" >/dev/null; then
  echo "cannot reach $SERVER; nothing was changed" >&2
  exit 1
fi
credential_kind=$(
  curl -fsS -m 10 -H "x-room-token: $BOOTSTRAP_TOKEN" "$SERVER/whoami" |
    jq -r '.kind // empty'
) || true
if [ -z "$credential_kind" ]; then
  echo "membership/davet was rejected; nothing was changed" >&2
  exit 1
fi

for_each_target install_skill

mkdir -p "$HOME/.local/bin"
install -m 700 "$ROOT/start-lobby.sh" "$HOME/.local/bin/loca-start"
install -m 700 "$ROOT/stop-lobby.sh" "$HOME/.local/bin/loca-stop"
install -m 700 "$ROOT/status.sh" "$HOME/.local/bin/loca-status"

SAFE_NAME=$(printf '%s' "$NAME" | tr -c 'A-Za-z0-9_-' '_')
ENV_FILE="$HOME/.loca/$SAFE_NAME.env"
mkdir -p "$HOME/.loca"
chmod 700 "$HOME/.loca"

SKILL_DIR="$HOME/.claude/skills/loca"
if [ "$TARGET" = "codex" ]; then
  SKILL_DIR="$HOME/.codex/skills/loca"
fi
printf '%s\n' "$BOOTSTRAP_TOKEN" |
  LOCA_ENV="$ENV_FILE" "$SKILL_DIR/connect.sh" setup "$SERVER" "$NAME"
unset BOOTSTRAP_TOKEN

if [ "$START_AFTER" -eq 1 ]; then
  LOCA_ENV="$ENV_FILE" LOCA_WORKDIR="$HOME" \
    "$HOME/.local/bin/loca-start" "$NAME" --runtime manual
  echo "delivery/presence is running; automatic model wake is not configured yet"
  echo "open the chosen agent runtime and invoke /loca or \$loca to bind wake-up"
else
  echo "installed without listener; start later with: loca-start '$NAME'"
fi

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "note: add $HOME/.local/bin to PATH to use loca-start/stop/status" ;;
esac
