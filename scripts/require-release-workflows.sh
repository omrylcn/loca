#!/usr/bin/env bash
set -euo pipefail

sha=${1:?usage: require-release-workflows.sh COMMIT_SHA}
required=("CI" "Security gates" "Dependency security audit")
deadline=$((SECONDS + 1800))

while (( SECONDS < deadline )); do
  runs=$(gh api \
    -H "Accept: application/vnd.github+json" \
    "/repos/${GITHUB_REPOSITORY}/actions/runs?head_sha=${sha}&per_page=100")
  pending=0

  for workflow in "${required[@]}"; do
    record=$(jq -r --arg workflow "$workflow" '
      [.workflow_runs[] | select(.name == $workflow)]
      | sort_by(.created_at) | last
      | if . == null then "missing\t\t0" else [.status, (.conclusion // ""), (.run_attempt // 1)] | @tsv end
    ' <<<"$runs")
    IFS=$'\t' read -r status conclusion attempt <<<"$record"

    if (( attempt > 1 )); then
      echo "release prerequisite was rerun: $workflow (attempt $attempt)" >&2
      exit 1
    fi

    case "$status:$conclusion" in
      completed:success)
        echo "release prerequisite passed: $workflow"
        ;;
      completed:*)
        echo "release prerequisite failed: $workflow ($conclusion)" >&2
        exit 1
        ;;
      *)
        echo "release prerequisite pending: $workflow ($status)"
        pending=1
        ;;
    esac
  done

  (( pending == 0 )) && exit 0
  sleep 15
done

echo "timed out waiting for release prerequisite workflows" >&2
exit 1
