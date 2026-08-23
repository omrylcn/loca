#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
SERVER_URL=""
OUTPUT="$ROOT/.env"

usage() {
  cat <<'EOF'
Usage: ./scripts/init-self-host.sh --server-url https://loca.example.com [--output PATH]

Creates a production compose environment with a cryptographically random
master secret and mode 0600. It refuses to overwrite an existing file.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --server-url)
      SERVER_URL="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! [[ "$SERVER_URL" =~ ^https://([A-Za-z0-9._-]+|\[[0-9A-Fa-f:]+\])(:[0-9]+)?$ ]]; then
  echo "--server-url must be an https origin without a path or credentials" >&2
  exit 2
fi
if [ -e "$OUTPUT" ] || [ -L "$OUTPUT" ]; then
  echo "refusing to overwrite existing file: $OUTPUT" >&2
  exit 2
fi

random_hex() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 32
  else
    od -An -N32 -tx1 /dev/urandom | tr -d ' \n'
  fi
}

mkdir -p "$(dirname "$OUTPUT")"
TEMP_FILE="${OUTPUT}.tmp.$$"
trap 'rm -f "$TEMP_FILE"' EXIT
umask 077
{
  printf 'ADMIN_TOKEN=adm_%s\n' "$(random_hex)"
  printf 'PUBLIC_SERVER_URL=%s\n' "$SERVER_URL"
} > "$TEMP_FILE"
chmod 600 "$TEMP_FILE"
mv "$TEMP_FILE" "$OUTPUT"
trap - EXIT

echo "created $OUTPUT (mode 0600)"
echo "next: docker compose config && docker compose up --build -d"
