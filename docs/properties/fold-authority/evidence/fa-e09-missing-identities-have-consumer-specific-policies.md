# fa-e09-missing-identities-have-consumer-specific-policies

## Discovery trigger

Existing-behavior record FA-E09 of spec #834 (first comment, at `265df096`),
surfaces daemon and store. Two consumers read stored block identities with
opposite policies for a missing entry. D4 keeps both policies when identities
become rows; #857 (PR #905) moves both readers onto one requested-mid lookup
and states that FA-E09 is preserved.

Exercised status: partial - the missing-row, changed-row, and epoch-mismatch
refusals and the transform's skip ran green in #905's recorded workspace gate
and in the #859 PR A run at `fd0b52aa5`. No test constructs a firing with
an empty selected set.

## Evidence trail

Code references are verified at `0ff62b29a` unless another tree is named.

- Transform: `enforce_block_identity` (`crates/daemon/src/transform.rs:5471`)
  reads through `WindowIdentities::get` and continues when it returns `None`
  (`:5485-5487`), so a missing entry yields no re-adoption and no
  `FrozenTargetDrift`; `apply_ingress_identities` then adopts the projected
  vector (`:5644-5651`).
- Publication: `publish_history_summarizer_chunk`'s transaction checks the
  predicate (`crates/memory-store/src/lib.rs:14124-14136`), refuses an empty
  selected set (`:14138-14143`), refuses a firing whose
  `withdrawn_selected_mid` is set (`:14144-14148`), reads the selected mids
  through `lookup_block_identity_rows` (`:14149-14155`), refuses any mid
  whose stored vector is absent or differs (`:14156-14171`), and then
  refuses a revert epoch mismatch as `CasConflict` in
  `history_publication_fence_tx` (`:6645-6650`, called at `:14174-14183`).
- Withdrawal (#905, merged as `21681e0ea`): identity rows are no longer
  pruned to the window, so a selected mid outside the window keeps a
  matching row. `withdraw_selection_outside_window`
  (`crates/daemon/src/transform.rs:5431-5469`) records the first selected
  mid the submitted window no longer holds on the in-flight firing, and the
  fence refuses that firing;
  `publish_rejects_a_firing_whose_selected_message_left_the_window`
  (`crates/memory-store/src/lib.rs:25993`) witnesses the refusal.
- At `265df096` the fence compared against the hydrated
  `meta.block_identity_by_mid` (`lib.rs:11845-11851` there) and the
  transform read the same map (`transform.rs:5367-5369` there). #905 removed
  the field and routed both through the row lookup.
- `IdentityDrift` and `identity_drift_requires_reject`
  (`transform.rs:5431-5444` at `265df096`) were deleted by `6477c9f27`
  ("Delete covered-drift rejection and the serialized-output memo",
  window-protocol #833, D25). A stored-versus-new mismatch now re-adopts or,
  for a frozen tail target, refuses with `FrozenTargetDrift` (`:5512-5516`).
- Reachability: enforcement runs only past the native gate
  (`crates/daemon/src/transform.rs:3207-3208`, call `:3684`); publication
  needs an admitted firing.

Gate results as recorded in the PR descriptions:

- #905 (#857; gate run at `871ebfb08`, base `57a820bb6`): fmt, clippy `-D
  warnings`, rustdoc `-D warnings`, `cargo +1.98 test --workspace
  --all-features --locked --no-fail-fast`, and the marker script pass; the
  run log records 6,064 passed, 0 failed, 66 ignored, with each named test
  `ok`.
- #859 PR A (`fd0b52aa5`): 6,124 passed, 0 failed, 66 ignored.

## Failure scenario

Publication treats a missing row as a match and publishes a summary of
content that changed, or the transform treats a missing row as drift and
refuses every pass after a provisional-tail delete.

## Timing windows and dependencies

- A selected message's row is edited or deleted during the producer await.
- A reset advances the revert epoch during a firing.
- A provisional pass deletes the tail row; the completing pass finds none.

## What a test must construct

A configured firing with its selected rows stored; during the await, each of:
the row removed, the row changed, the epoch advanced; a firing with an empty
selected set; a legal unrelated-tail extension that still publishes; a
transform pass over a mid whose row was deleted.

## Investigation log

### Q: Is FA-E09 preserved by #905?

- Sources examined: #905 description; the fence
  (`crates/memory-store/src/lib.rs:14124-14183`);
  `crates/daemon/src/transform.rs:5485-5487`; tests at
  `crates/memory-store/src/lib.rs:25938`,
  `crates/daemon/src/history_summarizer.rs:4055`,
  `crates/memory-store/src/lib.rs:30032`, and
  `crates/daemon/src/transform.rs:14628` (lines at `0ff62b29a`).
- Findings: Yes. The fence refuses a missing row
  (`publish_rejects_a_selected_message_whose_identity_row_is_gone`) and a
  changed row (`selected_range_identity_drift_during_await_rejects_without_cooldown`),
  and an epoch mismatch is a `CasConflict`
  (`publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict`).
  The round-trip test deletes the tail row and re-adopts a different final
  vector without error.
- Missing evidence: None for these clauses.
- Conclusion: resolved with answer; `Status: active`.

### Q: Is the empty-selected-set refusal witnessed?

- Sources examined: `git grep` for the refusal text and for empty
  `selected_range_identities` in tests.
- Findings: The text appears only at `crates/memory-store/src/lib.rs:14100`;
  no test builds an empty predicate.
- Missing evidence: A publication with an empty selected set.
- Conclusion: unresolved, needs that test.

### Q: Does #905's withdrawal record change the publication policy?

- Sources examined: `git show f0e39d04d 64a8bf371`;
  `crates/daemon/src/transform.rs:5431-5469`;
  `crates/daemon/src/history_summarizer.rs:533-555`;
  `crates/memory-store/src/lib.rs:14124-14183`, `:25791`.
- Findings: It adds a refusal. Identity rows are no longer pruned to the
  window, so a selected mid outside the window keeps a matching row. The
  transform records the first such mid on the in-flight firing
  (`withdrawn_selected_mid`), the firing's own transitions keep it
  (`keep_fields_other_writers_own`), and the fence refuses a firing that
  carries it before the per-mid comparison. The missing-row and changed-row
  refusals and the transform's skip of a missing row are unchanged, so the
  guarantee holds with the withdrawal as one more publication refusal.
- Missing evidence: None.
- Conclusion: resolved with answer: Check and evidence updated; the record
  stays `active`.
