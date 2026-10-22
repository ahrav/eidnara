# Lens records: group g5

Records for the U4, U5, and U6 obligations, verified against the HEAD of
`source-capture/1090-property-catalog` (`5a842d998`). References name files,
functions, constants, and tests.

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
Fault/timing angle: a hold or citation that lands between `expired_captures` under the reader lock and the per-capture writer commit; the recheck in `retire_expired_capture` shares the retirement's transaction
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

## Existing checks

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/tests/harness_sources.rs::harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it` | class and deadline on message and tool evidence; replay after deadline and after expiry keeps the deadline; `deadline - 1` and `deadline` sweeps; descriptor then evidence in one commit; canonical and repository-document rows untouched | unaudited |
| `crates/daemon/tests/harness_sources.rs::harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | live hold blocks expiry; cited capture keeps evidence and leaves the sweep; 65 captures drain in two calls; uncited capture retires later | unaudited |
| `crates/kernel/src/local_file.rs::a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept` | undated `harness_capture` ingest refused; pin inside the writer commit refuses `Conflict`; class isolation from the MemoryReviewer sweep | unaudited |
| `crates/daemon/src/memory_reviewer/lifecycle.rs::the_maintenance_slice_expires_harness_captures_and_reports_progress` | maintenance slice runs the harness sweep; capture report progress counters | unaudited |
| `crates/kernel/src/slice/write.rs::Envelope::retire_evidence` | production guard: `Conflict` while a live row cites the evidence | unaudited |
| `crates/daemon/tests/harness_sources.rs::opencode_tool_outcomes_follow_the_pinned_trace` | OpenCode outcome mapping over the pinned trace and malformed, absent, non-`bash` edits | unaudited |
| `crates/daemon/tests/harness_sources.rs::an_interrupted_call_waits_for_its_turn_to_complete` | interrupted call published only after turn completion | unaudited |
| `crates/daemon/tests/harness_sources.rs::pi_tool_outcomes_read_status_and_reported_truncation` | Pi outcome mapping | unaudited |
| `crates/daemon/tests/harness_sources.rs::equal_tool_bytes_with_different_outcomes_are_two_descriptors_of_one_artifact` | record binds to the occurrence; one artifact, two descriptors | unaudited |
| `crates/daemon/tests/harness_sources.rs::an_invalid_outcome_record_is_refused_before_retention` | refusal before retention on single and screened paths | unaudited |
| `crates/kernel/tests/kernel_source_descriptors.rs::a_tool_descriptor_stores_a_validated_outcome_record` | Kernel validation, 1024-byte edge, unchanged occurrence tuple | unaudited |
| `crates/kernel/tests/eval_identity.rs::the_core_encoder_reproduces_every_identity_golden_and_the_kernel_encoder` | identity goldens, unchanged by U5 | unaudited |
| `crates/daemon/tests/harness_sources.rs::receipts_written_before_the_outcome_record_replay_under_v2` | v1 receipts replay under v2; mismatches conflict; v1 evidence-only completion; v2 publish and replay | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_digest_shows_status_bytes_and_the_outer_nonblank_lines` | CRLF, single-line, empty, error-with-exit digests | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_digest_clips_on_character_boundaries_within_its_bounds` | multibyte excerpt clipping and 1 KiB call clip | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_wire_output_maps_its_variant_to_the_status` | wire variant to status mapping | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::a_status_spells_as_the_record_stores_it` | status spelling equals the stored record | unaudited |
| `crates/daemon/src/tool_outcome.rs::digest_tests::every_digest_is_bounded_and_shows_only_its_outer_lines` | proptest over bounds, excerpt equality, sentinel absence | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::the_c5_case_digest_shows_completed_and_the_refusal_in_tail` | C5.V1 digest | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::an_error_digest_shows_its_first_line_and_a_forged_marker_targets_nothing` | error digest; forged marker issues no target | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::tool_output_contains_sentinel` | sentinel absence at the chunk builder | unaudited |
| `crates/daemon/src/history_summarizer_chunk.rs::digest_chunk_tests::a_digest_cut_by_the_budget_withdraws_its_alias` | budget cut inside a digest withdraws its alias | unaudited |
| `crates/daemon/src/boundary.rs::digest_tests::the_boundary_chunk_renders_digests_for_answered_calls` | boundary site digest and sentinel absence | unaudited |
| `crates/daemon/src/history_summarizer_citations_golden.rs::rendered_aliases_match_the_hand_checked_native_oracles` | digest alias `transformed: true` in the citations golden | unaudited |
| `crates/daemon/src/history_summarizer_citations_golden.rs::a_forged_marker_in_native_text_cannot_keep_a_withdrawn_alias` | forged marker in native text | unaudited |
| `crates/daemon/src/history_summarizer_citations_golden.rs::truncation_withdraws_aliases_the_model_did_not_see_whole` | truncation withdrawal | unaudited |

None found: a test of the deadline on the `publish_screened` batch path; a
test validating a citation into a digest alias; a test of a persistent
per-row refusal in the harness sweep; a test republishing a unit whose
evidence-only row expiry retired.

## Fault map rows

| fault id | injection at HEAD | availability | records |
| --- | --- | --- | --- |
| F4 digest input change under one key | `store_receipt_as` rewrites a stored receipt to the v1 digest; changed `text` or `egress` on the publisher | available | sc-v1-receipt-replays-legacy |
| F11 `retain_until` versus clock | explicit `observed_at` on `publish`; explicit `now` on `expire_harness_captures`; `harness_capture` ingest accepts a past deadline | available | sc-harness-capture-deadline-replay-never-renews; sc-expiry-drains-within-page-bound |
| F12 pin acquired between select and commit | `capture_pins` and `capture_pin_refs` insert inside the writer commit (`a_harness_capture_pinned_after_it_was_selected_for_expiry_is_kept`); `capture_source_hold` before the sweep | available | sc-expiry-descriptor-first-cited-survives; sc-expiry-drains-within-page-bound |
| F13 evidence cited by an independent row | `cite` helper in `crates/daemon/tests/harness_sources.rs` (`insert_observation` of kind `note`) | available | sc-expiry-descriptor-first-cited-survives; sc-expiry-drains-within-page-bound |
| F14 more than 64 expired captures | 65 `publish` calls in `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` | available | sc-expiry-drains-within-page-bound |
| F16 budget truncation inside a part | `presented_input(built, budget)` with a searched budget | available | sc-digest-citations-never-reach-artifact |
| F17 forged alias marker in output | `«s9»` in an `OutputKind::ErrorText` | available | sc-digest-citations-never-reach-artifact |
| F33 absent or malformed tool outcome metadata | edits to cases of `testdata/opencode-tool-outcome-trace.json` in `opencode_tool_outcomes_follow_the_pinned_trace`; Pi `details` edits | available | sc-outcome-observed-bounded-occurrence-bound |
| F34 outcome record the Kernel refuses | mutate `SourceUnit.outcome` (`an_invalid_outcome_record_is_refused_before_retention`); `SourceDescriptorRequest.outcome` in the Kernel test | available | sc-outcome-observed-bounded-occurrence-bound |
| F35 adversarial tool output text | proptest `line()` strategy and fixed CRLF and multibyte cases in `tool_outcome.rs::digest_tests` | available | sc-digest-bounded-deterministic-char-boundary |

F33, F34, and F35 are new. The F1 to F25 set covers identity faults (F1, F2)
but has no fault for outcome metadata, an invalid outcome record, or the
output-text shapes the digest must clip, and the three records name those
inputs as their required faults.

Coverage witnesses these records need: `expired_capture_pinned_mid_sweep`
(sc-expiry-descriptor-first-cited-survives) and
`expired_capture_cited_elsewhere` (sc-expiry-descriptor-first-cited-survives,
sc-expiry-drains-within-page-bound), being named by the lead and not present
in the files at the HEAD read; `tool_output_contains_sentinel`
(sc-digest-bounded-deterministic-char-boundary), present as a test in
`history_summarizer_chunk.rs` and a doc reference in `boundary.rs`.

## Notes for the lead

- Reachability changed for the two digest records. The source labelled them
  `default-production`; the presented digest and its aliases exist only on a
  summarizer fold, and `DaemonConfig::eidnara_folds` requires a non-empty
  `model_chain`, which `DaemonConfig::default` leaves empty. Recorded as
  `explicit-config-only` with medium confidence. Whether the boundary
  trigger estimate formats digests under the default config is unresolved.
- Contract versus code, U5 and U6: the U5 ticket says one outcome extractor is
  shared with U6, and D9 says the digest is built from the U5 extractor. At
  HEAD both digest sites read wire blocks through `tool_outcome::from_output`
  and pass `exit_code: None`, so no digest shows `exit=N`;
  `tool_outcome::opencode` and `pi` feed only descriptors.
  `docs/compression-fidelity.md` states this behavior.
- Contract versus code, U4: the ticket says descriptors retire through
  `retire_observation`; the sweep calls `retire_capture_observation`, the
  path `retire_observation` takes for non-local-file ids. Same effect.
- `harness_capture` ingest requires a deadline but, unlike `curator_capture`,
  does not require it to be live at reference commit (`cas/ingest.rs`). No
  document names this either way.
- `Exercised` is `partial` on the deadline record: every U4 deadline test uses
  `SourcePublisher::publish`, while production capture uses
  `publish_screened`; both reach `evidence_request`.
- `FrozenAlias.transformed` has no production reader at HEAD; the citation
  guarantee rests on validation against `presented`.
- Citations moved from baseline: `hs.rs:794-795` (`canonical`/`None`) is now
  `evidence_request`; `hs.rs:950-958` and `:985-997` are now the `retained`
  map and `owners` in `publish_batch`; `envelope.rs:1301-1303` conflict is
  now absorbed by `stored_receipt`; `history_summarizer_chunk.rs:197-220` is
  `Builder::part`, and `:1304-1310` is `presented_input`; `cas/ingest.rs`
  deadline checks moved to the request check and the reference-commit check.
- Unresolved: a persistent per-row refusal starving the harness sweep page;
  an evidence-only receipt whose evidence expiry retired; the coverage
  witness names for F12 and F13 pending the lead's edit.
- Owner decisions carried open: OQ6 (row allowance) and OQ21 (re-admission
  after expiry).
