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

## Investigation log

### Q: Can the production grouper reach a parent buffer?

- Sources examined: `crates/retrieval/src/packing/grouping.rs` `Selected` and
  `group`; `crates/daemon/src/packing.rs` `prepare_optional`, which builds
  each `Selected` from the row and the bytes `fetch_payload` returned for that
  row alone; `grep -rn groups.json crates --include=*.rs`.
- Findings: `group` takes `&[Selected<'_>]` and nothing else; the only reader
  of the oracle parent is `crates/retrieval/tests/packing_grouping.rs`.
- Missing evidence: none.
- Conclusion: resolved with answer - the parent exists in the test alone, so
  a gap can only be filled by bytes the selection carried.

### Q: Is adjacency declared by the data or inferred by the grouper?

- Sources examined: the Q2 rulings in `catalog.md`; `grouping.rs` `merge`,
  whose run condition is `member.start <= current.span.end` after sorting by
  `(start, end, occurrence)`.
- Findings: adjacency is the offset relation "first ends where second starts";
  no tuple, schema, or export field declares it, and the frozen reference
  applies the same relation from the rule text.
- Missing evidence: none.
- Conclusion: resolved with answer - inferred from offsets under the recorded
  ruling; a change to the relation is a protocol change to both implementations.

### Q: Does the frozen reference share code or assumptions with production?

- Sources examined: `crates/retrieval/tests/support/frozen_packer.rs` imports
  (`std::collections::BTreeMap` only) and its coverage-map algorithm; the
  differential proptest's seeded generator and damage cases.
- Findings: the reference imports nothing from `retrieval::packing`; it marks
  each selected byte in a coverage map and reads runs off the map, while
  production merges sorted members. Both are written from the same rule text,
  so a misreading of the text would be shared.
- Missing evidence: an oracle independent of the rule text beyond the fixture
  tables in `groups.json`.
- Conclusion: resolved with answer - no code in common; the shared-text risk is
  bounded by the hand-written fixture tables, which the first test checks
  against the parent slice directly.
