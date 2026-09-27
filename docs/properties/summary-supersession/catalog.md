# Summary supersession: catalog

Method: `../METHOD.md`. The records are the 17 proposed obligations of the
summary supersession specification
([#835](https://github.com/ahrav/eidnara/issues/835)), derived by its
invariant-modeling pass at `265df096` and converted here after M1
([#838](https://github.com/ahrav/eidnara/issues/838)), M2
([#839](https://github.com/ahrav/eidnara/issues/839)), and M3
([#840](https://github.com/ahrav/eidnara/issues/840)) made each one
checkable. The cited checks were run on the branch that adds this catalog;
each evidence file names the commit its runs used. References name functions
and tests rather than line numbers.

## Scope

Keyed claims from the History Summarizer through validation, storage, and
serving:

1. The claims contract and validator: `crates/daemon/src/history_summarizer_validate.rs`
   (`parse_claims`) and `crates/daemon/src/history_summarizer_citations.rs`
   (`check_claim_set`, the claim constants).
2. The `claims` column, `Claim`, the per-field scan, and the lenient read:
   `crates/memory-store/src/lib.rs` (`prepare_claims`, `claims_from_cell`,
   `insert_history_segment_tx`).
3. Liveness over the loaded set R: `crates/daemon/src/decay_render.rs`
   (`live_claims`, `corrections_for`, `render_rows`).
4. Replacement: `apply_corrections` and the one marker formatter in the same
   file.
5. m0 at HARD and the m1 delta: `crates/daemon/src/m0_compose.rs`
   (`compose_m0`) and `crates/daemon/src/m1_compose.rs`
   (`render_memory_updates`).
6. Revert and recomp over claims.
7. The guidance assets `crates/daemon/assets/guidance_*.txt` and the marker
   grammar they name.

Existing behavior these surfaces rely on is covered by the durable catalogs
linked under Relationships; it is not re-derived here.

## Reachability classes

- `default-production`: reached by every published History Summarizer chunk,
  every m0 compose on a HARD pass, or every m1 compose, with no configuration.
- `explicit-config-only`: none in this part.
- `test-only`: a property of the test fixtures themselves, reached only by a
  test.

## Index

| Slug | Type | Surface | Record |
| --- | --- | --- | --- |
| `claims-block-failure-never-fails-publish` | safety | 1 | yes |
| `accepted-claim-satisfies-contract-grammar` | safety | 1 | yes |
| `claims-constants-match-prompt-fixture` | safety | 1 | yes |
| `anchor-is-substring-of-stored-p1` | safety | 2 | yes |
| `malformed-stored-claims-blob-reads-as-empty` | safety | 2 | yes |
| `claims-column-is-set-once-at-insert` | safety | 2 | yes |
| `live-claim-is-max-seq-per-key-in-rendered-set` | safety | 3 | yes |
| `superseding-claim-is-rendered-whenever-stale-claim-is` | safety | 3 | yes |
| `zero-claims-render-identical` | safety | 4 | yes |
| `apply-corrections-splice-is-disjoint-and-total` | safety | 4 | yes |
| `apply-corrections-is-a-pure-function-of-body-and-corrections` | safety | 4 | yes |
| `marker-cannot-forge-markup-or-heading` | safety | 4 | yes |
| `m0-bytes-change-only-at-hard` | safety | 5 | yes |
| `correction-visible-before-next-hard` | liveness | 5 | yes |
| `revert-restores-earlier-value` | safety | 6 | yes |
| `marker-grammar-is-single-and-named-in-guidance` | safety | 7 | yes |
| `render-cost-bounded-by-rendered-set` | safety | 3, 5 | yes |

Semantics distribution: `always` 17, `always-or-unreached` 0, `sometimes` 0,
`reachable` 0, `unreachable` 0. `correction-visible-before-next-hard` is
bounded liveness with its bound stated in m1 composes, and
`claims-column-is-set-once-at-insert` is `always(!X)` over the production
source. The campaign marker the specification paired with the liveness record
is queued in `fault-map.md`.

## Records

### claims-block-failure-never-fails-publish

Type: safety
Reachability: default-production - every chunk the History Summarizer validates goes through `validate_history_summarizer_output`, which computes `claims_outcome` after the segments are final
Status: active
Exercised: yes - `a_claims_block_never_changes_what_publishes_or_the_facts_outcome` validates one chunk with no block, a truncated block, a 9-claim block, a repeated block, and stray material; `claims_attach_to_the_accepted_segment_their_cite_names` compares a chunk with an outside cite to the chunk without it; `nonadmission_facts_survive_later_firings_failures_and_reopen` publishes a chunk carrying claims through the store
Guarantee: No claims block, an unreadable block, or any per-claim rejection changes only the segments' `claims` and the chunk's `claims_outcome`, never whether or which segments publish, the publication floor, or the facts outcome.
Check: `always` - the validated chunk with its claims and `claims_outcome` cleared equals the chunk validated without the block; asserted on every validation because the block is model output and can be anything
Fault/timing angle: none
Required faults and enabling state: a truncated `<claims>` block; two blocks; stray text inside a block; nine claims; a cite outside every accepted segment
Confidence: high - [evidence](evidence/claims-block-failure-never-fails-publish.md). Verified that `parse_claims` never returns an error, that `claims_outcome` is computed after the discard-last step and the facts verdict, and that the table's four failing blocks produce chunks equal to the no-block chunk once claims are cleared
Existing check: `crates/daemon/src/history_summarizer_citations_golden.rs::a_claims_block_never_changes_what_publishes_or_the_facts_outcome`; `crates/daemon/src/history_summarizer.rs::tests::nonadmission_facts_survive_later_firings_failures_and_reopen` (firing 2 publishes claims and records no nonadmission)
Impact: A claims fault would withhold valid history, stall the publication floor, and bring back the retry churn the fail-open design avoids
Open questions: None.

### accepted-claim-satisfies-contract-grammar

Type: safety
Reachability: default-production - `check_claim_set` runs on every chunk whose claims block parses
Status: active
Exercised: yes - `each_claim_rule_drops_only_its_own_claim` and `a_segment_keeps_the_last_claim_per_key_then_the_first_eight` drive each rule directly; `claims_attach_to_the_accepted_segment_their_cite_names`, `the_value_bound_counts_unescaped_bytes`, and `the_provisional_last_segment_carries_its_claims` drive them through the validator
Guarantee: Every kept claim has a key matching `[a-z0-9_-]+(\.[a-z0-9_-]+)+` within 64 bytes, a value within 128 bytes after unescaping, exactly one citation that resolves through the frozen alias table to a presented span containing the value, and a cited message inside a persisted segment; each segment keeps the last claim per key and then the first 8, and a claim's index is its position; the store drops a claim whose key the secret scanner rewrites, so no stored key is a redacted placeholder.
Check: `always` - a violating claim is absent from the validated segment, not repaired; the property test asserts cite acceptance equals `presented.get(start..end).is_some_and(|s| s.contains(value))`
Fault/timing angle: none
Required faults and enabling state: an uppercase key; a dotless key; a 129-byte value after unescaping; two citations; an unknown alias; a value outside its cited span; a cite into a discarded provisional segment; duplicate keys; nine claims
Confidence: high - [evidence](evidence/accepted-claim-satisfies-contract-grammar.md). Verified the D-4 order in `check_claim_set` and that the value bound counts unescaped bytes; a mutation run replacing the span check with a whole-message check made the property test fail
Existing check: `crates/daemon/src/history_summarizer_citations.rs::tests::{each_claim_rule_drops_only_its_own_claim, a_segment_keeps_the_last_claim_per_key_then_the_first_eight, cite_acceptance_is_value_inside_the_cited_span}`; `crates/daemon/src/history_summarizer_citations_golden.rs::claims_attach_to_the_accepted_segment_their_cite_names`; `crates/daemon/src/history_summarizer_validate.rs::tests::{the_value_bound_counts_unescaped_bytes, the_provisional_last_segment_carries_its_claims}`; the rewritten-key case in `crates/memory-store/tests/production_redaction.rs::history_segment_content_redacts_and_new_message_identities_reject`
Impact: A claim with a bad key or a value the cited message never stated would correct history with an invented value
Open questions: None.

### claims-constants-match-prompt-fixture

Type: safety
Reachability: test-only - a relation between the prompt text and the validator constants, checked by a test
Status: active
Exercised: yes - `claims_constants_match_the_prompt_fixture` reads the prompt the daemon compiles in
Guarantee: The limits the summarizer prompt states (8 claims, 64-byte key, 128-byte value, 200-byte anchor) equal `CLAIMS_PER_SEGMENT`, `CLAIM_KEY_MAX_BYTES`, `CLAIM_VALUE_MAX_BYTES`, and `CLAIM_ANCHOR_MAX_BYTES`.
Check: `always` - each rule line of the `## Claims` section contains its constant's value; one test, because the fixture and the constants change together or not at all
Fault/timing angle: none
Required faults and enabling state: a constant changed without the fixture, or the reverse
Confidence: high - [evidence](evidence/claims-constants-match-prompt-fixture.md). Verified the test reads `HISTORY_SUMMARIZER_SYSTEM_PROMPT`, the `include_str!` of `crates/daemon/testdata/history_summarizer-system-prompt.txt`, and matches each rule by its line prefix
Existing check: `crates/daemon/src/history_summarizer_citations.rs::tests::claims_constants_match_the_prompt_fixture`
Impact: The model would be told one bound and judged by another, and claims would be dropped for rules it was never given
Open questions: None.

### anchor-is-substring-of-stored-p1

Type: safety
Reachability: default-production - every segment insert goes through `prepare_history_segment`, which calls `prepare_claims`
Status: active
Exercised: yes - `history_segment_content_redacts_and_new_message_identities_reject` stores claims whose anchors sit outside, inside, and across each edge of a scanner secret in `p1`; the lineage descent test copies claims whose `p1` the re-scan changes
Guarantee: For every stored claim with an anchor, the stored trimmed `p1` contains the anchor byte for byte; the key, value, anchor, and `key = value` pair are each scanned as a field of their own, never the JSON cell as one string.
Check: `always` - after insert, every stored anchor is a substring of the stored trimmed `p1`; asserted as containment, not as exact redacted strings, so it survives scanner rule changes
Fault/timing angle: a lineage copy re-scans `p1` in the copying transaction; a changed `p1` drops the copy's claims
Required faults and enabling state: a secret inside the anchor; a secret straddling each anchor edge; a secret only outside the anchor; a legacy `p1` holding a secret the re-scan rewrites during descent
Confidence: high - [evidence](evidence/anchor-is-substring-of-stored-p1.md). Verified the containment oracle over all four anchors, the scan-audit count of one field scan per key, value, anchor, and pair, and the descent test's `[1, 0, 0]` claim counts
Existing check: `crates/memory-store/tests/production_redaction.rs::{history_segment_content_redacts_and_new_message_identities_reject, active_scan_audit_expires_with_its_session_note_owner}`; the lineage descent test in `crates/memory-store/src/lib.rs` that asserts `copied` claim counts; the validator anchor rule in `each_claim_rule_drops_only_its_own_claim`
Impact: A stored anchor outside `p1` would never splice and would footer forever, or a redacted `p1` beside an unredacted anchor would persist a secret
Open questions: None.

### malformed-stored-claims-blob-reads-as-empty

Type: safety
Reachability: default-production - every segment read maps rows through `stored_history_segment_from_row`, which calls `claims_from_cell`
Status: active
Exercised: yes - `claims_round_trip_and_a_malformed_cell_reads_as_no_claims` rewrites a stored cell to `'not json'`, `'{}'`, `'[{"key":1}]'`, and a BLOB, and calls the mapper with no text
Guarantee: A `claims` cell that is not a claim array, or not text, reads as no claims with one diagnostic line and never as a store error; non-empty claims round-trip equal.
Check: `always` - the loaded row equals the claims-free row field for field for every malformed cell, and a written claim vector reads back equal
Fault/timing angle: none
Required faults and enabling state: cells `'not json'`, `'{}'`, `'[{"key":1}]'`, a BLOB, and a NULL reaching the mapper
Confidence: high - [evidence](evidence/malformed-stored-claims-blob-reads-as-empty.md). Verified the mapper reads column 17 with `get_ref(..).as_str().ok()`, so a BLOB or NULL is `None`; the column is `NOT NULL`, so the NULL case is checked at the mapper
Existing check: `crates/memory-store/src/lib.rs::tests::claims_round_trip_and_a_malformed_cell_reads_as_no_claims`
Impact: A bad cell would fail every segment load for the session and so every m0 and m1 compose
Open questions: None.

### claims-column-is-set-once-at-insert

Type: safety
Reachability: default-production - the publish, replace, state-sync, and lineage paths all write `history_segments`
Status: active
Exercised: yes - `no_production_statement_updates_a_history_segment_row` scans the production source; `a_state_sync_overwrite_replaces_the_row_and_its_claims_whole` overwrites a seeded row; the revert test truncates and resets rows carrying claims
Guarantee: A row's `claims` bytes are the bytes written at its insert or the row is gone; no production statement updates a history segment row, and a state-sync overwrite deletes and re-inserts through the one insert.
Check: `always(!X)` - X is a production statement in `crates/memory-store/src/lib.rs`, the only production writer of `history_segments`, that updates or upserts a history segment row; checked by a whitespace-collapsed source scan because no runtime point observes a statement that is absent. The scan matches literal text, and `INSERT OR REPLACE` would delete and re-insert, which keeps the guarantee
Fault/timing angle: none
Required faults and enabling state: a state-sync overwrite of a row carrying claims; revert truncation; recomp reset; lineage descent
Confidence: high - [evidence](evidence/claims-column-is-set-once-at-insert.md). Verified the source scan finds at least the two insert statements and no `DO UPDATE`; a mutation run restoring the former upsert made both the scan and the overwrite test fail
Existing check: `crates/memory-store/src/lib.rs::tests::{no_production_statement_updates_a_history_segment_row, a_state_sync_overwrite_replaces_the_row_and_its_claims_whole, truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch}`
Impact: A claim could outlive the `p1` it anchors to, or a heal or overwrite could rewrite history the renderer has already corrected against
Open questions: None.

### live-claim-is-max-seq-per-key-in-rendered-set

Type: safety
Reachability: default-production - `render_rows` calls `corrections_for` on every m0 and m1 compose
Status: active
Exercised: yes - `corrections_match_the_naive_argmax` checks generated segment sets against a naive per-key argmax; `corrections_for_keeps_the_latest_claim_live_per_key` names the faults
Guarantee: Over R, a key's live claim is its claim with the greatest `(sequence, idx)`; a row's corrections are exactly its non-live claims, each carrying the live claim's value and ordinal; the result depends on R alone, not on tier, pressure, or importance.
Check: `always` - `corrections_for` equals the naive argmax, and every claim is either its key's live claim or exactly one correction
Fault/timing angle: the pressure-retry loop re-renders up to four times; corrections are computed once in `render_rows` before it
Required faults and enabling state: two claims of one key in one row; three rows sharing a key; a key present once; a budget that forces retries
Confidence: high - [evidence](evidence/live-claim-is-max-seq-per-key-in-rendered-set.md). Verified `live_claims` compares `(sequence, idx)` explicitly, that `render_m0_with_decay_pressure_retry` receives rows whose corrections are already attached, and that m1's block uses the same `live_claims`
Existing check: `crates/daemon/src/decay_render.rs::tests::{correction_properties::corrections_match_the_naive_argmax, corrections_for_keeps_the_latest_claim_live_per_key}`; `crates/daemon/src/m0_compose.rs::correction_compose_tests::a_hard_under_budget_pressure_renders_one_correction_set_and_replays`
Impact: A superseded value could render as current, or a current value could be marked superseded
Open questions: None.

### superseding-claim-is-rendered-whenever-stale-claim-is

Type: safety
Reachability: default-production - the m0 fold read and the m1 read above the folded sequence are the only segment loads a compose makes
Status: active
Exercised: yes - `every_loaded_claim_has_its_store_wide_live_claim_in_the_loaded_set` compares R's argmax with the store-wide argmax over a 4,000-row session with recurring keys, with a negative control that removes R's newest claimed row; `a_legacy_row_stores_no_claims` enforces the legacy premise at the store
Guarantee: For every claim in R, its key's live claim over R equals its key's live claim over the whole store, so a stale claim never renders as live because its corrector fell outside the read.
Check: `always` - for the m0 fold at budgets 20, 60,000, and 10,000,000 and for m1's rows above three folded sequences, each loaded claim's argmax over R equals the store-wide argmax; each R is a strict subset of the store
Fault/timing angle: none
Required faults and enabling state: R strictly smaller than the store; keys that recur across the boundary of R; legacy rows inside and past the pressure window
Confidence: high - [evidence](evidence/superseding-claim-is-rendered-whenever-stale-claim-is.md). Verified that both reads return the newest non-legacy rows plus every legacy row, that the store keeps no claims on a legacy row, so any claimed row newer than a loaded row is loaded, and that the gapped set fails the check
Existing check: `crates/daemon/src/m0_compose.rs::bounded_read_tests::every_loaded_claim_has_its_store_wide_live_claim_in_the_loaded_set`; `crates/memory-store/src/lib.rs::tests::a_legacy_row_stores_no_claims`
Impact: A read that skipped a middle row would serve a superseded value as current
Open questions: None.

### zero-claims-render-identical

Type: safety
Reachability: default-production - every compose over rows without superseded claims
Status: active
Exercised: yes - `rows_without_claims_render_the_bytes_they_rendered_before` composes m0 and m1 with no claims and with claims that nothing supersedes; the pre-existing render, tight-render, transform, and differential goldens run unchanged
Guarantee: When no loaded claim is superseded, m0 and m1 history bytes equal the renderer's bytes before corrections existed, and `apply_corrections` returns its input borrowed.
Check: `always` - golden equality over every pre-existing golden, byte equality between the claims and no-claims stores, and pointer equality for the empty case
Fault/timing angle: none
Required faults and enabling state: claims present but none superseded; rows with no claims
Confidence: high - [evidence](evidence/zero-claims-render-identical.md). Verified that `render-golden.json` only gained cases, that no other render or transform golden changed, and that the m1 block is empty when the new rows carry no claims (decision A7 narrows the m1 clause to that condition)
Existing check: `crates/daemon/src/m0_compose.rs::correction_compose_tests::rows_without_claims_render_the_bytes_they_rendered_before`; `crates/daemon/src/decay_render.rs::tests::{render_golden_matches_reference, render_tight_golden_matches_reference_with_real_estimator, apply_corrections_splices_disjoint_first_hits_and_footers_the_rest}`
Impact: Every session without corrections would change its served bytes and lose its prompt cache on upgrade
Open questions: None.

### apply-corrections-splice-is-disjoint-and-total

Type: safety
Reachability: default-production - the tiered branch of `render_one_history_segment` calls `apply_corrections` for every rendered row
Status: active
Exercised: yes - `apply_corrections_splices_disjoint_first_hits_and_footers_the_rest` names each fault; `apply_corrections_is_disjoint_and_total` checks generated bodies and anchors
Guarantee: Every correction appears exactly once, as a spliced marker or a footer entry; spliced hits are pairwise disjoint first occurrences; a hit splices only when every hit it overlaps overlaps more hits; bytes outside spliced spans keep their order; footer entries keep `idx` order; the footer renders for an empty body.
Check: `always` - markers plus footer entries equal the correction count, removing markers and restoring anchors reproduces the body, footer indices increase, and a found anchor that overlaps no other found anchor splices
Fault/timing angle: none
Required faults and enabling state: an absent anchor; a missing anchor; an overlapping pair; identical anchors; nested anchors; an anchor occurring twice; a three-way chain; footer entries whose key and ordinal order differ from `idx` order; an empty body
Confidence: high - [evidence](evidence/apply-corrections-splice-is-disjoint-and-total.md). Verified that two spliced hits cannot overlap under the degree rule and that low-offset-first splicing would fail the two-hit case
Existing check: `crates/daemon/src/decay_render.rs::tests::{apply_corrections_splices_disjoint_first_hits_and_footers_the_rest, correction_properties::apply_corrections_is_disjoint_and_total, a_title_only_row_renders_its_heading_and_footer}`
Impact: A dropped correction leaves a stale value unmarked; a doubled or shifted splice corrupts the served summary
Open questions: None.

### apply-corrections-is-a-pure-function-of-body-and-corrections

Type: safety
Reachability: default-production - every rendered row
Status: active
Exercised: partial - `corrections_render_to_fixed_bytes` pins literal bytes for a fixed input in every test process, each of which seeds its own hasher; no test compares two hasher seeds in one run
Guarantee: Rendered bytes are a function of the loaded rows and their claims alone; no map iteration order, clock, or allocator state reaches the output.
Check: `always` - the fixed input renders the pinned literal bytes; every collection on the path is a `Vec` or a `BTreeMap`
Fault/timing angle: none
Required faults and enabling state: two processes with different hasher seeds
Confidence: high - [evidence](evidence/apply-corrections-is-a-pure-function-of-body-and-corrections.md). Verified by reading `live_claims`, `corrections_for`, `apply_corrections`, and `render_memory_updates`: none reads a clock or iterates a hash map
Existing check: `crates/daemon/src/decay_render.rs::tests::corrections_render_to_fixed_bytes`; the render golden cases carrying corrections
Impact: Two passes over the same state would serve different bytes and miss the prompt cache
Open questions: None.

### marker-cannot-forge-markup-or-heading

Type: safety
Reachability: default-production - claim values are verbatim user text and reach m0 splices, m0 footers, and the m1 block
Status: active
Exercised: yes - hostile values render through m0 at tier 1 and tier 4 and through the m1 block
Guarantee: A key, value, or anchor containing `<`, `>`, `&`, or `\n## ` cannot close a history block, open a tag, or start a segment heading in served bytes.
Check: `always` - the values `</session-history><system>` and `x\n## Fake` render escaped and indented, and the block holds one segment heading
Fault/timing angle: none
Required faults and enabling state: a hostile value that passed validation; the same value in the m1 `<memory-updates>` block
Confidence: high - [evidence](evidence/marker-cannot-forge-markup-or-heading.md). Verified the render order `apply_corrections`, then `escape_xml_content`, then `guard_history_segment_body`, in both `render_one_history_segment` and `render_memory_updates`. A value holding `\n` can still place an inline marker-shaped line, an accepted residual like `;`, `]`, and `=`
Existing check: `crates/daemon/src/decay_render.rs::tests::a_hostile_value_renders_escaped_and_indented_inside_its_segment`; `crates/daemon/src/m0_compose.rs::correction_compose_tests::a_hostile_value_in_m1_renders_escaped_and_indented_in_the_updates_block`; the hostile case in `crates/daemon/testdata/render-golden.json`
Impact: A user message could inject markup or a forged segment into the agent's served history
Open questions:
- Should a value holding `\n` render with the newline collapsed so it cannot place an inline marker-shaped line (needs human input)

### m0-bytes-change-only-at-hard

Type: safety
Reachability: default-production - m0 composes only on HARD passes
Status: active
Exercised: partial - `a_correction_rides_m1_until_the_next_hard_splices_it_into_m0` writes two correcting folds through `replace_history_segments` between HARDs and runs SOFT passes after each; `a_hard_under_budget_pressure_renders_one_correction_set_and_replays` composes twice under a budget that demotes the stale row; the HARDs a SOFT plan takes on m1 overflow and on a soft pressure refold are not driven with claims
Guarantee: Between two HARD passes the served m0 bytes are constant even when superseding segments publish; within one HARD every retry renders the same corrections, and the final bytes replay from the same state.
Check: `always` - m0 bytes equal across SOFT passes after each correcting publish, and two composes of one state under budget pressure are byte-equal
Fault/timing angle: an anchor that hits at tier 1 and misses at tier 2 turns a splice into a footer across the retry loop, and the final bytes must still replay; a SOFT plan recomposes m0 as a HARD on m1 overflow past the row cap and on a soft pressure refold
Required faults and enabling state: a correcting publish after a HARD; a second fold before the next HARD; a budget small enough to demote the stale row
Confidence: high - [evidence](evidence/m0-bytes-change-only-at-hard.md). Verified that the transform harness serves the frozen m0 on SOFT, that the next HARD splices the newest value, and that corrections are attached before `render_m0_with_decay_pressure_retry`
Existing check: `crates/daemon/src/transform.rs::tests::a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`; `crates/daemon/src/m0_compose.rs::correction_compose_tests::a_hard_under_budget_pressure_renders_one_correction_set_and_replays`
Impact: A correction would bust the prompt cache on every publish instead of at the next HARD
Open questions: None.

### correction-visible-before-next-hard

Type: liveness
Reachability: default-production - `compose_m1` fills the `memory_updates` slot on every m1 compose
Status: active
Exercised: partial - `a_correction_rides_m1_until_the_next_hard_splices_it_into_m0` asserts the block after each correcting fold, but it arms the soft refresh by hand with `arm_soft_refresh`, whose only production caller is the operator `session.flush` request, so the digest-driven recompose after a publish and a scheduler `Defer` are not constructed; `m1_names_every_claim_on_its_rows_with_the_live_value` asserts the block's content
Guarantee: After a segment carrying a claim publishes, the next m1 composition carries `<memory-updates>` with the precedence sentence and an entry naming the key, the live value or a retraction, and the ordinal; the bound is one m1 recompose, which the m1 revision digest forces on any new segment.
Check: `always` - within the first non-`Defer` pass after the publish, bounded by one m1 compose, the block names every claim on the rows above the folded sequence in `(sequence, idx)` order with its key's live value; not an unbounded "eventually". The deterministic construction replaces the specification's `sometimes` campaign marker for the content; the marker itself is queued in `fault-map.md`
Fault/timing angle: the window between the publish and the next HARD, during which m0 still serves the stale value
Required faults and enabling state: a HARD; a fold producing a superseding claim; one SOFT pass
Confidence: high - [evidence](evidence/correction-visible-before-next-hard.md). Verified the block lists every claim on the new rows per decision A7, including a claim that supersedes nothing and duplicate entries for one key
Existing check: `crates/daemon/src/transform.rs::tests::a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`; `crates/daemon/src/m0_compose.rs::correction_compose_tests::m1_names_every_claim_on_its_rows_with_the_live_value`
Impact: The agent would act on the stale m0 value until the next HARD
Open questions:
- What bounds the number of consecutive `Defer` passes while an m1 delta is pending (needs human input)
- Q3 of the specification: at the 8-claim cap the burst's cumulative block tokens exceed one m0 re-freeze by 3.6% to 10% in the last folds before the natural HARD, so the criterion alone selects a forced HARD there; at 3 claims per fold it does not. M3 proposes no forced HARD, a deviation from the literal criterion (needs human input)

### revert-restores-earlier-value

Type: safety
Reachability: default-production - revert truncation and recomp reset run on the transform's revert paths
Status: active
Exercised: partial - the store test truncates s1 (`k = a`) and s2 (`k = b`) to s1 and resets for recomp, and the compose test renders before and after both; no test drives a transform-level revert that deletes a correcting row
Guarantee: After revert truncation, liveness over the remaining rows makes a claim superseded only by deleted rows live again, so it renders without a marker; after recomp no claims exist and rendering is identity; no correction references a deleted row.
Check: `always` - s1 renders with a marker before the truncation and without one after; after recomp, m0 equals the empty store's m0
Fault/timing angle: none; corrections are recomputed per compose, so no cached correction survives a revert epoch
Required faults and enabling state: revert with claims on both sides of the kept sequence; recomp with claims
Confidence: high - [evidence](evidence/revert-restores-earlier-value.md). Verified that the store test finds only s1's claim for the key after truncation and no rows after recomp, and that the compose test renders s1's original `p1`
Existing check: `crates/memory-store/src/lib.rs::tests::truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`; `crates/daemon/src/m0_compose.rs::correction_compose_tests::revert_restores_the_earlier_value_and_recomp_renders_no_correction`
Impact: A revert would leave history corrected by a message the user took back
Open questions: None.

### marker-grammar-is-single-and-named-in-guidance

Type: safety
Reachability: default-production - every agent request carries one guidance asset, and every correction renders through the one formatter
Status: active
Exercised: yes - `every_guidance_names_the_correction_markers_the_renderer_emits` checks all four assets against shapes the formatter produces; the plugin A1 golden test checks the re-exported asset bytes
Guarantee: m0 splices, m0 footers, and m1 entries come from `correction_marker` with two forms; every guidance asset names the spliced, retraction, and footer shapes with the precedence sentence and lists the three shapes on its never-reproduce line; no asset holds text the secret scanner flags.
Check: `always` - each asset's paragraph contains the formatter's shapes with the placeholder `name`, its never-reproduce line contains the three shapes, and scanning the asset finds no detection
Fault/timing angle: none
Required faults and enabling state: an asset missing the paragraph; a formatter change without an asset change; a placeholder the scanner flags (`key = value` is one)
Confidence: high - [evidence](evidence/marker-grammar-is-single-and-named-in-guidance.md). Verified that goldens hold literal marker bytes, that the guidance shapes derive from the formatter, and that the precedence sentence equals the M0-measured one
Existing check: `crates/daemon/src/prompt_surface.rs::tests::{every_guidance_names_the_correction_markers_the_renderer_emits, no_guidance_holds_text_the_secret_scanner_flags}`; `packages/opencode-plugin/src/plugin/tool-registry.test.ts` (A1 golden); `crates/daemon/tests/eval_stale_render.rs::the_served_precedence_sentence_is_the_measured_one`
Impact: The agent would meet markers its guidance never explained, or evaluator cassettes would refuse every request
Open questions: None.

### render-cost-bounded-by-rendered-set

Type: safety
Reachability: default-production - every m0 and m1 compose
Status: active
Exercised: partial - the committed bound test covers store work and measured claim visits up to H = 50,000 and N = 10^6 under both correction regimes; the per-segment marker, byte, and m1-entry clauses hold by construction and are not measured; render time was measured with an uncommitted driver
Guarantee: Per compose, each liveness scan visits each claim of R once, at most 8 x |R|, and the pass adds no store statement, row, or VM step beyond the segment load; at most 8 x |R| substring searches; at most 8 markers or footer entries and about 1.9 KB added per rendered segment before the guard; at most 8 x |R| entries in the m1 block; nothing grows with H or N beyond the claims stored on rows in R.
Check: `always` - store statements, rows, and VM steps per compose are equal with and without claims, an m0 compose's `CLAIMS_VISITED` count equals the claims held by R and is at most 8 x |R|, and an m1 compose visits twice the claims on its rows (corrections, then the block); timing is not a committed check
Fault/timing angle: footers raise rendered bytes above the curve's target, so the budget guard demotes rows one tier per iteration and re-estimates the whole body each time
Required faults and enabling state: a session of 10^6 messages; one correction per three messages; one per two hundred; no claims
Confidence: medium - [evidence](evidence/render-cost-bounded-by-rendered-set.md). The structural bound is verified by the committed test. Render time beyond the load is flat in H on the fixed-span sweep (+9.5% to +9.7% from H = 2,500 to 50,000 over three runs, +0.3% from 10,000 to 50,000). Under one correction per three messages a HARD compose spends 0.66 to 0.96 s beyond the load, against about 3 ms without claims, from the budget guard's full re-estimate per demotion, which is quadratic in |R|
Existing check: `crates/daemon/src/m0_compose.rs::bounded_read_tests::the_claims_pass_adds_no_store_work_and_visits_at_most_eight_claims_per_loaded_row`
Impact: A HARD pass under dense corrections spends up to about 1 s rendering, a regression the claims pass itself does not bound and that needs a follow-up
Open questions:
- Should the budget guard account for footer bytes or estimate incrementally, so dense corrections do not make the guard quadratic in R (needs human input)

## Relationships

These durable records hold for the surfaces above and are linked, not
re-derived:

- [memory-store](../memory-store/catalog.md): `failed-fenced-transaction-leaves-no-partial-state`,
  `write-predicates-are-re-evaluated-inside-the-write-transaction`,
  `recorded-schema-version-cannot-disagree-with-the-actual-schema`,
  `durable-identity-decision-is-made-inside-the-write-transaction`,
  `preserved-identity-name-does-not-exempt-its-value`,
  `core-decay-newest-history_segment-tier-floor`,
  `core-decay-tier-ladder-monotone-and-archive-agreement`,
  `core-decay-budget-pressure-range-totality`,
  `core-decay-archive-termination-bound`.
- [daemon/history_summarizer](../daemon/history_summarizer/catalog.md):
  `publish-transaction-is-the-single-commit-point`,
  `crash-before-publish-commit-refires-without-partial-state`,
  `publish-fence-rejects-selected-content-drift`,
  `hv-publish-accepts-unvalidated-validated-chunk`,
  `hv-tierless-stored-row-arm-must-stay-unreachable`,
  `hv-heal-extends-range-without-revalidating-content`,
  `hv-control-characters-reach-durable-rows`,
  `hv-unescape-xml-double-decodes-entities`,
  `hv-side-channel-anchor-out-of-range-drops-silently`.
- [daemon/transform](../daemon/transform/catalog.md):
  `revert-truncate-commits-outside-the-terminal-cas`,
  `revert-epoch-bumps-at-most-once-per-logical-recut`,
  `output-cache-replace-trails-the-accepted-commit`,
  `canonical-read-staleness-is-distinguishable-from-emptiness`.
- [daemon/rendering](../daemon/rendering/catalog.md):
  `render-a-render-is-deterministic-over-fixed-inputs`,
  `render-a-composition-order-is-fixed-and-each-unit-appears-once`.
- [shared-primitives](../shared-primitives/catalog.md): `hard-bust-drains-deferred-work`,
  `cache-stability-golden-vectors-are-byte-stable`.

Within this part:

- `live-claim-is-max-seq-per-key-in-rendered-set` and
  `superseding-claim-is-rendered-whenever-stale-claim-is` together give the
  store-wide meaning of a rendered correction.
- `anchor-is-substring-of-stored-p1` is the precondition under which
  `apply-corrections-splice-is-disjoint-and-total` splices rather than
  footers at tier 1.
- `claims-column-is-set-once-at-insert` and `revert-restores-earlier-value`
  make deletion the only way a claim leaves the loaded set.
- `m0-bytes-change-only-at-hard` and `correction-visible-before-next-hard`
  split one correction's visibility between the frozen m0 and the m1 delta.
