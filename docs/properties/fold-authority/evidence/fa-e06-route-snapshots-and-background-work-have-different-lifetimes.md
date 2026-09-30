# fa-e06-route-snapshots-and-background-work-have-different-lifetimes

## Discovery trigger

Existing-behavior record FA-E06 of #834's first comment, at `265df096`.
Surface: daemon. PR #903 (#855) preserves it as enabling state: the stored
session authority (FA-N02, FA-N03) is the answer to it, not a removal of it.
Its description records "FA-E06 ... is preserved as enabling state".

Exercised status: partial - sibling bindings with unequal captured
configurations and a live configuration that differs from the captured one
are each constructed by tests green in the #903 gate and the #859 PR A run at
`fd0b52aa5`. No test observes an admitted firing that outlives route
teardown, and no test combines the three.

## Evidence trail

All references are verified at HEAD `0ff62b29a`.

- A route binding captures its configuration at bind:
  `SessionBinding.config: DaemonConfig` (`crates/daemon/src/lib.rs:227-241`).
  `RouteBindings` keys bound routes by handle with a bind sequence
  (`:254-263`), and one session can hold several participating routes
  (`has_sibling`, `:351-357`).
- The fold-authority intent is read from the captured configuration
  (`fold_authority_intent`, `:338-349`), while summarizer preparation reads
  the live configuration: `let cfg = self.effective_config(&binding.project_root)`
  (`:5406`), with `effective_config` at `:4736-4745`. Wrapup does the same
  (`:5797`). One firing reads both: the model chain is live, while its
  project-memory gate is `binding.config.memory_enabled` (`:5640`).
- An admitted firing runs as a spawned module task
  (`spawn_history_summarizer_firing`, `:6194-6209`) that owns its store handle
  and session id. `unbind_route` (`:4487-4520`) removes the binding and, for
  the last route of a session, calls `purge_session_state`; it does not join
  or cancel the spawned firing.
- What changed at #903: the durable authority makes route disagreement
  explicit. A disagreeing sibling is admitted and runs under the stored
  authority with a pending target (`fold_authority.rs:116-138`). The live
  chain still decides model selection for a firing, so an emptied chain after
  adoption is a stall (see `fa-e05-empty-chain-refusal-is-late-and-change-gated`).

Checks at HEAD:

- `a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind`
  (`crates/daemon/src/fold_authority_handler_tests.rs:245`): routes 7
  (native) and 8 (Eidnara) bound to one session at once.
- `a_sibling_bound_during_the_change_keeps_it_pending` (`:778`): a sibling
  inserted between the plan and the reset.
- `no_fire_reason_is_durable_change_gated_and_cleared_by_fire`
  (`crates/daemon/src/lib.rs:41167`): the captured binding configuration has a
  chain while the handler's live configuration has none, so preparation
  reads the live one.
- `mtime_cache_reuses_unchanged_reads_and_invalidates_on_mtime_change`
  (`crates/daemon/src/config.rs:2350`): a configuration edit is visible to the
  live read after an mtime change.

Citation corrections from `265df096` to HEAD: `lib.rs:211-251` (binding and
route table) is now `:227-263`; the live read at `:5722` is `:5406`; the
firing's memory gate from the captured configuration at `:5924-5925` is
`:5640`; `config.rs:2158` is `:2350`.

## Failure scenario

Two windows bind one session with different chains. Without a session-wide
fact, each route would act on its own captured configuration, and a firing
started under one route would keep running after that route closes while the
other route folds differently.

## Timing windows and dependencies

A configuration edit between two binds; a route close while a firing awaits
its producer. The firing's lifetime is the module task's, not the route's.

## What a test must construct

A `sometimes` witness: two bound routes on one session with unequal captured
configurations; a preparation read that sees an edited live chain; a firing
held at its producer while its route unbinds, observed still running
afterwards. This is enabling state, not publication success.

## Investigation log

### Q: Does any test hold a firing across route teardown?

- Sources examined: tests calling `unbind_route` in `crates/daemon/src/lib.rs`
  (`:19977`, `:27997`, `:32531`); `route_teardown_keeps_an_admitted_checkpoint_alive`
  (`:22934`); `transform_unit/host_tests.rs:365`.
- Findings: No. The teardown tests cover memory-capture checkpoints, note
  evaluator registrations, observational bindings, and transform scratch
  state. None blocks a history-summarizer producer and then unbinds.
- Missing evidence: A test with `ProducerState.block_output`, an unbind of
  the only route, and an observation that the firing task still completes.
- Conclusion: unresolved, needs the teardown witness.

### Q: Is the record still true at HEAD, or does the durable authority remove it?

- Sources examined: PR #903 description; `lib.rs:338-357`, `:5406`.
- Findings: Still true. Routes still capture configurations and preparation
  still reads the live one. The durable authority decides which path runs;
  it does not unify the captured configurations.
- Missing evidence: None.
- Conclusion: resolved with answer; `Status: active`.
