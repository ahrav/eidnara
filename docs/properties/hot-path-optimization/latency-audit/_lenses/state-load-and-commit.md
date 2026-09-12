# Cache-state load and commit surface

This lens records what the per-turn `transform` pass reads from and writes to
`cache_state`, `pass_trace`, and `historian_side_channel_outbox`, and what any
change to that traffic must preserve. Anchors are checked in
`/local/home/ahrav/scratch/eidnara` at
`913234433ae36a80a6e22c6aac14c7f9aab74386` on 2026-09-10. It is analysis only;
no test ran and nothing outside this file changed. Fencing, `synchronous=FULL`,
snapshot isolation, and CAS obligations stay with the [memory-store][ms-catalog]
and [shared-primitives][sp-catalog] catalogs; callback batching boundaries stay
with [S2][s2]; prepared-field audit ownership stays with [R1][r1], [R2][r2],
and [R3][r3]. This lens adds the pass-level ordering of loads and commits, the
scalar-read equivalence, the pass-trace contract, the outbox delivery contract,
and the `meta` preparation contract.

One pass touches `cache_state` through [`MemoryStore::load`][load], which
runs [`CACHE_STATE_FULL_SELECT`][full-select] in one deferred read
transaction ([`with_conn`][with-conn]) and deserializes both `core_state` and
[`ModuleMeta`][meta-struct] with serde. The handler loads before the transform
for the [projection-cache epoch][epoch-read] and for
[`historian_active`][active], inside the transform through
[`load_transform_snapshot`][snapshot] (whose [implementation][snapshot-impl]
reads the row and the overlay tables in one read transaction), and after the
transform inside [`prepare_historian_fire`][prepare], whose `loaded` feeds the
[`record_no_fire`][no-fire] CAS write with `loaded.row_version` and gates on
[`pending_rewrite`][meta-pending]. The Emergency95 arm adds [two more
loads][floor-a] that compare [`publication_floor_ordinal`][meta-floor] before
and after historian work and may [rerun the transform][rerun]. A CAS conflict
[reruns `apply_once`][cas-retry], which reloads before the
[main commit][main-commit]. Every durable write runs through
[`with_conn_fenced`][fenced]: fence precheck, [`synchronous=FULL`
pin][pin-sync], IMMEDIATE transaction, [fence claim][claim-fence], commit.
[`PreparedWrite::execute`][pw-execute] wraps that and appends audit rows in
[`persist_audit`][audit] for every recorded scan, each keyed by an
[opaque id][opaque-id], so a [`trace_pass_received`][received] call is a
fenced, fsynced transaction that also inserts `scan_batches`, `field_scans`,
and `scan_owner_copies` rows under the `pass_trace` owner. The pass also runs
[`trace_pass_completed`][completed] after the response is built, on stable
passes [`trace_pass_stable`][stable] from inside the transform, and on failure
[`trace_pass_rejected`][rejected]. All four write the
[`pass_trace` table][pass-trace-sql] read back by [`load_pass_trace`][load-trace].
[`commit_transform`][commit] already upserts `pass_trace` in the commit
transaction for divergence and scheduler history but leaves `receive_count`
untouched on conflict and initializes it to `0` on a fresh insert.

The historian outbox drain runs [before the transform][drain-call] on every
pass with per-kind limit 32. [`drain_historian_side_channels`][drain] first
[deletes already-delivered rows][delete-all] in one fenced transaction, then
per kind loads due rows in a read transaction, delivers each row by inserting
the target row and marking `delivered_at_ms` in [one fenced
transaction][deliver], and then deletes the marked row in [a second fenced
transaction][delete-one]. The [doc comment][drain-doc] states the
mark-then-delete design intent. Rows are [enqueued][enqueue] by the publish
transaction into the [outbox table][outbox-sql], whose primary key is the
delivery identity and whose [due index][idx-due] serves the pending count. The
same drain also runs [after a historian publish][publish-drain] on the
historian task, so two drainers can overlap on one session. The `meta` blob is
stored through [`json_content`][json-content] under the
[`DurablePreserveIdentities`][policy] policy, which parses with
[`parse_json_with_unique_names`][unique], validates keys, walks and redacts a
clone, and [returns the original input bytes][clean-branch] when nothing
changed. Scalar fields go through [`prepare_field`][prepare-field] instead, and
every prepared text is bounded by [`MAX_DURABLE_TEXT_BYTES`][max-text].

## Candidate properties

### post-commit-pass-reads-observe-own-commit

- Type: safety
- Check: `always` - Within one pass, every `cache_state` read that executes
  after `commit_transform`, `descend_lineage`, `truncate_compartments_for_revert`,
  or an awaited historian firing observes a `row_version` at least as new as
  the one that work returned, and any CAS write derived from that read uses
  that `row_version`. `always` because the daemon consumes those reads on
  every committing pass, not only under a fault.
- Guarantee: Consolidating loads never hands a pre-commit snapshot to a
  consumer that runs after the pass's own durable write.
- Rationale: [`prepare_historian_fire`][prepare] loads after
  [`run_transform`][run] returns and passes `loaded` into
  [`record_no_fire`][no-fire], which commits `last_no_fire` under
  `loaded.row_version`. A pre-commit `loaded` makes that CAS fail silently
  (`let _ =`), so the skip reason is never persisted. The same `loaded` supplies
  `core.frozen_units` to `projected_post_drop_percentage` and the
  `fold_is_only_reclaim` debug assertion, and `meta.pending_rewrite`,
  `meta.historian.*`, and `meta.ordinal_continuation_base` to fire gating. The
  Emergency95 arm reads `publication_floor_ordinal` [after the transform][floor-a]
  and [again after historian work][floor-b]; those two reads must stay distinct
  because the comparison is the rerun trigger. Inside the transform, the
  lineage-switched path [loads][descent-load], [descends][descend], and then
  takes the [snapshot][snapshot], and the revert path [truncates][truncate] and
  then [reloads compartments][truncate-reload]. On CAS conflict the
  [retry loop][cas-retry] reruns `apply_once`, which reloads.
- Fault/timing angle: A consolidation reuses a snapshot taken before
  `commit_transform` for a consumer placed after it, or reuses the first
  `run_transform` snapshot for an Emergency95 rerun after an inline firing.
- Required faults and enabling state: A pass that commits, then reaches
  `prepare_historian_fire` with a new no-fire reason; an Emergency95 pass with
  a publication landing between the transform and the floor check; a CAS
  conflict injected between snapshot and commit.
- Reachability: default-production - the handler path at
  [lib.rs:8202-8377][handler] runs for every transform request;
  `compaction_enabled` defaults to `true` ([config.rs:121][cfg-compaction]).
  The Emergency95 arm needs usage at or above the emergency threshold.
- Existing check: [`no_fire_reason_is_durable_change_gated_and_cleared_by_fire`][t-no-fire]
  asserts `last_no_fire == Some("no_models")` after one pass and an unchanged
  `row_version` after a repeated reason, which fails if the post-commit load
  is replaced by a pre-commit one (unaudited).
  [`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`][t-emergency]
  uses the [`between_transform_and_prepare`][hook] hook (unaudited).
  [`handler_delta_boundary_divergence_recut_retries_cas_without_stale_projection`][t-cas]
  covers the CAS rerun (unaudited).
- Open questions:
  - Which of the three pre-commit loads (epoch, `historian_active`, snapshot)
    may share one snapshot? Merging them removes a window in which the
    projection-cache epoch is read before a concurrent recut; the specification
    should state whether closing that window is intended. (needs human input)

### pre-commit-scalar-projection-matches-full-deserialize

- Type: safety
- Check: `always` - For every stored `meta` text, a scalar projection of
  `revert_epoch` and `historian.state` returns the same value as
  `serde_json::from_str::<ModuleMeta>(meta)` when that deserialization
  succeeds, and when it fails, or when `core_state` fails to deserialize, the
  consumer takes the same conservative branch it takes today (`None` for the
  projection cache, `false` for `historian_active`). `always` because these
  reads sit on every pass.
- Guarantee: A narrow read never selects a projection-cache entry or clears a
  historian veto that the full load would not.
- Rationale: [`revert_epoch`][meta-epoch] and [`historian`][meta-historian]
  carry `#[serde(default)]`, and [`HistorianDurableState::state`][hds-state]
  does too, so an absent key deserializes to `0` or `Idle` where a JSON path
  extract returns NULL. [`HistorianPhase`][phase] serializes with
  `rename_all = "snake_case"`, so the stored strings are `idle`, `firing`,
  `awaiting_producer`, `validating`, `publishing`, matching `as_str`. An
  unknown string or a `null` under `revert_epoch` fails serde but not a path
  extract. Today [`lookup_full_projection_cache`][epoch-read] and
  [`expand_transform_tail_delta`][epoch-read-delta] map any load error to
  `None`, and [`historian_active`][active] maps it to `false`; a corrupt
  `core_state` column trips both because [`load`][load] decodes both columns.
  The comment at [lib.rs:4174][epoch-comment] states why the epoch must be the
  persisted one. `ModuleMeta` has no `deny_unknown_fields` and no `flatten`, and
  `revert_epoch` grows by `saturating_add`, so the SQLite integer range is not
  a practical concern but belongs in the equivalence statement.
- Fault/timing angle: none; the divergence is a data-shape difference, not an
  interleaving.
- Required faults and enabling state: Rows whose `meta` lacks `revert_epoch`
  or `historian`, rows with an unknown `historian.state` string, rows with
  `null` under `revert_epoch`, and rows whose `core_state` is not valid JSON.
- Reachability: default-production - both reads run on the ordinary handler
  path; the delta-request read needs a `tail_delta` request.
- Existing check: none found for equivalence between a narrow read and the
  full load, and none found for `Handler::historian_active` reading durable
  state. The seven in-transaction users of
  [`CACHE_STATE_META_SELECT`][meta-select] all deserialize the whole
  `ModuleMeta` as their predicate source and are not scalar reads.
- Open questions:
  - Must the narrow read fail the same way on a corrupt `core_state`, or is a
    meta-only read allowed to proceed? (needs human input)
  - Does SQLite's JSON path extraction return the first or the last duplicate
    key? Unresolved; only relevant if a writer bypasses
    `parse_json_with_unique_names`. No such writer of caller-influenced `meta`
    was found; [`reset_session_for_recomp`][recomp] writes a self-generated
    `ModuleMeta` without scanning.

### pass-receive-breadcrumb-is-recorded-once-per-pass-before-outcome

- Type: safety
- Check: `always` - For each transform request that reaches the handler after
  admission, `pass_trace.receive_count` increases by exactly one and
  `last_received_at_ms` is set, whether the transform later commits, stays
  stable, or is rejected; `reject_count` increases by exactly one on a
  rejection while `cache_state.row_version` is unchanged; `first_divergence`
  is NULL after a rejected pass; and `scheduler_history` gains exactly one
  observation per accepted pass, from either `trace_pass_stable` or
  `commit_transform`, never both. `always` because status, health, and the
  plugin display read these counters on every request.
- Guarantee: The receive breadcrumb is independent of the pass outcome and of
  the cache-state CAS.
- Rationale: [`trace_pass_received`][received] runs before
  [`run_transform`][run], sets `first_divergence = NULL`, and bumps
  `receive_count`. A rejected transform runs only
  [`trace_pass_rejected`][rejected-call]; no commit occurs. The [`PassTrace`
  doc][passtrace-doc] states the counters exist so a rejected pass leaves a
  trail without advancing `row_version`. [`commit_transform`][commit-trace]
  initializes `receive_count` to `0` on a fresh insert and leaves it alone on
  conflict, so a fold that runs the bump inside the commit records nothing for
  rejected passes and nothing for stable passes, which have no commit
  transaction at all ([`record_stable_pass_trace`][stable-call] runs only when
  `!committed`). Consumers: [session status][status-read] and
  [health][health-read] JSON, the `newest_pass_at` age computation at
  [lib.rs:6245-6254][age], and the plugin's `Passes: N received, M rejected`
  line at [command-handler.ts:267][plugin]. No scheduler decision reads
  `pass_trace`; [`load_pass_scheduler_history`][sched-history] has one
  non-store caller and it is a test at [transform.rs:13564][sched-test].
- Fault/timing angle: A fold moves the bump after `run_transform`, so a
  rejected or stable pass under-counts, or a rerun Emergency95 pass
  double-counts if the bump is attached to every commit.
- Required faults and enabling state: A transform that rejects (ordinal
  violation); a stable pass; an Emergency95 pass that reruns and commits
  twice; a fresh session whose first pass commits.
- Reachability: default-production - every transform request.
- Existing check: [`transform_reject_records_trace_without_advancing_row_version`][t-reject],
  [`transform_success_records_received_and_completed_trace`][t-success],
  [`repeated_rejects_increment_trace_and_overwrite_last_error`][t-repeat],
  [`sequential_failing_passes_trace_every_reject_while_cache_state_stays_frozen`][t-frozen],
  [`status_and_health_surface_pass_trace_for_rejected_sessions`][t-status],
  [`pass_trace_upserts_counts_and_caps_errors`][t-upserts],
  [`scheduler_trace_records_every_pass_and_preserves_variable_arm_state`][t-sched]
  (all unaudited). None found for `first_divergence` being NULL after a
  rejected pass, and none found for `receive_count` after an Emergency95 rerun.
- Open questions:
  - Is under-counting rejected passes an acceptable semantic change, or must
    the receive breadcrumb keep a home on the reject and stable paths? The
    existing tests encode `receive_count == reject_count` after rejects.
    (needs human input)
  - The `session_id` scan for `pass_trace` is owned by the `pass_trace` owner
    under `transform_diagnostics`; a fold moves it under `cache_state`. R3's
    owner-relationship normalization decides whether that is a change.

### pass-trace-write-stays-outside-the-cache-cas

- Type: safety
- Check: `always` - A `pass_trace` write never changes `cache_state` and a
  `pass_trace` failure never aborts an otherwise valid cache commit, unless
  the specification states the new coupling. `always` because the daemon
  ignores trace-write errors with `let _ =` on every call site.
- Guarantee: Diagnostics cannot veto or enlarge the state commit.
- Rationale: Every trace call site discards its result
  ([8131][received-call], [8196][rejected-call], [8436][completed-call],
  [1836][stable-call]). The doc comments on [`trace_pass_received`][received-doc]
  and [`trace_pass_completed`][completed-doc] state that these writes stay
  outside the fenced cache-state transaction so they cannot contend with,
  extend, or alter CAS semantics. Folding the bump into `commit_transform`
  makes a `pass_trace` constraint failure or a `session_id` identity refusal
  abort the state commit, which is the coupling the comments exclude. The
  identity refusal is already reachable: a secret-bearing `session_id` on a
  known session is [tolerated by the trace write][flagged] only because the
  row exists.
- Fault/timing angle: A `pass_trace` statement error inside the fused
  transaction rolls back the cache-state row.
- Required faults and enabling state: Inject a failure into the `pass_trace`
  upsert (constraint or fault point) during a committing pass and compare the
  cache-state outcome with the baseline.
- Reachability: default-production.
- Existing check: [`pass_trace_refuses_a_new_secret_session_and_keeps_tracing_a_stored_one`][t-secret]
  covers the identity gate (unaudited). None found for a `pass_trace` failure
  beside a successful cache commit.
- Open questions:
  - If the fold is accepted, which of the two comments is rewritten, and what
    replaces the "never contends with the pass commit" promise? (needs human
    input)

### side-channel-row-delivers-exactly-once

- Type: safety
- Check: `always` - For every `historian_side_channel_outbox` row, the target
  table receives exactly one row for it across all drains, restarts, and
  overlapping drainers, and the outbox state change that retires the row
  commits in the same transaction as the target insert. A delivery whose
  outbox row is no longer pending when its transaction runs must roll back its
  target insert. `always` because [`compartment_events`][events-insert] and
  [`user_memory_candidates`][obs-insert] are plain inserts with no dedupe;
  only [`primer_candidates`][primer-insert] upserts.
- Guarantee: The outbox row is the only duplicate guard for events and user
  observations.
- Rationale: [`deliver_historian_side_channel`][deliver] inserts the target
  and calls [`mark_historian_side_channel_delivered_tx`][mark] in one fenced
  transaction; the mark requires `changed == 1` under
  `delivered_at_ms IS NULL` and returns an error otherwise, which rolls the
  fenced transaction back. [`load_due_historian_side_channels`][load-due]
  selects only `delivered_at_ms IS NULL`, so a marked row is never redelivered
  even if the process dies before the [per-row delete][delete-one]; the
  [drain start][drain] deletes leftovers. The publish task also drains
  ([lib.rs:10766-10770][publish-drain]), so two drainers can load the same due
  row; the loser's mark returns `changed == 0`. With delete-in-place the
  equivalent guard is a `DELETE` that affects one row, else rollback.
  [`historian_side_channel_status`][status-sc] counts pending as
  `delivered_at_ms IS NULL`, so retiring by delete keeps the pending count
  unchanged.
- Fault/timing angle: Process crash between the mark commit and the delete
  commit; two drainers overlapping on one session; a target insert failing
  after the outbox state change in a reordered transaction.
- Required faults and enabling state: A published firing with events, primers,
  and user observations; a crash or abort injected between the two fenced
  transactions; a second drainer started between load and deliver.
- Reachability: default-production for the drain call
  ([lib.rs:8124][drain-call]); explicit-config-only for row delivery, because
  outbox rows come from [`publish_historian_chunk`][publish] and firing
  requires a configured `model_chain` ([config.rs:119][cfg-models],
  [`no_models` gate][no-models]); `user_observation` rows further require
  `user_memory_collection_enabled` ([config.rs:126][cfg-user-mem]).
- Existing check: [`historian_side_channel_outbox_recovers_after_restart`][t-restart]
  covers fail, reopen, redeliver once (unaudited).
  [`historian_side_channel_faults_are_isolated_and_retryable_per_kind`][t-faults]
  covers one failed kind and a later successful retry (unaudited). None found
  for a crash between mark and delete, and none found for two overlapping
  drainers.
- Open questions: None.

### side-channel-drain-honors-order-backoff-and-isolation

- Type: safety
- Check: `always` - A drain visits kinds in the order `event`, `primer`,
  `user_observation`; within a kind it delivers due rows ordered by
  `firing_seq, source_start, source_end, item_index` up to
  `min(per_kind_limit, 32)`; a row is due only when
  `next_attempt_at_ms <= now_ms`; a failed delivery increments
  `attempt_count`, sets `next_attempt_at_ms = now + 1000 * 2^min(attempt, 6)`
  capped at 60000, and stores the error capped at 2000 characters; a failure
  in one kind does not stop other kinds; the first bookkeeping error is
  returned after the loop completes. `always` because these rules define
  which rows a pass touches.
- Guarantee: Drain scheduling and retry shape are unchanged by the transaction
  restructuring.
- Rationale: [`HISTORIAN_SIDE_CHANNEL_KINDS`][kinds], the [drain loop][drain],
  the [due query][load-due] with its `INDEXED BY` order index
  ([baseline.sql:531-535][idx-order]), and
  [`record_historian_side_channel_failure`][failure] fix these values.
- Fault/timing angle: An empty-drain shortcut that skips the leftover delete,
  or a delete-in-place that changes which rows count as pending for the next
  pass.
- Required faults and enabling state: Multiple rows per kind across two
  firings, an injected failure on one kind, and a `now_ms` before and after
  the computed `next_attempt_at_ms`.
- Reachability: default-production for the empty drain; explicit-config-only
  for populated drains, as above.
- Existing check: [`historian_side_channel_faults_are_isolated_and_retryable_per_kind`][t-faults]
  covers isolation and one retry with `now_ms = i64::MAX` (unaudited). None
  found for ordering across firings, for the per-kind limit, or for the
  backoff schedule values.
- Open questions: None.

### meta-json-preparation-scans-every-persisted-byte

- Type: safety
- Check: `always` - For the `meta` text handed to `json_content`, the stored
  bytes are either byte-identical to `serde_json::to_string(meta)` when no
  substitution occurred, or the serialization of the redacted tree; a
  duplicate object name at any depth refuses the write; every object key is
  bound-checked and scanned; a detected value under an identity or integrity
  key refuses the write; a protected key with a container value refuses; a
  protected scalar substitutes and records a detection; and the recorded scan
  for field `meta` carries the same detections as the walk observed. `always`
  because every committing pass runs this path.
- Guarantee: No byte reaches the `meta` column that the scanner did not walk,
  and the audit receipt matches the bytes stored.
- Rationale: `commit_transform` serializes `meta` and hands the text to
  `json_content` at [lib.rs:8306-8315][commit-meta]. The [comment][unique-doc]
  on [`parse_json_with_unique_names`][unique] states the security invariant:
  `serde_json::Value` keeps only the last duplicate name, so an earlier
  secret-bearing value would bypass `prepare_value` and persist when the
  [unchanged-input branch][clean-branch] returns the original bytes. That
  branch is why byte identity holds when clean and why duplicate refusal is
  load-bearing. [`validate_json_keys`][keys] scans keys,
  [`prepare_value`][prepare-value] applies the key-directed policy, and
  [`record_observed_scan`][record-scan] attaches the detections under action
  `substitute`. No reader compares `meta` bytes; every reader deserializes
  ([`load`][load], the [meta-select users][meta-select]), and the transform
  compares values ([`next_meta != loaded.meta`][value-compare]). The workspace
  `serde_json` has `raw_value` only ([Cargo.toml:45][serde-feat]), so a
  re-serialized `Value` orders keys alphabetically, unlike struct order.
- Fault/timing angle: A single-pass redaction that streams input and returns
  the original bytes for an unchanged prefix while a later duplicate name
  shadows an earlier value; or one that substitutes without recording the
  detection.
- Required faults and enabling state: `meta` text with duplicate names at top
  level and nested; a secret in a `BTreeMap` key such as
  `block_identity_by_mid`; a secret under an integrity-named field such as
  `tail_hygiene_baseline.content_signature`; a protected key holding an
  object; and a clean `meta` compared byte-for-byte with the stored column.
- Reachability: default-production - every `commit_transform`.
- Existing check: [`duplicate_json_object_names_are_refused`][t-dup],
  [`a_key_directed_substitution_records_its_own_detection`][t-keydir],
  [`a_protected_key_holding_a_container_is_refused`][t-container],
  [`preserved_json_identities_do_not_exempt_integrity_fields_credential_names_or_nested_values`][t-preserved],
  [`cache_state_redacts_payloads_preserves_existing_ids_and_rejects_integrity`][t-cache-redact]
  (all unaudited). None found for byte identity of a clean stored `meta`
  against `serde_json::to_string(meta)`, and none found for a secret inside a
  `BTreeMap` key of `ModuleMeta`. The canonical policy records are
  [preserved-identity-name-does-not-exempt-its-value][ms-preserved] and
  [refused-durable-write-leaves-no-row-and-no-receipt][ms-refused].
- Open questions:
  - Must a redacted `meta` keep today's alphabetical key order, or is any
    deserializable form acceptable? No reader depends on order. (needs human
    input)

### foreign-write-lands-between-pass-loads

- Type: reachability
- Check: `sometimes` - During a campaign, at least one pass observes a
  `row_version` in its post-commit read that differs from the `row_version`
  its transform committed, because another actor (historian publish, wrapup
  recut, or state sync) committed in between. `sometimes` rather than
  `reachable` because the branch lines execute on every Emergency95 pass while
  the interleaving that makes the first property meaningful may never occur.
- Guarantee: The campaign exercises the state that distinguishes
  one-load-per-pass from per-consumer loads.
- Rationale: The rerun logic at [lib.rs:8365-8384][floor-b] exists for this
  interleaving. Without it, a single-load design and the current design are
  indistinguishable.
- Fault/timing angle: The window between `commit_transform` and
  `prepare_historian_fire` or the floor check.
- Required faults and enabling state: A concurrent publish or recut committed
  through a second handle inside that window; the
  [`between_transform_and_prepare`][hook] test hook is the existing seam.
- Reachability: default-production for the code; the interleaving needs a
  concurrent historian task or an external writer.
- Existing check: [`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`][t-emergency]
  constructs the interleaving through the hook (unaudited).
- Open questions: None.

## Existing checks

| Check | Source condition | Status |
| --- | --- | --- |
| [`no_fire_reason_is_durable_change_gated_and_cleared_by_fire`][t-no-fire] | post-commit load feeds `record_no_fire` CAS; repeated reason leaves `row_version` unchanged | unaudited |
| [`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`][t-emergency] | publication between transform and prepare triggers rerun | unaudited |
| [`handler_delta_boundary_divergence_recut_retries_cas_without_stale_projection`][t-cas] | CAS conflict reruns `apply_once` with a fresh load | unaudited |
| [`transform_snapshot_resists_commit_between_state_and_overlay_reads`][t-snap-resist] | one read transaction pins `cache_state` and overlays | unaudited |
| [`transform_snapshot_keeps_row_version_and_overlays_from_one_commit`][t-snap-keeps] | snapshot row_version matches overlays from the same commit | unaudited |
| [`transform_cas_conflict_leaves_every_overlay_table_empty`][t-cas-empty] | CAS loser commits no overlay rows | unaudited |
| [`competing_pass_counter_survives_direct_primary_lifecycle_and_reopen`][t-counter] | CAS loser rejected; state survives reopen | unaudited |
| [`transform_reject_records_trace_without_advancing_row_version`][t-reject] | reject: `receive_count == 1`, `reject_count == 1`, row_version unchanged | unaudited |
| [`transform_success_records_received_and_completed_trace`][t-success] | success: received then completed timestamps | unaudited |
| [`repeated_rejects_increment_trace_and_overwrite_last_error`][t-repeat] | counters increment per pass; last error overwritten | unaudited |
| [`sequential_failing_passes_trace_every_reject_while_cache_state_stays_frozen`][t-frozen] | four rejects: `receive_count == 4`, row_version frozen | unaudited |
| [`status_and_health_surface_pass_trace_for_rejected_sessions`][t-status] | status and health expose `pass_trace` after a reject | unaudited |
| [`status_distinguishes_current_and_historical_divergence`][t-divergence] | `first_divergence` NULL after stable; `last_divergence` retained | unaudited |
| [`pass_trace_upserts_counts_and_caps_errors`][t-upserts] | upsert counters, 256-entry scheduler ring, 2000-char error cap | unaudited |
| [`scheduler_trace_records_every_pass_and_preserves_variable_arm_state`][t-sched] | one scheduler observation per accepted pass | unaudited |
| [`pass_trace_refuses_a_new_secret_session_and_keeps_tracing_a_stored_one`][t-secret] | trace writes refuse a new secret session, tolerate a stored one | unaudited |
| [`historian_side_channel_outbox_recovers_after_restart`][t-restart] | failed row redelivered once after reopen; pending count drops to 0 | unaudited |
| [`historian_side_channel_faults_are_isolated_and_retryable_per_kind`][t-faults] | one failed kind leaves other kinds delivered; retry succeeds | unaudited |
| [`publish_historian_chunk_cas_conflict_leaves_no_transcript_row`][t-publish-cas] | CAS loser enqueues no outbox rows | unaudited |
| [`truncate_compartments_for_revert_removes_anchored_events_and_crossing_ranges`][t-truncate] | revert deletes outbox rows for the session | unaudited |
| [`duplicate_json_object_names_are_refused`][t-dup] | duplicate names refused at top level and nested; message omits the value | unaudited |
| [`a_key_directed_substitution_records_its_own_detection`][t-keydir] | protected-key substitution records a synthetic detection | unaudited |
| [`a_protected_key_holding_a_container_is_refused`][t-container] | protected key with container value refuses | unaudited |
| [`preserved_json_identities_do_not_exempt_integrity_fields_credential_names_or_nested_values`][t-preserved] | identity preservation limited to structural scalar names | unaudited |
| [`cache_state_redacts_payloads_preserves_existing_ids_and_rejects_integrity`][t-cache-redact] | commit redacts core payload, preserves legacy ids, refuses integrity secret in meta | unaudited |
| [`cache_state_identity_decision_comes_from_the_write_transaction`][t-identity-tx] | new-versus-existing session decided inside the fenced transaction | unaudited |
| [`open_pins_full_synchronous`][t-sync] | `synchronous=FULL` pinned on open and re-pinned per fenced write | unaudited |

None found:

- Equivalence between a narrow `meta` scalar read and `MemoryStore::load`.
- `Handler::historian_active` reading the durable phase (only the in-memory
  branch is implied by firing tests).
- `first_divergence` NULL after a rejected pass.
- `receive_count` after an Emergency95 rerun that commits twice.
- A `pass_trace` write failure beside a successful cache commit.
- Crash between the outbox mark commit and the delete commit.
- Two drainers overlapping on one session.
- Outbox ordering across firings, the per-kind limit, or the backoff values.
- Byte identity of a clean stored `meta` against `serde_json::to_string`.
- A secret in a `BTreeMap` key of `ModuleMeta`.

Suspiciously quiet: the drain result and every trace result are discarded
with `let _ =` in the handler, so a regression in either surfaces only through
`session.status` or `health`, and only if someone reads them.

## Contract-versus-code disagreements

- Documented intent versus the proposed fold, not code. The
  [`trace_pass_received` doc][received-doc] says the write stays outside the
  fenced cache-state transaction "so the observability write never contends
  with or extends the pass commit", and the
  [`trace_pass_completed` doc][completed-doc] says a completion breadcrumb
  "cannot alter CAS semantics or hold the commit transaction open longer". The
  code matches both. A fold into `commit_transform` contradicts both
  statements and must rewrite them.
- Documented intent versus the proposed delete-in-place, not code. The
  [drain doc][drain-doc] says the acknowledgement row "is deleted only after
  that commit, so restart replay is idempotent". The code matches. Delete in
  the delivery transaction keeps replay idempotent by a different mechanism;
  the comment must change with it.
- [`record_no_fire`][no-fire-doc] carries the doc comment `/// Delete`, which
  describes nothing the function does; it commits `last_no_fire` under CAS.
  Stale documentation, no behavioral disagreement.
- The audit's item (b) names only `trace_pass_received`; the pass also runs
  [`trace_pass_completed`][completed-call] as a second fenced transaction on
  every pass and [`trace_pass_stable`][stable-call] as a third on stable
  passes. Folding one of three does not remove per-pass trace transactions.
- Sibling catalog anchors are stale against this HEAD. The memory-store
  record [preserved-identity-name-does-not-exempt-its-value][ms-preserved]
  cites `lib.rs:4027-4036` and `lib.rs:17258`; the preparation code is at
  [3109-3287][prepare-collecting] and the test is at [15166][t-preserved]
  here. Not edited by this lens.

## Anchors

Corrections to the supplied anchors: `MemoryStore::load` closes at 6223, not
6222. The daemon call at 4170 is inside `expand_transform_tail_delta`
(4151), not `lookup_full_projection_cache` (4271-4289, load at 4278).
`prepare_json_content_collecting` spans 3109-3287, not 3276-3290;
`parse_json_with_unique_names` spans 3291-3372 with its comment at 3289-3290.
`deliver_historian_side_channel` spans 10917-10973, not 10917-10947.
`persist_audit` spans 2291-2449, not 2291-2445. `trace_pass_received` spans
6485-6535. `commit_transform` spans 8172-8632; the meta preparation is at
8306-8315 and the in-commit `pass_trace` upsert at 8427-8494.

[ms-catalog]: ../../../memory-store/catalog.md
[sp-catalog]: ../../../shared-primitives/catalog.md
[s2]: ../../catalog.md#callback-batching-preserves-observation-boundaries
[r1]: ../../catalog.md#prepared-field-output-and-audit-policy-agree
[r2]: ../../catalog.md#preparation-refusal-does-not-append-audit-state
[r3]: ../../catalog.md#redaction-audit-does-not-depend-on-retained-payload
[ms-preserved]: ../../../memory-store/catalog.md#preserved-identity-name-does-not-exempt-its-value
[ms-refused]: ../../../memory-store/catalog.md#refused-durable-write-leaves-no-row-and-no-receipt

[load]: ../../../../../crates/memory-store/src/lib.rs#L6196-L6223
[full-select]: ../../../../../crates/memory-store/src/lib.rs#L4581-L4582
[meta-select]: ../../../../../crates/memory-store/src/lib.rs#L4579-L4580
[snapshot]: ../../../../../crates/daemon/src/transform.rs#L3006
[snapshot-impl]: ../../../../../crates/memory-store/src/lib.rs#L6228-L6358
[with-conn]: ../../../../../crates/storage/src/lib.rs#L305-L321
[fenced]: ../../../../../crates/storage/src/lib.rs#L369-L433
[pin-sync]: ../../../../../crates/storage/src/lib.rs#L1170-L1171
[claim-fence]: ../../../../../crates/storage/src/lib.rs#L1899-L1917
[pw-execute]: ../../../../../crates/memory-store/src/lib.rs#L2245-L2272
[audit]: ../../../../../crates/memory-store/src/lib.rs#L2291-L2449
[opaque-id]: ../../../../../crates/memory-store/src/lib.rs#L2470-L2473
[max-text]: ../../../../../crates/memory-store/src/lib.rs#L465

[epoch-read]: ../../../../../crates/daemon/src/lib.rs#L4278-L4296
[epoch-read-delta]: ../../../../../crates/daemon/src/lib.rs#L4158-L4187
[epoch-comment]: ../../../../../crates/daemon/src/lib.rs#L4174
[active]: ../../../../../crates/daemon/src/lib.rs#L4574-L4587
[prepare]: ../../../../../crates/daemon/src/lib.rs#L5001-L5074
[no-fire]: ../../../../../crates/daemon/src/lib.rs#L5457-L5470
[no-fire-doc]: ../../../../../crates/daemon/src/lib.rs#L5456
[no-models]: ../../../../../crates/daemon/src/lib.rs#L5196-L5203
[handler]: ../../../../../crates/daemon/src/lib.rs#L8122-L8384
[drain-call]: ../../../../../crates/daemon/src/lib.rs#L8131-L8135
[received-call]: ../../../../../crates/daemon/src/lib.rs#L8138
[run]: ../../../../../crates/daemon/src/lib.rs#L8146-L8200
[rejected-call]: ../../../../../crates/daemon/src/lib.rs#L8201-L8208
[floor-a]: ../../../../../crates/daemon/src/lib.rs#L8222-L8230
[rerun]: ../../../../../crates/daemon/src/lib.rs#L8271-L8278
[floor-b]: ../../../../../crates/daemon/src/lib.rs#L8365-L8384
[completed-call]: ../../../../../crates/daemon/src/lib.rs#L8443
[hook]: ../../../../../crates/daemon/src/lib.rs#L8231-L8239
[status-read]: ../../../../../crates/daemon/src/lib.rs#L6204-L6254
[age]: ../../../../../crates/daemon/src/lib.rs#L6245-L6254
[health-read]: ../../../../../crates/daemon/src/lib.rs#L7833-L7881
[cfg-compaction]: ../../../../../crates/daemon/src/config.rs#L121
[cfg-models]: ../../../../../crates/daemon/src/config.rs#L119
[cfg-user-mem]: ../../../../../crates/daemon/src/config.rs#L126
[plugin]: ../../../../../packages/opencode-plugin/src/hooks/context/command-handler.ts#L265-L268

[cas-retry]: ../../../../../crates/daemon/src/transform.rs#L1948-L1987
[stable-call]: ../../../../../crates/daemon/src/transform.rs#L1827-L1851
[descent-load]: ../../../../../crates/daemon/src/transform.rs#L2918
[descend]: ../../../../../crates/daemon/src/transform.rs#L2929-L2940
[value-compare]: ../../../../../crates/daemon/src/transform.rs#L3192
[truncate]: ../../../../../crates/daemon/src/transform.rs#L4097-L4103
[truncate-reload]: ../../../../../crates/daemon/src/transform.rs#L4115
[main-commit]: ../../../../../crates/daemon/src/transform.rs#L4947
[sched-test]: ../../../../../crates/daemon/src/transform.rs#L13575

[received]: ../../../../../crates/memory-store/src/lib.rs#L6485-L6535
[received-doc]: ../../../../../crates/memory-store/src/lib.rs#L6482-L6484
[flagged]: ../../../../../crates/memory-store/src/lib.rs#L6496-L6514
[stable]: ../../../../../crates/memory-store/src/lib.rs#L6540-L6632
[completed]: ../../../../../crates/memory-store/src/lib.rs#L6637-L6685
[completed-doc]: ../../../../../crates/memory-store/src/lib.rs#L6634-L6636
[rejected]: ../../../../../crates/memory-store/src/lib.rs#L6691-L6744
[load-trace]: ../../../../../crates/memory-store/src/lib.rs#L6747-L6787
[sched-history]: ../../../../../crates/memory-store/src/lib.rs#L6792-L6825
[passtrace-doc]: ../../../../../crates/memory-store/src/lib.rs#L767-L784
[pass-trace-sql]: ../../../../../crates/memory-store/baseline.sql#L83-L91

[commit]: ../../../../../crates/memory-store/src/lib.rs#L8172-L8632
[commit-meta]: ../../../../../crates/memory-store/src/lib.rs#L8306-L8315
[commit-trace]: ../../../../../crates/memory-store/src/lib.rs#L8427-L8494
[json-content]: ../../../../../crates/memory-store/src/lib.rs#L2116-L2126
[record-scan]: ../../../../../crates/memory-store/src/lib.rs#L2133-L2143
[prepare-field]: ../../../../../crates/memory-store/src/lib.rs#L2204-L2243
[policy]: ../../../../../crates/memory-store/src/lib.rs#L3077-L3098
[prepare-collecting]: ../../../../../crates/memory-store/src/lib.rs#L3109-L3287
[keys]: ../../../../../crates/memory-store/src/lib.rs#L3157-L3170
[prepare-value]: ../../../../../crates/memory-store/src/lib.rs#L3181-L3276
[clean-branch]: ../../../../../crates/memory-store/src/lib.rs#L3282-L3286
[unique-doc]: ../../../../../crates/memory-store/src/lib.rs#L3289-L3290
[unique]: ../../../../../crates/memory-store/src/lib.rs#L3291-L3372
[recomp]: ../../../../../crates/memory-store/src/lib.rs#L10057-L10150
[serde-feat]: ../../../../../Cargo.toml#L45

[meta-struct]: ../../../../../crates/memory-store/src/lib.rs#L1341
[meta-epoch]: ../../../../../crates/memory-store/src/lib.rs#L1387-L1388
[meta-pending]: ../../../../../crates/memory-store/src/lib.rs#L1395-L1396
[meta-historian]: ../../../../../crates/memory-store/src/lib.rs#L1503-L1504
[meta-floor]: ../../../../../crates/memory-store/src/lib.rs#L1508-L1509
[phase]: ../../../../../crates/memory-store/src/lib.rs#L537-L546
[hds-state]: ../../../../../crates/memory-store/src/lib.rs#L579-L582

[drain]: ../../../../../crates/memory-store/src/lib.rs#L10806-L10851
[drain-doc]: ../../../../../crates/memory-store/src/lib.rs#L10803-L10805
[status-sc]: ../../../../../crates/memory-store/src/lib.rs#L10853-L10879
[load-due]: ../../../../../crates/memory-store/src/lib.rs#L10881-L10915
[deliver]: ../../../../../crates/memory-store/src/lib.rs#L10917-L10973
[failure]: ../../../../../crates/memory-store/src/lib.rs#L10975-L11014
[delete-all]: ../../../../../crates/memory-store/src/lib.rs#L11016-L11029
[delete-one]: ../../../../../crates/memory-store/src/lib.rs#L11031-L11053
[publish]: ../../../../../crates/memory-store/src/lib.rs#L10559
[publish-drain]: ../../../../../crates/memory-store/src/lib.rs#L10762-L10771
[kinds]: ../../../../../crates/memory-store/src/lib.rs#L4513-L4516
[events-insert]: ../../../../../crates/memory-store/src/lib.rs#L13580-L13601
[enqueue]: ../../../../../crates/memory-store/src/lib.rs#L13713-L13737
[mark]: ../../../../../crates/memory-store/src/lib.rs#L13739-L13764
[primer-insert]: ../../../../../crates/memory-store/src/lib.rs#L13766-L13808
[obs-insert]: ../../../../../crates/memory-store/src/lib.rs#L13810-L13831
[outbox-sql]: ../../../../../crates/memory-store/baseline.sql#L489-L505
[idx-due]: ../../../../../crates/memory-store/baseline.sql#L507-L510
[idx-order]: ../../../../../crates/memory-store/baseline.sql#L531-L535

[t-no-fire]: ../../../../../crates/daemon/src/lib.rs#L36044
[t-emergency]: ../../../../../crates/daemon/src/lib.rs#L35276
[t-cas]: ../../../../../crates/daemon/src/lib.rs#L22951
[t-reject]: ../../../../../crates/daemon/src/lib.rs#L23798
[t-success]: ../../../../../crates/daemon/src/lib.rs#L23828
[t-repeat]: ../../../../../crates/daemon/src/lib.rs#L23844
[t-frozen]: ../../../../../crates/daemon/src/lib.rs#L23876
[t-status]: ../../../../../crates/daemon/src/lib.rs#L23908
[t-divergence]: ../../../../../crates/daemon/src/lib.rs#L31994
[t-sched]: ../../../../../crates/daemon/src/transform.rs#L13532
[t-counter]: ../../../../../crates/daemon/tests/boundary_counter_durability.rs#L12
[t-snap-resist]: ../../../../../crates/memory-store/src/lib.rs#L16453
[t-snap-keeps]: ../../../../../crates/memory-store/src/lib.rs#L16507
[t-cas-empty]: ../../../../../crates/memory-store/src/lib.rs#L16576
[t-upserts]: ../../../../../crates/memory-store/src/lib.rs#L17545
[t-secret]: ../../../../../crates/memory-store/src/lib.rs#L15376
[t-restart]: ../../../../../crates/memory-store/src/lib.rs#L18920
[t-faults]: ../../../../../crates/memory-store/src/lib.rs#L18704
[t-publish-cas]: ../../../../../crates/memory-store/src/lib.rs#L19067
[t-truncate]: ../../../../../crates/memory-store/src/lib.rs#L20529
[t-dup]: ../../../../../crates/memory-store/src/lib.rs#L15112
[t-keydir]: ../../../../../crates/memory-store/src/lib.rs#L15029
[t-container]: ../../../../../crates/memory-store/src/lib.rs#L15129
[t-preserved]: ../../../../../crates/memory-store/src/lib.rs#L15166
[t-identity-tx]: ../../../../../crates/memory-store/src/lib.rs#L15309
[t-cache-redact]: ../../../../../crates/memory-store/tests/production_redaction.rs#L606
[t-sync]: ../../../../../crates/storage/src/lib.rs#L3682
