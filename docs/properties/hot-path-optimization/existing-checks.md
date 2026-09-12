# Existing checks for optimization preservation

The system is `/local/home/ahrav/scratch/eidnara`, at
`913234433ae36a80a6e22c6aac14c7f9aab74386`, checked on 2026-09-10.
The [catalog scope](catalog.md#scope-and-provenance) governs this inventory.
The supplied audit motivates inspection of these checks; external plans and
incidents are not supplied, and final scope confirmation is pending.

This is a focused inventory of checks bearing on the new deltas, not a
replacement for the existing catalogs. Every listed check is **unaudited**.
Descriptions identify what the source checks, not whether the oracle is
sufficient. No historical exercise claim is imported and no check runs here.
This is a working-tree inventory against the source baseline. It includes the
known relevant guards and claim-bearing checks on these preservation surfaces;
it does not claim exhaustive repository coverage. Explicit exclusions follow
the tables so an omitted category is not mistaken for absent tests.

## Canonical read

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [Read caps and completeness guards][read-guards] | Selection precedes the 8192-row heap; the payload prefix is bounded and missing typed decision rows fail closed. | unaudited |
| [Served-column guard][served-columns] | Positional reads are checked against expected result aliases. | unaudited |
| [Recorded cross-project read][cross-project] | The recorded foreign-project read is empty. | unaudited |
| [Project scope serving][project-scope] | Rows serve only to the project named by their scope. | unaudited |
| [Historical snapshot read][historical-read] | Reads return rows visible at the requested snapshot. | unaudited |
| [Object-filtered read][object-filter] | The filtered read serves the named visible objects. | unaudited |
| [Injectable categories][injectable-test] | Visible positive-category decisions survive conversion. | unaudited |
| [Revision inputs][revision-test] | Rendered inputs determine the revision comparison. | unaudited |
| [Dropped categories][dropped-test] | Renderer-excluded rows do not move the revision. | unaudited |
| [Render-cap suffix][suffix-test] | Bytes beyond the render cap do not move the revision. | unaudited |
| [Memory budget][memory-budget-test] | Budget-excluded rows are neither injected nor digested. | unaudited |
| [Withheld read][withheld-test] | The verdict is recorded with no injected rows. | unaudited |
| [Production transform composition][transform-memory-test] | The integration path observes canonical memory and its removal. | unaudited |
| [Lag and acknowledgement][lag-test] | Consumer lag withholds the block; acknowledgement restores serving. | unaudited |
| [Disabled memory][disabled-test] | A disabled pass takes no canonical read. | unaudited |
| [Configured memory budget][configured-budget-test] | The reader drops rows beyond the configured budget. | unaudited |
| [Generic row cap][row-cap-test] | Newest rows survive and truncation is reported. | unaudited |
| [bounded_selection_matches_a_full_sort_and_truncate][selection-reference] | Generated keys and caps compare selected rows and dropped status against an independent full sort/truncate. | unaudited |
| [Generic byte cap][byte-cap-test] | The result is nonempty, truncated, shorter than the input, and a contiguous newest prefix; exact byte usage and maximal fit are not asserted. | unaudited |
| [Render category boundary][render-category] | Nonpositive and unknown categories are excluded at rendering. | unaudited |
| [Memory markup escaping][render-markup] | Object IDs cannot forge block structure through markup or newlines. | unaudited |
| [Memory content cap][render-content-cap] | UTF-8 content is cut before the 64 KiB boundary. | unaudited |
| [Positive vocabulary][render-vocabulary] | Positive categories match the frozen TypeScript arrays. | unaudited |
| [Category render order][render-order] | Render order is a prefix of the positive vocabulary. | unaudited |

No whole-pipeline baseline-versus-pushdown oracle or K3/K4 marker is found
in the inspected selection and canonical-memory tests. Generic cap coverage
does not establish canonical other-domain pressure. Corrupt excluded-row
decoding is a quiet compatibility boundary, not a proven safe omission.

## Execution lifecycle

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [Admission permits][permits] | Pending and handler-task capacity are acquired separately before dispatch. | unaudited |
| [Handler completion fence][handler-fence] | Both the outer task and callback are tracked. | unaudited |
| [Close gate][close-gate] | Unstopped dispatch after the post-abort wait trips fatal state and refuses cleanup. | unaudited |
| [Retained reservations][reservations] | Staging and decode ownership release their respective charges. | unaudited |
| [Parse admission][parse-admission] | Parse residency is reserved before Value decoding; oversized and temporarily exhausted reservations have different refusals. | unaudited |
| [ByteCharge ownership][byte-charge] | Owned permits hold bytes through transfer and release them on drop. | unaudited |
| [Decode admission][decode-admission] | Both decoding bytes and job count must fit before admission. | unaudited |
| [Saturated requests][saturation-test] | `server_busy` occurs without handler dispatch. | unaudited |
| [Cancel/completion arbitration][cancel-test] | Cancellation and completion emit at most one observed terminal. | unaudited |
| [Simultaneous arbitration][simultaneous-test] | A simultaneous cancel/completion race still has one terminal. | unaudited |
| [closing_a_route_settles_its_admitted_work][route-overlap] | A hanging callback starts before Goodbye; the test observes cancelled settlement and exactly one route-gone. | unaudited |
| [Stream cancellation][stream-cancel] | A cancelled stream stops with one terminal. | unaudited |
| [Handler panic][handler-panic] | A callback panic maps to one redacted internal-error terminal. | unaudited |
| [`cancel_waits_for_the_request_blocking_work`][t-cancel-work] | Cancel during held blocking work settles nothing and releases no charge until the work is released, then one `cancelled` terminal and one release. | unaudited |
| [`route_close_waits_for_the_request_blocking_work`][t-close-work] | Goodbye during held blocking work runs no route-gone until the work is released, then the `cancelled` terminal, exactly one route-gone, and one release. | unaudited |
| [`route_close_waits_for_blocking_work_the_handler_did_not_await`][t-detached-work] | A handler answers without awaiting its work; route-gone still waits for the work, and the charge releases once. | unaudited |
| [`blocking_work_held_past_the_route_close_budget_is_fatal_not_cleaned_up`][t-fatal-work] | Work held past a shortened route-close budget produces no route-gone and a lifecycle-fatal shutdown. | unaudited |
| [`blocking_work_released_after_the_dispatch_abort_still_settles_cancelled`][t-late-work] | Work released after the close aborted the dispatch task, inside the post-abort budget, still yields the `cancelled` terminal, one route-gone, one release, and a graceful shutdown. | unaudited |
| [`a_blocking_work_panic_settles_as_one_internal_error`][t-panic-work] | A panic inside `run_blocking` settles as one `internal_error` terminal and releases the held charge once. | unaudited |
| [`blocking_work_panic_payload_is_redacted_from_process_stderr`][t-stderr-work] | A child process panicking inside `run_blocking` writes the fixed diagnostic and not the payload to stderr. | unaudited |
| [Output reservation][output-reservation] | Concurrent output is reserved before allocation. | unaudited |
| [Egress exhaustion][egress-exhaustion] | A blocked reservation deadline retires the generation. | unaudited |
| [Reserved-class isolation][reserved-isolation] | Saturated reserved work cannot consume a general slot. | unaudited |
| [General-class isolation][general-isolation] | Saturated general work cannot consume the reserve. | unaudited |
| [Request byte cap][request-cap] | Only transform-class requests receive the widened frame cap. | unaudited |
| [Parse node counting][parse-nodes] | String punctuation and escaped quotes do not become structural nodes. | unaudited |
| [Retained string copies][parse-copies] | The footprint covers every retained string copy. | unaudited |
| [Dense parse footprint][parse-dense] | Scalar-dense input is charged above its wire size; string content is counted separately. | unaudited |
| [Upload caps][upload-cap-test] | Reservation admission and release use the declared caps. | unaudited |
| [Finished upload reservation][upload-finish-test] | Finished page collection retains its reservation until release. | unaudited |
| [Reservation transfer][upload-keep-test] | A kept reservation releases neither bytes nor its pending slot. | unaudited |
| [Retry restoration][upload-restore] | Restored pages survive and yield to a newer begin. | unaudited |
| [Begin replacement][upload-begin] | Matching begin resumes; a different upload replaces ownership. | unaudited |
| [Refused replacement][upload-replace] | A refused replacement still frees the old route allocation. | unaudited |
| [Staging overload classification][upload-full] | The staging budget determines queue-full outcomes. | unaudited |
| [Stale upload eviction][upload-stale] | A later begin evicts an idle upload. | unaudited |
| [Upload activity][upload-active] | Activity defers stale eviction. | unaudited |
| [Decode counts and bytes][decode-test] | Decode admission is bounded by bytes and jobs and releases both exactly. | unaudited |
| [Cleared finish restoration][upload-cleared] | A cleared coordinator refuses restoration of an old finish. | unaudited |
| [Early discard][upload-discard] | Discard releases the declared total even before a page arrives. | unaudited |

The blocking-work tests cover the host seam with a test handler; no relocated
transform runs through it at the cited transform site. Pending-table size or
health counters alone are not physical completion witnesses. A transform may
have durable effects despite an unknown transport outcome, so terminal counts
are not durable-effect counts.

## Guarded store

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [Callback scope installation][scope-install] | Preexisting shadows are refused and the authorizer is installed. | unaudited |
| [Infrastructure comparison and restoration][scope-restore] | Infrastructure changes fail before commit; scope state is restored on release or unwind. | unaudited |
| [Facade scope placement][facade-scope] | The current thread's caller/domain/route scopes are installed inside the connection-locked callback. | unaudited |
| [Same-thread reentry guard][reentry-guard] | Reentry returns a backend error rather than relocking the connection. | unaudited |
| [Fenced callback boundary][fenced-guard] | The precheck, durability pin, transaction claim, scope release, and commit form the write boundary. | unaudited |
| [Temp shadow creation][shadow-create-test] | A callback cannot create a temp shadow of a baseline table. | unaudited |
| [Preexisting shadow][shadow-test] | Maintenance-created shadows refuse callbacks until removed. | unaudited |
| [Overridden lower function][lower-test] | An overridden SQLite function does not blind shadow detection. | unaudited |
| [Fenced schema changes][schema-test] | Fenced callbacks cannot leave a schema that breaks reopening. | unaudited |
| [Read unwind][unwind-test] | A panic does not strand the connection read-only. | unaudited |
| [Read durability][durability-test] | A read callback cannot lower fence durability. | unaudited |
| [Read guard escape][read-escape-test] | A callback cannot clear the read-only guard. | unaudited |
| [Transaction escape][tx-escape-test] | A callback cannot end the fenced transaction. | unaudited |
| [Fence row protection][fence-row-test] | A callback cannot damage its authority row. | unaudited |
| [Format marker][format-test] | A fenced callback cannot rewrite the format marker. | unaudited |
| [Reentry][reentry-test] | Same-thread reentry returns an error instead of relocking. | unaudited |
| [Cached statements][cached-test] | Cached writes and pragma writes remain denied in a read scope. | unaudited |
| [Snapshot and next-call freshness][snapshot-test] | Two reads see the original value; a later callback sees the intervening commit. | unaudited |
| [Read transaction control][read-tx-test] | A read callback cannot end its snapshot. | unaudited |
| [Attach/detach][attach-test] | Guarded callbacks cannot escape through other databases. | unaudited |
| [Pragma classification][pragma-test] | Introspection is allowed while setting pragmas is denied. | unaudited |
| [Write rollback][rollback-test] | Callback error rolls back the fenced write. | unaudited |
| [Read checkpoint refusal][checkpoint-test] | A read callback cannot checkpoint WAL. | unaudited |
| [Fenced persistence][persist-test] | A fenced write commits and persists. | unaudited |
| [Read-path writes][read-write-test] | Writes through the read connection are refused. | unaudited |
| [Maintenance path][maintenance-test] | Maintenance executes through the unfenced path. | unaudited |
| [Maintenance UDF][udf-test] | A registered scalar function serves reads, writes, and triggers. | unaudited |
| [Stale-writer precheck][stale-precheck] | A superseded writer cannot change journal mode before refusal. | unaudited |
| [Negative fence][negative-fence] | A negative database fence fails closed. | unaudited |
| [Writer handover][handover-test] | A superseded writer is fenced after handover. | unaudited |
| [Equal epoch][equal-epoch] | An equal-epoch writer is not refused as stale. | unaudited |

No complete baseline-versus-batched observation trace is found in these checks.
The statement-cache test does not itself prove arbitrary setup elision safe.
Maintenance, cached authorization, and facade ownership must be tested together
when their boundaries change; a missing facade scope is legal on nonfacade calls.

Checks added with the mode-gated authorizer (implementation base
`96709d0ef54bcfad2327878ab96e118fb8ba4969`; links are to the live tree):

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [Statement reuse probe][reuse-probe] | A warm fenced statement reports zero re-prepares across two callbacks; a foreign `CREATE TABLE` forces a re-prepare, the callback reads the new table, and a temp-shadow statement cached before the DDL is refused. | unaudited |
| [Read-path expiry witness][read-witness] | The `query_only` toggle expires cached statements on each read callback; two fenced callbacks in a row re-prepare nothing. | unaudited |
| [Temp-database write barrier][temp-write-test] | Temp DDL and temp DML are refused in a read callback and allowed in a fenced one. | unaudited |
| [Mode restoration after panic][mode-restore-test] | Maintenance regains pragma writes after a panicking read and a panicking fenced callback; `query_only` is restored, the partial write is rolled back, and the next fenced write re-pins `synchronous=FULL`. | unaudited |
| [Baseline escapes on the store connection][baseline-gate-test] | A pragma write, `ATTACH`, `BEGIN`, `SAVEPOINT`, fence-row insert, or format-marker delete in baseline text is refused by the store connection's gate; the pristine file then opens with benign text. | unaudited |
| [Store statements stay uncached][surface-guard-test] | No fence or durability-pin statement is found in the statement cache after an open. | unaudited |
| [Unrestricted statements do not reach guarded callbacks][gate-tests] | A fence upsert prepared unrestricted is reused without re-authorization until the cache is flushed, then refused; every `deny_baseline_escapes` denial is reachable; nested mode entry is a debug assertion. | unaudited |
| [Schema snapshot keyed on the schema and data versions][snapshot-key-test] | An unchanged key reuses the snapshot; a rename replaces it; maintenance discards it at an unchanged key; a foreign commit that writes the old schema version back still moves the key; defensive mode neutralizes `schema_version` and `writable_schema` writes; an oversized snapshot is not retained; the release comparison rescans only when the version moved. | unaudited |
| [Durability pin once per connection][pin-test] | A fenced write does not re-run the pin; the first fenced write after maintenance re-pins `synchronous=FULL`; a panicking maintenance callback still re-arms the pin and discards the snapshot. | unaudited |
| [Resource pragmas belong to the open path][resource-pragma-test] | Read and fenced callbacks are denied `cache_size`, `temp_store`, and `mmap_size` writes; maintenance-set values stand. | unaudited |
| [Statement evictions per text][eviction-probe] | A handle with no runs after its text had run counts as an eviction; an undersized cache shows one, a fitted cache none. | unaudited |
| [Memory-store connection profile][profile-test] | `cache_size` pages times the measured page size equals the budget; `mmap_size` is the budget capped by `MAX_MMAP_SIZE`; `temp_store` is memory. | unaudited |
| [Steady passes evict nothing][pass-probe] | A warm pass and four steady passes on one session prepare more distinct texts than the default capacity, fewer than the configured capacity with headroom, and re-create no cached statement. | unaudited |
| [Foreign rename observed][rename-test] | After an `ALTER TABLE ... RENAME` on a second connection, the next callback denies a temp shadow of the new name, allows the old one, and still refuses a maintenance-left shadow. | unaudited |

[reuse-probe]: ../../../crates/storage/src/lib.rs#L4548-L4632
[read-witness]: ../../../crates/storage/src/lib.rs#L4795-L4824
[temp-write-test]: ../../../crates/storage/src/lib.rs#L4831-L4854
[mode-restore-test]: ../../../crates/storage/src/lib.rs#L4860-L4912
[baseline-gate-test]: ../../../crates/storage/src/lib.rs#L4918-L4948
[surface-guard-test]: ../../../crates/storage/src/lib.rs#L4955-L4975
[gate-tests]: ../../../crates/storage/src/lib.rs#L2075-L2414

## History render

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [Inner budget guard][inner-guard] | Positive budgets drive oldest-first demotion. | unaudited |
| [Outer retry bound][outer-guard] | The wrapped history slice is retried at most three times above 105%. | unaudited |
| [SOFT/defer core branches][core-branches] | Defer queues work without replacing its prefix; SOFT applies rendered delta units. | unaudited |
| [SOFT producer boundary][soft-producer] | Ordinary SOFT submits m1 and other rendered units, not a replacement m0. | unaudited |
| [Newest tier][newest-tier] | The newest compartment renders its full tier. | unaudited |
| [Archived tier][archived-tier] | Tier 5 is omitted. | unaudited |
| [Empty tier body][empty-tier] | An empty body retains its title heading below archive tier. | unaudited |
| [Heading safety][heading-safety] | Historian titles remain on an XML-safe heading line. | unaudited |
| [Clean title parity][clean-title] | A clean title stays byte-identical. | unaudited |
| [Dates and body headings][date-headings] | Date ranges compress and heading-like body lines are indented. | unaudited |
| [Legacy rendering][legacy-render] | Legacy rows use truncation and their tier rule. | unaudited |
| [Malformed tier fallback][malformed-tier] | A pseudo-v2 row renders flat content rather than disappearing. | unaudited |
| [Stored-compartment projection][stored-render] | Stored fields project into the render representation. | unaudited |
| [Wrapped slice extraction][slice-extract] | The shortest complete literal block is extracted. | unaudited |
| [Oldest-first fixture][oldest-test] | A character-count fixture checks fit and newest survival. | unaudited |
| [Render golden][render-golden-test] | JSON expected bodies are compared with the renderer using `no_guard`. | unaudited |
| [Store-shape differential][shape-test] | Real-tokenizer costs, body SHA-256, and tier counts match supplied JSON cases. | unaudited |
| [Tight golden][tight-test] | Final bytes match JSON using the real estimator. | unaudited |
| [Cached count parity][cache-test] | Repeated cached counts equal direct tokenizer counts. | unaudited |
| [Cache hit accounting][cache-hit] | Digest-keyed hits skip retokenization. | unaudited |
| [Cache generation rotation][cache-rotation] | Recently hit entries survive promotion between generations. | unaudited |
| [Cache capacity][cache-capacity] | Insertion rotates the bounded generation. | unaudited |
| [Cache statistics][cache-stats] | Calls partition into hits, misses, and bypasses. | unaudited |
| [Cache key domains][cache-domains] | Kind-prefixed and raw-content keys do not alias in the fixture. | unaudited |

The tight-golden `fired` counter increments on final fit or empty output. It
does not independently witness more than one reference demotion. The
store-shape test's monotonic comparison is fixture-local, not a global BPE law.
No complete outer-boundary matrix or H3/H4 campaign marker is found
in these inspected checks. Goldens use JSON and SHA-256, not insta snapshots.

## Redaction ownership

| Check | Source condition or assertion | Status |
| --- | --- | --- |
| [Preparation guards][preparation] | Input bounds, detected new identities, and output bounds reject before scan append. | unaudited |
| [Audit transaction][audit-tx] | Applied effects and audit persist in one fenced transaction; replay does not append a new audit batch here. | unaudited |
| [Receipt actions][receipt-test] | Existing identities record preserve; content records substitute. | unaudited |
| [Post-redaction bound][bound-test] | Growth beyond 512 KiB refuses; an exactly fitting redacted output succeeds. | unaudited |
| [State-sync scan rollback][scan-rollback-test] | A metadata failure leaves pending_agent_drops empty; this is not an entire effect/audit rollback assertion. | unaudited |
| [Transaction identity decision][identity-test] | Identity status is decided from the write transaction. | unaudited |
| [Preserved JSON identities][json-identity-test] | Identity preservation does not exempt integrity fields, credential names, or nested values. | unaudited |
| [Trace identity policy][trace-identity-test] | A new secret session is refused while a stored session can continue tracing. | unaudited |
| [active_note_scan_audit_is_atomic_complete_and_opaque][audit-complete] | The test checks audit table counts, field IDs, zero/nonzero findings, opaque IDs, hashed owner keys, secret-byte absence with a positive control, detection-column names, and persistence after reopen. | unaudited |
| [Last-session-owner expiry][audit-expiry] | Session deletion removes the active note audit; compartment writes have expected scan counts. | unaudited |
| [Last shared owner][audit-last-owner] | Audit rows survive until their final owner expires. | unaudited |
| [Lineage scan links][audit-lineage] | Copying links existing scans without rescanning and survives source deletion. | unaudited |
| [Audit backup][audit-backup] | Online backup preserves active audit row counts and content. | unaudited |
| [Concurrent replay audit][audit-duplicate] | Duplicate facade calls invoke the operation once and retain one batch. | unaudited |
| [Write registry bindings][write-registry] | Declared write families reference real bindings and checked tests. | unaudited |
| [Cache-state policy][cache-state-policy] | Payloads redact, existing identities persist, and integrity refusal leaves no rejected row. | unaudited |
| [Diagnostic content][diagnostic-policy] | Diagnostics are redacted before persistence. | unaudited |
| [Authority identities][authority-policy] | New secret identities are refused while exact existing bindings are retained. | unaudited |
| [Mural integrity][mural-policy] | Secret bytes, hashes, and new identities are refused. | unaudited |
| [Workspace seed policy][workspace-policy] | Categories redact and identity refusal preserves rows and audit counts. | unaudited |
| [Authority creation/checksums][checksum-policy] | Creation and checksum fields refuse secret material. | unaudited |
| [Note field policy][note-policy] | Content substitutes and integrity fields refuse. | unaudited |
| [Durable content fixtures][content-fixtures] | Stored note content matches the versioned field-policy fixture. | unaudited |
| [Provider detection label][provider-label] | A provider-rule substitution persists its expected detection label. | unaudited |
| [Transaction facade policy][transaction-policy] | Transaction output redacts; replay and size refusal preserve committed audit counts. | unaudited |
| [Facade authority refusal][facade-refusal] | Non-module authority refuses mutation without changing observed notes or audit counts. | unaudited |
| [Idempotency identity policy][idempotency-policy] | New secret identities refuse before invocation without substitution or key collapse. | unaudited |
| [Compartment field policy][compartment-policy] | Content redacts and rejected new message identities leave prior compartments intact. | unaudited |
| [Note transition policy][transition-policy] | Transition content redacts and compiled artifacts refuse secrets. | unaudited |

No full six-policy differential matrix, in-memory no-append assertion, or
metadata-only retained-payload oracle is identified in this scope. No
allocation-benefit claim is established by these functional checks. Privacy
witnesses must use a unique long synthetic sentinel and must not reject legal
ExistingIdentity preservation merely because it retains input bytes.

## Explicitly deferred checks

The inventory above includes every known claim-bearing check identified for
these focused selection, ownership, callback, renderer, and field-preparation
changes. The following categories are deliberately not expanded here:

- Kernel startup/readiness, mutation authorization, artifact disposition, and
  wire-request validation tests exercise different operation boundaries. Their
  catalogs remain authoritative; K1 does not replace those generic obligations.
- Host correlation allocation, stream framing, subprocess panic-hook output,
  and generic connection setup remain in the [host catalog][host-catalog].
  The route-close overlap, cancellation, and relevant capacity tests are included.
- Storage opening, lease acquisition, baseline-format checks, and filesystem
  protection remain in [shared primitives][shared-catalog]. Their generic
  invariants are retained; callback authority and freshness are inventoried here.
- The renderer's [tagged-session fixture-builder test][fixture-builder] checks
  its support constructor, not the budget loop or its oracle. Broader transform
  state-machine tests remain in the [transform catalog][transform-catalog].
- Detailed note workflows and detector internals remain in the
  [memory-store catalog][memory-catalog]. All 21 claim-bearing tests in
  production_redaction.rs are listed above, including audit retention and backup.

These are scope deferrals, not findings that the tests are absent or adequate.

## Audit handoff

All tests go to `/testing:invariant-test-review` before an adequacy verdict.
Production guards go to
`/low-level-systems:defensive-assertions-and-invariant-guards`. Empty categories
above mean no check was identified in the stated inspected scope, not a claim
that no related check exists anywhere in the repository.

[read-guards]: ../../../crates/daemon/src/kernel_routes/read.rs#L159-L248
[injectable-test]: ../../../crates/daemon/src/canonical_memory.rs#L282
[revision-test]: ../../../crates/daemon/src/canonical_memory.rs#L315
[dropped-test]: ../../../crates/daemon/src/canonical_memory.rs#L373
[suffix-test]: ../../../crates/daemon/src/canonical_memory.rs#L390
[memory-budget-test]: ../../../crates/daemon/src/canonical_memory.rs#L412
[withheld-test]: ../../../crates/daemon/src/canonical_memory.rs#L457
[transform-memory-test]: ../../../crates/daemon/tests/transform_canonical_memory.rs#L43
[lag-test]: ../../../crates/daemon/tests/transform_canonical_memory.rs#L230
[disabled-test]: ../../../crates/daemon/tests/transform_canonical_memory.rs#L309
[configured-budget-test]: ../../../crates/daemon/tests/transform_canonical_memory.rs#L345
[row-cap-test]: ../../../crates/daemon/tests/kernel_routes.rs#L2002
[byte-cap-test]: ../../../crates/daemon/tests/kernel_routes.rs#L2156
[permits]: ../../../crates/host-runtime/src/dispatch.rs#L823-L855
[handler-fence]: ../../../crates/host-runtime/src/dispatch.rs#L873-L931
[close-gate]: ../../../crates/host-runtime/src/dispatch.rs#L1234-L1298
[reservations]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L513-L550
[saturation-test]: ../../../crates/host-runtime/tests/dispatch.rs#L294
[cancel-test]: ../../../crates/host-runtime/tests/dispatch.rs#L357
[simultaneous-test]: ../../../crates/host-runtime/tests/dispatch.rs#L452
[upload-cap-test]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L910
[upload-finish-test]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L965
[upload-keep-test]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1103
[scope-install]: ../../../crates/storage/src/lib.rs#L972-L1074
[scope-restore]: ../../../crates/storage/src/lib.rs#L652-L705
[facade-scope]: ../../../crates/memory-store/src/lib.rs#L5563-L5586
[shadow-create-test]: ../../../crates/storage/src/lib.rs#L2506
[shadow-test]: ../../../crates/storage/src/lib.rs#L2552
[lower-test]: ../../../crates/storage/src/lib.rs#L2597
[schema-test]: ../../../crates/storage/src/lib.rs#L2672
[unwind-test]: ../../../crates/storage/src/lib.rs#L2356-L2405
[durability-test]: ../../../crates/storage/src/lib.rs#L4042-L4107
[read-escape-test]: ../../../crates/storage/src/lib.rs#L3498
[tx-escape-test]: ../../../crates/storage/src/lib.rs#L3548
[fence-row-test]: ../../../crates/storage/src/lib.rs#L3595
[format-test]: ../../../crates/storage/src/lib.rs#L3700
[reentry-test]: ../../../crates/storage/src/lib.rs#L3832
[cached-test]: ../../../crates/storage/src/lib.rs#L4501
[snapshot-test]: ../../../crates/storage/src/lib.rs#L3946-L3981
[read-tx-test]: ../../../crates/storage/src/lib.rs#L3986
[attach-test]: ../../../crates/storage/src/lib.rs#L4019
[pragma-test]: ../../../crates/storage/src/lib.rs#L4052
[rollback-test]: ../../../crates/storage/src/lib.rs#L4116
[inner-guard]: ../../../crates/daemon/src/decay_render.rs#L324-L338
[outer-guard]: ../../../crates/daemon/src/m0_compose.rs#L185-L215
[oldest-test]: ../../../crates/daemon/src/decay_render.rs#L509
[render-golden-test]: ../../../crates/daemon/src/decay_render.rs#L614
[shape-test]: ../../../crates/daemon/src/decay_render.rs#L644
[tight-test]: ../../../crates/daemon/src/decay_render.rs#L765-L800
[cache-test]: ../../../crates/daemon/src/token_cache.rs#L188-L205
[preparation]: ../../../crates/memory-store/src/lib.rs#L2204-L2243
[audit-tx]: ../../../crates/memory-store/src/lib.rs#L2245-L2271
[receipt-test]: ../../../crates/memory-store/src/lib.rs#L18171-L18212
[bound-test]: ../../../crates/memory-store/src/lib.rs#L22694-L22725
[scan-rollback-test]: ../../../crates/memory-store/src/lib.rs#L22521-L22605
[identity-test]: ../../../crates/memory-store/src/lib.rs#L15309
[json-identity-test]: ../../../crates/memory-store/src/lib.rs#L15166
[trace-identity-test]: ../../../crates/memory-store/src/lib.rs#L15376
[served-columns]: ../../../crates/kernel/src/admission.rs#L3306
[cross-project]: ../../../crates/daemon/tests/kernel_routes.rs#L1354
[project-scope]: ../../../crates/daemon/tests/kernel_routes.rs#L1847
[historical-read]: ../../../crates/daemon/tests/kernel_routes.rs#L1919
[object-filter]: ../../../crates/daemon/tests/kernel_routes.rs#L2060
[selection-reference]: ../../../crates/daemon/tests/kernel_routes.rs#L2122-L2152
[render-category]: ../../../crates/daemon/src/memory_render.rs#L257
[render-markup]: ../../../crates/daemon/src/memory_render.rs#L284
[render-content-cap]: ../../../crates/daemon/src/memory_render.rs#L308
[render-vocabulary]: ../../../crates/daemon/src/memory_render.rs#L352
[render-order]: ../../../crates/daemon/src/memory_render.rs#L375
[parse-admission]: ../../../crates/daemon/src/lib.rs#L11805-L11826
[byte-charge]: ../../../crates/host-runtime/src/wire.rs#L430-L481
[decode-admission]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L406-L425
[route-overlap]: ../../../crates/host-runtime/tests/dispatch.rs#L1156-L1212
[stream-cancel]: ../../../crates/host-runtime/tests/dispatch.rs#L503
[handler-panic]: ../../../crates/host-runtime/tests/dispatch.rs#L551
[t-cancel-work]: ../../../crates/host-runtime/tests/dispatch.rs#L725-L776
[t-close-work]: ../../../crates/host-runtime/tests/dispatch.rs#L781-L825
[t-detached-work]: ../../../crates/host-runtime/tests/dispatch.rs#L830-L866
[t-fatal-work]: ../../../crates/host-runtime/tests/dispatch.rs#L871-L898
[t-late-work]: ../../../crates/host-runtime/tests/dispatch.rs#L901-L944
[t-panic-work]: ../../../crates/host-runtime/tests/dispatch.rs#L950-L984
[t-stderr-work]: ../../../crates/host-runtime/tests/dispatch.rs#L610-L612
[output-reservation]: ../../../crates/host-runtime/tests/dispatch.rs#L1034
[egress-exhaustion]: ../../../crates/host-runtime/tests/dispatch.rs#L1110
[reserved-isolation]: ../../../crates/host-runtime/tests/dispatch.rs#L1294
[general-isolation]: ../../../crates/host-runtime/tests/dispatch.rs#L1391
[request-cap]: ../../../crates/daemon/src/lib.rs#L18568
[parse-nodes]: ../../../crates/daemon/src/lib.rs#L18628
[parse-copies]: ../../../crates/daemon/src/lib.rs#L18647
[parse-dense]: ../../../crates/daemon/src/lib.rs#L18663
[upload-restore]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1130
[upload-begin]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1203
[upload-replace]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1258
[upload-full]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1274
[upload-stale]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1290
[upload-active]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1310
[decode-test]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1351
[upload-cleared]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1375
[upload-discard]: ../../../crates/daemon/src/kernel_routes/ingest.rs#L1397
[reentry-guard]: ../../../crates/storage/src/lib.rs#L195-L209
[fenced-guard]: ../../../crates/storage/src/lib.rs#L290-L316
[checkpoint-test]: ../../../crates/storage/src/lib.rs#L2792
[persist-test]: ../../../crates/storage/src/lib.rs#L3355
[read-write-test]: ../../../crates/storage/src/lib.rs#L3372
[maintenance-test]: ../../../crates/storage/src/lib.rs#L3758
[udf-test]: ../../../crates/storage/src/lib.rs#L3780
[stale-precheck]: ../../../crates/storage/src/lib.rs#L2124
[negative-fence]: ../../../crates/storage/src/lib.rs#L4145
[handover-test]: ../../../crates/storage/src/lib.rs#L4175
[equal-epoch]: ../../../crates/storage/src/lib.rs#L4233
[core-branches]: ../../../crates/cache-stability/src/lib.rs#L221-L287
[soft-producer]: ../../../crates/daemon/src/transform.rs#L4455-L4507
[newest-tier]: ../../../crates/daemon/src/decay_render.rs#L384
[archived-tier]: ../../../crates/daemon/src/decay_render.rs#L400
[empty-tier]: ../../../crates/daemon/src/decay_render.rs#L408
[heading-safety]: ../../../crates/daemon/src/decay_render.rs#L415
[clean-title]: ../../../crates/daemon/src/decay_render.rs#L437
[date-headings]: ../../../crates/daemon/src/decay_render.rs#L445
[legacy-render]: ../../../crates/daemon/src/decay_render.rs#L478
[malformed-tier]: ../../../crates/daemon/src/decay_render.rs#L493
[stored-render]: ../../../crates/daemon/src/decay_render.rs#L529
[slice-extract]: ../../../crates/daemon/src/decay_render.rs#L573
[cache-hit]: ../../../crates/daemon/src/token_cache.rs#L209
[cache-rotation]: ../../../crates/daemon/src/token_cache.rs#L223
[cache-capacity]: ../../../crates/daemon/src/token_cache.rs#L233
[cache-stats]: ../../../crates/daemon/src/token_cache.rs#L249
[cache-domains]: ../../../crates/daemon/src/token_cache.rs#L266
[audit-complete]: ../../../crates/memory-store/tests/production_redaction.rs#L68-L184
[audit-expiry]: ../../../crates/memory-store/tests/production_redaction.rs#L187
[audit-last-owner]: ../../../crates/memory-store/tests/production_redaction.rs#L248
[audit-lineage]: ../../../crates/memory-store/tests/production_redaction.rs#L327
[audit-backup]: ../../../crates/memory-store/tests/production_redaction.rs#L440
[audit-duplicate]: ../../../crates/memory-store/tests/production_redaction.rs#L484
[write-registry]: ../../../crates/memory-store/tests/production_redaction.rs#L532
[cache-state-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L606
[diagnostic-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L714
[authority-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L731
[mural-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L776
[workspace-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L822
[checksum-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L900
[note-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L999
[content-fixtures]: ../../../crates/memory-store/tests/production_redaction.rs#L1060
[provider-label]: ../../../crates/memory-store/tests/production_redaction.rs#L1086
[transaction-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L1131
[facade-refusal]: ../../../crates/memory-store/tests/production_redaction.rs#L1261
[idempotency-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L1391
[compartment-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L1461
[transition-policy]: ../../../crates/memory-store/tests/production_redaction.rs#L1512
[fixture-builder]: ../../../crates/daemon/src/decay_render.rs#L809
[host-catalog]: ../host-runtime/catalog.md
[shared-catalog]: ../shared-primitives/catalog.md
[transform-catalog]: ../daemon/transform/catalog.md
[memory-catalog]: ../memory-store/catalog.md
[snapshot-key-test]: ../../../crates/storage/src/lib.rs#L2171-L2305
[pin-test]: ../../../crates/storage/src/lib.rs#L2313-L2405
[rename-test]: ../../../crates/storage/src/lib.rs#L4639-L4679
[resource-pragma-test]: ../../../crates/storage/src/lib.rs#L4685-L4723
[eviction-probe]: ../../../crates/storage/src/lib.rs#L4729-L4788
[profile-test]: ../../../crates/memory-store/src/lib.rs#L15120-L15149
[pass-probe]: ../../../crates/daemon/src/lib.rs#L24573-L24603
