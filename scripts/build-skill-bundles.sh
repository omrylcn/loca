#!/usr/bin/env bash
# Compatibility entry point. Cargo invokes the platform-neutral Python builder
# directly; existing Linux/Docker callers may keep using this command.
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
exec python3 "$ROOT/scripts/build_skill_bundles.py" "$@"
