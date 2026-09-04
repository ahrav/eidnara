#!/usr/bin/env bash
# Backpressure: the runbook guard. Any failure blocks keep.
set -euo pipefail
cd "$(dirname "$0")/.."
crates/tokenizer/benches/guard.sh 2>&1 | grep -vE "^\s+(Compiling|Checking|Documenting|Finished|Generated|Running|Doc-tests|Blocking)" | tail -60
test "${PIPESTATUS[0]}" -eq 0
