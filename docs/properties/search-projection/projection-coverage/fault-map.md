# RP2.1 fault and enabling-state map

System: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. External sources and evidence limits:
[_lenses/model.md](_lenses/model.md#source-register).
No incidents were supplied. No fault campaign or test ran. The independent
evaluation and this pass's dispositions are recorded in
[portfolio-evaluation.md](portfolio-evaluation.md).

## Fault availability

| Fault or state | Available evidence/seam | Missing RP2.1 seam |
| --- | --- | --- |
| Repeated kernel acknowledgement and partial outbox batch | Existing tests in `kernel_outbox.rs:86-153,622-698`. | Local transaction and its independent recovered-state observer. |
| Abrupt termination and lost COMMIT/ack response | `crates/kernel/tests/cas_fault_injection.rs:1046-1094` has a child barrier, parent wait and kill/reap pattern; `:924-990` compares reopened state. Status unaudited. | Search process hooks and boundary observation are still missing. A clean close is insufficient. Ack boundaries belong to the export/recovery owner. |
| Duplicate/outdated prefix | Source sequences can be represented independently. | Local replay entry point and observable job identities. |
| Equal payload, distinct occurrence | Existing codec fixtures provide adjacent examples. | Retrieval tuple encoding and collision injection. |
| Five classes with revision/deletion | Canonical decision fixtures and two codecs exist. | Message/git ingest and an approved complete source inventory. |
| Exact tool spans | Text and multipart tool codec inputs exist. | Authoritative raw buffer, byte-span convention and durable readback. |
| Disabled hook attempts | Current config and route tests exist. | Accepted-gate model plus supervisor/startup attempts. |
| Canonical change versus cached grant | `stage1_eligibility.rs:186-224` and private cache test seam. | Shared kernel policy and retrieval adapter. |
| Invalid/missing dense coverage | None for local projection. | Embedding owner supplies validity facts; report consumer exposes sets. |
| Local capacity exhaustion | `pending_outbox` supplies a row cap only. | Byte measurement before admission, pending-count cap and explicit block result. |
| In-place canonical remediation | `crates/kernel/src/envelope.rs:395-438` rewrites `domains.name` and emits `operator_remediation` without changing source revision. | Approved bounded source mapping, derived-input readback and current-input hash comparison. No projected dependence on this field is established. |
| Completed acceptance witness matrix | CAS descriptor/driver accounting exists at `cas_fault_injection.rs:391-423,981-989`, status unaudited. | Approved three-surface campaign, product witnesses and fixed per-scenario requirement matrix. |

## Per-record precondition markers

Every marker below has `sometimes` semantics across its declared campaign.
Names are literal and globally prefixed, never assembled dynamically. A marker
records the listed preconditions, not the forbidden outcome. Each can fire on
a correct implementation. If a required marker does not fire, acceptance fails
the [first-class witness record][acceptance] even when safety observations pass.

Markers do not replace each record's input matrix. Retain fixture-owned
witnesses for every required source class, harness and gate-failure case. A
single hook attempt cannot stand in for all hooks; a single stale revision
cannot stand in for model, scope and sensitivity cases.

| Property | Constant marker | Independent preconditions |
| --- | --- | --- |
| [projection-commit-checkpoint-pending-atomic][atomic] | `rp21_projection_atomic_uncommitted_kill` | A multirow dense-required commit enters the local transaction and the process is killed before COMMIT is issued. |
| [projection-replay-does-not-resurrect][replay] | `rp21_projection_old_prefix_after_delete` | A deletion commit and a later old-prefix delivery are both issued against the same source lineage. |
| [projection-replay-does-not-resurrect][replay] | `rp21_projection_duplicate_after_reopen` | A prefix has been applied, the store reopens, and identical input is delivered again. |
| [projection-occurrence-payload-separation][identity] | `rp21_projection_equal_bytes_distinct_tuples` | Two fixture-owned source tuples differ while their independently captured payload bytes are equal. |
| [projection-occurrence-payload-separation][identity] | `rp21_projection_forced_payload_collision` | Two unequal source buffers are submitted with an injected identical digest result. |
| [projection-source-inventory-complete][inventory] | `rp21_projection_messages_revised` | Message input and its later source revision are offered through the source adapter. |
| [projection-source-inventory-complete][inventory] | `rp21_projection_claims_deleted` | Canonical claim creation and retirement are committed. |
| [projection-source-inventory-complete][inventory] | `rp21_projection_promoted_memory_revised` | An explicitly promoted memory and its canonical replacement are committed. |
| [projection-source-inventory-complete][inventory] | `rp21_projection_git_rows_removed` | Durable commit-source entries and a source-policy removal event are offered. |
| [projection-source-inventory-complete][inventory] | `rp21_projection_tool_selection_changed` | A raw tool result is offered under two declared span selections. |
| [projection-source-inventory-complete][inventory] | `rp21_projection_complete_declared_nonempty_inventory` | The independent bounded source inventory `E(c,p)` is known and nonempty, and the product emits a completeness declaration for that checkpoint/policy. Equality with the projection is checked separately. |
| [projection-raw-tools-exact-and-lexical][raw] | `rp21_projection_raw_multibyte_default` | A source buffer with multibyte text, CRLF and overlapping selected spans is offered with default dense-tool policy disabled. |
| [projection-raw-tools-exact-and-lexical][raw] | `rp21_projection_raw_multipart_error` | An error result with multiple native parts is captured before any projection conversion. |
| [projection-n13-hooks-stay-gated][gates] | `rp21_projection_missing_gate_evidence` | A named N1.3 activation is attempted with required acceptance evidence absent; the scenario identifies the missing gate and entry point. |
| [projection-n13-hooks-stay-gated][gates] | `rp21_projection_failed_gate_evidence` | A named N1.3 activation is attempted with present evidence that fails its required gate; the scenario identifies the failed gate and entry point. |
| [projection-n13-hooks-stay-gated][gates] | `rp21_projection_unsupported_or_inapplicable_evidence` | Activation is attempted with an unsupported adapter or evidence inapplicable to its target. A bounded scenario value distinguishes these two cases from missing and failed evidence. |
| [projection-canonical-eligibility-authority][eligibility] | `rp21_projection_stale_grant_after_retirement` | A projected grant is retained, its canonical source is retired, and both adapter validations are requested. |
| [projection-canonical-eligibility-authority][eligibility] | `rp21_projection_pinned_adapter_pair` | Both adapters receive the same mixed candidate batch, scope, destination and pinned canonical facts. |
| [projection-lexical-dense-coverage-distinct][coverage] | `rp21_projection_lexical_current_vector_old` | A lexical row is current and fixture-owned vector metadata names an older revision or model before the report is requested. |
| [projection-lexical-dense-coverage-distinct][coverage] | `rp21_projection_same_render_new_revision` | Two source revisions render equal text but retain different canonical revision identities. |
| [projection-bounded-admission-preserves-progress][bounds] | `rp21_projection_pending_full_next_commit` | Pending count equals the approved cap and a complete source commit requiring another job is offered. |
| [projection-bounded-admission-preserves-progress][bounds] | `rp21_projection_complete_commit_over_cap` | Fixture-measured bytes or row count of one complete commit exceed the declared local cap before admission. |
| [projection-remediation-invalidates-derived-bytes][remediation] | `rp21_projection_remediation_without_revision_change` | A witnessed `domains.name` remediation changes independently reconstructed input bytes under approved bounded `M`, while the canonical source revision is unchanged. Record actual before/after occurrence tuples and input hashes; do not assert that every embedding-key field agrees. |
| [projection-acceptance-situations-witnessed][acceptance] | `rp21_projection_acceptance_situations_witnessed` | A completed approved campaign has independent witnesses for every required cell of its frozen nonempty three-surface matrix `R`. The root marker is excluded from `R`; unknown, skipped or unfired required cells prevent this witness. |

The old-vector marker describes an allowed stored input awaiting repair, not
a report that incorrectly counts that vector. The raw-default marker records
policy and input, not absence of a job. The retirement marker records source
change and requests, not an unauthorized result. Gate markers require attempts,
not successful execution of disabled work.

## Shared marker ownership

Cross-store acknowledgement has one owner:
[rp21-ack-follows-local-release][ack-owner]. Consume
`rp21_ack_local_commit_interrupted` and `rp21_ack_response_lost` from
[export-recovery/fault-map.md][export-faults] without redefining them here.
Their boundary traces may also support local-state comparison after reopen.
The local pre-COMMIT kill marker remains owned by this map. The contended-writer
scenario, ack bounds and lock-release order remain with the ack owner.

## Binding acceptance matrix

The coordinator freezes `R` before observations, using the required markers and
scenario dimensions from this map, [export/recovery][export-faults] and
[embedding][embedding-faults]. Each cell has a constant marker name and a
predeclared bounded `scenario_id`; identifiers are not dynamically appended to
marker names. Witness data identifies the run and actual observed event.
The following dimensions are mandatory for the declared supported RP2.1 scope:

| Dimension | Required witness, beyond an offered workload |
| --- | --- |
| Source coverage | Each of messages, canonical claims, promoted memory, git commits and selected raw tool spans has its required lifecycle cases and an observed complete declaration over independent nonempty inventory. Applicable message/tool cases cover both supported harnesses. |
| Canonical authority | The declared revision, tombstone, scope and sensitivity cases run; remediation applicability is resolved from approved `M`. If included, record changed canonical field bytes and current-input hashes, not an assumed key collision. |
| Evidence gates | Missing, failed and unsupported/inapplicable evidence occupy separate scenario cells for each applicable hook/entry point. Valid supported activation controls are required once the gate exists. |
| Certified inference | The embedding owner's certified artifact/tokenizer/inference path observes offered input, with certification identity and input witness. Counting-double tests and a disabled external lane cannot substitute for this required path. |
| Crash boundaries | Every declared local, ack, embedding-persistence and selector boundary has an external barrier/termination/reopen witness. A callback reached or a clean restart alone is insufficient. |
| Recovery | The [progress owner][progress-owner] supplies actual `Current` state witnesses for finite normal catch-up and authorized recovery; declared rebuild-after-pruning and embedding recovery cases also need their required outcomes within RP2.9-approved bounds. |

The root check is `sometimes` at completed-campaign scope, not a percentage
metric. Every required cell must have a known witness; one arbitrary marker
cannot satisfy it. Never use a safety violation as a witness condition.
The root checks situations, while the safety/liveness owners judge correctness.

Required acceptance receives no arbitrary optional exemption. Clearly unrelated
optional checks may be excluded only by scope declared before the campaign.
Optional unsupported external lanes remain disabled; their refusal is not a
replacement for required supported-path evidence. Unresolved source mapping,
missing numeric approvals, skipped cases and unfired markers block acceptance.
A conditional record whose premise is demonstrably absent from approved scope
has a recorded applicability decision, not a fabricated passing witness.

## Timing and observation rules

- Check local safety during active fault injection and after reopen. Only
  committed-prefix states are allowed; response loss does not identify which
  prefix committed.
- Track attempted prefixes, observed local commits, acknowledgement attempts
  and acknowledgement responses separately. Reconcile by occurrence/job key;
  one commit may create zero or many rows and retries may create none.
- Hold source incarnation, policy and model identity explicit in the oracle.
  Do not compare unrelated source epochs by integer sequence alone.
- Class inventories use source-authored expectations. A class omitted by both
  the projector and its self-derived denominator must fail the independent
  comparison.
- For bounded progress, consume [normal catch-up and authorized recovery][progress-owner]
  from the export/recovery owner. RP2.9 supplies numeric bounds and the admitted
  finite input envelope. The root matrix requires actual `Current` witnesses;
  a quiet-window admission marker alone does not demonstrate convergence.
- A process-crash run does not establish power-loss behavior. SQLite settings,
  filesystem contract and storage-fault evidence belong to the durability
  validation handoff, not to a claimed result here.

## Leverage ranking by cheapest valid oracle

1. Identity and raw bytes: deterministic tuple/buffer comparisons isolate
   collapse and normalization without inference or process scheduling.
2. Gate attempts and pinned eligibility pairs: direct entry-point/canonical
   comparisons expose authority errors with a small fixture.
3. Coverage set algebra and five-class inventory: use scripted validity facts
   and independent source inventories before adding expensive full-path runs.
4. Local capacity admission: small approved test caps expose whole-commit
   refusal and progress corruption without a large corpus.
5. Atomicity and replay: require actual local persistence and controlled
   termination/reopen, so follow the cheaper contract fixtures.

These are routing recommendations for `/testing:test-strategy`, not test-form
choices. The independent evaluator's findings and coordinator decisions are
recorded in [portfolio-evaluation.md](portfolio-evaluation.md).

[atomic]: catalog.md#projection-commit-checkpoint-pending-atomic
[replay]: catalog.md#projection-replay-does-not-resurrect
[identity]: catalog.md#projection-occurrence-payload-separation
[inventory]: catalog.md#projection-source-inventory-complete
[raw]: catalog.md#projection-raw-tools-exact-and-lexical
[gates]: catalog.md#projection-n13-hooks-stay-gated
[eligibility]: catalog.md#projection-canonical-eligibility-authority
[coverage]: catalog.md#projection-lexical-dense-coverage-distinct
[bounds]: catalog.md#projection-bounded-admission-preserves-progress
[remediation]: catalog.md#projection-remediation-invalidates-derived-bytes
[acceptance]: catalog.md#projection-acceptance-situations-witnessed
[ack-owner]: ../export-recovery/catalog.md#rp21-ack-follows-local-release
[progress-owner]: ../export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge
[export-faults]: ../export-recovery/fault-map.md
[embedding-faults]: ../embedding/fault-map.md
