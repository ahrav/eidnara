# RP2.8 grouping and optional-scan properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `89c5589e` (the tip of the RP2.8 U2 branch the U3 change was authored
against). Method: `../../METHOD.md` and `property-discovery-and-catalog`.

Source: the RP2.8 specification
([#629](https://github.com/ahrav/eidnara/issues/629)), whose identity and
grouping contract, packing contract, and acceptance rows AC2, AC5, and the
AC8 optional-bound portion name these obligations, and the RP2.8 U3 ticket
([#633](https://github.com/ahrav/eidnara/issues/633)) that lands their
executable checks.

This part owns grouping as a pure function of the selected set, the per-
identity coverage partition, the optional skip-and-continue scan, and the six
optional-phase bounds. Required-phase behavior is the `required/` part.

## Observation contract

The pure functions are `crates/retrieval/src/packing/grouping.rs` `group` and
`crates/retrieval/src/packing/scan.rs` `skip_and_continue` and
`admit_optional_set`, exercised by `crates/retrieval/tests/packing_grouping.rs`
against the oracle parent in `crates/retrieval/tests/fixtures/packing/groups.json`
and the frozen reference in `crates/retrieval/tests/support/frozen_packer.rs`
(`packing-reference-v3`). The daemon entry is `crates/daemon/src/packing/mod.rs`
`prepare_optional`, exercised by `crates/daemon/tests/packing_optional.rs`;
`PackingTrace` is the observation point for stage order and payload loads.
The production packer receives selected bytes only; the parent buffer exists
in the test alone.

## Q2 rulings

Recorded by the repository owner at the U3 change:

- Two spans of one grouping key are declared adjacent when the first ends
  where the second starts. No tuple, schema, or export field is added.
- A span whose end is before its start, whose length is not its payload
  length, or that reaches past a whole-buffer sibling of the same key is
  refused with `Ungrouped::SpanOverflow`; nothing is trusted or clamped.
- Overlapping spans whose bytes differ on the overlap are refused with
  `Ungrouped::OverlapDisagreement`; every span of that run carries it.
- A span with start equal to end is refused with `Ungrouped::EmptySpan` and is
  never rendered.

Rules the change fixes beneath those rulings, encoded in both the production
grouper and the frozen reference:

- A whole-buffer sibling bounds its key only after it passed the reversed,
  empty, and length checks itself.
- A whole-object row whose payload is empty (persistence permits a zero-byte
  payload) has no span to refuse: it is its own group with one empty range,
  as the required phase admits the same payload at zero cost.
- Within a key, members sort by `(start, end, occurrence)`. Two persisted
  spans of one key never share offsets, because the identifier covers the
  span; the identifier tiebreak fixes member order for damaged rows so the
  production grouper and the frozen reference cannot disagree on it.
- A whole-object (`NonGrouping`) row is keyed by its own occurrence
  identifier in both the grouper and the reference.
- A group's fused position is that of its earliest member that landed in a
  range; a refused span never advances a group.
- A repeated optional identity is excluded as `Duplicate` before any read;
  the first request for it stands.
- The fused-candidates bound is checked on the request count before any read;
  the payload-loads bound is checked on the rows that survived exclusion. The
  kernel judges at most `MAX_ELIGIBILITY_CANDIDATES` per batch, so a caller
  fused-candidates bound above it is lowered to the cap before the check, as
  the required phase lowers its payload-loads bound.
- A missing or corrupt optional row is excluded and the next statement runs;
  any other fault, including a statement the deadline interrupted, ends the
  statements at once, as the required phase stops at its first fault.
- Grouping types report selected bytes by length in `Debug`, never by content,
  as the required phase's materialized items do.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [packing-groups-carry-only-selected-bytes](#packing-groups-carry-only-selected-bytes) | safety | test-only | always | active | high |
| [packing-coverage-is-a-per-identity-partition](#packing-coverage-is-a-per-identity-partition) | safety | test-only | always | active | high |
| [packing-optional-scan-skips-and-continues](#packing-optional-scan-skips-and-continues) | safety | default-production | always | active | high |
| [packing-optional-bounds-refuse-at-limit-plus-one](#packing-optional-bounds-refuse-at-limit-plus-one) | safety | test-only | always | active | high |

## Records

### packing-groups-carry-only-selected-bytes

Type: safety
Reachability: test-only - `group` is called from `prepare_optional` and the
tests; `prepare_optional` has no production caller at this base (`grep -rn
'prepare_optional' crates --include=*.rs` finds `crates/daemon/src/packing/mod.rs`
and `crates/daemon/tests/packing_optional.rs`).
Status: active
Exercised: yes - `crates/retrieval/tests/packing_grouping.rs`
`oracle_parent_groups_match_the_expected_tables_without_a_parent_read`,
`spans_of_another_revision_or_representation_never_merge_and_groups_follow_fused_order`,
`tied_offsets_of_distinct_occurrences_order_by_identifier_in_both_implementations`,
`whole_objects_of_one_class_are_their_own_group_in_both_implementations`,
`production_grouping_and_scan_never_diverge_from_the_frozen_reference`;
`crates/daemon/tests/packing_optional.rs`
`same_parent_spans_group_and_are_charged_as_one_merged_range`.
Guarantee: Overlapping and adjacent spans of one key merge into a range whose
bytes equal the parent bytes for the merged range assembled from the selected
bytes alone; disjoint spans stay separate ranges in offset order; no gap is
filled; every range is valid UTF-8; spans of another revision or
representation never merge; groups follow the fused order of their first
constituent; the packer never receives a parent buffer.
Check: `always` - against the oracle parent, each expected range's bytes equal
the parent slice and the groups' total byte count passes the no-gap oracle
(carried bytes equal the selected byte union); the gap-filling merger over a
gap holding a multibyte character fails that same oracle; revision and representation variants form separate
groups; `first_fused` is increasing; the frozen reference, a coverage-map
algorithm with no code in common with production, agrees on every generated
set, including sets with flipped bytes, stretched ends, reversed spans, empty
spans, and whole-object rows.
`always` because a single filled gap or dropped byte changes rendered context.
Fault/timing angle: none; grouping is a pure function.
Required faults and enabling state: Overlapping, adjacent, and disjoint spans
of one tool call; the same ranges at another revision and representation; a
disjoint pair whose gap holds a multibyte character.
Confidence: high - [evidence](evidence/packing-groups-carry-only-selected-bytes.md).
The oracle parent lives only in the fixture and the reference has no code in
common with production.
Existing check: `crates/retrieval/tests/identity.rs`
`parent_groups_share_a_parent_across_spans_and_never_replace_occurrences`
fixes the key; it merges nothing.
Impact: A filled gap renders bytes the user never selected; a merge across
revisions mixes two versions of one buffer.
Open questions: None.

### packing-coverage-is-a-per-identity-partition

Type: safety
Reachability: test-only - same as the first record.
Status: active
Exercised: yes - `crates/retrieval/tests/packing_grouping.rs`
`every_selected_span_is_grouped_or_carries_a_typed_reason`, and the
differential test's refused-set comparison; `crates/daemon/tests/packing_optional.rs`
`optional_faults_are_excluded_with_a_reason_and_never_refuse_the_preparation`
and `a_corrupt_optional_payload_is_excluded_and_the_scan_continues` for the
duplicated, missing, kernel-excluded, revision-moved, tombstoned, and corrupt
optional rows.
Guarantee: Every selected identity appears exactly once: as a member of one
group or in the refused set with an `Ungrouped` reason; no identity is dropped
without a reason and none is counted twice; a repeated optional request is
excluded as `Duplicate` while the identity's first request proceeds, and an
identity that never reaches grouping carries exactly one identity-scoped
`OptionalExclusion` reason; the preparation continues.
Check: `always` - the set of identities across groups and refusals equals the
selected set with no duplicate; the reversed, empty, overflowing,
past-whole-buffer-sibling, disagreeing, and UTF-8-splitting spans each carry
their named reason while a split pair that rejoins validly forms a group;
through the daemon entry a repeated, unpersisted, retracted, revision-moved,
tombstoned, or corrupt-payload optional row is excluded as `Duplicate`,
`Missing`, `Excluded(Retracted)`, `Stale`, `Stale`, or `Corrupt` in the order
found, a repeated request for a missing identity yields `Duplicate` for the
later request and `Missing` for the first, the duplicate of a live row is read
and loaded once, the corrupt load is not charged, and the surviving rows are
admitted. `always` because
coverage is asserted per identity, not by aggregate count.
Fault/timing angle: none.
Required faults and enabling state: A stored span that is reversed, empty,
longer than its payload, past its whole-buffer sibling, overlapping with
different bytes, or cut inside a multibyte character; an optional request that
is repeated or names no row; an optional row that is retracted, tombstoned,
whose revision moved past the request's, or whose payload bytes were altered
in the store.
Confidence: high - [evidence](evidence/packing-coverage-is-a-per-identity-partition.md).
Existing check: none before this change.
Impact: A silently dropped span is missing context with no signal; a
double-counted span is charged or rendered twice.
Open questions: None.

### packing-optional-scan-skips-and-continues

Type: safety
Reachability: default-production - `skip_and_continue` is the memory-trim
rule: `crates/daemon/src/canonical_memory.rs` calls
`crates/daemon/src/m0_compose.rs` `trim_memories_to_budget`, which delegates to
`skip_and_continue`; the read runs when `memory_enabled` is set, which
`crates/daemon/src/config.rs` defaults to `true`. The optional packer's use of the
same rule through `prepare_optional` has no production caller at this base
(same grep as the first record); the memory-trim path carries the production
reachability.
Status: active
Exercised: yes - `crates/retrieval/tests/packing_grouping.rs`
`the_scan_skips_and_continues_and_the_prefix_packer_does_not` and the
differential test; `crates/daemon/tests/packing_optional.rs`
`optional_groups_are_admitted_by_skip_and_continue_over_the_remaining_budget`;
`crates/daemon/src/m0_compose.rs` `budget_boundaries_match_the_replaced_loop`
and `skip_and_continue_delegation_matches_the_replaced_loop` for the memory
trim against its replaced loop.
Guarantee: Each optional group is visited once in fused order; a group whose
marginal cost exceeds the remaining budget is skipped and the scan continues;
unused budget is success; the production scan never diverges from the frozen
reference; the prefix packer is not the baseline.
Check: `always` - budget 10 against costs 11, 4, 6 admits positions 1 and 2 in
order with remaining 0, and the daemon entry admits the small and medium groups
behind a big one over a budget of their rendered costs; the visit counter equals the item count; a budget
larger than the sum reports the unused remainder; the prefix packer admits
nothing on the same input and differs; the frozen reference agrees on every
generated cost list and budget; the memory trim that delegates to the same
rule matches its replaced loop on boundary budgets and generated rows.
`always` because the rule is the baseline policy on every request.
Fault/timing angle: none.
Required faults and enabling state: Costs 11, 4, 6 with remaining budget 10;
rendered costs big, small, medium against small plus medium through the daemon
entry; generated cost lists and budgets.
Confidence: high - [evidence](evidence/packing-optional-scan-skips-and-continues.md).
Existing check: `crates/daemon/src/m0_compose.rs` `trim_memories_to_budget`
implemented the rule for memories before this change and now delegates to
`skip_and_continue`; its `trim_memories_tests` keep the replaced loop as the
reference model.
Impact: A prefix packer stops at the first large group and starves every
smaller later group.
Open questions: None.

### packing-optional-bounds-refuse-at-limit-plus-one

Type: safety
Reachability: test-only - same as the first record.
Status: active
Exercised: yes - `crates/retrieval/tests/packing_grouping.rs`
`optional_bounds_saturate_at_their_limit_and_refuse_at_limit_plus_one`;
`crates/daemon/tests/packing_optional.rs`
`an_optional_bound_at_limit_plus_one_refuses_with_the_bound_before_any_load`,
`a_fused_set_beyond_the_kernel_batch_cap_is_refused_before_any_read`.
Guarantee: Fused candidates, parents, spans per parent, payload loads, payload
bytes, and per-item maximum each admit a set at their supplied limit and
refuse a set one past it with a `BoundExceeded` naming the bound and the
crossing position; fused candidates are checked before any row is read and the
rest before any optional payload is loaded; a fused-candidates bound above the
kernel batch cap is lowered to the cap, so a set the kernel could not judge is
refused as a bound violation before any read.
Check: `always` - a four-span set at the exact limits is admitted; each bound
tightened by one refuses with its own name and position through the pure
functions; through the daemon entry a fused-candidates refusal leaves
`payload_loads()` at the required count, and a set one past the kernel cap
under a caller bound of twice the cap is refused with `FusedCandidates` at the
cap and no optional event is traced. The other five bounds' position before
the load hold is fixed by `prepare_optional`'s order and asserted by no daemon
test yet. `always` because every bound is a fail-closed limit.
Fault/timing angle: none.
Required faults and enabling state: Each bound set one below the fixture in
isolation; a request count one past the kernel batch cap under a caller bound
above it.
Confidence: high - [evidence](evidence/packing-optional-bounds-refuse-at-limit-plus-one.md).
Existing check: none before this change.
Impact: An unbounded optional set loads or groups without limit.
Open questions:

- The approved numeric values belong to RP2.9; tests use fixture values and
  claim no production approval. (needs human input)
- The five post-read bounds are asserted before any load through the pure
  function only; a daemon test per bound is queued in
  `portfolio-evaluation.md`.

## Relationship map

- `identity/` fixes the grouping key that `GroupIdentity::of` reads; this part
  merges within a key and never derives one.
- `required/` produces the `remaining` budget the scan record consumes and
  precedes every optional event; its integer-budget record covers the memory
  trim's float clamp that sits before the shared scan.
- Within this part, the coverage partition is the domain of the bytes record
  (every grouped identity is a partition member), the bounds record gates
  which rows reach grouping, and the scan record consumes the groups' costs.
- `OptionalCostOverflow` is exercised by `crates/daemon/tests/packing_optional.rs`
  but carries no record here; the optional phase's acquisition deadline is
  exercised there too, its mid-statement interruption is not. Both are in
  `portfolio-evaluation.md`.
