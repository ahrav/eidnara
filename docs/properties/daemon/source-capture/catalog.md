# Source capture: catalog

Method: [`../../METHOD.md`](../../METHOD.md). The records come from the
property catalog published with the source-capture specification
([#1066](https://github.com/ahrav/eidnara/issues/1066), comments 1 and 2),
which was verified at baseline `08de9051a`. Each record here was re-verified
against the code and tests of the finished U1 to U10 stack, the tree this
catalog lands on. References name files, functions, constants, and tests
rather than line numbers, because the stack is still under review; where a
baseline citation moved or no longer exists, the record's evidence file says
so in its investigation log.

Abbreviations: `hs.rs` is `crates/daemon/src/harness_sources.rs`; `hs-tests`
is `crates/daemon/tests/harness_sources.rs`; `sc.rs` is
`crates/daemon/src/source_capture.rs`; `sc-tests` is
`crates/daemon/src/source_capture_tests.rs`.

## Scope

The source-capture path in `crates/daemon` and the Kernel surfaces it rests
on in `crates/kernel`: consent and the after-response handoff
(`source_capture.rs`), the harness adapters and the screened publisher
(`harness_sources.rs`), the tool outcome record and summarizer digest
(`tool_outcome.rs`, `history_summarizer_chunk.rs`, `boundary.rs`),
repository-document capture at the m0 compose read (`project_docs.rs`),
session-delete retirement (`lib.rs`), and the Kernel's identity, descriptor,
retention, expiry, GC, admission, and disclosure rules. The boundary itself
is described in [`docs/source-capture.md`](../../../source-capture.md).

## Reachability classes

- `default-production`: reached by a daemon running `DaemonConfig::default()`.
- `explicit-config-only`: reached only when a user opens a gate that ships
  closed. Source capture is gated by the user-tier key
  `source_capture.enabled`, which defaults to false, so every capture-path
  record is in this class.
- `test-only`: constructed only by a test.

## Status

Part A records describe behavior that existed at the baseline. Two of them
are `invalidated` because a unit of the stack inverted them; each names the
record that now carries its obligation. Part B records were `proposed` in the
specification and are `active` here because each has a landed check that was
read at authoring time. The gap records cover the resource bounds the
specification's evaluation queued and the R1 reach and single-store stop
condition this catalog's evaluation queued.

## Index

| Slug | Part | Unit | Supports | Type | Reachability | Status | Check | Exercised |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| [`sc-adapter-identity-native-or-refused`](#sc-adapter-identity-native-or-refused) | A | U3 | R4 | safety | explicit-config-only | active | `always` | yes |
| [`sc-harness-classes-have-no-production-publisher`](#sc-harness-classes-have-no-production-publisher) | A | U2 | R1 | safety | explicit-config-only | invalidated | `always(!X)` | yes |
| [`sc-receipt-replays-equal-digest-conflicts-otherwise`](#sc-receipt-replays-equal-digest-conflicts-otherwise) | A | U5 | D2, D7 | safety | default-production | active | `always` | partial |
| [`sc-admission-fixed-class-pair-under-event-other`](#sc-admission-fixed-class-pair-under-event-other) | A | baseline | R12 | safety | default-production | active | `always` | partial |
| [`sc-unprovenanced-text-stores-sensitive-or-refuses-secrets`](#sc-unprovenanced-text-stores-sensitive-or-refuses-secrets) | A | baseline | R13 | safety | default-production | active | `always` | yes |
| [`sc-sensitive-denied-remote-hidden-from-auto-surfaces`](#sc-sensitive-denied-remote-hidden-from-auto-surfaces) | A | baseline | R13 | safety | default-production | active | `always` | yes |
| [`sc-retire-evidence-conflicts-while-cited`](#sc-retire-evidence-conflicts-while-cited) | A | baseline | D6, C11 | safety | default-production | active | `always` | yes |
| [`sc-curator-capture-requires-finite-live-deadline`](#sc-curator-capture-requires-finite-live-deadline) | A | U4 | R11 | safety | default-production | active | `always` | yes |
| [`sc-capture-expiry-class-scoped-paged-citation-safe`](#sc-capture-expiry-class-scoped-paged-citation-safe) | A | U4 | D6 | safety | default-production | active | `always` | yes |
| [`sc-live-evidence-reference-blocks-artifact-gc`](#sc-live-evidence-reference-blocks-artifact-gc) | A | U4 | R11, C11 | safety | default-production | active | `always` | partial |
| [`sc-occurrence-class-set-frozen-across-mirrors`](#sc-occurrence-class-set-frozen-across-mirrors) | A | U9 | D12 | safety | default-production | active | `always` | yes |
| [`sc-descriptor-revision-exceeds-live-predecessor-only`](#sc-descriptor-revision-exceeds-live-predecessor-only) | A | U9 | D12, C9 | safety | default-production | active | `always` | yes |
| [`sc-reduction-drops-body-keeps-failure-variant`](#sc-reduction-drops-body-keeps-failure-variant) | A | baseline | AE2 | safety | default-production | active | `always` | yes |
| [`sc-tool-call-line-carries-no-outcome`](#sc-tool-call-line-carries-no-outcome) | A | U6 | R9 | safety | explicit-config-only | active | `always` | yes |
| [`sc-project-docs-canonical-hash-over-fixed-list`](#sc-project-docs-canonical-hash-over-fixed-list) | A | U10 | R7 | safety | default-production | active | `always` | yes |
| [`sc-ready-snapshot-drops-native-messages`](#sc-ready-snapshot-drops-native-messages) | A | baseline | R2, D1 | safety | default-production | active | `always` | yes |
| [`sc-revision-lead-refused-and-retryable`](#sc-revision-lead-refused-and-retryable) | A | U2 | R4 | safety | explicit-config-only | active | `always` | yes |
| [`sc-readmission-blocked-after-purge-tombstone`](#sc-readmission-blocked-after-purge-tombstone) | A | U4 | R11 | safety | default-production | active | `always` | partial |
| [`sc-session-delete-leaves-kernel-descriptors`](#sc-session-delete-leaves-kernel-descriptors) | A | U8 | R6 | safety | default-production | invalidated | `always` | yes |
| [`sc-unfinished-assistant-text-deferred`](#sc-unfinished-assistant-text-deferred) | A | U2, U5 | R1 | safety | explicit-config-only | active | `always` | yes |
| [`sc-consent-gate-admits-capture`](#sc-consent-gate-admits-capture) | B | U1, U2 | R3 | safety | explicit-config-only | active | `always` | partial |
| [`sc-response-independent-of-publication`](#sc-response-independent-of-publication) | B | U2 | R2 | safety | explicit-config-only | active | `always` | partial |
| [`sc-one-outstanding-batch-refuse-newest`](#sc-one-outstanding-batch-refuse-newest) | B | U2 | R14 | safety | explicit-config-only | active | `always` | yes |
| [`sc-unit-cap-refuses-whole-never-prefix`](#sc-unit-cap-refuses-whole-never-prefix) | B | U1 | R10 | safety | explicit-config-only | active | `always` | partial |
| [`sc-terminal-disposition-never-retried`](#sc-terminal-disposition-never-retried) | B | U2 | R4 | safety | explicit-config-only | active | `always` | partial |
| [`sc-pending-unit-publishes-next-ready-pass`](#sc-pending-unit-publishes-next-ready-pass) | B | U2 | R1 | liveness | explicit-config-only | active | `always` | partial |
| [`sc-capture-creates-no-decision`](#sc-capture-creates-no-decision) | B | U2, U10 | R12, R13 | safety | explicit-config-only | active | `always(!X)` | partial |
| [`sc-capture-logs-carry-no-text`](#sc-capture-logs-carry-no-text) | B | U2 | C12 | safety | explicit-config-only | active | `always` | yes |
| [`sc-harness-capture-deadline-replay-never-renews`](#sc-harness-capture-deadline-replay-never-renews) | B | U4 | R11 | safety | explicit-config-only | active | `always` | partial |
| [`sc-expiry-descriptor-first-cited-survives`](#sc-expiry-descriptor-first-cited-survives) | B | U4 | R11 | safety | explicit-config-only | active | `always` | yes |
| [`sc-expiry-drains-within-page-bound`](#sc-expiry-drains-within-page-bound) | B | U4 | R11 | liveness | explicit-config-only | active | `always` | yes |
| [`sc-outcome-observed-bounded-occurrence-bound`](#sc-outcome-observed-bounded-occurrence-bound) | B | U5 | R8 | safety | explicit-config-only | active | `always` | yes |
| [`sc-v1-receipt-replays-legacy`](#sc-v1-receipt-replays-legacy) | B | U5 | R8 | safety | explicit-config-only | active | `always` | yes |
| [`sc-digest-bounded-deterministic-char-boundary`](#sc-digest-bounded-deterministic-char-boundary) | B | U6 | R9 | safety | explicit-config-only | active | `always` | yes |
| [`sc-digest-citations-never-reach-artifact`](#sc-digest-citations-never-reach-artifact) | B | U6 | R9 | safety | explicit-config-only | active | `always(!X)` | partial |
| [`sc-rerun-exactly-one-live-generation`](#sc-rerun-exactly-one-live-generation) | B | U7 | R5 | safety | explicit-config-only | active | `always` | yes |
| [`sc-delete-retires-before-memory-store-delete`](#sc-delete-retires-before-memory-store-delete) | B | U8 | R6 | safety | default-production | active | `always` | yes |
| [`sc-delete-retry-recovers-crash-between-stores`](#sc-delete-retry-recovers-crash-between-stores) | B | U8 | R6 | liveness | explicit-config-only | active | `always` | partial |
| [`sc-deleted-session-not-resurrected-by-queued-capture`](#sc-deleted-session-not-resurrected-by-queued-capture) | B | U8 | R6 | safety | explicit-config-only | active | `always(!X)` | yes |
| [`sc-repodoc-one-live-revision-above-historical-head`](#sc-repodoc-one-live-revision-above-historical-head) | B | U9, U10 | R7 | safety | explicit-config-only | active | `always` | yes |
| [`sc-repodoc-raw-bytes-separate-from-prompt-hash`](#sc-repodoc-raw-bytes-separate-from-prompt-hash) | B | U10 | R7 | safety | explicit-config-only | active | `always` | yes |
| [`sc-repodoc-only-verified-missing-retires`](#sc-repodoc-only-verified-missing-retires) | B | U10 | R7 | safety | explicit-config-only | active | `always` | yes |
| [`sc-consented-window-reaches-publisher`](#sc-consented-window-reaches-publisher) | gap | U2, U3, U10 | R1 | reachability | explicit-config-only | active | `reachable` | yes |
| [`sc-capture-state-has-one-durable-store`](#sc-capture-state-has-one-durable-store) | gap | U2 | stop condition | safety | explicit-config-only | active | `always` | yes |
| [`sc-capture-resource-bounds`](#sc-capture-resource-bounds) | gap | U1, U2, U5, U10 | R8, R10, R14 | safety | explicit-config-only | active | `always` | partial |

Semantics distribution: 40 `always`, 4 `always(!X)`, and 1 `reachable` over
45 records (active records: 39, 3, and 1); 3 of the `always` records are
liveness records with an explicit bound. The eight `sometimes` coverage markers are registered
in `crates/eval-core/src/markers.rs` and listed in
[`fault-map.md`](fault-map.md#coverage-witnesses).

## Records

## Part A: existing behavior

### sc-adapter-identity-native-or-refused

Type: safety
Reachability: explicit-config-only - the adapters' only production caller is `CaptureJob::units` in `sc.rs`, reached behind `source_capture::consents`, whose `source_capture.enabled` key defaults to false at user tier.
Status: active
Exercised: yes - `hs-tests` `adapters_bind_native_identity_exactly_and_refuse_missing_identity` builds OpenCode and Pi records, asserts each identity tuple in class field order, and asserts every refusal arm including the `{id, message}` Pi row (`MissingIdentity("type")`); `crates/daemon/tests/eval_ingestion.rs` `each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none` flips identity fields and payload fields one at a time; `sc-tests` `pi_entry_identity_on_the_row_decides_capture_and_never_the_response` drives rows without `type` through the capture path and counts three `adapter_refused`.
Guarantee: Every unit's identity values are the harness's own identifiers (OpenCode `info.id`, `info.sessionID`, `callID`, `state.time.end`; Pi `id`, `parentId`, `toolCallId`, `message.timestamp`), and a record lacking any one, or a Pi row without `type: "message"`, yields an `AdapterRefusal` and no unit.
Check: `always` - `opencode_units`/`pi_units` returns units whose identity follows the class field order with values read from the record, or `Err(NotAnObject | MissingIdentity | SessionMismatch | MalformedRevision | UnsupportedShape)` with no unit; no identity value moves when only text, role, or tool output changes. Each call is a pure function of one record, so the property is evaluated per call.
Fault/timing angle: none.
Required faults and enabling state: records missing `id`, `sessionID`, `role`, `time`, `callID`, `state.time.end`, `parentId`, `toolCallId`, `message`, or `timestamp`; a foreign `sessionID`; a Pi row with a missing, null, empty, or non-`"message"` `type` (F19); an identity value over 512 bytes (F2, refused by `SourcePublisher::prepare`, not by the adapter).
Confidence: high - [evidence](evidence/sc-adapter-identity-native-or-refused.md). Both adapters read in full at HEAD; each refusal arm matched to an assertion in the three tests named above; the production caller and consent gate read.
Existing check: `hs-tests` `adapters_bind_native_identity_exactly_and_refuse_missing_identity`, `pi_refuses_unsupported_message_roles`, `opencode_refuses_unsupported_roles_before_projecting_any_parts`, `pi_tool_results_require_boolean_error_status`, `opencode_refuses_pi_namespace`, `pi_refuses_opencode_namespace`, `opencode_requires_parts_array`, `refused_bytes_and_identities_retain_nothing` (600-byte identity); `eval_ingestion.rs` `each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none`; `sc-tests` `pi_entry_identity_on_the_row_decides_capture_and_never_the_response`; unaudited.
Impact: A payload-derived identity would alias distinct native occurrences or split one occurrence across passes (R4).
Open questions:
- Disposition name for a Pi row without `type` (OQ10). Resolved in U3: `pi_units` returns `MissingIdentity("type")` for a missing, null, or empty `type` and `UnsupportedShape("type")` for any other value; `CaptureJob::units` counts both as `adapter_refused`. Verified in `ticket-1083.md` and in `pi_units`.

### sc-harness-classes-have-no-production-publisher

Type: safety
Reachability: explicit-config-only - since U2 the production publisher for `messages` and `raw_tool_spans` exists and runs when `source_capture.enabled` and `memory.enabled` are both true; under `DaemonConfig::default()` the key is false and no harness descriptor is published.
Status: invalidated - U2 (#1082) added the production publisher: `Handler::capture_after_response` in `crates/daemon/src/lib.rs` hands a consented window to `SourceCapture::offer`, and `CaptureJob::publish_locked` in `sc.rs` calls `publish_screened`, which publishes `messages` and `raw_tool_spans` descriptors. The publication bound now lives in `sc-consent-gate-admits-capture`, which requires that publication happen only under the consent snapshot; the R1 reach obligation (every text block, tool output, and tool error of a consented window reaches `publish_screened`) is carried by `sc-consented-window-reaches-publisher`, the record that closes the E2 gap in `portfolio-evaluation.md`.
Exercised: yes - `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` publishes `messages` and `raw_tool_spans` descriptors through the production handler, which refutes the baseline claim; `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` shows the key off, memory off, and subagent cases leave the kernel tip unchanged.
Guarantee: At baseline `08de9051a` a production daemon published no `messages` or `raw_tool_spans` descriptor; at HEAD it publishes them whenever consent admits capture.
Check: `always(!X)` - historical: after production transform passes, the live descriptor inventory for `messages` and `raw_tool_spans` is empty. This no longer holds with the key on; the replacement is the consent predicate in `sc-consent-gate-admits-capture`.
Fault/timing angle: none.
Required faults and enabling state: a daemon with `source_capture_enabled: true` and `memory_enabled: true`, a non-subagent session on a native serializer profile, and one transform pass with a user text block and a tool result.
Confidence: high - [evidence](evidence/sc-harness-classes-have-no-production-publisher.md). Production call chain read from `lib.rs` to `publish_screened`; the inverting test read.
Existing check: `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` and `capture_off_memory_off_or_a_subagent_session_runs_no_adapter`; `crates/eval-core/src/manifest.rs` `Ingestion::AdapterIngestedNoProductionCaller` (label only; its serde name still reads `production caller: none`, and its doc comment now says the evaluator drives its adapters outside their production callers); unaudited.
Impact: Any check or label that still relies on "no production publisher" misstates what a capture-enabled store holds.
Open questions: None.

### sc-receipt-replays-equal-digest-conflicts-otherwise

Type: safety
Reachability: default-production - `publish_batch` runs in production through `ClaimMaterializer` (`crates/daemon/src/claim_sources.rs`), and the capture path reaches it through `publish_screened` behind the consent key.
Status: active
Exercised: partial - `hs-tests` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically` constructs replay with an unchanged tip and no staged artifact, and `IdentityReused` for changed text and another scope; `publication_receipt_conflicts_on_provider_egress_change` and `publication_receipt_distinguishes_delimiters_inside_fields` cover egress and delimiter inputs; `publication_receipt_digest_layout_is_pinned` pins three `source-request.v2` digests; `receipts_written_before_the_outcome_record_replay_under_v2` pins the frozen v1 digest and replays v1 receipts; `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` replays after memo loss and `a_concurrent_publication_of_a_unit_replays_or_conflicts` publishes the same unit from a second publisher between the handoff and the batch. A receipt committed between the preview and the commit is not constructed.
Guarantee: A unit republished under an identical publisher context answers `replayed: true` from `source-descriptor:{key}` with no new commit; a change to any digest input (format tag, text, domain, scope, role, asserted sensitivity, egress, outcome record) under the same key refuses `IdentityReused` and retains nothing; a receipt written under the frozen `source-request.v1` digest replays.
Check: `always` - a second `publish` of an equal unit returns `Ok(Published { replayed: true, object_id == first })` with `tip()` and `staged_artifacts_for_test()` unchanged; any digest-input change returns `Err(IdentityReused)` with both unchanged; a batch whose key holds a receipt under neither digest returns `BatchRefusal { error: Conflict, unit: Some(i) }`. Equality on receipt state is checkable after each call, so `always`.
Fault/timing angle: a receipt committed by another writer after the preview and before the commit: `publish_within` returns the `(Ok(receipt), None)` replay arm; `publish_batch` returns `BatchRefusal { error: Conflict, unit: None }` when its main key replayed, and treats a per-unit receipt `Conflict` inside the commit as unattributed so the next preview decides replay or conflict.
Required faults and enabling state: restart or memo loss between publications (F3); a publisher field, text, or outcome change at one identity and revision (F4); a second writer of the same unit (F5), both before the preview (constructed) and between preview and commit (not constructed).
Confidence: high - [evidence](evidence/sc-receipt-replays-equal-digest-conflicts-otherwise.md). `receipt_key`, `request_digest`, `digest_inputs`, `stored_receipt`, `publish_within`, and `publish_batch` read at HEAD; each test above read.
Existing check: `hs-tests` `publication_receipt_digest_layout_is_pinned`, `publication_receipt_conflicts_on_provider_egress_change`, `publication_receipt_distinguishes_delimiters_inside_fields`, `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`, `capture_batch_names_the_unit_whose_receipt_conflicts`, `receipts_written_before_the_outcome_record_replay_under_v2`; `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` (replay after memo loss; the `replay_after_memo_loss` marker is recorded in `a_rerun_keeps_one_live_generation_across_a_restart`), `changed_bytes_under_one_identity_are_terminal_and_never_retried`, `a_concurrent_publication_of_a_unit_replays_or_conflicts`; unaudited.
Impact: A digest change under an unchanged key would make every stored receipt conflict; a missing conflict would let changed bytes overwrite an identity's evidence silently.
Open questions:
- How U5 reads a v1 receipt under a v2 digest (OQ9). Resolved in U5: `stored_receipt` probes the `source-request.v2` digest, then the frozen `LEGACY_REQUEST_DIGEST_FORMAT` (`source-request.v1`) digest, for both the descriptor and the evidence receipt, and a key holding neither is `Conflict`. Verified in `ticket-1085.md`, `stored_receipt`, and `receipts_written_before_the_outcome_record_replay_under_v2`.
- Construction of a receipt committed between the preview and the commit (the remaining F5 window): unresolved, needs a commit hook or a second store handle that commits inside that window.

### sc-admission-fixed-class-pair-under-event-other

Type: safety
Reachability: default-production - `SourcePublisher::apply` records `EventKind::Other` for every publication, including `ClaimMaterializer` batches in `claim_sources.rs`; the fixed pairs for `messages` and `raw_tool_spans` are reached in production only behind the consent key, and `git_commits` and `repository_documents` pairs through their publishers.
Status: active
Exercised: partial - `hs-tests` `mixed_sessions_create_pending_only_for_message_text` asserts `Published.admission` and the durable `admission_decisions` classes for user, assistant, and tool units; `crates/daemon/tests/git_sources.rs` `selected_commits_are_retained_exactly_and_rebuild_without_the_repository` and `hs-tests` `repository_documents_publish_untrusted_supersede_and_project` assert the repository pair; `crates/kernel/tests/kernel_admission.rs` `an_inert_citation_does_not_consume_approval_capacity` asserts `Other` leaves `Maturity::Candidate`; `sc-tests` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` asserts no decision and AutoInject hidden after reopen. No test reads back `event_kind`, `candidate_id`, or `approval_object_id` on a publication's admission row.
Guarantee: Every publication records one admission with `EventKind::Other`, no candidate or approval id, and a class pair fixed by (class, role); `Other` yields `Outcome::Deny` and changes neither maturity nor disposition.
Check: `always` - `Published.admission` equals the fixed pair (`raw_tool_spans` -> `(TrustedToolResult, ToolUntrustedOutput)`; `git_commits` and `repository_documents` -> `(UntrustedRepoText, RepoUntrustedText)`; `messages` with role `user` -> `(ExplicitUser, UserExplicit)`; other `messages` -> `(ModelInference, AssistantInference)`), and the subject's maturity after publication equals its prior value. A table lookup per publication, so `always`.
Fault/timing angle: the pair is resolved inside the commit by `ProvenanceRule::resolve`, so an admission change between preview and commit is not carried forward.
Required faults and enabling state: one unit per (class, role); a fresh subject; a kernel reopen before reading maturity and visibility.
Confidence: high - [evidence](evidence/sc-admission-fixed-class-pair-under-event-other.md). `admission_rule`, `ProvenanceRule::resolve`, `SourcePublisher::apply`, and `event_effect` in `crates/kernel/src/admission.rs` read; the tests above read.
Existing check: `hs-tests` `mixed_sessions_create_pending_only_for_message_text`, `repository_documents_publish_untrusted_supersede_and_project`; `git_sources.rs` `selected_commits_are_retained_exactly_and_rebuild_without_the_repository`; `kernel_admission.rs` `an_inert_citation_does_not_consume_approval_capacity`, `visible_reads_exclude_decisionless_objects_and_apply_maturity_rows`; `sc-tests` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` and `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` (tool pair through the capture path); unaudited.
Impact: R12 and the event-kind stop condition rest on this table; a promoting event kind would raise captured text to automatic visibility.
Open questions: None.

### sc-unprovenanced-text-stores-sensitive-or-refuses-secrets

Type: safety
Reachability: default-production - `KernelStore::ingest_exact_artifact` and `ingest_exact_artifacts_with` are called in production by `SourcePublisher::publish_within`, `publish_batch` (through `ClaimMaterializer`), and `crates/daemon/src/memory_reviewer/project_text.rs` and `lifecycle.rs`; the capture path reaches them behind the consent key.
Status: active
Exercised: yes - `crates/kernel/tests/kernel_exact_artifacts.rs` `a_secret_is_refused_before_persistence_with_a_reason_that_names_no_content` and `oversized_and_non_utf8_payloads_are_refused_before_materialization` construct the exact-lane refusals; `crates/kernel/tests/kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` constructs the clamp on the redacting lane; `hs-tests` `refused_bytes_and_identities_retain_nothing` refuses a secret tool output with the tip and inventory unchanged; `mixed_sessions_create_pending_only_for_message_text` asserts each descriptor's sensitivity equals its evidence object's and both are `Sensitive`.
Guarantee: An exact ingest of UTF-8 text asserting `Normal` without affirmative repository provenance stores `Sensitive`; a payload the scanner would rewrite refuses `ExactBytesRewritten` and retains nothing; a non-UTF-8 exact payload refuses `UnsupportedShape`; the descriptor records the stored class.
Check: `always` - after `publish`, the evidence object reports `Sensitive` for asserted `Normal` with `provenance: None`; a secret-bearing block returns `Err(Artifact(ExactBytesRewritten))` with `tip()` unchanged and an empty inventory; each exported row's `sensitivity` equals its evidence object's. Evaluated per ingest.
Fault/timing angle: none.
Required faults and enabling state: a recognizable secret (F8); asserted `Normal`; `provenance: None` (every unit except `git_commits`, per `git_sources::commit_provenance`); a non-UTF-8 payload at the kernel boundary (a `SourceUnit` carries `String` text, so the publisher cannot offer one).
Confidence: high - [evidence](evidence/sc-unprovenanced-text-stores-sensitive-or-refuses-secrets.md). `PreparedArtifact::new` in `crates/kernel/src/cas/ingest.rs`, `evidence_request`, `stored_sensitivity`, and the batch path read; tests read.
Existing check: `kernel_exact_artifacts.rs` `a_secret_is_refused_before_persistence_with_a_reason_that_names_no_content`, `oversized_and_non_utf8_payloads_are_refused_before_materialization`; `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive`; `hs-tests` `refused_bytes_and_identities_retain_nothing`, `mixed_sessions_create_pending_only_for_message_text`; `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` (a captured tool row is `Sensitive`); unaudited.
Impact: R13 rests here; an unclamped `Normal` row would be remotely eligible, and a rewritten secret would store bytes that differ from the source.
Open questions:
- In the batch path a secret-bearing unit fails `PreparedArtifact::new` inside `ingest_exact_artifacts_with`, which maps the failure to `KernelError::InvalidInput` before the commit closure runs, so `BatchRefusal.unit` is `None`. `CaptureJob::publish_locked` then records every keyed unit of the batch as `Disposition::Pending` and counts `kernel_error`; the pending entries are offered ahead of unknown units on the next pass and age `oldest_deferred_age_ms` until they publish or the memo evicts them. Whether one secret-bearing tool output blocks its whole batch on every pass: unresolved, needs a capture-path test with a secret-bearing tool output beside a clean unit.

### sc-sensitive-denied-remote-hidden-from-auto-surfaces

Type: safety
Reachability: default-production - `Sensitivity::denies_destination`, `eligibility_for`, `surface_visibility`, and `judge` run on every artifact eligibility read, visible-surface read, and `judge_eligibility` call, and `crates/daemon/src/lib.rs` dispatches `"kernel.egress.decide"` to `handle_kernel_egress_decide` with no gate. Captured `Sensitive` objects themselves exist only with `source_capture.enabled` (`DaemonConfig::default` sets `source_capture_enabled: false`).
Status: active
Exercised: yes - `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` constructs unprovenanced and non-UTF-8 artifacts and reads both destinations; `admission.rs` unit test `visibility_matrix_and_sensitivity_ceiling_are_total` walks every row and surface; `kernel_admission.rs` `dispositions_and_sensitivity_only_reduce_visibility` reads `AutoSearch` and `ExplicitSearch`; `kernel_eligibility.rs` `every_verdict_class_and_precedence_pair_matches_the_expected_table` judges `secret` and `sensitive` candidates at both destinations; `kernel_routes.rs` `egress_gate_judges_the_owner_by_its_served_class` drives the route for a tainted owner; `source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` reads real captures through `AutoInject` after a reopen; that capture-path read does not distinguish `Sensitive` hiding from decisionless hiding.
Guarantee: `Sensitive` is denied the `Remote` destination and allowed `Local`, `Secret` is denied both, and `Sensitive` is hidden from `AutoInject` and `AutoSearch` at every policy layer.
Check: `always` - `Sensitive.denies_destination(Remote) == Some(SensitiveRemote)` and `(Local) == None`; `Secret.denies_destination(_) == Some(Secret)`; `surface_visibility(row, AutoInject|AutoSearch, Sensitive) == Hidden` for every row; `judge` with served `Sensitive` at `Remote` returns `ProviderSensitive`; the egress route turns `Allowed` into `Refused(OwnerSensitive)` for a served `Secret` owner, or a non-`Normal` served owner at `Remote`, with zero provider requests. `always` because these are pure policy functions evaluated on every read.
Fault/timing angle: the egress route reads artifact class and owner citation in one `egress_candidates` snapshot, so a reclassifying ingest cannot land between them.
Required faults and enabling state: a `Sensitive` object and artifact; a `Remote` read (F21); `AutoInject` and `AutoSearch` reads.
Confidence: high - [evidence](evidence/sc-sensitive-denied-remote-hidden-from-auto-surfaces.md). Read every function above and the six tests at HEAD, including the eligibility table the source had left unread.
Existing check: `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive`; `admission.rs` `visibility_matrix_and_sensitivity_ceiling_are_total`; `kernel_admission.rs` `dispositions_and_sensitivity_only_reduce_visibility`; `kernel_eligibility.rs` `every_verdict_class_and_precedence_pair_matches_the_expected_table`; `kernel_routes.rs` `egress_gate_judges_the_owner_by_its_served_class`; `source_capture_tests.rs` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`; all `unaudited`.
Impact: captured exact bytes reach a remote model or an automatic surface, which violates R13 and the #708 disclosure stop condition.
Open questions:
- May a future read tool disclose captured tool errors to the model, and under which gate (OQ2)? (needs human input)

### sc-retire-evidence-conflicts-while-cited

Type: safety
Reachability: default-production - `memory_reviewer::lifecycle::kernel_slice` calls `expire_local_file_captures` on every lifecycle pass while the Kernel is ready, and its `retire_expired_capture` calls `retire_evidence`; `lifecycle::run` is spawned from `crates/daemon/src/lib.rs` once the Memory Store opens. The harness publisher's compensating `SourcePublisher::retire_evidence` and the harness sweep reach it only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_source_descriptors.rs` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence` retains an uncited row and a row a live descriptor cites, retires each, and asserts the outcome, tip, and liveness; `harness_sources.rs` `permanent_refusal_reports_failed_retirement_attempt` injects a retirement failure through a trigger.
Guarantee: `Envelope::retire_evidence` refuses with `Conflict` while any live observation, decision, decision event of a live decision, or live asserted edge cites the evidence, the commit rolls back, and capture pins are not consulted.
Check: `always` - retiring cited evidence returns `Err(Conflict)` with `tip()` unchanged, the citing descriptor live, and `invalidated_commit_seq == None`; retiring uncited evidence returns `Ok`, sets `invalidated_commit_seq` to the commit, and records `evidence_retire`; a second retirement is `NotFound`. `always` because the predicate `EVIDENCE_CITED_SQL` is evaluated on every call.
Fault/timing angle: check and invalidation share one writer transaction. `SourcePublisher::retire_evidence` treats `NotFound` on an already invalidated object as retired, so a replayed compensation is idempotent.
Required faults and enabling state: a live citing descriptor, then its retirement (F13); a pinned but uncited evidence row for the pin half.
Confidence: high - [evidence](evidence/sc-retire-evidence-conflicts-while-cited.md). Read `retire_evidence_inner`, `EVIDENCE_CITED_SQL`, the sweep's `cited_elsewhere`, the publisher wrapper, and both tests. The pin half rests on code reading.
Existing check: `kernel_source_descriptors.rs` `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence`; `harness_sources.rs` `permanent_refusal_reports_failed_retirement_attempt`; `harness_sources.rs` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` (cited arm); all `unaudited`.
Impact: KTD6's "cites or holds" is citations only; every expiry sweep carries its own pin predicate (D6, C11). A regression would let expiry remove independently cited evidence.
Open questions:
- Should a test retire pinned, uncited evidence directly to pin down that `retire_evidence` ignores holds? (unresolved, needs a Kernel test)

### sc-curator-capture-requires-finite-live-deadline

Type: safety
Reachability: default-production - the checks sit in Kernel ingest (`cas/ingest.rs` request validation and `insert_reference`), reached by every `curator_capture` ingest a MemoryReviewer run issues; the `harness_capture` half is reached only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_memory_reviewer_holds.rs` `memory_reviewer_captures_must_carry_a_finite_acquisition_reference` ingests with `None`, `now - 1`, and `now + 1 h`; `a_committed_capture_replays_after_its_retention_deadline` ingests with `now + 200 ms`, sleeps 300 ms, and replays; `local_file.rs` unit test `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` refuses an undated `harness_capture` ingest.
Guarantee: An ingest with `retention_class == "curator_capture"` and `retain_until: None` is refused before any byte is retained, a new reference whose deadline is at or before the store clock is refused at reference commit, and an identical committed request replays after its deadline.
Check: `always` - `ingest_artifact(curator_capture, None)` returns `ArtifactErrorKind::InvalidInput`; `Some(now - 1)` returns `InvalidInput` from `insert_reference`; a committed request replayed after its deadline returns the first handle; `harness_capture` with `None` returns `InvalidInput` with `tip()` unchanged. `always` because validation runs on every ingest.
Fault/timing angle: deadline liveness reads `crate::current_time_ms()` inside the reference commit, so the replay test depends on a 200 ms deadline and a 300 ms sleep against the wall clock, a flake risk under host load.
Required faults and enabling state: `None` and past deadlines; a committed request replayed after expiry (F11).
Confidence: high - [evidence](evidence/sc-curator-capture-requires-finite-live-deadline.md). Read both ingest sites and three tests; confirmed with `git grep` that the replay test existed at baseline.
Existing check: `kernel_memory_reviewer_holds.rs` `memory_reviewer_captures_must_carry_a_finite_acquisition_reference`; `kernel_memory_reviewer_holds.rs` `a_committed_capture_replays_after_its_retention_deadline`; `local_file.rs` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` (undated arm); all `unaudited`.
Impact: a capture without a finite live deadline never enters expiry and holds bytes indefinitely. `harness_capture` mirrors only the finite-deadline half; a past harness deadline commits and is due at the next sweep.
Open questions:
- Should ingest liveness use an injected clock so the replay test needs no sleep? (unresolved, needs a `/testing:test-strategy` decision)

### sc-capture-expiry-class-scoped-paged-citation-safe

Type: safety
Reachability: default-production - `kernel_slice` runs `expire_local_file_captures` then `expire_harness_captures` before `run_staging_maintenance` on every lifecycle pass while the Kernel is ready; `lifecycle::run` is spawned with no gate once the Memory Store opens. Harness rows exist only with `source_capture.enabled`.
Status: active
Exercised: yes - `local_file.rs` unit tests construct a pin taken after selection, a seated receipt under the reserved producer, a replayed expiry receipt, a purge-degraded pin, 512 retired rows of history, and a harness row beside a curator row; `memory_reviewer_project_text.rs` `expiry_retires_the_capture_and_its_detail_but_keeps_independent_support`, `retained_captures_do_not_starve_newer_expired_captures` (65 captures, 64 cited), and `a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep` (64 non-evidence rows ahead of one capture) drive the public sweep.
Guarantee: One `expire_local_file_captures` call retires at most 64 expired, unpinned `curator_capture` captures, oldest `retain_until` first, each in its own reserved-producer commit, keeps evidence another live row cites, and never touches another retention class.
Check: `always` - after one call with N expired unpinned captures, `retired + retained <= min(N, 64)`; every retired row had the class and `retain_until <= now` at its commit; pinned or future-deadline captures are unchanged; evidence cited elsewhere stays live and counts as `retained`, then leaves the candidate set; `canonical` and `harness_capture` rows are unchanged. Across calls, N expired unpinned uncited captures clear within `ceil(N / 64)` calls (the liveness half `sc-expiry-drains-within-page-bound` generalizes). `always` because every sweep call must satisfy it.
Fault/timing angle: a pin acquired between the reader-side `expired_captures` query and the writer-side `EXPIRED_UNPINNED_SQL` recheck keeps the capture as `Conflict`; a refused row is skipped and the page continues.
Required faults and enabling state: more than 64 expired captures (F14); a pin mid-sweep (F12); evidence cited independently (F13); other classes in the same store. Coverage witnesses `expired_capture_pinned_mid_sweep` and `expired_capture_cited_elsewhere` mark the F12 and F13 preconditions.
Confidence: high - [evidence](evidence/sc-capture-expiry-class-scoped-paged-citation-safe.md). Read `expire_captures`, `retire_expired_capture`, `EXPIRED_UNPINNED_SQL`, `expired_captures_sql`, `kernel_slice`, and nine tests.
Existing check: `local_file.rs` `a_caller_cannot_seat_a_receipt_the_expiry_would_replay`, `a_replayed_expiry_receipt_is_not_counted_as_a_retirement`, `a_capture_pinned_after_it_was_selected_for_expiry_is_kept`, `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`, `a_purge_degraded_pin_does_not_keep_a_lapsed_capture`, `expiry_candidate_cost_does_not_grow_with_retired_history`; `memory_reviewer_project_text.rs` `expiry_retires_the_capture_and_its_detail_but_keeps_independent_support`, `retained_captures_do_not_starve_newer_expired_captures`, `a_capture_the_store_refuses_to_retire_does_not_stall_the_sweep`; all `unaudited`.
Impact: U4 reuses this mechanism through `ExpiringClass`, so a defect here reaches both classes. The sweep runs as a sibling of `run_staging_maintenance` in `kernel_slice`; a failing curator sweep skips the harness sweep and staging maintenance for that pass.
Open questions: None.

### sc-live-evidence-reference-blocks-artifact-gc

Type: safety
Reachability: default-production - `run_staging_maintenance` runs `run_artifact_gc` from `kernel_slice` on every lifecycle pass while the Kernel is ready.
Status: active
Exercised: partial - `kernel_gc.rs` covers a live `canonical` row at `now = 13 * DAY_MS`, invalidation grace, the `retain_until` floor, overflow at `i64::MAX`, pin and pin-release grace, pin expiry, and the pending-purge override; no test sweeps a live row at a far-future `now`.
Guarantee: Outside a pending purge, GC never reclaims a digest while any `evidence_meta` row with that digest is live or any unreleased pin reference names it, and after invalidation keeps the bytes until the latest of `MAX(retain_until)`, `invalidated_at + 14 days`, and `pin_released_at + 14 days`.
Check: `always` - for every candidate without a pending unlink, a live evidence row or an unreleased pin reference yields `Withheld::Retained` at any `now`; an invalidated digest is reclaimed only when `now >= deadline`; an overflowing deadline retains. `always` because every GC candidate is judged by `prepare_reclaim`.
Fault/timing angle: a `Reclaiming` reservation carries the decision across a crash; a current-epoch `Live` reservation with an unexpired lease retains; a pending unlink bypasses every retention check by design.
Required faults and enabling state: harness or canonical evidence kept live while `now` advances years (F15); invalidated evidence inside and beyond 14 days; a held and a released pin; a pending unlink over a live row.
Confidence: high - [evidence](evidence/sc-live-evidence-reference-blocks-artifact-gc.md). Read `prepare_reclaim`, `reclaim_candidate`, and seven GC tests; the far-future live-row case rests on code reading because the live check precedes every clock comparison.
Existing check: `kernel_gc.rs` `referenced_and_recently_invalidated_artifacts_survive`, `invalidated_artifact_reclaims_after_grace_but_backward_clock_keeps_it`, `retain_until_is_an_absolute_floor_not_an_extra_grace_start`, `overflowing_grace_deadline_keeps_artifact`, `active_capture_pin_and_pin_release_grace_protect_artifact`, `maintenance_preserves_only_the_grace_remaining_after_pin_expiry`, `pending_purge_unlinks_a_live_reference_and_sweeps_its_digest_temps`; `harness_sources.rs` `harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` (expiry sweep at `NOW + 32 d`, not GC); all `unaudited`.
Impact: a live descriptor loses its exact bytes, or captured bytes never leave the store; physical retention exceeds `retain_until` by up to the 14-day grace (R11, C11).
Open questions:
- Which existing GC test drives a `canonical` row through a far-future sweep? (unresolved, needs one `kernel_gc` test at a far-future `now` over a live row; none exists at HEAD)

### sc-occurrence-class-set-frozen-across-mirrors

Type: safety
Reachability: default-production - `encode_preserving_span` runs on every descriptor publication and projection rebuild, and the retrieval CHECK applies to every `occurrences` insert.
Status: active
Exercised: yes - `eval_identity.rs` `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` compares the mirrors and encodes 24 fixture records through both encoders; `schema_inventory.rs` `the_check_vocabularies_equal_the_rust_enums` compares the CHECK to `OccurrenceClass::ALL`; `kernel_source_descriptors.rs` `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals` covers `unknown_class` and `precedence_class_before_everything`.
Guarantee: The six-class identity contract (codes and order, field orders, representations, harnesses, object formats, the 512-byte cap, and encoding version 2) is identical in the Kernel, eval-core, the retrieval `occurrences.class` CHECK, and the identity fixtures, and any other class code is refused first.
Check: `always` - the eval-core and Kernel constants and per-class fields are equal; the CHECK equals `OccurrenceClass::ALL` codes; all 24 fixture records encode to their pinned occurrence and lineage ids through both encoders; an unknown class returns `Err(UnknownClass)` before any other refusal. `always` because the contract is static.
Fault/timing angle: none.
Required faults and enabling state: the four mirrors; a class name outside the set.
Confidence: high - [evidence](evidence/sc-occurrence-class-set-frozen-across-mirrors.md). Read `source_identity.rs`, `eval-core/src/occurrence.rs`, `baseline.sql`, and the three tests at HEAD.
Existing check: `eval_identity.rs` `the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder`; `schema_inventory.rs` `the_check_vocabularies_equal_the_rust_enums`; `kernel_source_descriptors.rs` `the_kernel_encoder_matches_golden_identifiers_and_string_payload_refusals`; all `unaudited`.
Impact: two components derive different occurrence ids for one source. U9 grew the set to six and bumped the projection schema to 8; a binary below 8 cannot re-encode `repository_documents`.
Open questions:
- Is `RepositoryDocuments` in scope (OQ1)? Resolved for landing in U9: ticket-1081 names OQ1 as an owner gate and lists no blocker, and the class landed dark. Neither file records a written owner confirmation.

### sc-descriptor-revision-exceeds-live-predecessor-only

Type: safety
Reachability: default-production - `Envelope::publish_verified` runs for every source descriptor publication through `publish_source_descriptors`, including the git and claim publishers; harness classes reach it only with `source_capture.enabled`.
Status: active
Exercised: yes - `kernel_source_descriptors.rs` `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt`, `representations_and_spans_of_one_source_are_separate_lineages`, and `concurrent_successors_serialize_into_one_chain` publish advancing and stale revisions; `retirement_preserves_domain_ownership_without_reusing_old_revision_ids` retires revision 3 and republishes 2 for `messages`.
Guarantee: For every class except `repository_documents`, a revision at or below the live descriptor's in its lineage is refused `RevisionNotAdvanced`, a higher revision invalidates the live row, sets its successor, and returns `replaced_object_id` in one commit, and with no live row a revision below a retired one starts a fresh chain.
Check: `always` - with live revision r: r' <= r returns `Err(RevisionNotAdvanced)` with `tip()` unchanged; r' > r returns `Ok` with `replaced_object_id == Some(live)`, the predecessor's `superseded_by` set, and both changes at one `commit_seq`; with the lineage retired at 3, revision 2 returns `Ok` with `replaced_object_id == None` and revision 3 returns `Kernel(Conflict)`. `always` because every publication passes the check.
Fault/timing angle: two live rows per lineage report `CorruptCanonicalRow`; racing successors serialize into one chain.
Required faults and enabling state: two revisions in either order (F9); a retired lineage followed by a lower revision (F10). Coverage witness `retired_lineage_republished` marks F10 for the guarded class.
Confidence: high - [evidence](evidence/sc-descriptor-revision-exceeds-live-predecessor-only.md). Read `publish_verified`, `live_lineage`, `historical_head`, and five tests; confirmed the retired-lineage test existed at baseline.
Existing check: `kernel_source_descriptors.rs` `a_successor_invalidates_exactly_its_predecessor_in_one_commit_and_replays_from_its_receipt`, `representations_and_spans_of_one_source_are_separate_lineages`, `concurrent_successors_serialize_into_one_chain`, `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`, `repository_document_revisions_exceed_the_historical_head_including_retired_rows` (guarded class); `harness_sources.rs` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`, `permanent_refusal_reports_failed_retirement_attempt`; all `unaudited`.
Impact: a stale revision replaces a newer live one. U9 adds the historical-head guard for `repository_documents` only (`sc-repodoc-one-live-revision-above-historical-head`); every other class keeps this rule.
Open questions:
- Does publishing revision 2 after retiring revision 3 succeed at baseline? Resolved: yes for `messages`, asserted at baseline by `retirement_preserves_domain_ownership_without_reusing_old_revision_ids`.
- Should the historical-head guard apply to every class (OQ13)? Resolved in U9: `repository_documents` only; other classes must pass `expected_head: None`.

### sc-reduction-drops-body-keeps-failure-variant

Type: safety
Reachability: default-production - `apply_once` in `crates/daemon/src/transform.rs` calls `select_reductions_with_outcome` under `producer_gate` on every transform pass, and its rebuild loop applies `wire::reduced_block`; no `source_capture` or other configuration key gates it
Status: active
Exercised: yes - `crates/daemon/src/wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras` constructs all eight `OutputKind` variants with output and block extras; `crates/daemon/src/selection.rs::frozen_arc_blocks_never_re_emitted` freezes one result id; `crates/daemon/src/transform.rs::agent_drop_ids_freeze_add_only_through_flat_ids`, `coverage_extending_soft_prunes_covered_red_units`, `host_reversibility_true_reduction_stays_canonical_after_false_flip`, and `owned_profile_drains_pending_drops_without_request_surface` assert served `[dropped]` bytes; `reduced_block_renders_placeholder_without_tag_prefix` asserts the tagged `[dropped §2§]` form
Guarantee: When reduction drops a tool arc, every unfrozen result block becomes a `[dropped]` placeholder (`[dropped §N§]` under a tag overlay) with its `OutputKind` class preserved (`Text|Json|Content` to `Text`, `Error*` to `ErrorText`, `ExecutionDenied` kept with the reason replaced), its id, tool name, `provider_executed`, and both extras maps kept, and the native output bytes absent.
Check: `always` - every `ReductionDecision` that `expand_arc` emits for a result id has kind `drop` and payload `"[dropped]"`; `reduced_block` of an `Error*` output has kind `ErrorText { text: <placeholder> }`, of `ExecutionDenied` has `reason: Some(<placeholder>)`, and of the rest has `Text`; no reduced block holds a byte of the native output body; no decision names a frozen id; a pure per-block mapping, so it must hold on every evaluation
Fault/timing angle: a frozen result id is skipped by `expand_arc`; a tag overlay changes the served placeholder text
Required faults and enabling state: F20, an arc selected for `FullDrop` or `DedupFullDrop` with each `OutputKind`; a frozen result id; a tag overlay naming the dropped block
Confidence: high - [evidence](evidence/sc-reduction-drops-body-keeps-failure-variant.md). Read `DROPPED_PLACEHOLDER` and `expand_arc` in `selection.rs`, `reduced_block` in `wire.rs`, the `apply_once` rebuild loop in `transform.rs`, and the OpenCode tool-output decode in `codec/opencode.rs`; read each test named above; extras are copied verbatim and their contents are not audited
Existing check: `wire.rs::reduced_tool_result_keeps_failure_variant_and_output_extras`; `selection.rs::frozen_arc_blocks_never_re_emitted`; the five `transform.rs` tests named in `Exercised`; all unaudited
Impact: Collapsing `Error*` to `Text` tells the model a failed call succeeded; once dropped, the body survives only in a retained `raw_tool_spans` source, which at HEAD exists only when `source_capture.enabled` is set (AE2)
Open questions:
- Can a codec place native output bytes in `provider_extras`, which `reduced_block` copies verbatim? (unresolved, needs an extras-population audit per codec)

### sc-tool-call-line-carries-no-outcome

Type: safety
Reachability: explicit-config-only - the summarizer fold requires `DaemonConfig::eidnara_folds`, which needs a non-empty `model_chain`, and `DaemonConfig::default` leaves it empty; `build_history_summarizer_chunk` is called from `maybe_spawn_reattach` in `crates/daemon/src/lib.rs` and from the summarizer firing path in `transform.rs`, and the boundary chunk runs in `check_history_segment_trigger_with_index` in `crates/daemon/src/boundary.rs`; neither reads `source_capture.enabled`; whether the boundary-trigger estimate formats digests under the default configuration is unverified
Status: active
Exercised: yes - `crates/daemon/src/history_summarizer_chunk.rs::tool_output_contains_sentinel` places `SENTINEL-7f3a` on a middle output line and asserts it is absent while `head` and `tail` render; `crates/daemon/src/boundary.rs::the_boundary_chunk_renders_digests_for_answered_calls` does the same at the second site with answered and unanswered calls, with and without narrative; `an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` constructs F17; `a_digest_cut_by_the_budget_withdraws_its_alias` constructs F16; `duplicate_tool_call_ids_resolve_results_by_arc_id` and `tool_result_only_message_absorbs_into_preceding_assistant_block` cover arc lookup and tool-only absorption
Guarantee: At both `TC:` construction sites a tool call renders as `TC: {description}`, `TC: {name}({key_arg})`, or `TC: {name}`, and a tool result renders that summary followed by the outcome digest from `tool_outcome::outcome_digest`; no tool output byte outside the `head` and `tail` excerpts enters the chunk; tool-only parts are `transformed: true`; native marker brackets are escaped; an alias cut by truncation is withdrawn.
Check: `always` - every call summary starts with `TC: `; the key argument is the first of `filePath|path|pattern|query` cut at 60 scalars with `…`, else the first of `symbol|module|action` uncut; no sentinel placed on a tool output line other than the first and last nonblank lines appears in the chunk at either site; every tool-only `FrozenAlias.transformed == true`; after `presented_input` every remaining alias's `marker + presented` occurs in order in the kept prefix; asserted on every built chunk because the chunk is a pure function of its blocks and budget
Fault/timing angle: a token budget cutting inside a digest (F16); a forged alias marker in output (F17)
Required faults and enabling state: tool results whose middle lines carry a sentinel, at both sites (the `tool_output_contains_sentinel` marker is recorded at the chunk-builder site; `the_boundary_chunk_renders_digests_for_answered_calls` covers the boundary site); an error result; F16; F17; an unanswered call in a message with and without narrative
Confidence: high - [evidence](evidence/sc-tool-call-line-carries-no-outcome.md). Read `format_tool_summary`, `tool_parts`, `ChunkBuilder::part`, `presented_input`, `extract_key_arg`, `tool_outcome::{from_output, outcome_digest, digest, append_digests}`, and the boundary `tool_results`, `tool_call_parts`, `chunk_message_part`; read each named test; verified both sites share one formatter and pass `exit_code: None`
Existing check: the six tests named in `Exercised`; `crates/daemon/tests/history_summarizer_truncate_differential.rs::optimized_matches_frozen_reference_at_production_windows` covers the truncation function the withdrawal relies on; all unaudited
Impact: The full log, or a secret in its middle, reaching the summarizer prompt; a citation naming bytes the model never saw whole
Open questions:
- OQ17, second `TC:` site: resolved in U6 (`ticket-1088.md`): both sites render through `format_tool_summary` and `tool_outcome::digest`; verified in `boundary.rs::tool_call_parts` and its test
- The slug name and the baseline guarantee ("no status, exit code, or error text") no longer describe HEAD, where the line carries status, byte count, and two excerpts; should the lead rename the slug or invalidate it in favor of `sc-digest-bounded-deterministic-char-boundary`? (needs human input)

### sc-project-docs-canonical-hash-over-fixed-list

Type: safety
Reachability: default-production - `inject_docs` defaults `true` in `DaemonConfig::default()`; `ConfigKey::MemoryClassifierInjectDocs` (`/memory_classifier/inject_docs`) is `TierClass::UserOnly`; the m0 composer in `crates/daemon/src/m0_compose.rs` calls `read_project_docs_canonical` whenever `inputs.inject_docs` holds; the raw-byte capture beside it is a separate, `explicit-config-only` record
Status: active
Exercised: yes - `crates/daemon/src/project_docs.rs` tests `project_docs_golden_matches_reference` (six golden cases, case 3 holds a BOM, CRLF, trailing space and tab, and trailing newlines), `symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped` (sparse oversize and FIFO), `oversized_doc_is_skipped`, `a_file_is_missing_only_from_a_directory_that_exists`; `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` asserts a whitespace-only edit leaves `canonical_hash` unchanged; `document_capture_leaves_the_response_and_respects_inject_docs` covers `inject_docs: false`
Guarantee: The block covers only `ARCHITECTURE.md` then `STRUCTURE.md`, each read without following a final-component symlink as a regular file of at most 256 KiB of UTF-8 or skipped, and the hash covers a canonical form, so whitespace-only edits change neither hash nor block while raw bytes differ; `M0Composition::docs_hash` has no reader.
Check: `always` - every `crates/daemon/testdata/project-docs-golden.json` case reproduces `rendered_block` and `canonical_hash`; an edit touching only a BOM, CRLF, trailing spaces or tabs, or trailing newlines yields an identical hash and block; a symlink, FIFO, oversize, or non-UTF-8 file is skipped while the other renders; a pure function of two file reads, so it must hold on every evaluation
Fault/timing angle: the open is `NOFOLLOW` and the descriptor type and cap are checked on the opened file, so a swap of the file after the open changes nothing; a parent-directory swap before the open is outside `NOFOLLOW`
Required faults and enabling state: whitespace-only edits; F18 fixtures (symlink, FIFO, sparse oversize, non-UTF-8); `inject_docs: false`
Confidence: high - [evidence](evidence/sc-project-docs-canonical-hash-over-fixed-list.md). Read `PROJECT_DOC_FILES`, `MAX_PROJECT_DOC_BYTES`, `canonicalize_doc_content`, `read_document`, `read_project_docs_canonical`, `leave_reads`, `config::read_bounded_file`, and the composer; read each named test; the golden file's git history shows only its initial import
Existing check: the seven tests named in `Exercised`; all unaudited
Impact: A whitespace-only edit evicting the cached m0 prefix; a symlink or special file exfiltrating bytes into the prompt; the canonical hash is no revision trail, which U10's `RepositoryDocuments` capture supplies (R7)
Open questions:
- `crates/daemon/src/history_segment_coverage.rs` says `docs_hash` is persisted with the rendered m0 bytes: resolved by a grep of `crates/`, `packages/`, SQL, and Markdown at HEAD, which finds no reader or writer beyond the assignment in `m0_compose.rs`; the comment disagrees with the code

### sc-ready-snapshot-drops-native-messages

Type: safety
Reachability: default-production - the transform handler in `crates/daemon/src/lib.rs` builds `Arc::new(parsed.ready_snapshot())` and calls `TransformSnapshotCache::finish_ready` on every pass that reaches ready, with no configuration gate
Status: active
Exercised: yes - `crates/daemon/src/lib.rs::ready_snapshot_shares_messages_and_charges_at_least_their_bytes` sets `native_messages: Some` and asserts `None` on the snapshot, `Arc::ptr_eq` on `messages[0]`, and a charge that grows with message bytes; `ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` sends a native request with a 64 KiB `provider_data` field through the handler and asserts the lease holds no native messages and is charged under 64 KiB
Guarantee: The ready snapshot carries `native_messages: None` and a resolution-only coverage, shares the CK message `Arc`s, and its retained-byte charge excludes native payload; the native window a consented pass hands to source capture lives in the `CaptureHandoff` under its own `RetainedCharge`.
Check: `always` - `request.ready_snapshot().native_messages.is_none()`; `Arc::ptr_eq(&snapshot.messages[i], &request.messages[i])` for every `i`; `snapshot_retained_bytes` excludes native payload; the `debug_assert!` in `finish_ready` holds in debug builds, and release builds enforce nothing at that point; asserted on every snapshot because the cache budgets charge exactly what the snapshot keeps
Fault/timing angle: none
Required faults and enabling state: a request with `native_messages: Some` (`opencode-aisdk` or `pi` profile) and a large native payload; a pass reaching ready
Confidence: high - [evidence](evidence/sc-ready-snapshot-drops-native-messages.md). Read `ready_snapshot`, `snapshot_retained_bytes`, `finish_ready`, the handler's snapshot and `source_capture::Offer` construction, and `SourceCapture::offer`; read both named tests
Existing check: the two `lib.rs` tests named in `Exercised`; the `debug_assert!` in `finish_ready`; `crates/daemon/src/transform/surface_census.rs::auto_search_is_enabled_by_the_wire_default_and_drives_the_hint_query` asserts a request on a non-native serializer profile has no native window and does not exercise the snapshot; all unaudited
Impact: The ready LRU and active-lease budget under-charging the native bytes a snapshot pins; D1 holds because capture takes its own charged reference instead of the snapshot's
Open questions:
- Should release builds enforce the absence in `finish_ready`? (unresolved, needs `/low-level-systems:defensive-assertions-and-invariant-guards`)

### sc-revision-lead-refused-and-retryable

Type: safety
Reachability: explicit-config-only - `CaptureJob::publish_locked` passes `crate::now_ms()` as `observed_at` to `publish_screened`, whose `prepare` applies the lead; that path runs only behind the consent key. Other production publishers do not meet the check: `ProvenanceRule::Inherited` units and `repository_documents` are exempt in `prepare`.
Status: active
Exercised: yes - `crates/daemon/tests/eval_ingestion.rs` `generated_observations_never_lead_and_a_boundary_fixture_refuses` accepts a unit at exactly `MAX_REVISION_LEAD_MS` of lead and refuses one millisecond more with `RevisionAhead` and the tip unchanged; `hs-tests` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically` refuses `i64::MAX` with nothing staged; `sc-tests` `a_revision_lead_leaves_the_unit_pending` drives a user message two hours ahead through the capture path, counts `units_refused_retryable` on two passes with `units_memo_skipped` 0, and finds no live descriptor.
Guarantee: A non-inherited unit outside `repository_documents` whose native revision exceeds `observed_at` plus `MAX_REVISION_LEAD_MS` (one hour) is refused `RevisionAhead` before any byte is retained, and `is_permanent` classifies it retryable, so capture keeps it pending and offers it again.
Check: `always` - `prepare` returns `Err(RevisionAhead { revision, observed_at })` exactly when `revision > observed_at + lead`, `is_permanent() == false`, and `classify` maps it to `UnitsRefusedRetryable` with `Disposition::Pending`. Per call.
Fault/timing angle: harness and daemon clocks disagree by more than the lead (F25); the unit stays pending until the daemon clock passes `revision - lead` or the record leaves the window.
Required faults and enabling state: a native timestamp ahead of the daemon clock by more than one hour; capture enabled; a second pass carrying the same record.
Confidence: high - [evidence](evidence/sc-revision-lead-refused-and-retryable.md). `prepare`, `PublishError::is_permanent`, `classify`, and the memo pending path read; three tests read.
Existing check: `hs.rs` `pins::revision_lead_matches_the_evaluator_pin`; `crates/daemon/tests/eval_seam_probe.rs` (constant pin); `eval_ingestion.rs` `generated_observations_never_lead_and_a_boundary_fixture_refuses`; `hs-tests` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`; `sc-tests` `a_revision_lead_leaves_the_unit_pending`; unaudited.
Impact: A skewed host retries every such unit on every pass and captures none of them until its clock catches up; a permanent classification would lose them for good.
Open questions:
- Whether a unit that stays `RevisionAhead` across many passes should ever become terminal or be reported beyond `oldest_deferred_age_ms`. (needs human input)

### sc-readmission-blocked-after-purge-tombstone

Type: safety
Reachability: default-production - `artifact_is_blocked` runs in every exact and batch ingest; tombstones come only from `delete_artifact` with `ArtifactDeletionKind::Purge`, an operator action. Harness re-observation reaches it only with `source_capture.enabled`.
Status: active
Exercised: partial - `kernel_deletion.rs` `purge_tombstones_unlinks_degrades_pins_and_is_idempotent` purges a digest and re-ingests its bytes under a new key; `purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen` re-ingests inside the pending-unlink window; `kernel_cas.rs` `eligibility_matrix_includes_secret_unknown_and_tombstone` seeds a tombstone and re-ingests. Partial: no test purges a finite-class capture, and none re-ingests after ordinary GC (F27).
Guarantee: Exact ingest refuses `ReAdmissionBlocked` for a digest in `artifact_purge_tombstones` (permanent) or `artifact_pending_unlinks`, and `PublishError::is_permanent` returns `true` for that refusal. In a capture batch the block is found during batch ingest and rejects the whole batch as `Conflict` with no named unit, so the memo records nothing and each later pass that admits the unit is refused the same way.
Check: `always` - ingest of a tombstoned or pending-unlink digest under a new key returns `ReAdmissionBlocked` and retains nothing; `is_permanent(Artifact(ReAdmissionBlocked)) == true`; a capture batch that admits such a unit ends `kernel_error` and retains nothing. `always` because the check runs on every new reference.
Fault/timing angle: a purge commits tombstone and pending unlink together, and GC later clears only the pending row. Ordinary GC of expired capture bytes writes neither row, so identical bytes re-observed under a new key after expiry and reclaim are admitted by code reading.
Required faults and enabling state: an operator purge of a captured digest, then identical bytes under a new unit key (F24); an expired capture reclaimed by ordinary GC, then identical bytes under a new key (F27).
Confidence: medium - [evidence](evidence/sc-readmission-blocked-after-purge-tombstone.md). Read `artifact_is_blocked`, both tombstone and pending-unlink insert sites, the immutability triggers, `is_permanent`, `classify`, and four tests; the F27 reading has no test.
Existing check: `kernel_deletion.rs` `purge_tombstones_unlinks_degrades_pins_and_is_idempotent`, `purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen`; `kernel_cas.rs` `eligibility_matrix_includes_secret_unknown_and_tombstone`; `kernel_backup.rs` `a_backup_that_does_not_carry_a_purge_the_target_committed_cannot_be_restored`; all `unaudited`.
Impact: under an operator purge, identical capture bytes stay refused permanently. PR #1084 states expiry-driven reclaim tombstones the digest (the OQ21 loss condition). Code shows only an operator purge does.
Open questions:
- Is `ReAdmissionBlocked` permanent by intent for the pending-unlink arm (OQ20)? Resolved in U2 (ticket-1082): terminal for the process lifetime. The ticket expects a cleared pending unlink to admit after restart; by code reading every pending unlink carries a permanent tombstone, so it does not.
- Should `harness_capture` purges leave the digest re-admissible (OQ21)? (needs human input; the premise in pr-1084 that expiry tombstones disagrees with code, unresolved, needs an F27 test)

### sc-session-delete-leaves-kernel-descriptors

Type: safety
Reachability: default-production - `handle_session_delete_value` in `crates/daemon/src/lib.rs` calls `retire_captured_sources` on every `session.delete`; it skips the Kernel only when `KernelOpenCoordinator::expected` (`crates/daemon/src/kernel_routes/mod.rs`) is false, which defaults `true`; `source_capture.enabled` does not gate it
Status: invalidated - U8 (#1087) inverted it: `handle_session_delete_value` now calls `retire_captured_sources`, which runs `source_capture::retire_session_pages`, before `store.delete_session`; the obligation is carried by `sc-delete-retires-before-memory-store-delete`, with `sc-delete-retry-recovers-crash-between-stores` and `sc-deleted-session-not-resurrected-by-queued-capture`
Exercised: yes - the baseline guarantee is refuted at HEAD by `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`, which captures two sessions, deletes one, and asserts zero live descriptors for it and the other untouched
Guarantee: At baseline `08de9051a`, `session.delete` deleted memory-store rows and purged the memory-capture memo under `capture_commit_gate` and made no Kernel call.
Check: `always` - after a delete, Kernel descriptor inventories are unchanged; false at HEAD for any session with live `messages` or `raw_tool_spans` descriptors
Fault/timing angle: none
Required faults and enabling state: a session with captured descriptors; a delete
Confidence: high - [evidence](evidence/sc-session-delete-leaves-kernel-descriptors.md). Read `handle_session_delete_value` at both commits, `retire_captured_sources`, `KernelOpenCoordinator::expected`, `retire_session_pages`, `session_page_intent`, `session_page`, and the five U8 tests
Existing check: at baseline, none for the Kernel; the inverting tests at HEAD are `session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`, `session_delete_with_the_kernel_starting_deletes_nothing`, `a_delete_before_the_first_publication_drops_the_pending_capture`, `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages`, `session_retirement_pages_are_keyed_by_session_and_ids`; memory-store gate tests `lib.rs::a_cancelled_session_delete_still_purges_in_memory_state`, `explicit_capture_checkpoints_serialize_with_session_delete_off_the_worker`, `session_delete_waits_for_the_capture_gate_off_the_runtime_worker`; all unaudited
Impact: Inverted: every `session.delete` on a daemon that expects a Kernel now depends on Kernel readiness, including sessions with no captures
Open questions:
- OQ12, Kernel unavailable during delete: resolved in U8 as fail closed; verified in `retire_captured_sources` and `session_delete_with_the_kernel_starting_deletes_nothing`
- OQ4, post-delete passes: resolved within approved scope in U8 (no tombstone; a running harness session republishes, as the handler doc states); the tombstone stays the product owner's decision (needs human input)

### sc-unfinished-assistant-text-deferred

Type: safety
Reachability: explicit-config-only - `opencode_units` runs in production only from `CaptureJob::units`, behind the consent key.
Status: active
Exercised: yes - `hs-tests` `unfinished_assistant_text_is_deferred_but_settled_tools_publish` sets `time.completed` to null, then removes it, then adds `info.error` without a completion time, asserts only `raw_tool_spans` units each time, publishes the settled tools, then completes the message and finds only the completed text in the inventory; `an_interrupted_call_waits_for_its_turn_to_complete` asserts a tool part with `state.metadata.interrupted` under an unfinished turn yields no unit. No capture-path test drives an unfinished assistant window through `CaptureJob`.
Guarantee: `opencode_units` defers assistant text whose message has no completion timestamp, and defers an interrupted tool call of that message, while other settled tool results in the same record publish.
Check: `always` - for an assistant record without `time.completed`, the result holds no `messages` unit and no unit for a tool part whose `state.metadata.interrupted` is true, and it holds one `raw_tool_spans` unit per other `completed` or `error` tool part. Per call.
Fault/timing angle: a window observed mid-turn; the next pass re-reads the record and publishes its text once `time.completed` is set.
Required faults and enabling state: an assistant message without a completion time (F26); a settled tool part under it; an interrupted tool part under it.
Confidence: high - [evidence](evidence/sc-unfinished-assistant-text-deferred.md). The `opencode_units` part loop and `tool_outcome::opencode_interrupted` read; both tests read.
Existing check: `hs-tests` `unfinished_assistant_text_is_deferred_but_settled_tools_publish`, `an_interrupted_call_waits_for_its_turn_to_complete`; unaudited.
Impact: Publishing partial text would pin the message's revision at its creation time with bytes that later change, which then conflict as `IdentityReused`.
Open questions:
- Deferred assistant text produces no unit, so capture neither counts it nor records it in the memo as `Pending`, though U2's ticket lists this record under "pending state". Whether the capture report should count deferred text: unresolved, needs a decision from the U2 owner.

## Part B: obligations the stack added

### sc-consent-gate-admits-capture

Type: safety
Reachability: explicit-config-only - the transform pass builds a capture
`Offer` only when `source_capture::consents` holds, and that requires
`source_capture_enabled`, which defaults to false and is user-tier only.
Status: active
Exercised: partial - the gates are constructed; F29, a reload between the pass
and the job, is not. `crates/daemon/src/source_capture_tests.rs`
`capture_off_memory_off_or_a_subagent_session_runs_no_adapter` runs key off,
`memory_enabled` off, and `is_subagent` true against a real Kernel store and
asserts every counter is zero and the tip unchanged;
`a_route_claiming_another_harness_captures_nothing` asserts a `pi` route
serving an OpenCode window admits no job.
Guarantee: An adapter runs and bytes are copied only when the pass's consent
snapshot has `source_capture.enabled` and `memory_enabled` set,
`is_subagent` clear, a native serializer profile, and a profile harness equal
to the route's claimed harness.
Check: `always` - on every handoff, adapter invocation implies that the
snapshot predicate held. It is evaluated per handoff, because the predicate
is a per-pass snapshot.
Fault/timing angle: a config reload between the pass and the job. The job
carries the `Offer` and never re-reads config, so it runs under the snapshot
that admitted it.
Required faults and enabling state: F28 (key off, `memory_enabled` off,
`is_subagent` true, route harness mismatch); F29 (consent changed between
pass and job) for the timing angle.
Confidence: high - [evidence](evidence/sc-consent-gate-admits-capture.md).
I read `consents`, the `Offer` construction in the transform pass,
`SourceCapture::offer`, and `CaptureJob::units`, which is the only capture
caller of the adapters. I also read both tests.
Existing check: `source_capture_tests.rs`
`capture_off_memory_off_or_a_subagent_session_runs_no_adapter`
(`unaudited`); `a_route_claiming_another_harness_captures_nothing`
(`unaudited`); `config.rs`
`hostile_project_tier_cannot_change_privileged_values_and_warns_per_key` and
module `source_capture_parity` (`unaudited`).
Impact: exact user text and tool bytes are retained without the distinct grant
the plan requires.
Open questions:
- Two switches or the existing three (OQ15). Resolved in U1: two switches
  plus the subagent skip. `consents` never reads `memory_auto_capture` or
  `auto_promote`.
- No test constructs a config reload between the pass and the job (F29).

### sc-response-independent-of-publication

Type: safety
Reachability: explicit-config-only - the after-response capture work exists
only when `consents` admitted the pass; with the key at its default the pass
attaches none.
Status: active
Exercised: partial - `source_capture_tests.rs`
`the_response_is_identical_whatever_capture_and_the_kernel_are_doing`
compares capture-on and capture-off responses byte for byte with the Kernel
Ready, Starting, writer held by another thread's commit, and slot full;
`a_spent_budget_before_the_writer_commits_nothing` asserts a response time
bound under `Deadline` but no byte comparison;
`document_capture_leaves_the_response_and_respects_inject_docs` and
`pi_entry_identity_on_the_row_decides_capture_and_never_the_response` compare
responses; `transform_unit/host_tests.rs`
`the_host_starts_source_capture_after_the_response_settles` drives the
transport path.
Guarantee: A capture-enabled pass returns the same response bytes as a
capture-disabled pass, and the request path awaits neither publication, Kernel
phase, nor publication outcome.
Check: `always` - a predicate over every returned response: its bytes equal
the capture-off response for the same input. The property must hold on every
response, so it is not a coverage marker.
Fault/timing angle: the window between response construction and wire
handoff. It holds only a synchronous budget charge. The slot is taken with
`try_acquire_owned`, and the job is spawned unawaited after
`settle_prepared`.
Required faults and enabling state: F6 (Kernel Starting, `Deadline`), F22
(slot full), F30 (writer held by a foreground commit).
Confidence: high - [evidence](evidence/sc-response-independent-of-publication.md).
I read `capture_after_response`, the transport handler's after-response
ordering, `CaptureHandoff::into_job`, `CaptureJob::run`, and the tests.
Existing check: the five tests named in `Exercised` (`unaudited`).
Impact: a Kernel stall becomes a transform stall, or the response varies with
Kernel state.
Open questions:
- Latency-equality threshold for AC7 (OQ19) (needs human input).
- The `Deadline` case asserts a time bound but compares no bytes with a
  capture-off baseline.

### sc-one-outstanding-batch-refuse-newest

Type: safety
Reachability: explicit-config-only - the slot is taken only by a
`CaptureHandoff`, which only a consented pass creates.
Status: active
Exercised: yes - `source_capture_tests.rs`
`the_response_is_identical_whatever_capture_and_the_kernel_are_doing`, slot
full section: the `before_publish` hook holds the first batch, and a second
pass is refused `refused_queue_full == 1` with `retained_bytes` unchanged and
`active == 1`. The section carries the coverage witness
`handoff_refused_slot_busy`.
`a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`
asserts 32 published and one `units_deferred`.
`shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
asserts that a panic releases the slot and charges and that shutdown waits.
`a_window_past_the_remaining_budget_is_refused_resident_short` and
`crates/daemon/tests/harness_sources.rs`
`capture_batch_defers_units_beyond_its_bounds` cover the remaining bounds.
Guarantee: At most one capture job, publishing batches of at most 32 units and
64 KiB of text, is outstanding per daemon store. A handoff while one is
outstanding is refused with `refused_queue_full` and counted, and it retains
nothing once refused.
Check: `always` - `active <= 1`, and `retained_bytes` after a refused
handoff equals its value before that pass. The bound must hold at every
instant, and the zero-retention claim is checkable at refusal.
Fault/timing angle: two passes racing for the slot. The charge is taken at
response construction and the slot at settlement. The permit lives in the
`spawn_blocking` closure and drops only when the closure returns or unwinds.
Required faults and enabling state: F22 (a second pass while the first batch
is held inside publication).
Confidence: high - [evidence](evidence/sc-one-outstanding-batch-refuse-newest.md).
I read `SourceCapture::default`, `offer`, `into_job`, `CaptureJob::run`,
`RetainedCharge`, `ActiveSlot`, and `publish_screened`'s bounds, plus the
tests.
Existing check: the tests named in `Exercised` (`unaudited`).
Impact: unbounded resident bytes from retained windows, or concurrent writer
pressure from capture.
Open questions:
- The ticket says the slot is acquired before any window reference is
  retained past the response. The code holds the charge and `Arc`
  references from response construction to settlement and takes the slot
  afterward. The net retention of a refused handoff is zero and it makes no
  copy, but the wording and the code differ.
- One job can run two serial batches: the window's, then the documents'
  (`capture_documents`). The outstanding bound counts jobs.

### sc-unit-cap-refuses-whole-never-prefix

Type: safety
Reachability: explicit-config-only - `publish_screened` has two production
callers, the capture job's window batch and its documents batch, and both run
only under consent.
Status: active
Exercised: partial - at the publisher seam only; no transform-path or
tool-unit oversize test. `crates/daemon/tests/harness_sources.rs`
`capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings`
publishes a 65536-byte unit with equal artifact bytes. It then sends two
siblings around a 65537-byte unit and asserts `UnitTooLarge`, both siblings
published, evidence `(3, 65536)`, four new receipts, none for the oversized
unit, and an unchanged tip on retry.
`capture_batch_refusals_take_no_admission_slot` asserts that an oversized
unit takes no batch slot. `source_capture_tests.rs`
`unreadable_documents_are_counted_and_retire_nothing` asserts that a 70 KiB
document counts `documents_oversize`.
Guarantee: Every retained capture artifact equals the complete native string.
A unit above 64 KiB has no evidence object and a `unit_too_large`
disposition, and its siblings still publish.
Check: `always` - every capture evidence object equals its unit text and is
at most 65536 bytes. This is a property of every stored object.
Fault/timing angle: none.
Required faults and enabling state: F31 (unit text above 64 KiB beside
sibling units).
Confidence: high - [evidence](evidence/sc-unit-cap-refuses-whole-never-prefix.md).
I read the length screen in `publish_screened`, which runs before `prepare`,
`evidence_request`'s whole-text payload, the adapters (no truncation),
`classify`, `plan_document`, and the tests.
Existing check: the three tests named in `Exercised` (`unaudited`);
`MAX_SOURCE_UNIT_BYTES` aliases `MAX_CAPTURE_SOURCE_BYTES` in
`crates/memory-store/src/memory_capture.rs` (`unaudited`).
Impact: a spliced prefix is presented as an exact source, or one oversized
unit silences its window.
Open questions:
- No transform-path test sends an oversized unit, so the supervisor's
  `units_unit_too_large` counter and its terminal memo entry are untested.

### sc-terminal-disposition-never-retried

Type: safety
Reachability: explicit-config-only - the memo and the dispositions exist only
inside consented capture jobs.
Status: active
Exercised: partial - `source_capture_tests.rs`
`changed_bytes_under_one_identity_are_terminal_and_never_retried` edits an
OpenCode user message in place under the same `time.created`. It asserts one
`units_identity_conflict` across two edited passes, an unchanged tip, and one
live row with the original bytes.
`a_concurrent_publication_of_a_unit_replays_or_conflicts` asserts that the
memo answers for the terminal unit. `harness_sources.rs`
`capture_batch_names_the_unit_whose_receipt_conflicts` asserts that the
conflict names its unit and commits nothing. No test re-probes a terminal
unit after memo loss.
Guarantee: A terminal disposition for an unchanged `(receipt key, request
digest)` is never retried while its memo entry lives. After a memo loss the
Kernel's re-probe commits nothing, and the prior live descriptor stays live.
Check: `always` - a terminal memo entry with equal key and digest is never a
publication candidate and never becomes `Pending`, and the tip is unchanged
by any later attempt of that unit. Both must hold on every pass.
Fault/timing angle: a memo loss (restart, any `session.delete` through
`invalidate_sessions`, eviction past 4 MiB, poison). The next pass offers
the unit again, the Kernel answers `Conflict` from its receipt before any
commit, and the siblings in that batch wait one pass.
Required faults and enabling state: F4 (an OpenCode user message edited in
place keeping `time.created`); F3 (memo loss) for the re-probe.
Confidence: high - [evidence](evidence/sc-terminal-disposition-never-retried.md).
I read `is_permanent`, `BatchRefusal::conflicting_unit`, the error arm of
`publish_locked`, `classify`, candidate selection in `CaptureJob::publish`,
`Memo`, and the tests.
Existing check: the tests named in `Exercised` (`unaudited`);
`PublishError::is_permanent` (`unaudited`, no table test).
Impact: retry amplification on the Kernel writer, or a minted revision that
evades `IdentityReused`.
Open questions:
- Daemon-minted revision for proven edits (OQ5). Resolved in U2: D2 stands.
  Changed bytes are a terminal `IdentityReused`, and the code has no minting
  path.
- The re-probe after memo loss has no test, so the `replay_after_memo_loss`
  precondition is not paired with a terminal unit.

### sc-pending-unit-publishes-next-ready-pass

Type: liveness
Reachability: explicit-config-only - pending state exists only in the memo of
consented capture jobs.
Status: active
Exercised: partial - Starting, deferral, and revision lead are constructed; no
test runs a pass after a slot-full refusal. `source_capture_tests.rs`
`a_unit_pending_behind_a_starting_kernel_publishes_on_the_next_ready_pass`
runs pass N with no Kernel open (`kernel_unavailable == 1`), opens the
Kernel, and asserts one live descriptor after pass N+1.
`a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`,
`a_revision_lead_leaves_the_unit_pending`,
`a_delete_after_the_handoff_publishes_nothing_for_the_queued_window`, and
`shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot`
cover the deferral, retryable refusal, invalidation, and panic paths.
Guarantee: A pending unit whose input is unchanged and that the next window
carries is offered to `publish_screened` on the first later pass with the
Kernel Ready and the slot free. It publishes or becomes terminal there,
unless more than one batch of pending and unknown units precedes it or its
refusal is retryable.
Check: `always` - after Kernel Ready, a free slot, and one settled pass that
carries the unit, the unit is not pending, given at most 32 candidates and
64 KiB ahead of it. The bound is one pass, counted in passes, because the
code gives no wall-clock bound.
Fault/timing angle: Kernel Starting on pass N and Ready on pass N+1. A
refused or invalidated job records nothing, so its units stay candidates.
Required faults and enabling state: F6 (`kernel_store()` returns Starting on
one pass); F25 (a retryable revision lead); F22 (a refused window).
Confidence: high - [evidence](evidence/sc-pending-unit-publishes-next-ready-pass.md).
I read the early `kernel_unavailable` end in `CaptureJob::publish`, the
pending-first candidate order, `classify`, the error arm of
`publish_locked`, `Memo::record`, and the tests.
Existing check: the tests named in `Exercised` (`unaudited`).
Impact: a window observed once and never captured, with no signal.
Open questions:
- Pending set per unit or per window. Resolved in U2: per unit, keyed in the
  memo, and units are re-derived from each window. Windows older than the
  submitted head are outside the guarantee (C7).
- No test sends a third pass after a slot-full refusal to observe that the
  refused window's units publish.

### sc-capture-creates-no-decision

Type: safety
Reachability: explicit-config-only - capture admissions are written only by
the consented capture job's `SourcePublisher`.
Status: active
Exercised: partial - `source_capture_tests.rs`
`captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`
publishes user text and tool results, reopens the Kernel from disk, and
asserts nothing captured is visible to `Surface::AutoInject` and the
decision count is unchanged.
`an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
asserts `(TrustedToolResult, ToolUntrustedOutput)` and `Sensitive` for a
`read` output.
`a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`
asserts `(UntrustedRepoText, RepoUntrustedText)`, `Sensitive`, and no new
decision for documents. The CI "Invented provenance" step scans the capture
modules. No test reads a captured row's provenance.
Guarantee: No capture creates a memory decision, carries a `decision_kind`,
or carries `RepositoryProvenance`. Every capture admission is
`EventKind::Other` with the fixed class pair and stores at least
`Sensitive`.
Check: `always(!X)` - after every capture commit and after a Kernel reopen,
the decision count is unchanged, every capture admission row has
`event_kind = Other` with no candidate or approval id, and every captured
evidence row has no provenance and a stored sensitivity of at least
`Sensitive`. The forbidden state has no detection point of its own.
Fault/timing angle: none; a Kernel reopen is the state that must preserve it.
Required faults and enabling state: a published unit; a Kernel reopen (F3);
a `RepositoryDocuments` unit.
Confidence: high - [evidence](evidence/sc-capture-creates-no-decision.md).
I read `SourcePublisher::apply` (`EventKind::Other`, no candidate),
`evidence_request`, `git_sources::commit_provenance` (`None` off
`GitCommits`), the capture publisher's `Sensitive`, the CI step, and the tests.
Existing check: the tests named in `Exercised` (`unaudited`); CI step
"Invented provenance" in `.github/workflows/ci.yml` (`unaudited`);
`surface_visibility` in `crates/kernel/src/admission.rs` (`unaudited`).
Impact: captured bytes become model-visible under a weaker policy.
Open questions:
- No row-level assertion shows captured evidence carries no provenance. U2
  deferred it as TS7.

### sc-capture-logs-carry-no-text

Type: safety
Reachability: explicit-config-only - every capture log line and counter is
emitted by the supervisor, which only consented passes reach. The report
reads zero when the key is off.
Status: active
Exercised: yes - `source_capture_tests.rs`
`the_capture_report_carries_no_unit_text` drives an identity conflict, an
adapter refusal whose session id is a sentinel, and a window too large. It
asserts that neither the report nor `health().metrics` contains the sentinel,
`msg_sentinel`, or the project digest. The `source_capture.rs` test
`log_lines_carry_only_fixed_codes_and_counts` renders every `Count` and
asserts the characters.
Guarantee: Every log line and counter the capture path emits carries a stage,
a reason code, and counts only, never unit text, paths, identities, or
digests.
Check: `always` - over every emitted record, the record consists of a
`'static` stage, a `Count` name, and integers. Every record must comply.
Fault/timing angle: error paths that would format the offending value.
`kernel_end` collapses every `KernelError` to a code, and no
`BatchRefusal` or `PublishError` is formatted.
Required faults and enabling state: each refusal and Kernel error path: F4
(identity conflict), F1 (adapter refusal), F32 (window above the supervisor
budget), F6 (Kernel unavailable or `Deadline`).
Confidence: high - [evidence](evidence/sc-capture-logs-carry-no-text.md). I
read the `counts!` vocabulary, `log_line` (the module's only `eprintln!`),
`report`, `kernel_end`, and a grep of the capture-path modules for other
logging, plus both tests.
Existing check: the two tests named in `Exercised` (`unaudited`); `impl
std::fmt::Debug for SourceUnit` (`unaudited`; it prints identity values).
Impact: secret-bearing tool output, session ids, or paths appear in logs
outside every disclosure gate.
Open questions:
- `SourceUnit`'s `Debug` prints identity values, including a document's
  `relative_path`, so it is no guard against paths. No log formats it at HEAD.
- The `session.delete` error response formats a `KernelError` display. That
  is a response, not a capture log; its display strings were not audited.

### sc-harness-capture-deadline-replay-never-renews

Type: safety
Reachability: explicit-config-only - only `SourcePublisher::evidence_request` sets `harness_capture`, and the capture path that publishes `messages` and `raw_tool_spans` runs only when `source_capture::consents` holds, which requires `config.source_capture_enabled`; `DaemonConfig::default` sets it `false` and the key is user-tier only (`ConfigKey::SourceCaptureEnabled`)
Status: active
Exercised: partial - `crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` publishes a message and a tool unit through `SourcePublisher::publish`, reads the class and deadline, republishes after the deadline and after expiry, and keeps a `canonical` git row and a repository document; `crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` refuses a `harness_capture` ingest without a deadline; the `publish_screened` batch path that `SourceCapture` uses, and equal-text units in one batch, are not constructed for the deadline
Guarantee: Every `messages` and `raw_tool_spans` evidence ingest carries `retention_class = "harness_capture"` and `retain_until = observed_at + 30 days`, where `observed_at` is the caller's clock; every other class keeps `canonical` and no deadline; a later attempt of the same unit leaves the stored deadline unchanged.
Check: `always` - over every capture evidence row, `retention_class == "harness_capture"` and `retain_until == observed_at + HARNESS_CAPTURE_RETENTION_MS` of the attempt that created the row, and over every replay the stored `retain_until` is unchanged; asserted on every evaluation because a single renewing replay breaks the retention bound
Fault/timing angle: a later attempt (restart, retry of a pending unit, or a pass after expiry) answers from the descriptor or evidence receipt, whose request digest excludes the clock, so it keeps the first row and deadline
Required faults and enabling state: F11 - two publications of one unit with the second after the first deadline; a sweep at `deadline - 1` and at `deadline`; a `canonical` row in the same store
Confidence: high - [evidence](evidence/sc-harness-capture-deadline-replay-never-renews.md). Read `evidence_request`, `publish_within`, `publish_batch` owner mapping and `retained` map, `request_digest` and `digest_inputs`, the `cas/ingest.rs` deadline checks, `HARNESS_CAPTURE_RETENTION_MS`, and the landed test body
Existing check: `crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` (unaudited); `crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` undated-ingest refusal (unaudited); `crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress` (unaudited)
Impact: Captured text is retained without bound, or every pass renews its deadline
Open questions:
- Per-unit versus per-evidence-row deadline under shared text (OQ14): resolved in U4 as a per-row deadline where the earliest owner wins; units share a row only inside one batch, where `observed_at` is identical (`ticket-1084.md`; `evidence_request` doc comment; `publish_batch` owners)
- Does an evidence-only receipt whose evidence the sweep retired republish its unit: unresolved, needs a test of that sequence
- Re-admission of identical bytes after their artifact is purged (OQ21) (needs human input)

### sc-expiry-descriptor-first-cited-survives

Type: safety
Reachability: explicit-config-only - `kernel_slice` in `crates/daemon/src/memory_reviewer/lifecycle.rs` runs `expire_harness_captures` on every maintenance slice, but its predicate selects only `harness_capture` rows, which only the consent-gated capture path writes (`source_capture_enabled` default `false`)
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` asserts the expiry commit records `["observation_retire", "evidence_retire"]` and keeps canonical and git rows; `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` constructs a live `capture_source_hold` over every capture and an independent `note` observation citing one; `crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` inserts a pin inside the writer transaction after reader selection
Guarantee: Expiry retires every live descriptor of a capture before or in the commit that retires its evidence, never retires evidence a live row cites or a live hold pins at that commit, and never touches another retention class.
Check: `always` - for every evidence row the harness sweep retired, its commit found no live citer (`EVIDENCE_CITED_SQL`) and no live pin (`EXPIRED_UNPINNED_SQL`), and every source descriptor citing it was invalidated in that commit or an earlier one; evaluated per commit because the writer recheck is the only point that decides
Fault/timing angle: a hold or citation that lands between `expired_captures` under the reader lock and the page's writer commit; the recheck in `retire_expired_capture` shares the retirement's transaction
Required faults and enabling state: F12 - a pin acquired between selection and commit; F13 - expired capture evidence cited by an independent row; canonical and git evidence in the same store
Confidence: high - [evidence](evidence/sc-expiry-descriptor-first-cited-survives.md). Read `expire_captures`, `retire_expired_capture`, `EXPIRED_UNPINNED_SQL`, `expired_captures_sql`, `live_capture_observations`, `cited_elsewhere`, `retire_evidence_inner`, `retire_capture_observation`, and the three landed tests
Existing check: `crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` (unaudited); `crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` (unaudited); `crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` (unaudited); `Envelope::retire_evidence` conflict guard (unaudited)
Impact: Cited evidence vanishes under a decision, or a live descriptor outlives its bytes
Open questions: None.

### sc-expiry-drains-within-page-bound

Type: liveness
Reachability: explicit-config-only - same sweep and predicate as the record above; only consent-gated capture writes `harness_capture` rows
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` publishes 65 messages with one cited, asserts `(63, 1)` then `(1, 0)` then nothing; `crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress` drives one maintenance slice and reads the reported progress
Guarantee: Each sweep call commits at most 64 captures, oldest `retain_until` first, and N expired, unpinned, uncited captures are retired within `ceil(N / 64)` calls with no new expiries.
Check: `always` - per call `retired + retained <= MAX_EXPIRED_CAPTURES_PER_CALL`; after `ceil(N / 64)` calls with no new expiries, zero live expired, unpinned, uncited `harness_capture` evidence remains; the bound is in calls because the page bounds rows, not time
Fault/timing angle: a cited or refused row inside a page; cited rows leave the candidate set once their descriptors retire, and refused rows are skipped within the call
Required faults and enabling state: F14 - more than 64 expired captures with one cited row among them; F12 - a live hold that blocks every capture until it lapses
Confidence: high - [evidence](evidence/sc-expiry-drains-within-page-bound.md). Read `MAX_EXPIRED_CAPTURES_PER_CALL`, `expired_captures_sql` ordering and filters, the `expire_captures` error arm, `kernel_slice`, `SourceCapture::record_expiry`, and the landed tests
Existing check: `crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` (unaudited); `crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress` (unaudited)
Impact: The store never drains, or a cited row starves the page
Open questions:
- Descriptor and receipt rows outlive retirement (OQ6) (needs human input)
- Can a persistently refused candidate hold a page slot on every call: unresolved, needs a test that forces a persistent per-row refusal at the head of the order

### sc-outcome-observed-bounded-occurrence-bound

Type: safety
Reachability: explicit-config-only - outcome records ride `raw_tool_spans` units, which only the consent-gated capture path publishes (`source_capture_enabled` default `false`)
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs::opencode_tool_outcomes_follow_the_pinned_trace` maps every case of `testdata/opencode-tool-outcome-trace.json` plus malformed, absent, and non-`bash` edits; `pi_tool_outcomes_read_status_and_reported_truncation` covers Pi; `equal_tool_bytes_with_different_outcomes_are_two_descriptors_of_one_artifact` publishes exit-1 and error calls with equal bytes; `an_invalid_outcome_record_is_refused_before_retention` checks the refusal set before retention; `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` checks the Kernel validation and the unchanged occurrence tuple
Guarantee: A tool descriptor's outcome record is at most 1 KiB serialized and validated before any byte is retained; `exit_code` appears only from an integer `state.metadata.exit` on an OpenCode `bash` call and `exception_ref` only from `info.error.name` on an interrupted call; an absent or malformed field stays unknown; equal bytes with different outcomes are two descriptors citing one artifact; identity encoding bytes are unchanged.
Check: `always` - over every published descriptor, `outcome` is `None` or passes `check_outcome` (class `raw_tool_spans`, version 1, status agreeing with the representation, `byte_length` equal to the selected bytes, encoded length at most 1024), no `exit_code` exists without the fixture-proven key, and `occurrence_tuple` equals the encoding without the record; the identity goldens hold
Fault/timing angle: none
Required faults and enabling state: F33 - completed with exit 1, error without exit, absent metadata, malformed metadata; F34 - a record the Kernel refuses (over 1 KiB, wrong byte length, wrong class, wrong version, secret or empty tool name)
Confidence: high - [evidence](evidence/sc-outcome-observed-bounded-occurrence-bound.md). Read `ToolOutcome`, `validate`, `check_outcome`, `SourceDescriptorDetail`, `SourcePublisher::prepare`, `tool_outcome::opencode` and `pi`, the trace fixture, and the landed tests; confirmed no U5 change to `eval_identity.rs`
Existing check: the five daemon tests and the Kernel test named in `Exercised` (unaudited); `crates/kernel/tests/eval_identity.rs::the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` (unaudited)
Impact: A fabricated `exit=0` reports a failed call as a success, or the outcome moves occurrence ids
Open questions:
- Which `state.metadata` key carries the exit code (OQ3): resolved in U5 by the pinned OpenCode 1.19.0 trace `crates/daemon/testdata/opencode-tool-outcome-trace.json`; `exit` maps on `bash` only (`tool_outcome::opencode`)

### sc-v1-receipt-replays-legacy

Type: safety
Reachability: explicit-config-only - receipts under `eidnara-daemon/harness-sources` exist only after consent-gated capture publishes (`source_capture_enabled` default `false`)
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs::receipts_written_before_the_outcome_record_replay_under_v2` pins the v1 digest layout, rewrites stored receipts to v1 digests with `store_receipt_as`, and asserts replay on the single and batch paths with no enrichment, `IdentityReused` for changed text and egress, completion of a v1 evidence-only receipt under v2 on both paths, and v2 publish and replay for a new unit
Guarantee: A unit whose receipts were stored under `source-request.v1` replays with `replayed: true` and no enrichment; a v1 evidence-only receipt completes with a v2 descriptor; a changed payload or context under that key is still `IdentityReused`.
Check: `always` - over every publication whose key holds a v1 receipt, a match on the v1 digest of the current inputs replays and commits nothing new for that receipt, and a match on neither digest refuses `IdentityReused`; evaluated on every replay because one unprobed path fails a whole batch
Fault/timing angle: a v1 evidence-only receipt left by a partially completed single-unit publication; under v2 its descriptor receipt then records v2
Required faults and enabling state: F4 on the digest tag - v1 receipts in a store under the v2 publisher, plus changed text and changed egress under a legacy key
Confidence: high - [evidence](evidence/sc-v1-receipt-replays-legacy.md). Read `REQUEST_DIGEST_FORMAT`, `LEGACY_REQUEST_DIGEST_FORMAT`, `digest_inputs`, `request_digest`, `legacy_request_digest`, `stored_receipt`, its two call sites in `publish_within` and `publish_batch`, and the landed test
Existing check: `crates/daemon/tests/harness_sources.rs::receipts_written_before_the_outcome_record_replay_under_v2` (unaudited)
Impact: Every pre-upgrade capture counts as a permanent conflict and blocks its batch
Open questions:
- Probe v1 then v2, or move to a new key (OQ9): resolved in U5 by probing the v2 digest then the frozen v1 digest under the same key for both receipts (`stored_receipt`)

### sc-digest-bounded-deterministic-char-boundary

Type: safety
Reachability: explicit-config-only - the digest reaches a model only through a summarizer fold (`build_history_summarizer_chunk` then `presented_input` in `crates/daemon/src/lib.rs`), and `DaemonConfig::eidnara_folds` requires a non-empty `model_chain`, which `DaemonConfig::default` leaves empty; the path is independent of `source_capture`; the source record labelled it `default-production`, and whether the boundary trigger estimate formats digests under the default config is unresolved
Status: active
Exercised: yes - `crates/daemon/src/tool_outcome.rs::digest_tests` pins CRLF, single-line, empty, error-with-exit, two- and three-byte clipping, and a 1 KiB call clip, and the proptest `every_digest_is_bounded_and_shows_only_its_outer_lines` generates multibyte, quote, bracket, tab, and CR lines; `history_summarizer_chunk.rs::digest_chunk_tests::the_c5_case_digest_shows_completed_and_the_refusal_in_tail` decodes the C5.V1 corpus source; `tool_output_contains_sentinel` and `boundary.rs::digest_tests::the_boundary_chunk_renders_digests_for_answered_calls` check the sentinel at both sites
Guarantee: Each tool-result digest is at most 1 KiB; each excerpt is at most 128 bytes, the escaped first or last nonblank line cut on a UTF-8 character boundary with a `…` clip marker; equal inputs give equal bytes; no output byte outside the two excerpts appears, at either `TC:` site.
Check: `always` - over every digest built, `len <= MAX_DIGEST_BYTES`, each excerpt `len <= CLAIM_VALUE_MAX_BYTES` and equals `clip` of its escaped outer line, the digest is a function of its inputs, and a sentinel on a middle line is absent; asserted per digest because one oversize or split digest reaches the prompt
Fault/timing angle: none
Required faults and enabling state: F35 - CRLF, multibyte, single-line, empty, and over-bound outputs, and an over-bound call summary
Confidence: medium - [evidence](evidence/sc-digest-bounded-deterministic-char-boundary.md). Read `outcome_digest`, `digest`, `clip`, `from_output`, both call sites, and the landed tests; behavior is high confidence, the reachability label is medium
Existing check: the tests named in `Exercised` (unaudited)
Impact: A split code point in the prompt, or the full log reaching the summarizer
Open questions:
- Does the boundary trigger estimate format digests for a session under `DaemonConfig::default()`: unresolved, needs a read of the trigger gating

### sc-digest-citations-never-reach-artifact

Type: safety
Reachability: explicit-config-only - aliases and citations exist only on a summarizer fold, which needs a non-empty `model_chain` (`DaemonConfig::eidnara_folds`); the default is empty; the source record labelled it `default-production`
Status: active
Exercised: partial - `history_summarizer_chunk.rs::digest_chunk_tests::an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` constructs a forged `«s9»` in tool output; `a_digest_cut_by_the_budget_withdraws_its_alias` constructs a budget cut inside a digest; `history_summarizer_citations_golden.rs::rendered_aliases_match_the_hand_checked_native_oracles` pins a digest alias with `transformed: true`; no landed test validates a citation into a digest alias
Guarantee: A digest is frozen `transformed = true` under the part's existing `sN` alias, so a citation resolves to presented digest bytes and never to native artifact offsets; a forged marker in tool output never becomes a target; an alias cut by truncation is withdrawn.
Check: `always(!X)` - X is a citation accepted against bytes the model did not see whole as presented; the state has no detection point of its own, so assert on every built chunk that each digest-bearing alias is `transformed`, every marker in the text names an issued alias, and after `presented_input` each remaining alias's marker and whole `presented` bytes occur in order in the kept prefix
Fault/timing angle: the budget cut in `presented_input`, which runs after the full chunk with every digest is built
Required faults and enabling state: F16 - a budget that cuts inside a digest; F17 - a marker literal in tool output
Confidence: medium - [evidence](evidence/sc-digest-citations-never-reach-artifact.md). Read `push_message`, `absorb_tool_only`, `Builder::part`, `presented_input`, citation validation against `presented`, and the landed tests; confirmed no production reader of `FrozenAlias.transformed`; the reachability label is medium
Existing check: the tests named in `Exercised` (unaudited); `history_summarizer_citations_golden.rs::a_forged_marker_in_native_text_cannot_keep_a_withdrawn_alias` and `truncation_withdraws_aliases_the_model_did_not_see_whole` (unaudited)
Impact: A citation claims bytes the model never saw whole
Open questions: None.

### sc-rerun-exactly-one-live-generation

Type: safety
Reachability: explicit-config-only - only `CaptureJob::publish_locked` calls `publish_screened` for tool units, and capture runs only when `source_capture::consents` holds, which requires `DaemonConfig::source_capture_enabled` (user tier, default `false` in `config.rs`) and `memory_enabled`
Status: active
Exercised: yes - `crates/daemon/tests/harness_sources.rs::a_tool_rerun_leaves_exactly_one_live_generation` publishes Pi re-runs in order (error to success with fewer blocks), a distinct call with equal text, a redelivered 1 and duplicate 2, a stale 1 with changed bytes, reversed 2 then never-published 1, published 1 beside new 2 in one window, and two generations in one batch, and reads live generations per call and `created_commit_seq` against the predecessor's retirement; `crates/daemon/src/source_capture_tests.rs::a_rerun_keeps_one_live_generation_across_a_restart` drives an OpenCode call re-run in place through the capture path with the memo emptied between generations (`replay_after_memo_loss`, `reversed_generation_delivery`); `crates/daemon/src/harness_sources.rs::tests::a_rerun_retirement_rechecks_the_call_inside_its_commit` constructs a newer live generation and a missing predecessor inside the commit; `crates/kernel/tests/kernel_source_descriptors.rs::a_writer_reads_the_live_descriptors_of_some_identity_values` covers the narrowed in-transaction read
Guarantee: After every commit made through `publish_screened` under the capture commit gate, each call `(project_id, harness, session_id, parent_message_id, tool_call_id)` has at most one live `raw_tool_spans` generation, and the predecessor's retirement commits in the successor's commit.
Check: `always` - over every commit, live descriptors per call are at most one, and each retired predecessor's `invalidated_commit_seq` equals its successor's `created_commit_seq`; the property is a state invariant evaluated at every commit
Fault/timing angle: reversed and duplicate delivery (`2,1` and `1,2,1`); restart with an empty memo between the two results; a newer generation appearing between the plan preview and the commit
Required faults and enabling state: two results for one `tool_call_id` with different `result_revision` (`state.time.end` or Pi `timestamp`); F3 memo loss; F36 reordered delivery
Confidence: high - [evidence](evidence/sc-rerun-exactly-one-live-generation.md). Read `publish_screened`, `plan_reruns`, `live_generations`, `predecessors`, `Reruns::intent`, `Reruns::apply`, `retire_descriptors`, and the commit-gate acquisition in `CaptureJob::publish`; read all three daemon tests at HEAD; confirmed no other production caller publishes `raw_tool_spans`
Existing check: `crates/daemon/tests/harness_sources.rs::a_tool_rerun_leaves_exactly_one_live_generation` (unaudited); `crates/daemon/src/source_capture_tests.rs::a_rerun_keeps_one_live_generation_across_a_restart` (unaudited); `crates/daemon/src/harness_sources.rs::tests::a_rerun_retirement_rechecks_the_call_inside_its_commit` (unaudited); `crates/kernel/tests/kernel_source_descriptors.rs::a_writer_reads_the_live_descriptors_of_some_identity_values` (unaudited); `crates/daemon/tests/harness_sources.rs::rerun_lookup_cost_at_ten_thousand_live_tool_descriptors` (ignored measurement, unaudited)
Impact: two live results for one call reach search and recall, or zero live results after reversed delivery
Open questions:
- Source and cost of the predecessor lookup (OQ11): resolved in U7 with the filtered inventory narrowed on `tool_call_id`, no Kernel index; PR #1100 records a 245 ms median at 10 000 live descriptors on a quiet host against the 500 ms criterion
- Does a re-run keep its `callID` (OQ3): resolved for this record in U7; a new `callID` is another call and a kept `callID` with a new `state.time.end` is a newer generation, and both readings are handled
- `SourcePublisher::publish` and `publish_batch` publish tool units without reconciliation, and a store written before U7 converges only on redelivery of the newest generation; no production caller uses the unreconciled path

### sc-delete-retires-before-memory-store-delete

Type: safety
Reachability: default-production - `handle_session_delete_value` calls `retire_captured_sources` on every delete while `KernelOpenCoordinator::expected()` is true, which `lib.rs` sets for `StorageBackend::Sqlite`; the inventory and the fail-closed refusal run whether or not capture is enabled, and a non-empty retirement needs captures, which are `explicit-config-only`
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` captures a session and a second session of the project and asserts no live descriptor names the session at the F23 cut, the other session untouched, and the retired rows readable; `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` keeps the same session id under another project and under Pi; `session_delete_with_the_kernel_starting_deletes_nothing` asserts `kernel_unavailable` with the memory rows kept; `session_retirement_pages_are_keyed_by_session_and_ids` pins page keys
Guarantee: The memory-store delete of a session runs only after Kernel commits have retired every live `messages` and `raw_tool_spans` descriptor of the bound project, harness, and session; any inventory, commit, or availability failure returns before the memory-store delete and leaves the session undeleted.
Check: `always` - at the memory-store delete, zero live `messages` or `raw_tool_spans` descriptors name the bound `(project_id, harness, session_id)`; an ordering invariant checked at a fixed code point
Fault/timing angle: a capture committing between inventory and retirement, excluded by `capture_commit_gate`; the expiry sweep retiring listed rows between a page's preview and its commit; a Kernel in `Starting`
Required faults and enabling state: a session with captures; F6 Kernel `Starting` or unavailable during the delete; a second session and a second harness under the same session id
Confidence: high - [evidence](evidence/sc-delete-retires-before-memory-store-delete.md). Read `handle_session_delete_value`, `retire_captured_sources`, `retire_session_pages`, `session_page`, `session_page_intent`, and `retire_descriptors`; read the four tests; confirmed the ordering inside one gate-held blocking closure
Existing check: `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` (unaudited); `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` (unaudited); `session_delete_with_the_kernel_starting_deletes_nothing` (unaudited); `session_retirement_pages_are_keyed_by_session_and_ids` (unaudited); `a_route_claiming_another_harness_captures_nothing` (unaudited)
Impact: a deleted conversation's bytes stay live and queryable
Open questions:
- Kernel unavailable during delete (OQ12): resolved in U8, fail closed, for every session of a SQLite deployment including sessions with no captures
- No component retries a delete that answered `kernel_unavailable`; PR #1101 records that the OpenCode plugin sends one `deleteSession` and logs a failure (needs human input)
- Gate hold time grows with the session's captured descriptors and is not measured

### sc-delete-retry-recovers-crash-between-stores

Type: liveness
Reachability: explicit-config-only - the window between the two stores holds state only when the session has captures, which exist only with `source_capture.enabled`; the retry route itself runs under `DaemonConfig::default()`
Status: active
Exercised: partial - `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` sets `stop_after_retirement`, gets `injected_stop`, then one retry answers `ok` with `deleted_rows > 0` and an unchanged Kernel tip; `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` stops after one page of two rows and one delete completes; the cut runs in one process, so process restart and power loss are not exercised
Guarantee: After a stop between the Kernel retirement commit and the memory-store delete, one retry of `session.delete` with a ready Kernel and an open memory store answers `ok`, commits nothing new in the Kernel, and completes the memory-store delete.
Check: `always` - after exactly one retry call, the answer is `ok`, the Kernel tip is unchanged, zero live descriptors name the session, and the memory rows are gone; the bound is one call, the unit the route bounds
Fault/timing angle: the window between the two store commits; a stop between retirement pages
Required faults and enabling state: F23 cut after the Kernel commit; a captured session
Confidence: medium - [evidence](evidence/sc-delete-retry-recovers-crash-between-stores.md). Read the F23 branch, `retire_session_pages`, `session_page_intent`, and both tests; the cut is in-process, and the memory-store side is read only through the retry's `deleted_rows`
Existing check: `crates/daemon/src/source_capture_tests.rs::session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores` (unaudited); `session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages` (unaudited); `session_retirement_pages_are_keyed_by_session_and_ids` (unaudited)
Impact: a session that can neither be deleted nor recovered
Open questions:
- A process-restart variant of F23 is not landed
- Who issues the retry: the route names the caller's next `session.delete`, and no caller retries today (needs human input)

### sc-deleted-session-not-resurrected-by-queued-capture

Type: safety
Reachability: explicit-config-only - a capture handoff exists only when `source_capture::consents` holds (`source_capture_enabled`, default `false`)
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` parks the job in `before_publish` before it takes the gate, completes `session.delete`, releases the job, and asserts `session_invalidated == 1`, an unchanged tip, and no live descriptor for the session; `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` (U2) bumps the epoch under the gate and shows the next pass captures the window again
Guarantee: A capture whose pass sampled the delete epoch before a `session.delete` commits nothing once that delete has run, because the job rechecks the epoch under `capture_commit_gate` before any Kernel write.
Check: `always(!X)` - X is a descriptor created after a delete by work whose pass began before it; the state has no detection point of its own, so the check is a forbidden-state invariant, not `unreachable`
Fault/timing angle: handoff accepted, delete runs, job runs; a job already holding the gate when the delete arrives (the delete waits, then retires what it published)
Required faults and enabling state: F37 a capture job parked before the gate while the delete runs
Confidence: high - [evidence](evidence/sc-deleted-session-not-resurrected-by-queued-capture.md). Read `SourceCapture::epoch`, `invalidate_sessions`, the epoch sample in `lib.rs`, `CaptureJob::publish`, and `publish_locked`'s first check; read both tests
Existing check: `crates/daemon/src/source_capture_tests.rs::a_delete_before_the_first_publication_drops_the_pending_capture` (unaudited); `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` (unaudited)
Impact: a deleted session reappears as live evidence
Open questions:
- Post-delete passes for the same harness session (OQ4): resolved within scope in U8; a harness session that keeps running republishes on its next pass, stated in the route doc; the tombstone that would refuse later captures is the product owner's decision (needs human input)
- The epoch is global: a delete drops every session's queued handoff and empties the memo; the next pass recaptures

### sc-repodoc-one-live-revision-above-historical-head

Type: safety
Reachability: explicit-config-only - the only producer is `CaptureJob::capture_documents`, reached when `consents` holds and the route's `inject_docs` is set; the Kernel guard applies to any `repository_documents` publication
Status: active
Exercised: yes - `crates/kernel/tests/kernel_source_descriptors.rs::repository_document_revisions_exceed_the_historical_head_including_retired_rows` publishes 1, 2, 3, retires 3, refuses 2, 3, 0, and a stale-head 4, checks domain precedence, reads the head, publishes 4, checks a second path, and runs a two-producer race (`retired_lineage_republished`); `crates/daemon/tests/harness_sources.rs::repository_documents_publish_untrusted_supersede_and_project` refuses 0 and 5 after retirement and republishes at head plus one; `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` drives revisions 1 to 5 through capture with two removals
Guarantee: Each `(project_id, repository_id, relative_path)` lineage has at most one live descriptor, and every published revision is at least 1 and strictly above the highest revision the lineage ever held, retired rows included, with the caller's expected head equal to that head.
Check: `always` - inside the publication transaction and over every store state; a state invariant
Fault/timing angle: delete then recreate; two producers reading one head
Required faults and enabling state: F10 a lineage retired at revision 3, then a publication at 2; F5 two producers proposing the same next revision
Confidence: high - [evidence](evidence/sc-repodoc-one-live-revision-above-historical-head.md). Read `publish_verified`'s guard, `historical_head`, both `source_lineage_head` readers, `Prepared::expected_head`, `plan_document`, and the three tests; confirmed document evidence is `canonical` retention, so expiry leaves the rows the head reads
Existing check: `crates/kernel/tests/kernel_source_descriptors.rs::repository_document_revisions_exceed_the_historical_head_including_retired_rows` (unaudited); `crates/daemon/tests/harness_sources.rs::repository_documents_publish_untrusted_supersede_and_project` (unaudited); `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` (unaudited); `a_document_is_judged_against_its_own_project_among_many` (unaudited); `crates/kernel/tests/kernel_source_descriptors.rs::retirement_preserves_domain_ownership_without_reusing_old_revision_ids` (baseline for messages, unaudited)
Impact: a stale capture revives a retired identity at a lower revision
Open questions:
- Scope confirmation (OQ1): the class landed in U9 and U10 on a ticket with no blocker; no recorded product-owner confirmation was found (needs human input)
- Guard scope across classes (OQ13): resolved in U9, `RepositoryDocuments` only
- The Kernel permits a gap above the head; contiguity is the producer's rule in `Prepared::expected_head`

### sc-repodoc-raw-bytes-separate-from-prompt-hash

Type: safety
Reachability: explicit-config-only - same producer and gates as the record above
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` reads the artifact bytes after the first publication and after a whitespace-only edit, compares `read_project_docs_canonical(...).canonical_hash` before and after that edit, and shows an unchanged file commits nothing and a reuse pass captures nothing; `document_capture_leaves_the_response_and_respects_inject_docs` shows equal response bytes with capture on and off; `crates/daemon/src/project_docs.rs` goldens pin the canonical hash
Guarantee: A captured document's artifact bytes equal the raw bytes the pass read for its prompt; a whitespace-only edit publishes a new revision while the canonical prompt hash is unchanged; an unchanged file publishes nothing, because the skip compares the live artifact digest and never the canonical hash.
Check: `always` - for every document publication the artifact equals the `DocumentRead::Text` buffer, and every skip is an equality of `payload_id(text)` with the live row's `artifact_digest`
Fault/timing angle: a swap or growth after open, closed by the bounded no-follow read; a CAS-retry attempt leaving stale reads, closed by emptying the slot per attempt
Required faults and enabling state: an edit touching only whitespace
Confidence: high - [evidence](evidence/sc-repodoc-raw-bytes-separate-from-prompt-hash.md). Read `read_document`, `read_project_docs_canonical`, `leave_reads`, `clear_reads`, `SourceCapture::offer`, and `plan_document`; read the tests; the CAS-retry clear is taken from PR #1103 and not re-read at both call sites
Existing check: `crates/daemon/src/source_capture_tests.rs::a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` (unaudited); `document_capture_leaves_the_response_and_respects_inject_docs` (unaudited); `crates/daemon/src/project_docs.rs::tests::project_docs_golden_matches_reference` (unaudited)
Impact: evidence of what the agent read that cannot reproduce the file
Open questions:
- The test compares `read_project_docs_canonical` directly, not the pass's stored `docs_hash`

### sc-repodoc-only-verified-missing-retires

Type: safety
Reachability: explicit-config-only - same producer and gates as the records above
Status: active
Exercised: yes - `crates/daemon/src/source_capture_tests.rs::unreadable_documents_are_counted_and_retire_nothing` arranges a symlink, 70 KiB text, 300 KiB text, non-UTF-8 bytes, and a mode-0 file (skipped as root) and asserts each count rises and the live row is unchanged; `a_renamed_document_retires_the_old_path_and_publishes_the_new`; `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires` covers removal and a missing file with no live row; `crates/daemon/src/project_docs.rs::tests::a_file_is_missing_only_from_a_directory_that_exists` makes a vanished root `Unreadable`
Guarantee: A `repository_documents` descriptor is retired only by a `DocumentRead::Missing` read of a file whose parent directory exists; `NotRegular`, `Oversize`, `NotUtf8`, `Unreadable`, and text above 64 KiB retire nothing; a rename retires the old path and publishes the new one.
Check: `always` - every retirement of a `repository_documents` descriptor is attributed to a `Missing` read through `retire_document`
Fault/timing angle: a transient permission error; an unmounted root; an editor's unlink-then-write save, which reads as a verified removal and republishes above the head on the next pass
Required faults and enabling state: F18 symlink, growth past a cap, non-UTF-8, permission error; F38 a missing parent directory
Confidence: high - [evidence](evidence/sc-repodoc-only-verified-missing-retires.md). Read `read_document`, `plan_document`, `retire_document`, and the other retirement paths; found no other production writer that retires the class
Existing check: `crates/daemon/src/source_capture_tests.rs::unreadable_documents_are_counted_and_retire_nothing` (unaudited); `a_renamed_document_retires_the_old_path_and_publishes_the_new` (unaudited); `crates/daemon/src/project_docs.rs::tests::a_file_is_missing_only_from_a_directory_that_exists` (unaudited); `symlinked_doc_is_skipped`, `a_doc_that_outgrows_the_cap_under_the_read_is_skipped`, `oversized_doc_is_skipped` (unaudited)
Impact: a transient read failure erases the revision trail
Open questions:
- A document past a cap keeps a live revision that no longer matches the file (needs human input)

## Gap records

### sc-consented-window-reaches-publisher

Type: reachability
Reachability: explicit-config-only - `Handler::handle_transform_typed` builds
  an `Offer` only when `source_capture::consents` holds, and
  `source_capture.enabled` defaults to false.
Status: active
Exercised: yes - `sc-tests`
  `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`
  drives an OpenCode window through the transform route and reads a
  `messages` descriptor, a `raw_tool_spans` descriptor with representation
  `tool_error`, and one with `tool_output`; `a_pi_window_publishes_user_text_and_tool_results`
  does the same for a Pi window, reading a `tool_output` and a `tool_error`
  row; `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`
  reaches the document batch.
Guarantee: Every OpenCode and Pi message text block, tool output, and tool
  error in a consented transform window, and every configured document the
  pass read, reaches `harness_sources::publish_screened` through
  `CaptureJob::publish_locked` or `CaptureJob::capture_documents`, unless the
  adapter refuses its record or a bound defers it.
Check: `reachable` - for each harness (OpenCode, Pi) and each class and
  representation (`messages` text, `raw_tool_spans` `tool_output` and
  `tool_error`, `repository_documents` text), a consented pass yields a live
  descriptor of that class and representation after the job settles.
  `reachable` because the obligation is that the production path executes
  for each kind of unit.
Fault/timing angle: none; the units arrive after the response settles, so
  each test settles the capture job before reading the store.
Required faults and enabling state: `source_capture.enabled` and
  `memory.enabled` on, a non-subagent session on a native serializer profile,
  and a window holding each kind of unit.
Confidence: high - [evidence](evidence/sc-consented-window-reaches-publisher.md).
  Read the call chain from `handle_transform_typed` and
  `capture_after_response` through `SourceCapture::offer`,
  `CaptureHandoff::into_job`, `CaptureJob::units`, and `publish_locked`, and
  the three tests.
Existing check: `sc-tests`
  `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`,
  `a_pi_window_publishes_user_text_and_tool_results`,
  `a_configured_document_is_one_lineage_whose_edits_supersede_and_removal_retires`;
  all `unaudited`.
Impact: a consented window whose units never reach the publisher leaves no
  source to recover after compaction, which is the gap R1 closes.
Open questions: None.

### sc-capture-state-has-one-durable-store

Type: safety
Reachability: explicit-config-only - the capture modules run only under
  consent; the scanned source is the production code of those modules.
Status: active
Exercised: yes - `sc.rs` unit test `capture_state_has_one_durable_store`
  scans the production source of `source_capture.rs`, `harness_sources.rs`,
  `tool_outcome.rs`, and `project_docs.rs` for file-write, SQLite, and store
  open calls.
Guarantee: Capture keeps durable state in the Kernel alone: its modules write
  through `KernelStore` commits, and the memo, the slot, and the document
  slot live in process.
Check: `always` - the production source of the capture modules contains no
  `rusqlite::`, `OpenOptions`, `fs::write`, `File::create`, `create_dir`,
  `MemoryStore::open`, or `KernelStore::open`. `always` because a second
  durable store would appear only through one of these calls in those
  modules.
Fault/timing angle: none.
Required faults and enabling state: none; the check reads the source.
Confidence: medium - [evidence](evidence/sc-capture-state-has-one-durable-store.md).
  The scan covers the four capture modules and the named call forms; a store
  opened through another module and handed to capture would pass it.
Existing check: `sc.rs` `capture_state_has_one_durable_store` (`unaudited`);
  the CI "Invented provenance" gate scans `harness_sources.rs`,
  `source_capture*`, and `project_docs.rs` for another property.
Impact: a second durable store for capture state is a stop condition of the
  specification; it would need its own retention, deletion, and recovery.
Open questions:
- Should the scan widen to the handler code in `lib.rs` that builds the
  offer? (unresolved, needs a `/testing:test-strategy` decision)

### sc-capture-resource-bounds

Type: safety
Reachability: explicit-config-only - the unit, batch, window, memo, and document bounds sit on the capture path behind `source_capture_enabled` (default `false`); the Kernel payload and text-field caps are `default-production` for every artifact ingest, and the summarizer digest bound is `explicit-config-only` because a digest reaches a model only through a summarizer fold, which needs a non-empty `model_chain` (`DaemonConfig::eidnara_folds`) that `DaemonConfig::default` leaves empty; this record claims these caps only as the outer limits capture sits under
Status: active
Exercised: partial - `crates/daemon/tests/harness_sources.rs::capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` publishes a 65 536-byte unit with equal artifact bytes and refuses a 65 537-byte unit with no evidence and no receipt while both siblings publish, then refuses it again; `capture_batch_refusals_take_no_admission_slot` and `capture_batch_defers_units_beyond_its_bounds` cover 32 units and 64 KiB; `crates/daemon/src/source_capture_tests.rs::a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one`, `the_capture_report_carries_no_unit_text` (`refused_window_too_large`), `a_window_past_the_remaining_budget_is_refused_resident_short`, `a_documents_text_is_charged_with_its_window`, and `unreadable_documents_are_counted_and_retire_nothing` (70 KiB document); `crates/daemon/src/source_capture.rs::tests::the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral`; `crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention` (outcome above 1 KiB); `crates/daemon/src/tool_outcome.rs::tests::every_digest_is_bounded_and_shows_only_its_outer_lines` and `a_digest_clips_on_character_boundaries_within_its_bounds`; `crates/kernel/tests/kernel_cas.rs::payload_limit_is_inclusive_at_the_artifact_cap` and `oversized_text_fields_are_rejected_before_staging`. Not exercised: a `raw_tool_spans` unit above 64 KiB through the capture path, and a window holding a very large tool output beside small siblings
Guarantee: Capture retains a unit only when its text is at most `MAX_SOURCE_UNIT_BYTES` (65 536 bytes) and then retains it whole; a larger unit is `unit_too_large` with nothing retained and no batch slot taken; a batch admits at most 32 units and 64 KiB of text and defers the rest; the supervisor holds at most `MAX_RETAINED_WINDOW_BYTES` (32 MiB) of charged window bytes plus `MEMO_BUDGET_BYTES` (4 MiB) of memo; every outcome record and every summarizer digest is at most 1 KiB; all of these sit under the Kernel's 64 MiB payload and 1 KiB text-field caps.
Check: `always` - every retained capture artifact equals its unit text and is at most 65 536 bytes, every committed batch holds at most 32 units and 65 536 text bytes, the supervisor's `retained_bytes` never exceeds 32 MiB, and every stored outcome record and every built digest encodes to at most 1024 bytes
Fault/timing angle: a window whose charge exceeds the retained budget is refused before the unit screen, so a tool output above about 16 MiB refuses its whole window on every pass that carries it
Required faults and enabling state: F31 a unit above 64 KiB in a batch with siblings, and a window above the retained budget; a 33-unit window; an outcome record above 1 KiB
Confidence: medium - [evidence](evidence/sc-capture-resource-bounds.md). Read every constant and the code that applies it at HEAD and every test named above; medium because the tool-output case is covered only through message units on a class-agnostic branch and the window-level refusal of siblings is untested
Existing check: `crates/daemon/tests/harness_sources.rs::capture_batch_refuses_an_oversized_unit_whole_and_publishes_its_siblings` (unaudited); `capture_batch_defers_units_beyond_its_bounds` (unaudited); `capture_batch_refusals_take_no_admission_slot` (unaudited); `an_invalid_outcome_record_is_refused_before_retention` (unaudited); `crates/daemon/src/source_capture_tests.rs::a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` (unaudited); `the_capture_report_carries_no_unit_text` (unaudited); `a_window_past_the_remaining_budget_is_refused_resident_short` (unaudited); `a_documents_text_is_charged_with_its_window` (unaudited); `crates/daemon/src/source_capture.rs::tests::the_memo_stays_within_its_byte_budget_and_keeps_the_first_deferral` (unaudited); `crates/daemon/src/tool_outcome.rs::tests::every_digest_is_bounded_and_shows_only_its_outer_lines` (unaudited); `crates/kernel/tests/kernel_cas.rs::payload_limit_is_inclusive_at_the_artifact_cap` (unaudited); `oversized_text_fields_are_rejected_before_staging` (unaudited); `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` (unaudited)
Impact: a truncated tool output presented as an exact source, unbounded resident memory from large windows, or one oversized unit silencing its window
Open questions:
- A test with a `raw_tool_spans` unit above 64 KiB through `CaptureJob` is not landed
- A tool output above about 16 MiB refuses its whole window, so R10's sibling rule holds only below the retained bound (needs human input)
- The host wire frame bound on the transform request was not located

## Relationship map

- Consent and handoff: `sc-consent-gate-admits-capture` gates every capture
  record; `sc-response-independent-of-publication`,
  `sc-one-outstanding-batch-refuse-newest`, and `sc-capture-resource-bounds`
  bound the handoff; `sc-pending-unit-publishes-next-ready-pass` and
  `sc-terminal-disposition-never-retried` split what the next pass retries.
- Identity and replay: `sc-adapter-identity-native-or-refused` feeds
  `sc-receipt-replays-equal-digest-conflicts-otherwise`, which
  `sc-v1-receipt-replays-legacy` extends across the digest version;
  `sc-rerun-exactly-one-live-generation` and
  `sc-repodoc-one-live-revision-above-historical-head` refine
  `sc-descriptor-revision-exceeds-live-predecessor-only` per class.
- Admission and disclosure: `sc-admission-fixed-class-pair-under-event-other`,
  `sc-unprovenanced-text-stores-sensitive-or-refuses-secrets`, and
  `sc-sensitive-denied-remote-hidden-from-auto-surfaces` are the Kernel rules
  `sc-capture-creates-no-decision` relies on.
- Retention: `sc-harness-capture-deadline-replay-never-renews` sets the
  deadline that `sc-expiry-descriptor-first-cited-survives` and
  `sc-expiry-drains-within-page-bound` enforce, on the pattern of
  `sc-capture-expiry-class-scoped-paged-citation-safe` and
  `sc-curator-capture-requires-finite-live-deadline`;
  `sc-retire-evidence-conflicts-while-cited` and
  `sc-live-evidence-reference-blocks-artifact-gc` keep cited bytes;
  `sc-readmission-blocked-after-purge-tombstone` bounds what returns after a
  purge.
- Summarizer: `sc-reduction-drops-body-keeps-failure-variant` is why the
  retained source matters; `sc-tool-call-line-carries-no-outcome`,
  `sc-digest-bounded-deterministic-char-boundary`, and
  `sc-digest-citations-never-reach-artifact` bound what the summarizer sees;
  `sc-outcome-observed-bounded-occurrence-bound` is the descriptor-side record.
- Deletion: `sc-delete-retires-before-memory-store-delete` replaces the
  invalidated `sc-session-delete-leaves-kernel-descriptors`;
  `sc-delete-retry-recovers-crash-between-stores` and
  `sc-deleted-session-not-resurrected-by-queued-capture` cover its windows.
- Repository documents: `sc-project-docs-canonical-hash-over-fixed-list` is
  the prompt-side read; `sc-repodoc-raw-bytes-separate-from-prompt-hash` and
  `sc-repodoc-only-verified-missing-retires` are the capture side.
- Production reach: `sc-consented-window-reaches-publisher` carries R1's
  reach and `sc-capture-state-has-one-durable-store` the single-store stop
  condition; `sc-harness-classes-have-no-production-publisher` is
  invalidated by U2; `sc-ready-snapshot-drops-native-messages` and
  `sc-unfinished-assistant-text-deferred` describe what the publisher sees.
