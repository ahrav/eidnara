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

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [packing-body-serialized-once-through-the-guard](#packing-body-serialized-once-through-the-guard) | safety | test-only | always | active | high |
| [packing-adjustment-removes-last-admitted-within-the-cap](#packing-adjustment-removes-last-admitted-within-the-cap) | safety | test-only | always | active | high |
| [packing-limits-fail-closed-at-manifest-parse](#packing-limits-fail-closed-at-manifest-parse) | safety | default-production | always | active | high |
| [packing-output-byte-identical-across-cache-states](#packing-output-byte-identical-across-cache-states) | safety | test-only | always | active | high |

## Records

### packing-body-serialized-once-through-the-guard

Type: safety
Reachability: test-only - `finalize` has no production caller at this base
(`grep -rn 'finalize(' crates/daemon/src --include=*.rs` finds the packing
module and its tests).
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
`PreparedOutput::measure`, reserved to exactly the measured length, and
written exactly once through `MeasuredOutput::write_to`; a refused
preparation is never written; the body equals the closed ledger's text and
satisfies the serialized-bytes bound and both accounting bounds.
Check: `always` - the guard's per-thread call counters read `(1, 1)` for a
fitting render under the byte profile and the exact tokenizer, `(2, 1)` after
one adjustment pass, and `(4, 0)` when three passes end in refusal; the body's
length equals the ledger's rendered bytes and the serialized-bytes limit set
to that value admits it; the body's bytes equal the ledger text. `always`
because one over-budget byte is a broken provider contract.
Fault/timing angle: none.
Required faults and enabling state: A limit equal to the render's length; the
`guard_calls` test-support counters in `crates/daemon/src/dispatch.rs`.
Confidence: high -
[evidence](evidence/packing-body-serialized-once-through-the-guard.md).
Existing check: `crates/daemon/tests/prepared_output.rs` covers the guard's
own measure-then-write contract, including refusal one past
`MAX_WIRE_BODY_BYTES`; none covered the packer's use of it. The packing path
reaches the guard's transport refusal only through a render past 64 MiB, which
no fixture constructs.
Impact: A body written without the measured reservation can exceed the length
the caller reserved.
Open questions: None.

### packing-adjustment-removes-last-admitted-within-the-cap

Type: safety
Reachability: test-only - as above.
Status: active
Exercised: yes - `crates/daemon/tests/packing_serialize.rs`
`a_wrapper_overflow_removes_the_last_admitted_group_and_reclaims_its_wrappers`,
`cap_exhaustion_emits_nothing_and_never_removes_required_items`,
`an_accounting_overflow_is_repaired_the_same_way`, and
`an_exhausted_budget_refuses_before_any_measurement`.
Guarantee: When the closed render exceeds the serialized-bytes bound or an
accounting bound, each pass removes exactly the last-admitted optional group
and its wrappers, and the rebuilt ledger equals the ledger an admission of the
remaining groups would have closed; the loop stops at the first fitting render
or when the pass cap or the admitted list is spent, returning
`AdjustmentCapExhausted` with the exceeded bound and emitting no bytes; an
exhausted evaluation budget refuses with `Deadline` before any measurement;
the required items are in every emitted body, because the admission carries
the required render it was scanned onto and adjustment rebuilds from that.
Check: `always` - a limit one below the full render removes the group with the
highest partition index, and the body and ledger equal those of a fresh
admission of the first two groups, with the total down by exactly the removed
group's charge; a cap of one suffices for that limit and a cap of zero returns
the typed failure with zero passes; a limit equal to the required render
removes all three groups in reverse admission order and emits the required
render; one byte less returns the failure with three passes and no write; a cap
of two stops at two; an estimated-tokens overflow yields the same body as the
serialized-bytes overflow. `always` because a removed required item or an
emitted over-budget body violates the contract.
Fault/timing angle: none.
Required faults and enabling state: Serialized-bytes and estimated-tokens
limits one below the closed render; a limit below the required render.
Confidence: high -
[evidence](evidence/packing-adjustment-removes-last-admitted-within-the-cap.md).
Existing check: none before this change.
Impact: Adjustment that removes an arbitrary group breaks determinism;
adjustment that emits after cap exhaustion breaks the budget.
Open questions: None.

### packing-limits-fail-closed-at-manifest-parse

Type: safety
Reachability: default-production - `RuntimeManifest::parse` runs on every
manifest read.
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
missing a name, an unknown name, a non-numeric value, or a version
disagreement fails at parse; a zero value for any limit but the adjustment
pass cap, or a rendered-bytes or serialized-bytes value past the transport
maximum, fails at conversion; a manifest without the group parses and yields
no limits. The packing path reaches no legacy clamp or truncation.
Check: `always` - each refusal above returns its variant; the parsed limits
feed the required, optional, and accounting bounds, compared whole against
literal bound structs; a source tripwire finds none of the legacy budget
helpers in the packing sources and no `selection.rs` under `packing/`. The
tripwire is a name check, not a proof that no second authority exists: bounds
are caller-supplied values, so the single-authority claim rests on the
production caller, which does not exist at this base: `finalize` has no
route. `always` because an unapproved or partial
limit set would let the packer run under a bound nobody approved.
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
Exercised: yes - `crates/daemon/tests/packing_serialize.rs`
`identical_inputs_give_byte_identical_output_across_cache_states_threads_and_processes`.
Guarantee: The same selected set, bounds, and profile yield the same body,
so the same SHA-256 identity, with the cost cache cleared, warm, or rotated,
from four concurrent threads, and from a fresh process, both with slack and at
an estimated-tokens edge where a count off by one would change the
adjustment; the identity covers the rendered bytes, so a different adjustment
gives a different identity where a digest of the member set would not.
Check: `always` - every identity equals its reference under each cache state;
the child process prints the same identity; the adjusted preparation's
identity differs from the full one while the digests of their members,
admitted plus removed, agree. `always` because a cache-dependent body would
make the apply-time digest check fail on a retry.
Fault/timing angle: Concurrent misses on the cost cache; the cache stores the
same count under the same key, so the render is unaffected.
Required faults and enabling state: `cost_cache::clear` and
`cost_cache::rotate` (test-support seams) and a re-executed test binary.
Confidence: high -
[evidence](evidence/packing-output-byte-identical-across-cache-states.md).
Existing check: none before this change.
Impact: A body that depends on cache state cannot be validated at apply.
Open questions: None.
