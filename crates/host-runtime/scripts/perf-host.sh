#!/usr/bin/env bash
set -euo pipefail

# The script lives in crates/host-runtime/scripts; ROOT is the workspace root.
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"

BUDGET_BENCH=""
BUDGET_CHILD=""
SHM_BENCH=""
# Offered-rate points shared by the plan preview and execution paths. The
# default string stays byte-identical to DEFAULT_RATES in
# crates/host-runtime/benches/ipc_budget.rs (parity-tested by
# crates/host-runtime/tests/perf_budget_runner.rs).
BUDGET_RATES="${BUDGET_RATES:-20000 50000 80000}"

# Builds one bench target and prints the executable path Cargo reports.
# Cargo's `Executable <src> (<path>)` line names the artifact under whatever
# target directory is configured (CARGO_TARGET_DIR, build.target-dir): relative
# to the build cwd when it lies beneath it, absolute otherwise. The path is
# taken as printed rather than reconstructed under $ROOT/target.
bench_binary() {
  local package="${1:?package}" bench="${2:?bench}" out path
  out=$(cd "$ROOT" && cargo bench -p "$package" --bench "$bench" --no-run --locked 2>&1) || {
    echo "$out"
    echo "$bench bench build failed" >&2
    exit 1
  }
  path=$(echo "$out" | sed -nE "s#^[[:space:]]*Executable [^(]*\(([^)]*/${bench}-[0-9a-f]+)\)\$#\1#p" | tail -1)
  [[ -z "$path" || "$path" = /* ]] || path="$ROOT/$path"
  [[ -n "$path" && -x "$path" ]] || {
    echo "could not locate $bench bench binary" >&2
    exit 1
  }
  printf '%s\n' "$path"
}

budget_build() {
  BUDGET_BENCH=$(bench_binary host-runtime ipc_budget)
}

shm_build() {
  SHM_BENCH=$(bench_binary shm-transport hardware_envelope)
}

shm_run() {
  local out="${1:?outdir}" mode="${2:?mode}" args=()
  # The fixed-ring harness rejects --designated-host outright, so smoke is the
  # only campaign it can produce.
  [[ "$mode" == "shm-smoke" ]] || {
    echo "unsupported shared-memory mode: $mode (expected shm-smoke)" >&2
    exit 1
  }
  args+=(--smoke)
  mkdir -p "$out"
  local evidence="$out/hardware-envelope-${mode#shm-}.json"
  [[ ! -e "$evidence" ]] || {
    echo "refusing existing evidence file $evidence" >&2
    exit 1
  }
  shm_build
  "$SHM_BENCH" "${args[@]}" >"$evidence"
  grep -Eq '"local_verdict"[[:space:]]*:[[:space:]]*"MECHANISM_SMOKE_ONLY"' "$evidence" &&
    grep -Eq '"designated_host_verdict"[[:space:]]*:[[:space:]]*"BLOCKED"' "$evidence" || {
    echo "shared-memory harness did not retain separate local/designated verdicts" >&2
    exit 1
  }
  cat "$evidence"
  echo "evidence: $evidence" >&2
}

budget_env() {
  if [[ -z "${EIDNARA_IPC_BUDGET_COMMIT:-}" ]]; then
    # Evidence identity: stamping the clean HEAD hash onto a binary built
    # from modified sources lets two different dirty builds share one
    # BuildId, pass `compatible`, and merge as though they measured the
    # same code. Only bench build inputs gate this; docs and evidence
    # output stay writable during a run.
    if [[ -n "$(git -C "$ROOT" status --porcelain -- crates Cargo.toml Cargo.lock)" ]]; then
      echo "refusing dirty build inputs (crates/, Cargo.toml, Cargo.lock);" \
        "commit or stash, or set EIDNARA_IPC_BUDGET_COMMIT explicitly" >&2
      exit 1
    fi
    EIDNARA_IPC_BUDGET_COMMIT="$(git -C "$ROOT" rev-parse --short HEAD)"
    export EIDNARA_IPC_BUDGET_COMMIT
  fi
  export EIDNARA_IPC_BUDGET_RUSTC="${EIDNARA_IPC_BUDGET_RUSTC:-$(rustc --version)}"
}

budget_trap() {
  # Interrupts kill the tracked bench child, then finalize the active
  # manifest as interrupted so the attempt stays retained and out of the
  # aggregate. Masking INT/TERM first keeps the handler from re-entering.
  # Only the tracked child is signalled: `kill 0` targets the whole
  # process group, which includes the invoking shell whenever this script
  # runs without job control (CI, wrapper shells).
  trap 'trap "" INT TERM; \
    { [[ -z "${BUDGET_CHILD:-}" ]] || kill "$BUDGET_CHILD" 2>/dev/null || true; }; \
    sleep 0.5; \
    EIDNARA_IPC_BUDGET_MODE=finalize-interrupted EIDNARA_IPC_BUDGET_OUT="$BUDGET_OUT" \
    "$BUDGET_BENCH" || true; exit 130' INT TERM
}

budget_collect() {
  local arm="$1" class="$2" block="$3"
  shift 3
  # The bench runs as a tracked background child so the INT/TERM trap can
  # signal exactly it; `wait` surfaces the signal to the trap immediately
  # and still propagates the bench's exit status under `set -e`.
  env "$@" \
    EIDNARA_IPC_BUDGET_MODE=collect \
    EIDNARA_IPC_BUDGET_OUT="$BUDGET_OUT" \
    EIDNARA_IPC_BUDGET_ARM="$arm" \
    EIDNARA_IPC_BUDGET_CLASS="$class" \
    EIDNARA_IPC_BUDGET_BLOCK="$block" \
    ${BUDGET_PAIR:+EIDNARA_IPC_BUDGET_PAIR="$BUDGET_PAIR"} \
    "$BUDGET_BENCH" > >(tee -a "$BUDGET_OUT/collection.log") &
  BUDGET_CHILD=$!
  local rc=0
  wait "$BUDGET_CHILD" || rc=$?
  BUDGET_CHILD=""
  return "$rc"
}

# Even blocks reverse arm order to counter time-dependent drift.
# evidence::counterbalanced_schedule).
budget_block() {
  local block="$1"
  shift
  local arms=(atomic-floor ring-serial ring-open ring-throughput)
  # shellcheck disable=SC2206
  local rates=($BUDGET_RATES)
  if (((block - 1) % 2 == 1)); then
    arms=(ring-throughput ring-open ring-serial atomic-floor)
    # The rate sweep reverses with the arms so within-block position is not confounded with offered rate.
    local reversed=()
    for ((i = ${#rates[@]} - 1; i >= 0; i--)); do reversed+=("${rates[i]}"); done
    rates=("${reversed[@]}")
  fi
  for arm in "${arms[@]}"; do
    if [[ "$arm" == ring-open ]]; then
      for rate in "${rates[@]}"; do
        budget_collect ring-open same-l3 "$block" "$@" "EIDNARA_IPC_BUDGET_RATE=$rate"
      done
    else
      budget_collect "$arm" same-l3 "$block" "$@"
    fi
  done
  # Cross-NUMA paired arms: auto-selection either finds a pair or
  # finalizes a structured skip without failing the block. Their order
  # Their order reverses on even blocks exactly like the same-L3 arms.
  # too.
  local cross=(atomic-floor ring-serial)
  if (((block - 1) % 2 == 1)); then
    cross=(ring-serial atomic-floor)
  fi
  for arm in "${cross[@]}"; do
    BUDGET_PAIR="${BUDGET_CROSS_PAIR:-}" budget_collect "$arm" cross-numa "$block" "$@"
  done
}

# Fails when any same-L3 attempt under the given evidence directory finalized as
# skipped; cross-NUMA skips are the documented optional outcome and are ignored.
budget_require_same_l3() {
  local out="${1:?outdir}" skipped
  skipped=$(grep -l '"state": *"skipped"' "$out"/*same-l3*/manifest.json 2>/dev/null || true)
  if [[ -n "$skipped" ]]; then
    echo "$skipped" >&2
    echo "a required same-L3 arm was skipped; the run has no primary measurement" >&2
    return 1
  fi
}

budget_run() {
  local blocks="$1"
  shift
  budget_build
  budget_env
  [[ -e "$BUDGET_OUT" && -n "$(ls -A "$BUDGET_OUT" 2>/dev/null)" ]] && {
    echo "refusing nonempty evidence directory $BUDGET_OUT" >&2
    exit 1
  }
  mkdir -p "$BUDGET_OUT"
  # The planned attempt set is persisted first: aggregation verifies
  # every planned attempt has a finalized manifest, so a deleted or
  # omitted attempt directory cannot summarize as a smaller experiment.
  EIDNARA_IPC_BUDGET_MODE=record-plan EIDNARA_IPC_BUDGET_OUT="$BUDGET_OUT" \
    EIDNARA_IPC_BUDGET_BLOCKS="$blocks" EIDNARA_IPC_BUDGET_RATES="$BUDGET_RATES" \
    "$BUDGET_BENCH"
  budget_trap
  for block in $(seq 1 "$blocks"); do
    budget_block "$block" "$@"
  done
  # Same-L3 arms are the primary measurements. The bench finalizes a
  # structured skip with exit 0 when no valid pair exists, so their manifests
  # are inspected before aggregation can summarize a run with no primary data.
  budget_require_same_l3 "$BUDGET_OUT"
  EIDNARA_IPC_BUDGET_MODE=aggregate EIDNARA_IPC_BUDGET_OUT="$BUDGET_OUT" "$BUDGET_BENCH" \
    >"$BUDGET_OUT/summary.stdout.json"
  echo "evidence: $BUDGET_OUT"
}

case "${2:-${1:-}}" in
budget-plan)
  budget_build
  EIDNARA_IPC_BUDGET_MODE=plan EIDNARA_IPC_BUDGET_BLOCKS="${EIDNARA_IPC_BUDGET_BLOCKS:-10}" \
    EIDNARA_IPC_BUDGET_RATES="$BUDGET_RATES" "$BUDGET_BENCH"
  exit 0
  ;;
budget-preflight)
  budget_build
  budget_env
  EIDNARA_IPC_BUDGET_MODE=plan EIDNARA_IPC_BUDGET_RATES="$BUDGET_RATES" "$BUDGET_BENCH"
  BUDGET_OUT=$(mktemp -d)
  BUDGET_PAIR="${BUDGET_PAIR:-}"
  budget_trap
  budget_collect atomic-floor same-l3 1 \
    EIDNARA_IPC_BUDGET_WARMUP_BATCHES=2 EIDNARA_IPC_BUDGET_BATCHES=5 EIDNARA_IPC_BUDGET_EXCHANGES=1000
  budget_collect ring-serial same-l3 1 \
    EIDNARA_IPC_BUDGET_WARMUP_OPS=200 EIDNARA_IPC_BUDGET_MEASURED_OPS=1000
  # The same-L3 arms are the experiment's primary measurements; the bench
  # finalizes a structured skip with exit 0 when no valid pair exists, so the
  # finalized manifests are inspected rather than trusting the exit status.
  # The manifests are written by the bench itself before it exits, unlike the
  # tee'd collection log, which a separate process may still be flushing.
  if ! budget_require_same_l3 "$BUDGET_OUT"; then
    echo "preflight failed" >&2
    rm -rf "$BUDGET_OUT"
    exit 1
  fi
  if [[ -n "${BUDGET_CROSS_PAIR:-}" ]]; then
    # An explicit cross pair must fail preflight, not the final run: an
    # invalid pair finalizes a failed attempt and exits nonzero here.
    BUDGET_PAIR="$BUDGET_CROSS_PAIR" budget_collect atomic-floor cross-numa 1 \
      EIDNARA_IPC_BUDGET_WARMUP_BATCHES=2 EIDNARA_IPC_BUDGET_BATCHES=5 EIDNARA_IPC_BUDGET_EXCHANGES=1000
  fi
  rm -rf "$BUDGET_OUT"
  echo "preflight ok"
  exit 0
  ;;
esac

case "${2:-}" in
shm-smoke)
  shm_run "${1:?outdir}" "$2"
  exit 0
  ;;
budget-smoke | budget-pilot | budget-final)
  BUDGET_OUT="${1:?outdir}"
  BUDGET_PAIR="${BUDGET_PAIR:-}"
  case "$2" in
  budget-smoke)
    BUDGET_RATES="${BUDGET_SMOKE_RATES:-20000}"
    budget_run 1 \
      EIDNARA_IPC_BUDGET_WARMUP_BATCHES=5 EIDNARA_IPC_BUDGET_BATCHES=20 EIDNARA_IPC_BUDGET_EXCHANGES=2000 \
      EIDNARA_IPC_BUDGET_WARMUP_OPS=500 EIDNARA_IPC_BUDGET_MEASURED_OPS=5000 \
      EIDNARA_IPC_BUDGET_WARMUP_SECS=1 EIDNARA_IPC_BUDGET_MEASURE_SECS=2
    ;;
  budget-pilot)
    budget_run 3 \
      EIDNARA_IPC_BUDGET_WARMUP_BATCHES=20 EIDNARA_IPC_BUDGET_BATCHES=100 EIDNARA_IPC_BUDGET_EXCHANGES=10000 \
      EIDNARA_IPC_BUDGET_WARMUP_OPS=10000 EIDNARA_IPC_BUDGET_MEASURED_OPS=120000 \
      EIDNARA_IPC_BUDGET_WARMUP_SECS=2 EIDNARA_IPC_BUDGET_MEASURE_SECS=5
    ;;
  budget-final)
    budget_run "${EIDNARA_IPC_BUDGET_BLOCKS:-10}" \
      EIDNARA_IPC_BUDGET_WARMUP_BATCHES=50 EIDNARA_IPC_BUDGET_BATCHES=200 EIDNARA_IPC_BUDGET_EXCHANGES=10000 \
      EIDNARA_IPC_BUDGET_WARMUP_OPS=20000 EIDNARA_IPC_BUDGET_MEASURED_OPS=150000 \
      EIDNARA_IPC_BUDGET_WARMUP_SECS=2 EIDNARA_IPC_BUDGET_MEASURE_SECS=10
    ;;
  esac
  exit 0
  ;;
budget-summarize)
  BUDGET_OUT="${1:?outdir}"
  budget_build
  budget_require_same_l3 "$BUDGET_OUT"
  EIDNARA_IPC_BUDGET_MODE=aggregate EIDNARA_IPC_BUDGET_OUT="$BUDGET_OUT" "$BUDGET_BENCH"
  exit 0
  ;;
esac

echo "unknown operation: ${2:-${1:-}}" >&2
exit 1
