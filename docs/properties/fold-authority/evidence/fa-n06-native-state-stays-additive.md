# fa-n06-native-state-stays-additive

## Discovery trigger

Proposed record FA-N06 of #834 (D3b: identity adoption belongs to the folding
path; D3c: under native folds nothing keyed by message id survives in
metadata). Ticket #855; PR #903, with M2 (#905, #906) moving identities and
derived state out of the metadata record. Surfaces: daemon and store.

Exercised status: partial - transform passes after a large folding state,
native state sync, a resent descent, and the 1k versus 20k native metadata
bound are witnessed. The record names a "bootstrap" writer that no witness
identifies (see the investigation log).

## Evidence trail

All references are verified at HEAD `0ff62b29a`.

- Dispatch: `apply_once` sends every native pass except a subagent lineage
  switch to `apply_additive_only` (`crates/daemon/src/transform.rs:3217-3219`).
- The additive path calls only `apply_ingress_scalars` (`:2904`), which sets
  `newest_live_block_id`, `newest_live_ordinal`, and `last_usage`
  (`:5593-5608`). Identity insertion, provisional-tail removal, tail and basis
  re-adoption, and their counters live in `apply_ingress_identities`
  (`:5612`), called only on the folding path (`:4169`).
- The additive commit carries no identity upserts and no derived state: its
  `TransformCommit` (`:3124-3143`) sets no identity delta, and its result
  sets `derived: None` (`:3162`), so no served fingerprints or baseline parts
  are promoted into the daemon cache. Since `ccab18208` (#903 on `main`) the
  commit that adopts native authority sets `clear_identities` (`:3139`),
  which deletes the session's identity rows inside that transaction
  (`crates/memory-store/src/lib.rs:11201-11203`), so a legacy
  compaction-off row that recorded identities leaves none behind.
- After #905 and #906, `ModuleMeta` holds no identity map, served
  fingerprints, or baseline parts; identities are `block_identities` rows and
  the other two are daemon cache state. The scalar `tail_hygiene_baseline`
  and `coverage_ordinal` remain in metadata
  (`crates/memory-store/src/lib.rs:2121`, fields in that struct).
- State sync on a `Some(false)` session treats the history-segment batch as
  empty (`crates/memory-store/src/lib.rs:11521-11526`), so it writes no seed
  boundary, segments, or drop and strip seeds.
- The authority reset that makes a session native deletes identity rows and
  segments and empties core and metadata (`lib.rs:13357-13427`).

Witnesses (#903 unless noted):

- `assert_native_state` (`crates/daemon/src/fold_authority_handler_tests.rs:19-29`)
  checks `eidnara_folds == Some(false)`, no identity rows, no
  `tail_hygiene_baseline`, no `coverage_ordinal`, and
  `folded_history_segment_seq == 0`. It runs in
  `native_authority_skips_every_fold_step_even_with_a_live_chain` (`:46`,
  which also asserts `newest_live_block_id` and `last_usage` are set),
  `a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`
  (`:219`), `authority_changes_survive_restarts_and_serve_first_passes`
  (`:840`, starting from a folded session with identity rows),
  `a_resent_descent_after_a_change_to_native_is_acknowledged_as_a_replay`
  (`:555`), `native_metadata_does_not_grow_with_the_message_count` (`:645`),
  and after every native transform in
  `bounded_operation_histories_follow_the_authority_model` (`:1030`).
- Store: `a_native_authority_state_sync_keeps_the_session_free_of_fold_coordinates`
  (`crates/memory-store/src/lib.rs:25225`), with an unadopted positive
  control that does write three segments and coverage.
- Read inventory: `native_pass_reads_are_bounded_independent_of_stored_rows`
  (`crates/daemon/src/transform_read_bound.rs:764`).
- Folding-path counterpart:
  `a_completed_tail_that_turns_provisional_is_removed_and_re_adopted_exactly`
  (`crates/daemon/src/transform.rs:14638`).

## Failure scenario

A long native session keeps adopting identities through shared ingress, or a
state sync seeds a boundary. Metadata or identity rows then grow with message
count under an authority that never reads them, which is the wall #834
removes.

## Timing windows and dependencies

None within a pass. The dependency is the set of writers that can touch a
native session: transform passes, state sync, lineage descent, and the
authority reset. Each must leave the forbidden fields empty.

## What a test must construct

A large folding state; a legal quiescent change to native; then each writer
in turn (additive passes over append, provisional tail, and a native slice;
a state sync with seeds; a descent edge); assert the forbidden fields are
empty and the ingress scalars updated.

## Investigation log

### Q: Which writer does "bootstrap" name?

- Sources examined: #834 D3, D3d (the `AlreadyBootstrapped` descent
  disposition); `apply_authority_state_sync` (the `seed_boundary_id` seed);
  the handler tests.
- Findings: Two readings fit. The state-sync seed boundary is witnessed
  under native authority by the store test. The lineage `AlreadyBootstrapped`
  disposition is not constructed under native authority; the descent tests
  cover `descended`, `replay`, and `not_compaction_shape`.
- Missing evidence: The spec author's meaning, and an `AlreadyBootstrapped`
  native witness if it is the lineage disposition.
- Conclusion: needs human input.

### Q: Does a native session receive served fingerprints in the daemon cache?

- Sources examined: `transform.rs:3159-3173`; `derived_state.rs`.
- Findings: No. The additive result proposes no derived state, so nothing is
  promoted.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: Does `main`'s native-adoption clear change this record?

- Sources examined: `git show ccab18208`;
  `crates/daemon/src/transform.rs:3124-3143`;
  `crates/memory-store/src/lib.rs:2610-2614`, `:11201-11203`;
  `crates/daemon/src/fold_authority_handler_tests.rs:399-422`.
- Findings: It strengthens it. A legacy compaction-off row could hold
  identity rows and no fold coordinates; adopting native authority through
  the additive path now releases them in the adopting commit, and
  `legacy_rows_adopt_by_their_fold_artifacts` asserts the native state
  after that adoption.
- Missing evidence: None.
- Conclusion: resolved with answer: evidence updated; the record stays
  `active`.
