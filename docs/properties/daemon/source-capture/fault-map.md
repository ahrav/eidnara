# Source capture: fault map

Fault classes F1 to F25 are the specification's
([#1066](https://github.com/ahrav/eidnara/issues/1066), comment 2). F26 to
F38 are added here, each because no earlier class covers the input a record
needs. The injection column names what the tree offers at authoring time.

## Fault classes

| Fault | Injection | Availability | Records |
| --- | --- | --- | --- |
| F1 missing or malformed native identity | field removal in `hs-tests` `adapters_bind_native_identity_exactly_and_refuse_missing_identity`; a record naming another session (`the_capture_report_carries_no_unit_text`) | available | sc-adapter-identity-native-or-refused; logs |
| F2 identity over 512 bytes | `"s".repeat(600)` in `refused_bytes_and_identities_retain_nothing` | available | sc-adapter-identity-native-or-refused |
| F3 restart / memo loss | `SourceCapture::forget_memo_for_test`; publish twice; `SourceCapture::forget_memo_for_test`; drop handler and `KernelStore::open`; `SourceCapture::forget_memo_for_test` | available, witness `replay_after_memo_loss`; available; not paired with a terminal unit; available | sc-receipt-replays-equal-digest-conflicts-otherwise; terminal disposition; no decision; sc-rerun-exactly-one-live-generation |
| F4 digest input change under one key | mutate text, scope, egress, domain, or sensitivity; `store_receipt_as` with the v1 digest; same OpenCode id and `time.created`, changed text; `store_receipt_as` rewrites a stored receipt to the v1 digest; changed `text` or `egress` on the publisher | available | sc-receipt-replays-equal-digest-conflicts-otherwise; terminal disposition; logs; sc-v1-receipt-replays-legacy |
| F5 concurrent duplicate publication | `SourceCapture::before_publish` hook publishing from a second `SourcePublisher`; two producers reading one `source_lineage_head` in the Kernel test | partial - before the preview only; preview-to-commit window missing; available for `repository_documents` at the Kernel; missing elsewhere | sc-receipt-replays-equal-digest-conflicts-otherwise; sc-repodoc-one-live-revision-above-historical-head |
| F6 Kernel closed / `Deadline` | handler built without a Kernel; `before_publish` sleeping past `CAPTURE_BUDGET`; `kernel.set_expected(true)` with the Kernel unopened | available | response independence; pending unit; logs; sc-delete-retires-before-memory-store-delete |
| F7 scope missing or mismatched | existing scope tests | available | sc-adapter-identity-native-or-refused |
| F8 secret-bearing or non-UTF-8 bytes | `SECRET` fixture via `publish`; non-UTF-8 at `ingest_exact_artifact` | available for single publish; batch path constructible, no test | sc-unprovenanced-text-stores-sensitive-or-refuses-secrets |
| F9 revision regression, same lineage | publish two revisions in either order | available | sc-descriptor-revision-exceeds-live-predecessor-only |
| F10 retired lineage then lower revision | `retire_observation`, republish lower (`retirement_preserves_domain_ownership_without_reusing_old_revision_ids`); retire revision 3, publish 2 | available, tested; available | sc-descriptor-revision-exceeds-live-predecessor-only; sc-repodoc-one-live-revision-above-historical-head |
| F11 `retain_until` versus clock | `now_ms() - 1` at ingest; sweep `now` argument; explicit `observed_at` on `publish`; explicit `now` on `expire_harness_captures`; `harness_capture` ingest accepts a past deadline | available; ingest reads the wall clock; available | sc-curator-capture-requires-finite-live-deadline; sc-harness-capture-deadline-replay-never-renews; sc-expiry-drains-within-page-bound |
| F12 pin acquired between select and commit | `acquire_execution_hold` or raw `capture_pins` insert before `retire_expired_capture`; `capture_pins` and `capture_pin_refs` insert inside the writer commit (`a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`); `capture_source_hold` before the sweep | available, tested; witness `expired_capture_pinned_mid_sweep`; available | sc-capture-expiry-class-scoped-paged-citation-safe; sc-expiry-descriptor-first-cited-survives; sc-expiry-drains-within-page-bound |
| F13 evidence cited by an independent row | `insert_observation` citing the evidence (`note_observation`, `cite`); `cite` helper in `crates/daemon/tests/harness_sources.rs` (`insert_observation` of kind `note`) | available, tested; witness `expired_capture_cited_elsewhere`; available | sc-retire-evidence-conflicts-while-cited; sc-capture-expiry-class-scoped-paged-citation-safe; sc-expiry-descriptor-first-cited-survives; sc-expiry-drains-within-page-bound |
| F14 more than 64 expired captures | 65 captures in `retained_captures_do_not_starve_newer_expired_captures`; 65 `publish` calls in `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | available, tested; available | sc-capture-expiry-class-scoped-paged-citation-safe; sc-expiry-drains-within-page-bound |
| F15 far-future GC sweep | `run_staging_maintenance(far future)` over a live row | available, no test | sc-live-evidence-reference-blocks-artifact-gc |
| F16 budget truncation inside a part | `presented_input(built, budget)` with the budget search in `a_digest_cut_by_the_budget_withdraws_its_alias`; `presented_input(built, budget)` with a searched budget | available | sc-tool-call-line-carries-no-outcome; sc-digest-citations-never-reach-artifact |
| F17 forged alias marker in output | U+00AB/U+00BB text in `an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing`; `«s9»` in an `OutputKind::ErrorText` | available | sc-tool-call-line-carries-no-outcome; sc-digest-citations-never-reach-artifact |
| F18 symlink, FIFO, sparse oversize, swap | `project_docs.rs` test fixtures; non-UTF-8 by golden-style write; parent-directory swap has no fixture; `unreadable_documents_are_counted_and_retire_nothing` arrangements | available (unix) except the swap; available (unix; permission case skipped as root) | sc-project-docs-canonical-hash-over-fixed-list; sc-repodoc-only-verified-missing-retires |
| F19 Pi row without `type`/`parentId` | `{id, message}` fixture | available, witness `pi_wire_row_without_type` | sc-adapter-identity-native-or-refused |
| F20 drop with each `OutputKind` | the eight cases in `reduced_tool_result_keeps_failure_variant_and_output_extras`; frozen id in `frozen_arc_blocks_never_re_emitted` | available | sc-reduction-drops-body-keeps-failure-variant |
| F21 remote disclosure request | `ArtifactDestination::Remote`; route `"remote"` | available, tested | sc-sensitive-denied-remote-hidden-from-auto-surfaces |
| F22 slot busy / second pass racing | `SourceCapture::before_publish` hook (cfg(test)) blocking the first job | available | one outstanding batch; response independence; pending unit |
| F23 stop between Kernel commit and memory-store delete | `#[cfg(test)]` `SourceCapture::stop_after_retirement` returning `injected_stop`; `SourceCapture::stop_after_retirement` (in-process); `retire_session_pages` with `pages = 1` | available, in-process only (no process kill); available in-process; process restart missing | sc-session-delete-leaves-kernel-descriptors (inverting witness); sc-delete-retires-before-memory-store-delete; sc-delete-retry-recovers-crash-between-stores |
| F24 digest purged then re-observed | `delete_artifact(Purge)` then ingest under a new key | available, tested without a finite class | sc-readmission-blocked-after-purge-tombstone |
| F25 native timestamp ahead of daemon clock | `now_ms() + 2 h` in `a_revision_lead_leaves_the_unit_pending`; `revision + lead + 1` in `eval_ingestion.rs`; `time.created` two hours ahead | available | sc-revision-lead-refused-and-retryable; pending unit |
| F26 assistant record observed mid-turn | `time.completed` null or absent; `state.metadata.interrupted` | available | sc-unfinished-assistant-text-deferred |
| F27 expired capture reclaimed by ordinary GC, identical bytes re-observed under a new key | `expire_harness_captures`, `run_staging_maintenance(retain_until + 14 d)`, then publish a new unit with equal bytes | constructible, no test | sc-readmission-blocked-after-purge-tombstone |
| F28 consent predicate false at the pass (new: no existing id covers config gates) | `capture_config(false)`, `memory_enabled: false`, `is_subagent: true`, route harness mismatch | available | consent gate |
| F29 consent changed between pass and job (new: snapshot timing) | none; needs a rebind while the `before_publish` hook holds the job | constructible, no test | consent gate |
| F30 Kernel writer held by a foreground commit (new: distinct from Starting) | `KernelStore::commit` holding its closure on a thread | available | response independence |
| F31 unit text above 64 KiB beside siblings (new: size fault) | `"b".repeat(MAX_SOURCE_UNIT_BYTES + 1)` in `publish_screened`; 70 KiB document; `MAX_SOURCE_UNIT_BYTES + 1` unit; a window above the retained budget | available at the publisher seam; no transform-path test; available for message units and windows; missing for tool units through capture | unit cap; sc-capture-resource-bounds |
| F32 window above the supervisor byte budget (new: window bound) | a 2 MiB or 9 MiB JSON string window through `SourceCapture::offer` | available | one outstanding batch; logs |
| F33 absent or malformed tool outcome metadata | edits to cases of `testdata/opencode-tool-outcome-trace.json` in `opencode_tool_outcomes_follow_the_pinned_trace`; Pi `details` edits | available | sc-outcome-observed-bounded-occurrence-bound |
| F34 outcome record the Kernel refuses | mutate `SourceUnit.outcome` (`an_invalid_outcome_record_is_refused_before_retention`); `SourceDescriptorRequest.outcome` in the Kernel test | available | sc-outcome-observed-bounded-occurrence-bound |
| F35 adversarial tool output text | proptest `line()` strategy and fixed CRLF and multibyte cases in `tool_outcome.rs::digest_tests` | available | sc-digest-bounded-deterministic-char-boundary |
| F36 reordered or duplicate generation delivery (new) | `publish_screened` with windows in order 2,1 and 1,2,1 | available | sc-rerun-exactly-one-live-generation |
| F37 capture job parked before the gate across a delete (new) | `SourceCapture::before_publish` hook | available | sc-deleted-session-not-resurrected-by-queued-capture |
| F38 missing parent directory (new) | `read_document` on `dir/gone/ARCHITECTURE.md` | available | sc-repodoc-only-verified-missing-retires |

## Coverage witnesses

Each witness is a constant marker registered in `MARKERS`
(`crates/eval-core/src/markers.rs`) with the test that owns it. The test
asserts the independent preconditions of a vulnerable window, then records
the marker with `eval_core::Coverage::record` and checks
`Coverage::complete` for its own test path, so the marker fires on a correct
implementation and a test that stops reaching the window fails. None asserts
the violation. `the_marker_registry_is_unique_and_incomplete_until_every_marker_fires`
in `crates/eval-core/tests/render.rs` keeps the names unique.

| Witness | Location | Preconditions asserted |
| --- | --- | --- |
| `replay_after_memo_loss` | `crates/daemon/src/source_capture_tests.rs` `a_rerun_keeps_one_live_generation_across_a_restart` | a published generation, then an emptied memo before the next pass |
| `expired_capture_pinned_mid_sweep` | `crates/kernel/src/local_file.rs` `a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | the reader selects the expired row, then a pin lands before the writer commit |
| `expired_capture_cited_elsewhere` | `crates/daemon/tests/harness_sources.rs` `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | an independent row cites the first expired capture in page order before the sweep |
| `retired_lineage_republished` | `crates/kernel/tests/kernel_source_descriptors.rs` `repository_document_revisions_exceed_the_historical_head_including_retired_rows` | a lineage retired at head 3, then a publication at 4 |
| `tool_output_contains_sentinel` | `crates/daemon/src/history_summarizer_chunk.rs` `tool_output_contains_sentinel` | the output holds the sentinel, and neither outer nonblank line does |
| `pi_wire_row_without_type` | `crates/daemon/tests/harness_sources.rs` `adapters_bind_native_identity_exactly_and_refuse_missing_identity` | the `{id, message}` row a plugin sends when it cannot read the entry |
| `handoff_refused_slot_busy` | `crates/daemon/src/source_capture_tests.rs` `the_response_is_identical_whatever_capture_and_the_kernel_are_doing`, the slot-full section | the first job is active inside publication when the second pass hands off |
| `reversed_generation_delivery` | `crates/daemon/src/source_capture_tests.rs` `a_rerun_keeps_one_live_generation_across_a_restart` | an older generation of a call delivered after a newer one published |

## Per-property required faults

Each record's `Required faults and enabling state` line in
[`catalog.md`](catalog.md) names its faults by these ids.

## Leverage ranking

Cheapest valid oracle first, for the checks still missing:

1. F5 preview-to-commit window: a `before_publish`-style hook between the
   screened preview and the descriptor commit settles the concurrent-duplicate
   arm of `sc-receipt-replays-equal-digest-conflicts-otherwise`.
2. F31 through the transform route with a tool unit: one pass settles
   `sc-unit-cap-refuses-whole-never-prefix` for `raw_tool_spans`.
3. F15: one far-future `run_staging_maintenance` over a live row settles the
   remaining arm of `sc-live-evidence-reference-blocks-artifact-gc`.
4. F27: GC reclaim then re-observation settles the code reading behind OQ21.
5. F29: a rebind while `before_publish` holds a job settles the snapshot
   timing of `sc-consent-gate-admits-capture`.
6. F8 in a screened batch: one secret-bearing unit beside clean siblings
   settles whether the whole capture batch refuses.
7. F23 with a process restart: the cross-process half of
   `sc-delete-retry-recovers-crash-between-stores`.
