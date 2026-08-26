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

if ! grep -Fq "## [$VERSION]" "$ROOT/CHANGELOG.md"; then
  echo "CHANGELOG.md has no section for $VERSION" >&2
  exit 1
fi

if grep -Fq 'include_str!("../../../docs/' "$ROOT/crates/server/src/main.rs" \
  && ! grep -Eq '^COPY[[:space:]]+docs[[:space:]]+docs$' "$ROOT/Dockerfile"; then
  echo "Dockerfile must copy docs used by server include_str! assets" >&2
  exit 1
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
