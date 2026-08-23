#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_dir"

files=(
  crates/server/src/hub.rs
  crates/server/src/main.rs
  crates/server/src/store.rs
  web/index.html
  crates/server/tests/ws_roundtrip.rs
)

printf 'commit %s\n' "$(git rev-parse HEAD)"
printf '%-40s %8s %10s %10s\n' file lines functions unwraps
for file in "${files[@]}"; do
  lines="$(wc -l < "$file" | tr -d ' ')"
  functions=0
  unwraps=0
  if [[ "$file" == *.rs ]]; then
    functions="$(rg -c '^\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn\s+[A-Za-z0-9_]+' "$file" || true)"
    unwraps="$(
      (rg -o '\.unwrap\(\)' "$file" || true) | wc -l | tr -d ' '
    )"
  fi
  printf '%-40s %8s %10s %10s\n' "$file" "$lines" "$functions" "$unwraps"
done

production_unwraps="$(
  (rg -o '\.unwrap\(\)' crates/server/src -g '*.rs' || true) \
    | wc -l \
    | tr -d ' '
)"
printf 'production_server_unwraps %s\n' "$production_unwraps"

if [[ "${1:-}" == "--tests" ]]; then
  cargo test --workspace -- --list \
    | rg ': test$' \
    | wc -l \
    | awk '{ print "rust_tests " $1 }'
fi
