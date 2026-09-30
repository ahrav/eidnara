# fa-n03-authority-change-requires-quiescent-bind

## Discovery trigger

Proposed record FA-N03 of #834 (D3, C2; persistence handoff co-commit set 2:
authority replacement, epoch advance, reset core and metadata, and retirement
of fold artifacts and identity rows). Ticket #855; PR #903. Surfaces: daemon
and store.

Exercised status: yes - each quiescence blocker, both directions, the
interleaved sibling bind, and recomp preservation have a named witness that
ran green in the #903 gate and in the #859 PR A full workspace run at `fd0b52aa5`.

## Evidence trail

All references are verified at HEAD `0ff62b29a`.

- The planner proposes a change only for an adopted row whose admitted intent
  disagrees, on the binding's first pass, with no sibling binding, and with
  the durable half quiescent (`crates/daemon/src/fold_authority.rs:113-130`).
  Otherwise it keeps `eidnara_folds = applied` and reports a
  `PendingAuthority` with `SiblingBound`, `NotQuiescent`, or `LaterBind`
  (`:116-138`). A blocked change never alters the serving mode.
- The durable half comes from `load_fold_authority`: `quiescent` is
  summarizer Idle and no pending publication row
  (`crates/memory-store/src/lib.rs:13236-13242`).
- The change runs inside the attempt loop
  (`crates/daemon/src/transform.rs:2026-2067`). The reset call is wrapped by
  `ctx.sibling_fence.without_sibling` (`:2035-2037`).
- `BindingFence::without_sibling` (`crates/daemon/src/lib.rs:301-313`) holds
  the `RouteBindings` mutex while it runs the reset and runs it only while the
  pass's bind sequence is present with no participating sibling
  (`RouteBindings::has_sibling`, `:351-357`). A sibling bind therefore lands
  before the reset (the change stays pending) or after it.
- `reset_session_for_authority` (`crates/memory-store/src/lib.rs:13189`) calls
  `reset_session` (`:13246`). Inside the fenced transaction it checks the
  row-version CAS, then re-checks summarizer Idle and the pending-publication
  row (`:13272-13283`) and returns `Ok(None)` without writing when either
  blocks. The same transaction writes the replacement
  (`eidnara_folds: replacement_authority.or(prior)`, `:13301`), advances
  `revert_epoch`, empties core and metadata, deletes identity rows (`:13328`),
  retires scan owners, and deletes segments, transcripts, candidates, and the
  pending publication.
- `reset_session_for_recomp` (`:13176`) passes no replacement, so the adopted
  authority survives an ordinary reset. `reset_no_survivor` uses it
  (`crates/daemon/src/transform.rs:2192`).
- A rerun transform (`rerun_transform`, `crates/daemon/src/lib.rs:9294`, which
  runs with `PassState::Reload` at `:9306`) plans with `first_pass: false`
  (`:9207-9210`), so an emergency rerun never changes authority.

Witnesses:

- `a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`
  (`crates/daemon/src/fold_authority_handler_tests.rs:219`): native to Eidnara
  and back, each with `revert_epoch` advanced by one.
- `a_sibling_binding_keeps_the_change_pending_until_a_quiescent_bind` (`:245`)
  and `a_sibling_bound_during_the_change_keeps_it_pending` (`:778`), the
  second inserting a sibling from the post-plan hook.
- `a_busy_summarizer_keeps_the_change_pending_across_a_restart` (`:289`).
- `an_emergency_rerun_after_publication_keeps_the_change_pending` (`:656`).
- `an_ordinary_recomp_reset_preserves_the_adopted_authority` (`:314`).
- Store: `the_authority_reset_writes_its_replacement_and_ordinary_resets_keep_the_authority`,
  `a_busy_summarizer_refuses_the_authority_reset`, and
  `a_pending_publication_refuses_the_authority_reset_of_an_idle_session`
  (`crates/memory-store/src/lib.rs:29456`, `:29489`, `:29516`).

## Failure scenario

A second window binds with a different chain while the first window's firing
is awaiting its producer. If the change applied, the firing would publish
segments built under the old authority into a session that now claims the
other one: new authority with old coordinates.

## Timing windows and dependencies

Two windows exist. The binding-table window spans plan to reset; the fence
mutex closes it. The durable window spans `load_fold_authority` to the reset
transaction; the in-transaction re-check closes it. The lock order is the
bindings mutex, then the store connection.

## What a test must construct

An adopted session and a disagreeing admitted bind; each blocker alone
(sibling at plan, sibling during the reset, non-Idle summarizer, pending
publication on an Idle session); a recomp reset; an emergency rerun.

## Investigation log

### Q: Does the restart witness restart the process?

- Sources examined: `fold_authority_handler_tests.rs:289-311`, `:825-838`.
- Findings: The busy-summarizer test clears the binding table in place. The
  reopened-store restart is `restarted` (`:825`), used by
  `authority_changes_survive_restarts_and_serve_first_passes` and the bounded
  histories. Neither of those holds a busy summarizer.
- Missing evidence: A reopened handler over a store with a non-Idle
  summarizer.
- Conclusion: unresolved, needs a busy-summarizer case through `restarted`.
  The store-level refusal covers the durable check.

### Q: Is a pending change applied without a new bind?

- Sources examined: `fold_authority.rs:116-117`; `lib.rs:361-369`.
- Findings: No. After the first pass settles (`settle_first_pass`), later
  passes plan `LaterBind`; only a new bind resets `first_pass_settled`.
- Missing evidence: None.
- Conclusion: resolved with answer.
