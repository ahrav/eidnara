# RP2.1 required witness matrix

This document freezes the nonempty required witness matrix `R` for the RP2.1
specification ([#347](https://github.com/ahrav/eidnara/issues/347)) as ticket
P1 ([#352](https://github.com/ahrav/eidnara/issues/352)) requires. It maps the
parent's twelve acceptance criteria (AC1-AC12) and nine testing seams (T1-T9)
onto the 63 marker definitions owned by the three fault maps. It renames and
redefines nothing: every marker keeps its definition site, and the exact
witness condition remains the text in that map.

The machine-readable form is
[`crates/kernel/tests/fixtures/rp2-1/witness-matrix.json`](../../../crates/kernel/tests/fixtures/rp2-1/witness-matrix.json).
The kernel test `rp21_construction_inputs` extracts the marker definitions from
the three fault maps and checks that the fixture lists exactly those markers,
once each, in definition order, with the aggregate marker recorded as the
result; that every AC and seam owns at least one required cell; that the gate
cells enumerate every hook, entry point, and evidence state named in the
[construction contracts](construction-contracts.md); and that every
required-cell row, both coverage tables, and the witness-record-field bullets in
this document are byte-for-byte the rendering of the fixture. A disagreement
between the fixture, the fault maps, and those parts of this document fails
that test. The prose sections and the definition-sites table below are read by
people, not by the test.

## Definition sites

| Map | Definition section | Markers |
| --- | --- | --- |
| [Export and recovery](export-recovery/fault-map.md) | `## Independent situation markers` | 23 |
| [Projection and coverage](projection-coverage/fault-map.md) | `## Per-record precondition markers` | 24 |
| [Embedding](embedding/fault-map.md) | `## Per-property map` and `## Additional discriminating preconditions` | 16 |

The projection map consumes `rp21_ack_local_commit_interrupted` and
`rp21_ack_response_lost` from the export map without redefining them. The
matrix files each marker under its defining map.

## How to read a cell

Each required cell is one `(marker, scenario_id)` pair. Marker names are
constant. Scenario identifiers are predeclared and bounded; a campaign never
appends an identifier to a marker name. A cell is satisfied by a witness
record that identifies the run and the observed event, not by an attempted
workload, a reached callback, or a passing safety assertion.

- **Independent oracle** names the evidence that decides the cell without
  consulting projector, exporter, or job-table output.
- **Negative control** names the wrong implementation that the cell must
  reject; if the control passes, the cell is not discriminating.
- **Integration observation** names the real-store or real-daemon boundary
  the witness is taken at.
- **Built by** names the RP2.1 tickets that construct the product path. The
  coordinator, not those tickets, owns witness closure (AC11) through final
  RP2.9 acceptance.

Cell kinds:

- **required**: the marker must fire under every listed scenario.
- **conditional applicability**: the record's premise depends on the approved
  source mapping. Under the construction contracts the approved mapping
  excludes `domains.name`, so the premise of
  `rp21_projection_remediation_without_revision_change` is absent from scope.
  The cell is satisfied by the recorded applicability decision plus the
  name-only control that reconstructs mapped input bytes before and after a
  remediation and finds them equal. It is never satisfied by a fabricated
  passing witness, and a mapping change reopens the cell.

Gate cells: the three gate markers cover missing, failed, unsupported, and
inapplicable evidence for every hook at every entry point. The valid supported
activation control is required once the U4h gate exists; the fixture records
it under `gate_controls` as a control beside the gate cells, not as a marker
cell, because a marker records preconditions and never a successful activation
of disabled work.

## Required cells

### Export and recovery map

| Marker | Property | AC | Seams | Scenario cells | Independent oracle | Negative control | Integration observation | Built by |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `rp21_export_writes_between_pages` | [rp21-export-fixed-s-exactly-once](export-recovery/catalog.md#rp21-export-fixed-s-exactly-once) | AC1 | T4 | `insert_between_pages`, `revise_between_pages`, `delete_between_pages` | Fixture ledger E(S) built from seeded canonical operations before export; every key appears once with its S bytes. | Oracle derived from projector output, or a page that reflects a post-S mutation, must fail. | U1f export over a real kernel store while U1a-visible commits land between page requests. | #360 |
| `rp21_export_operator_remediation_during_snapshot` | [rp21-export-fixed-s-exactly-once](export-recovery/catalog.md#rp21-export-fixed-s-exactly-once) | AC1, AC10 | T4 | `name_only_remediation_after_s` | Independent reconstruction of mapped input bytes before and after the remediation under CC7; the fixture records mapping_consumes_domain_name=false. | A mapping that invents a byte dependency on domains.name, or a page whose bytes change without a source revision change, must fail. | Real operator_remediation commit after S while an export attempt is active and before T is fixed. | #358, #360 |
| `rp21_export_prune_with_fence` | [rp21-export-retention-fence-covers-read](export-recovery/catalog.md#rp21-export-retention-fence-covers-read) | AC1 | T7 | `prune_attempt_under_fence` | Consumer registration row and retained outbox history read independently of the exporter. | A prune that removes history below a registered fence, or an export that completes after such a prune, must fail. | U1d capture hold plus real prune_outbox attempt on the same store. | #358, #359 |
| `rp21_export_retention_lost_mid_build` | [rp21-export-retention-fence-covers-read](export-recovery/catalog.md#rp21-export-retention-fence-covers-read) | AC1 | T7, T4 | `fence_witness_invalidated`, `source_bytes_removed`, `history_pruned_below_s` | Harness-recorded invalidation event and candidate state before the next export, catch-up, or publication decision. | Publication after source or history loss, or cleanup errors swallowed, must fail. | U1d/U1e validity checks against a real store with GC and purge degradation injected. | #358, #359, #360 |
| `rp21_export_oversize_row_present` | [rp21-export-predecode-bounds](export-recovery/catalog.md#rp21-export-predecode-bounds) | AC1 | T6 | `oversize_first_row`, `oversize_later_row` | Independently measured encoded size of the fixture row against the declared standalone cap, observed before decoder entry. | A skipped row, a truncated row, or a decoder entry before admission must fail. | U1f page request whose next key is the oversized row. | #360 |
| `rp21_export_page_budget_edge` | [rp21-export-predecode-bounds](export-recovery/catalog.md#rp21-export-predecode-bounds) | AC1 | T6 | `row_budget_edge`, `byte_budget_edge` | Independent row and byte accounting of the page; the deferred row appears intact exactly once on the next page. | A row split across pages, dropped, or duplicated must fail. | U1f paging with admitted input and one fitting-alone row. | #360 |
| `rp21_catchup_split_commit` | [rp21-catchup-complete-commit-prefix](export-recovery/catalog.md#rp21-catchup-complete-commit-prefix) | AC2 | T4 | `multi_ordinal_over_batch_cut` | Complete canonical transaction ledger with every ordinal of the commit. | Progress advanced after a partial ordinal set must fail. | U1a reader returning only complete commits over a real commit_log. | #355, #362 |
| `rp21_catchup_published_retained` | [rp21-catchup-complete-commit-prefix](export-recovery/catalog.md#rp21-catchup-complete-commit-prefix) | AC2 | T4 | `published_above_checkpoint` | Retained change_event inventory compared with outbox rows regardless of published_at. | A reader that omits published rows, or reports them as an empty commit, must fail. | U1a read after another publisher marks rows published. | #355 |
| `rp21_catchup_empty_commit` | [rp21-catchup-complete-commit-prefix](export-recovery/catalog.md#rp21-catchup-complete-commit-prefix) | AC2 | T4 | `empty_commit_in_range` | commit_log entry with zero outbox rows distinguished from missing data by the retained inventory. | An empty commit reported as end-of-stream or missing data must fail. | U1a read across an empty commit inside (after_commit, through_commit]. | #355 |
| `rp21_ack_local_commit_interrupted` | [rp21-ack-follows-local-release](export-recovery/catalog.md#rp21-ack-follows-local-release) | AC2 | T5 | `kill_after_local_commit_before_ack` | Child stdout barrier after local COMMIT, external kill, reopened search state and kernel checkpoint read independently. | Ack observed before the local COMMIT barrier, or reopened checkpoint ahead of durable rows, must fail. | U2c ack path under the shared CAS crash pattern. | #363 |
| `rp21_ack_writer_contended` | [rp21-ack-follows-local-release](export-recovery/catalog.md#rp21-ack-follows-local-release) | AC2 | T5 | `writer_held_at_ack` | Ownership event order: local transaction released before kernel writer acquisition. | Both transactions held at once, or local rows visible only after ack, must fail. | U2c ack while another operation holds the kernel writer. | #363 |
| `rp21_ack_response_lost` | [rp21-ack-follows-local-release](export-recovery/catalog.md#rp21-ack-follows-local-release) | AC2 | T5 | `ack_commit_response_suppressed` | Durable kernel checkpoint read back after the suppressed response; per-identity attempt and effect counts. | Treating the lost response as rollback, or re-acking beyond the paired durable prefix, must fail. | U2c replay after an ack whose response is lost. | #363 |
| `rp21_replacement_switch_interrupted` | [rp21-replacement-selects-complete-compatible-state](export-recovery/catalog.md#rp21-replacement-selects-complete-compatible-state) | AC6 | T5 | `selector_stage_written`, `selector_verify_complete`, `selector_selection_durable`, `selector_old_release_pending` | One selected identity per read, canonical O(T), compatible tuple, and consumer-release order read after reopen. | A read that observes mixed or partial state, or old consumers released before durable selection, must fail. | U5e selection interrupted at each declared boundary with a concurrent reader. | #385, #386 |
| `rp21_rebuild_missing_db_after_prune` | [rp21-rebuild-after-pruning-converges](export-recovery/catalog.md#rp21-rebuild-after-pruning-converges) | AC7 | T9 | `missing_db_after_prune` | Independent reads prove pre-S history pruned, local state removed, retained canonical source nonempty; canonical O(T) at a stable T. | Rebuild that never reaches Current, or that reaches Current with a moved T, must fail. | U5h rebuild episode after confirmed pruning. | #388 |
| `rp21_rebuild_quiet_window_admitted` | [rp21-rebuild-after-pruning-converges](export-recovery/catalog.md#rp21-rebuild-after-pruning-converges) | AC7 | T9 | `quiet_window_admitted` | Admission record with fixed finite T, retained inputs within approved caps, dependencies available, injection stopped. | Admission without stopped injection or without a fixed T must fail. | U5h episode admission after a fault or load phase. | #388 |
| `rp21_healthy_lagged_target_admitted` | [rp21-catchup-and-authorized-recovery-converge](export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge) | AC7 | T9 | `healthy_backlog_admitted` | Admission record with local prefix below finite T, healthy stores, retained inputs, all gate acceptances, approved caps. | A retry that moves T or resets the clock, or safety without progress, must fail. | U5d/U5h ordinary CatchingUp episode. | #384, #388 |
| `rp21_authorized_recovery_current_observed` | [rp21-catchup-and-authorized-recovery-converge](export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge) | AC7, AC9 | T9 | `authorized_recovery_current` | Durable recovery authorization record, gate acceptances, and an independent Current observation through the episode's T. | Recovery without explicit authorization, or Current claimed by a no-op, must fail. | U5h Disabled-to-Current recovery episode on a real store. | #388 |
| `rp21_disable_pending_consumer` | [rp21-disable-preserves-consumer-obligations](export-recovery/catalog.md#rp21-disable-preserves-consumer-obligations) | AC9 | T7 | `disable_lagging` | Consumer rows and kernel pre-operation tip read independently; ConsumerPending outcome recorded. | Silent deregistration of a lagging consumer must fail. | U5g disable request with checkpoint below tip. | #387 |
| `rp21_disable_last_consumer` | [rp21-disable-preserves-consumer-obligations](export-recovery/catalog.md#rp21-disable-preserves-consumer-obligations) | AC9 | T7 | `deregister_last` | Registered-consumer census and gated-read outcome NoRequiredConsumer. | Pruning with no consumer, or gated reads served without a consumer, must fail. | U5g deregistration of the only caught-up consumer. | #387 |
| `rp21_abandon_request_with_barrier` | [rp21-disable-preserves-consumer-obligations](export-recovery/catalog.md#rp21-disable-preserves-consumer-obligations) | AC9 | T7 | `abandon_with_barrier` | Audit record ConsumerAbandonment and the incomplete deletion barrier row. | Abandonment without an audit record, or a barrier dropped by abandonment, must fail. | U5g authorized abandonment naming a barrier-recorded consumer. | #387 |
| `rp21_recovery_stale_authority` | [rp21-recovery-preserves-canonical-authority](export-recovery/catalog.md#rp21-recovery-preserves-canonical-authority) | AC6 | T7 | `stale_grant_invalidated`, `stale_grant_revised` | Independent canonical mutation ledger and final verdicts from the kernel authority. | A projected grant honored after canonical invalidation or revision must fail. | U5h recovery or egress attempt against a valid old projection. | #353, #388 |
| `rp21_recovery_contract_mismatch` | [rp21-recovery-preserves-canonical-authority](export-recovery/catalog.md#rp21-recovery-preserves-canonical-authority) | AC6 | T7 | `schema`, `tokenizer`, `model`, `policy`, `identity` | Persisted projection tuple compared with the active tuple, one dimension changed per scenario. | Serving after a mismatch on any single dimension must fail. | U5a/U5e open with each mismatch dimension. | #381, #385 |
| `rp21_recovery_authority_unavailable` | [rp21-recovery-preserves-canonical-authority](export-recovery/catalog.md#rp21-recovery-preserves-canonical-authority) | AC6 | T7 | `authority_read_failed` | Injected canonical read failure and the egress outcome. | Unavailable authority turned into a projected grant must fail. | U5h egress while the canonical read is failed. | #353, #388 |

### Projection and coverage map

| Marker | Property | AC | Seams | Scenario cells | Independent oracle | Negative control | Integration observation | Built by |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `rp21_projection_atomic_uncommitted_kill` | [projection-commit-checkpoint-pending-atomic](projection-coverage/catalog.md#projection-commit-checkpoint-pending-atomic) | AC2 | T5 | `kill_before_local_commit` | Reopened rows, tombstones, checkpoint, and Pending compared with the whole prior state. | A mixed prefix after reopen must fail. | U2b multirow dense-required commit killed before COMMIT. | #362 |
| `rp21_projection_old_prefix_after_delete` | [projection-replay-does-not-resurrect](projection-coverage/catalog.md#projection-replay-does-not-resurrect) | AC2, AC3 | T4 | `old_prefix_after_delete` | Ledger tombstone for the lineage and the reopened row set. | A resurrected occurrence after old-prefix delivery must fail. | U2b replay of an old prefix after a deletion commit. | #362 |
| `rp21_projection_duplicate_after_reopen` | [projection-replay-does-not-resurrect](projection-coverage/catalog.md#projection-replay-does-not-resurrect) | AC2 | T4 | `duplicate_after_reopen` | Ledger multiplicity and Pending identity set after identical redelivery. | Duplicate rows or duplicate outstanding jobs must fail. | U2b redelivery of an applied prefix after reopen. | #362 |
| `rp21_projection_equal_bytes_distinct_tuples` | [projection-occurrence-payload-separation](projection-coverage/catalog.md#projection-occurrence-payload-separation) | AC3 | T1 | `messages`, `canonical_claims`, `promoted_memory`, `git_commits`, `raw_tool_spans`, `claim_promoted_dual_membership` | Fixture-owned tuples in source-identity-fixtures.json encoded with the CC4 reference encoder; payload bytes equal by construction. | Collapsing occurrence identity to payload identity must fail. | U2a write, close, and reopen of the fixture records. | #361 |
| `rp21_projection_forced_payload_collision` | [projection-occurrence-payload-separation](projection-coverage/catalog.md#projection-occurrence-payload-separation) | AC3 | T1 | `unequal_bytes_equal_digest` | Injected identical digest for two unequal buffers; byte comparison result. | Sharing storage or replacing evidence on a digest match without byte equality must fail. | U1b/U2a collision injection through the test-support digest seam. | #356, #361 |
| `rp21_projection_messages_revised` | [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | AC3 | T4 | `opencode`, `pi` | Independent message inventory E(c,p) with both revisions from the harness fixture. | Omitting the revised message from the report and denominator must fail. | U4a message ingest for each supported harness. | #372 |
| `rp21_projection_claims_deleted` | [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | AC3 | T4 | `claim_retired` | Canonical claim creation and retirement commits in the operation ledger. | A retired claim still reported as covered must fail. | U4c claim occurrence integration over a real kernel. | #374 |
| `rp21_projection_promoted_memory_revised` | [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | AC3 | T4 | `promoted_replaced` | Positive stable-memory-domain decisions and their canonical replacement in the ledger. | A replaced promoted memory reported under its old revision must fail. | U4c promoted-memory integration over a real kernel. | #374 |
| `rp21_projection_git_rows_removed` | [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | AC3 | T4 | `git_source_policy_removal` | Durable commit-source entries and the source-policy removal event in the ledger. | Rows retained after removal, or rows deleted on a failed scan, must fail. | U4d/U4e git inventory reconciliation. | #375, #376 |
| `rp21_projection_tool_selection_changed` | [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | AC3 | T4 | `opencode:whole_block_then_span`, `pi:whole_block_then_span` | Two declared span selections over one retained buffer; occurrence tuples differ only in span. | Merging the two selections or normalizing the buffer must fail. | U4b raw tool projection for each supported harness. | #373 |
| `rp21_projection_complete_declared_nonempty_inventory` | [projection-source-inventory-complete](projection-coverage/catalog.md#projection-source-inventory-complete) | AC3, AC11 | T4 | `messages`, `canonical_claims`, `promoted_memory`, `git_commits`, `raw_tool_spans` | Independent nonempty inventory E(c,p) per class; the product's completeness declaration for that checkpoint and policy. | A declaration over an empty or unknown inventory, or an aggregate denominator hiding a class, must fail. | U4f coverage report per class over a real store. | #379 |
| `rp21_projection_raw_multibyte_default` | [projection-raw-tools-exact-and-lexical](projection-coverage/catalog.md#projection-raw-tools-exact-and-lexical) | AC3 | T1 | `multibyte_crlf_overlapping_spans` | Independently captured native buffer bytes and CC3 span offsets. | Normalized bytes or a dense job created under default policy must fail. | U4b raw tool indexing with default dense-tool policy disabled. | #373 |
| `rp21_projection_raw_multipart_error` | [projection-raw-tools-exact-and-lexical](projection-coverage/catalog.md#projection-raw-tools-exact-and-lexical) | AC3 | T1 | `multipart_error_result` | Native multipart error result captured before any projection conversion. | Joined parts or a lossy conversion must fail. | U1b/U4b capture of a multipart error result. | #356, #373 |
| `rp21_projection_missing_gate_evidence` | [projection-n13-hooks-stay-gated](projection-coverage/catalog.md#projection-n13-hooks-stay-gated) | AC9 | T7 | every hook × every entry point (48 cells: `<hook>@<entry>`) | Independent hook manifest and absent evidence for the named gate; the refused activation names gate and entry point. | Activation with absent evidence must fail. | U4h gate evaluation at each entry point. | #380 |
| `rp21_projection_failed_gate_evidence` | [projection-n13-hooks-stay-gated](projection-coverage/catalog.md#projection-n13-hooks-stay-gated) | AC9 | T7 | every hook × every entry point (48 cells: `<hook>@<entry>`) | Present evidence that fails its gate; the refused activation names gate and entry point. | Activation with failed evidence must fail. | U4h gate evaluation at each entry point. | #380 |
| `rp21_projection_unsupported_or_inapplicable_evidence` | [projection-n13-hooks-stay-gated](projection-coverage/catalog.md#projection-n13-hooks-stay-gated) | AC9, AC12 | T7 | every hook × every entry point × {`unsupported`, `inapplicable`} (96 cells: `<hook>@<entry>@<state>`) | Capability disposition from CC8 and evidence target identity; bounded scenario value distinguishes the two cases. | Simulated capability or evidence applied to the wrong target must fail. | U4h gate evaluation with an unsupported adapter and with inapplicable evidence. | #380 |
| `rp21_projection_stale_grant_after_retirement` | [projection-canonical-eligibility-authority](projection-coverage/catalog.md#projection-canonical-eligibility-authority) | AC6 | T7 | `retired_after_grant` | Canonical retirement in the ledger; both adapter validations requested. | A retained projected grant honored by either adapter must fail. | P2 kernel eligibility and daemon parity; U4c retrieval adapter. | #353, #374 |
| `rp21_projection_pinned_adapter_pair` | [projection-canonical-eligibility-authority](projection-coverage/catalog.md#projection-canonical-eligibility-authority) | AC6 | T7 | `daemon_vs_kernel`, `retrieval_vs_kernel` | Independently authored expected verdicts for a mixed batch on pinned facts. | Divergent verdicts, or a verdict from a joined snapshot, must fail. | P2 kernel and daemon parity; U4c retrieval parity. | #353, #374 |
| `rp21_projection_lexical_current_vector_old` | [projection-lexical-dense-coverage-distinct](projection-coverage/catalog.md#projection-lexical-dense-coverage-distinct) | AC3 | T4 | `older_revision_vector`, `older_model_vector` | Fixture-owned vector metadata naming an older revision or model; independent M = R minus V. | Counting the old vector as valid coverage must fail. | U4f coverage report with old vector metadata. | #379 |
| `rp21_projection_same_render_new_revision` | [projection-lexical-dense-coverage-distinct](projection-coverage/catalog.md#projection-lexical-dense-coverage-distinct) | AC3 | T4 | `equal_text_new_revision` | Two revisions with equal rendered text and different canonical revision identities. | Treating equal text as current dense coverage for the new revision must fail. | U4f coverage report after a same-text revision. | #379 |
| `rp21_projection_pending_full_next_commit` | [projection-bounded-admission-preserves-progress](projection-coverage/catalog.md#projection-bounded-admission-preserves-progress) | AC5, AC7 | T6 | `pending_cap_reached` | Independent Pending count at the approved cap and the offered complete commit. | Skipping the commit, dropping a job, or advancing progress at capacity must fail. | U2b admission when Pending equals its cap. | #362 |
| `rp21_projection_complete_commit_over_cap` | [projection-bounded-admission-preserves-progress](projection-coverage/catalog.md#projection-bounded-admission-preserves-progress) | AC2 | T6 | `rows_over_cap`, `bytes_over_cap` | Fixture-measured rows and bytes of one complete commit against the declared local cap before admission. | Splitting the commit, enlarging bounds, or truncation after decode must fail. | U2b admission of an oversized complete commit. | #362 |
| `rp21_projection_remediation_without_revision_change` (conditional applicability) | [projection-remediation-invalidates-derived-bytes](projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes) | AC10 | T3 | `applicability_decision_recorded`, `name_only_control` | CC7 records that the approved mapping excludes domains.name; the control reconstructs mapped input bytes before and after remediation and finds them equal. | A fabricated passing witness, or changed input bytes under an excluded field, must fail. | U1c/U1f name-only remediation control over a real store. | #357, #360 |

### Embedding map

| Marker | Property | AC | Seams | Scenario cells | Independent oracle | Negative control | Integration observation | Built by |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `rp21_embedding_full_sequence_straddles_window` | [embedding-count-authority-is-untruncated](embedding/catalog.md#embedding-count-authority-is-untruncated) | AC4 | T1 | `straddle_window_bytes_fit` | Pinned complete token sequence from the independent token fixture; both tokenizer artifact identities observed. | Counting only a truncated prefix must fail. | U3a exact count over the verified artifact. | #364 |
| `rp21_embedding_invalid_input_offered_to_ready_lane` | [embedding-input-is-rejected-before-inference](embedding/catalog.md#embedding-input-is-rejected-before-inference) | AC4 | T2 | `token_limit_plus_one`, `byte_cap_overflow`, `count_unavailable`, `identity_unavailable` | Deterministic engine attempt counter baselined after certification; valid exact-limit control input. | Counter growth for invalid input, or silent truncation, must fail. | U3a preflight in front of the real engine. | #364 |
| `rp21_embedding_pending_rediscovered_after_descriptor_loss` | [embedding-pending-drives-one-job-table](embedding/catalog.md#embedding-pending-drives-one-job-table) | AC5 | T8 | `rediscovered_twice`, `admission_response_suppressed`, `capacity_pressure` | Durable Pending readback plus admission and worker observations. | A second worker or queue for one K, or Pending erased by refusal, must fail. | U3d dispatch of durable Pending through the existing JobTable. | #368 |
| `rp21_embedding_identity_changed_with_result_held` | [embedding-completion-is-identity-fenced](embedding/catalog.md#embedding-completion-is-identity-fenced) | AC5 | T3 | `revision`, `model`, `fingerprint`, `dimension`, `epoch`, `input_hash`, `tombstone` | Independent current-source oracle after one dimension changes while a validated old result is held. | Completion that ignores any single dimension must fail. | U3b guard at the mutation boundary. | #365 |
| `rp21_embedding_process_killed_at_persistence_boundary` | [embedding-complete-requires-durable-vector](embedding/catalog.md#embedding-complete-requires-durable-vector) | AC5, AC2 | T5 | `vector_write_issued_before_commit`, `vector_committed_before_completion_mark` | Reopened identity plus vector bytes after termination at each boundary. | Completion marked without a durable matching vector must fail. | U3c persistence under the shared crash pattern. | #367 |
| `rp21_embedding_restart_has_stable_pending_and_service` | [embedding-restart-retries-durable-pending](embedding/catalog.md#embedding-restart-retries-durable-pending) | AC7 | T9 | `restart_fresh_jobtable` | Pre-crash durable Pending(K), fresh process and table, unchanged identity, healthy service window. | Endless restart, no work, or a moved deadline must fail. | U3d/U3e restart episode with a fresh JobTable over durable Pending. | #368, #369 |
| `rp21_embedding_query_arrives_during_backfill_saturation` | [embedding-backfill-preserves-query-admission](embedding/catalog.md#embedding-backfill-preserves-query-admission) | AC8 | T8 | `query_under_cap_during_saturation` | Offered-query ledger with offered, admitted, started, and completed states; backfill envelope full. | FIFO starvation of the query must fail. | U3g backfill saturation with an offered query. | #371 |
| `rp21_embedding_gc_candidate_has_concurrent_holder` | [embedding-identity-gc-preserves-live-work](embedding/catalog.md#embedding-identity-gc-preserves-live-work) | AC5 | T3 | `held_result`, `live_reference`, `identity_update_race`, `dispatch_race` | Independent live-reference census during the GC slice. | Deleting a live holder must fail. | U3f GC racing a holder. | #370 |
| `rp21_embedding_stop_occurs_with_native_work_held` | [embedding-supervisor-shares-budget-and-joins](embedding/catalog.md#embedding-supervisor-shares-budget-and-joins) | AC8 | T8 | `cancel_with_native_held`, `deadline_with_native_held`, `shutdown_with_slice_due` | Task, permit, and live-charge census observed independently until physical exit. | Charges released on future drop, or shutdown completing before owners exit, must fail. | U3g/U4g supervisor stop with native work held. | #371, #377 |
| `rp21_embedding_verified_path_replaced_after_load` | [embedding-count-authority-is-untruncated](embedding/catalog.md#embedding-count-authority-is-untruncated) | AC4 | T1 | `artifact_path_replaced` | Retained verified bytes and the replaced pathname observed before counting. | Counting from the replaced path must fail. | U3a count after artifact path replacement. | #364 |
| `rp21_embedding_model_name_changes_without_fingerprint_change` | [embedding-completion-is-identity-fenced](embedding/catalog.md#embedding-completion-is-identity-fenced) | AC5, AC6 | T3 | `model_name_only` | Model name changed with fingerprint and dimension held equal. | Completion or serving keyed on fingerprint alone must fail. | U3b guard and U5a mismatch detection. | #365, #381 |
| `rp21_embedding_shared_payload_has_distinct_occurrences` | [embedding-identity-gc-preserves-live-work](embedding/catalog.md#embedding-identity-gc-preserves-live-work) | AC3, AC5 | T3 | `completion_phase`, `gc_phase` | Two occurrence IDs with identical payload bytes in the fixture set. | Completing or collecting by payload identity must fail. | U3c completion and U3f GC over shared payloads. | #367, #370 |
| `rp21_embedding_vector_commit_response_is_lost` | [embedding-complete-requires-durable-vector](embedding/catalog.md#embedding-complete-requires-durable-vector) | AC5 | T5 | `commit_response_suppressed` | Durable vector readback after the suppressed response. | Completion attempted on a response-less commit without readback must fail. | U3c completion after a lost storage response. | #367 |
| `rp21_embedding_retryable_failure_precedes_recovery` | [embedding-restart-retries-durable-pending](embedding/catalog.md#embedding-restart-retries-durable-pending) | AC7 | T9 | `retryable_failure_then_window` | Injected retryable failure before the fault-free window and the durable disposition. | Retry that exceeds the approved attempt bound or resets the clock must fail. | U3e retry disposition then recovery window. | #369 |
| `rp21_embedding_budget_crosses_between_stages` | [embedding-supervisor-shares-budget-and-joins](embedding/catalog.md#embedding-supervisor-shares-budget-and-joins) | AC8 | T8 | `queue_to_embed`, `embed_to_sqlite`, `sqlite_to_dense_checkpoint` | Original EvalBudget deadline D observed independently at each stage entry. | A stage that renews the budget must fail. | U3g stage transitions with the shared budget. | #371 |
| `rp21_embedding_gc_obsolete_identity_is_unreferenced` | [embedding-identity-gc-preserves-live-work](embedding/catalog.md#embedding-identity-gc-preserves-live-work) | AC5 | T3 | `obsolete_unreferenced_before_slice` | Independently identified obsolete identity with no live reference before the bounded slice. | A completed sweep that leaves it in place must fail. | U3f eligible cleanup slice over a real store. | #370 |

## Result marker

`rp21_projection_acceptance_situations_witnessed` is the aggregate acceptance
marker. It records the matrix's result: a completed approved campaign has a
known witness for every required cell above. It is one of the 63 definitions
but is not a required cell in its own matrix; this exception waives no other
cell. Unknown, skipped, unfired, or retroactively changed-limit cells prevent
this marker from firing.

## Coverage by acceptance criterion

| Criterion | Cells | Markers |
| --- | --- | --- |
| AC1 | 6 | `rp21_export_writes_between_pages`, `rp21_export_operator_remediation_during_snapshot`, `rp21_export_prune_with_fence`, `rp21_export_retention_lost_mid_build`, `rp21_export_oversize_row_present`, `rp21_export_page_budget_edge` |
| AC2 | 11 | `rp21_catchup_split_commit`, `rp21_catchup_published_retained`, `rp21_catchup_empty_commit`, `rp21_ack_local_commit_interrupted`, `rp21_ack_writer_contended`, `rp21_ack_response_lost`, `rp21_projection_atomic_uncommitted_kill`, `rp21_projection_old_prefix_after_delete`, `rp21_projection_duplicate_after_reopen`, `rp21_projection_complete_commit_over_cap`, `rp21_embedding_process_killed_at_persistence_boundary` |
| AC3 | 14 | `rp21_projection_old_prefix_after_delete`, `rp21_projection_equal_bytes_distinct_tuples`, `rp21_projection_forced_payload_collision`, `rp21_projection_messages_revised`, `rp21_projection_claims_deleted`, `rp21_projection_promoted_memory_revised`, `rp21_projection_git_rows_removed`, `rp21_projection_tool_selection_changed`, `rp21_projection_complete_declared_nonempty_inventory`, `rp21_projection_raw_multibyte_default`, `rp21_projection_raw_multipart_error`, `rp21_projection_lexical_current_vector_old`, `rp21_projection_same_render_new_revision`, `rp21_embedding_shared_payload_has_distinct_occurrences` |
| AC4 | 3 | `rp21_embedding_full_sequence_straddles_window`, `rp21_embedding_invalid_input_offered_to_ready_lane`, `rp21_embedding_verified_path_replaced_after_load` |
| AC5 | 9 | `rp21_projection_pending_full_next_commit`, `rp21_embedding_pending_rediscovered_after_descriptor_loss`, `rp21_embedding_identity_changed_with_result_held`, `rp21_embedding_process_killed_at_persistence_boundary`, `rp21_embedding_gc_candidate_has_concurrent_holder`, `rp21_embedding_model_name_changes_without_fingerprint_change`, `rp21_embedding_shared_payload_has_distinct_occurrences`, `rp21_embedding_vector_commit_response_is_lost`, `rp21_embedding_gc_obsolete_identity_is_unreferenced` |
| AC6 | 7 | `rp21_replacement_switch_interrupted`, `rp21_recovery_stale_authority`, `rp21_recovery_contract_mismatch`, `rp21_recovery_authority_unavailable`, `rp21_projection_stale_grant_after_retirement`, `rp21_projection_pinned_adapter_pair`, `rp21_embedding_model_name_changes_without_fingerprint_change` |
| AC7 | 7 | `rp21_rebuild_missing_db_after_prune`, `rp21_rebuild_quiet_window_admitted`, `rp21_healthy_lagged_target_admitted`, `rp21_authorized_recovery_current_observed`, `rp21_projection_pending_full_next_commit`, `rp21_embedding_restart_has_stable_pending_and_service`, `rp21_embedding_retryable_failure_precedes_recovery` |
| AC8 | 3 | `rp21_embedding_query_arrives_during_backfill_saturation`, `rp21_embedding_stop_occurs_with_native_work_held`, `rp21_embedding_budget_crosses_between_stages` |
| AC9 | 7 | `rp21_authorized_recovery_current_observed`, `rp21_disable_pending_consumer`, `rp21_disable_last_consumer`, `rp21_abandon_request_with_barrier`, `rp21_projection_missing_gate_evidence`, `rp21_projection_failed_gate_evidence`, `rp21_projection_unsupported_or_inapplicable_evidence` |
| AC10 | 2 | `rp21_export_operator_remediation_during_snapshot`, `rp21_projection_remediation_without_revision_change` |
| AC11 | 1 | `rp21_projection_complete_declared_nonempty_inventory` |
| AC12 | 1 | `rp21_projection_unsupported_or_inapplicable_evidence` |

## Coverage by testing seam

| Seam | Cells | Markers |
| --- | --- | --- |
| T1 | 6 | `rp21_projection_equal_bytes_distinct_tuples`, `rp21_projection_forced_payload_collision`, `rp21_projection_raw_multibyte_default`, `rp21_projection_raw_multipart_error`, `rp21_embedding_full_sequence_straddles_window`, `rp21_embedding_verified_path_replaced_after_load` |
| T2 | 1 | `rp21_embedding_invalid_input_offered_to_ready_lane` |
| T3 | 6 | `rp21_projection_remediation_without_revision_change`, `rp21_embedding_identity_changed_with_result_held`, `rp21_embedding_gc_candidate_has_concurrent_holder`, `rp21_embedding_model_name_changes_without_fingerprint_change`, `rp21_embedding_shared_payload_has_distinct_occurrences`, `rp21_embedding_gc_obsolete_identity_is_unreferenced` |
| T4 | 16 | `rp21_export_writes_between_pages`, `rp21_export_operator_remediation_during_snapshot`, `rp21_export_retention_lost_mid_build`, `rp21_catchup_split_commit`, `rp21_catchup_published_retained`, `rp21_catchup_empty_commit`, `rp21_projection_old_prefix_after_delete`, `rp21_projection_duplicate_after_reopen`, `rp21_projection_messages_revised`, `rp21_projection_claims_deleted`, `rp21_projection_promoted_memory_revised`, `rp21_projection_git_rows_removed`, `rp21_projection_tool_selection_changed`, `rp21_projection_complete_declared_nonempty_inventory`, `rp21_projection_lexical_current_vector_old`, `rp21_projection_same_render_new_revision` |
| T5 | 7 | `rp21_ack_local_commit_interrupted`, `rp21_ack_writer_contended`, `rp21_ack_response_lost`, `rp21_replacement_switch_interrupted`, `rp21_projection_atomic_uncommitted_kill`, `rp21_embedding_process_killed_at_persistence_boundary`, `rp21_embedding_vector_commit_response_is_lost` |
| T6 | 4 | `rp21_export_oversize_row_present`, `rp21_export_page_budget_edge`, `rp21_projection_pending_full_next_commit`, `rp21_projection_complete_commit_over_cap` |
| T7 | 13 | `rp21_export_prune_with_fence`, `rp21_export_retention_lost_mid_build`, `rp21_disable_pending_consumer`, `rp21_disable_last_consumer`, `rp21_abandon_request_with_barrier`, `rp21_recovery_stale_authority`, `rp21_recovery_contract_mismatch`, `rp21_recovery_authority_unavailable`, `rp21_projection_missing_gate_evidence`, `rp21_projection_failed_gate_evidence`, `rp21_projection_unsupported_or_inapplicable_evidence`, `rp21_projection_stale_grant_after_retirement`, `rp21_projection_pinned_adapter_pair` |
| T8 | 4 | `rp21_embedding_pending_rediscovered_after_descriptor_loss`, `rp21_embedding_query_arrives_during_backfill_saturation`, `rp21_embedding_stop_occurs_with_native_work_held`, `rp21_embedding_budget_crosses_between_stages` |
| T9 | 6 | `rp21_rebuild_missing_db_after_prune`, `rp21_rebuild_quiet_window_admitted`, `rp21_healthy_lagged_target_admitted`, `rp21_authorized_recovery_current_observed`, `rp21_embedding_restart_has_stable_pending_and_service`, `rp21_embedding_retryable_failure_precedes_recovery` |

## Later evidence every witness record carries

Campaign execution is not a P1 closure prerequisite, but every later witness
record for a required cell carries these fields. Missing fields make the
record unknown, and an unknown required cell blocks witness closure.

- `matrix_version`
- `marker`
- `scenario_id`
- `run_id`
- `observed_event`
- `product_revision`
- `toolchain`
- `features`
- `shard`
- `configuration_hash`
- `source_identities_and_hashes`
- `seed`
- `minimized_history`
- `fault_boundary`
- `external_barrier_kill_reopen_receipt`
- `attempts`
- `acknowledgements`
- `unknown_outcomes`
- `effects_per_identity`
- `resource_and_time_observations`
- `limit_manifest_version`
- `approval_digest`
- `certification_identity`

Actual crash cuts require the external barrier, kill, and reopen receipt from
the shared child-process pattern; a callback reached inside a surviving
process is not a cut. Certified inference requires the certification identity
of the tokenizer artifact and engine plus the input witness; a counting double
or a disabled external lane is not evidence. Current outcomes require the
independent `Current` observation through the episode's fixed target, not a
no-op progress report. Source identities and hashes, seeds, toolchain,
features, and configuration hashes pin the run so a witness can be replayed
and audited. Limit changes after a candidate run require a new protocol
version and cannot retroactively satisfy a cell.

## Scope rules

- Required cells receive no arbitrary optional exemption. Only scope declared
  before the campaign may exclude clearly unrelated optional checks.
- Optional unsupported external lanes stay disabled; their refusal never
  replaces required supported-path evidence.
- Unresolved source mapping, missing numeric approvals, skipped cases, and
  unfired markers block acceptance.
- Restricted corpora and large crash images live only in approved external
  storage. Witness records carry references, integrity metadata, and access
  requirements, never the payloads.
