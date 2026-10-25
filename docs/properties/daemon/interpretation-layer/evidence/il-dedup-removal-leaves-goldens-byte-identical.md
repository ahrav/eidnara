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
- CI repeats no base comparison. Its test step
  (`.github/workflows/ci.yml:690-694`) runs `selection.rs:1461`, which
  compares current output with the current `selection-golden.json`
  (`selection.rs:1462-1568`), and the differential, which compares two
  current implementations (`crates/daemon/tests/selection_differential.rs:2252-2253`).

## Failure scenario

A golden held a dedup decision for a real harness name; removing the selector
would change it inside a move-verbatim PR.

## Timing windows and dependencies

None.

## What a test must construct

Covered for U4 by the one-time testdata diff. A recurring guarantee for later
move-only changes needs a check that diffs `crates/daemon/testdata` against
the PR base; none exists.

## Investigation log

### Q: Was `dedup_name` normalized before the dedup lookup at the U4 base?

- Sources examined: `selection.rs` at `a91dbc2b6`.
- Findings: no; the comparison read the raw name, so only an exact `mcp_`
  spelling could reach it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does CI re-check the goldens against the PR base?
- Sources examined: `.github/workflows/ci.yml:690-694`;
  `selection.rs:1461-1568`; `selection_differential.rs:2248-2253`.
- Findings: no. The tests compare against the checkout's own fixtures, so a
  change that edits code and goldens together passes.
- Missing evidence: an owner decision on a recurring base diff.
- Conclusion: needs human input; the record states the U4 diff as a one-time
  observation.

### Q: Why does confidence move from the source's `medium` to `high`?

- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: the empty testdata diff between `a91dbc2b6` and `97afdaff1` is a direct observation of the claim.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.

### Q: Does the record run under `DaemonConfig::default()`, as the source's label says?
- Sources examined: `crates/daemon/src/config.rs:147`, `:187-191`;
  `crates/daemon/src/transform.rs:3350-3351`, `:3364-3370`, `:4133`.
- Findings: no. The default `model_chain` is empty, so `eidnara_folds()` is
  false and `apply_once` hands the request to `apply_additive_only`, which
  projects the window (`transform.rs:2852`) and composes m0
  (`transform.rs:3061`) without selecting, injecting, or rendering a
  reduced call. A switched-lineage subagent request passes the gate and
  returns a passthrough. The removed selector ran inside selection, which runs from
  `transform.rs:4133` inside `apply_once`.
- Missing evidence: none.
- Conclusion: resolved with answer: `explicit-config-only`.
