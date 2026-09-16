# RP2.8 selection identity properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
Base: `cb259ee05a610ad56beea8f3bd2c414e1b3eb771` (the tip of the RP2.7.U1
branch the U1 change was authored against). Method: `../../METHOD.md` and
`property-discovery-and-catalog`.

Source: the RP2.8 specification
([#629](https://github.com/ahrav/eidnara/issues/629)), whose identity and
grouping contract and acceptance row AC1 name these obligations, and the RP2.8
U1 ticket ([#631](https://github.com/ahrav/eidnara/issues/631)) that lands
their executable checks. The parent identity and parent-group key the records
consume are frozen by the RP2.7 fusion catalog
(`../../fusion-routes/fusion/catalog.md`,
`fusion-parent-groups-are-not-voters`).

This part owns what the packer reads: per-occurrence attribution over shared
payload bytes, the grouping key, and the classes that yield no key. Budget,
grouping of spans into groups, accounting, and application are separate parts.

## Observation contract

The observation point is the public API of `crates/retrieval/src/packing.rs`,
exported at `crates/retrieval/src/lib.rs:27` and exercised by
`crates/retrieval/tests/packing_identity.rs`. Persistence and collision refusal
are the existing `crates/retrieval/src/lib.rs` `persist_occurrences` path.
Eligibility is observed only through `crates/retrieval/src/eligibility.rs`
`judge_occurrences`; the retrieval crate holds no verdict.

## Q1 ruling

Only `raw_tool_spans` groups. Its occurrences are the one class cut into
ranges of a parent buffer. `messages`, `canonical_claims`, `promoted_memory`,
and `git_commits` persist whole objects and derive the typed non-grouping
result. Recorded by the RP2.8 owner at the U1 change;
`crates/retrieval/src/packing.rs` `Grouping::applies_to` encodes it.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [packing-attribution-follows-the-occurrence-row](#packing-attribution-follows-the-occurrence-row) | safety | test-only | always | active | high |
| [packing-group-key-is-never-parent-alone](#packing-group-key-is-never-parent-alone) | safety | test-only | always | active | high |

## Records

### packing-attribution-follows-the-occurrence-row

Type: safety
Reachability: test-only - `read_selected` and `SelectedOccurrence` are called
from `crates/retrieval/tests/packing_identity.rs` only; `grep -rn 'packing::'
crates --include=*.rs` outside `crates/retrieval/src/packing.rs` finds that
test file alone, so no packer reads a selection at this base.
Status: active
Exercised: yes - `crates/retrieval/tests/packing_identity.rs`
`byte_twins_share_one_payload_row_and_keep_their_own_attribution` and
`reads_refuse_unknown_identities_oversized_selections_and_disagreeing_columns`; the collision
half is `crates/retrieval/tests/occurrences.rs`
`forced_collisions_refuse_unequal_values_and_replay_keeps_identities`.
Guarantee: Two occurrences with equal payload bytes and distinct identities
persist as one payload row and two occurrence rows, each read back with its own
sensitivity, provenance, and eligibility candidate; differing bytes under one
payload digest are refused with the typed collision error and unchanged row
counts; no read resolves attribution through a payload identifier.
Check: `always` - after persisting two byte twins the payload count is 1 and
the occurrence count is 2; `read_selected` over both identities returns two
entries whose sensitivity and source match their own rows and whose payload
references are equal; keying the read set by payload reference leaves one
survivor whose sensitivity differs from the lost twin's, and reading the
payload identifier as an occurrence identifier is refused as unknown; a stored
revision, representation, or span column that disagrees with the tuple, or a
tuple that does not digest to the row's identifier, makes the read refuse the
row as corrupt; the eligibility report judged from the
returned candidates names both occurrences in order; a forced payload
collision returns `PayloadCollision` and writes nothing. `always`
because every read must hold it, not only a campaign's reachable subset.
Fault/timing angle: none; persistence and read are pure functions of the rows.
Required faults and enabling state: Two raw tool spans over one buffer with
different tool calls, sensitivities, and sources; a payload digest seam that
presents other bytes under a stored digest.
Confidence: high - [evidence](evidence/packing-attribution-follows-the-occurrence-row.md).
Every clause is asserted by a test at this base, and the collision seam is the
existing `test-support` digest override.
Existing check: `crates/retrieval/tests/occurrences.rs`
`forced_collisions_refuse_unequal_values_and_replay_keeps_identities` covers
the collision refusal and row counts before this change.
Impact: A payload-keyed read would hand one occurrence's sensitivity or
eligibility to a byte twin from another source, leaking or hiding bytes under
the wrong verdict.
Open questions: None.

### packing-group-key-is-never-parent-alone

Type: safety
Reachability: test-only - `Grouping::derive` is called from
`crates/retrieval/src/packing.rs` `decode` and from
`crates/retrieval/tests/packing_identity.rs`; the read itself has no
production caller at this base (same grep as the first record).
Status: active
Exercised: yes - `crates/retrieval/tests/packing_identity.rs`
`grouping_keys_need_parent_revision_and_representation_together`,
`classes_outside_the_grouping_set_yield_the_typed_non_grouping_result`,
`grouping_refuses_columns_that_disagree_with_the_tuple`, and
`non_grouping_rows_whose_columns_disagree_with_the_tuple_are_refused`.
Guarantee: A grouping key is class, parent identity, canonical revision, and
representation together; occurrences of one parent that differ only in
revision or only in representation derive unequal keys; a class outside the
grouping set derives the typed non-grouping result rather than a parent-only
key; stored columns that disagree with the tuple bytes are refused for every
class, grouping or not.
Check: `always` - spans of one tool call at one revision and representation
derive one key regardless of span; a revision, representation, or tool-call
change derives another key, and the parent alone matches exactly when only
revision changed; every class outside `raw_tool_spans` derives
`Grouping::NonGrouping(class)`; a revision, representation, or span column
that disagrees with the tuple is refused with `TupleMismatch` for every class;
a stored row of every non-grouping class whose revision, span, representation,
or class-and-representation column pair was rewritten under its tuple is
refused by `read_selected` as `CorruptRow`, as is a `raw_tool_spans` row
relabelled to a non-grouping class; every single flipped tuple bit is refused
or derives a different key. `always` because the key is a pure function of the
row on every derivation.
Fault/timing angle: none.
Required faults and enabling state: Spans of one tool call at two revisions
and two representations; one occurrence of every class, persisted and then
rewritten column by column; a tuple presented with an altered column or a
flipped bit.
Confidence: high - [evidence](evidence/packing-group-key-is-never-parent-alone.md).
Every clause is asserted by a test at this base over the kernel's real tuple
encoding.
Existing check: `crates/retrieval/tests/identity.rs`
`parent_groups_share_a_parent_across_spans_and_never_replace_occurrences`
covers the parent-plus-revision key from RP2.7.U1; it has no class or
representation clause and no non-grouping result.
Impact: A key equal on parent alone would merge bytes from two revisions or
two representations of one source into one group; a parent-only key for a
whole-object class would invent groups the Q1 ruling forbids.
Open questions: None.
