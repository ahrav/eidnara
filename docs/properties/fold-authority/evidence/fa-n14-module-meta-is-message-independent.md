# fa-n14-module-meta-is-message-independent

## Discovery trigger

Proposed record FA-N14 of specification #834 (constraint C3, decisions D4a
and D4d), owned by ticket #859 and landed in three slices: #859 PR A
(`fold-authority/m2-exit`, `d7b330113`), #859 PR B
(`fold-authority/m2-exit-covered-systems`, `b45416ac0`), and #859 PR C
(`fold-authority/m2-exit-bounds`, code tree `0ff62b29a`). Its entry
depends on #857 (PR #905, identities leave the record) and #858 (PR #906,
served fingerprints and baseline parts leave the record). The ticket asks
that every field left in `ModuleMeta` be scalar or byte-capped and that
the `(N, W)` x authority x firing-state regression pass.

Exercised status: yes - #859 PR C's recorded run at `0ff62b29a` (6,156
passed) ran the composite witness, the render identity test, the matrix, the bound table, the
summarizer inventory, the per-field caps, the store-shaping tests, and the
caller-side pair tests; #859 PR B's run at `b45416ac0` (6,131 passed) ran
the covered-row tests. The composite witness commits a synthetic stress
state.

## Evidence trail

All references are verified at `0ff62b29a`.

- Write path: `commit_transform` shapes `meta` with `shape_stored_meta`,
  serializes it, and scans it with `json_content`
  (`crates/memory-store/src/lib.rs:11034-11041`, `:3028-3038`); the
  single-pass JSON preparation applies `ensure_durable_text_bound` first
  (`:4303`, `:4577-4583`); the bound is
  512 KiB (`:429`).
- Bound table: `recorded_metadata_bounds`
  (`crates/daemon/src/transform_meta_bound.rs:610-729`), asserted by
  `every_metadata_field_has_a_recorded_bound_within_the_headroom`
  (`:732-748`) and reused by the composite witness. The `inventory!` macro
  (`:242-249`) destructures `ModuleMeta` exhaustively (`:637`), so a new
  field fails to compile until it is listed. Every entry is a byte count;
  the table has no `Unbounded` or `Configured` variant.
  `legacy_history_segment_seqs` is charged at 512 sequences (`:624`,
  `:675`), `history_summarizer` at `history_summarizer_bound()` (`:682`),
  `last_recut` at `ascii(MAX_LAST_RECUT_BYTES)` (`:652`),
  `pending_rewrite_last_failure` at `ascii(MAX_PENDING_REWRITE_DETAIL_BYTES)`
  (`:661`), `last_render_config` at
  `text(5 * MAX_REQUEST_IDENTITY_BYTES) + RENDER_IDENTITY_FRAMING_BYTES`
  (`:640`; the 512-byte framing constant at `:751-754` covers the labels,
  separators, length prefixes, and epoch parts), `pending_rewrite` as an
  exact inventory of `PendingRewriteState` (`:653-658`), and
  `archive_fold_seq`, the newest archive segment's sequence above the fold
  (`crates/memory-store/src/lib.rs:2246-2250`), as an integer
  (`transform_meta_bound.rs:674`, added
  by `fd0b52aa5` in #859 PR A). The sum with key overhead (`object`,
  `:734`) is under 128 KiB, and the sum plus `SELECTED_IDENTITY_BUDGET_BYTES`
  is at most 384 KiB (`:734-743`).
- Composite witness (`e02b22383`):
  `composition_witness_a_meta_with_every_field_near_its_bound_commits_and_reloads_within_the_total`
  (`:978-1072`) commits `worst_case_module_meta` (`:822-975`) with its
  identity delta. The record sets free text to `\u0001` characters that
  escape to six bytes (`escaped`, `:756-758`), collections to capacity,
  integers to their extremes, the worst-case summarizer with a
  `near_budget_selection` within 256 bytes of 262,144 (`:767-793`), the
  worst-case synthetic pair (`:597-608`), and `last_recut` and
  `pending_rewrite_last_failure` to 1,008 bytes of secret-bearing text,
  near their 1,024-byte bound (`redacting`, `:760-765`). The test asserts the
  stored selection is within
  the budget, every other stored field within its recorded bound, the record
  within the recorded total plus the budget and under 512 KiB, the row length
  equal to the stored text, the two shaped fields reloaded redacted within
  their bounds, the rest of the reload equal to the committed record, and
  one identity row per selected mid. At `e02b22383` the commit reported
  371,060 stored bytes, 262,014 of them selection, against 115,457 recorded
  plus 262,144; since `78fc312db` the fixture's `last_render_config` holds
  five escaped inputs at their bound plus 512 framing bytes (`:831-835`).
  The record is a synthetic stress state: no pass produces every field at
  its bound at once.
- Summarizer inventory: `history_summarizer_bound` (`:273-483`) destructures
  `HistorySummarizerDurableState` and every nested struct through
  `inventory!` (`:242-249`), takes the longest serialized variant of each
  enum (`:251-262`), charges digests at 64 hex characters, details at
  `MAX_SUMMARIZER_DETAIL_BYTES`, producer identifiers at
  `MAX_PRODUCER_IDENTITY_BYTES`, reservation identifiers at
  `MAX_MEMORY_REVIEWER_RESERVATION_ID_BYTES`, and the selection at 0
  (`:458`), since the table charges it separately at 256 KiB. Since
  `0ff62b29a` it also charges `withdrawn_selected_mid`, the first selected
  mid a window dropped while the firing was in flight
  (`crates/memory-store/src/lib.rs:976`), at the escaped size of a mid at
  its 128-byte ingress bound (`transform_meta_bound.rs:476`), and
  `last_abandon` (`crates/memory-store/src/lib.rs:1043`) as an exact
  inventory of `HistorySummarizerAbandon`'s firing sequence, reason, and time
  (`transform_meta_bound.rs:477-481`); the worst-case state sets both at
  their extremes (`:578-583`).
  `every_history_summarizer_field_has_an_enforced_bound` (`:587-595`)
  serializes `worst_case_history_summarizer` (`:485-585`), which fills every
  free-text field with characters that escape to two bytes and every
  collection to capacity, with an empty selection (`:526`), and asserts it
  fits.
- Summarizer writers: `bounded_detail` caps a detail at
  `MAX_RAW_SUMMARIZER_DETAIL_BYTES`, 64 KiB of raw text
  (`crates/daemon/src/history_summarizer.rs`), and the store's shaping cuts
  the redacted form to `MAX_SUMMARIZER_DETAIL_BYTES`, 512 serialized bytes
  (both constants live in `crates/memory-store/src/lib.rs`; the daemon
  re-exports the second), applied by
  `abandon_with_detail` (`history_summarizer.rs:363-365`), `retain_backoff`
  (`:451-453`), `record_no_fire` (`crates/daemon/src/lib.rs:5902`, which
  compares the stored form at `:5903-5907`),
  `record_blocked_eligibility` (`:17552`), and
  `record_history_summarizer_connect_failure` (`:17611`). The chunk
  fingerprint and the retry record's `model_chain_digest` are SHA-256 hex
  (`history_summarizer.rs:166-179`;
  `crates/memory-store/src/lib.rs:548`). A harness over 128 serialized bytes
  is refused before any write and a longer run id is a start failure
  (`history_summarizer.rs:1834-1843`, `:1895-1901`); the session slug keeps
  48 bytes (`:1616`, `:1621-1624`). Reservation identifiers over 128
  serialized bytes are refused before any write
  (`crates/memory-store/src/lib.rs:668-690`, `:13808-13814`); no-fire
  details in the timeline keep a raw prefix within 128 serialized bytes
  (`crates/memory-store/src/summarizer_timeline.rs:13`, `:94-95`).
- Store-owned shaping (`186f3c076`): `shape_stored_meta`
  (`crates/memory-store/src/lib.rs:6094-6167`) runs in `commit_transform`
  (`:11034`) over `last_failure`, `last_no_fire`, every `NoFire` detail in
  the firing timeline and pending eligibility, `pending_rewrite_last_failure`,
  and `last_recut`. `shaped_meta_text` (`:6074-6090`) leaves a value alone
  when it is within its bound and stable under both redactors; otherwise it
  scans the value with `write.content`, so the field's receipt records any
  detection, and keeps `redacted_prefix_within_serialized_bytes` of the
  result. That function (`:6024-6032`) redacts with the durable and then the
  transaction redactor and, while a redactor would change the cut, moves it
  back to the start of that redactor's earliest finding (`earliest_finding`),
  so a state sync that re-prepares the record with the transaction redactor
  stores the same bytes and each scan removes a whole finding
  (`a_cut_that_exposes_a_finding_backs_off_in_a_bounded_number_of_scans`).
  `bounded_detail` hands the store up to 64 KiB, so a credential that
  crosses the 512-byte bound is scanned whole
  (`a_credential_that_crosses_the_detail_bound_is_redacted_whole`);
  `NoFire::new` and `pending_rewrite_detail`
  (`crates/daemon/src/transform.rs:6831-6848`) keep raw prefixes. `MAX_PENDING_REWRITE_DETAIL_BYTES`
  and `MAX_LAST_RECUT_BYTES`, 1,024 each, live in the store
  (`crates/memory-store/src/lib.rs:6039`, `:6041`); the descent, reset, and
  revert-truncation writers cut `last_recut` to the stable prefix
  (`:12706-12712`, `:13288-13299`, `:13505-13510`).
  `todo_state_within_bounds` checks both task-list bounds on the raw and
  the redacted form (`todo_state_bounds_hold_for_the_raw_and_the_redacted_form`); `set_todo_state` refuses a state that fails it
  (`:10309-10311`), and the bust capture records `[]` for one
  (`newest_bounded_todowrite_state_json`,
  `crates/daemon/src/injection.rs:214-222`).
  `synthetic_todo_pair_within_bounds` (`crates/memory-store/src/lib.rs:6147-6152`)
  measures the pair with `stored_json_len`, the longer of the durable and
  transaction scans (`:6155-6160`). `SyntheticTodo::admitted`
  (`crates/daemon/src/injection.rs`) measures it under the longest
  serialized anchor; `advance_synthetic_todo` freezes a pair only when it is
  admitted, and `injection_pending_after_capture` applies the same decision,
  so a refused pair leaves no injection pending
  (`a_state_whose_pair_is_refused_leaves_no_injection_pending`).
  Tests:
  `a_secret_bearing_summarizer_detail_is_stored_within_its_bound_with_its_detection_recorded`
  (`crates/memory-store/tests/production_redaction.rs:968`; stored detail
  redacted, within 512 bytes, stable under both redactors, and its
  receipt's finding count at least one),
  `a_committed_last_recut_is_stored_within_its_bound`
  (`crates/memory-store/src/lib.rs:24789`; a 200 KiB recut stored as its
  1,024-byte prefix),
  `a_revert_keeps_last_recut_within_its_bound_when_a_surviving_id_is_long`
  (`:24809`; a 4 KiB surviving id),
  `a_replacement_pair_over_its_bound_after_redaction_clears_the_persisted_pair`
  and `a_replacement_pair_near_its_bound_persists_and_reloads_within_it`
  (`crates/daemon/src/transform.rs:21623`, `:21655`; the caller-side pair
  through `advance_synthetic_todo` and a reload),
  `a_redacted_detail_cut_keeps_its_length_through_another_redaction`
  (`crates/memory-store/src/lib.rs:24756`),
  `an_abandon_keeps_a_secret_bearing_detail_within_its_bound_after_redaction`
  (`:24944`),
  `set_todo_state_refuses_a_state_whose_redacted_form_passes_its_bound`
  (`:24984`), `a_secret_bearing_start_failure_stays_within_its_bound_once_stored`
  (`crates/daemon/src/history_summarizer.rs:4275`),
  `a_captured_state_whose_redacted_form_passes_its_bound_reads_as_an_empty_list`
  (`crates/daemon/src/injection.rs:785`), and
  `a_synthetic_pair_over_its_bound_after_redaction_is_refused` (`:820`). The
  #859 PR C description records the negative control for each; this
  evidence does not rerun those controls.
- The store's abandon path: before `6267f66d4`,
  `abandon_history_summarizer_run_if_matching_with_publish_failure` stored
  the caller's redacted detail whole. Since `6267f66d4` (#859 PR C) it
  redacts the detail and keeps the prefix within
  `MAX_SUMMARIZER_DETAIL_BYTES` that both redactors leave unchanged
  (`crates/memory-store/src/lib.rs:13701`, `:13715-13722`), and stores that
  prefix (`:13768-13770`). Its daemon
  callers still pass uncut `publish rejected: {reason}` details for fence,
  caller-fence, overlap, and conflict refusals
  (`crates/daemon/src/history_summarizer.rs:737-743`, `:756-762`,
  `:773-779`, `:794-806`) and `memory_reviewer handoff failed: {error}`
  (`:2589`, `:2598-2605`); the store-side cut bounds them all. A fence
  reason names the selected mid (`crates/memory-store/src/lib.rs:14165-14169`),
  and a mid may be 128 bytes of control characters
  (`crates/daemon/src/wire.rs:215`, `:285-290`), which serialize to 768
  bytes. `an_abandon_keeps_a_failure_detail_within_its_serialized_bound`
  (`crates/memory-store/src/lib.rs:25787`) abandons with
  `publish rejected: ` and 400 control characters (2,418 serialized bytes,
  `:25629`) and asserts the stored detail is a prefix of it, at most 512
  serialized bytes and more than 506 (`:25648-25650`).
- State sync: caps on anchors, watermarks, todo state, directives, marker
  ids, and the synthetic todo pair (`crates/memory-store/src/lib.rs:5976-5980`,
  `:6141-6143`, `:6164`; `within_bound` at `:6166`), now with a todo
  serialized-length cap (`:5980`, `:6333-6340`; `set_todo_state` at
  `:10309-10311`). After content
  preparation, `check_prepared_state_sync_bounds` re-checks each capped value
  as stored, since a redaction placeholder can be longer than its secret
  (`:6398`, `:6402-6448`). Since `186f3c076` the seeded pair and the
  acknowledged watermarks are checked with `stored_json_len`
  (`:6295-6299`, `:6379-6383`);
  `state_sync_refuses_values_whose_stored_form_passes_their_bound`
  (`:24849`) refuses each at exactly its raw bound with a stored form past
  it and asserts the refused sync writes nothing. A sync whose result would
  hold more than 512 legacy segments is refused before any write
  (`:11505-11534`).
- Covered systems: `ModuleMeta` has no covered list. The rows live in
  `covered_system_messages` (`crates/memory-store/baseline.sql:40-58`). The
  transform reads them with its snapshot
  (`crates/daemon/src/transform.rs:3371-3372`; store
  `crates/memory-store/src/lib.rs:8773`), records them
  through `record_covered_systems` (`transform.rs:6351-6366`), and commits a
  `CoveredSystemMessageDelta` inside the CAS (`:5189-5194`, `:5244`; store
  `crates/memory-store/src/lib.rs:11120-11127`). Reset removes the rows
  (`:13329`); a descent target starts with none (`:12962-12964`).
- Other caps: request identity strings over 256 bytes
  (`crates/daemon/src/transform.rs:149`, `:2236-2241`); 16 pending hint ids
  (`:148`, `:4218-4222`); mids over 128 bytes (`crates/daemon/src/wire.rs:215`,
  `:285-290`).
- Matrix: `module_meta_size_is_independent_of_message_count_and_window_size`
  (`crates/daemon/src/transform_meta_bound.rs:1273`) builds each cell with
  `matrix_meta` (`:1163-1270`). An active-firing cell seeds summarizer state
  and commits a selection loaded from the first 100 window mids' identity
  rows (`:1195-1221`); the final assertion requires the Eidnara cell to retain
  100 selected identities and the native cell zero, because native passes
  write no identity rows (`:1239-1251`). It asserts meta under 512 KiB
  (`:1265-1268`). `equal_apart_from_digit_width` (`:1144-1161`) compares
  digit-normalized JSON within 1%. Negative controls: restored window-sized
  `pending_user_hint_block_ids` (`:1305-1312`), digit-only and zero-array growth
  (`:1313-1326`), and a string that grows only in escaping cost (`:1328-1336`).

## Failure scenario

A field grows with N or W, or a writer stores more than its bound; a long
session crosses 512 KiB and every later pass fails its commit.

## Timing windows and dependencies

None; growth accumulates across passes. The worst case composes a full
256 KiB selection with every field at its cap.

## What a test must construct

The matrix under both authorities and firing states with negative controls;
each cap at its bound and one past it; many folds with distinct system
messages; a composite record with every field at its bound and a full
selection, committed; a store-side abandon with a detail over the bound
(constructed by #859 PR C).

## Investigation log

### Q: Are `covered_system_messages` and `legacy_history_segment_seqs` acceptable without a byte cap?

- Sources examined: the bound table at `d7b330113` and `0ff62b29a`;
  `git show b45416ac0`.
- Findings: At `d7b330113` both were `Unbounded`. The owner decided: #859
  PR B moves covered system messages into rows outside `meta` and caps
  legacy segments at 512 per session;
  `covered_systems_grow_in_m0_while_the_stored_meta_stays_fixed`
  (`crates/daemon/src/transform.rs:26370`) shows 40 distinct covered systems
  rendered while the stored meta changes by digit widths alone, and
  `state_sync_refuses_a_result_over_the_legacy_segment_cap`
  (`crates/memory-store/src/lib.rs:25037`) refuses the 513th legacy row.
- Missing evidence: None.
- Conclusion: resolved with answer: the owner decision is implemented.

### Q: Is the `history_summarizer` bound enforced?

- Sources examined: `history_summarizer_bound`, the summarizer writers, and
  the tests named in `git show bdf564e3a --stat`.
- Findings: Yes. #859 PR C replaced the declared 12 KiB allowance with the
  inventory, and every writer cuts or refuses its free-text fields. Before
  `6267f66d4` the store's abandon path stored an uncut `last_failure`;
  `6267f66d4` cuts it in the store, and
  `an_abandon_keeps_a_failure_detail_within_its_serialized_bound` witnesses
  the cut. The PR reports a negative control that fails with the cut
  removed; the test's at-most-512 assertion is the one that discriminates.
- Missing evidence: None.
- Conclusion: resolved with answer: enforced on every writer since
  `6267f66d4`.

### Q: Do the bounds hold after redaction for details, the task-list setter, and the synthetic pair?

- Sources examined: `git show 6fc85742f`; `git show 186f3c076`; the
  store-owned shaping bullet above.
- Findings: A design review found that each of the three checked the raw
  value, so a secret whose placeholder is longer than the secret could carry
  a value that met its bound past it once stored. `6fc85742f` checked the
  redacted form in each place, but only under the durable redactor, and its
  daemon-side cut handed the store already-redacted text, so the store's
  scan recorded no detection. `186f3c076` moves final shaping into the
  store, keeps prefixes stable under both redactors, bounds `last_recut`,
  and checks the seeded pair and acknowledged watermarks in their stored
  form; each case has a secret-bearing test at its bound.
- Missing evidence: None.
- Conclusion: resolved with answer: closed by `6fc85742f` and `186f3c076`
  (#859 PR C).

### Q: Does the recorded `last_render_config` bound cover the stored render identity?

- Sources examined: `git show 78fc312db`;
  `crates/daemon/src/transform_meta_bound.rs:640`, `:751-754`, `:1342-1368`.
- Findings: A design review found that the bound charged four request
  inputs plus 256 bytes, while the stored identity carries all five request
  inputs (render config, provider, model, system prompt hash, upgrade
  state) and its framing. `78fc312db` (#859 PR C) charges five inputs plus
  `RENDER_IDENTITY_FRAMING_BYTES` = 512, updates the composite fixture, and
  adds `a_render_identity_from_five_escaped_inputs_at_their_bound_fits_its_allowance`
  (`:1342`): a real pass with each input at 256 `\u0001` characters stores
  an identity with all 1,280 of them, at most 512 framing bytes, and a
  serialized length within the recorded bound. The #859 PR C
  description records its negative control.
- Missing evidence: None.
- Conclusion: resolved with answer: found by the review and closed by
  `78fc312db`.

### Q: Does a test commit the worst-case record?

- Sources examined: `git show e02b22383 -- crates/daemon/src/transform_meta_bound.rs`;
  the composite witness bullet above.
- Findings: Yes since `e02b22383`. The composite witness commits one record
  with every field near its recorded bound and a selection within 256
  bytes of the budget, then checks every stored field, the total, and the
  reload. At `6fc85742f` the worst-case summarizer was serialized only and
  the 384 KiB composite was arithmetic. The composite and the matrix's
  native active-firing cell are synthetic stress states.
- Missing evidence: None.
- Conclusion: resolved with answer: `Exercised: yes` since `e02b22383`.

### Q: What happens to a covered list stored in `meta` before #859 PR B?

- Sources examined: `ModuleMeta` (`crates/memory-store/src/lib.rs:2121`);
  `git show b45416ac0 -- crates/memory-store`.
- Findings: The field is gone, and no load or migration copies an existing
  list into rows, so the first load drops it.
- Missing evidence: An owner decision for existing sessions.
- Conclusion: needs human input.

### Q: Does the inventory cover the fields that `main` and `0ff62b29a` added?

- Sources examined: `git show fd0b52aa5 0ff62b29a`;
  `crates/daemon/src/transform_meta_bound.rs:476-481`, `:674`;
  `crates/memory-store/src/lib.rs:976`, `:1043`, `:2246-2250`.
- Findings: Yes. `archive_fold_seq` (from the archive commits on `main`) is
  charged as an integer by `fd0b52aa5` in #859 PR A, and the summarizer's
  `withdrawn_selected_mid` and `last_abandon` (from #905 and the archive
  commits) are charged by `0ff62b29a` in #859 PR C. The exhaustive
  destructuring makes a field that the table does not list a compile error,
  and PR C's run at `0ff62b29a` passed with the composite witness.
- Missing evidence: None.
- Conclusion: resolved with answer: evidence updated; the record stays
  `Exercised: yes`.
