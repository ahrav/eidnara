# packing-coverage-is-a-per-identity-partition

## Discovery trigger

RP2.8 requires that no selected span disappears without a typed reason and
that coverage is a per-identity partition, not an aggregate count. Q2 names
the reasons: offsets outside the parent or overflowing, overlap disagreement,
and the empty span.

## Evidence trail

- `crates/retrieval/src/packing/grouping.rs` `Partition` holds `groups` and
  `refused`; `merge` pushes a refusal for every member it drops and `finish`
  refuses a whole run on disagreement or invalid UTF-8.
- `crates/retrieval/tests/packing_grouping.rs`
  `every_selected_span_is_grouped_or_carries_a_typed_reason` builds a map from
  every identity to its group or reason, asserts no identity appears twice,
  and asserts the map's key set equals the selected set.
- The differential proptest compares the refused set to the frozen reference
  on every generated set.

## Failure scenario

A span dropped without a reason leaves the user with less context than the
selection promised and no signal; a span in two groups is rendered twice.

## Timing windows and dependencies

None.

## What a test must construct

- One stored span per reason, plus a split pair that rejoins validly and must
  form a group.
