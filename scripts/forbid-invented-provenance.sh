#!/usr/bin/env bash
# Usage: forbid-invented-provenance.sh
#
# Curator review inputs classify Sensitive because they carry no verified
# provenance. The byte-verifying Git publisher is the only producer that may
# construct affirmative provenance, so review-staging sources, Curator sources,
# and their tests must not name `RepositoryProvenance` or populate a
# `provenance` field. The bracketed character keeps the pattern from matching this file.
#
# Exit 1 on a match, 2 on a tool failure.
set -euo pipefail

PATTERN='Repositor[y]Provenance|provenanc[e]:[[:space:]]*Some'
PATHS=(
  ':(glob)crates/kernel/src/review_staging.rs'
  ':(glob)crates/kernel/tests/kernel_review_staging.rs'
  ':(glob)crates/**/curator*'
  ':(glob)crates/**/curator/**'
)

status=0
git grep -n -I -E -e "$PATTERN" -- "${PATHS[@]}" || status=$?
if [ "$status" -eq 0 ]; then
  echo "invented provenance in a Curator or review-staging path" >&2
  exit 1
fi
if [ "$status" -ne 1 ]; then
  echo "forbid-invented-provenance: git grep failed with exit $status" >&2
  exit 2
fi
