# fa-n04-quiescent-change-completes

## Discovery trigger

Proposed record FA-N04 of #834 (D3: "Native to Eidnara starts from the
current native slice as a first pass, never from pre-native coverage"; a
daemon restart empties the binding table). Ticket #855; PR #903. Surfaces:
daemon and store.

Exercised status: partial - the change and the following first pass happen
inside one transform call and are witnessed across two reopened handlers. No
test kills the process between the reset commit and that pass's own commit.

## Evidence trail

All references are verified at HEAD `0ff62b29a`.

- The bound is in attempts. When the plan carries `change`, the pass calls the
  fenced reset (`crates/daemon/src/transform.rs:2026-2037`). A committed reset
  logs and `continue`s (`:2045-2054`), consuming one attempt of
  `MAX_CAS_RETRIES = 8` (`:81`). The next attempt re-reads the record; the
  stored value now equals the intent, so the plan stays (`fold_authority.rs:113-114`)
  and the pass serves on the reset row.
- A CAS conflict on the reset retries while `attempt < MAX_CAS_RETRIES`
  (`transform.rs:2061-2064`). A change requested with no attempts left fails
  the pass with `CasConflict` (`:2027-2032`).
- The reset empties core and metadata and deletes identity rows and history
  segments (`crates/memory-store/src/lib.rs:13357-13427`), so the next attempt
  resolves the submitted window with no coverage: a first pass. For Eidnara
  intent, `apply_once` then runs the folding path; for native intent, the
  additive path (`transform.rs:3217-3219`).
- The reset keeps the consumed descent edge
  (`crates/memory-store/src/lib.rs:13377-13383`), so a
  descended target replays its descent rather than copying the source again.
- A restart builds a new `Handler` over the reopened store, so the binding
  table is empty and the first bind is a first pass
  (`RouteBindings::insert` sets `first_pass_settled: false`,
  `crates/daemon/src/lib.rs:318-330`).

Witnesses in `crates/daemon/src/fold_authority_handler_tests.rs` (#903):

- `authority_changes_survive_restarts_and_serve_first_passes` (`:840`): a
  folding session with identity rows; restart; a native bind changes it
  (native state asserted, epoch + 1); restart; an Eidnara bind serves
  `big_messages_from(40)` and commits with `coverage_ordinal == None` and
  epoch + 1.
- `a_quiescent_bind_changes_authority_in_both_directions_through_the_reset`
  (`:219`).
- `a_descended_target_whose_binding_disagrees_resets_to_a_first_pass`
  (`:591`): one epoch-fenced reset after descent, `descent_completed` false,
  `ordinal_continuation_base` cleared.
- `bounded_operation_histories_follow_the_authority_model` (`:1030`): 100
  seeded histories of 32 operations with reopen; after every transform the
  stored authority and epoch equal the reference model's.

## Failure scenario

The change commits, then the process dies before the same pass commits. On
restart the stored authority already equals the new intent, so no second
reset runs. If the first pass after restart reused state from before the
reset, the session would serve pre-native coverage under Eidnara.

## Timing windows and dependencies

The reset commit and the pass commit are two transactions inside one
`apply_once_with_estimator` call. Between them another writer can move the
row; the pass then retries within the same attempt budget. The reset leaves
no coverage, so the post-restart pass is a first pass by construction; no
test proves it for a kill inside that window.

## What a test must construct

A quiescent adopted session; a disagreeing admitted bind; a native slice
(the OpenCode compaction summary head plus the recent messages); the reset;
then either a normal pass or a process kill before the pass commit and a
reopen. Assert one epoch advance, no coverage, and a committed first pass.

## Investigation log

### Q: Does the restart witness cut the process between the reset and the next transform?

- Sources examined: `fold_authority_handler_tests.rs:840-866`; PR #903
  "Tests added".
- Findings: No. The restarts fall between transform calls. The reset and the
  first pass share one call, so a restart "after reset, before the next
  transform" is only a kill inside that call.
- Missing evidence: A test-only kill point after `reset_session_for_authority`
  commits, followed by a reopen and one transform.
- Conclusion: unresolved, needs the kill-point test (the same test-only seam
  class #834 names for FA-N09).

### Q: Is the "first pass" oracle stronger than `coverage_ordinal == None`?

- Sources examined: the assertions at `:760-766`.
- Findings: The final pass is quiet (1,000 of 50,000 tokens), so no fold can
  create coverage. The test shows no pre-native coverage is reused. It does
  not use an OpenCode compaction-shaped slice.
- Missing evidence: A slice headed by the native summary message, as
  `native_metadata_bytes` (`:557`) builds.
- Conclusion: resolved with answer for coverage reuse; the slice shape is a
  refinement.

### Q: What bounds obligation (a), and how many attempts does it allow?

- Sources examined: `crates/daemon/src/transform.rs:81`, `:2013-2116`
  at `0ff62b29a`; portfolio evaluation H1.
- Findings: One call of `apply_once_with_estimator`. The attempt counter
  starts at 0 (`:2019`) and each retry requires it below
  `MAX_CAS_RETRIES = 8`, so a final attempt runs after eight retries: at
  most nine attempts, one authority reset (`:2045-2054`). The Check now
  separates this fault-free recovery (a) from the post-kill obligation (b).
- Missing evidence: The kill-point witness for (b).
- Conclusion: resolved with answer for (a); (b) unresolved, needs the
  kill-point test.
