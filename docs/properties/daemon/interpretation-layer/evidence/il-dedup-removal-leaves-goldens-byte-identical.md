# il-dedup-removal-leaves-goldens-byte-identical

## Discovery trigger

Issue #1105 Part B S2 (R8): U4's execution note made a golden diff in the removal a
defect.

## Evidence trail

- `git diff --stat a91dbc2b6 97afdaff1 -- crates/daemon/testdata` prints
  nothing: U4 (#1117, merged as `97afdaff1`) changed no golden or fixture.
- The U4 diff deletes the dedup list, `dedup_name`, and `select_tool_dedup`
  from `crates/daemon/src/selection.rs` and the mirrored constants from
  `crates/daemon/tests/selection_differential.rs`, which now reads the module
  through `daemon::tool_vocabulary_evaluation` (`selection_differential.rs:39`).
- `selection_differential.rs:2248` `optimized_matches_frozen_reference_on_identical_calls`
  generates identical calls under every name in `TOOLS`
  (`selection_differential.rs:1389-1415`), including `mcp_read` and `mcp_grep`.
- `crates/daemon/src/selection.rs:2828` asserts no decision for identical
  calls.

## Failure scenario

A golden held a dedup decision for a real harness name; removing the selector
would change it inside a move-verbatim PR.

## Timing windows and dependencies

None.

## What a test must construct

Covered.

## Investigation log

### Q: Was `dedup_name` normalized before the dedup lookup at the U4 base?

- Sources examined: `selection.rs` at `a91dbc2b6`.
- Findings: no; the comparison read the raw name, so only an exact `mcp_`
  spelling could reach it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: the empty testdata diff between `a91dbc2b6` and `97afdaff1` is a direct observation of the claim.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.
