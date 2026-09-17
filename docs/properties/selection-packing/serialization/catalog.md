# RP2.8 serialization and adjustment properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `016c7127` (the tip of the RP2.8 U4a branch the U4b change was authored
against). Method: `../../METHOD.md` and `property-discovery-and-catalog`.

Source: the RP2.8 specification
([#629](https://github.com/ahrav/eidnara/issues/629)), whose packing contract
and acceptance rows AC7 and AC8 name these obligations, and the RP2.8 U4b
ticket ([#635](https://github.com/ahrav/eidnara/issues/635)) that lands their
executable checks.

This part owns the single serialization through the daemon's measure-then-write
guard, overflow repair within the approved pass cap, the packing group of the
runtime limit manifest, and byte-identical output across cache states, threads,
and processes. Binding the profile through apply is the U5a part.

The rendered-bytes and estimated-tokens bounds belong to the accounting part:
`prepare_optional` refuses a closed render past either
(`crates/daemon/src/packing/mod.rs:746`, recorded in
`../accounting/catalog.md` as
`packing-accounting-bounds-refuse-at-limit-plus-one`), so `finalize` receives
no admission over them and re-checks neither. `SerializationBounds` carries the
serialized-bytes limit and the pass cap only. Adjustment repairs a
serialized-bytes overflow; a rebuilt ledger is the admitted ledger less the
removed groups' entries, and the profile charges each fragment independently
of what follows it (every fragment starts with `<` and ends with `\n`, which
the exact tokenizer's pre-tokenizer splits on; `AccountingProfile::heuristic`
records the same requirement for an estimator), so a rebuild's total is the
admitted total less the removed groups' costs and cannot re-enter an
accounting bound the admission passed.

## Observation contract

`crates/daemon/src/packing/serialize.rs` holds the limits, the adjustment
loop, and the prepared body; `crates/daemon/src/projection_gates.rs` parses
the packing group; `crates/daemon/src/dispatch.rs` is the guard the body
passes through. `crates/daemon/tests/packing_serialize.rs` drives them through
`prepare_required`, `prepare_optional`, and `finalize` under the
one-token-per-byte profile, so every bound sits on a byte boundary.

## Q4 rulings

Recorded by the repository owner at the U4b change:

- The adjustment pass cap counts groups; each pass removes the last-admitted
  optional group and rebuilds the ledger from the required render, so the
  group's wrappers leave with it.
- Packing limits join the runtime manifest as one all-or-none group applied
  only when the `search_projection.packing.approved` flag is enabled. A
  present group without the flag, a partial group, an unknown name, or a
  protocol version that disagrees with the identity fails at parse.
- There is one manifest protocol version and no legacy or migration path; the
  daemon compares the manifest's version with the identity's copy and gives no
  other meaning to the string.
- Rollout order. A build without the packing group refuses a manifest that
  carries any `packing_*` limit or the `search_projection.packing.approved`
  flag as an unknown limit or hook, and that refusal closes the gate for every
  hook (`crates/daemon/src/projection_admission.rs`, `AdmissionInputs::read`).
  Deploy the daemon before `runtime-manifest.json` gains these keys, and a
  rollback to an older build must also revert the manifest. In the other
  direction nothing changes: this build reads a manifest without the group as
  before and yields `PackingLimitRefusal::Absent` to the packing path only.
  `../../search-projection/construction-contracts.md` CC9 records the same
  order beside the vector group's.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [packing-body-serialized-once-through-the-guard](#packing-body-serialized-once-through-the-guard) | safety | test-only | always | active | high |
| [packing-adjustment-removes-last-admitted-within-the-cap](#packing-adjustment-removes-last-admitted-within-the-cap) | safety | test-only | always | active | high |
| [packing-limits-fail-closed-at-manifest-parse](#packing-limits-fail-closed-at-manifest-parse) | safety | explicit-config-only | always | active | high |
| [packing-output-byte-identical-across-cache-states](#packing-output-byte-identical-across-cache-states) | safety | test-only | always | active | high |

## Records

### packing-body-serialized-once-through-the-guard

Type: safety
Reachability: test-only - `finalize` has no production caller at this base
(`grep -rn 'packing::' crates/daemon/src --include=*.rs` finds the module
declaration and an unrelated `retrieval::packing` import; `finalize` and
`prepare_optional` are called from `crates/daemon/tests/` only).
Status: active
Exercised: yes - `crates/daemon/tests/packing_serialize.rs`
`the_body_is_measured_once_reserved_exactly_and_written_through_the_guard`,
`each_serialization_bound_saturates_at_its_value_and_refuses_at_value_plus_one`,
`a_wrapper_overflow_removes_the_last_admitted_group_and_reclaims_its_wrappers`,
and `cap_exhaustion_emits_nothing_and_never_removes_required_items`; the
guard's own refusal past the transport maximum is
`crates/daemon/tests/prepared_output.rs`
`cap_plus_one_and_arithmetic_overflow_fail_before_write`.
Guarantee: An accepted preparation is measured once per pass by
`PreparedOutput::measure` and written exactly once through
`MeasuredOutput::write_to`, which verifies the measured length; a refused
preparation is never written; the body equals the closed ledger's text and
satisfies the serialized-bytes bound. It satisfies both accounting bounds
because the admission it came from passed them and adjustment only shortens
the render. Under manifest-derived bounds the guard's transport refusal is
unreachable: `PackingLimits::from_manifest` refuses a body bound past
`MAX_WIRE_BODY_BYTES`, the optional phase refuses a render past the
rendered-bytes bound, and the body is that render or a shorter one; the
`SerializationBound::Transport` arm is `always-or-unreached` from the packing
path and is exercised at the guard only.
Check: `always` - the guard's per-thread call counters read `(1, 1)` for a
fitting render under the byte profile and the exact tokenizer, `(2, 1)` after
one adjustment pass, `(4, 0)` when three passes end in refusal, `(1, 0)` when
the serialized-bytes bound refuses at a pass cap of zero, and `(0, 0)` when
the optional phase refuses an accounting bound; the body's length equals the
ledger's rendered bytes and the serialized-bytes limit set to that value
admits it; the body's bytes equal the ledger text. The counters count every
guard call on the test thread since the reset, so `(1, 1)` proves one
measurement and one write happened, and the body equality proves the returned
bytes are the ledger's; only together do they show the body went through the
guard once. `always` because one over-budget byte is a broken provider
contract.
Fault/timing angle: none.
Required faults and enabling state: A limit equal to the render's length; the
`guard_calls` test-support counters in `crates/daemon/src/dispatch.rs`.
Confidence: high -
[evidence](evidence/packing-body-serialized-once-through-the-guard.md).
Existing check: `crates/daemon/tests/prepared_output.rs` covers the guard's
own measure-then-write contract, including refusal one past
`MAX_WIRE_BODY_BYTES`; none covered the packer's use of it. The packing path
cannot reach the guard's transport refusal under manifest-derived bounds; a
hand-built `AccountingBounds` past the transport maximum and a 64 MiB render
would, and no fixture constructs one.
Impact: A body written without the measured reservation can exceed the length
the caller reserved.
Open questions: None.

### packing-adjustment-removes-last-admitted-within-the-cap

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `crates/daemon/tests/packing_serialize.rs`
`a_wrapper_overflow_removes_the_last_admitted_group_and_reclaims_its_wrappers`,
`the_exact_tokenizer_charges_fragments_independently_so_a_rebuild_drops_only_the_removed_cost`,
`cap_exhaustion_emits_nothing_and_never_removes_required_items`,
`an_accounting_overflow_is_refused_by_the_optional_phase_before_any_measurement`,
`an_exhausted_budget_refuses_before_any_measurement`, and
`a_budget_that_ends_while_the_body_is_written_refuses_the_preparation`.
Guarantee: When the closed render exceeds the serialized-bytes bound, each
pass removes exactly the last-admitted optional group and its wrappers, and
the rebuilt body equals the body an admission of the remaining groups would
have closed; the rebuilt ledger equals that admission's ledger when no group
was skipped, and otherwise differs only in its `GroupOpen`/`GroupClose`
labels, which keep the partition index while a fresh admission of the
remaining set re-indexes from zero; the loop stops at the first fitting render or when the
pass cap or the admitted list is spent, returning `AdjustmentCapExhausted`
with the exceeded bound and emitting no bytes; an exhausted evaluation budget
refuses with `Deadline` before any measurement, and a budget that ends while
the fitting body is written and hashed refuses with `Deadline` instead of
returning the preparation; the required items are in
every emitted body, because the admission carries the required render it was
scanned onto and adjustment rebuilds from that. A render past a rendered-bytes
or estimated-tokens bound never reaches adjustment: `prepare_optional`
refuses it as `PreparationRefusal::Accounting` before any measurement.
Check: `always` - a limit one below the full render removes the group with the
highest partition index, and the body and ledger equal those of a fresh
admission of the first two groups, with the total down by exactly the removed
group's charge; a cap of one suffices for that limit and a cap of zero returns
the typed failure with zero passes; a limit equal to the required render
removes all three groups in reverse admission order and emits the required
render; one byte less returns the failure with three passes and no write; a cap
of two stops at two; each accounting bound one below the closed render, fed to
both phases from one `AccountingBounds`, is refused by the optional phase
with the guard counters at `(0, 0)`, and both bounds at the render admit and
serialize with zero passes; a budget cancelled from the guard's write hook
returns `Deadline`. `always` because a removed required item or an
emitted over-budget body violates the contract.
Fault/timing angle: the window between the loop's poll and the return, while
the body is written and hashed; `guard_calls::on_write` ends the budget
inside it.
Required faults and enabling state: A serialized-bytes limit one below the
closed render; a limit below the required render; each accounting bound one
below the closed render at the optional phase; a budget cancelled from the
write hook.
Confidence: high -
[evidence](evidence/packing-adjustment-removes-last-admitted-within-the-cap.md).
Existing check: none before this change.
Impact: Adjustment that removes an arbitrary group breaks determinism;
adjustment that emits after cap exhaustion breaks the budget.
Open questions: None.

### packing-limits-fail-closed-at-manifest-parse

Type: safety
Reachability: explicit-config-only - `RuntimeManifest::parse` runs on every
manifest read, but the packing branch runs only when the manifest names at
least one `packing_*` limit; a manifest without the group leaves `packing`
as `None`, so the approval, partial-group, and malformation refusals need a
manifest that carries the group. The conversion clauses are `test-only` at
this base: `PackingLimits::from_manifest`, its `Zero`, `OutOfRange`, and
`Absent` refusals, and the four bound mappings are called from
`crates/daemon/tests/packing_serialize.rs` only.
Status: active
Exercised: yes - `crates/daemon/tests/packing_serialize.rs`
`packing_limits_join_the_manifest_as_one_approved_group`,
`an_unapproved_partial_unknown_mismatched_or_zero_packing_group_is_refused`,
and `the_packing_path_reaches_no_legacy_clamp_or_selection_module`;
`crates/daemon/tests/projection_gates.rs`
`gate_tables_match_the_frozen_construction_contract` keeps the frozen
required set unchanged.
Guarantee: The eleven `PACKING_LIMITS` are read by name as one group only
when the approval flag is enabled; a present group without the flag, a group
missing a name, an unknown name, or a non-numeric value fails at parse; a
version disagreement fails at parse before the packing branch, an inherited
manifest rule this record relies on rather than owns; a zero value for any limit but the adjustment
pass cap, or a rendered-bytes or serialized-bytes value past the transport
maximum, fails at conversion; a manifest without the group parses and yields
no limits. The packing path reaches no legacy clamp or truncation.
Check: `always` - each refusal above returns its variant; the parsed limits
feed the required, optional, and accounting bounds, compared whole against
literal bound structs; one `PackingLimits` drives both phases in
`each_serialization_bound_saturates_at_its_value_and_refuses_at_value_plus_one`,
where each body bound at the render admits and one below refuses: the
rendered-bytes and estimated-tokens bounds at the optional phase, the
serialized-bytes bound at `finalize`; a source tripwire finds none of the
legacy budget helpers in the packing sources and no `selection.rs` under
`packing/`. The tripwire is a name check, not a proof that no second
authority exists: bounds are caller-supplied values, so the single-authority
claim rests on the production caller, which U5a lands. `always` because an
unapproved or partial limit set would let the packer run under a bound nobody
approved.
Fault/timing angle: none.
Required faults and enabling state: Manifest fixtures with the group and
each malformation.
Confidence: high -
[evidence](evidence/packing-limits-fail-closed-at-manifest-parse.md).
Existing check: `crates/daemon/tests/projection_gates.rs` refusal tests cover
the frozen limit set and the vector group.
Impact: An unapproved limit silently governs production packing.
Open questions: None.

### packing-output-byte-identical-across-cache-states

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: partial - `crates/daemon/tests/packing_serialize.rs`
`identical_inputs_give_byte_identical_output_across_cache_states_threads_and_processes`
varies the byte profile across clear, warm, rotate, four threads, and a
child process; the exact tokenizer, whose counts the cache saves and whose
delta depends on the anchored tail, is not varied across cache states.
Guarantee: The same selected set, bounds, and profile yield the same body,
so the same SHA-256 identity, or the same typed refusal, with the cost cache
cleared, warm, or rotated, from four concurrent threads, and from a fresh
process, both with slack and at an estimated-tokens edge one below the closed
render, where a count off by one would flip the optional phase between
admission and `PreparationRefusal::Accounting`; the identity covers the
rendered bytes, so a different adjustment gives a different identity where a
digest of the member set would not.
Check: `always` - every identity equals its reference under each cache state;
the refusal at the token edge, value and limit included, equals its reference
under each cache state; the child process prints the same identity; the
adjusted preparation's identity differs from the full one while the digests of
their members, admitted plus removed, agree. `always` because a
cache-dependent body would make the apply-time digest check fail on a retry.
Fault/timing angle: Concurrent reads of the cost cache; the four threads
start after sequential calls refilled the rotated cache, so they hit rather
than race a miss. The cache stores the same count under the same key, so a
miss race would leave the render unaffected too; no test constructs one.
Required faults and enabling state: `cost_cache::clear` and
`cost_cache::rotate` (test-support seams) and a re-executed test binary.
Confidence: high -
[evidence](evidence/packing-output-byte-identical-across-cache-states.md).
Existing check: none before this change.
Impact: A body that depends on cache state cannot be validated at apply.
Open questions: None.
