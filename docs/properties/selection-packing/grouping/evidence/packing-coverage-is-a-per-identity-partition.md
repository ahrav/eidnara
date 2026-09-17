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

## Investigation log

### Q: Can one identity land in a group and in the refused set?

- Sources examined: `grouping.rs` `merge` (`retain` pushes a refusal for each
  member it drops and keeps the rest) and `finish` (refuses every member of a
  disagreeing or non-UTF-8 run, or pushes the range); `group`, which assigns
  each `Selected` to exactly one bucket.
- Findings: each member takes one path; a member kept by `merge` reaches
  exactly one run, and `finish` moves that run whole to `ranges` or `refused`.
  `every_selected_span_is_grouped_or_carries_a_typed_reason` asserts the
  no-duplicate and equal-key-set clauses over one row per reason.
- Missing evidence: none.
- Conclusion: resolved with answer - the partition is structural, and the
  test asserts it per identity rather than by count.

### Q: Is an empty whole-object row a refusal or a group?

- Sources examined: `merge`, where `EmptySpan` applies only when
  `!member.whole_buffer`; `crates/kernel/src/source_identity.rs` `validate_span`,
  which accepts `span: None` on any buffer, including an empty one; the required phase, which
  admits the same payload at zero cost;
  `whole_objects_of_one_class_are_their_own_group_in_both_implementations`,
  which groups `promoted_memory("decision-d", "")` as one `0..0` range.
- Findings: the first U3 revision refused it as `EmptySpan`; review at the
  U3 change showed persistence permits the payload and the required phase
  admits it, so refusing it in the optional phase was inconsistent. Both
  implementations now emit one empty range; `packing-reference-v3` records it.
- Missing evidence: none.
- Conclusion: resolved with answer - its own group with one empty range; only
  an explicit span can be empty.

### Q: Where do the optional exclusions found before grouping belong?

- Sources examined: `crates/daemon/src/packing.rs` `prepare_optional`
  (`Duplicate` before any read; `Missing` and `Corrupt` from the read hold;
  `Stale` and `Excluded` after judgment; `Corrupt` again from the load hold or
  verification) and `excludable`, which lets only a missing or corrupt row
  continue the statements;
  `optional_faults_are_excluded_with_a_reason_and_never_refuse_the_preparation`
  and `a_corrupt_optional_payload_is_excluded_and_the_scan_continues`, the
  latter altering a payload row in the store as the required part's corrupt
  test does.
- Findings: each excluded identity appears once in `OptionalAdmission.excluded`
  with its reason, in the order found, and never reaches `group`; any other
  fault refuses the whole preparation rather than dropping one identity.
- Missing evidence: none.
- Conclusion: resolved with answer - the partition covers the identities that
  reached grouping; the exclusions are the typed reasons for those that did
  not, and the daemon test asserts them by identity.
