# Generation: existing checks

Date: 2026-09-19. System: `crates/daemon` history summarizer generation.
Inspected HEAD: `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The [catalog](catalog.md#scope-and-evidence-boundary) records the supplied local
plan, its digest, and revision drift. External scope is that plan plus local
code, docs, and history; no additional incident logs or related repositories
are supplied. No test or CI command runs in this discovery lane.

All checks below have status `unaudited`. Inspection establishes their code and
assertion targets, not that they pass or prove the claimed property. Tables
inventory claim-bearing checks relevant to this part. Generic transaction,
retry, tier-floor, and renderer checks retain their endpoint ownership.

## Production checks

| Location and check | Condition and failure signal | Status |
| --- | --- | --- |
| [config.rs:139-145][config]; [lib.rs:5685-5692][lib] | An empty model chain produces `no_models`; configuration defaults to an empty chain. This is a reachability gate, not a fidelity check. | unaudited |
| [chunk.rs:197-221,271-274][chunk], `Builder::part` and `push_message` | Native marker brackets are escaped; only unchanged single text blocks are marked verbatim. No material-obligation check occurs. | unaudited |
| [chunk.rs:368-395][chunk], `flush_current_block` | A nonfirst block over budget returns its aliases and stops admission. The first block may exceed budget. | unaudited |
| [chunk.rs:1309-1340][chunk], `presented_input` | A truncated prefix keeps an alias only if its marker and complete presented bytes survive. It does not say which material facts survive. | unaudited |
| [validate.rs:271-289,588-643][validator] | One complete output root, usable segments, mapped boundaries, and no split terminal tool arc are required; failures return validation errors. | unaudited |
| [validate.rs:1136-1225][validator], `validate_parsed_history_segments` | Nonempty P1, legal ranges, coverage, and `unprocessed_from` are checked. The missing-tier message asks for P1-P4 although absent P2/P3 have already fallen back. | unaudited |
| [validate.rs:645-709,771-783][validator] | Discard-last and citable accepted range are computed separately. `debug_assert!(history_segments.len() <= emitted_count)` checks count monotonicity, not preserved meaning. | unaudited |
| [citations.rs:141-172][citations], `check_fact_set` | Citation count, alias existence, nonempty UTF-8 span, and accepted ordinal are checked. Errors include `MissingCitation`, `UnknownAlias`, `InvalidSpan`, and `OutsideAcceptedSegment`. Fact prose is not compared with source meaning. | unaudited |
| [history_summarizer.rs:2460-2497][driver], `publish_output_from_awaiting` | `length_capped` rejects even a closed document with a length-cap error; ordinary validation failure records abandonment. | unaudited |
| [history_summarizer.rs:643-666,735-753][driver], `publish_validated_chunk` | Fingerprint and publication-fence failures prevent a successful publish result. This part requires evidence attribution to that result, not a second fence invariant. | unaudited |

## Input, identity, and prompt checks

| Location and test | Assertion target and useful diagnostic | Status |
| --- | --- | --- |
| [prompt.rs:462-464][prompt], `xml_escaping_matches_prompt_reference_order` | Exact escaped strings. | unaudited |
| [prompt.rs:481-582][prompt], `history_summarizer_prompt_golden_matches_typescript_reference` | Exact seed selections, reference/memory blocks, and assembled prompt. Requires nonempty goldens, selection variation, seed fallback, and emitted reference/memory blocks. Diagnostics include `prompt mismatch` and `prompt golden never emitted session references`. | unaudited |
| [citations_golden.rs:88-145][golden], `rendered_aliases_match_the_hand_checked_native_oracles` | Message/block identity, presented bytes, transformed flag, block hashes, unique markers, rebuild, and serialization equality. | unaudited |
| [citations_golden.rs:149-184][golden], `the_prompt_notation_rule_ends_a_part_before_the_separator_the_renderer_emits` | The inter-part separator is excluded from presented bytes and prompt notation. | unaudited |
| [citations_golden.rs:285-307][golden], `multibyte_spans_must_fall_on_character_boundaries` | Valid multibyte ranges accept; cuts inside a scalar return `InvalidSpan`. | unaudited |
| [citations_golden.rs:347-385][golden], `truncation_withdraws_aliases_the_model_did_not_see_whole` | Only the intact first alias survives; citing the cut part returns `UnknownAlias`. This setup uses different build/present budgets. | unaudited |
| [citations_golden.rs:389-413][golden], `a_forged_marker_in_native_text_cannot_keep_a_withdrawn_alias` | Escaped source marker text cannot keep the cut second alias. | unaudited |
| [citations_golden.rs:417-453][golden], `withdrawal_matches_the_kept_text_across_every_budget` | Alias survival matches marker-plus-content presence. `some budget must keep a strict subset` is an existing local anti-vacuity assertion. | unaudited |
| [citations_golden.rs:457-482][golden], `an_oversized_first_block_at_the_same_budget_is_admitted_then_withdrawn` | A production-shaped single budget admits an oversized first block, then truncates it and withdraws its alias. | unaudited |
| [citations_golden.rs:486-505][golden], `appending_a_tail_does_not_change_the_frozen_range_aliases` | Later native input leaves frozen-range alias values unchanged. | unaudited |
| [citations_golden.rs:509-522][golden], `surrounding_whitespace_trimmed_from_a_single_block_is_a_transformation` | Trimmed assistant text is marked transformed; unchanged user text is not. | unaudited |
| [truncate_differential.rs:103-140][truncate-tests] | `optimized_matches_frozen_reference_at_production_windows`, `exact_token_budget_returns_original_input`, and `optimized_matches_frozen_reference` compare truncation with the frozen UTF-16 reference or unchanged input. They do not annotate material meaning. | unaudited |

## Acceptance and publication checks

| Location and test | Assertion target and limitation | Status |
| --- | --- | --- |
| [validate.rs:1656-1677][validator], `tierless_history_segments_reject_while_p1_only_output_keeps_soft_fallbacks` | Flat v1 output rejects; P1-only produces equal P1/P2/P3 and empty P4. No semantic comparison occurs. | unaudited |
| [validate.rs:1681-1712][validator], `mismatched_tier_close_parses_leniently_while_tierless_output_still_rejects` | A mismatched close yields the expected effective tier bodies. The test comment's provider attribution is not treated as a supplied incident. | unaudited |
| [validate.rs:1716-1730][validator], `lenient_tier_extraction_bounds_bodies_and_guards_overcapture` | A later opener bounds a tier body; expected parsed strings stay separate. | unaudited |
| [validate.rs:1897-1918][validator], `terminal_unprocessed_boundary_closes_a_completed_arc_forward` | Healing extends the terminal range and updates its boundary identity and publication floor. It does not inspect the summary's meaning about the absorbed result. | unaudited |
| [validate.rs:1922-1956][validator], `completed_arc_past_chunk_end_rejects_instead_of_publishing_half` and `discard_last_cannot_reopen_a_completed_arc` | An out-of-chunk result prevents accepted terminal coverage; discarding a segment cannot split the completed arc. | unaudited |
| [validate.rs:1960-1984][validator], `discard_last_uses_numeric_sparse_ordinal_distance` | A sparse numeric lookahead keeps the final segment and returns floor 3, rather than counting only present messages. | unaudited |
| [validate.rs:2038-2079][validator], `discarded_last_suppresses_every_side_channel_for_the_whole_run` | A citation into discarded coverage rejects the fact set; events, observations, and primers are empty. | unaudited |
| [validate.rs:2221-2239][validator], `force_keep_last_preserves_final_history_segment_and_side_channels` | Both segments remain, but only the earlier event remains. The name does not mean every side channel survives. | unaudited |
| [citations_golden.rs:188-281][golden], `a_fact_set_is_accepted_whole_or_rejected_whole_while_history_publishes` | Accepted/rejected fact sets, `NotRequested`, and `NoFacts` remain distinct; valid history remains in `ValidatedChunk`. Despite the name, this test calls validation, not storage publication. | unaudited |
| [citations_golden.rs:311-343][golden], `citations_outside_the_finally_accepted_segment_are_rejected` | Final-segment discard makes its citation invalid; the retained citation alone accepts. | unaudited |
| [citations_golden.rs:526-632][golden], `a_force_kept_final_segment_is_citable_and_an_unwrapped_block_is_still_a_fact_set` | Force-kept coverage remains citable. Unwrapped, malformed, unclosed, empty-bullet, and citation-only fact material produces distinct extraction results without losing valid history. | unaudited |
| [history_summarizer.rs:4154-4204][driver], `wired_history_summarizer_happy_path_sends_validates_and_publishes` | Store rows, P1 bytes, boundary identity, and publication floor are observed. Input is `placeholder prompt`, not case-linked real assembly. | unaudited |
| [history_summarizer.rs:4208-4258][driver], `selected_range_identity_drift_during_await_rejects_without_cooldown` | A changed selected fingerprint returns `FenceRejected`, leaves prior history only, and writes no transcript for the attempted range. | unaudited |
| [history_summarizer.rs:4262-4306][driver], `tail_identity_extension_during_await_still_publishes` | An unrelated tail extension preserves publication; useful control for source-drift attribution. | unaudited |
| [history_summarizer.rs:6139-6168][driver], `length_capped_closed_output_is_rejected_before_publish` | A closed but capped output does not append history and records a length-cap failure. | unaudited |
| [history_summarizer.rs:6172-6199][driver], `truncated_output_validation_reject_records_cap_hint` | A truncated, capped output records both validation rejection and the cap hint. | unaudited |
| [history_summarizer.rs:6389-6420][driver], `validation_rejection_advances_to_valid_fallback` | A rejected flat primary followed by valid tiered fallback completes under the fallback model, with two starts and a published row. | unaudited |

## Guidance and correction evidence

| Location and check | Assertion target and limitation | Status |
| --- | --- | --- |
| [tool-registry.test.ts:32-44,69-82][registry-tests] | The enabled registry has `eidnara_reduce`, `eidnara_search`, `eidnara_note`, and `eidnara_memory`; the disabled plugin registers nothing. This does not review the summarizer's recovery promises. | unaudited |
| [tool-registry.test.ts:272-319][registry-tests], `keeps guidance within the registered memory address and search-source contracts` | Checks memory addresses and memory-only search guidance in four named daemon assets. The summarizer prompt and generated hint footer are not among those assets. | unaudited |
| [search tools.test.ts:151-182,202-232][search-tests] | Empty sources issue no read, unsupported sources report errors, and text search ranks canonical memory summaries through `explicit_search`. | unaudited |
| [search tools.test.ts:428-439][search-tests], `advertises only the memory source in both the full and light descriptions` | Pins memory-only description strings. It does not establish arbitrary guidance meaning or exact-source recovery. | unaudited |
| [transform.rs:8721-8746][transform], `render_user_hint` | The UTF-16 cap assertion bounds hint size. Since #926 the footer offers a project-memory search for the fragments' topic instead of full-context recovery; the cap does not validate the footer's meaning. | unaudited |
| [history_summarizer.rs:3186-3219][driver], `stored_history_segment_importance_is_clamped_before_narrowing` | Checks default/clamp conversion of importance. It does not compare real importance outputs or effective serving after rubric changes. | unaudited |

The contradiction is inspected, not inferred from test names: prompt lines
[130,212][system-prompt] assume search recovery while [17,362][system-prompt]
disclaim it; [history hint candidates:8286-8316][transform] feed the footer,
but [the registered executor:172-260][search-execute] searches memory only.
This is static evidence for a named false premise, not an executed consumer
incident or a waiver of U5's witness and dependency requirements.

## Missing fidelity checks and suspiciously quiet areas

| Category | Finding |
| --- | --- |
| Shared C1-C6 corpus and source/revision/span admission | U1 (#718) adds `crates/daemon/testdata/compression-fidelity.json`, validated by `src/compression_fidelity_corpus.rs` with controls in `src/compression_fidelity_tests.rs`, unaudited. |
| Compiled/runtime corpus digest agreement | None found for compression fidelity. [cf-corpus-byte-identity][corpus-owner] in evaluation owns this check; generation consumes its result. |
| Answer-key exclusion and pre-output human material annotations | None found for compression fidelity. Prompt byte goldens do not establish this review process. |
| Native material obligation exposure | None found. Alias presence, ordinal coverage, and a `TC:` line are insufficient substitutes. |
| Temporal/polarity/evidence fidelity across title and body | None found. No inspected production guard performs entailment checking. |
| Scripted/real provenance and human review completeness | None found. The proposed evaluation command is absent. |
| Joined authored/parsed/healed/published provenance | None found for the fidelity corpus. Separate endpoint tests exist above. |
| Material generation situation marker | None found. Existing local anti-vacuity checks do not bind a material obligation to real prompt assembly and store publication. |
| Five per-situation campaign markers | None found for material transformation, actual-budget material truncation, rejected-primary/consumed-fallback, earlier-coverage/final-discard, or un-authored inherited P2/P3. Existing endpoint fixtures do not establish the required case-linked assembly. See the [fault map](fault-map.md#coverage-checks-to-add). |
| Joint producer/hint guidance capability review | None found. Existing four-asset guidance checks do not cover these owners together or record manual judgments against registry configurations. |
| U5 witnessed correction acceptance | None found for prior named failure, fresh same-case semantic review, required-obligation regression checks, and disclosed rubric/serving effects as one correction gate. |

The [MemoryReviewer corpus:44-70][curator-corpus] separates sources, scripts, and
expectations, but `judge` uses fixture-specific substring rules. It is a reuse
lead, not a fidelity oracle or evidence of a human-reviewed generation baseline.
The private [TestProducer::start:22431-22466][lib] records each attempt's
system text, user prompt, and model since U2 (#719), which is the complete
request capture generation claims need. No production export is needed.

The historical `run-history_summarizer-eval.ts` and `history_summarizer-eval`
corpus paths are absent from tracked HEAD. No matching invocation is found in
the current CI workflow. [CI:612-621][ci] declares Rust workspace tests and
doctests; this is configuration evidence only, not evidence of execution.
A separate [typed-producer mutation script:12-16,40-70][mutation] exists. It
replaces a backend failure with success and checks which test fails; it does
not check false authority, recall, or semantic fidelity. It is unaudited and
is not run here because it edits a test as part of its operation.

The scope survey counts lexical test attributes and assertion tokens, not
executed checks or assertion quality:

| Module | Lines | `test`/`tokio::test` attributes | Assertion macro tokens |
| --- | ---: | ---: | ---: |
| `history_summarizer_chunk.rs` | 1989 | 19 | 76 |
| `history_summarizer_prompt.rs` | 571 | 2 | 16 |
| `history_summarizer_validate.rs` | 1967 | 17 | 70 |
| `history_summarizer_citations_golden.rs` | 633 | 12 | 65 |
| `history_summarizer.rs` | 5536 | 50 | 249 |

This density is concentrated on mechanics. It does not close the semantic or
joined-path gaps. Existing-test strength belongs to
`/testing:invariant-test-review`; guard strength belongs to
`/low-level-systems:defensive-assertions-and-invariant-guards`.

[config]: ../../../../crates/daemon/src/config.rs
[lib]: ../../../../crates/daemon/src/lib.rs
[chunk]: ../../../../crates/daemon/src/history_summarizer_chunk.rs
[prompt]: ../../../../crates/daemon/src/history_summarizer_prompt.rs
[validator]: ../../../../crates/daemon/src/history_summarizer_validate.rs
[citations]: ../../../../crates/daemon/src/history_summarizer_citations.rs
[golden]: ../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[driver]: ../../../../crates/daemon/src/history_summarizer.rs
[truncate-tests]: ../../../../crates/daemon/tests/history_summarizer_truncate_differential.rs
[curator-corpus]: ../../../../crates/daemon/tests/support/memory_reviewer_corpus.rs
[ci]: ../../../../.github/workflows/ci.yml
[mutation]: ../../../../packages/e2e-tests/scripts/run-rust-history_summarizer-producer-mutation.ts
[corpus-owner]: ../evaluation/catalog.md#cf-corpus-byte-identity
[registry-tests]: ../../../../packages/opencode-plugin/src/plugin/tool-registry.test.ts
[search-tests]: ../../../../packages/opencode-plugin/src/tools/eidnara-search/tools.test.ts
[transform]: ../../../../crates/daemon/src/transform.rs
[system-prompt]: ../../../../crates/daemon/testdata/history_summarizer-system-prompt.txt
[search-execute]: ../../../../packages/opencode-plugin/src/tools/eidnara-search/execute.ts
