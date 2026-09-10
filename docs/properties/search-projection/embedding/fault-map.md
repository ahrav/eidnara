# Embedding fault and situation map

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Sources and external scope: [catalog source register](catalog.md#source-register).
All proposed records are unexercised. Availability below describes construction
seams in source, not executed coverage.

## Fault classes and availability

| Class | Fault or enabling state | Available evidence and missing seam |
| --- | --- | --- |
| F1 | Full token sequence straddles the model window; provider count differs; verified artifact path is replaced. | Verified bundle bytes exist. Untruncated count API and independent model-token fixture are missing. |
| F2 | Input exceeds byte or token cap, or exact count/identity cannot be obtained. | Counting engine and byte-boundary tests exist. Product token/coverage gate is missing. |
| F3 | Duplicate pending discovery, full JobTable, or lost admission descriptor. | Local retained-key replay and capacity controls exist. Durable pending scanner and handoff trace are missing. |
| F4 | Delayed result after revision, model, fingerprint, dimension, epoch, hash, or tombstone change; same-revision remediation if the approved mapping uses that field. | Engine can park a call. Atomic current-identity completion predicate using authoritative mapped bytes is missing. |
| F5 | Termination or I/O failure at vector/completion commit boundaries. | Reuse the unaudited kernel CAS barrier/kill/reopen pattern at `crates/kernel/tests/cas_fault_injection.rs:924-989`, `:1046-1094`. Product vector store and commit hooks are still missing. |
| F6 | Fresh process after dispatch, expired/evicted local result, or retryable inference failure. | JobTable incarnation, failure injection, and the kernel CAS process-kill pattern exist. Durable restart driver and approved recovery window are missing. |
| F7 | Saturated backfill with an independently available query slot. | Gated native engine and FIFO trace exist. Product priority admission and its approved service envelope are missing. |
| F8 | GC selection races active references, newer identity, or late completion. | ResultLease provides a local lifetime seam. Durable identity GC/reference accounting is missing. |
| F9 | Cancellation/deadline during wait, count, inference, or persistence; another supervisor slice is due. | Kernel EvalBudget, RequestCtx cancellation, and ManualClock exist separately. Async query lane/EvalBudget integration is a shared prerequisite with RP2.7.U3; RP2.1.U3 integrates priority and embedding maintenance. |

The deterministic engine records calls before its blocking gate and increments
completed-text count afterward (`crates/host-runtime/tests/support/synapse.rs:99-122`).
Use that separation to observe physical work, not to infer tokenizer semantics.
Ignored external-runtime checks remain opt-in and are not run in this pass.

## Per-property map

Each marker below has `sometimes` semantics at campaign scope. It describes
independent preconditions and can fire on a correct implementation. Safety
checks run during faults. Recovery checks run after faults stop under approved L.
The central owner must declare the enabled acceptance scope. The shared
[acceptance-situation record](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed)
requires every declared enabled marker/scenario dimension across the three
maps. An optional or unreached inference path cannot pass embedding acceptance.

| Property | Required faults and enabling state | Independent marker |
| --- | --- | --- |
| [embedding-count-authority-is-untruncated](catalog.md#embedding-count-authority-is-untruncated) | F1; oracle token sequence exceeds the window while bytes fit; both tokenizer artifact identities are observed. | `rp21_embedding_full_sequence_straddles_window` |
| [embedding-input-is-rejected-before-inference](catalog.md#embedding-input-is-rejected-before-inference) | F2; invalid input is offered to a ready lane after a valid control input; startup call count has been recorded. | `rp21_embedding_invalid_input_offered_to_ready_lane` |
| [embedding-pending-drives-one-job-table](catalog.md#embedding-pending-drives-one-job-table) | F3; committed K is discovered twice, with admission response suppressed and capacity pressure in a separate case. | `rp21_embedding_pending_rediscovered_after_descriptor_loss` |
| [embedding-completion-is-identity-fenced](catalog.md#embedding-completion-is-identity-fenced) | F4; dispatch captures K, then an independent source/identity update commits before held completion is released. | `rp21_embedding_identity_changed_with_result_held` |
| [embedding-complete-requires-durable-vector](catalog.md#embedding-complete-requires-durable-vector) | F5; a validated result exists and the process is terminated at a recorded persistence boundary. | `rp21_embedding_process_killed_at_persistence_boundary` |
| [embedding-restart-retries-durable-pending](catalog.md#embedding-restart-retries-durable-pending) | F6; pre-crash durable Pending(K), fresh process/table, unchanged identity, and healthy recovery service window. | `rp21_embedding_restart_has_stable_pending_and_service` |
| [embedding-backfill-preserves-query-admission](catalog.md#embedding-backfill-preserves-query-admission) | F7; backfill fills its declared envelope before a valid query arrives with query occupancy below its cap. | `rp21_embedding_query_arrives_during_backfill_saturation` |
| [embedding-identity-gc-preserves-live-work](catalog.md#embedding-identity-gc-preserves-live-work) | F8; obsolete candidate selected while a result/reference is held; identity update or dispatch races its deletion phase. | `rp21_embedding_gc_candidate_has_concurrent_holder` |
| [embedding-supervisor-shares-budget-and-joins](catalog.md#embedding-supervisor-shares-budget-and-joins) | F9; cancellation/deadline is observed independently while native work is blocked and another slice is due. | `rp21_embedding_stop_occurs_with_native_work_held` |

### Completion-fence scenario dimensions

`rp21_embedding_identity_changed_with_result_held` covers independent changes
to revision, model, fingerprint, dimensions, epoch, input hash, and tombstone
state. Extend its input-hash dimension with same-revision operator remediation
when the approved input mapping depends on the changed field. This is not a
distinct marker: the independent premise is authoritative mapped bytes changing
while an old result is held, regardless of whether the revision changes.

`crates/kernel/src/envelope.rs:395-438` changes `domains.name` while retaining
the loaded object's revision. Domain names may be absent from the approved
embedding mapping. Central scope declaration must record applicability; when
applicable, observe actual authoritative mapped bytes before and after the
remediation. Do not rely on a stale projected hash or assume all K fields stay
unchanged. The
[projection remediation record](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)
owns source invalidation. This refinement mandates neither a new key/version
nor a broad erasure policy.

## Additional discriminating preconditions

These are separate constant marker names, not dynamically generated names.
They prevent a broad marker from hiding an omitted conjunct of a record.

| Marker | Required observation |
| --- | --- |
| `rp21_embedding_verified_path_replaced_after_load` | Verified bytes are retained and the corresponding artifact pathname is replaced before counting. |
| `rp21_embedding_model_name_changes_without_fingerprint_change` | Model name changes while fingerprint and vector dimension are deliberately held equal. |
| `rp21_embedding_shared_payload_has_distinct_occurrences` | Two occurrence IDs with identical payload bytes coexist during completion or GC. |
| `rp21_embedding_vector_commit_response_is_lost` | Storage commit is attempted and its response is suppressed before completion is attempted. |
| `rp21_embedding_retryable_failure_precedes_recovery` | A submitted attempt receives the injected retryable failure before the fault-free window starts. |
| `rp21_embedding_budget_crosses_between_stages` | The original D is crossed between two stage entry points while the operation still has work. |
| `rp21_embedding_gc_obsolete_identity_is_unreferenced` | An independently identified obsolete identity has no live reference before the bounded GC slice. |

No marker requires stale work to complete, a vector to be lost, admission to
starve, or resources to leak. An unfired marker is a workload gap or a changed
reachability premise to investigate, not evidence that the safety rule passed.

## Observation and effect accounting

- Keep an external trace of attempted dispatches, observed descriptors, ready
  pages, vector commit attempts, confirmed commits, and completion markers.
- Reconcile by K and occurrence identity. Retries are many attempts for one
  durable effect; raw equality between attempt count and vector count is wrong.
- A ready page and a successful SQL call are not durability witnesses. Reopen
  persistent state after termination and compare identity plus vector bytes.
- A wrong-shape test alone cannot establish stale-identity fencing. Independently
  vary each field while keeping all others valid.
- Observe live owners/permits before releasing the native gate. A returned
  cancellation response proves no physical completion by itself.
- Process-crash evidence does not prove power-loss durability. Storage-owner
  handoff must name the crash model before choosing physical fault machinery.
- The reusable CAS suite states that same evidence limit at
  `crates/kernel/tests/cas_fault_injection.rs:1-6`. Reuse its child barrier,
  kill/join, and reopen pattern; product hooks and oracle remain prerequisites.

## Bounded recovery prerequisites

RP2.9 owns the values for L. Before execution it must approve pending and batch
caps, retry attempts, fault-free recovery duration, query occupancy and waiting
limits, native service envelope, supervisor slice work, lease duration, and
cancellation observation bounds. Do not substitute existing Synapse defaults.

The restart episode fixes a finite pending set, leaves each target current, and
provides declared service opportunities after faults cease. The saturation
episode provides finite native-call service and query load within its declared
envelope. A permanent model failure, continued overload, or uncooperative
native call does not satisfy those recovery premises. Its non-success state
must remain explicit rather than appearing as a successful bound check.
Liveness checks apply per admitted episode, where admission means independently
constructed premises, not a successful implementation outcome. RP2.9-unapproved
bounds block acceptance; missing declared episodes fail situation coverage.
The async query lane and EvalBudget bridge remain shared with RP2.7.U3, with
priority integration in RP2.1.U3. Full query-route work is not reassigned here.

## Leverage ranking by cheapest valid oracle

1. Count authority and preflight: independent token fixtures plus the existing
   engine-call observer isolate the earliest irreversible work boundary.
2. Completion fencing: a held result and one-field current-identity mutations
   discriminate the missing compare-and-write without a whole-system campaign.
3. Pending ownership and query admission: reuse JobTable and the gated engine;
   add only the product handoff/service observation after implementation exists.
4. Supervisor ownership and GC: require physical-holder and durable-reference
   observation, not merely API outcomes or elapsed wall time.
5. Durability and restart: require an implemented storage protocol, actual
   termination, external boundary evidence, and reopen. Adapt the existing
   kernel CAS process-crash pattern rather than create a broad crash harness.
   In-memory fakes still cannot establish the stated claim.

These are routing priorities for `/testing:test-strategy`, not chosen test
forms. [Portfolio evaluation](portfolio-evaluation.md) records the central
analyst's findings and the local dispositions. Enabled acceptance scope and
open design prerequisites remain central-owner decisions.
