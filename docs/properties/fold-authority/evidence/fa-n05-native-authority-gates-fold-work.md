# fa-n05-native-authority-gates-fold-work

## Discovery trigger

Proposed record FA-N05 of #834 (D3a: one gate in the shared preparation
entry, both callers, plus a `session.wrapup` refusal; no durable no-fire
write). Ticket #855; PR #903. Surface: daemon.

Exercised status: partial - the first-unit caller and the wrapup refusal are
witnessed under a stored native authority with a non-empty live chain. The
emergency rerun caller under native authority has no witness, and the
wrapup refusal's snapshot and boundary work are not counted.

## Evidence trail

All references are verified at HEAD `0ff62b29a`.

- The gate is the first statement of `Handler::prepare_history_summarizer`
  (`crates/daemon/src/lib.rs:9423-9434`): when
  `pass.result.fold_authority.eidnara_folds` is false it returns
  `PreparedHistorySummarizerAction::Complete(HistorySummarizerDiagnostics::disabled(NATIVE_AUTHORITY))`.
  `NATIVE_AUTHORITY` is `"native_authority"` (`:26`). `disabled` sets
  `fired = false`, `state = "disabled"`, `reason` and `no_fire` to the reason
  (`crates/daemon/src/transform.rs:1530-1540`).
- The value comes from the plan over the stored record, not from the live
  configuration: `AppliedFoldAuthority.eidnara_folds = authority.eidnara_folds`
  (`transform.rs:2117-2122`). While a change is pending, the plan keeps the
  stored value (`fold_authority.rs:131-138`).
- The fold work behind the gate is `prepare_history_summarizer_fire`
  (`lib.rs:5299`): its own `store.load` (`:5318`), the trigger evaluation, the
  `no_models` check against the live `effective_config` (`:5406`, `:5578`),
  `record_no_fire` (`:5895`), and firing assembly.
- The two callers are the first unit (`lib.rs:8976`, after the subagent arm)
  and the emergency rerun (`lib.rs:9077`). Both call the gated function.
- `session.wrapup` (`handle_session_wrapup_value`, `lib.rs:7183`) refuses at
  `:7325-7344` with `ok: false`, `disposition: "failed"`,
  `reason: "native_authority"`, and zero rounds. The refusal follows the
  wrapup latch claim and an entry `store.load` (`:7264`), and precedes the
  snapshot read (`:7358`) and boundary work. An unadopted session uses
  the binding's intent (`:7334`).
- No error variant is added; the refusal and the diagnostics reuse existing
  shapes.

Witnesses in `crates/daemon/src/fold_authority_handler_tests.rs` (#903):

- `native_authority_skips_every_fold_step_even_with_a_live_chain` (`:46`):
  the handler's live configuration has a chain; the binding is native. A hook
  between transform and preparation records the full `cache_state` load
  count, and the count is unchanged after preparation. Trigger timings are
  zero, `last_no_fire` is `None`, the producer never starts or connects, and a
  repeated pass leaves `row_version` unchanged.
- `wrapup_is_refused_under_native_authority` (`:107`): `reason` is
  `native_authority` and the producer never starts.
- `a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind` (`:245`)
  and `a_sibling_bound_during_the_change_keeps_it_pending` (`:778`): an
  Eidnara-intent binding over a stored native session still gets
  `no_fire = native_authority`.

## Failure scenario

A user adds a summarizer model to a native session while another window is
open. If preparation read the live chain, it would fire and create history
segments in a session whose harness also compacts: two fold authorities.

## Timing windows and dependencies

The configuration can change after bind. The gate reads the pass's applied
authority, which the pass computed from the store before its own load, so a
configuration edit cannot bypass it within a pass.

## What a test must construct

A stored native authority with a non-empty live chain; a first-unit pass; an
emergency pass (usage at or above 95 percent of the context limit) that
reaches the rerun caller; a `session.wrapup` call; counters for preparation
loads, trigger calls, producer calls, no-fire commits, and wrapup snapshot
and boundary reads.

## Investigation log

### Q: Does any test reach the emergency rerun caller under native authority?

- Sources examined: the handler tests; `request` defaults
  (`crates/daemon/src/lib.rs:24440-24442`, 45,000 of 50,000 tokens);
  `an_emergency_rerun_after_publication_keeps_the_change_pending`
  (`crates/daemon/src/fold_authority_handler_tests.rs:656`).
- Findings: No. The default request is 90 percent, below the emergency
  decision. The emergency rerun test runs under a stored Eidnara authority.
  Both callers share the gated function, so the gate holds structurally.
- Missing evidence: A native-authority pass that reaches `lib.rs:9077`.
- Conclusion: unresolved, needs an emergency-pressure native witness.

### Q: Are wrapup's snapshot and boundary reads counted?

- Sources examined: `wrapup_is_refused_under_native_authority`; PR #903
  "Follow-up coverage".
- Findings: No. The test asserts the reason and zero producer starts. PR
  #903 lists wrapup-specific snapshot and boundary counters as follow-up
  coverage pending an owner decision.
- Missing evidence: A counter on the transform snapshot read and the boundary
  resolution inside wrapup.
- Conclusion: needs human input (owner decision on the follow-up).

### Q: Can high usage construct the second caller under native authority?

- Sources examined: `crates/daemon/src/lib.rs:8918-8942`, `:8957-8974`,
  `:9077`, `:9428-9432` at `0ff62b29a`; the handler tests in
  `fold_authority_handler_tests.rs` and `lib.rs`; portfolio evaluation H2.
- Findings: No. Native preparation returns `Complete`, which settles in the
  first unit at any pressure; the rerun follows only a completed `Busy`
  wait. The one native handler witness (`fold_authority_handler_tests.rs:46`)
  runs at 90 percent (`lib.rs:24496-24498`); no native test runs at or
  above 95 percent. The second caller is covered structurally by the shared
  gate.
- Missing evidence: A native first-unit pass at or above 95 percent.
- Conclusion: resolved with answer for the rerun caller (structural
  coverage only); unresolved, needs the native high-pressure first-unit
  witness.

### Q: Do the window-cap firing and the archive that `main` added stay behind the gate?

- Sources examined: `crates/daemon/src/lib.rs:5299`, `:5367`, `:5500-5530`,
  `:9423-9433`; `git log 1d2cd55a0..74e347d9e` (the window-cap and archive
  commits of #901 and #902).
- Findings: Yes. `archive_window` and the window-cap cut run inside
  `prepare_history_summarizer_fire`, which only the gated
  `prepare_history_summarizer` calls, so a native pass reaches neither.
  `dfe3e3893` changed the wrapup refusal's text to name the host's native
  compaction; the refusal reason stays `native_authority`.
- Missing evidence: None.
- Conclusion: resolved with answer: the guarantee holds; the record stays
  `active`.
