# Existing checks

Every claim-bearing check in the tree that touches this catalog's records.
Status is `unaudited` for all of them: adequacy belongs to a separate review.

## Canonical resolution and target binding

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `canonical_and_promoted_descriptors_resolve_to_their_originating_decision_and_target_it` | `crates/daemon/tests/memory_reviewer_broker.rs` | both production classes resolve to `CanonicalSource` with the decision's id and live revision; the hold protects exactly the descriptor's evidence; two descriptors bind one equal `CanonicalTarget` whose commit token is the decision's last change, not its creation; `MEMORY_CLASSES` excludes `git_commits`; registry rows equal before and after | unaudited |
| `a_moved_missing_stale_or_wrong_kind_owner_refuses_the_subject_and_the_target` | same | supersession refuses `OriginRevoked` from the bound target and from fresh resolution; a stale descriptor revision refuses `ExpectationChanged` at the broker with zero bytes; an unregistered owner refuses `NotFound`; a stale bound decision revision refuses `ExpectationChanged`; an evidence object as owner refuses `ExpectationChanged` at `proposal_target` and `Scope` at the broker with zero bytes; tracked rows equal before and after | unaudited |
| `a_resolved_canonical_subject_still_refuses_the_remote_destination` | same | the Remote destination refuses a resolved canonical artifact `PolicyBlocked` with zero bytes | unaudited |
| `canonical_and_promoted_forms_share_an_origin_and_a_revoked_decision_revokes_both` | same | shared origin across the two forms; Remote `PolicyBlocked`; decision retirement revokes both forms; stale native revision refused | unaudited |
| `a_canonical_owner_must_be_a_live_decision` | same | a scoped observation as owner refuses `ExpectationChanged` | unaudited |
| `a_canonical_subject_resolves_through_its_decision_and_abstains_for_a_remote_model` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | a canonical `ReviewTarget::Memory` subject reaches the coordinator, resolves, and settles `owner_sensitive` with zero connections, no attempt, and the decision and descriptor rows live | unaudited |
| `the_production_classes_are_walked_in_order_and_unproven_canonical_descriptors_are_not_selected` | `crates/daemon/tests/memory_reviewer_selection.rs` | the production walk passes Sensitive canonical descriptors without a job, through both classes and from a mid-walk cursor | unaudited |
| `memory_reviewer_review_selection_is_not_scheduled_while_production_selection_is_closed` | `crates/daemon/src/lib.rs` | an open activation gate schedules no `MemoryReviewerReviewSelection` while `PRODUCTION_SELECTION_OPEN` is `false`; a constant assertion forces the test to change when the gate opens | unaudited |
| `a_selected_eligible_memory_becomes_a_published_proposal_through_the_shared_path` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | Git-only shared path from selection to a readable proposal; a native subject's target is the descriptor | unaudited |

## Private result expiry and reconciliation

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `review_transfer_acquires_before_releasing_and_moves_only_live_memory_reviewer_references` | `crates/kernel/tests/kernel_memory_reviewer_holds.rs` | candidate and run deadlines equal the queue deadline after transfer; live read at the deadline is `Expired`; selected read with a selection inside the window succeeds past the deadline, and one at the deadline refuses | unaudited |
| `an_unselected_transferred_result_expires_with_its_queue_and_its_hold_is_reconciled` | `crates/daemon/tests/memory_reviewer_settlement.rs` | hold outlives the queue on its own clock; row keeps the queue deadline; reconciler keeps a pending hold; sweep closes the receipt `expired` at the run deadline; list shows the terminal; read is `not_selected`; reconciler releases the orphaned hold once and finds nothing twice; a late completion is fenced | unaudited |
| `a_completed_receipt_selects_the_staged_proposal_and_reads_pass_the_kernel` | same | selection leaves the row deadline; `completed_at_ms` is the settlement clock; a sweep at the queue deadline leaves the complete receipt, its completion time, its selection, and its read untouched; read past the queue deadline succeeds while the hold is live and refuses `review_expired` at its expiry; live read past the deadline is `Expired`; reconciler leaves the selected hold | unaudited |
| `kernel_results_stay_private_until_the_receipt_selects_them` | same | sealed row readable by identity, public read `not_selected`, list empty; same-generation recovery reuses the hold; abstaining recovery releases it | unaudited |
| `the_sweep_closes_an_in_progress_receipt_at_the_queue_deadline_before_its_run_deadline` | `crates/memory-store/tests/memory_reviewer_ledger.rs` | a receipt with a run deadline past the queue deadline closes `expired` at the queue deadline with `completed_at_ms` set and no selection; the job expires with it; the reconciler's two questions answer false and empty | unaudited |
| `an_attempt_never_outlives_the_job_queue_deadline` | same | completion at the queue deadline is stale and writes nothing | unaudited |
| `a_selected_result_answers_only_for_its_project_digest_and_generation` | same | the reconciler's selection question answers true for the selection's digest, candidate, and generation and false for another digest, another generation, or another candidate | unaudited |
| `the_reconciler_releases_a_losing_generations_hold_and_keeps_the_winners` | `crates/daemon/tests/memory_reviewer_settlement.rs` | after a takeover the losing generation's hold is released as an orphan while the receipt is in progress at the next generation; the winner's hold survives its selection; the losing row stays sealed and unreadable through the receipt | unaudited |
| `a_sweep_inside_the_settlement_window_fences_the_selection_and_releases_the_hold` | same | the sweep closes the receipt `expired` between the Kernel envelope and the completion write; the completion is fenced; the settlement releases the hold; the reconciler finds nothing | unaudited |
| `a_selected_row_whose_owner_or_class_changed_refuses_the_read` | same | a stored witness naming another generation refuses `scope_mismatch`; a row reclassified `secret` refuses `dependency_refused`; the receipt is unchanged | unaudited |

## Broker-free recovery and full-lineage reads

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `a_resumed_claim_adopts_the_durable_result_without_the_broker_or_a_new_request` | `crates/daemon/tests/memory_reviewer_settlement.rs` | the staged record's union and marker; adoption under a fresh broker publishes the byte-identical reference with no added attempt; a second adopt is fenced; a takeover at generation 2 completes `unknown` and leaves the generation-1 row sealed | unaudited |
| `a_result_sealed_before_its_transfer_is_adopted_under_an_empty_execution_hold` | same | a row sealed before its transfer, the lost run's hold released, and a replacement hold covering nothing: adoption extends the hold over the record's inputs and publishes; the same window with a retired input abstains `expectation_changed`; with no hold at all it abstains `budget_exhausted` | unaudited |
| `a_durable_result_whose_lineage_moved_or_lacks_a_record_is_not_adopted` | same | retired cited source abstains `expectation_changed` and releases the hold; edited union bytes abstain; a record joined to an attempt index the ledger lacks abstains; a record removed from the witness abstains; a completed marker with no row completes `unknown` | unaudited |
| `a_selected_read_refuses_when_an_uncited_member_no_longer_stands` | same | retiring an uncited disclosed source after selection refuses the read `dependency_refused`; the receipt is unchanged | unaudited |
| `the_staged_dependencies_are_the_brokers_union_including_uncited_inputs_and_ancestry` | same | a fabricated canonical member reaches the row's ancestry and the read refuses it | unaudited |
| `a_proposal_without_a_completed_marker_at_its_generation_is_not_staged` | same | a live settlement with only a `Failed` marker abstains `expectation_changed` and seals no row | unaudited |
| `schema_illegal_proposals_are_refused_at_decode` (staging rules) | `crates/kernel/tests/kernel_review_staging.rs` | a proposal spec without a record, a subject spec with one, another generation, another version, an empty or oversized union, and a short digest are each `Invalid` | unaudited |
| `a_resumed_generation_adopts_the_result_a_lost_run_sealed_without_a_send` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | a settlement that panics between the Kernel envelope and the ledger completion; the coordinator resumes the same claim, adopts the sealed reference under the transferred review hold, and the peer sees zero connections | unaudited |
| `an_unknown_attempt_outcome_completes_unknown_and_cancellation_joins_the_attempt` | `crates/daemon/tests/memory_reviewer_coordinator.rs` | an unterminated marker completes the receipt `unknown` with zero connections and no added attempt | unaudited |
| `a_resumed_generation_with_a_cancelled_marker_completes_unknown_without_a_send` | same | a cancelled marker on a resumed run completes `unknown` with zero connections | unaudited |
| `a_not_dispatched_marker_alone_lets_the_run_proceed_with_a_new_attempt` | same | a recheck-lapsed `not_dispatched` marker admits one request and a second, completed attempt | unaudited |
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
| `integer lexemes a double cannot reproduce arrive exact through routed and control responses` | `packages/opencode-plugin/src/shared/host-client/client.test.ts` | raw tokens at the extrema and around 2^53 through `request` and `hostStatus`; count, u64, and i64 domains on the decoded values | unaudited |
| `routeOpen omits the ambient consumer identity only when asked, for that bind alone` | same | the ambient pair is sent by default, omitted with `consumerIdentity: null`, and sent again on the next default bind; the environment is unchanged | unaudited |
| `exact-json.test.ts` | `packages/opencode-plugin/src/shared/host-client/` | the reviver on every value shape, the width bound, the three domains, human and raw JSON output | unaudited |

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

## Suspiciously quiet areas

- No test exercises class tightening (a decision reclassified `Sensitive`
  after resolution) at the coordinator seam; the broker's `judge` folds the
  decision's sensitivity into the descriptor's on every read, which the broker
  tests cover for retirement but not reclassification.
- No test places the originating decision in another project's scope; the
  Kernel's `WrongScope` verdict on the decision candidate maps to `Scope` in
  `judge`, but only the staged-subject scope test constructs it.
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
- A reviver makes `JSON.parse` walk the value recursively, so nesting a few
  thousand levels deep now fails as invalid JSON where the plain parse
  accepted it; the daemon's serializer nests no deeper than 128, and no test
  pins the bound.
- No test drives `review show` to a rendered proposal against a real host; the
  selected read is reached in Rust and decoded in TypeScript from the same
  wire shape.
- No test exercises a request timeout or an aborted signal through the real
  transport in the command; the error path is exercised with thrown errors.
- No test drives observer open, `module_projects`, and a worker pass in one
  process; the view and the pass are exercised in two tests.
- No test records a `temporary_capture` member and adopts it; the member
  reconstruction for captures is read from code.
- No test constructs the window after the Kernel result and before the hold
  transfer; the execution hold expires on its own cutoff and the row keeps its
  queue deadline by the same mechanism the post-transfer tests witness.
