#!/usr/bin/env bash
# Usage:
#   forbid-tool-literals.sh            scan every tracked daemon source file
#   forbid-tool-literals.sh --staged   scan lines added by the staged diff
#
# Exit 1 on a match, 2 on a tool failure. Tool names and argument keys are
# vocabulary: they live in crates/daemon/src/tool_vocabulary.rs and nowhere
# else in daemon production source. The scan skips that module, `*_tests.rs`
# files, `tests/` and `testdata/` directories, and every top-level
# `#[cfg(test)]` module region, from its attribute to the next `}` in column 0
# (rustfmt places a top-level item's closing brace there). A distinctive
# literal opens a string literal or a JSON key inside one; the bracketed
# characters keep the pattern from matching its own spelling in this file.
set -euo pipefail

PATTERN='"(fil[e]Path|fil[e]_path|ol[d]String|ne[w]String|ol[d]_string|ne[w]_string|bas[h]_status|bas[h]_kill|tod[o]write|appl[y]_patch|af[t]_outline|af[t]_zoom|af[t]_search)([^A-Za-z0-9_]|$)|"mc[p]_'
MODULE='crates/daemon/src/tool_vocabulary.rs'

report_tool_failure() {
  echo "forbid-tool-literals: $1 failed with exit $2" >&2
  exit 2
}

# Every path below is relative to the repository root, whatever the caller's
# working directory.
status=0
root=$(git rev-parse --show-toplevel) || status=$?
[ "$status" -eq 0 ] || report_tool_failure "git rev-parse" "$status"
cd "$root"

# A `case` glob's `*` also matches `/`, so the last pattern covers every depth.
scanned() {
  case "$1" in
    "$MODULE" | *_tests.rs | */tests/* | */testdata/*) return 1 ;;
    crates/daemon/src/*.rs) return 0 ;;
    *) return 1 ;;
  esac
}

# Prints `line<TAB>text` for every line outside a top-level `#[cfg(test)]`
# module region whose text matches the pattern.
scan_file() {
  awk -v pattern="$PATTERN" '
    region { if ($0 ~ /^}/) region = 0; next }
    /^#\[/ { if ($0 ~ /^#\[cfg\(test\)\]/) pending = 1 }
    /^(pub(\(crate\))? )?mod [A-Za-z0-9_]+ \{/ { if (pending) { region = 1; pending = 0; next } }
    !/^#\[/ { pending = 0 }
    $0 ~ pattern { print FNR "\t" $0 }
  ' "$1"
}

if [ "${1:-}" = "--staged" ]; then
  status=0
  files=$(git diff --cached --name-only --diff-filter=ACMR) || status=$?
  [ "$status" -eq 0 ] || report_tool_failure "git diff" "$status"
  found=0
  while IFS= read -r file; do
    if [ -z "$file" ] || ! scanned "$file"; then continue; fi
    # `>` marks added lines, so an added line that begins with `+` is still one.
    added=$(git diff --cached -U0 --no-color --output-indicator-new='>' -- "$file" | awk '
      /^@@ / { match($0, /\+[0-9]+/); line = substr($0, RSTART + 1, RLENGTH - 1) + 0; next }
      /^>/ { print line; line++ }
    ') || report_tool_failure "awk" "$?"
    [ -n "$added" ] || continue
    staged=$(mktemp)
    git show ":$file" > "$staged" || { rm -f "$staged"; report_tool_failure "git show" "$?"; }
    hits=$(scan_file "$staged") || { rm -f "$staged"; report_tool_failure "awk" "$?"; }
    rm -f "$staged"
    while IFS=$'\t' read -r line text; do
      [ -n "$line" ] || continue
      if printf '%s\n' "$added" | grep -qx "$line"; then
        echo "$file:$line: $text" >&2
        found=1
      fi
    done <<< "$hits"
  done <<< "$files"
  if [ "$found" -ne 0 ]; then
    echo "tool-vocabulary literal in staged daemon source; use $MODULE" >&2
    exit 1
  fi
  exit 0
fi

files=$(git ls-files -- 'crates/daemon/src') || status=$?
[ "$status" -eq 0 ] || report_tool_failure "git ls-files" "$status"
found=0
while IFS= read -r file; do
  if [ -z "$file" ] || ! scanned "$file"; then continue; fi
  hits=$(scan_file "$file") || report_tool_failure "awk" "$?"
  while IFS=$'\t' read -r line text; do
    [ -n "$line" ] || continue
    echo "$file:$line: $text" >&2
    found=1
  done <<< "$hits"
done <<< "$files"
if [ "$found" -ne 0 ]; then
  echo "tool-vocabulary literal in daemon production source; use $MODULE" >&2
  exit 1
fi
