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
| [decay_render.rs:547-661](../../../../crates/daemon/src/decay_render.rs#L547-L661) | Positive-budget guard demotes the first nonarchived row until the unwrapped body fits or is empty; no importance exemption. | unaudited |
| [m0_compose.rs:131-168](../../../../crates/daemon/src/m0_compose.rs#L131-L168) | Measures the wrapped session-history slice and retries at most three extra times above 105% of requested budget. It returns the last render even above that trigger. This is no body-budget allowance or guaranteed wrapper/whole-invocation cap. | unaudited |
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
| [decay_render.rs:765-770](../../../../crates/daemon/src/decay_render.rs#L765-L770), `archived_tier_is_omitted` | Explicit tier 5 renders empty. | unaudited |
| [decay_render.rs:773-777](../../../../crates/daemon/src/decay_render.rs#L773-L777), `empty_tier_body_renders_title_only_heading` | Empty P4 renders `## 3-4 · Title`. | unaudited |
| [decay_render.rs:780-832](../../../../crates/daemon/src/decay_render.rs#L780-L832), title and date/body-heading tests | XML escaping, one heading line, date formatting, and guarded nested headings. No semantic title oracle. | unaudited |
| [decay_render.rs:835-852](../../../../crates/daemon/src/decay_render.rs#L835-L852), `budget_guard_demotes_oldest_first` | Output fits 80 characters and newest survives; all three rows use importance 50. | unaudited |
| [decay_render.rs:923-949](../../../../crates/daemon/src/decay_render.rs#L923-L949), `row_counted_guard_renders_the_whole_body_guard_bytes` | Under the exact tokenizer the row-counted guard renders the bytes of a whole-body guard over 40 and 400 mixed rows at eight budgets, including rows whose headings end in whitespace. | unaudited |
| [decay_render.rs:951-985](../../../../crates/daemon/src/decay_render.rs#L951-L985), `an_under_predicting_estimator_hands_the_rest_to_the_whole_body_guard` | A `len / 4` estimator stops the row-counted demotions early on at least one budget, and the whole-body guard finishes them on the whole-body guard's bytes. | unaudited |
| [decay_render.rs:987-1024](../../../../crates/daemon/src/decay_render.rs#L987-L1024), `row_counts_predict_the_exact_body_count` | The per-row prediction equals the exact count of the joined body after demotion leaves whitespace-ended headings. | unaudited |
| [decay_render.rs:1026-1050](../../../../crates/daemon/src/decay_render.rs#L1026-L1050), `guard_demotions_count_changed_rows_and_the_body_once` | Over 200 demotions of 2,000 rows count the whole body once. | unaudited |
| [decay_render.rs:1053-1094](../../../../crates/daemon/src/decay_render.rs#L1053-L1094), `stored_history_segment_projects_and_renders` | Stored-row projection, P1 rendering, empty-P1 flat content, and fallback to flat content. | unaudited |
| [decay_render.rs:1172-1210](../../../../crates/daemon/src/decay_render.rs#L1172-L1210), `render_golden_matches_reference` | Nonempty golden corpus and exact rendered bodies, with per-case mismatch diagnostics. | unaudited |
| [decay_render.rs:1213-1336](../../../../crates/daemon/src/decay_render.rs#L1213-L1336), `redacted_store_shape_matches_ts_at_real_history_budgets` | Fixture shape, token costs, budget fit, body hashes, and tier counts match the retained differential table. | unaudited |
| [decay_render.rs:1339-1384](../../../../crates/daemon/src/decay_render.rs#L1339-L1384), `render_tight_golden_matches_reference_with_real_estimator` | Exact body goldens and every tight case fitting its budget or reaching the floor. This is not hand-reviewed meaning. | unaudited |

No corpus-specific high-importance omission or authored/parsed/effective-tier
provenance assertion was found. Existing generic tests should be reused.

## Publication, composition, and warm replay

| Location and check | What its assertions cover | Status |
| --- | --- | --- |
| [history_summarizer.rs:4154-4205](../../../../crates/daemon/src/history_summarizer.rs#L4154-L4205), `wired_history_summarizer_happy_path_sends_validates_and_publishes` | One scripted producer start, Idle completion, publication floor, appended segment identity, and stored P1. | unaudited |
| [history_summarizer.rs:4208-4258](../../../../crates/daemon/src/history_summarizer.rs#L4208-L4258), `selected_range_identity_drift_during_await_rejects_without_cooldown` | Await-time identity mutation rejects publication, leaves prior rows, and does not advance the floor. | unaudited |
| [transform.rs:21111-21216](../../../../crates/daemon/src/transform.rs#L21111-L21216), `new_history_segment_extends_coverage_on_soft_advancing_the_anchor` | Seeded publication drives m1, advances boundary, removes covered raw tail, and replays m0/m1 identically on SOFT+. It does not run the producer. | unaudited |
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
| [memory_render.rs:256-279,307-334](../../../../crates/daemon/src/memory_render.rs#L256-L334) | Nonpositive categories drop; UTF-8 truncation respects the 64 KiB content cap. | unaudited |
| [m0_compose.rs:294-350](../../../../crates/daemon/src/m0_compose.rs#L294-L350), budget boundary and reference-model tests | Skip-and-continue selection matches the retained reference loop. | unaudited |
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
