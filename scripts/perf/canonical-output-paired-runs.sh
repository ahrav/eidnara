#!/usr/bin/env bash
# Frozen schedule: odd pairs run A then B, even pairs run B then A, where A is the
# baseline binary and B is the candidate. Every run is a fresh process.
set -euo pipefail

TOOLCHAIN="${EIDNARA_TOOLCHAIN:-1.98}"
MICRO_SAMPLES="${EIDNARA_MICRO_SAMPLES:-200}"
TRANSFORM_SAMPLES="${EIDNARA_TRANSFORM_SAMPLES:-30}"
PAIRS=10
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
# tree and build cache untouched.
build_revision() {
  local rev="$1" scratch="$2" label="$3"
  local sha
  sha=$(git rev-parse --verify "${rev}^{commit}")
  local tree="$scratch/worktree-$label"
  git worktree add --detach --quiet "$tree" "$sha"
  (
    cd "$tree"
    cargo "+$TOOLCHAIN" build --release -p daemon --features "$FEATURES" \
      --example "$EXAMPLE" --locked \
      --target-dir "$scratch/target-$label" >&2
  )
  git worktree remove --force "$tree"
  local binary="$scratch/target-$label/release/examples/$EXAMPLE"
  echo "$binary|$sha"
}

run_driver() {
  local binary="$1" label="$2" sha="$3" out="$4"
  "$binary" --label "$label" --commit "$sha" \
    --micro-samples "$MICRO_SAMPLES" --transform-samples "$TRANSFORM_SAMPLES" \
    --out "$out"
}

write_provenance() {
  local out_dir="$1" mode="$2" schedule="$3"
  shift 3
  local harness_rev dirty
  harness_rev=$(git rev-parse HEAD)
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then dirty=true; else dirty=false; fi
  {
    echo "{"
    echo "  \"kind\": \"canonical-output-paired-runs/v1\","
    echo "  \"mode\": \"$mode\","
    echo "  \"harness_revision\": \"$harness_rev\","
    echo "  \"harness_dirty\": $dirty,"
    echo "  \"toolchain\": \"$(cargo "+$TOOLCHAIN" --version)\","
    echo "  \"rustc\": \"$(rustc "+$TOOLCHAIN" --version)\","
    echo "  \"features\": \"$FEATURES\","
    echo "  \"profile\": \"release\","
    echo "  \"micro_samples\": $MICRO_SAMPLES,"
    echo "  \"transform_samples\": $TRANSFORM_SAMPLES,"
    echo "  \"pairs\": $PAIRS,"
    echo "  \"cpu_model\": \"$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ *//')\","
    echo "  \"nproc\": $(nproc),"
    echo "  \"kernel\": \"$(uname -sr)\","
    echo "  \"schedule\": $schedule,"
    while [ $# -gt 0 ]; do
      echo "  $1"
      shift
    done
    echo "  \"unix_time_s\": $(date +%s)"
    echo "}"
  } >"$out_dir/provenance.json"
}

main() {
  [ $# -ge 3 ] || usage
  local mode="$1"
  cd "$(repo_root)"
  local scratch
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/eidnara-canonical-output.XXXXXX")
  trap 'rm -rf "$scratch"' EXIT

  case "$mode" in
    baseline)
      [ $# -eq 3 ] || usage
      local rev="$2" out_dir="$3"
      mkdir -p "$out_dir"
      local built binary sha
      built=$(build_revision "$rev" "$scratch" A)
      binary="${built%|*}"
      sha="${built#*|}"
      local schedule="["
      for i in $(seq 1 $PAIRS); do
        local run
        run=$(printf 'run-%02d' "$i")
        run_driver "$binary" "baseline/$run" "$sha" "$out_dir/$run.json"
        schedule+="\"$run:A\""
        [ "$i" -lt $PAIRS ] && schedule+=","
      done
      schedule+="]"
      write_provenance "$out_dir" baseline "$schedule" \
        "\"baseline_commit\": \"$sha\","
      ;;
    paired)
      [ $# -eq 4 ] || usage
      local base_rev="$2" cand_rev="$3" out_dir="$4"
      mkdir -p "$out_dir"
      local built_a built_b bin_a sha_a bin_b sha_b
      built_a=$(build_revision "$base_rev" "$scratch" A)
      bin_a="${built_a%|*}"
      sha_a="${built_a#*|}"
      built_b=$(build_revision "$cand_rev" "$scratch" B)
      bin_b="${built_b%|*}"
      sha_b="${built_b#*|}"
      local schedule="["
      for i in $(seq 1 $PAIRS); do
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
        schedule+="\"$pair:$order\""
        [ "$i" -lt $PAIRS ] && schedule+=","
      done
      schedule+="]"
      write_provenance "$out_dir" paired "$schedule" \
        "\"baseline_commit\": \"$sha_a\"," \
        "\"candidate_commit\": \"$sha_b\","
      ;;
    *)
      usage
      ;;
  esac
  echo "wrote $out_dir"
}

main "$@"
