#!/usr/bin/env bash
# Build the per-skill distribution bundles — the SINGLE source of truth for the
# skill files a user installs. Both the server download endpoint (Web) and the
# Desktop Host's local Skill Library serve/extract THESE exact bytes, so the two
# surfaces hand out identical, checksummed packages.
#
#   usage: build-skill-bundles.sh [OUTPUT_DIR]
#
# OUTPUT_DIR defaults to dist/skill-bundles. Each caller passes its OWN directory
# (build.rs uses Cargo's OUT_DIR; the packaging test uses a temp dir), so
# concurrent builds/tests never clobber each other. Every archive is published
# atomically (staged in a sibling temp dir, then `mv`-renamed into place), so a
# concurrent reader never sees a half-written file.
#
# For each skill, under OUTPUT_DIR:
#   <name>.zip            deterministic, content-addressable archive
#   <name>.manifest.json  { name, skill_version, app_version, bundle_sha256,
#                           files: [ { path, sha256 } ] }
#
# The file set is EXACTLY the git-tracked runtime files of the skill (never an
# untracked, generated, or secret file that merely happens to sit in the tree),
# minus tests. A secondary gate refuses any real credential-shaped token.
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OUT="${1:-$ROOT/dist/skill-bundles}"
APP_VERSION=$("$ROOT/scripts/version.sh")

# ZIP's DOS timestamp floor is 1980-01-01; a fixed default keeps the artifact
# content-addressable even when an unrelated commit changes HEAD's timestamp.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-315532800}
case "$SOURCE_DATE_EPOCH" in
  ''|*[!0-9]*) echo "SOURCE_DATE_EPOCH must be an integer Unix timestamp" >&2; exit 2 ;;
esac

if ! git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "build-skill-bundles.sh requires a git work tree (it ships only tracked files)" >&2
  exit 2
fi

# name -> source skill directory under skill/.
declare -A SKILL_SRC=(
  [loca]="skill/agent-room"
  [loca-care]="skill/loca-care"
)

mkdir -p "$OUT"

build_one() {
  local name="$1" srcrel="${SKILL_SRC[$1]}"
  # Stage inside OUT so the final `mv` is an atomic same-filesystem rename.
  local stage; stage=$(mktemp -d "$OUT/.build.XXXXXX")
  # shellcheck disable=SC2064
  trap "rm -rf '$stage'" RETURN
  local kit="$stage/$name"
  mkdir -p "$kit"

  # ONLY git-tracked runtime files — an untracked/generated/secret file that
  # merely sits in the tree is never shipped. Tests and caches are excluded.
  local rel
  while IFS= read -r -d '' path; do
    rel=${path#"$srcrel"/}
    case "$rel" in
      tests/*|*/tests/*|*.pyc|*/__pycache__/*) continue ;;
      *.env|.env|*.request|.request)
        # Credential-container files never belong in a distributed skill.
        echo "REFUSING: credential-container file '$path' must not be in a skill bundle" >&2
        exit 5 ;;
    esac
    # A tracked symlink would make `cp` dereference its target and pull content
    # from OUTSIDE the skill tree into the bundle — refuse it outright.
    if [ -L "$ROOT/$path" ]; then
      echo "REFUSING: '$path' is a symlink; skill bundles must ship only regular files" >&2
      exit 4
    fi
    mkdir -p "$kit/$(dirname "$rel")"
    cp "$ROOT/$path" "$kit/$rel"
    # Preserve the source's executable bit deterministically.
    if [ -x "$ROOT/$path" ]; then chmod 700 "$kit/$rel"; else chmod 644 "$kit/$rel"; fi
  done < <(git -C "$ROOT" ls-files -z -- "$srcrel")
  find "$kit" -type d -exec chmod 755 {} +

  # Credential gate: the CENTRAL policy (scripts/credential_scan.py) scans for
  # EVERY real token class the server mints plus any bound-credential assignment,
  # and names only the offending file — never the value. Tracked-only staging
  # already blocks untracked junk; this blocks a credential that slipped into a
  # tracked file.
  if ! python3 "$ROOT/scripts/credential_scan.py" "$kit" >&2; then
    echo "REFUSING: a credential-shaped value was found in the '$name' bundle" >&2
    exit 3
  fi

  # Deterministic archive: fixed mtimes + sorted entries + no extra fields.
  find "$kit" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
  local zip="$stage/$name.zip"
  ( cd "$stage"
    mapfile -d '' -t entries < <(find "$name" -type f -print0 | LC_ALL=C sort -z)
    TZ=UTC zip -qX "$zip" "${entries[@]}" )

  # Manifest via a safe JSON serializer (no hand-built JSON / escaping bugs).
  python3 "$ROOT/scripts/skill_manifest.py" "$name" "$APP_VERSION" "$kit" "$zip" \
    > "$stage/$name.manifest.json"

  # Publish atomically: rename into place (same filesystem, so no partial file
  # is ever visible; a concurrent identical build merely re-writes equal bytes).
  mv -f "$zip" "$OUT/$name.zip"
  mv -f "$stage/$name.manifest.json" "$OUT/$name.manifest.json"
  echo "$name -> $OUT/$name.zip"
}

for name in "${!SKILL_SRC[@]}"; do
  build_one "$name"
done
