# Existing checks

Every claim-bearing check in the tree that touches this catalog's records.
Status is `unaudited` for all of them: adequacy belongs to a separate review.

## Semantic evidence corpus

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `corpus_*` (thirteen cases) | `crates/daemon/tests/memory_reviewer_coordinator.rs` | each case's cited bytes, score (valid and off-side citations, warranted and unwarranted claims, independent support), origin groups, limitations, and unchanged target tokens equal its declarations; the `[0,40)` prefix and an off-side citation score as valid citations and unsupported claims; the whole commit and a span of it are one origin and byte-identical commits in two repositories are two | unaudited |
| `the_corpus_covers_every_relation_once` | same | thirteen distinct labels; retain, retire, revise, no change, and abstention each appear | unaudited |

## Canonical resolution and target binding

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it` | `crates/daemon/tests/memory_reviewer_broker.rs` | both production classes resolve to `CanonicalSource` with the decision's id and live revision; the hold protects exactly the descriptor's evidence; two descriptors bind one equal `CanonicalTarget` whose commit token is the decision's last change, not its creation; `MEMORY_CLASSES` excludes `git_commits`; registry rows equal before and after | unaudited |
| `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target` | same | supersession refuses `OriginRevoked` from the bound target and from fresh resolution; a stale descriptor revision refuses `ExpectationChanged` at the broker with zero bytes; an unregistered owner refuses `NotFound`; a stale bound decision revision refuses `ExpectationChanged`; an evidence object as owner refuses `ExpectationChanged` at `proposal_target` and `Scope` at the broker with zero bytes; a decision in another project's scope as owner resolves but refuses `Scope` at the broker with zero bytes; tracked rows equal before and after | unaudited |
| `a_resolved_canonical_or_promoted_subject_still_refuses_the_remote_destination` | same | the Remote destination refuses a resolved canonical or promoted artifact `PolicyBlocked` with zero bytes | unaudited |
| `a_missing_or_empty_decision_identity_is_unsupported` | same | a stored identity with a missing or empty leading field refuses `Unsupported` at resolution | unaudited |
| `a_class_tightened_after_resolution_refuses_every_destination` | same | a promoted subject readable locally before the change refuses `PolicyBlocked` with zero bytes on the local and Remote destinations after the same bytes are ingested under `Secret` | unaudited |
| `a_deleted_artifact_refuses_the_execution_hold` | same | after evidence deletion the descriptor still resolves and its execution hold refuses `UnavailableEvidence` | unaudited |
| `canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both` | same | shared origin across the two forms; Remote `PolicyBlocked`; decision retirement revokes both forms; stale native revision refused | unaudited |
| `a_canonical_owner_must_be_a_live_decision` | same | a scoped observation as owner refuses `ExpectationChanged` | unaudited |
| `a_revalidation_pass_is_reused_only_at_the_kernel_state_it_passed_at` | same | a source's last revalidation pass is reused only at the Kernel egress state it passed at; inside a classification window the full judgement runs and refuses, and an advanced generation after the window forces the full judgement again | unaudited |
| `production_classes_are_refused_the_remote_model_at_selection_and_in_the_coordinator` | `crates/daemon/tests/memory_reviewer_worker.rs` | over the production claim materializer's three descriptors of one decision, each permitted locally and `SensitiveRemote` for the Remote destination: the production selector with no class override admits none; forced jobs over them settle `owner_sensitive` in the production worker with no attempt row, zero connections accepted or queued at the peer, a `not_selected` read, and Kernel object states unchanged | unaudited |
| `an_unserved_peer_counts_a_connection_it_never_answered` | same | positive control for the zero-connection count: a client that connects to a peer with no serving task, and sends nothing, is counted once | unaudited |
| `fixture_canonical_and_promoted_forms_reach_readable_proposals_targeting_their_decision` | same | fixture witness: a canonical and a promoted descriptor over test-supplied repository provenance go through the production selector, slot enqueue, and worker to `Complete` receipts and readable `retain` proposals whose target is the decision's id, revision, and last-change token; each cites its own form; no Kernel commit | unaudited |
| `the_production_classes_are_walked_in_order_and_a_cursor_resumes_in_the_second_class` | `crates/daemon/tests/memory_reviewer_selection.rs` | canonical claims precede promoted memories in the walk, and a cursor into the second class resumes there | unaudited |
| `memory_reviewer_review_selection_is_not_scheduled_while_production_selection_is_closed` | `crates/daemon/src/lib.rs` | an open activation gate schedules no `MemoryReviewerReviewSelection` while `PRODUCTION_SELECTION_OPEN` is `false`; a constant assertion forces the test to change when the gate opens | unaudited |
| `the_selected_classes_are_exactly_the_decision_derived_classes` | `crates/daemon/tests/memory_reviewer_broker.rs` | over every `OccurrenceClass`, `decision_derived` and `MEMORY_CLASSES` membership agree, so no class is selected without resolving through its decision or resolves through a decision without being selectable | unaudited |
| `const` assertion after `MEMORY_CLASSES` | `crates/daemon/src/memory_reviewer/selection.rs` | every walked class satisfies `decision_derived` at compile time; `related_memories::CLASSES` is the same constant, so discovery cannot walk a different set | unaudited |
| `a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | Git-only shared path from selection to a readable proposal; a native subject's target is the descriptor | unaudited |

## Private result expiry and reconciliation

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `review_transfer_acquires_before_releasing_and_moves_only_live_memory_reviewer_references` | `crates/kernel/tests/kernel_memory_reviewer_holds.rs` | candidate and run deadlines equal the queue deadline after transfer; live read at the deadline is `Expired`; selected read with a selection inside the window succeeds past the deadline, and one at the deadline refuses | unaudited |
| `one_envelope_stages_seals_and_moves_retention_to_the_review_hold` | same | one Kernel envelope seals the proposal, writes the review hold over the disclosed input, and releases the execution hold in one commit; a sealed run refuses a second envelope before writing | unaudited |
| `a_refused_transfer_leaves_the_proposal_sealed_and_the_execution_hold_live` | same | a transfer refused under its savepoint commits the sealed row, writes no review hold, and leaves the execution hold and its pins unchanged | unaudited |
| `a_transfer_refused_after_its_writes_rolls_back_to_the_savepoint` | same | a refusal at the transfer's last step rolls back the review pin and references it inserted | unaudited |
| `a_refused_seal_writes_nothing_of_the_envelope` | same | sealing at an instant earlier than an unsealed staging's heartbeat is refused and the envelope writes nothing | unaudited |
| `an_unselected_transferred_result_expires_with_its_queue_and_its_hold_is_reconciled` | `crates/daemon/tests/memory_reviewer_settlement.rs` | hold outlives the queue on its own clock; row keeps the queue deadline; reconciler keeps a pending hold; sweep closes the receipt `expired` at the run deadline; list shows the terminal; read is `not_selected`; reconciler releases the orphaned hold once and finds nothing twice; a late completion is fenced | unaudited |
| `a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel` | same | selection leaves the row deadline; `completed_at_ms` is the settlement clock; a sweep at the queue deadline leaves the complete receipt, its completion time, its selection, and its read untouched; read past the queue deadline succeeds while the hold is live and refuses `review_expired` at its expiry; live read past the deadline is `Expired`; reconciler leaves the selected hold | unaudited |
| `kernel_results_stay_private_until_the_receipt_selects_them` | same | sealed row readable by identity, public read `not_selected`, list empty; same-generation recovery reuses the hold; abstaining recovery releases it | unaudited |
| `the_sweep_closes_an_in_progress_receipt_at_the_queue_deadline_before_its_run_deadline` | `crates/memory-store/tests/memory_reviewer_ledger.rs` | a receipt with a run deadline past the queue deadline closes `expired` at the queue deadline with `completed_at_ms` set and no selection; the job expires with it; the reconciler's two questions answer false and empty | unaudited |
| `an_attempt_never_outlives_the_job_queue_deadline` | same | completion at the queue deadline is stale and writes nothing | unaudited |
| `a_selected_result_answers_only_for_its_project_digest_and_generation` | same | the reconciler's selection listing carries exactly the selection's digest, candidate, and generation, so another digest, another generation, or another candidate does not match | unaudited |
| `the_reconciler_releases_a_losing_generations_hold_and_keeps_the_winners` | `crates/daemon/tests/memory_reviewer_settlement.rs` | after a takeover the losing generation's hold is released as an orphan while the receipt is in progress at the next generation; the winner's hold survives its selection; the losing row stays sealed and unreadable through the receipt | unaudited |
| `a_sweep_inside_the_settlement_window_fences_the_selection_and_releases_the_hold` | same | the sweep closes the receipt `expired` between the Kernel envelope and the completion write; the completion is fenced; the settlement releases the hold; the reconciler finds nothing | unaudited |
| `a_selected_row_whose_owner_or_class_changed_refuses_the_read` | same | a stored witness naming another generation refuses `scope_mismatch`; a row reclassified `secret` refuses `dependency_refused`; the receipt is unchanged | unaudited |

## Broker-free recovery and full-lineage reads

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `a_resumed_claim_adopts_the_durable_result_without_the_broker_or_a_new_request` | `crates/daemon/tests/memory_reviewer_settlement.rs` | the staged record's union and marker; adoption under a fresh broker publishes the byte-identical reference with no added attempt; a second adopt is fenced; a takeover at generation 2 completes `unknown` and leaves the generation-1 row sealed | unaudited |
| `a_result_sealed_before_its_transfer_is_adopted_under_an_empty_execution_hold` | same | a row sealed before its transfer, the lost run's hold released, and a replacement hold covering nothing: adoption extends the hold over the record's inputs and publishes; the same window with a retired input abstains `expectation_changed`; with no hold at all it abstains `budget_exhausted` | unaudited |
| `a_sealed_untransferred_row_is_adopted_and_transferred_by_a_live_settlement` | same | a live settlement that finds its proposal sealed by an earlier attempt with retention not yet moved adopts the byte-identical row and transfers retention; a sealed row with other bytes conflicts and moves nothing | unaudited |
| `a_durable_result_whose_lineage_moved_or_lacks_a_record_is_not_adopted` | same | retired cited source abstains `expectation_changed` and releases the hold; edited union bytes abstain; a record joined to an attempt index the ledger lacks abstains; a record removed from the witness abstains; a completed marker with no row completes `unknown` | unaudited |
| `a_selected_read_refuses_when_an_uncited_member_no_longer_stands` | same | retiring an uncited disclosed source after selection refuses the read `dependency_refused`; the receipt is unchanged | unaudited |
| `the_staged_dependencies_are_the_brokers_union_including_uncited_inputs_and_ancestry` | same | a fabricated canonical member reaches the row's ancestry and the read refuses it | unaudited |
| `a_proposal_without_a_completed_marker_at_its_generation_is_not_staged` | same | a live settlement with only a `Failed` marker abstains `expectation_changed` and seals no row | unaudited |
| `schema_illegal_proposals_are_refused_at_decode` (staging rules) | `crates/kernel/tests/kernel_review_staging.rs` | a proposal spec without a record, a subject spec with one, another generation, another version, an empty or oversized union, and a short digest are each `Invalid` | unaudited |
| `a_resumed_generation_adopts_the_result_a_lost_run_sealed_without_a_send` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | a settlement that panics between the Kernel envelope and the ledger completion; the coordinator resumes the same claim, adopts the sealed reference under the transferred review hold, and the peer sees zero connections | unaudited |
| `an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | an unterminated marker completes the receipt `unknown` with zero connections and no added attempt | unaudited |
| `a_resumed_generation_with_a_cancelled_marker_completes_unknown_without_a_send` | same | a cancelled marker on a resumed run completes `unknown` with zero connections | unaudited |
| `a_not_dispatched_marker_alone_lets_the_run_proceed_with_a_new_attempt` | same | a recheck-lapsed `not_dispatched` marker admits one request and a second, completed attempt | unaudited |
| `a_hold_cap_refusal_on_resume_leaves_the_receipt_open_instead_of_abstaining` | same | a project at its active-hold cap refuses the resumed run's replacement hold; the run returns the Kernel refusal with the receipt still open and zero connections, and adopts the sealed reference once the cap clears | unaudited |
| `a_fresh_claim_carries_the_first_attempt_and_an_aged_one_is_renewed_before_the_next` | same | a claim acquired just before the run carries its first attempt with no renewal write; once provider time has aged the lease, the claim is renewed before the next attempt at the clock the read batch ran under | unaudited |
| `a_lease_at_the_slack_boundary_is_used_and_one_past_it_is_renewed` | same | the freshness window is inclusive: a lease with exactly the slack elapsed carries the attempt, and one a millisecond older is renewed first | unaudited |
| `a_run_cancelled_during_a_read_batch_returns_without_waiting_on_its_side_renewal` | same | cancellation during a read batch returns `Cancelled` while the renewal spawned beside the batch is still stalled before its lease-expiry read | unaudited |
| `the_live_claim_expiry_reads_this_jobs_claim_without_writing` | `crates/memory-store/tests/memory_reviewer_ledger.rs` | the expiry read reports the live claim's expiry and extends nothing; a renewal moves it; at its expiry the claim is not live, and a claim on another job never is | unaudited |
| `decoding_accepts_only_bytes_that_re_encode_to_themselves_and_their_digest` | `crates/context-core/src/memory_reviewer_policy_union.rs` | decode refuses a wrong digest, edited bytes, whitespace, and another version | unaudited |

## Observational routes

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `an_observational_binding_reads_its_project_and_takes_no_part_in_background_work` | `crates/daemon/src/lib.rs` | observer alone on a dormant project: worker, scheduler, root binding, and roster views empty, own route resolves; beside a live scheduled harness and on a second root: views equal the live snapshot; both close orders keep the other session bound | unaudited |
| `a_pass_over_a_view_without_the_project_leaves_its_ready_job_unclaimed` | `crates/daemon/tests/memory_reviewer_worker.rs` | gate open, Ready job, two passes over a hand-built empty view: no claim, no receipt, zero connections; a view naming the project runs it on the next pass | unaudited |
| `an_observational_route_reads_the_same_outcomes_as_an_ordinary_route` | `crates/daemon/tests/memory_reviewer_wire.rs` | `review.list`, `review.read`, a malformed identity, and an unknown field answer byte-equal through a `cli` route and the ordinary route | unaudited |

## Exact integers at the client seam

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `integer lexemes a double cannot reproduce arrive exact through exact-integer routed and control responses` | `packages/opencode-plugin/src/shared/host-client/client.test.ts` | raw tokens at the extrema and around 2^53 through `request` under `exactIntegers` and through `hostStatus`; count, u64, and i64 domains on the decoded values; a 21-digit lexeme refused with no token in diagnostics | unaudited |
| `a default routed response decodes as JSON.parse does, so module payloads forwarded to OpenCode never carry a bigint` | same | a `transform` recipe whose inserted message value carries 2^53+1 and a nanosecond timestamp decodes to rounded numbers and survives `JSON.stringify`; a 21-digit lexeme decodes as `1e20` | unaudited |
| `routeOpen omits the ambient consumer identity only when asked, for that bind alone` | same | the ambient pair is sent by default, omitted with `consumerIdentity: null`, and sent again on the next default bind; the environment is unchanged | unaudited |
| `stream items decode exactly only under the exact_json response mode` | `packages/opencode-plugin/src/shared/host-client/connection.test.ts` | the same stream item is a rounded `number` under the default mode and a `bigint` under `exact_json` | unaudited |
| `exact-json.test.ts` | `packages/opencode-plugin/src/shared/host-client/` | the reviver on every value shape, the width bound, refusal when `JSON.parse` withholds the lexeme, the three domains including an unsafe integer-valued double, human and raw JSON output | unaudited |

## Review command

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `parseReviewArgs` | `packages/cli/src/commands/review.test.ts` | defaults, bounds, and every rejected argument shape | unaudited |
| `review list` | same | route identity, envelope, single request, text and JSON rendering, terminals, states, malformed pages | unaudited |
| `review show` | same | one read, every field, inert text, exact integers, every read terminal, malformed proposals | unaudited |
| `review status` | same | `host.status` only, overlapping counters, unavailable over zero, not-ready block, absent block | unaudited |
| `connection lifecycle` | same | close on every path, codes without messages, absent connection file, no data directory, no context module | unaudited |
| `review over the host transport` | `packages/cli/src/commands/review.wire.test.ts` | real `HostClient` against the fake daemon: catalog probe, route identity without the ambient pair, envelope, exact decode, closed client, route refusal, status | unaudited |
| `review command against the direct host` | `packages/e2e-tests/src/rust-runner/review-cli.test.ts` | real handshake and flat envelopes against the hermetic host; refusals decoded; backend counters unchanged | unaudited |
| tarball smoke | `scripts/smoke-tarball-install.ts` | installed `eidnara review status`, `list`, and `show` reach a running daemon | unaudited |

## Response budgets

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `response_usage_accumulates_across_attempts_and_generations_and_exhaustion_refuses_before_a_marker_is_charged` | `crates/memory-store/tests/memory_reviewer_ledger.rs` | full allowance on the first marker; remainder after a completed and a failed response; usage past a ceiling refused; unterminated attempt at takeover exhausts the successor; `unknown` with bytes and `failed` without bytes exhaust; the ceiling-exact boundary | unaudited |
| `response_usage_columns_are_bounded_and_written_once_at_the_schema` | same | bytes on an in-flight row refused; a recorded terminal and its usage cannot be rewritten or cleared | unaudited |
| `responses_are_charged_against_the_jobs_remaining_allowance_and_refusals_record_known_consumption` | `crates/daemon/tests/memory_reviewer_model_request.rs` | a chunked body refused at the chunk crossing a 40 KiB remainder; text refused one byte past its remainder before allocation; a declared length past the remainder refused unread; a provider error body charged; the per-response constants still cap a larger allowance | unaudited |
| `responses_consume_the_jobs_raw_ceiling_across_attempts_and_exhaustion_sends_nothing_more` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | two 600 KiB responses through the real run; the second refused with the remainder recorded; the third round refused before a marker with no request byte; `budget_exhausted` | unaudited |
| `responses_consume_the_jobs_text_ceiling_across_attempts_and_exhaustion_sends_nothing` | `crates/daemon/tests/memory_reviewer_disclosure.rs` | two 40 KiB texts; the second refused before allocation with 24 KiB recorded; the third attempt refused with no request byte | unaudited |
| `cancelled_and_failed_responses_record_the_bytes_they_consumed` | same | a provider error body's bytes recorded; a cancelled read records the bytes read | unaudited |

## Suspiciously quiet areas

- No test exercises class tightening (a decision reclassified `Sensitive`
  after resolution) at the coordinator seam; the broker's `judge` folds the
  decision's sensitivity into the descriptor's on every read, which the broker
  tests cover for retirement but not reclassification.
- No test re-publishes the descriptor at a new revision after binding to show
  the bound target is unchanged; each revision is its own object, so the bound
  expectation cannot observe the new row, but no witness records that.
- No test drives the daemon lifecycle loop end to end through
  `sweep_and_sample` with a transferred hold; the reconciler is exercised
  directly, and the loop's ordering (sweep, reconcile, Kernel maintenance) is
  read from code.
- No test reopens either store between the Kernel envelope and the sweep; the
  durable rows are read through the same handles that wrote them.
- No test decodes a maximum-size response body full of twenty-digit tokens;
  the width bound is read from code.
- A reviver makes `JSON.parse` walk the value recursively, so on the exact path
  nesting a few thousand levels deep fails as invalid JSON where the plain
  parse accepts it; the daemon's serializer nests no deeper than 128, and no
  test pins the bound.
- No test drives a real `transform` pass end to end with a message value past
  2^53; the default-mode decode is asserted at the host client, and the recipe
  consumer's rejection of a `bigint` is read from `validateJsonValue`.
- No test drives `review show` to a rendered proposal against a real host; the
  selected read is reached in Rust and decoded in TypeScript from the same
  wire shape.
- No test exercises a request timeout or an aborted signal through the real
  transport in the command; the error path is exercised with thrown errors.
- No test measures wall-clock or memory for the padded-response path; the
  byte counts the tests assert are the resource evidence, and no latency or
  quality threshold is claimed.
- No test crashes between the response and the terminal write and then
  resumes; the unterminated-is-full rule is exercised by leaving an attempt
  open at takeover.
- No test drives observer open, `module_projects`, and a worker pass in one
  process; the view and the pass are exercised in two tests.
- No test records a `temporary_capture` member and adopts it; the member
  reconstruction for captures is read from code.
- No test constructs the window after the Kernel result and before the hold
  transfer; the execution hold expires on its own cutoff and the row keeps its
  queue deadline by the same mechanism the post-transfer tests witness.

## Kernel acceptance

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `retain_settles_once_without_a_canonical_change_and_replays_across_keys_and_after_the_hold` | `crates/kernel/tests/kernel_review_acceptance.rs` | settlement head fields; no change to the target's row, last change, or surface eligibility; replay under a second key without a commit; replay after the review hold is released; `review_settlement` lookup; a reused caller key with another digest conflicts | unaudited |
| `create_inserts_one_decision_admitted_as_model_inference_with_durable_support` | same | the created decision's registry identity; the stored admission (`model_inference`, `memory_classifier_inference`, `candidate`, `active`, `explicit_labeled`); explicit search serves it and auto-inject does not; the support is a linked citation and survives the hold; a replay writes no second decision | unaudited |
| `revise_writes_a_fresh_successor_that_inherits_no_maturity_and_supersedes_the_target` | same | fresh successor id; preserved domain and source identity at revision 2, and the predecessor's scope and kind whatever scope the caller names; predecessor invalidated, superseded by the successor, and no longer served; the stored admission is a `candidate`, `active`, `explicit_labeled` model inference although the predecessor was verified and auto-served; the successor cites the support; a second revise of the superseded target refuses and leaves the successor unchanged | unaudited |
| `an_approval_is_not_revised_into_model_written_text` | same | a revise whose target is an `adr_accepted` decision refuses `changed` without a commit | unaudited |
| `an_accepted_decision_serves_no_lower_than_the_support_it_cites_reads_today` | same | a created decision and a revised successor each serve to explicit search until a re-ingest tightens one of their support rows to `Secret`, after which explicit search refuses them | unaudited |
| `support_citation_cost_does_not_scale_with_own_history` | `crates/kernel/src/admission.rs` | the served read's cost for 200 support citations stays within 2x between one and 41 own-history rows | unaudited |
| `retire_runs_the_existing_retirement_and_reports_its_cascade` | same | the retirement's commit and zero cascade for a decision that grants nothing | unaudited |
| `retiring_an_approval_reports_the_dependents_its_cascade_demoted` | same | an accepted-ADR approval with one verified dependent reports `granted_authority_before` and one demotion | unaudited |
| `every_memory_action_refuses_a_target_that_moved_after_the_proposal` | same | retain, revise, and retire each refuse a touched target with no tip move | unaudited |
| `a_quarantined_owner_refuses_without_being_invalidated` | same | a live but quarantined target refuses `changed` | unaudited |
| `a_selection_past_the_queue_deadline_is_expired_but_a_settlement_still_replays` | same | a selection at the deadline is `review_expired`, one before sealing is `invalid`, and a settled proposal replays at the late selection | unaudited |
| `cleanup_and_purge_after_acceptance_keep_the_settlement_and_the_originals` | same | the predecessor's descriptor stays live after a revise; release, row deletion, and purge leave the successor live, explicitly served, and citing the purged evidence, and the settlement replayable | unaudited |
| `create_needs_a_scope_of_the_runs_project_and_no_change_on_a_subject_writes_nothing` | same | a missing scope is `invalid`; another project's or an unknown scope is `unavailable`; no-change on a subject settles unchanged; a deleted subject refuses | unaudited |
| `concurrent_acceptances_of_one_proposal_settle_it_once` | same | two threads: one applies, one replays the same settlement | unaudited |
| `a_caller_key_reused_for_another_proposal_conflicts` | same | the same caller key and digest for another proposal conflicts and settles nothing; a caller claiming the settlement producer is `invalid` | unaudited |
| `a_moved_target_another_project_a_released_hold_and_changed_evidence_refuse_and_write_nothing` | same | `changed` for a touched target, a redirected target, deleted and tightened evidence, and a retired owner; `unavailable` for another project's run; `invalid` for a run of another job; `review_expired` for a released hold; no tip move and no settlement for each | unaudited |
| `a_settlement_survives_a_reopen_and_replays_under_a_new_key` | same | the settlement replays from a reopened store | unaudited |
