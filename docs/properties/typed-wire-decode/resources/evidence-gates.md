# Typed-wire resource evidence gates

System: typed-wire decode resources.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](source-register.md) defines P, B, and review provenance.

This file contains **one evidence gate**. It is not a runtime property or
liveness claim. R5 remains a genuine test-only allocation budget invariant;
its instrumentation validity rules do not create a second evidence gate.

## EG1: decode-projection payoff

Status: required; before leg collected in
[`evidence/eg1-decode-projection/`](evidence/eg1-decode-projection/README.md)
at production revision `85accd89` with the retained harness patch; after leg
and verdict pending the typed-wire U1 ticket.

Origin: P:L18, P:L57, and P:L213. The record
[`decode-projection-payoff-has-comparable-evidence`](catalog.md#decode-projection-payoff-has-comparable-evidence)
is preserved as invalidated for category mismatch. Its
[evidence trail](evidence/decode-projection-payoff-has-comparable-evidence.md)
remains available. Moving the obligation here does not weaken it.

### Exact obligation

Before a typed-wire decode payoff verdict, retain comparable before and after
raw records for **both 40- and 200-message mixed corpora**. Four size/artifact
cells are mandatory. Each corpus has immutable raw bytes, a checksum, and
recorded message, block, and payload distributions.

Time actual production `serde_json::from_slice::<TransformRequest>` followed
by `wire::project_messages` from the same body. Keep the request and projection
live through the declared endpoint. State before collection whether result
destruction is inside the interval. An already decoded request, a derive-only
mirror, or projection-only timing cannot satisfy this operation contract.

The manifest records before and after source and binary identities, Rust
toolchain, locked dependencies, build profile and features, allocator, host,
corpus hashes, warmup, exact operation boundary, process-level replication,
assignment/order, raw samples, summary, uncertainty calculation, and the
predeclared gain/noise rule. Configurations must be comparable; artifact
identities differ by design. Do not pool iterations as independent processes.

The operation excludes transport, probing, admission, store work, and response
encoding unless those are measured under separately named boundaries. It
cannot claim handler-level latency or fulfill historical W1 size classes.

### Decision and stop semantics

- Missing cells, unverifiable identities, invalid timing boundaries, or an
  unset noise rule yield `blocked`, never a passing payoff verdict.
- `proceed` requires the predeclared gain/noise rule to clear for every required
  size point. Preserve raw contrary results rather than selecting a winner.
- A gain within noise yields **stop for the whole plan**, as P:L18 requires.
  U2/U4 identity and evidence work does not rescue an unproven U1 payoff.
- A regression or failure to clear the predeclared rule is not a faster claim.
  No percentage, repetition count, or significance threshold is invented here.
- Preserve the other P:L18 stops: changed plugin-shaped projection golden
  bytes, changed original A1-A3 admission outcomes, or a needed wire-visible
  field, literal, or error-code change. These are inherited plan decisions.

### Fixed evidence receipt

Retain the original name `typed-wire-resources-measurement-pair`. It marks
manifest completeness only when both sizes and both legs have valid raw
records. It requires no speedup and is not one of R7's runtime `sometimes`
checks. A completion rollup cannot substitute for four individual cell records.

### Ownership and routing

Route in order:

1. `/quantitative-analysis:statistics-and-benchmarking-discipline` fixes the
   claim, outcome, timing, workload, and reporting semantics.
2. `/quantitative-analysis:benchmark-experiment-design` fixes replication,
   order, inference, noise/stopping rule, and the decision across both sizes.
3. `/performance:bench-compare` executes the frozen schedule against identified
   artifacts and retains the evidence.

Pin B (`e451a2b470ae8663b4613ca04f019a30b6d7df53`) or a verified equivalent
before artifact; local `main` is stale. The existing hot_path bench requires
`bench-internals` (`crates/daemon/Cargo.toml:77-81`); Cargo execution must use
`--locked`. These are handoff requirements; no command runs in this task.

W1 stays invalidated at
`docs/properties/hot-path-optimization/latency-audit/catalog.md:1772-1773`.
P:L213's requested W1 update needs an owner disposition. This gate supplies
plan-local evidence requirements without silently reactivating that record.

### Open owner questions

- Who owns the raw artifact and predeclared noise/uncertainty rule for both
  sizes? (needs human input)
- How does the owner reconcile P:L213's W1 update with W1's invalidated status?
  Retain plan-local evidence until resolved. (needs human input)
- Final binaries, raw records, and a valid measurement schedule remain missing.
  They are prospective work, not findings that this documentation can close.
