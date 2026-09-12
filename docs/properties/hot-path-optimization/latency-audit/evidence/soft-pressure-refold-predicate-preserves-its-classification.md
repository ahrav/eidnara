# soft-pressure-refold-predicate-preserves-its-classification

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The parent's [portfolio evaluation](../../portfolio-evaluation.md#gaps-queued)
queued this record as its gap 3 (finding 10): the SOFT branch tokenizes the
frozen m0 payload and the composed m1 body on every SOFT pass with the
uncached tokenizer and compares the counts against fixed constants, and no
record froze that predicate. The parent's H records take the SOFT-versus-refold
classification as given and constrain what each class preserves; an
optimization that caches or approximates these two counts changes which class
a pass lands in while every H record still passes.

## Evidence trail

- The SOFT arm at [`PassPlan::Soft`][soft-arm] calls [`compose_m160-237`][m1-compose]
  with [`cached_estimate_tokens`][soft-m1-compose] as its estimator; that
  estimator is used only for user-profile trimming
  ([`m1_compose.rs:194-208`][m1-trim]).
- The predicate at [`:4309-4327`][soft-predicate]: `m0_tokens` is
  `tokenizer::estimate_tokens(&unit.frozen_payload)` for the frozen unit with
  key `m0`, or `0` when none exists; `m1_has_content` is `m1.body !=
  M1_PLACEHOLDER`; `m1_tokens` is `tokenizer::estimate_tokens(&m1.body)`
  when there is content, else `0`; `pressure_refold` is
  `m1.memory_update_count > 40`, or content and `m1_tokens as f64 >
  history_budget_tokens * 0.20` and `history_budget_tokens > 0.0`, or
  content and `m0_tokens >= 500` and `m1_tokens as f64 > m0_tokens as f64 *
  0.15`. Both counts are direct [`tokenizer::estimate_tokens`][soft-direct]
  calls, not the injected `estimate_tokens` parameter.
- When `pressure_refold` is true the arm loads compartments and calls
  [`compose_m0_for_context`][refold-branch] with the injected estimator; the
  parent's [H2][h2] names that as the "pressure refold" boundary distinct from
  an ordinary SOFT.
- [`M1_PLACEHOLDER`][placeholder] is the string [`assemble_m206-231`][assemble]
  returns when every m1 piece is empty, so `m1_has_content` is exactly "some
  compartment, profile, or note block rendered".
- `history_budget_tokens` reaches the predicate from the handler's
  `ProducerContext` at [`lib.rs:8239-8242`][budget-filter], which admits only
  finite values `>= 0.0` from the request and falls back to the bound
  budget; a zero budget disables the second disjunct through the
  `> 0.0` guard.
- [`compose_m160-237`][m1-compose] returns `memory_update_count: 0`
  unconditionally at [`:231`][m1-count-zero]; the field has no other writer
  in the workspace.
- The comment at [`transform.rs:1868-1870`][hard-only-doc] says the injected
  estimator is HARD-only; it does not describe these two direct calls.

## Failure scenario

A cache-backed count is substituted for one of the two direct calls. At a
boundary value the cached count differs from the direct count (a truncated
`u32`, a stale entry, an approximation), so `m1_tokens > budget * 0.20`
flips, and a pass that the reference keeps as an ordinary SOFT rematerializes
m0, or the reverse. A batched estimate reuses a count computed for a
different `m1.body`. Either way the frozen bytes and the prompt content change
while both H2 classes remain individually well formed.

## Timing windows and dependencies

None in time. The inputs are the frozen m0 payload, the composed m1 body, the
budget, and the update count, all fixed at the point of evaluation.

## What a test must construct

SOFT passes at each boundary: `history_budget_tokens` such that `m1_tokens`
sits at the 0.20 share; a frozen m0 of 499 and 500 tokens with `m1_tokens` at
the 0.15 ratio; an `m1.body` equal to the placeholder; a store with no `m0`
frozen unit; and a reference predicate evaluated with the direct tokenizer on
the same texts. No existing check exercises the predicate. The first disjunct
cannot be constructed at HEAD (see the log).

The reference is a frozen test-only copy of the predicate at
`transform.rs:4298-4316`, not the live function, because the change replaces
it. The update-count arm at 40 and 41 is reachable only through a directly
constructed `M1Composition` until a writer for `memory_update_count` exists.

## Investigation log

### Q: Are the constants 40, 0.20, 0.15, and 500 a contract or tuning?

- Sources examined: [`:4309-4327`][soft-predicate], the transform catalog's
  history records ([H1][h1], [H2][h2]), `docs/` for the constants.
- Findings: The constants appear only in the predicate; no document names
  them. A frozen reference pins them either way.
- Missing evidence: A specification statement.
- Conclusion: needs human input.

### Q: Is the `memory_update_count > 40` disjunct reachable at HEAD?

- Sources examined: [`M1Composition`][m1-struct],
  [`compose_m1:231`][m1-count-zero], a workspace search for the field.
- Findings: The only constructor sets the count to `0` and the only reader is
  the predicate. The disjunct is dead code at HEAD, so the record's "for
  fixed ... update count" holds trivially with the count fixed at zero. A
  reference must still include the disjunct, since a specification may give
  the field a writer; the catalog's W10 marker for it cannot fire today.
- Missing evidence: The intended source of `memory_update_count`.
- Conclusion: resolved with answer - unreachable at HEAD; raised for W10 and
  for the record's required state, which names it as constructible.

## Implementation evidence and owner decision

The preceding discovery snapshot is retained at its stated baseline. Current
checks and explicit decision provenance are in [shared selection and pressure
accounting](shared-selection-and-pressure-accounting.md).

The owner authorizes removal of the dead update-count arm, not changes to
0.20, 0.15, 500, or their comparisons. The original predicate is frozen in a
test-only reference before production edits and evaluated with count zero.
All 48 full-pass cases pass `0` to the reference; the composition no longer
carries the count, so no test asserts a composed value for it.
Thus removal preserves every production-reachable classification, while the
reference remains independent of the implementation.

The SOFT spy observes the exact frozen m0 and composed m1 texts through the
injected estimator, verifies cache/direct parity, and compares both the
response action and the `pressure_refold` reason. Separate predicate-seam
tests cover an absent m0, a placeholder, empty content, and warm cache hits.
The update-count source question is resolved by retirement, not by inventing
a writer or treating an unreachable condition as covered.

[soft-arm]: ../../../../../crates/daemon/src/transform.rs#L4295-L4308
[soft-m1-compose]: ../../../../../crates/daemon/src/transform.rs#L4306
[soft-direct]: ../../../../../crates/daemon/src/transform.rs#L4309-L4320
[soft-predicate]: ../../../../../crates/daemon/src/transform.rs#L4309-L4327
[refold-branch]: ../../../../../crates/daemon/src/transform.rs#L4328-L4355
[hard-only-doc]: ../../../../../crates/daemon/src/transform.rs#L1868-L1870
[m1-struct]: ../../../../../crates/daemon/src/m1_compose.rs#L92-L101
[m1-compose]: ../../../../../crates/daemon/src/m1_compose.rs#L160-L237
[m1-trim]: ../../../../../crates/daemon/src/m1_compose.rs#L194-L208
[m1-count-zero]: ../../../../../crates/daemon/src/m1_compose.rs#L231
[placeholder]: ../../../../../crates/daemon/src/memory_render.rs#L10-L12
[assemble]: ../../../../../crates/daemon/src/memory_render.rs#L206-L231
[budget-filter]: ../../../../../crates/daemon/src/lib.rs#L8239-L8242
[h1]: ../../catalog.md#history-budget-selection-preserves-reference-bytes
[h2]: ../../catalog.md#history-budget-boundaries-remain-distinct
