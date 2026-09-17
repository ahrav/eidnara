# RP2.8 required-phase properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `d3d7663b` (the tip of the RP2.8 U1 branch the U2 change was authored
against). Method: `../../METHOD.md` and `property-discovery-and-catalog`.

Source: the RP2.8 specification
([#629](https://github.com/ahrav/eidnara/issues/629)), whose packing and
accounting contract and acceptance rows AC3 and AC7 name these obligations,
and the RP2.8 U2 ticket ([#632](https://github.com/ahrav/eidnara/issues/632))
that lands their executable checks.

This part owns the required phase: what refuses a required occurrence, the
integer budget it is charged against, and the trace that witnesses that no
optional event and no retrieval call precede its completion. Optional
grouping and admission are the U3 part.

## Observation contract

The pure decisions are `crates/retrieval/src/packing/required.rs`
(`admit_required`, `reserve_required`), exercised by
`crates/retrieval/tests/packing_required.rs`. The daemon entry is
`crates/daemon/src/packing.rs` `prepare_required`, exercised by
`crates/daemon/tests/packing_required.rs` against a seeded kernel and a
search projection; the `PackingTrace` it fills is the observation point for
stage order, retrieval calls, and payload loads.

## Q3 rulings

Recorded by the repository owner at the U2 change:

- `retrieval::packing::RequiredContextFailure` owns the required-failure
  classes. The daemon maps it to the `PreparationFailure` outcome literal when
  RP2.7.U4 lands that outcome vocabulary.
- Required overflow is its own variant, `OverBudget { limit, charged }`,
  distinct from the per-item and set-wide `Oversized` bounds.
- Required bytes come from the projection payload row named by the
  occurrence's own `PayloadRef`, verified by digest and length. That read is a
  payload load, counted by the payload-loads bound and trace counter; it is
  not a retrieval call, and no kernel canonical re-read exists.
- The packer judges eligibility itself at the start of the required phase in
  one kernel batch; a selection-time report is not accepted as input.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [packing-required-phase-precedes-optional-work](#packing-required-phase-precedes-optional-work) | safety | test-only | always | active | high |
| [packing-budget-is-an-integer-never-clamped](#packing-budget-is-an-integer-never-clamped) | safety | test-only | always | active | high |
| [packing-required-bytes-are-charged-untruncated](#packing-required-bytes-are-charged-untruncated) | safety | test-only | always | active | high |

## Records

### packing-required-phase-precedes-optional-work

Type: safety
Reachability: test-only - `prepare_required` is called from
`crates/daemon/tests/packing_required.rs` only; `grep -rn 'prepare_required'
crates --include=*.rs` outside `crates/daemon/src/packing.rs` finds that test
file alone, so no route packs a selection at this base.
Status: active
Exercised: yes - `crates/daemon/tests/packing_required.rs`
`each_required_fault_yields_exactly_one_class_with_zero_optional_events`,
`a_required_occurrence_the_kernel_excludes_is_ineligible_not_missing`,
`corrupt_payload_bytes_and_foreign_tuples_are_refused_as_corrupt`,
`an_expired_deadline_refuses_the_required_phase_before_any_optional_event`,
`more_requests_than_the_load_bound_are_refused_before_any_read`, and
`every_failure_class_has_a_distinct_literal`; the hidden verdict and the
stale and superseded verdicts are exercised at the pure layer by
`crates/retrieval/tests/packing_required.rs`
`every_verdict_maps_to_exactly_one_failure_class`.
Guarantee: A required occurrence that is missing, stale, hidden, corrupt,
ineligible, or oversized, or a required set whose cost exceeds the token
limit, yields exactly one `RequiredContextFailure` class, and the trace shows
zero optional events and zero retrieval calls; an exhausted `EvalBudget`
yields the deadline refusal with the same trace.
Check: `always` - for each fault class one required occurrence exhibiting it
returns that class and no other; `class()` literals are pairwise distinct;
`PackingTrace::optional_events()` is zero and `retrieval_calls()` equals the
seeded lane count after every refusal and every success; admission-stage
refusals record no `Loaded` event and `payload_loads()` stays zero; the
deadline refusal and the load-bound refusal record no event. `always`
because the phase must hold it on every request, not only on a reachable
subset.
Fault/timing angle: The `EvalBudget` deadline is polled between stages; a
budget that expires mid-phase refuses at the next poll.
Required faults and enabling state: An unpersisted identifier; a request
revision other than the row's; a tombstoned row; a kernel with no object for
the row's source; a payload row rewritten to other bytes; an occurrence row
whose tuple is another occurrence's; per-item, load-count, and byte-total
bounds below the fixture; a token limit one below the required cost; an
expired and a cancelled budget.
Confidence: high - [evidence](evidence/packing-required-phase-precedes-optional-work.md).
Every clause is asserted at this base; the hidden verdict is asserted only
at the pure layer because the kernel fixture has no hidden surface.
Existing check: none before this change.
Impact: An optional event before the required phase completes would let
optional context consume budget the required set needed, and a retrieval call
would make the packer a ranking authority.
Open questions: None.

### packing-budget-is-an-integer-never-clamped

Type: safety
Reachability: test-only - `ClaudeTokens::from_budget` has no production
caller at this base; no route parses a wire budget yet, so the manifest's
packing group is the only budget source a caller can reach.
Status: active
Exercised: yes - `crates/daemon/src/packing.rs` tests
`budgets_are_integers_and_never_clamped` and
`the_retained_malformed_budget_clamp_is_not_inherited`; the two
`compile_fail` doctests on `ClaudeTokens`.
Guarantee: The packer's budget and cost unit is the integer `ClaudeTokens`;
a NaN, negative, non-integer, infinite, over-range, or absent budget is
refused with a typed `BudgetRefusal`, never rounded, saturated, or clamped; `EmbedTokens` and
`ClaudeTokens` do not substitute for each other at compile time.
Check: `always` - each malformed value maps to its refusal variant; the
legacy profile trim answers a NaN or negative budget as if it were one token
while `from_budget` refuses the same value; both cross-substitution doctests
fail to compile. `always` because a single clamped budget is an over-budget
edit.
Fault/timing angle: none.
Required faults and enabling state: NaN, negative zero, negative, fractional,
infinite, 2^64, and absent budget values.
Confidence: high - [evidence](evidence/packing-budget-is-an-integer-never-clamped.md).
The negative control runs the retained legacy clamp and shows it answering.
Existing check: `crates/daemon/src/m0_compose.rs`
`trim_user_profile_to_budget` clamps `budget_tokens.max(1.0)`; it is the
precedent the packer must not inherit, not a check.
Impact: A clamped or rounded budget emits over-budget bytes or silently
drops required context.
Open questions: None.

### packing-required-bytes-are-charged-untruncated

Type: safety
Reachability: test-only - same grep as the first record.
Status: active
Exercised: yes - `crates/daemon/tests/packing_required.rs`
`required_cost_at_the_limit_succeeds_and_one_above_fails_without_truncation`
and `a_required_payload_beyond_the_legacy_cut_is_materialized_and_charged_whole`;
`crates/retrieval/tests/packing_required.rs`
`reservation_charges_every_byte_and_stops_exactly_at_the_limit`;
`crates/daemon/src/packing.rs` test
`the_sixty_four_kib_silent_cut_is_not_inherited`.
Guarantee: Every materialized required byte equals the selected payload byte
and is charged through the named accounting profile as the rendered delta of
its fragment (the `accounting/` part fixes the delta rule); the required-only cost exactly at
the token limit succeeds and one above fails; no required payload is cut at
64 KiB or anywhere else.
Check: `always` - the materialized bytes equal the persisted payload bytes;
the sum of item costs equals `charged`; a limit equal to the cost succeeds and
a limit one below returns `OverBudget` carrying the limit and the sum through
the crossing item; a 64 KiB + 7 payload is materialized and charged whole
through the entry while the legacy memory line renders exactly 64 KiB of it. `always` because a single uncharged or cut byte
breaks the accounting contract.
Fault/timing angle: none.
Required faults and enabling state: A one-token-per-byte estimator so the
limit sits on a byte boundary; a payload longer than 64 KiB.
Confidence: high - [evidence](evidence/packing-required-bytes-are-charged-untruncated.md).
The estimator is injected, so the boundary is exact rather than approximate.
Existing check: `crates/daemon/src/memory_render.rs` `render_memory_line`
cuts at 64 KiB; it is the precedent, not a check.
Impact: An uncharged byte overruns the provider budget; a cut byte changes
the meaning of required context without a signal.
Open questions: None.
