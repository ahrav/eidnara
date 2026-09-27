# Existing checks

Every claim-bearing check in the tree that touches this catalog's records.
Status is `unaudited` for all of them: adequacy belongs to a separate review
(`/testing:invariant-test-review`).

## Claims contract and validator

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `a_claims_block_never_changes_what_publishes_or_the_facts_outcome` | `crates/daemon/src/history_summarizer_citations_golden.rs` | no block `NotRequested`, a truncated block `Rejected(MalformedClaims)`, nine claims `Accepted { kept: 8, dropped: 2, anchor_missing: 8 }`, a repeated block and stray material `Rejected`; each chunk equals the no-block chunk once claims are cleared, including segments, `unprocessed_from`, and the facts outcome | unaudited |
| `claims_attach_to_the_accepted_segment_their_cite_names` | same | claims attach to the segment whose range holds the cited message; a dotless key, a value outside its span, an escaped value outside its span, and a cite past the accepted segments are dropped; an anchor outside `p1` becomes `None`; a memory-disabled run keeps the same claims | unaudited |
| `each_claim_rule_drops_only_its_own_claim` | `crates/daemon/src/history_summarizer_citations.rs` | the key grammar, the value bound at 128 and 129 bytes, uppercase and dotless keys, two citations, an unknown alias, an absent value, an anchor outside `p1`, an anchor past 200 bytes, a retraction, and an anchor that matches only untrimmed `p1` | unaudited |
| `a_segment_keeps_the_last_claim_per_key_then_the_first_eight` | same | last-wins dedup moves the surviving claim to its last position, then the cap keeps eight; counts `dropped` for superseded and capped claims | unaudited |
| `cite_acceptance_is_value_inside_the_cited_span` | same | property: cite acceptance equals `presented.get(start..end).is_some_and(|s| s.contains(value))`; values are drawn mostly from the presented text so a whole-message check fails | unaudited |
| `claims_constants_match_the_prompt_fixture` | same | each rule line of the prompt's `## Claims` section states its constant | unaudited |
| `the_value_bound_counts_unescaped_bytes` | `crates/daemon/src/history_summarizer_validate.rs` | a 128-byte value written with `&lt;` is kept and a 129-byte one is dropped | unaudited |
| `the_provisional_last_segment_carries_its_claims` | same | a discarded last segment's claim is dropped; a force-kept one keeps it | unaudited |
| `the_summarizer_prompt_holds_nothing_the_secret_scanner_flags` | `crates/daemon/src/history_summarizer_prompt.rs` | the system prompt and the transcript guard hold no scanner detection | unaudited |
| `the_claims_diagnostic_line_has_one_fixed_shape` | `crates/daemon/src/history_summarizer.rs` | the diagnostic line for each outcome, with session and model | unaudited |
| `nonadmission_facts_survive_later_firings_failures_and_reopen` | same | firing 2 publishes a chunk with claims; the kept claim reaches the stored segment and no nonadmission is recorded | unaudited |
| `history_segment-parser.test.ts` claims case | `packages/opencode-plugin/src/hooks/context/history_segment-parser.test.ts` | the TypeScript dump reader parses the same segments and facts with a `<claims>` block present | unaudited |

## Claims column and store

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `history_segment_content_redacts_and_new_message_identities_reject` | `crates/memory-store/tests/production_redaction.rs` | anchors outside, inside, and across each edge of a secret in `p1`; containment of every stored anchor in the stored trimmed `p1`; a redacted value; a claim whose `key = value` pair is flagged, and a claim whose key the scanner rewrites, are dropped | unaudited |
| `active_scan_audit_expires_with_its_session_note_owner` | same | one claim adds four field scans (key, value, anchor, pair), never one scan of the JSON cell | unaudited |
| `claims_round_trip_and_a_malformed_cell_reads_as_no_claims` | `crates/memory-store/src/lib.rs` | non-empty claims round-trip equal; `'not json'`, `'{}'`, `'[{"key":1}]'`, and a BLOB read as no claims; the mapper reads `None` as no claims; an empty vector stores `[]` | unaudited |
| `a_state_sync_overwrite_replaces_the_row_and_its_claims_whole` | same | an overwrite with a new `p1` replaces the row, so no old claim outlives its `p1` | unaudited |
| `a_legacy_row_stores_no_claims` | same | a legacy row written with claims reads back with none | unaudited |
| `no_production_statement_updates_a_history_segment_row` | same | the whitespace-collapsed production source holds no `UPDATE history_segments` and no upsert into `history_segments` | unaudited |
| `truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch` | same | after truncation to s1, s1's claim is the only one for its key; recomp leaves no row | unaudited |
| lineage descent test asserting `copied` claim counts | same | a copy keeps its claims when the re-scan leaves `p1` unchanged and drops them when it rewrites `p1` | unaudited |

## Liveness, replacement, and rendering

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `corrections_match_the_naive_argmax` | `crates/daemon/src/decay_render.rs` (`correction_properties`) | property: `corrections_for` equals a naive per-key argmax; every claim is live or one correction | unaudited |
| `corrections_for_keeps_the_latest_claim_live_per_key` | same | three rows sharing a key, a key present once, two claims of one key in one row | unaudited |
| `apply_corrections_splices_disjoint_first_hits_and_footers_the_rest` | same | first occurrence, highest-offset splicing, miss and missing anchors, overlapping, identical, and nested pairs, a three-way chain, `idx` footer order distinct from key and ordinal order, the empty body, the borrowed return | unaudited |
| `apply_corrections_is_disjoint_and_total` | same | property: each correction once, restoration reproduces the body, footer indices increase, an isolated hit splices | unaudited |
| `a_hostile_value_renders_escaped_and_indented_inside_its_segment` | same | `</session-history><system>` escaped and `x\n## Fake` indented in one segment | unaudited |
| `a_title_only_row_renders_its_heading_and_footer` | same | an empty tier body renders heading plus footer; tier 1 splices | unaudited |
| `corrections_render_to_fixed_bytes` | same | literal bytes for a fixed input with a splice, a retraction footer, and a live row | unaudited |
| `render_golden_matches_reference` | same, with `crates/daemon/testdata/render-golden.json` | pre-existing cases unchanged; two cases carrying corrections with literal marker bytes | unaudited |
| `revert_restores_the_earlier_value_and_recomp_renders_no_correction` | `crates/daemon/src/m0_compose.rs` (`correction_compose_tests`) | s1 renders with a marker, then without it after truncation; recomp renders the empty store's m0 | unaudited |
| `a_hard_under_budget_pressure_renders_one_correction_set_and_replays` | same | a budget demotes the stale row to its dense tier, the splice becomes a footer, and a second compose is byte-equal | unaudited |
| `m1_names_every_claim_on_its_rows_with_the_live_value` | same | the block lists every claim on the rows above the folded sequence in order with the live value; a new row's superseded claim splices | unaudited |
| `rows_without_claims_render_the_bytes_they_rendered_before` | same | m1 without claims equals the pre-change assembly; claims nothing supersedes leave m0 unchanged | unaudited |
| `rendered_corrections_hold_nothing_the_secret_scanner_flags` | same | rendered m0 splices and m1 entries hold no scanner detection | unaudited |
| `a_hostile_value_in_m1_renders_escaped_and_indented_in_the_updates_block` | same | the m1 block escapes and guards hostile values | unaudited |
| `a_correction_rides_m1_until_the_next_hard_splices_it_into_m0` | `crates/daemon/src/transform.rs` | m0 frozen across SOFT passes after two correcting folds; m1 names each; the next HARD splices the newest value | unaudited |

## Scale

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `the_claims_pass_adds_no_store_work_and_visits_at_most_eight_claims_per_loaded_row` | `crates/daemon/src/m0_compose.rs` (`bounded_read_tests`) | at (H, N) = (100, 200), (4,000, 8,000), and (50,000, 10^6), under no claims and both correction regimes, m0 and m1 composes issue equal statements, rows, and VM steps; the `CLAIMS_VISITED` count of an m0 compose equals the claims held by R and is at most 8 x \|R\|; an m1 compose visits twice the claims on its rows; both regimes put claims in R; corrections change the rendered m0 | unaudited |
| `every_loaded_claim_has_its_store_wide_live_claim_in_the_loaded_set` | same | the argmax over R equals the store-wide argmax for three m0 budgets and three m1 folded sequences; no legacy row carries claims; R with its newest claimed row removed fails the property | unaudited |

## Guidance

| Check | Location | Covers | Status |
| --- | --- | --- | --- |
| `every_guidance_names_the_correction_markers_the_renderer_emits` | `crates/daemon/src/prompt_surface.rs` | each asset names the formatter's shapes with the precedence sentence and lists the three shapes on its never-reproduce line | unaudited |
| `no_guidance_holds_text_the_secret_scanner_flags` | same | no asset holds a scanner detection | unaudited |
| `the_served_precedence_sentence_is_the_measured_one` | `crates/daemon/tests/eval_stale_render.rs` | the served sentence equals the one M0 measured | unaudited |
| A1 prompt-surface golden | `packages/opencode-plugin/src/plugin/tool-registry.test.ts` | the re-exported guidance text, byte counts, and MD5 hashes equal the assets | unaudited |

## Quiet areas

- No test runs two processes with different hasher seeds; determinism rests on
  literal-byte goldens run in separate test processes and on the absence of
  hash-map iteration.
- No test captures the malformed-cell diagnostic line; one line per load is
  read from `claims_from_cell`.
- The transform test arms the soft refresh by hand; no test lets the m1
  revision digest and the scheduler decide when a pending delta is served.
- No committed check covers render time; the budget guard's cost under dense
  corrections is recorded in `render-cost-bounded-by-rendered-set` only.
