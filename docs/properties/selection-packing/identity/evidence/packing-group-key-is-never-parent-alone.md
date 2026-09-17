# packing-group-key-is-never-parent-alone

## Discovery trigger

RP2.8 KTD2 makes parent identity plus canonical representation revision own
grouping, never parent alone, and Q1 asks which classes group at all. The U1
ticket adds the retrieval-owned key over class, parent, canonical revision, and
representation with a typed result for classes that do not group.

## Evidence trail

- `crates/retrieval/src/fusion/identity.rs` `ParentGroupKey::derive` reads the
  parent from the tuple through `kernel::source_identity::whole_buffer_lineage_digest`
  and refuses a revision, representation, or span that disagrees with the tuple.
- `crates/retrieval/src/packing/mod.rs` `Grouping::applies_to` encodes the Q1
  ruling: only `raw_tool_spans` groups.
- `crates/retrieval/src/packing/mod.rs` `Grouping::derive` runs
  `ParentGroupKey::derive` for every class as the tuple witness, then returns
  `NonGrouping(class)` for every class outside the grouping set and otherwise
  wraps the parent key with the class and representation as explicit
  components. A non-grouping class that skipped the witness would let a
  rewritten revision or class column pass `read_selected` and reach the
  eligibility candidate.
- `crates/retrieval/tests/packing_identity.rs`
  `grouping_keys_need_parent_revision_and_representation_together`,
  `classes_outside_the_grouping_set_yield_the_typed_non_grouping_result`,
  `grouping_refuses_columns_that_disagree_with_the_tuple`, and
  `non_grouping_rows_whose_columns_disagree_with_the_tuple_are_refused` assert
  the clauses over the kernel's real tuple encoding.

## Failure scenario

A key equal on parent alone merges a tool call's revision-1 and revision-2
output, or its `tool_output` and `tool_error`, into one group whose bytes come
from two buffers. A whole-object class given a parent key would be grouped by
a later ticket although the ruling forbids it.

## Timing windows and dependencies

None. The key is a pure function of the row.

## What a test must construct

- Spans of one tool call at two revisions and two representations, plus a
  whole-buffer occurrence of the same call.
- One well-formed occurrence of every class, persisted, then rewritten column
  by column through a raw connection and read back.
- A tuple presented with an altered revision, representation, or span, and
  every single-bit flip of the tuple.
