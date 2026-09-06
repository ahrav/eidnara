#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
exec bun "$repo_root/crates/daemon/gen/gen-differential-golden.ts"
