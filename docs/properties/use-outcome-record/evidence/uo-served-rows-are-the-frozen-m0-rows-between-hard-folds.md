# uo-served-rows-are-the-frozen-m0-rows-between-hard-folds

## Discovery trigger

Spec #1145 surface S1: the record writer (#1155) stores the ids of the frozen
composition, so the served rows between folds must be that composition.

Record kind: existing behavior, reconstructed from the specification catalog and
re-verified at this branch tip.

## Evidence trail

References are verified at the #1157 branch tip (the commit that adds this file;
its parent is `d85de937b`).

- The additive engine composes m0 from the pinned read inside its HARD arm and
  assigns the composition at `crates/daemon/src/transform.rs:3122`.
- Soft and Defer passes keep the frozen m0 unit and the stored
  `meta.project_memory`; the response field `project_memory` is read from meta
  (`apply_additive_only` (`crates/daemon/src/transform.rs:2854`)).
- Each pass still reads the Kernel (`project_memory_read`
  (`crates/daemon/src/lib.rs:5509`)); the read feeds only the m1 revision
  signal, and a changed revision requests a HARD fold
  (`project_memory_revision_change_requests_a_hard_fold`
  (`crates/daemon/src/transform.rs:13993`)).
- Correction: the spec cited `transform.rs:13972`, which at this tip lies inside
  the test
  `withheld_canonical_read_composes_no_block_and_differs_from_empty_memory`; the
  production sites are the composition assignments named above.

- Guarantee under test: Between two HARD folds of a session, the served
  `<project-memory>` rows are the composition frozen into session meta at the
  last HARD fold (m0), not the result of a later Kernel read.
- Existing check:
  `transform_composes_project_memory_from_canonical_rows_and_observes_absence`
  (`crates/daemon/tests/transform_canonical_memory.rs:46`) (third pass keeps
  `project_memory`); `project_memory_revision_change_requests_a_hard_fold`
  (`crates/daemon/src/transform.rs:13993`). Status: unaudited.

## Failure scenario

A pass after a disposition commit serves rows from the new read while meta still
names the old revision, so the record and the bytes disagree.

## Timing windows and dependencies

The window is between the HARD fold's memory-store commit and the next HARD
fold.

## What a test must construct

A HARD fold, a disposition commit that changes the visible rows, and a SOFT
pass; assert m0 bytes and `meta.project_memory` are unchanged, then force a HARD
fold and assert the new composition.

Required faults and enabling state: A session with a HARD fold over a Canonical
composition, then a Kernel commit that changes the visible rows, then a non-HARD
pass.

## Investigation log

### Q: Does any open question remain for this record?

- Sources examined: the specification catalog entry, the code and tests cited above.
- Findings: the cited code and tests agree with the guarantee at this tip.
- Missing evidence: none for the guarantee as stated.
- Conclusion: resolved with answer - no open question.
