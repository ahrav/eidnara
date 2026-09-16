# packing-output-byte-identical-across-cache-states

## Discovery trigger

RP2.8's packing contract requires identical inputs to produce byte-identical
output across cache states, threads, and a fresh process, so the digest the
apply step validates is a function of the selection and not of the daemon's
cost cache.

## Evidence trail

- `crates/daemon/src/packing/render.rs` `Ledger::delta` counts through the
  profile's charge function; `crates/daemon/src/token_cache.rs` serves a count
  under its accounting revision or recomputes it, so a miss and a hit return the
  same value for the same text.
- `crates/daemon/src/packing/serialize.rs` `Preparation::identity` digests the
  written body, so two preparations agree exactly when their bytes agree.
- `crates/daemon/tests/packing_serialize.rs` renders the same fixture with the
  cache cleared, warm, and rotated, both with slack and at an estimated-tokens
  bound one below the full render where a count off by one would change the
  adjustment, from four scoped threads, and from the test binary re-executed as
  a child process, and compares each identity to its reference; the adjusted
  and full preparations differ in identity while the digests of their members
  agree.

## Failure scenario

A count that differs between a cache hit and a recomputation shifts a group
across the budget boundary, so a retry of the same request renders a
different body and fails the apply-time digest check.

## Timing windows and dependencies

Concurrent misses on one key store the same value; the render does not depend
on which thread stored it.

## What a test must construct

- The `cost_cache::clear` and `cost_cache::rotate` test-support seams.
- Scoped threads over one fixture and a child process running one ignored test
  by exact name.
