# fa-n12-identity-observations-survive-window-loss

## Discovery trigger

Proposed obligation FA-N12 of spec #834 (D4, identity rows), owned by ticket
#857 and landed by PR #905 (head `5d9ff4581`, merged as `21681e0ea`). Before
#905 a load hydrated every stored identity into `ModuleMeta` and a commit made
the `block_identities` table equal a window-pruned map (#824 D12), which
deleted identities that publication still needed.

Exercised status: yes - the store and daemon identity family ran green in
#905's recorded workspace gate and again in #906's and #859 PR A's
runs.

## Evidence trail

Code references are verified at `0ff62b29a`.

- One lookup: `lookup_block_identity_rows`
  (`crates/memory-store/src/lib.rs:4885-4902`) selects the requested mids with
  `mid IN (SELECT value FROM json_each(?2))`. It backs the snapshot read
  (`:8859`, inside `load_transform_snapshot_with_hook` `:8814`), the public
  `load_block_identities` (`:8638-8646`), and the publication fence
  (`:14190`).
- Delta: `BlockIdentityDelta { upserts, deletes }` (`:1911-1914`).
  `apply_block_identity_delta` (`:4926-5001`) refuses a delta that writes and
  deletes one mid (`:4935-4945`), skips an unchanged vector (`:4964-4967`),
  deletes only the listed mids (`:4973-4978`), and upserts the rest.
- CAS coupling: `commit_transform` (`:10918`) runs the row-version CAS
  (`:11146-11155`), the new-row identity check (`:11156-11164`), and the
  history-segment sequence check (`:11165-11176`); the CAS and the sequence
  check return `Replay`. It then writes `cache_state` (`:11191-11200`) and
  applies the delta (`:11204-11206`) in the same transaction. No `Replay`
  return follows the delta in this closure (it ends `Applied` at `:11440`).
  Meta and core pass the durable-text scans before `write.execute`
  (`:11067-11076`, `:11102`).
- Transform: the pass requests the projection's mids
  (`crates/daemon/src/transform.rs:3373-3379`), holds `WindowIdentities`
  (`:5404-5439`), and sends its delta with the commit (`:5252`). A pass whose
  only change is the delta still commits (`:5201-5204`). The only
  `deletes.insert` is `WindowIdentities::remove` (`:5436`), which runs only
  for the provisional tail (`:5620-5622`).
- Lifetime: reset deletes every row (`delete_block_identities`
  `crates/memory-store/src/lib.rs:5003-5005`, which delegates to
  `ScanOwnedRows::delete_session_rows` `:4843-4851`, called in `reset_session`
  at `:13400`); `delete_session` deletes from every table with a `session_id`
  column (`:8524-8598`); descent copies the source's rows with NULL
  `scan_version` (`:13033-13042`).
- Schema: `block_identities` and the partial index
  `block_identities_by_scan_version` (`crates/memory-store/baseline.sql:27-38`).

Gate results as recorded in the PR descriptions:

- #905 (#857; gate run at `871ebfb08`, base `57a820bb6`): `cargo +1.98 fmt --all
  -- --check`, clippy `-D warnings`, rustdoc `-D warnings`, `cargo +1.98
  test --workspace --all-features --locked --no-fail-fast`, and the marker
  script pass. The run log records 6,064 passed, 0 failed, 66 ignored, with
  each named test `ok`. The ignored driver `record_identity_transaction_timings`
  (`crates/memory-store/src/lib.rs:33789`) records a 300-mid delta commit at
  4.00 ms and 4.27 ms and a 300-mid snapshot read at 0.43 ms and 0.49 ms at
  100k and 1M rows (SQLite 3.51.3, WAL, `synchronous=2`); evidence, not a
  gate.
- #906 (gate run at `1d2cd55a0`) and the #859 PR A (`fd0b52aa5`): the same tests
  are `ok` (6,079 and 6,124 passed, 0 failed).

## Failure scenario

A session's window shrinks after a fold, a pass commits the window-pruned
map, and the rows for a selected message outside the window disappear. The
firing's publication then refuses a legal chunk, or a later edit to that
message is never compared because the transform has no stored identity.

## Timing windows and dependencies

- A commit between the snapshot's state read and its identity read; the test
  hook runs between the two (`:8575`).
- CAS loss or a durable-text refusal after the transform built its delta.
- An aborting trigger after the delete, between two upserts, or after the
  upserts; the whole commit rolls back.

## What a test must construct

A session with many identity rows and a small window; a pass that omits most
mids; a CAS-losing commit; an oversized meta; an injected failure after each
mutation kind; a descent and a reset; a publication whose selected message is
outside the window; comparison with a per-session reference map across
reopen.

## Investigation log

### Q: Can any path delete an omitted mid outside reset, deletion, or native adoption?

- Sources examined: `transform.rs:5404-5439`, `:5612-5661`;
  `crates/memory-store/src/lib.rs:4926-5001`, `:5003-5005`.
- Findings: No. Ordinary folding-window updates retain omitted identity
  rows: the transform's only per-mid delete is the provisional tail, and
  the store deletes a single mid only from `delta.deletes`. Native adoption
  clears legacy identity rows in the adopting transaction (`clear_identities`,
  `crates/daemon/src/transform.rs:3136-3140`;
  `crates/memory-store/src/lib.rs:11201-11203`); reset, session deletion,
  and descent-target replacement clear whole sessions.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: Are #905's deferred test-review follow-ups tracked?

- Sources examined: #905 description, "Test review follow-ups not taken
  here".
- Findings: Four items are listed (vary core and metadata in the rollback
  test; compare `(mid, scan_version)` pairs across descent; exact-vector
  oracles in the meta-bound and revision 3 tests; an exact multi-message
  selected-vector assertion for FA-E04). No tracker item is named.
- Missing evidence: A tracker reference.
- Conclusion: needs human input.

### Q: Does #905's final form still keep omitted mids?

- Sources examined: `git show f0e39d04d 64a8bf371 5d9ff4581`;
  `crates/daemon/src/transform.rs:5441-5479`, `:20472`;
  `crates/memory-store/src/lib.rs:14179-14183`.
- Findings: Yes. The window still deletes no omitted row. When a window
  drops a selected mid of an in-flight firing, the transform records the
  withdrawal on the firing and the publication is fenced out, while the
  identity rows stay as they were. The witness was renamed from
  `a_window_that_omits_the_selected_message_keeps_its_identity_for_the_publication`
  to
  `a_window_that_drops_a_selected_message_fences_the_publication_out_and_keeps_its_rows`
  and now asserts the fence refusal beside the unchanged rows.
- Missing evidence: None.
- Conclusion: resolved with answer: evidence updated; the record stays
  `active`.
