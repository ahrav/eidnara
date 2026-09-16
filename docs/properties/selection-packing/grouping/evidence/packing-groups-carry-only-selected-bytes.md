# packing-groups-carry-only-selected-bytes

## Discovery trigger

RP2.8's identity and grouping contract merges only overlapping or declared-
adjacent half-open ranges within one parent, revision, and representation,
keeps disjoint spans in offset order, fills no gap, preserves UTF-8, and never
loads a whole parent to group. Acceptance row AC2 names the oracle parent and
the gap-filling negative control.

## Evidence trail

- `crates/retrieval/src/packing/grouping.rs` `group` partitions by
  `GroupIdentity`, sorts members by offset, and merges a member into the
  current run only when it starts at or before the run's end; the merged bytes
  are the run's bytes plus the member's bytes after the overlap. The function
  receives `Selected { row, bytes }` and nothing else.
- `crates/retrieval/tests/fixtures/packing/groups.json` holds the oracle
  parent and expected group tables; the test slices the parent to check each
  range and compares the groups' byte total to the selected byte union.
- `crates/retrieval/tests/support/frozen_packer.rs` encodes the rule text
  independently; the differential proptest compares groups, ranges, bytes,
  members, and refusals over generated sets.
- `crates/daemon/tests/packing_optional.rs` shows the daemon entry charging a
  two-range group as one cost equal to its merged byte length.

## Failure scenario

A merger that reads the parent for `[first.start, last.end)` renders the gap
between two disjoint spans, including bytes the selection never named; a
merger that concatenates without checking the overlap duplicates bytes.

## Timing windows and dependencies

None.

## What a test must construct

- An oracle parent with a multibyte character in a gap.
- Spans that overlap, abut, and stand apart, at two revisions and two
  representations.
- The frozen reference and a seeded generator over spans at character
  boundaries.
