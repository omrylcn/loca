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

# name -> source skill directory under skill/.
declare -A SKILL_SRC=(
  [loca]="skill/agent-room"
  [loca-care]="skill/loca-care"
)

# The file set to ship. Prefer the COMMITTED manifest — it needs no .git, so this
# runs inside a Docker build context that has no repository. Fall back to git only
# when the manifest is absent (a fresh tree before it was generated, or a test
# fixture repo). The manifest is kept honest by the packaging test, which asserts
# it equals `git ls-files` (see scripts/skill_bundle_files.py); a Docker build can
# therefore never ship a stale or partial set.
MANIFEST_LIST="$ROOT/scripts/skill-bundle-files.txt"
ALL_FILES=()
if [ -f "$MANIFEST_LIST" ]; then
  while IFS= read -r line; do
    case "$line" in ''|'#'*) continue ;; esac
    ALL_FILES+=("$line")
  done < "$MANIFEST_LIST"
elif git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  while IFS= read -r line; do
    [ -n "$line" ] && ALL_FILES+=("$line")
  done < <(python3 "$ROOT/scripts/skill_bundle_files.py" "$ROOT")
else
  echo "build-skill-bundles.sh needs either $MANIFEST_LIST or a git work tree" >&2
  exit 2
fi

mkdir -p "$OUT"

build_one() {
  local name="$1" srcrel="${SKILL_SRC[$1]}"
  # Stage inside OUT so the final `mv` is an atomic same-filesystem rename.
  local stage; stage=$(mktemp -d "$OUT/.build.XXXXXX")
  # shellcheck disable=SC2064
  trap "rm -rf '$stage'" RETURN
  local kit="$stage/$name"
  mkdir -p "$kit"

  # The pre-selected file set (ALL_FILES) already excludes tests/caches; here we
  # take this skill's files and REFUSE the two things that must never ship even if
  # somehow listed: a credential-container file, and a symlink (which `cp` would
  # dereference, pulling content from OUTSIDE the skill tree into the bundle).
  local path rel
  for path in "${ALL_FILES[@]}"; do
    case "$path" in "$srcrel"/*) ;; *) continue ;; esac
    rel=${path#"$srcrel"/}
    case "$rel" in
      *.env|.env|*.request|.request)
        echo "REFUSING: credential-container file '$path' must not be in a skill bundle" >&2
        exit 5 ;;
    esac
    if [ -L "$ROOT/$path" ]; then
      echo "REFUSING: '$path' is a symlink; skill bundles must ship only regular files" >&2
      exit 4
    fi
    # A manifest entry that is not a real file in the tree must fail loudly rather
    # than silently shipping an incomplete bundle (e.g. a stale manifest, or a
    # file not copied into the Docker build context).
    if [ ! -f "$ROOT/$path" ]; then
      echo "REFUSING: '$path' is listed for the bundle but missing from the tree" >&2
      exit 6
    fi
    mkdir -p "$kit/$(dirname "$rel")"
    cp "$ROOT/$path" "$kit/$rel"
    # Preserve the source's executable bit deterministically.
    if [ -x "$ROOT/$path" ]; then chmod 700 "$kit/$rel"; else chmod 644 "$kit/$rel"; fi
  done
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
