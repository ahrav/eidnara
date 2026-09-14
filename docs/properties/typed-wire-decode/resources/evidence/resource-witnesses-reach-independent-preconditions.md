# resource-witnesses-reach-independent-preconditions

System: resource campaign situation coverage, test-only.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](../source-register.md) defines P and B. No campaign runs.

## Discovery trigger

A1/A3/B1 and the plan require more than ordinary successful text decoding.
The footprint, fallback, and sharing assertions can all pass
without their risky situation being constructed. Every required marker must
describe a precondition that a correct implementation can reach.

## Evidence trail

1. `docs/properties/hot-path-optimization/latency-audit/catalog.md:319-350`
   treats A3 as partial: a held charge, not concurrent ring requests.
2. `crates/daemon/src/lib.rs:20261-20276` exposes `TestPool::hold` backed by
   real ByteBudget permits. This is stronger than inferring pressure from an
   error response or using an unbacked successful reservation.
3. `:20414-20443` constructs pressure, reads ShortfallMarker, releases the
   owner, then retries. Its capacities still depend on current footprint.
4. `crates/daemon/src/metered_decode.rs:112-129,294-302` exposes the shortfall
   record and a process-global count. Neither alone proves another owner was
   live or that a particular corpus case reached the gate.
5. `crates/daemon/src/wire.rs:1749-1792` checks shared shell identity, copy-on-
   write separation, and a block owner surviving projection drop.
6. `crates/daemon/tests/parse_charge_covers_typed_decode.rs:110-123` reaches
   both tree stages for dense native arrays. It does not witness all text,
   error, escape, and fallback cases proposed by this resource catalog.

All checks are unaudited. These are test-only observation mechanisms over
production paths, not evidence that default traffic constructs each state.

## Failure scenario

A supposed fallback test never fails typed decoding. A supposed pressure
test sees `queue_full` caused by a different gate. A supposed shared-owner
test drops both owners before measuring. An allocation gate runs against an
empty subtree. A separate evidence receipt can miss a before/after cell;
that concern belongs to EG1 rather than runtime reachability.

Competing explanation: the intended state has become unreachable, rather
than the workload failing to create it. First verify independent setup and
the marker predicate; a finite missed witness does not refute formal liveness.

## Timing windows and dependencies

Witness byte shape before decode, acquired holder bytes before reservation,
typed-prefix progress before fallback, and live allocation identities before
cleanup. Instrumentation is supporting evidence, not a second implementation
of the admission verdict. No marker asserts the safety violation itself.

For failures, use independently valid prefixes and explicit malformed or
duplicate fields. For pressure, record the holder's acquisition receipt and
keep it alive through the observed reservation attempt.

## What a test must construct

The exact constant marker list is in
[fault-map.md](../fault-map.md#independent-coverage-markers). Each marker has
its own independent `sometimes` check and occurrence result. The twelve
resource checks retain their names. Their conjunction is only a completion
rollup, not an aggregate `sometimes` that could mask a missing situation.

- Successful direct and actual two-stage tree inputs with owned output.
- Fallback after a nonempty typed prefix and failure after large allocations.
- Escaped scratch, long keys, and fixed boundary-neighbour inputs.
- A fitting request competing with a distinct live holder.
- Typed input and projection live together, including canonical workspace.
- A shared owner that survives cache removal and an isolated nonempty subtree.
- Keep `typed-wire-resources-measurement-pair` as EG1's separate evidence
  receipt for all four size/artifact cells; it is not a thirteenth runtime check.

## Investigation log

### Q: Is ShortfallMarker an independent witness by itself?

- Sources examined: TestPool, drained-pool test, and ShortfallMarker fields.
- Findings: It records the candidate meter's needed/charged/capacity values.
  It lacks the separate holder's acquisition and lifetime evidence.
- Missing evidence: Correlated independent holder receipts for fixed fixtures.
- Conclusion: resolved with answer. Pair it with the known live holder and
  precomputed fixture requirement; do not infer pressure from the error code.

### Q: What should happen if a marker never fires?

- Sources examined: METHOD coverage rules and the scoped tests above.
- Findings: Existing cases only cover subsets. A missing situation can mean a
  generator gap or an unreachable precondition after behavior changes.
- Missing evidence: A final candidate campaign with per-marker receipts.
- Conclusion: unresolved, needs the downstream test owner to investigate each
  absence. Never replace it with `sometimes(true)` or a violation predicate.

### Q: Does one aggregate sometimes establish all situations?

- Sources examined: METHOD coverage rules; original marker table; finding 6.
- Findings: Each fixed marker needs its own predicate, identity, and result.
  A summary conjunction cannot replace those individual checks.
- Missing evidence: Candidate campaign receipts for the twelve resource checks.
- Conclusion: resolved on semantics. Retain names, make checks independent,
  and keep the measurement receipt with EG1. No runtime liveness claim is added.

## Typed-wire U1 execution, 2026-09-13

Markers constructed by the executed tests, each with its own precondition
observed independently of the safety verdict: direct success on the frozen
corpora (`whole_request_decode_fits_its_resident_charge`), tree conversion of
text-heavy bodies (`parse_charge_covers_text_heavy_peaks_on_both_lanes`),
fallback after a typed prefix fails at a late duplicate key
(`parse_charge_covers_a_failed_typed_prefix_and_its_tree_fallback`, which
asserts the walk accepted and the typed decode refused), escaped scratch (the
escaped variants), pool shortfall with a held pool
(`a_drained_pool_refuses_a_fitting_body_as_transient_and_records_the_shortfall`
in `crates/daemon/src/lib.rs`, where a backed `TestPool` holds bytes for a
distinct owner, the refusal records the shortfall, and the same body decodes
after the holder drops; `a_held_pool_stops_the_direct_lane_walk_before_it_unescapes_a_large_string`
and `a_pool_with_room_for_the_prefix_only_refuses_before_the_large_string_is_unescaped`
prove refusal ordering against a drained or unbacked reserve, not an
independent owner),
boundary neighbours (`text_heavy_admission_ceiling_witnesses`), live
projection with shared shells (`decode_and_projection_fit_the_declared_pool`),
and isolated allocation scope (`message_decode_stays_within_the_allocation_budget`).
Not constructed here: a late typed error other than a duplicate key, a shared
owner surviving cache eviction, and concurrent holders.
