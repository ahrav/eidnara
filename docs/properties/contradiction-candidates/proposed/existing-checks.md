# Existing checks: P5 proposed obligations

Verified at the #1164 branch tip (base `979fb814e`, scorer commits
`05acd0107` and `446c7363f`). Every status is
`unaudited`: this file records that a check exists and what it asserts;
adequacy belongs to `/testing:invariant-test-review` (tests) and
`/low-level-systems:defensive-assertions-and-invariant-guards` (production
guards). An existing check never removes a record from the catalog.

## S-A scorer (`crates/daemon/tests/contradiction_candidates_scorer.rs`)

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `term_sets_follow_the_documented_fold_and_floor` | `:31-80` | hand-written term sets: fold, floor after fold, parts, digit boundaries, private-use and underscore atoms, combining marks, duplicates, `İ` | case fold | unaudited |
| `one_identifier_can_satisfy_the_term_gates` | `:82-98` | one three-part identifier holds four terms and passes the gates | none directly | unaudited |
| `a_contained_summary_scores_full_and_disjoint_texts_score_nothing` | `:100-129` | a contained summary scores 6 of 6; a disjoint pair is `Below { matched_terms: 0, .. }` | oversize (refusal is not score 0) | unaudited |
| `case_folds_identically_on_both_sides_and_diacritics_do_not_fold` | `:131-159` | case-only difference scores 5 of 5 and equals self-score; plain-`e` and NFD spellings score 0 of 4 | case fold | unaudited |
| `a_case_change_inside_an_identifier_changes_its_parts` | `:163-171` | `configLoader` gives {configloader, config, loader}; `CONFIGLOADER` and `configloader` give {configloader} | case fold (the case change crosses a part boundary) | unaudited |
| `identifiers_in_the_descriptor_match_prose_in_the_summary` | `:173-185` | `HTTPServer::configLoader` matches "HTTP server config loader" 4 of 5 | none directly | unaudited |
| `the_count_floors_block_short_or_thin_overlaps` | `:187-222` | each of the three gates blocks; exactly 1/2 passes | none directly | unaudited |
| `an_oversize_descriptor_is_refused_whole_and_an_exact_fit_is_analyzed` | `:224-241` | bounds `{65536, 32769}`; 65536 bytes with 32768 atoms analyzed; 65537 bytes is `InputTooLong` | oversize | unaudited |
| `an_unscorable_summary_is_reported_and_the_others_still_score` | `:243-274` | bound 4097 atoms; 8193-byte and floor-emptied summaries unscorable in id order; at-bound and plain summaries score, checked by kept ids only | oversize (partial: no comparison against a run without the unscorable summary) | unaudited |
| `the_top_k_is_the_same_under_every_input_order` | `:276-318` | K + 1 ties, a lower-score wider match, and an unscorable summary give an equal result over 18 input orders | determinism | unaudited |
| `a_decision_object_id_appears_once_and_the_better_entry_wins_in_any_order` | `:320-346` | one entry per object id, the better-ranked one, in both orders | determinism | unaudited |
| `the_matcher_digest_pins_every_rule` | `:348-400` | the full preimage, `ANALYSIS_CONTRACT_EPOCH`, the matcher epoch, and the digest literal | determinism, case fold (pins the rules) | unaudited |
| `a_non_ascii_spelling_that_folds_to_ascii_matches_the_ascii_spelling` | `:404-425` | the Kelvin sign spelling `\u{212A}elvin` folds to {kelvin} and matches `Kelvin` and `kelvin` across the two sides, 3 of 4 | case fold | unaudited |
| `a_summary_with_more_than_64_distinct_terms_scores_every_term` | `:429-452` | 70 `termNN` atoms hold 71 terms and score 41 of 71 against 40 of them; the uppercase spelling plus `École` scores 41 of 72 | none directly | unaudited |

## S-A scorer, differential (`crates/daemon/tests/contradiction_candidates_scorer_differential.rs`)

`reference` (`:16-65`) applies the documented rules directly: atoms and
parts, `str::to_lowercase` per term, the floor on scalar values, a
`BTreeSet`, and containment with the three gates.

| Check | Location | Asserts | Records | Status |
| --- | --- | --- | --- | --- |
| `term_sets_match_the_documented_rules` | `:179-189` | `term_set` equals the reference under both bounds for generated mixed ASCII and non-ASCII text | case fold | unaudited |
| `summary_scores_match_the_documented_rules` | `:191-205` | `DescriptorTerms::score` equals the reference for generated descriptors and summaries | case fold, oversize (unscorable versus score 0) | unaudited |
| `summaries_drawn_from_the_descriptor_score_as_the_rules_say` | `:207-230` | summaries built from the descriptor's own words, some uppercased, score as the reference | case fold | unaudited |
| `ascii_summaries_with_many_distinct_terms_score_as_the_rules_say` | `:235-245` | ASCII summaries of up to 160 numbered words, past 64 distinct terms, score as the reference | none directly | unaudited |
| `score_descriptor_keeps_the_reference_top_k` | `:247-284` | `score_descriptor` keeps the reference's top K in rank order and lists the reference's unscorable ids | determinism | unaudited |

Production guards: the compile-time assertion that the descriptor bound is
at least `MAX_SOURCE_UNIT_BYTES`
(`crates/daemon/src/contradiction_candidates/scorer.rs:66-68`); the
analyzer's refusals (`crates/retrieval/src/lexical/analysis.rs:78-105`),
which the scorer propagates.

Analyzer checks the scorer relies on:
`crates/retrieval/tests/lexical_analysis.rs:244-297` (bounds) and
`identity_pins_the_manifest` (`:389-413`); see
`../dependencies/existing-checks.md` S1.

## S-B to S-H and R5

None found. The trigger, candidate set, store, sweep, read, flag, and
dispute-handoff behavior does not exist at the branch tip; the store
(#1165), sweep (#1166), pass (#1167), and read (#1168) tickets add the code
and the checks. The 12 proposed records carry `Existing check: none;
proposed`.

## Suspiciously quiet areas

- No test runs the scorer on a second thread or process (see the
  determinism record's open question).
- No test combines a repeated decision object id with more than K passing
  summaries across many orders; a property test over random permutations
  is the stronger oracle for the determinism record.
- No test checks that the other summaries' results equal a run without the
  unscorable summary (the oversize record's partial clause).
- No test ties `matcher_digest()` to the scores of a fixed corpus, so a
  behavior change that keeps the preimage's rule ids leaves the digest
  unchanged (portfolio G1).
- The scorer's `TooManyAtoms` path is unreachable by construction and has no
  test; the analyzer's own test covers the refusal.
- No runtime caller exists, so nothing yet checks that the pass records
  `matcher_digest()` with each row (#1165, #1167).
