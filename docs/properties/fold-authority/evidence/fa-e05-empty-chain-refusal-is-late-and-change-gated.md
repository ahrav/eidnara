# fa-e05-empty-chain-refusal-is-late-and-change-gated

## Discovery trigger

Existing-behavior record FA-E05 of #834's first comment, at `265df096`.
Surface: daemon. PR #903 (#855) replaces it with the early `native_authority`
gate (FA-N05), as its description records under "Replaced property
clauses". D3 keeps the late `no_models` and `NoModels` guards on the firing
path as defense for a stored Eidnara authority whose live chain is empty.

Exercised status: partial - the surviving stall case is exercised by
`no_fire_reason_is_durable_change_gated_and_cleared_by_fire`, green in the
#903 gate and the #859 PR A run at `fd0b52aa5`. No test forces the ignored commit
error or the trigger-false arm with an empty live chain.

## Evidence trail

At `265df096` (spec citations): preparation loaded the row, evaluated the
trigger, and only then rejected an empty chain with `no_models`
(`crates/daemon/src/lib.rs:5792-5819`, `:5840-5872` at that tree), wrote the
reason through a change-gated `record_no_fire` whose commit result was
discarded (`:6161-6173`), and had no compaction gate on this path.

At HEAD `0ff62b29a`:

- Replaced: `prepare_history_summarizer` returns `native_authority` first
  whenever the pass's applied authority is native
  (`crates/daemon/src/lib.rs:9423-9434`). An empty binding chain gives
  native intent (`crates/daemon/src/config.rs:181-185`), so a fresh session
  with the default empty chain adopts native and never reaches the late
  check.
- Surviving: under an applied Eidnara authority, `prepare_history_summarizer_fire`
  (`lib.rs:5299`) still loads the row (`:5318`), evaluates the trigger, and
  on `!trigger.fire` returns `trigger_false` or `busy` (`:5553-5576`). On a
  firing trigger with an empty live chain (`cfg` from `effective_config` at
  `:5406`) it records and returns `no_models` (`:5578-5584`).
- `record_no_fire` (`lib.rs:5895-5912`) returns when the stored reason is
  equal and otherwise commits with `let _ = store.commit(...)`, ignoring the
  error.
- Defense guards: the assembler returns `NoModels` for an empty chain
  (`crates/daemon/src/history_summarizer_chunk.rs:908-912`) and the driver
  returns `HistorySummarizerDriveError::NoModels`
  (`crates/daemon/src/history_summarizer.rs:1840-1842`).
- Wrapup keeps its own late check under Eidnara authority
  (`lib.rs:5797-5802`).
- `session.status` reports this state as
  `eidnara, summarizer stalled (no models at the last pass)`
  (`lib.rs:6760-6763`), a #904 change.

Checks at HEAD:

- `no_fire_reason_is_durable_change_gated_and_cleared_by_fire`
  (`crates/daemon/src/lib.rs:41167`). The binding carries
  `default_test_config()` with a chain (`binding`, `:19675-19693`), while
  the handler's live configuration has an empty chain. The pass adopts
  Eidnara and then records `no_models`; a repeat leaves `row_version`
  unchanged.
- `session_wrapup_no_models_is_terminal_and_retains_command` (`:37569`).
- `session_status_names_a_stalled_eidnara_summarizer_in_the_authority_prefix`
  (`crates/daemon/src/fold_authority_handler_tests.rs:1037`).

Citation corrections from `265df096` to HEAD: callers `lib.rs:9014`, `:9150`
are now `:8976`, `:9077`; `:9503-9520` (the preparation entry) is
`:9475-9486` and now holds the gate; `history_summarizer_chunk.rs:619-623` is
`:908-912`; `history_summarizer.rs:1655-1657` is `:1840-1842`; the checks at
`lib.rs:42058` and `:39354` are at `:41167` and `:37569`.

## Failure scenario

Before #903, a session with no summarizer model paid for a full preparation
load and a trigger evaluation on every firing turn, then declined. After
#903, the remaining risk is a stored Eidnara session whose chain the user
removed: it keeps evaluating the trigger and never folds, and only the stall
status says so.

## Timing windows and dependencies

The live configuration can change after adoption. The stall persists until
a chain returns or a quiescent bind changes the authority to native.

## What a test must construct

A stored Eidnara authority; an empty live chain; both trigger outcomes; a
repeated reason; a failing no-fire commit (for example a CAS conflict from a
concurrent writer) that leaves the pass result unchanged.

## Investigation log

### Q: Does the stall reach production at HEAD?

- Sources examined: `fold_authority.rs:95-139`; `config.rs:181-185`;
  `lib.rs:5406`.
- Findings: Yes, with an explicit edit. A session adopts Eidnara only with an
  admitted non-empty chain; removing the chain later leaves the stored
  authority. A bind after the edit on a quiescent session changes it to
  native. A session that keeps a stale route, or a sibling binding, stays
  stalled.
- Missing evidence: None.
- Conclusion: resolved with answer; the surviving clause is
  explicit-config-only.

### Q: Is the ignored commit error still correct?

- Sources examined: `lib.rs:5895-5912`.
- Findings: A lost race drops the diagnostic write only; the next firing turn
  records it again. No test constructs the failure.
- Missing evidence: A forced commit failure inside `record_no_fire`.
- Conclusion: unresolved, needs that witness.
