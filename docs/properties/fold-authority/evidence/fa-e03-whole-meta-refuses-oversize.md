# fa-e03-whole-meta-refuses-oversize

## Discovery trigger

Existing-behavior record FA-E03 of specification #834 (first comment),
describing `265df096`: transform metadata is one durable text field bounded
at 512 KiB, and an oversized first commit is refused with the row left
absent. Ticket #859 keeps it as "the recoverable guard" that FA-N14's matrix
shows is not reached. The same guard is catalogued for the window protocol
as
[WP-E10](../../window-protocol/catalog.md#wp-e10-durable-meta-bound-refuses-the-cliff),
whose evidence records that the refusal half lost its witness; this record
cross-references it and adds the fold-authority M2 evidence.

Exercised status: partial - a store-level refusal over an existing row is
witnessed (#905, green again in #859 PR C's run at `0ff62b29a`); the matrix
asserts the guard is not reached (#859 PR A); no witness refuses a first
transform commit with the row absent, and none checks covered-system rows
after a refusal.

## Evidence trail

References are verified at `0ff62b29a` unless another tree is named.

- Guard: `MAX_DURABLE_TEXT_BYTES = 512 * 1024`
  (`crates/memory-store/src/lib.rs:429`); `ensure_durable_text_bound`
  returns `Redaction(InputLimit)` above it (`:4599-4605`).
- Chain on the transform commit: `shape_stored_meta`, then
  `serde_json::to_string(meta)`, then `write.json_content("meta", ..)`
  (`:11069-11076`) ->
  `prepare_json_content_collecting` (`:3050-3060`, `:4304-4314`) ->
  `prepare_json_content_single_pass`, whose first statement is the guard
  (`:4303`). This runs while the commit is prepared, before `write.execute`
  opens the transaction (`:11102`); the row write runs inside it
  (`:11191-11200`).
- Refusal witness: `a_refused_commit_writes_no_identity_row` (`:33881`),
  added by #905 (`871ebfb08`), commits a `meta` whose `last_render_config`
  is one byte over the bound on top of an existing row with an identity
  delta, and asserts `InputLimit`, unchanged identity rows, and an unchanged
  row version. It ran in #905's workspace gate and in #859 PR A's run.
- Related: `a_prepared_content_field_that_grows_past_the_durable_bound_on_redaction_is_refused`
  (`:32203`) pins the exact boundary for prepared content (input at exactly
  the bound, output over it after redaction);
  `state_sync_metadata_scan_failure_rolls_back_earlier_writes` (`:32030`)
  shows a stored oversized `meta` refused inside a state sync with earlier
  writes rolled back.
- Not reached: `matrix_meta` asserts every matrix cell's stored `meta` is
  under 512 KiB (`crates/daemon/src/transform_meta_bound.rs:1265-1268`), in
  `module_meta_size_is_independent_of_message_count_and_window_size`
  (`:1273`), green in #859 PR A's run.
- Covered-system rows (#859 PR B, `b45416ac0`): the commit applies a
  `CoveredSystemMessageDelta` inside the transaction, after the
  `cache_state` write (`crates/memory-store/src/lib.rs:11207-11214`).
  `apply_covered_system_message_delta` (`:5047-5149`) groups the written
  rows toward the 256 KiB `SCAN_DOCUMENT_CHUNK_BYTES` batching threshold
  (`:4735`; `scan_documents`, `:4739-4759`): a row that would carry a
  non-empty document past the threshold starts a new one, and a single row
  larger than the threshold stays whole in its own document. Each completed
  document passes through `json_content` (`prepare_document`, `:4854-4878`)
  and so meets the 512 KiB durable-text guard. A refusal there rolls the
  transaction back, and the durable rows stay as they were.
  `covered_system_rows_round_trip_in_ordinal_order_and_retire_their_receipts`
  (`:34298`) shows a lost CAS and a write-and-delete refusal leave the rows
  unchanged.
  `covered_system_rows_past_one_scan_document_split_and_retire_every_receipt`
  (`:34403`, #859 PR C, `e02b22383`) writes four individually sub-threshold
  rows whose combined serialized length exceeds 256 KiB and stays below
  512 KiB; the one
  write holds more than one document receipt, and the receipts stay until
  the last row of the write is removed. No test drives a durable-text
  refusal with a covered delta.
- #906 (#858) changed `a_turn_the_host_refuses_is_returned_with_its_code`
  because ordinary growth no longer crosses the bound at about 2,000
  messages; it now uses a secret-bearing session id.
- Corrections to the spec's citations at `265df096`: S`:427` (constant) is
  `:429`; S`:3998-4004` (guard) is `:4599-4605`; the growing fields
  S`:1668` (`baseline_parts`), `:1876` (`block_identity_by_mid`), and `:2035`
  (`served_output_fingerprint`), read at `265df096`, are removed by #905 and
  #906; S`:1929` (`tail_hygiene_baseline`) remains as scalars only. The
  existing check D`transform_meta_bound.rs:20-99`
  (`first_hard_pass_meta_respects_the_store_durable_text_bound`) was deleted
  by #833 (`7a8fb84b`), as WP-E10 records. The spec's guard chain
  (`:9221-9227` -> `:2520-2529` -> `:3703-3709` -> `:3719-3724`) was not
  re-read at `265df096`; the HEAD chain above replaces it.

## Failure scenario

A record past 512 KiB is refused on every pass, so the session serves raw
history. Without the guard an oversized record would be stored and a later
re-scan would refuse its own row.

## Timing windows and dependencies

Meta validation runs during preparation, before transaction execution.
Covered-system document validation runs inside the transaction after the
`cache_state` write. A refusal rolls back the transaction and preserves
durable rows.

## What a test must construct

A fresh store and a first transform commit whose metadata is one byte over
the bound: assert `InputLimit`, no `cache_state` row, no identity rows, and
no covered-system rows; the same at exactly the bound commits. A later
commit refused by the guard, through `meta` or through an oversized
covered-system document, leaves the covered rows as they were.

## Investigation log

### Q: Does a witness refuse a first transform commit and leave the row absent?

- Sources examined: memory-store tests naming `MAX_DURABLE_TEXT_BYTES`;
  `transform_meta_bound.rs`; WP-E10's evidence.
- Findings: #905's test refuses a later commit through the test commit API,
  not a transform, and the row already exists.
- Missing evidence: A transform-level first-commit refusal.
- Conclusion: unresolved, needs that test; shared with WP-E10.

### Q: Is this record preserved or replaced by M2?

- Sources examined: #834 Verification Strategy; ticket #859 obligations.
- Findings: #859 states "FA-E03 ... is preserved as the recoverable guard".
  The guard code is unchanged at `0ff62b29a`.
- Missing evidence: None.
- Conclusion: resolved with answer: preserved, `Status: active`.

### Q: Does a refused commit leave the covered-system rows unchanged?

- Sources examined: `git show b45416ac0 -- crates/memory-store`; the
  covered-system store tests (`crates/memory-store/src/lib.rs:34298`,
  `:34473`, `:34509`, `:34539`).
- Findings: The rows are written inside the commit transaction after the
  `meta` guard, and each row document passes the same guard, so a refusal
  writes no row. The tests cover a lost CAS and a write-and-delete refusal,
  not a durable-text refusal.
- Missing evidence: A refusal by the `meta` guard and by an oversized
  covered-system document, each comparing the rows before and after.
- Conclusion: unresolved, needs that test.
