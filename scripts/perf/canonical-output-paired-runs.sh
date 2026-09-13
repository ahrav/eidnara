#!/usr/bin/env bash
# Frozen schedule: odd pairs run A then B, even pairs run B then A, where A is the
# baseline binary and B is the candidate. Every run is a fresh process.
set -euo pipefail
# `build_revision` runs inside `$(...)`; without this, bash drops `errexit` in
# command substitutions and a failed build would go unnoticed until the driver
# is missing.
shopt -s inherit_errexit

TOOLCHAIN="${EIDNARA_TOOLCHAIN:-1.98}"
MICRO_SAMPLES="${EIDNARA_MICRO_SAMPLES:-200}"
TRANSFORM_SAMPLES="${EIDNARA_TRANSFORM_SAMPLES:-30}"
RUNS=10
FEATURES="bench-internals,test-support"
EXAMPLE="canonical_output_evidence"

usage() {
  cat >&2 <<'EOF'
usage:
  canonical-output-paired-runs.sh baseline <rev> <out-dir>
      Build <rev> in an isolated worktree and run the evidence driver in ten
      independent processes: run-01.json .. run-10.json plus provenance.json.
  canonical-output-paired-runs.sh paired <baseline-rev> <candidate-rev> <out-dir>
      Build both revisions and run ten process pairs on the frozen AB/BA
      schedule: pair-NN-A.json, pair-NN-B.json, plus provenance.json.
environment:
  EIDNARA_TOOLCHAIN (default 1.98)
  EIDNARA_MICRO_SAMPLES (default 200), EIDNARA_TRANSFORM_SAMPLES (default 30)
EOF
  exit 2
}

repo_root() {
  git rev-parse --show-toplevel
}

# The detached worktree and separate target directory keep the caller's working
# tree and build cache untouched. Prints the built binary path.
build_revision() {
  local sha="$1" label="$2"
  local tree="$scratch/worktree-$label"
  git worktree add --detach --quiet "$tree" "$sha"
  (
    cd "$tree" || exit 1
    cargo "+$TOOLCHAIN" build --release -p daemon --features "$FEATURES" \
      --example "$EXAMPLE" --locked \
      --target-dir "$scratch/target-$label" >&2
    cargo "+$TOOLCHAIN" tree -p daemon --features "$FEATURES" --locked \
      -e features --prefix none -f '{p} {f}' \
      | grep -E '^(daemon|memory-store|serde|serde_json) v' \
      | sed -E 's/ \(\*\)$//; s#\([^)]*\) ##' | sort -u \
      >"$scratch/features-$label.txt"
  )
  git worktree remove --force "$tree"
  echo "$scratch/target-$label/release/examples/$EXAMPLE"
}

run_driver() {
  local binary="$1" label="$2" sha="$3" out="$4"
  "$binary" --label "$label" --commit "$sha" \
    --micro-samples "$MICRO_SAMPLES" --transform-samples "$TRANSFORM_SAMPLES" \
    --out "$out"
}

json_string_array() {
  local first=1 item
  printf '['
  for item in "$@"; do
    [ "$first" -eq 1 ] || printf ','
    first=0
    printf '"%s"' "$item"
  done
  printf ']'
}

json_lines_array() {
  local file="$1"
  if [ -s "$file" ]; then
    mapfile -t lines <"$file"
    json_string_array "${lines[@]}"
  else
    printf '[]'
  fi
}

write_provenance() {
  local out_dir="$1" mode="$2" schedule="$3" baseline_sha="$4" candidate_sha="${5:-}"
  local harness_rev dirty
  harness_rev=$(git rev-parse HEAD)
  # Only sources that reach the built binaries decide dirtiness; evidence and
  # documentation edits do not.
  if [ -n "$(git status --porcelain --untracked-files=no -- Cargo.toml Cargo.lock crates scripts)" ]; then
    dirty=true
  else
    dirty=false
  fi
  {
    echo "{"
    echo "  \"kind\": \"canonical-output-paired-runs/v1\","
    echo "  \"mode\": \"$mode\","
    echo "  \"harness_revision\": \"$harness_rev\","
    echo "  \"harness_dirty\": $dirty,"
    echo "  \"toolchain\": \"$(cargo "+$TOOLCHAIN" --version)\","
    echo "  \"rustc\": \"$(rustc "+$TOOLCHAIN" --version)\","
    echo "  \"features\": \"$FEATURES\","
    echo "  \"resolved_features_baseline\": $(json_lines_array "$scratch/features-A.txt"),"
    if [ -n "$candidate_sha" ]; then
      echo "  \"resolved_features_candidate\": $(json_lines_array "$scratch/features-B.txt"),"
    fi
    echo "  \"profile\": \"release\","
    echo "  \"micro_samples\": $MICRO_SAMPLES,"
    echo "  \"transform_samples\": $TRANSFORM_SAMPLES,"
    echo "  \"runs\": $RUNS,"
    echo "  \"nproc\": $(nproc),"
    echo "  \"kernel\": \"$(uname -sr)\","
    echo "  \"schedule\": $schedule,"
    echo "  \"baseline_commit\": \"$baseline_sha\","
    if [ -n "$candidate_sha" ]; then
      echo "  \"candidate_commit\": \"$candidate_sha\","
    fi
    echo "  \"unix_time_s\": $(date +%s)"
    echo "}"
  } >"$out_dir/provenance.json"
}

main() {
  [ $# -ge 3 ] || usage
  local mode="$1"
  cd "$(repo_root)"
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/eidnara-canonical-output.XXXXXX")
  # A failed build leaves its worktree registered; prune it with the scratch dir.
  trap 'rm -rf "$scratch"; git worktree prune' EXIT

  case "$mode" in
    baseline)
      [ $# -eq 3 ] || usage
      local out_dir="$3" sha binary
      sha=$(git rev-parse --verify "$2^{commit}")
      mkdir -p "$out_dir"
      binary=$(build_revision "$sha" A)
      local -a schedule=()
      for i in $(seq 1 "$RUNS"); do
        local run
        run=$(printf 'run-%02d' "$i")
        run_driver "$binary" "baseline/$run" "$sha" "$out_dir/$run.json"
        schedule+=("$run:A")
      done
      write_provenance "$out_dir" baseline "$(json_string_array "${schedule[@]}")" "$sha"
      ;;
    paired)
      [ $# -eq 4 ] || usage
      local out_dir="$4" sha_a sha_b bin_a bin_b
      sha_a=$(git rev-parse --verify "$2^{commit}")
      sha_b=$(git rev-parse --verify "$3^{commit}")
      mkdir -p "$out_dir"
      bin_a=$(build_revision "$sha_a" A)
      bin_b=$(build_revision "$sha_b" B)
      local -a schedule=()
      for i in $(seq 1 "$RUNS"); do
        local pair order
        pair=$(printf 'pair-%02d' "$i")
        if [ $((i % 2)) -eq 1 ]; then order="AB"; else order="BA"; fi
        if [ "$order" = "AB" ]; then
          run_driver "$bin_a" "$pair/A" "$sha_a" "$out_dir/$pair-A.json"
          run_driver "$bin_b" "$pair/B" "$sha_b" "$out_dir/$pair-B.json"
        else
          run_driver "$bin_b" "$pair/B" "$sha_b" "$out_dir/$pair-B.json"
          run_driver "$bin_a" "$pair/A" "$sha_a" "$out_dir/$pair-A.json"
        fi
        schedule+=("$pair:$order")
      done
      write_provenance "$out_dir" paired "$(json_string_array "${schedule[@]}")" "$sha_a" "$sha_b"
      ;;
    *)
      usage
      ;;
  esac
  echo "wrote $out_dir"
}

main "$@"
