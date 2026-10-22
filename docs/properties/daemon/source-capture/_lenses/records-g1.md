# Source capture: records lens g1

Lens: records for seven Part A slugs, rewritten into the METHOD schema and
re-verified at HEAD `5a842d998` (branch
`source-capture/1090-property-catalog`). References name functions, types,
constants, and tests. Abbreviations: `hs.rs` is
`crates/daemon/src/harness_sources.rs`; `hs-tests` is
`crates/daemon/tests/harness_sources.rs`; `sc.rs` is
`crates/daemon/src/source_capture.rs`; `sc-tests` is
`crates/daemon/src/source_capture_tests.rs`.

Reachability evidence shared by the capture-path records: the adapters
`opencode_units` and `pi_units` and the capture publisher run in production
only from `CaptureJob::units` and `CaptureJob::publish_locked` in `sc.rs`.
`Handler::capture_after_response` in `crates/daemon/src/lib.rs` starts that
job only when `source_capture::consents` returns true, and `consents` requires
`config.source_capture_enabled && config.memory_enabled && !is_subagent`.
`DaemonConfig::default()` sets `source_capture_enabled: false`
(`crates/daemon/src/config.rs`), and `ConfigKey::SourceCaptureEnabled` is
`TierClass::UserOnly`. Capture-path records are therefore
`explicit-config-only`.

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
Status: invalidated - U2 (#1082) added the production publisher: `Handler::capture_after_response` in `crates/daemon/src/lib.rs` hands a consented window to `SourceCapture::offer`, and `CaptureJob::publish_locked` in `sc.rs` calls `publish_screened`, which publishes `messages` and `raw_tool_spans` descriptors. The obligation now lives in `sc-consent-gate-admits-capture`, which requires that publication happen only under the consent snapshot.
Exercised: yes - `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` publishes `messages` and `raw_tool_spans` descriptors through the production handler, which refutes the baseline claim; `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` shows the key off, memory off, and subagent cases leave the kernel tip unchanged.
Guarantee: At baseline `08de9051a` a production daemon published no `messages` or `raw_tool_spans` descriptor; at HEAD it publishes them whenever consent admits capture.
Check: `always(!X)` - historical: after production transform passes, the live descriptor inventory for `messages` and `raw_tool_spans` is empty. This no longer holds with the key on; the replacement is the consent predicate in `sc-consent-gate-admits-capture`.
Fault/timing angle: none.
Required faults and enabling state: a daemon with `source_capture_enabled: true` and `memory_enabled: true`, a non-subagent session on a native serializer profile, and one transform pass with a user text block and a tool result.
Confidence: high - [evidence](evidence/sc-harness-classes-have-no-production-publisher.md). Production call chain read from `lib.rs` to `publish_screened`; the inverting test read.
Existing check: `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` and `capture_off_memory_off_or_a_subagent_session_runs_no_adapter`; `crates/eval-core/src/manifest.rs` `Ingestion::AdapterIngestedNoProductionCaller` (label only; its doc comment still says no adapter has a production caller); unaudited.
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
Existing check: `hs-tests` `publication_receipt_digest_layout_is_pinned`, `publication_receipt_conflicts_on_provider_egress_change`, `publication_receipt_distinguishes_delimiters_inside_fields`, `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`, `capture_batch_names_the_unit_whose_receipt_conflicts`, `receipts_written_before_the_outcome_record_replay_under_v2`; `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` (witness `replay_after_memo_loss`), `changed_bytes_under_one_identity_are_terminal_and_never_retried`, `a_concurrent_publication_of_a_unit_replays_or_conflicts`; unaudited.
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
- In the batch path a secret-bearing unit fails `PreparedArtifact::new` inside `ingest_exact_artifacts_with`, which maps the failure to `KernelError::InvalidInput` before the commit closure runs, so `BatchRefusal.unit` is `None`. `CaptureJob::publish_locked` then records no memo entry and counts `kernel_error`, and every admitted sibling is offered again on the next pass. Whether one secret-bearing tool output blocks its whole batch on every pass: unresolved, needs a capture-path test with a secret-bearing tool output beside a clean unit.

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

## Existing checks

All `unaudited`.

| Check | Covers |
| --- | --- |
| `hs-tests` `adapters_bind_native_identity_exactly_and_refuse_missing_identity` | identity tuples for both adapters; every refusal arm; Pi rows without `type` (witness `pi_wire_row_without_type`) |
| `hs-tests` `pi_refuses_unsupported_message_roles`, `opencode_refuses_unsupported_roles_before_projecting_any_parts` | role gates |
| `hs-tests` `pi_tool_results_require_boolean_error_status` | boolean `isError` |
| `hs-tests` `opencode_refuses_pi_namespace`, `pi_refuses_opencode_namespace` | harness mismatch |
| `hs-tests` `opencode_requires_parts_array` | `parts` shape |
| `hs-tests` `unfinished_assistant_text_is_deferred_but_settled_tools_publish` | deferred assistant text, settled tools publish |
| `hs-tests` `an_interrupted_call_waits_for_its_turn_to_complete` | interrupted tool deferred until turn completes |
| `hs-tests` `publication_receipt_digest_layout_is_pinned` | three pinned `source-request.v2` digests |
| `hs-tests` `publication_receipt_conflicts_on_provider_egress_change` | egress change conflicts, nothing staged |
| `hs-tests` `publication_receipt_distinguishes_delimiters_inside_fields` | length-prefixed digest |
| `hs-tests` `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically` | replay, `IdentityReused`, `RevisionAhead` at `i64::MAX`, succession |
| `hs-tests` `refused_bytes_and_identities_retain_nothing` | secret refusal; 600-byte identity refusal |
| `hs-tests` `mixed_sessions_create_pending_only_for_message_text` | fixed pairs; descriptor sensitivity equals evidence sensitivity |
| `hs-tests` `repository_documents_publish_untrusted_supersede_and_project` | repository document pair |
| `hs-tests` `capture_batch_names_the_unit_whose_receipt_conflicts` | batch conflict names its unit |
| `hs-tests` `receipts_written_before_the_outcome_record_replay_under_v2` | frozen v1 digest; v1 replay; neither-digest conflict |
| `hs.rs` `pins::revision_lead_matches_the_evaluator_pin` | lead constant equals the evaluator's |
| `crates/daemon/tests/eval_seam_probe.rs` | lead constant equals the evaluator's |
| `eval_ingestion.rs` `generated_observations_never_lead_and_a_boundary_fixture_refuses` | lead boundary: equal accepted, one more refused |
| `eval_ingestion.rs` `each_adapter_identity_field_flips_its_ids_and_payload_fields_flip_none` | identity fields move ids; payload fields move none |
| `git_sources.rs` `selected_commits_are_retained_exactly_and_rebuild_without_the_repository` | git pair |
| `kernel_admission.rs` `an_inert_citation_does_not_consume_approval_capacity` | `Other` keeps `Candidate` |
| `kernel_admission.rs` `visible_reads_exclude_decisionless_objects_and_apply_maturity_rows` | `Candidate` hidden from AutoInject |
| `kernel_exact_artifacts.rs` `a_secret_is_refused_before_persistence_with_a_reason_that_names_no_content` | exact lane refuses secrets |
| `kernel_exact_artifacts.rs` `oversized_and_non_utf8_payloads_are_refused_before_materialization` | exact lane refuses non-UTF-8 |
| `kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive` | clamp on the redacting lane; Remote denied |
| `sc-tests` `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss` | production publication; tool pair and `Sensitive`; replay after memo loss (witness `replay_after_memo_loss`) |
| `sc-tests` `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` | no publication without consent |
| `sc-tests` `changed_bytes_under_one_identity_are_terminal_and_never_retried` | conflict terminal in memo |
| `sc-tests` `a_concurrent_publication_of_a_unit_replays_or_conflicts` | second writer before the preview (F5, partial) |
| `sc-tests` `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen` | no decision; hidden after reopen |
| `sc-tests` `a_revision_lead_leaves_the_unit_pending` | F25 through capture, retryable and pending |
| `sc-tests` `pi_entry_identity_on_the_row_decides_capture_and_never_the_response` | Pi rows without `type` counted `adapter_refused` |
| `crates/eval-core/src/manifest.rs` `Ingestion::AdapterIngestedNoProductionCaller` | label only |

None found: a capture-path test of an unfinished assistant window; a capture-path
test of a secret-bearing unit beside a clean unit; a direct `is_permanent` table
test; a read-back of `event_kind` on a publication's admission row; a receipt
committed between preview and commit.

## Fault map rows

| fault id | injection at HEAD | availability | records |
| --- | --- | --- | --- |
| F1 missing or malformed native identity | field removal in `hs-tests` `adapters_bind_native_identity_exactly_and_refuse_missing_identity` | available | sc-adapter-identity-native-or-refused |
| F2 identity over 512 bytes | `"s".repeat(600)` in `refused_bytes_and_identities_retain_nothing` | available | sc-adapter-identity-native-or-refused |
| F3 restart / memo loss | `SourceCapture::forget_memo_for_test`; publish twice | available, witness `replay_after_memo_loss` | sc-receipt-replays-equal-digest-conflicts-otherwise |
| F4 digest input change under one key | mutate text, scope, egress, domain, or sensitivity; `store_receipt_as` with the v1 digest | available | sc-receipt-replays-equal-digest-conflicts-otherwise |
| F5 concurrent duplicate publication | `SourceCapture::before_publish` hook publishing from a second `SourcePublisher` | partial - before the preview only; preview-to-commit window missing | sc-receipt-replays-equal-digest-conflicts-otherwise |
| F7 scope missing or mismatched | existing scope tests | available | sc-adapter-identity-native-or-refused |
| F8 secret-bearing or non-UTF-8 bytes | `SECRET` fixture via `publish`; non-UTF-8 at `ingest_exact_artifact` | available for single publish; batch path constructible, no test | sc-unprovenanced-text-stores-sensitive-or-refuses-secrets |
| F19 Pi row without `type`/`parentId` | `{id, message}` fixture | available, witness `pi_wire_row_without_type` | sc-adapter-identity-native-or-refused |
| F25 native timestamp ahead of daemon clock | `now_ms() + 2 h` in `a_revision_lead_leaves_the_unit_pending`; `revision + lead + 1` in `eval_ingestion.rs` | available | sc-revision-lead-refused-and-retryable |
| F26 assistant record observed mid-turn | `time.completed` null or absent; `state.metadata.interrupted` | available | sc-unfinished-assistant-text-deferred |

F26 is new: F1 covers a missing identity, while a missing `time.completed` on
an assistant record is a legal mid-turn state that defers rather than refuses,
so it needs its own id.

`sc-harness-classes-have-no-production-publisher` and
`sc-admission-fixed-class-pair-under-event-other` need no fault.

## Notes for the lead

- Contract versus code: `crates/eval-core/src/manifest.rs` doc comment on
  `enum Ingestion` says "No ingestion adapter has a production caller". At HEAD
  `CaptureJob::units` calls both adapters in production behind the consent key.
  The label is stale; `docs/properties/evaluator/evidence/ing-adapter-ingested-no-production-caller.md`
  shows as modified in the worktree, so another agent may be handling it.
- Contract versus code (batch secret): `publish_screened`'s doc lists size,
  identity, revision, and scope refusals as per-unit; an artifact refusal is
  not per-unit. In `ingest_exact_artifacts_with` a failed
  `PreparedArtifact::new` maps to `KernelError::InvalidInput` with no unit, so
  a secret-bearing tool output refuses its whole capture batch and nothing is
  memo-recorded. Read, not executed. Recorded as an open question on
  `sc-unprovenanced-text-stores-sensitive-or-refuses-secrets`.
- The capture publisher asserts `Sensitivity::Sensitive` itself
  (`CaptureJob::publish`), so for captured units the stored `Sensitive` comes
  from the assertion, not the unprovenanced clamp. The clamp is exercised
  through the `Normal` corpus publisher in `hs-tests`.
- Ticket U2 lists `sc-unfinished-assistant-text-deferred` under "pending
  state". At HEAD deferred text yields no unit, so it is absent rather than
  `Disposition::Pending`. Open question recorded.
- Scope changes since baseline: `RevisionAhead` now exempts
  `repository_documents` (U9/U10) and inherited units; the unfinished-text
  record gained the interrupted-tool deferral (U5); the digest is
  `source-request.v2` with a v1 probe (U5); the request digest also binds the
  outcome record.
- Moved citations: every baseline `hs-tests` line number moved; the record
  names tests instead. Baseline `hs-tests:888` is now
  `unfinished_assistant_text_is_deferred_but_settled_tools_publish`;
  `hs-tests:1515` (fixed pairs) is `mixed_sessions_create_pending_only_for_message_text`;
  baseline `hs.rs:1146` pin is `pins::revision_lead_matches_the_evaluator_pin`.
- `sc-admission-fixed-class-pair-under-event-other` lowered from `yes` to
  `partial`: no test reads `event_kind`, `candidate_id`, or
  `approval_object_id` on a publication's admission row.
- `sc-revision-lead-refused-and-retryable` raised to `yes` and `high`: the
  boundary and the capture-path retry are both constructed.
