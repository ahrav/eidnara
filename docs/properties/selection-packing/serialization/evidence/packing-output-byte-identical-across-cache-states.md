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
  bound one below the full render where a count off by one would flip the
  optional phase between admission and `PreparationRefusal::Accounting`, from
  four scoped threads, and from the test binary re-executed as a child
  process, and compares each identity, or each typed refusal with its value
  and limit, to its reference; the adjusted and full preparations differ in
  identity while the digests of their members agree.

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

## Investigation log

### Q: Is any cache state shared between the parent test and the child process?

- Sources examined: `crates/daemon/src/token_cache.rs` (the cache is a
  process-wide `static CACHE: Mutex<Option<Generations>>` with per-thread
  statistics; `clear` and `rotate` are the test-support controls);
  `identical_inputs_give_byte_identical_output_across_cache_states_threads_and_processes`,
  which re-executes the test binary with `--exact --ignored` and reads the
  child's `identity=` line.
- Findings: the cache lives in process memory and nothing persists it, so the
  child starts cold; its identity equals the parent's warm, cleared, and
  rotated identities and the identities from scoped threads over one fixture.
- Missing evidence: none.
- Conclusion: resolved with answer - the process boundary is a genuine cold
  start, and the comparison is on the SHA-256 of the written body.

### Q: Why is the refusal compared at the token edge, not only the body?

- Sources examined: `refusal_at_token_edge` and the sweep over cleared, warm,
  and rotated caches in the same test; `admit_render` in
  `crates/daemon/src/packing/render.rs`.
- Findings: the body is identical whenever admission succeeds, so a cache
  that changed a count by one would show only at a bound; the test sets the
  estimated-tokens bound one below the closed render and requires the same
  typed `AccountingExceeded` from every cache state.
- Missing evidence: none.
- Conclusion: resolved with answer - the edge comparison is the part of the
  check that can detect a count drift the body comparison cannot.
