# Delivery existing-check inventory

Scope: compression-fidelity delivery in `ahrav/eidnara`, inspected 2026-09-19
at `99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
External evidence is the supplied local plan and repository history described
in [catalog.md](catalog.md#scope-and-provenance). No incident logs or related
repositories were supplied; this does not describe a restriction on external
evidence. No checks were executed.

Every listed check has status **unaudited**. A source read confirms what the
check asserts, not whether it is sufficient or runs in required CI. The scope
is claim-bearing checks on this part's delivery mechanisms, not every test in
the large transform module. Citation/publication identity has a separate owner;
only its prerequisite checks are linked here.

## Production checks and bounded transformations

| Location | Semantics and diagnostic | Status |
| --- | --- | --- |
| [history_summarizer_validate.rs:588-592](../../../../crates/daemon/src/history_summarizer_validate.rs#L588-L592) | Rejects output without a usable segment: `HistorySummarizer returned no usable history_segments.` | unaudited |
| [history_summarizer_validate.rs:1136-1144](../../../../crates/daemon/src/history_summarizer_validate.rs#L1136-L1144) | Rejects a segment without nonempty P1. The diagnostic asks for all four tiers even though P1-only output has deliberate fallback. | unaudited |
| [history_summarizer_validate.rs:645-679](../../../../crates/daemon/src/history_summarizer_validate.rs#L645-L679) | Discards a provisional final segment under the stated boundary conditions; rejects no forward progress. These are coverage rules, not semantic checks. | unaudited |
| [history_summarizer_validate.rs:689-707,771](../../../../crates/daemon/src/history_summarizer_validate.rs#L689-L771) | A bad fact set returns `ExtractionOutcome::Rejected` with no facts while valid history remains publishable; a debug assertion bounds accepted segment count by emitted count. | unaudited |
| [decay_render.rs:701-841](../../../../crates/daemon/src/decay_render.rs#L701-L841) | Positive-budget guard demotes the first nonarchived row until the unwrapped body fits or is empty; no importance exemption. Under a counter that counts joins exactly, the row-counted prediction is the body's count. | unaudited |
| [m0_compose.rs:134-173](../../../../crates/daemon/src/m0_compose.rs#L134-L173) | Measures the wrapped session-history slice and retries at most three extra times above 105% of requested budget. It returns the last render even above that trigger. This is no body-budget allowance or guaranteed wrapper/whole-invocation cap. Under a counter that counts joins exactly, [memory_render.rs:184-229](../../../../crates/daemon/src/memory_render.rs#L184-L229) supplies the slice count from the body count when the block is the rendered history. | unaudited |
| [canonical_memory.rs:147-174,195-211](../../../../crates/daemon/src/canonical_memory.rs#L147-L211) | Store/freshness failures return `Withheld`; visible positive decisions are budget-trimmed. | unaudited |
| [transform.rs:8609-8620](../../../../crates/daemon/src/transform.rs#L8609-L8620) | After compression and formatting, drops lines of at most two bytes, meaning only `- ` remains. If every fragment drops, returns no hint. This is not a filter on all snippets of two characters or fewer. | unaudited |
| [transform.rs:8633](../../../../crates/daemon/src/transform.rs#L8633) | `debug_assert!(utf16_len(&wrapped) <= USER_HINT_TOTAL_CHAR_CAP)` checks size, not meaning. | unaudited |
| [transform-session-client.ts:398-425](../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L398-L425) | Parse/apply rejection throws `rust transform recipe rejected`; output-slot exhaustion throws `CaptureBudgetExceeded`. | unaudited |
| [opencode-transform-adapter.ts:205-231](../../../../packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts#L205-L231) | A nonempty boundary requires a synthetic user m0 scoped to the session. Diagnostic names the boundary and actual head. | unaudited |
| [transform-session-client.ts:1540-1631](../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L1540-L1631) | Source/container and heuristic invocation checks precede publication; failure logs raw serving and `finishPass(false)`. | unaudited |
| [transform-session-client.ts:72,1552-1556](../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L72) with the [adapter profile](../../../../packages/opencode-plugin/src/hooks/context/opencode-transform-adapter.ts#L351) and [application call](../../../../packages/opencode-plugin/src/hooks/context/transform-session-client.ts#L1552-L1556) | Uses `opencode-heuristic` with 250-permille headroom in final-array validation. | unaudited |
| [invocation-budget.ts:44-71](../../../../packages/opencode-plugin/src/hooks/context/invocation-budget.ts#L44-L71) | Sums entry bytes, estimates once, and rounds headroom charge up. With a known limit, only over-limit growth declines; `limit_unknown` and `shrinks` can accept. | unaudited |

No production entailment check, omission-disposition check, or semantic
recovery-promise guard was found in these paths. That absence does not authorize
adding a production evaluator.

The unwrapped-body budget, wrapped-history retry trigger, and final-array policy
have distinct verified scopes. The empty history wrapper is deliberate cache
state, not a body-budget violation. None grants a global 105% allowance or an
absolute whole-context cap. Measure surfaces independently without assuming
additive estimates or combining Rust and TypeScript counts. Static scope is
resolved; full-invocation and raw-fallback execution evidence remains missing.

## Parser and renderer checks

| Location and check | What its assertions cover | Status |
| --- | --- | --- |
| [history_summarizer_validate.rs:1656-1677](../../../../crates/daemon/src/history_summarizer_validate.rs#L1656-L1677), `tierless_history_segments_reject_while_p1_only_output_keeps_soft_fallbacks` | Flat producer XML rejects; P1-only becomes P1/P2/P3 `full summary` and empty P4. | unaudited |
| [history_summarizer_validate.rs:1681-1712](../../../../crates/daemon/src/history_summarizer_validate.rs#L1681-L1712), `mismatched_tier_close_parses_leniently_while_tierless_output_still_rejects` | Mismatched close yields bounded tier bodies; flat output still rejects. | unaudited |
| [history_summarizer_validate.rs:1716-1730](../../../../crates/daemon/src/history_summarizer_validate.rs#L1716-L1730), `lenient_tier_extraction_bounds_bodies_and_guards_overcapture` | A subsequent opener bounds a missing or mismatched tier close. | unaudited |
| [decay_render.rs:950-955](../../../../crates/daemon/src/decay_render.rs#L950-L955), `archived_tier_is_omitted` | Explicit tier 5 renders empty. | unaudited |
| [decay_render.rs:958-962](../../../../crates/daemon/src/decay_render.rs#L958-L962), `empty_tier_body_renders_title_only_heading` | Empty P4 renders `## 3-4 · Title`. | unaudited |
| [decay_render.rs:965-1017](../../../../crates/daemon/src/decay_render.rs#L965-L1017), title and date/body-heading tests | XML escaping, one heading line, date formatting, and guarded nested headings. No semantic title oracle. | unaudited |
| [decay_render.rs:1020-1037](../../../../crates/daemon/src/decay_render.rs#L1020-L1037), `budget_guard_demotes_oldest_first` | Output fits 80 characters and newest survives; all three rows use importance 50. | unaudited |
| [decay_render.rs:1108-1134](../../../../crates/daemon/src/decay_render.rs#L1108-L1134), `row_counted_guard_renders_the_whole_body_guard_bytes` | Under the exact tokenizer the row-counted guard renders the bytes of a whole-body guard over 40 and 400 mixed rows at eight budgets, including rows whose headings end in whitespace. | unaudited |
| [decay_render.rs:1156-1197](../../../../crates/daemon/src/decay_render.rs#L1156-L1197), `an_exact_counter_renders_the_whole_body_guard_bytes_from_row_counts` | A counter that counts joins exactly renders the whole-body guard's bytes over 40 and 400 mixed rows at eight budgets and returns the body's exact count; a release build counts no text longer than one row. | unaudited |
| [decay_render.rs:1199-1233](../../../../crates/daemon/src/decay_render.rs#L1199-L1233), `an_under_predicting_estimator_hands_the_rest_to_the_whole_body_guard` | A `len / 4` estimator stops the row-counted demotions early on at least one budget, and the whole-body guard finishes them on the whole-body guard's bytes. | unaudited |
| [decay_render.rs:1235-1272](../../../../crates/daemon/src/decay_render.rs#L1235-L1272), `row_counts_predict_the_exact_body_count` | The per-row prediction equals the exact count of the joined body after demotion leaves whitespace-ended headings. | unaudited |
| [decay_render.rs:1274-1298](../../../../crates/daemon/src/decay_render.rs#L1274-L1298), `guard_demotions_count_changed_rows_and_the_body_once` | Over 200 demotions of 2,000 rows count the whole body once. | unaudited |
| [decay_render.rs:1301-1312](../../../../crates/daemon/src/decay_render.rs#L1301-L1312), `guarded_body_matches_escaping_then_indenting_headings` | One-pass escaping and heading guard equal escaping, then indenting heading-like lines, on generated bodies. | unaudited |
| [decay_render.rs:1314-1324](../../../../crates/daemon/src/decay_render.rs#L1314-L1324), `pressure_from_the_newest_window_matches_pressure_from_every_row` | Pressure from the newest `PRESSURE_WINDOW` importances equals pressure from every importance, bit for bit, on generated importances and budgets. | unaudited |
| [decay_render.rs:1326-1379](../../../../crates/daemon/src/decay_render.rs#L1326-L1379), `heading_matches_formatting_the_sanitized_title_and_date_range` | The one-pass heading equals formatting the collapsed, escaped title and the date range, on generated titles, dates, and message ranges. | unaudited |
| [decay_render.rs:1382-1436](../../../../crates/daemon/src/decay_render.rs#L1382-L1436), `owned_rows_project_as_borrowed_rows` | Owned rows project as borrowed rows, except that only tiered rows carry corrections, and both render the same bytes at every tier. | unaudited |
| [decay_render.rs:1439-1480](../../../../crates/daemon/src/decay_render.rs#L1439-L1480), `stored_history_segment_projects_and_renders` | Stored-row projection, P1 rendering, empty-P1 flat content, and fallback to flat content. | unaudited |
| [decay_render.rs:1558-1596](../../../../crates/daemon/src/decay_render.rs#L1558-L1596), `render_golden_matches_reference` | Nonempty golden corpus and exact rendered bodies, with per-case mismatch diagnostics. | unaudited |
| [decay_render.rs:1599-1722](../../../../crates/daemon/src/decay_render.rs#L1599-L1722), `redacted_store_shape_matches_ts_at_real_history_budgets` | Fixture shape, token costs, budget fit, body hashes, and tier counts match the retained differential table. | unaudited |
| [decay_render.rs:1725-1770](../../../../crates/daemon/src/decay_render.rs#L1725-L1770), `render_tight_golden_matches_reference_with_real_estimator` | Exact body goldens and every tight case fitting its budget or reaching the floor. This is not hand-reviewed meaning. | unaudited |
| [memory_render.rs:319-362](../../../../crates/daemon/src/memory_render.rs#L319-L362), `an_exact_counter_counts_the_history_block_from_its_body` | Under an exact counter the session-history block's count equals the exact count of the extracted block across budgets and whitespace-ended headings, and the bytes equal `render_m0`'s; an earlier opening tag or a closing tag inside the body falls back to counting the block. | unaudited |
| [token_cache.rs:415-460](../../../../crates/daemon/src/token_cache.rs#L415-L460), `paragraph_joins_count_exactly_from_their_rows` | A paragraph-joined body of counted rows counts exactly as one cache hit with no tokenized bytes; the wrapped block and blank-line runs count exactly. | unaudited |
| [token_cache.rs:463-496](../../../../crates/daemon/src/token_cache.rs#L463-L496), `a_repeated_short_paragraph_content_hits_without_tokenizing` | A second count of paragraph-cut content whose spans are all short, or alternate keyed and short spans, is one hit with no tokenized bytes. | unaudited |
| [token_cache.rs:499-514](../../../../crates/daemon/src/token_cache.rs#L499-L514), `a_paragraph_count_that_tokenizes_records_a_miss` | A first paragraph count of short spans records one miss and no hit. | unaudited |
| [token_cache.rs:517-546](../../../../crates/daemon/src/token_cache.rs#L517-L546), `a_paragraph_count_admits_a_bounded_number_of_span_entries` | Content with more spans than the paragraph bound counts exactly and admits at most that many span entries; a run of tiny paragraphs counts exactly. | unaudited |
| [crates/tokenizer/src/parity_tests.rs:107-131](../../../../crates/tokenizer/src/parity_tests.rs#L107-L131), `paragraph_spans_count_as_their_text` | Paragraph-span counts plus one newline per join equal the tokenizer's count, and the reference's count wherever every piece merges whole, on generated texts and join runs. | unaudited |

No corpus-specific high-importance omission or authored/parsed/effective-tier
provenance assertion was found. Existing generic tests should be reused.

## Publication, composition, and warm replay

| Location and check | What its assertions cover | Status |
| --- | --- | --- |
| [history_summarizer.rs:4154-4205](../../../../crates/daemon/src/history_summarizer.rs#L4154-L4205), `wired_history_summarizer_happy_path_sends_validates_and_publishes` | One scripted producer start, Idle completion, publication floor, appended segment identity, and stored P1. | unaudited |
| [history_summarizer.rs:4208-4258](../../../../crates/daemon/src/history_summarizer.rs#L4208-L4258), `selected_range_identity_drift_during_await_rejects_without_cooldown` | Await-time identity mutation rejects publication, leaves prior rows, and does not advance the floor. | unaudited |
| [transform.rs:21111-21216](../../../../crates/daemon/src/transform.rs#L21111-L21216), `new_history_segment_extends_coverage_on_soft_advancing_the_anchor` | Seeded publication drives m1, advances boundary, removes covered raw tail, and replays m0/m1 identically on SOFT+. It does not run the producer. | unaudited |
| [crates/memory-store/src/lib.rs:24626-24673](../../../../crates/memory-store/src/lib.rs#L24626-L24673), `a_planned_fold_reads_text_only_for_the_rows_its_plan_keeps` | A planned fold returns the rows of the full fold; a row its plan rejects keeps only its sequence, importance, and claims, and an archive-typed row with content reads as non-archive. | unaudited |
| [transform.rs:12952-12991](../../../../crates/daemon/src/transform.rs#L12952-L12991), `canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot` | Positive content is rendered; negative category is omitted; snapshot metadata persists. | unaudited |
| [transform.rs:12995-13055](../../../../crates/daemon/src/transform.rs#L12995-L13055), `withheld_canonical_read_composes_no_block_and_differs_from_empty_memory` | Stale, abstained, and unavailable states carry withheld metadata rather than available-empty metadata. | unaudited |
| [transform.rs:13059-13092](../../../../crates/daemon/src/transform.rs#L13059-L13092), `project_memory_revision_change_requests_a_hard_fold` | Same rendered set avoids HARD; changed or withheld sets fold and change visible memory. | unaudited |

## Memory inclusion and exclusion

| Location and check | What its assertions cover | Status |
| --- | --- | --- |
| [canonical_memory.rs:282-311](../../../../crates/daemon/src/canonical_memory.rs#L282-L311), `injectable_rows_are_visible_decisions_in_a_positive_category` | Drops negative category, labeled row, and nondecision row. | unaudited |
| [canonical_memory.rs:315-408](../../../../crates/daemon/src/canonical_memory.rs#L315-L408), revision tests | Digest follows rendered rows, not read timestamp, dropped rows, or edits past the render cap. | unaudited |
| [canonical_memory.rs:412-454](../../../../crates/daemon/src/canonical_memory.rs#L412-L454), `rows_past_the_memory_budget_are_neither_injected_nor_digested` | Oversized row is absent and cannot move the revision. | unaudited |
| [canonical_memory.rs:457-474](../../../../crates/daemon/src/canonical_memory.rs#L457-L474), `withheld_reads_record_the_verdict_and_inject_nothing` | Withheld and available-empty reads differ despite both having no rows. | unaudited |
| [memory_render.rs:365-388,416-443](../../../../crates/daemon/src/memory_render.rs#L365-L443) | Nonpositive categories drop; UTF-8 truncation respects the 64 KiB content cap. | unaudited |
| [m0_compose.rs:299-355](../../../../crates/daemon/src/m0_compose.rs#L299-L355), budget boundary and reference-model tests | Skip-and-continue selection matches the retained reference loop. | unaudited |
| [crates/daemon/tests/transform_canonical_memory.rs:48-217](../../../../crates/daemon/tests/transform_canonical_memory.rs#L48-L217) | Candidate, retired, superseded, negative-category, wrong-project, and wrong-domain examples do not appear; quarantine changes the next pass; warm state remains stable. | unaudited |
| [crates/daemon/tests/transform_canonical_memory.rs:243-318](../../../../crates/daemon/tests/transform_canonical_memory.rs#L243-L318) | Lag withholds memory; acknowledgment restores it on a later pass. | unaudited |
| [crates/daemon/tests/transform_canonical_memory.rs:322-356](../../../../crates/daemon/tests/transform_canonical_memory.rs#L322-L356) | Disabled memory takes no canonical read. | unaudited |
| [crates/daemon/tests/transform_canonical_memory.rs:358-449](../../../../crates/daemon/tests/transform_canonical_memory.rs#L358-L449) | Configured budget removes oversized memory at the reader. | unaudited |

The `REJECTED_APPROACH` category is not an explicitly rejected admission state.
No provider-level fidelity pair for an `ExplicitReject` row was found in the
delivery checks above. Preserve that negative separately.

## Hints and provider boundary

| Location and check | What its assertions cover | Status |
| --- | --- | --- |
| [transform.rs:22918-22962](../../../../crates/daemon/src/transform.rs#L22918-L22962), `user_hint_query_sanitizes_nested_reminders_comments_markup_and_tags` | Query stripping and preservation of late terms in long/multiblock input. | unaudited |
| [transform.rs:22965-22984](../../../../crates/daemon/src/transform.rs#L22965-L22984), `empty_user_hint_decision_skips_future_queries` | One empty frozen hint decision and exactly one lexical query across repeats. | unaudited |
| [rust-mode-transform.test.ts:1733-1829](../../../../packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts#L1733-L1829) | Missing recipe, wrong retry base, missing synthetic m0, and invalid delta retain input and nack; subsequent full send can recover. | unaudited |
| [invocation-budget.test.ts:7-55](../../../../packages/opencode-plugin/src/hooks/context/invocation-budget.test.ts#L7-L55) | Charges all entries with heuristic headroom; checks limit, shrinking, unknown-limit, and estimator-generation behavior. | unaudited |
| [hermetic-host.test.ts:225-307](../../../../packages/e2e-tests/src/rust-runner/hermetic-host.test.ts#L225-L307) | Fixture readiness/control contract and backend completed counter. This does not assert valid summary publication. | unaudited |
| [rust-fold-under-pressure.test.ts:15-89](../../../../packages/e2e-tests/tests/rust-fold-under-pressure.test.ts#L15-L89) | Optional fold scenario checks wire shrink and nonempty history; inactive path checks only that the explicit fold switch is off. | unaudited |

The fold switch is `EIDNARA_E2E_FOLD=1`
([rust-scenario-support.ts:12-18](../../../../packages/e2e-tests/src/rust-scenario-support.ts#L12-L18)).
The [mode manifest](../../../../packages/e2e-tests/mode-manifest.json) lists this
scenario and separate quarantine reasons for other broad scenarios. Neither
listing nor a passing gate test supplies fidelity exercise evidence.

## Record-to-check mapping

All references here reuse the inventory above; no second validator is implied.

| Record | Existing checks or required dependency | Status |
| --- | --- | --- |
| `cf-tier-transitions-preserve-qualified-meaning` | Parser fallback, render goldens, and seeded m1/warm replay; semantic review remains separate. | unaudited |
| `cf-pressure-omission-discloses-evidence-loss` | Renderer pressure/omission checks, joined to recovery's [canonical disposition predicate](../recovery/catalog.md#cf-unavailable-evidence-no-credit). | unaudited |
| `cf-memory-credit-requires-admitted-visible-content` | Canonical-reader, memory-render, and `crates/daemon/tests/transform_canonical_memory.rs` checks. | unaudited |
| `cf-hints-do-not-strengthen-source-claims` | Query preservation, empty-decision reuse, fragment filter, and wrapper size guard; no joined semantic/drop witness found. | unaudited |
| `cf-delivery-credit-requires-published-folded-capture` | Evaluation's [fixture receipt](../evaluation/catalog.md#cf-fixture-script-qualification), then each invocation's publication/range, recipe, capture, and raw-tail checks. | unaudited |
| `cf-delivery-scenarios-reach-qualified-invocations` | Component construction leads; none supplies all 14 independently reported marker outcomes and required subcases. | unaudited |
| `cf-serving-resource-boundaries` | Body-budget goldens, separate wrapper retry guard, query reuse, pinned memory/exclusion, warm m0/m1 replay, and invocation-budget checks; add offline source audit for forbidden work. | unaudited |

## Suspiciously quiet areas and routing

- None found: a combined valid-publication, accepted-range replacement,
  nonempty correlated provider capture, and raw-tail exclusion assertion.
- None found: source-backed semantic title/hint review or unavailable-versus-
  abstention accounting in deterministic delivery tests.
- None found: the complete required fidelity situation set, including a
  high-importance row removed by the hard guard, all memory negatives, and
  separately observed whole-fragment drop after hint compression.
- None found: a joined R9 check for no feature-added per-transform semantic
  judge, native reread, or storage query with preserved budgets/gates/caches/
  admission. Cost reporting, owned by evaluation, cannot replace this check.
- Existing daemon inventory counts and old hint test locations are not reused.
  The current wrapper maximum is 470 UTF-16 units, not the old inventory's
  458; the fragment cap, not the 800-unit total cap, is the reachable loss edge.

Route test strength to `/testing:invariant-test-review` and production guard
strength to `/low-level-systems:defensive-assertions-and-invariant-guards`.
The [fault map](fault-map.md) names the missing situations without selecting
a test framework or asserting that an existing component test proves them.
