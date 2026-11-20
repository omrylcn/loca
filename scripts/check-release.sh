#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=$("$ROOT/scripts/version.sh")
TAG="${1:-}"

if ! [[ "$VERSION" =~ ^(0|[1-9][0-9]*)\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
  echo "invalid workspace version: $VERSION" >&2
  exit 1
fi

mapfile -t PACKAGE_VERSIONS < <(
  cargo metadata --manifest-path "$ROOT/Cargo.toml" --locked --no-deps --format-version 1 |
    jq -r '.packages[].version' |
    sort -u
)
if [ "${#PACKAGE_VERSIONS[@]}" -ne 1 ] || [ "${PACKAGE_VERSIONS[0]}" != "$VERSION" ]; then
  printf 'workspace package versions do not match canonical %s: %s\n' \
    "$VERSION" "${PACKAGE_VERSIONS[*]:-none}" >&2
  exit 1
fi

SKILL_VERSION=$(tr -d '[:space:]' < "$ROOT/skill/agent-room/VERSION")
if [ "$SKILL_VERSION" != "$VERSION" ]; then
  echo "skill version $SKILL_VERSION does not match canonical $VERSION" >&2
  exit 1
fi

CARE_SKILL_VERSION=$(tr -d '[:space:]' < "$ROOT/skill/loca-care/VERSION")
if [ "$CARE_SKILL_VERSION" != "$VERSION" ]; then
  echo "loca-care skill version $CARE_SKILL_VERSION does not match canonical $VERSION" >&2
  exit 1
fi

DESKTOP_VERSION=$(jq -r '.version // empty' "$ROOT/desktop/src-tauri/tauri.conf.json")
if [ "$DESKTOP_VERSION" != "$VERSION" ]; then
  echo "desktop version $DESKTOP_VERSION does not match canonical $VERSION" >&2
  exit 1
fi

DESKTOP_SKILL_VERSION=$(awk '
  $0 == "name = \"skill-bundles\"" { in_package = 1; next }
  in_package && /^version = / {
    value = $0
    sub(/^[^=]*=[[:space:]]*"/, "", value)
    sub(/".*$/, "", value)
    print value
    exit
  }
' "$ROOT/desktop/src-tauri/Cargo.lock")
if [ -z "$DESKTOP_SKILL_VERSION" ] || [ "$DESKTOP_SKILL_VERSION" != "$VERSION" ]; then
  echo "desktop skill-bundles lock version ${DESKTOP_SKILL_VERSION:-missing} does not match canonical $VERSION" >&2
  exit 1
fi

# The desktop is a separate Cargo workspace with its own lock. A textual
# version check is not enough: this command detects every stale transitive or
# path-package lock entry without rewriting the release candidate.
if ! (cd "$ROOT/desktop/src-tauri" && cargo metadata --locked --no-deps --format-version 1 >/dev/null); then
  echo "desktop Cargo.lock is stale; refresh it before release" >&2
  exit 1
fi

if ! grep -Fq "## [$VERSION]" "$ROOT/CHANGELOG.md"; then
  echo "CHANGELOG.md has no section for $VERSION" >&2
  exit 1
fi

if grep -Eq 'include_str!\("(\.\./)+docs/' "$ROOT"/crates/server/src/main.rs "$ROOT"/crates/server/src/routes/static_assets.rs \
  && ! grep -Eq '^COPY[[:space:]]+docs[[:space:]]+docs$' "$ROOT/Dockerfile"; then
  echo "Dockerfile must copy docs used by server include_str! assets" >&2
  exit 1
fi

# The server release workflow must leave its release as a draft. The action's
# default publishes at once, which would skip the check of the built assets
# and the separate approval that publication needs.
RELEASE_WORKFLOW="$ROOT/.github/workflows/release.yml"
if [ -f "$RELEASE_WORKFLOW" ]; then
  publish_steps=$(grep -c 'uses: softprops/action-gh-release@' "$RELEASE_WORKFLOW" || true)
  draft_steps=$(awk '
    /^      - / { in_step = 0 }
    /uses: softprops\/action-gh-release@/ { in_step = 1 }
    in_step && /^[[:space:]]+draft:[[:space:]]*true[[:space:]]*$/ { count++; in_step = 0 }
    END { print count + 0 }
  ' "$RELEASE_WORKFLOW")
  if [ "$publish_steps" -lt 1 ] || [ "$draft_steps" -ne "$publish_steps" ]; then
    echo "release.yml must create its GitHub Release with draft: true ($draft_steps of $publish_steps release steps do)" >&2
    exit 1
  fi
fi

# Every workflow the release waits for must run on every push to master. The
# release looks for each of them on the tagged commit; one that is filtered
# by paths never runs on a commit that does not touch those paths, and the
# release then waits half an hour for a run that cannot come and fails
# (v0.14.0: the dependency audit ran only when a Cargo file changed).
REQUIRED_WORKFLOWS="$ROOT/scripts/require-release-workflows.sh"
if [ -f "$REQUIRED_WORKFLOWS" ] && [ -d "$ROOT/.github/workflows" ]; then
  python3 - "$REQUIRED_WORKFLOWS" "$ROOT/.github/workflows" <<'PY'
import pathlib, re, sys

script, workflows = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
listed = re.search(r'^required=\(([^)]*)\)', script.read_text(), re.M)
if not listed:
    sys.exit("require-release-workflows.sh: no required=(...) list")
required = re.findall(r'"([^"]+)"', listed.group(1))
by_name = {}
for path in sorted(workflows.glob("*.yml")):
    text = path.read_text()
    name = re.search(r'^name:\s*(.+?)\s*$', text, re.M)
    if name:
        by_name[name.group(1).strip('"\'')] = (path.name, text)
bad = []
for name in required:
    if name not in by_name:
        bad.append(f"{name}: no workflow has this name")
        continue
    file, text = by_name[name]
    push = re.search(r'^  push:[ \t]*\n((?:    .*\n|[ \t]*\n)*)', text, re.M)
    if not push:
        bad.append(f"{file}: does not run on push")
    elif not re.search(r'^    branches:.*\bmaster\b', push.group(1), re.M):
        bad.append(f"{file}: its push trigger does not name master")
    elif re.search(r'^    paths(-ignore)?:', push.group(1), re.M):
        bad.append(f"{file}: its push trigger is filtered by paths")
if bad:
    print("a workflow the release waits for must run on every push to master:", file=sys.stderr)
    for line in bad:
        print("  " + line, file=sys.stderr)
    sys.exit(1)
PY
fi

# The desktop test build (no release) must build the packages exactly as the
# release does: the marked build steps, the tauri-action pin, the baked server
# and the flavor arguments are compared line for line.
DESKTOP_RELEASE="$ROOT/.github/workflows/desktop-release.yml"
DESKTOP_TEST_BUILD="$ROOT/.github/workflows/desktop-test-build.yml"
if [ -f "$DESKTOP_TEST_BUILD" ]; then
  desktop_build_steps() {
    awk '/# >>> desktop build steps/ { on = 1 } on { print } /# <<< desktop build steps/ { on = 0 }' "$1"
    grep -E 'uses: tauri-apps/tauri-action@|LOCA_DEFAULT_SERVER:' "$1"
    awk '/^[[:space:]]+args: >-[[:space:]]*$/ { left = 4 } left > 0 { print; left-- }' "$1"
  }
  release_steps=$(desktop_build_steps "$DESKTOP_RELEASE")
  test_steps=$(desktop_build_steps "$DESKTOP_TEST_BUILD")
  if ! grep -q '# <<< desktop build steps' <<<"$release_steps" \
    || [ "$release_steps" != "$test_steps" ]; then
    echo "desktop-test-build.yml must build exactly as desktop-release.yml (marked steps, tauri-action, LOCA_DEFAULT_SERVER, args):" >&2
    diff <(printf '%s\n' "$release_steps") <(printf '%s\n' "$test_steps") >&2 || true
    exit 1
  fi
fi

if [ -n "$TAG" ] && [ "$TAG" != "v$VERSION" ]; then
  echo "tag $TAG does not match canonical version v$VERSION" >&2
  exit 1
fi

if [ -n "$TAG" ] && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  tag_commit=$(git rev-list -n 1 "$TAG" 2>/dev/null || printf '%s' "${GITHUB_SHA:-HEAD}")
  master_ref=origin/master
  git show-ref --verify --quiet refs/remotes/origin/master || master_ref=master
  if ! git merge-base --is-ancestor "$tag_commit" "$master_ref"; then
    echo "release tag $TAG is not on $master_ref" >&2
    exit 1
  fi
  if git rev-parse -q --verify "$TAG^{tag}" >/dev/null 2>&1; then
    git -c gpg.format=ssh \
      -c gpg.ssh.allowedSignersFile="$ROOT/.github/release-signers" \
      verify-tag "$TAG"
  elif git rev-parse -q --verify "$TAG" >/dev/null 2>&1; then
    echo "release tag $TAG must be an annotated signed tag" >&2
    exit 1
  fi
fi

echo "release metadata ok: v$VERSION"
