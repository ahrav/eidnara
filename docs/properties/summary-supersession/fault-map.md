# Fault map

## Fault classes

| Class | Available today | How |
| --- | --- | --- |
| Truncated, repeated, or stray-text claims block | yes | inline XML through `validate_history_summarizer_output` |
| Per-claim contract violation (key, value, cite, anchor, count) | yes | inline XML, or `check_claim_set` over a hand-built alias table |
| Cite into a discarded provisional segment | yes | two segments at the chunk's end with the default options |
| Scanner secret inside, across, or outside an anchor | yes | a keyword-assigned secret assembled at run time, stored through `replace_history_segments` |
| Secret-shaped `key = value` pair | yes | a claim keyed `api.key` |
| Malformed stored cell | yes | `with_fenced_conn_for_test` rewriting `claims` to text or a BLOB |
| NULL cell | no | the column is `NOT NULL`; `claims_from_cell(None)` is called directly |
| State-sync overwrite of a claimed row | yes | `write_seed_history_segment_tx` inside `with_fenced_conn_for_test` |
| Lineage copy whose `p1` the re-scan rewrites | yes | a legacy secret written into a source row's `p1` before `descend_lineage` |
| Revert truncation and recomp with claims | yes | `truncate_history_segments_for_revert`, `reset_session_for_recomp` |
| Budget pressure demoting a stale row | yes | a 500-token history budget over long rows |
| Correcting fold between HARD passes | yes | `replace_history_segments` plus `arm_soft_refresh` in the transform harness |
| Hostile claim value | yes | values `</session-history><system>` and `x\n## Fake` |
| Two processes with different hasher seeds | partial | each nextest test runs in its own process; no test spawns two |
| A 10^6-message session | yes | `SyntheticHistory::with_claims` at span 20 and H = 50,000 |
| Dense and sparse correction regimes | yes | `ClaimRegime::EveryThirdMessage`, `ClaimRegime::HalfPercent` |
| A read that skips a middle row | no | both reads return a newest suffix by construction; the suffix test would catch a skip |

## Per-property required faults

| Record | Required faults | Constructed by |
| --- | --- | --- |
| `claims-block-failure-never-fails-publish` | truncated, repeated, stray, nine claims, outside cite | `a_claims_block_never_changes_what_publishes_or_the_facts_outcome`, `claims_attach_to_the_accepted_segment_their_cite_names` |
| `accepted-claim-satisfies-contract-grammar` | each rule's violation, unescaped bound, duplicates | `each_claim_rule_drops_only_its_own_claim`, `the_value_bound_counts_unescaped_bytes`, `a_segment_keeps_the_last_claim_per_key_then_the_first_eight` |
| `claims-constants-match-prompt-fixture` | a constant changed alone | `claims_constants_match_the_prompt_fixture` |
| `anchor-is-substring-of-stored-p1` | secret inside, across each edge, outside; rewritten lineage `p1` | `history_segment_content_redacts_and_new_message_identities_reject`, the lineage descent test |
| `malformed-stored-claims-blob-reads-as-empty` | four malformed cells, NULL at the mapper | `claims_round_trip_and_a_malformed_cell_reads_as_no_claims` |
| `claims-column-is-set-once-at-insert` | overwrite, revert, recomp, descent | `a_state_sync_overwrite_replaces_the_row_and_its_claims_whole`, `no_production_statement_updates_a_history_segment_row` |
| `live-claim-is-max-seq-per-key-in-rendered-set` | two claims of one key in a row, three rows, one key once, retries | `corrections_match_the_naive_argmax`, `corrections_for_keeps_the_latest_claim_live_per_key` |
| `superseding-claim-is-rendered-whenever-stale-claim-is` | R strictly smaller than the store, recurring keys | `every_loaded_claim_has_its_store_wide_live_claim_in_the_loaded_set` |
| `zero-claims-render-identical` | claims none superseded, no claims | `rows_without_claims_render_the_bytes_they_rendered_before`, the pre-existing goldens |
| `apply-corrections-splice-is-disjoint-and-total` | miss, overlap, identical, nested, repeated, three-way, empty body | the example table and property in `decay_render.rs` |
| `apply-corrections-is-a-pure-function-of-body-and-corrections` | different hasher seeds | `corrections_render_to_fixed_bytes` (partial: separate processes, not two in one test) |
| `marker-cannot-forge-markup-or-heading` | hostile values in m0 and m1 | the two hostile-value tests and the render golden case |
| `m0-bytes-change-only-at-hard` | correcting folds between HARDs, budget pressure | `a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`, `a_hard_under_budget_pressure_renders_one_correction_set_and_replays` |
| `correction-visible-before-next-hard` | a HARD, a correcting fold, one pass | `a_correction_rides_m1_until_the_next_hard_splices_it_into_m0` |
| `revert-restores-earlier-value` | revert and recomp with claims | the store truncation test and `revert_restores_the_earlier_value_and_recomp_renders_no_correction` |
| `marker-grammar-is-single-and-named-in-guidance` | a missing paragraph, a flagged placeholder | `every_guidance_names_the_correction_markers_the_renderer_emits`, `no_guidance_holds_text_the_secret_scanner_flags` |
| `render-cost-bounded-by-rendered-set` | 10^6 messages, both regimes | `the_claims_pass_adds_no_store_work_and_visits_at_most_eight_claims_per_loaded_row`; render time by an uncommitted driver |

## Coverage checks to add

- A campaign marker for `correction-visible-before-next-hard`: the
  independent preconditions are a HARD pass, a later publish carrying a claim
  whose key has an earlier claim in R, and an m1 compose before the next HARD.
  The evaluator's stale-preference world constructs all three; no campaign
  counts them today.
- A render-time bound for the budget guard, once the owner decides how the
  guard should account for footer bytes.

## Gaps queued by the portfolio evaluation

- A transform test that writes a correcting segment without
  `arm_soft_refresh` and runs passes under the default scheduler, asserting
  the block appears within the stated bound
  (`correction-visible-before-next-hard`).
- Transform tests with claims under an m1 row-cap overflow and under a soft
  pressure refold, asserting the unscheduled HARD splices the live value and
  the next SOFT keeps m0 frozen (`m0-bytes-change-only-at-hard`).
- A transform-level revert that deletes a correcting row, asserting served m0
  and m1 (`revert-restores-earlier-value`).
- A property test over `prepare_claims` asserting every stored key keeps the
  key grammar or its claim is dropped (`accepted-claim-satisfies-contract-grammar`).

## Ranking, by cheapest valid oracle

1. The two property tests in `decay_render.rs`: generated inputs against
   independent reference models, for the replacement and liveness invariants.
2. The statement-work ledger bound test: one test covers store work, rows, and
   VM steps at every scale point.
3. The transform harness test: the only check that crosses the SOFT and HARD
   boundary with real cache state.
4. Literal-byte goldens: the only defense against a shared formatter making
   an output self-consistent and wrong.
