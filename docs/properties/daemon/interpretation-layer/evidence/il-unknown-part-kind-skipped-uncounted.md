# il-unknown-part-kind-skipped-uncounted

## Discovery trigger

Issue #1105 Part A lens pass (existing tests and failure modes): the adapter skips
several part shapes with no unit, no refusal, and no counter.

## Evidence trail

- OpenCode skip arms: `crates/daemon/src/harness_sources.rs:307`
  (`ignored` text), `:308` (assistant text of a turn without
  `time.completed`), `:331` (`_ => continue` when `state.status` is neither
  `completed` nor `error`), `:334-336` (interrupted call of an incomplete
  turn), `:357` (`_ => {}` for every other part type).
- Pi skip arm: `harness_sources.rs:448`, a user or assistant content item whose `type` is not
  `text`.
- No `Count` variant names a skip (`crates/daemon/src/source_capture.rs:75-112`).
- The independent reader classifies every arm with a closed reason
  (`crates/daemon/src/source_corpus.rs:573-594`): `non_text_part`,
  `ignored_text`, `unsettled_text`, `unsettled_tool`, `skipped_item`.
- U3 witnesses: `crates/daemon/tests/source_corpus_replay.rs:225` (an `image`
  part), `:345` (a pending tool part), `:93` (the frozen OpenCode session
  holds `non_text_part` records).

## Failure scenario

A window holds a `reasoning` part and a running tool. Neither yields a unit and
no counter moves; without the reader's classification, the report would count
them as unexplained loss.

## Timing windows and dependencies

The running, pending, and interrupted arms are time-dependent: the part
becomes a unit on a later pass once the call settles or the turn completes.
The other arms are permanent for the part.

## What a test must construct

A session holding one part per arm, the reader's expected table, and a replay
asserting each class maps to one reason; for the time-dependent arms, a second
pass after the turn completes.

## Investigation log

### Q: Should the daemon count skips, or should the oracle classify them?

- Sources examined: #1105 R3 and U1; `source_corpus.rs:573-594`; the U3
  report.
- Findings: U1 landed the closed `NonUnitReason` enumeration and U3 reconciles
  every frozen session with `unclassified_loss == 0` through it.
- Missing evidence: none.
- Conclusion: resolved with answer: oracle-side classification.

### Q: Does every arm have a witness?

- Sources examined: the `replay` tests and the frozen sessions.
- Findings: `non_text_part` and `unsettled_tool` (pending) have witnesses; the
  frozen sessions' census reports one aborted call. `ignored_text` and an
  interrupted tool of an incomplete turn have no replay witness.
- Missing evidence: sessions holding those shapes.
- Conclusion: unresolved, needs a fixture with an `ignored` text part and an
  interrupted call read before the turn completes.
