# Existing checks: P5 dependency surfaces

Authored at `3a3b916b7`; references re-verified at the #1164 branch tip
(base `979fb814e`), and the S8 table re-read after #1161. Every status is
`unaudited`: this pass records that a check
exists and what it asserts; adequacy belongs to
`/testing:invariant-test-review` (tests) and
`/low-level-systems:defensive-assertions-and-invariant-guards` (production
guards). An existing check never removes a record from the catalog.

## S1 retrieval analyzer (`crates/retrieval/tests/lexical_analysis.rs`)

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `byte_bound_is_checked_before_analysis_at_exact_and_over_bound_sizes` | `:244-277` | exact-size input passes; one byte over is `InputTooLong`; segments sum; bytes not scalars; NUL counts | unaudited |
| `atom_bound_refuses_the_next_atom_at_exact_and_over_bound_counts` | `:279-297` | exactly `max_atoms` passes; one more is `TooManyAtoms`; segments share the count | unaudited |
| `acronym_case_and_digit_boundaries` and the other goldens | `:51-218` | hand-authored atoms and parts per documented rule; file header `:1` states goldens are authored from the rules, never from analyzer output | unaudited |
| `duplicates_are_retained_in_both_columns` | `:180` | duplicates kept in input order | unaudited |
| `parts_never_exceed_the_scalar_values_of_their_atom` | `:198-217` | parts count bounded by scalar count | unaudited |
| `identity_pins_the_manifest` | `:389-412` | preimage carries every field; epoch equals `identifier-analysis.v1`; digest equals a literal | unaudited |

Production guards: the two refusals in `analyze_segments`
(`analysis.rs:83-88`, `:92-96`). No assertion on purity or on absence of case
folding exists as a check; the goldens imply both (e.g. `HTTPServer` stays
`HTTPServer` in atoms, `:192`).

## S2 scope-filtered Kernel read

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `a_scope_filter_keeps_rows_whose_redacted_term_the_algebra_reports_uncertain` | `crates/kernel/tests/kernel_scope.rs:235-388` | filter on `branch=main` keeps matching, redacted, and no-term scopes, drops the other branch; a redacted filter value keeps everything | unaudited |
| `the_domain_selection_equals_the_filtered_full_read` | `crates/kernel/tests/kernel_canonical_read.rs:227` | domain selection equals the full read filtered to the domain's decisions | unaudited |
| `the_domain_selection_folds_the_evidence_class_of_a_decision_kind_artifact` | `:286` | sensitivity folding for decision-kind rows | unaudited |
| `the_domain_selection_reaches_a_target_past_the_row_cap_of_the_unfiltered_order` | `:420` | domain read sees rows the capped unfiltered read drops | unaudited |
| `the_domain_selection_reads_no_row_of_another_domains_payload_pressure` | `:448` | another domain's payload volume does not cost the domain read | unaudited |
| in-module test at `crates/kernel/src/admission.rs:6240-6257` | | domain read on `memory` returns the one visible decision after a reopen | unaudited |
| `a_read_over_the_row_cap_serves_the_newest_rows_and_flags_truncation` | `crates/daemon/tests/kernel_routes.rs:1958` | `MAX_READ_ROWS` newest rows, `truncated: true` | unaudited |
| `bounded_selection_matches_a_full_sort_and_truncate` | `:2080` | `NewestRows` equals sort-then-truncate | unaudited |
| `a_read_over_the_byte_budget_serves_the_newest_rows_that_fit` | `:2112` | byte budget cut | unaudited |
| `rows_serve_only_to_the_project_their_scope_names` | `:1803` | cross-project rows absent | unaudited |
| `recorded_cross_project_read_is_empty` | `:1366` | | unaudited |
| `a_request_naming_another_project_root_is_refused_before_any_work` | `:1487` | project mismatch before work | unaudited |

Production guards: `CorruptCanonicalRow` when a visible decision lacks its
payload size or row (`read.rs:215-219`, `:239-243`).

None found: no test asserts the Kernel API's lack of a row bound (it is an
absence, not a check), and no test drives `visible_domain_decisions_as_of_in_scope`
with a `ScopeTermFilter` on `Dimension::Project` directly; the daemon tests
cover that path through `kernel.read`.

## S3 budgeted Kernel reads

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `decision_pages_examine_a_bounded_row_count_and_cut_before_the_byte_bound` | `crates/kernel/tests/kernel_slice.rs:1600-1760+` | `max_rows` counts invalidated rows; byte cut after one live row; a 1-byte bound still returns one decision; twin bounds one byte apart; cursor semantics; lineage commit | unaudited |
| `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly` | `crates/kernel/tests/kernel_claim_facts.rs:466-540+` | `TooManyClaims` for two ids under `max_claims = 1`; `DuplicateClaim`; `NotADecision`; `FutureSnapshot`; oversized causal payload is `Unknown(Oversized)` | unaudited |
| `an_exhausted_budget_is_refused_before_the_projection_is_read` | `crates/retrieval/tests/claims.rs:993-1014` | an exhausted budget answers `Deadline` before any read (the retrieval `claims` consumer of `claim_facts_at_within_budget`) | unaudited |
| `an_eligibility_read_against_held_kernel_readers_ends_at_the_budget` | `crates/daemon/tests/embedding_dispatch.rs:1102` | a held reader pool ends a budgeted read at its bound | unaudited |
| `exhausted_budget_is_refused_before_kernel_readers_are_taken` | `crates/daemon/tests/search_catchup.rs:1876` | | unaudited |

Production guards: `check_claim_bounds` (`claim_facts.rs:336-358`);
`AcquireLimit::check` (`open.rs:1535-1541`); progress handler installed by
`LimitedConnection::new` (`:1712-1720`).

None found: no test holds the reader pool against
`decision_page_within_budget` specifically; no test exercises a deadline
crossing inside a `claim_facts` statement (the progress handler path) for
`claim_facts_at_within_budget` itself as opposed to the acquire wait.

## S4 source-capture publication (`crates/daemon/src/source_capture_tests.rs`)

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `the_response_is_identical_whatever_capture_and_the_kernel_are_doing` | `:330` | response bytes independent of capture and kernel state | unaudited |
| `capture_latency_measurement` | `:938` | measured pass latency with capture | unaudited |
| `a_33_unit_window_publishes_32_and_the_next_pass_publishes_the_deferred_one` | `:437` | batch unit bound and deferral | unaudited |
| `shutdown_drains_an_in_flight_batch_and_a_panicking_job_releases_its_slot` | `:674` | slot released on panic; drain on shutdown | unaudited |
| `a_spent_budget_before_the_writer_commits_nothing` | `:1081` | exhausted budget publishes nothing | unaudited |
| `a_spent_budget_counts_the_document_reads_it_drops` | `:1105` | `documents_refused` on deadline | unaudited |
| `a_window_past_the_remaining_budget_is_refused_resident_short` | `:1242` | byte budget refusal | unaudited |
| `the_capture_report_carries_no_unit_text` | `:791` | report has no unit text | unaudited |
| `the_capture_report_reaches_host_status_field_for_field` | `:844` | closed vocabulary surfaces | unaudited |
| `capture_off_memory_off_or_a_subagent_session_runs_no_adapter` | `:298` | reachability gate | unaudited |
| `a_delete_after_the_handoff_publishes_nothing_for_the_queued_window` | `:1045` | delete epoch check | unaudited |
| `a_refused_window_still_captures_its_documents` | `:2236` | documents batch is separate from the window batch | unaudited |
| `a_settled_report_counts_the_job_that_released_the_slot` | `source_capture.rs:1797` | slot count after release | unaudited |

Production guards: `try_acquire_owned` else `RefusedQueueFull`
(`source_capture.rs:728-731`); `catch_unwind` around `publish_locked`
(`:996-1006`); `AfterResponse` run only on `RequestOutcome::Response`
(`lib.rs:13662-13664`).

None found: no test asserts that exactly one `Count` terminal is recorded
per job across every exit path (the tests count specific terminals per
scenario). No post-publish hook exists outside `cfg(test)` (`before_publish`,
`between_screen_and_commit`, `source_capture.rs:1536-1538`, `:805`, `:915`).

## S5 memory-store baseline

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `the_published_baseline_digest_is_the_one_every_open_checks` | `crates/memory-store/tests/baseline.rs:254-265` | `baseline_digest()` equals the stored `format_marker` | unaudited |
| `a_file_with_a_different_baseline_is_refused_as_a_baseline_mismatch` | `:458` | refusal message | unaudited |
| `a_store_created_under_the_prior_baseline_refuses_to_open_untouched` | `:492-514` | prior-baseline fixture refuses with `StoreError::Baseline`, bytes unchanged | unaudited |
| `fresh_open_creates_memory_sqlite_with_the_eidnara_identity_and_the_whole_baseline` | `:199` | inventory | unaudited |
| activation identity-mismatch table | `crates/daemon/src/memory_reviewer/activation.rs:491-495` | a changed `memstore_baseline_digest` closes as `IdentityMismatch("memory store baseline")` | unaudited |

Existing catalog records that apply:
`docs/properties/shared-primitives/catalog.md:676`
`store-schema-identity-matches-the-baseline` (active, audited at U2 per its
text) covers the open refusal for every consumer baseline.
`docs/properties/memory-store/catalog.md:717`, `:761`, `:819` are the
invalidated migration records; they document that no migration path exists.

## S6 harness-capture retention (`crates/kernel/src/local_file.rs` tests)

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | `:797` | writer recheck keeps a pinned row | unaudited |
| `a_refused_harness_capture_rolls_back_alone_inside_its_page_commit` | `:895` | page rollback and recommit without the refused capture | unaudited |
| `a_caller_cannot_seat_a_receipt_the_expiry_would_replay` | `:710` | reserved producer | unaudited |
| `a_replayed_expiry_receipt_is_not_counted_as_a_retirement` | `:728` | | unaudited |
| `expiry_candidate_cost_does_not_grow_with_retired_history` | `:1138` | SQL steps flat over retired history | unaudited |
| `the_maintenance_slice_expires_harness_captures_and_reports_progress` | `crates/daemon/src/memory_reviewer/lifecycle.rs:745` | an expired row is retired by one pass; `harness_expiry` reported | unaudited |

Production guards: `still_expired` recheck under the writer
(`local_file.rs:467-484`); `retire_evidence` refuses `Conflict` while cited
(`slice/write.rs:606-608`).

None found: no test constructs a `decision_events` citation (a dispute) on a
harness capture's evidence and runs the sweep; `cited_elsewhere` is tested
through the MemoryReviewer class only as far as this pass found.

## S7 routed `review.*` reads and `kernel.read`

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| `review_operations_are_disabled_without_module_authority_and_list_and_read_receipts_under_it` | `crates/daemon/tests/memory_reviewer_wire.rs:15` | `disabled` without MODULE authority; strict bodies (`:238-252`) | unaudited |
| `an_observational_route_reads_the_same_outcomes_as_an_ordinary_route` | `:260` | `cli` route reads | unaudited |
| `a_published_proposal_reads_from_every_root_after_a_newer_root_binds` | `:342` | | unaudited |
| `review_accept_applies_the_selected_proposal_only_under_the_owners_acceptance_record` | `:409` | `v` field is `invalid_params` (`:643-649`) | unaudited |
| wire-doc literal test | `crates/daemon/src/memory_reviewer/wire.rs:448-463+` | the doc's `review.accept` terminal set equals the produced set | unaudited |
| `an_observational_binding_reads_its_project_and_takes_no_part_in_background_work` | `crates/daemon/src/lib.rs:35928` | | unaudited |

None found: no test drives `store_unavailable` on `review.list`/`review.read`
through a failing store read, as far as this pass found (the terminal is
mapped at `wire.rs:137`, `:255`).

## S8 `eidnara_memory` tool surface

| Check | Location | Asserts | Status |
| --- | --- | --- | --- |
| "an explicitly empty allowlist admits no action and sends nothing to the daemon" | `packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1185-1196` | every action refused; zero transport calls | unaudited |
| "an omitted allowlist admits the default actions, which exclude dispute" | `:1197-1212` | `dispute` refused; zero transport calls | unaudited |
| "an omitted allowlist admits every default action" | `:1213-1222` | `get` admitted | unaudited |
| "a partial allowlist admits only the listed actions" | `:1223-1233` | `create` refused under `["get"]` | unaudited |
| approve/enforce/delete refusals | `:1150-1159` | | unaudited |
| "is a gated mutation: tool-call identity, agent, and allowlist" | `:1338-1354` | `dispute` refused without a tool-call id, from the context-researcher agent, and under `["get"]`; zero transport calls | unaudited |
| "is in the exposed schema exactly when the allowlist names it" | `:1356` | dispute fields follow the allowlist | unaudited |
| pi "rejects disallowed actions and malformed writes before any kernel call" | `packages/pi-plugin/src/tools/eidnara-memory.test.ts:303-326` | `list`, `approve`, `enforce` refused; zero transport calls | unaudited |
| pi "is absent from the default schema and refused by the default allowlist" | `:422-431` | default enum omits `dispute`; refusal; zero transport calls | unaudited |
| pi registration matrix | `packages/pi-plugin/src/tools/index.test.ts:40-70` | `memoryToolEnabled: false` omits `eidnara_memory` | unaudited |

At `3a3b916b7` no opencode test invoked the tool with `agent:
CONTEXT_RESEARCHER_AGENT` (`"context-researcher"`,
`packages/opencode-plugin/src/agents/context-researcher.ts:1`); since #1161
the dispute test (`tools.test.ts:1341`) observes the in-tool refusal at
`tools.ts:128-130` for `dispute`. No test asserts `isEidnaraMemoryMutation`
directly; it is exercised through the tool-call-identity error path
(`tools.test.ts:1340`). None found: no test calls `get` without a tool-call
id.

## Suspiciously quiet areas

- `visible_domain_decisions_as_of_in_scope` has no bound of its own and no
  test that measures its cost against a large same-project domain.
- `decision_page_within_budget` has one test and no held-reader test.
- The documents batch inside the capture job has its own tests but no test
  asserting the two-batch shape against the header's "one screened batch".
- The pi plugin's tool has no allowlist and no test of a disallowed action
  beyond the closed list.
