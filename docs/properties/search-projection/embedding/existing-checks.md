# Existing embedding checks

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
External scope and why-consulted sources: [source register](catalog.md#source-register).
All checks in this inventory are **unaudited** for this handoff. Source inspection
does not establish test adequacy, coverage, or a successful execution.

The inventory covers checks bearing on the original nine embedding deltas and the
existing mechanisms they reuse. The full Claude tokenizer and host protocol
inventories remain in their owning catalogs. Their historical run claims are
not refreshed by this document.

The bounded-dispatch update adds two catalog records after this pinned inventory.
Its checks are `project_scan_cursor_advances_across_more_than_two_wrong_scope_pages`,
`max_jobs_bounds_terminal_dispositions`,
`malformed_candidate_is_obsoleted_without_poisoning_valid_work`, and
`terminal_search_deadline_preserves_the_candidate_for_retry` in
`crates/daemon/tests/embedding_dispatch.rs`, plus
`eligibility_cardinality_mismatch_is_a_release_error` in the dispatcher module.

## Reused canonical inventories

- [Claude tokenizer checks](../../tokenizer/existing-checks.md): the seven
  tokenizer records remain the authority for provider accounting. They are
  not an oracle for the embedding model vocabulary.
- [Host U3 checks](../../host-runtime/discovered-at-u3/existing-checks.md): reuse
  the named Synapse records. Coordinates and reachability in that older
  inventory require the corrections in [the readback](_lenses/00-existing-coverage.md).
- [Scheduled Dreamer slot](../../daemon/handlers/catalog.md#scheduled-dreamer-slot-runs-once-through-lease-and-receipt):
  reuse the existing lease/receipt contract for its actual scheduled task.

## Durable corrections to reused evidence

These corrections are checked against the pinned source, not inherited from
old exercise claims. They apply to this handoff; the older catalogs are not
edited. Existing tests and guards remain unaudited.

| Older claim | Current evidence | Handoff interpretation |
| --- | --- | --- |
| The tokenizer has no production caller. | `crates/daemon/Cargo.toml:32` depends on tokenizer; the production cache miss path calls `tokenizer::estimate_tokens` at `crates/daemon/src/token_cache.rs:127-141`. | Claude counting has production callers. Its vocabulary still cannot certify EmbedTokens. |
| Synapse is composed only in tests/examples. | `crates/daemon/src/bin/eidnara_host/serve.rs:1097-1127` constructs and composes Synapse. `:1027-1065` selects configured verified artifacts or an unsupported/disabled fallback. | Composition/fallback is default-production. Certified inference and live jobs are explicit-config-only, not blanket test-only. |
| Completed jobs always evict by oldest completion time. | `crates/host-runtime/src/synapse/jobs.rs:156-158` ranks `(last_polled_at.or(completed_at), completed_at)`; `:773-797` and `:840-851` use that rank for eligible victims. | Unpolled jobs can exhibit oldest-completion order. Do not generalize that test to polled jobs or ignore victim eligibility. |
| ORT tests pass by returning early when the library is absent. | `crates/host-runtime/tests/synapse_bundle.rs:579-581` explicitly ignores the certified-vector test pending the documented opt-in. | Source presence and optional execution are not acceptance witnesses. |

The [central evaluation](portfolio-evaluation.md#four-lens-evaluation) retains
these corrections outside lens working material. The shared
[acceptance-situation record](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed)
requires all declared enabled dimensions, not an optional inference path that
the campaign never reaches.

## Reusable kernel crash pattern

| Existing surface | Semantics and limitation | Status |
| --- | --- | --- |
| `crates/kernel/tests/cas_fault_injection.rs:1-6` | The suite is feature-gated by `test-support`; its documented scope is process crash and injected errors, excluding power loss, torn writes, and cold-device persistence. | unaudited |
| `crash_barrier`, `crates/kernel/tests/cas_fault_injection.rs:1046-1054` | Emits and flushes the named barrier, then parks the child. | unaudited |
| `run_crash_child`, `crates/kernel/tests/cas_fault_injection.rs:1056-1083` | Reexecutes the child role and waits for its barrier through stdout. This is a suite-local pattern, not a generic exported embedding harness. | unaudited |
| `ChildGuard::drop`, `crates/kernel/tests/cas_fault_injection.rs:1085-1094` | Kills a live child and waits for process exit. | unaudited |
| `crash_windows_recover_idempotently_and_match_no_crash_execution`, `crates/kernel/tests/cas_fault_injection.rs:924-989` | Uses child termination and repeated reopen/recovery to compare CAS state with an uninterrupted run. | unaudited |

Reuse this barrier/kill/reopen pattern for the embedding handoff after product
storage and hooks exist. The CAS-specific hooks and oracle do not exercise
search.sqlite vector completion. The missing work is the product store,
commit-boundary hooks, and per-K oracle, not a new broad crash framework.

## Runtime guards and assertions

Paths in this table are repository-relative. Error text identifies the checked
branch; it is not a proposed RP2.1 error vocabulary.

| Check location | Semantics and message or observation | Status |
| --- | --- | --- |
| `crates/host-runtime/src/synapse/bundle.rs:198-205` | The parsed manifest bytes must match the optional outer digest; failure says `bundle manifest does not match the digest its generation committed`. | unaudited |
| `crates/host-runtime/src/synapse/bundle.rs:259-278`, `:782-795` | Read verified tokenizer artifacts and compare canonical fingerprint; failures report artifact hash mismatch or canonical embedding-space mismatch. | unaudited |
| `crates/host-runtime/src/synapse/bundle.rs:836-869` | `model_max_length` must be integral and at least manifest max_tokens; pad token must exist. This validates configuration, not full input length. | unaudited |
| `crates/host-runtime/src/synapse/bundle.rs:447-619` | Serving-limit validation covers result capacity, wire pages, query permits, request scratch, queue metadata, and retained metadata. It does not approve RP2.9 values. | unaudited |
| `crates/host-runtime/src/synapse/inference.rs:222-240` | Dimension, finiteness, and unit-norm validation reports `vector contains a non-finite component` or the corresponding dimension/norm error. | unaudited |
| `crates/host-runtime/src/synapse/inference.rs:344-385` | Empty or zero-token input is rejected; result count and vector shape are checked. No untruncated over-token rejection is present. | unaudited |
| `crates/host-runtime/src/synapse/inference.rs:466-470` | `debug_assert_eq!` checks certification rows with message `load_bundle admits only certifiable corpora`; it is not an input-token guard. | unaudited |
| `crates/host-runtime/src/synapse/mod.rs:344-399` | In-process embedding checks item count, per-text bytes, aggregate bytes, lane state, CPU admission, and output shape. | unaudited |
| `crates/host-runtime/src/synapse/protocol.rs:781-887` | Model/fingerprint/epoch constraints, bytes, exact input hash, and duplicate IDs are validated before batch dispatch. | unaudited |
| `crates/host-runtime/src/synapse/jobs.rs:173-185` | Debug assertions check sufficient queued/result bytes before release; release uses saturating subtraction. | unaudited |
| `crates/host-runtime/src/synapse/jobs.rs:387-489` | Closed/full/result-too-large/key-conflict outcomes gate admission; retained equal payloads reuse the job except retryable failure. | unaudited |
| `crates/host-runtime/src/synapse/jobs.rs:491-535`, `:568-594` | Only queued work starts; completed work resists late publication; count/dimension mismatch fails the job. No source revision is compared. | unaudited |
| `crates/host-runtime/src/synapse/jobs.rs:597-658` | Incarnation/key/cursor validation and page lease preserve local result identity and lifetime. | unaudited |
| `crates/host-runtime/src/synapse/mod.rs:647-722`, `:725-767` | Query deadline gates admission, native start, result receipt, and output construction. Worker owns query permit and text charge through the native call. | unaudited |
| `crates/host-runtime/src/synapse/mod.rs:1148-1161` | Shutdown closes admission, joins tracked work, clears jobs, and waits for an in-process CPU holder. There is no finite native-call deadline here. | unaudited |
| `crates/kernel/src/applicability/checkout.rs:170-203` | Cancellation is sticky; crossing the deadline raises the shared interrupt and `check` returns BudgetExhausted. | unaudited |
| `crates/kernel/src/open.rs:1379-1409` | SQLite progress calls the shared stop predicate, but the installer is crate-private. | unaudited |
| `crates/daemon/src/dreamer_scheduler.rs:163-220` | Cancellation can drop an in-flight tick; failed store work waits the idle poll. This is not an embedding sweep. | unaudited |

## Claim-bearing tests at current coordinates

All names below identify existing source tests. None is executed in this pass.
Closely related checks are grouped by file; each named check is unaudited.

### Artifact and counting prerequisites

| Test and location | Assertion family | Status |
| --- | --- | --- |
| `fingerprint_binds_initializer_names_to_their_hashes`, `crates/host-runtime/src/synapse/bundle.rs:1059` | Swapping initializer names changes the fingerprint. | unaudited |
| `every_artifact_hash_and_embedding_scalar_participates_in_the_fingerprint`, `crates/host-runtime/src/synapse/bundle.rs:1086` | Perturbs each fingerprint input and separately checks excluded fields, including model name. | unaudited |
| `tokenizer_ceiling_may_exceed_the_manifest_limit`, `crates/host-runtime/src/synapse/bundle.rs:979` | Higher config ceiling is allowed; lower ceiling is rejected. | unaudited |
| `fractional_tokenizer_ceilings_are_rejected`, `crates/host-runtime/src/synapse/bundle.rs:994` | Rejects fractional ceilings while accepting an integral numeric value. | unaudited |
| `one_bit_changes_to_each_artifact_disable_the_lane`, `crates/host-runtime/tests/synapse_bundle.rs:276` | Artifact corruption disables loading. | unaudited |
| `a_stale_fingerprint_disables_the_lane`, `crates/host-runtime/tests/synapse_bundle.rs:352` | Rejects a stale manifest fingerprint. | unaudited |
| `the_committed_fixture_carries_its_canonical_fingerprint`, `crates/host-runtime/tests/synapse_bundle.rs:369` | Pins fixture fingerprint. | unaudited |
| `a_bundle_manifest_outside_the_committed_digest_does_not_load`, `crates/host-runtime/tests/synapse_bundle.rs:397` | Binds the inner bundle to the outer generation digest. | unaudited |
| `certified_bundle_loads_and_serves_expected_vectors`, `crates/host-runtime/tests/synapse_bundle.rs:579-581` | Real-runtime certification and expected vectors; explicitly ignored without the documented opt-in. | unaudited |
| `production_bundle_from_environment_certifies_offline`, `crates/host-runtime/tests/synapse_bundle.rs:649` | Production-bundle certification with external artifacts; not an exact-count API check. | unaudited |

### Admission, replay, and result identity

| Test and location | Assertion family | Status |
| --- | --- | --- |
| `embed_query_rejects_every_constraint_violation`, `crates/host-runtime/tests/synapse_protocol.rs:642` | Query constraints and no inference on rejection. | unaudited |
| `embed_batch_validation_creates_no_job_and_no_inference`, `crates/host-runtime/tests/synapse_protocol.rs:869` | Batch constraint rejection before job creation and engine calls. | unaudited |
| `exact_boundary_batches_are_accepted`, `crates/host-runtime/tests/synapse_protocol.rs:954` | Existing byte/item boundaries are inclusive. | unaudited |
| `equal_replays_reuse_one_job_and_one_inference`, `crates/host-runtime/tests/synapse_protocol.rs:990` | Retained equal batch requests reuse process-local work. | unaudited |
| `unknown_and_foreign_jobs_are_module_restarted`, `crates/host-runtime/tests/synapse_protocol.rs:1284` | Unknown/foreign job identifiers are rejected. | unaudited |
| `wrong_request_key_for_a_live_job_is_a_schema_violation`, `crates/host-runtime/tests/synapse_protocol.rs:1303` | A live job cannot be polled with another request key. | unaudited |
| `admission_count_boundary_is_exact_and_never_evicts_live_work`, `crates/host-runtime/tests/synapse_jobs.rs:64` | Count pressure does not evict running or queued work. | unaudited |
| `queued_byte_boundary_is_exact_and_releases_on_completion`, `crates/host-runtime/tests/synapse_jobs.rs:152` | Input-byte admission and release boundary. | unaudited |
| `completed_jobs_evict_oldest_first_under_count_pressure`, `crates/host-runtime/tests/synapse_jobs.rs:200` | Eviction ordering in the constructed unpolled case. | unaudited |
| `expired_jobs_return_module_restarted`, `crates/host-runtime/tests/synapse_jobs.rs:258` | Process-local retention expiry, not durable restart recovery. | unaudited |
| `jobs_survive_route_loss_and_serve_a_fresh_route`, `crates/host-runtime/tests/synapse_jobs.rs:297` | Route loss keeps a local job available to another route. | unaudited |
| `failed_jobs_report_their_stored_error`, `crates/host-runtime/tests/synapse_jobs.rs:436` | Stored job failure is visible to polling. | unaudited |
| `a_vector_shape_that_disagrees_with_the_job_fails_only_that_job`, `crates/host-runtime/tests/synapse_jobs.rs:607` | Wrong local shape is rejected. | unaudited |
| `an_identical_retry_replaces_a_failed_job`, `crates/host-runtime/src/synapse/jobs.rs:1123` | Retryable failures differ from permanently retained failures. | unaudited |
| `a_late_publish_ready_leaves_a_completed_job_unchanged`, `crates/host-runtime/src/synapse/jobs.rs:1217` | Late local publication does not mutate completed state. | unaudited |

### Resource lifetime and query scheduling

| Test and location | Assertion family | Status |
| --- | --- | --- |
| `a_charged_job_transfers_shrinks_and_releases_exact_permits`, `crates/host-runtime/src/synapse/jobs.rs:921` | Charges follow admitted local work. | unaudited |
| `non_admitted_outcomes_leave_the_candidate_charge_with_the_caller`, `crates/host-runtime/src/synapse/jobs.rs:964` | Failed admission retains caller custody. | unaudited |
| `failure_eviction_and_expiry_release_their_charges`, `crates/host-runtime/src/synapse/jobs.rs:1039` | Terminal local cleanup releases charges. | unaudited |
| `sweep_releases_expired_charges_without_a_request_path`, `crates/host-runtime/src/synapse/jobs.rs:1092` | Idle sweep releases retained charges. | unaudited |
| `admission_reserves_result_capacity_before_inference_allocates_it`, `crates/host-runtime/src/synapse/jobs.rs:1334` | Result capacity precedes inference allocation. | unaudited |
| `a_page_leased_result_is_not_evicted_while_its_page_is_served`, `crates/host-runtime/src/synapse/jobs.rs:1382` | A served page keeps result capacity live. | unaudited |
| `bounded_query_waiters_are_fifo_and_reject_bound_plus_one`, `crates/host-runtime/tests/synapse_protocol.rs:67` | Query waiter capacity and FIFO. | unaudited |
| `expired_waiter_releases_its_slot_without_engine_work`, `crates/host-runtime/tests/synapse_protocol.rs:112` | Expired queued query does not infer. | unaudited |
| `mixed_batch_and_query_waiters_share_fifo_cpu_without_starvation`, `crates/host-runtime/tests/synapse_protocol.rs:186` | Asserts query, query, batch, query order. This is FIFO evidence, not priority. | unaudited |
| `shutdown_cancels_waiters_but_drains_started_query`, `crates/host-runtime/tests/synapse_protocol.rs:236` | Started native work delays shutdown; queued query is cancelled. | unaudited |
| `route_loss_drops_queued_query_without_engine_work_and_releases_slot`, `crates/host-runtime/tests/synapse_protocol.rs:312` | Queued query cancellation on route loss. | unaudited |
| `boundary_waiters_with_maximal_texts_are_all_admitted`, `crates/host-runtime/tests/synapse_protocol.rs:416` | Ignored boundary scenario; do not count it as exercised. | unaudited |
| `queued_query_wait_is_bounded_by_its_deadline`, `crates/host-runtime/tests/synapse_jobs.rs:389` | Existing query deadline during wait. | unaudited |
| `shutdown_with_queued_running_and_retained_jobs_is_graceful`, `crates/host-runtime/tests/synapse_jobs.rs:499` | Mixed local job states at shutdown. | unaudited |
| `embed_blocking_shares_the_cpu_permit_and_reports_a_held_lane`, `crates/host-runtime/src/synapse/mod.rs:1308` | Synchronous and routed calls share CPU admission. | unaudited |
| `shutdown_disables_the_lane_for_late_callers`, `crates/host-runtime/src/synapse/mod.rs:1338` | Late callers cannot restart a shut-down lane. | unaudited |

### Shared scheduler checks

`crates/daemon/src/dreamer_scheduler.rs` contains the following adjacent checks.
They concern review-user-memories scheduling, not embedding backfill.

| Test | Line | Assertion family | Status |
| --- | --- | --- | --- |
| `missed_slots_are_not_back_filled_and_a_backlog_drains_oldest_first` | 707 | Due-slot ordering and skipped historical slots. | unaudited |
| `each_slot_is_leased_at_the_instant_it_is_acquired` | 753 | Lease time is sampled at acquisition. | unaudited |
| `a_failed_project_lookup_defers_the_tick_and_keeps_the_pending_slot` | 859 | Read failure preserves scheduling work. | unaudited |
| `a_failed_lease_acquisition_retains_the_slot_for_the_next_tick` | 897 | Acquisition failure retains the due slot. | unaudited |
| `a_failed_lease_completion_retains_the_slot_and_the_retry_records_it` | 935 | Completion failure is retryable through retained state. | unaudited |
| `a_predecessor_claim_is_recovered_under_its_own_slot_by_the_next_generation` | 1009 | Successor recovers the predecessor slot. | unaudited |
| `an_expired_predecessor_lease_yields_a_fresh_claim_a_live_one_is_rebound` | 1100 | Expiry and live-claim recovery differ. | unaudited |
| `run_stops_mid_tick_at_cancellation_without_leasing_the_next_slot` | 1180 | Stop during one run does not lease the next slot. | unaudited |
| `run_stops_at_cancellation` | 1231 | Parked loop stops. | unaudited |
| `run_waits_the_idle_poll_after_a_deferred_tick_or_a_retained_slot` | 1257 | Retained work does not create a tight retry loop. | unaudited |

## Explicit none found

At the pinned revision, no product check was found for these proposed seams.
This historical list does not override the bounded-dispatch additions above:

1. Untruncated EmbedTokens count from the certified tokenizer bytes.
2. Compile-time or runtime separation of EmbedTokens and ClaudeTokens.
3. Exact over-token rejection while input bytes remain within the byte cap.
4. Durable pending-to-JobTable dispatch, including lost admission response.
5. Current revision/model/fingerprint/dimension/hash completion transaction.
   This includes same-revision remediation only for approved mappings that
   depend on the remediated field; authoritative current bytes must be checked.
6. Reopened vector durability before a product completion marker.
7. Process restart retry from search pending rows within approved bounds.
8. Query-priority admission under product backfill saturation.
9. Durable identity GC racing pending work and delayed results.
10. One shared EvalBudget across embedding, SQLite progress, and dense work.

No RP2.1 assertions, crash failpoints, situation markers, or approved numeric
campaign limits are found for these seams. None is silently credited to an
adjacent host or kernel CAS test.

## Suspiciously quiet areas

- An exact hash of all input bytes coexists with truncated model input by the
  wire contract. Hash checks cannot detect that semantic omission.
- Current fingerprint code excludes model name. A fingerprint-only completion
  check misses the plan's explicit model-identity requirement.
- Current JobTable deduplication is retention- and incarnation-scoped. Pure
  inference may repeat after restart without violating its existing contract.
- A blocked native call can outlive logical cancellation. Bounded reporting
  and physical draining need different observations.
- Old inventory labels cannot establish current reachability or exercise. The
  durable corrections above record concrete drift without editing another catalog.
- `crates/kernel/src/envelope.rs:395-438` remediates `domains.name` without
  changing the loaded object's revision. Whether any embedding input uses
  that field is undecided. K already carries an input hash, so the relevant gap
  is checking authoritative current bytes rather than a stale projected hash.
