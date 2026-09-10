#!/usr/bin/env bash
# Usage:
#   forbid-comment-markers.sh            scan every tracked file
#   forbid-comment-markers.sh --staged   scan lines added by the staged diff
#
# Exit 1 on a match, 2 on a tool failure. The bracketed character keeps the
# pattern from matching its own spelling in this file.
set -euo pipefail

PATTERN='commentlin[t]'

report_tool_failure() {
  echo "forbid-comment-markers: $1 failed with exit $2" >&2
  exit 2
}

if [ "${1:-}" = "--staged" ]; then
  # `>` marks added content, which distinguishes it from `+++` file headers
  # and from added lines that begin with `+`.
  diff=$(git diff --cached -U0 --no-color --output-indicator-new='>')
  status=0
  matches=$(printf '%s\n' "$diff" | grep -E '^>' | grep -n -i -E -e "$PATTERN") || status=$?
  if [ "$status" -eq 0 ]; then
    echo "forbidden comment marker in staged changes:" >&2
    echo "$matches" >&2
    echo "Remove the marker and stage the file again." >&2
    exit 1
  fi
  if [ "$status" -ne 1 ]; then
    report_tool_failure "grep" "$status"
  fi
  exit 0
fi

status=0
git grep -n -I -i -E -e "$PATTERN" -- . || status=$?
if [ "$status" -eq 0 ]; then
  echo "forbidden comment marker found in tracked files" >&2
  exit 1
fi
if [ "$status" -ne 1 ]; then
  report_tool_failure "git grep" "$status"
fi
