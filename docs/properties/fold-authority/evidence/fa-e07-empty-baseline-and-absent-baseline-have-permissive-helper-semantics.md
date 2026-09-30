# fa-e07-empty-baseline-and-absent-baseline-have-permissive-helper-semantics

## Discovery trigger

Existing-behavior record FA-E07 of spec #834 (first comment, at `265df096`),
surface daemon. It records that the tail-hygiene helpers treat an empty
measured prefix and an absent previous baseline permissively, which is safe
only while missing retained parts cannot reach them. #858 (PR #906) moves the
parts to the daemon cache; its ticket states that FA-E07 is preserved for
genuinely empty parts and no longer reachable from absence.

Exercised status: yes - `durable_scalars_join_retained_parts_by_generation`
constructs both clauses and ran green in #906's recorded workspace gate and
in the #859 PR A run at `fd0b52aa5`.

## Evidence trail

Code references are verified at `0ff62b29a` unless another tree is named.

- At `265df096`, `same_measured_prefix(baseline: &[Part], current)` returned
  `Some(0)` for an empty baseline slice and any current sequence
  (`crates/daemon/src/tail_hygiene.rs:983-1011` there), and
  `refresh_tail_hygiene_baseline(measured, _, None, _)` built generation 1
  with `baseline_parts: measured.parts` (from `:1017` there).
- `e15a09a6` ("Keep session meta bounded as covered history grows", after
  `265df096`) added an excluded-prefix length and digest; #906 moved parts,
  length, and digest into `BaselineParts`
  (`crates/daemon/src/tail_hygiene.rs:1006-1011`).
- At HEAD `same_measured_prefix(baseline: &BaselineParts, current)`
  (`:1036-1069`) first requires `current.len() >= excluded_prefix_len +
  parts.len()` and a matching digest of `current[..excluded_prefix_len]`
  (`:1040-1045`); with an empty `parts` list the loop does nothing and the
  result is `Some(0)`.
- `refresh_tail_hygiene_baseline` (`:1080-1165`) takes `previous_parts`.
  With `previous` absent or on a bust it builds generation `previous + 1`,
  evaluable, not invalidated, with parts at that generation (`:1105-1133`).
  With a durable baseline and absent or wrong-generation parts it returns
  invalidated before `same_measured_prefix` (`:1136-1140`).
- Caller: `crates/daemon/src/transform.rs:4953-4976` (spec
  `transform.rs:4881-4893` at `265df096`). A non-bust pass with no durable
  baseline still leaves it absent; the no-previous refresh runs on a bust.
- Reachability: at `265df096` `apply_once` entered the folding path whenever
  `ctx.compaction_enabled` held (`transform.rs:3050-3052` there; default
  true, `config.rs:125` there). Since `8b1e04944` (#903) it enters only when
  the stored authority folds (`crates/daemon/src/transform.rs:3207-3208`),
  which needs a non-empty chain (`crates/daemon/src/config.rs:181-185`).

Gate results as recorded in the PR descriptions:

- #906 (#858; gate run at `1d2cd55a0`): fmt, clippy `-D warnings`, rustdoc `-D
  warnings`, workspace tests (`--all-features --locked --no-fail-fast`), and
  the marker script pass; the run log records 6,079 passed, 0 failed, 66
  ignored, with the named tests `ok`.
- #859 PR A (`fd0b52aa5`): 6,124 passed, 0 failed, 66 ignored.

## Failure scenario

A consumer passes absent parts to the prefix comparison as an empty list. The
empty prefix matches, the pass computes turn deltas over a baseline it cannot
verify, and hygiene decisions follow from it. At HEAD the parts filter
returns invalidated first, so this scenario belongs to FA-N11.

## Timing windows and dependencies

None for the helpers themselves; they are pure. The permissive result is
reached only with genuinely empty parts at the matching generation or with
no durable baseline.

## What a test must construct

Parts with an empty list at the durable generation and a current sequence
that appends; a refresh with no previous baseline; a non-empty matching
control; a missing-parts control that must invalidate.

## Investigation log

### Q: Is FA-E07 preserved or replaced by #906?

- Sources examined: #858 ticket, "Property and Verification Obligations";
  `tail_hygiene.rs:1036-1165`; `tail_hygiene.rs:2278`.
- Findings: Preserved. Both helper clauses hold; the join test's "present
  but empty" case evaluates and its `fresh` case yields generation 1,
  evaluable, with parts. Absence no longer reaches the empty-prefix path.
- Missing evidence: None.
- Conclusion: resolved with answer; `Status: active`.

### Q: Did the helper's contract change?

- Sources examined: `git log -S excluded_prefix_digest --
  crates/daemon/src/tail_hygiene.rs`.
- Findings: Yes, in scope: "matches every current sequence" now requires the
  excluded prefix to match. With `excluded_prefix_len` 0 it matches every
  sequence, as at `265df096`.
- Missing evidence: None.
- Conclusion: resolved with answer; recorded as a spec correction.

### Q: Does an empty prefix match every current sequence unconditionally?

- Sources examined: `crates/daemon/src/tail_hygiene.rs:1036-1069` at
  `0ff62b29a`; portfolio evaluation I2.
- Findings: No. The excluded prefix must match the recorded length and
  digest; the Guarantee now says so.
- Missing evidence: None.
- Conclusion: resolved with answer.
