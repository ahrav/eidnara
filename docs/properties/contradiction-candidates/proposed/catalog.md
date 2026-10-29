# Proposed obligation catalog: P5 write-time contradiction candidates

Part of `docs/properties/contradiction-candidates/`. Companion part:
`../dependencies/` (discovered facts about implemented surfaces).

These records were authored at `3a3b916b7` as proposed design obligations
for unbuilt behavior. #1164 builds the scorer
(`daemon::contradiction_candidates::scorer`,
`crates/daemon/src/contradiction_candidates/scorer.rs`) and converts the
three S-A records to `Status: active` with `Exercised: yes`; their tests are
in `crates/daemon/tests/contradiction_candidates_scorer.rs`. The references of
every record #1164, #1167, or #1168 converts are verified at the #1168 branch
tip. The store (#1165), sweep (#1166), pass (#1167), and read (#1168) tickets
make the other records checkable. #1166 builds the sweep and
converts the sweep record to `Status: active`. #1167 builds the pass and
converts the trigger, single-flight, candidate-set, partial-result, row-key,
rebuild, and no-Kernel-write records; the flag-off record keeps its read arm
for #1168. #1168 builds the read route and the tool action and converts the
two read records, the confirm-linkage record, and the flag-off record, so
every record is active at its tip. Source: the
`/software-design:invariant-driven-domain-modeling` pass whose certificate is
`certificate.md`; its open items are listed in `open-questions.md`. The
records were first proposed by that pass, and each carries the reachability
class verified at the branch tip.

Companion files: [existing-checks.md](existing-checks.md),
[fault-map.md](fault-map.md),
[portfolio-evaluation.md](portfolio-evaluation.md), `evidence/<slug>.md`
for the active records, [certificate.md](certificate.md),
[open-questions.md](open-questions.md).

## Index

| Slug | Surface | Type | Semantics | Reachability | Status |
| --- | --- | --- | --- | --- | --- |
| cc-scorer-output-deterministic-under-contract-version | S-A | safety | always | explicit-config-only | active, exercised yes |
| cc-oversize-text-refused-whole-never-truncated | S-A | safety | always | explicit-config-only | active, exercised partial (result-equivalence clause unasserted) |
| cc-case-fold-applied-identically-to-both-sides | S-A | safety | always | explicit-config-only | active, exercised yes |
| cc-pass-starts-only-after-publication-job-terminal | S-B | safety | always | explicit-config-only | active, exercised partial (#1167) |
| cc-passes-in-flight-bounded-and-overflow-counted | S-B | safety | always | explicit-config-only | active, exercised yes (#1167) |
| cc-candidate-set-bounded-with-counted-truncation | S-C | safety | always | explicit-config-only | active, exercised partial (#1167) |
| cc-partial-result-is-descriptor-granular-prefix | S-C | safety | always | explicit-config-only | active, exercised yes (#1167) |
| cc-row-key-unique-and-caps-enforced | S-D | safety | always | explicit-config-only | active, exercised yes (#1165, #1167) |
| cc-table-rebuildable-from-evidence-bytes-and-live-decisions | S-D | safety | always | explicit-config-only | active, exercised yes (#1165, #1167) |
| cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched | S-E | safety | always | explicit-config-only | active, exercised partial (#1166) |
| cc-read-serves-only-live-decision-rows-ids-and-scores-only | S-F | safety | always | explicit-config-only | active, exercised yes (#1168) |
| cc-read-distinguishes-disabled-unavailable-and-empty | S-F | safety | always | default-production | active, exercised yes (#1168) |
| cc-flag-off-no-pass-no-write-no-table-read | S-G | safety | always(!X) | default-production | active, exercised yes (#1166, #1167, #1168) |
| cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm | S-H | safety | always | explicit-config-only | active, exercised yes (#1168) |
| cc-pass-and-sweep-write-no-kernel-event-and-no-signal | R5 | safety | always(!X) | explicit-config-only | active, exercised partial (#1166, #1167) |

15 records at the #1168 tip: 15 active, 0 proposed.
Semantics distribution: 15 `always` (13 plain and 2 written `always(!X)` for
forbidden states), 0
`always-or-unreached`, 0 `sometimes`, 0 `reachable`, 0 `unreachable`; all 15
are safety. Reachability: 13 `explicit-config-only`, 2 `default-production`
(verified at the #1168 tip). The ticket mapping in the
Status column follows the #1163 decomposition; each ticket re-verifies the
records it converts.

## Source preamble and records

Schema: `docs/properties/METHOD.md` at HEAD `3a3b916b7`. As authored, every
record was a proposed design obligation for unbuilt behavior
(`Status: active - PROPOSED`,
`Exercised: not yet`, `Existing check: none; proposed`). Reachability is the
intended class once the unit lands; the whole feature is behind
`contradiction_candidates.enabled`, default false, so every runtime record is
`explicit-config-only` except where noted. Each proposed record adds a
Falsifier line and a Legal witness line after the schema fields; the active
records replace both with their tests. Slug prefix `cc-`.

Record count: 15. By surface: S-A 3, S-B 2, S-C 2, S-D 2, S-E 1, S-F 2,
S-G 1, S-H 1, R5 cross-cutting 1.

Vocabulary used below: "eligible decision" is the one shared predicate OQ-B
fixed, built by #1166 as `eligible`
(`crates/daemon/src/contradiction_candidates/eligibility.rs:7-24`: registry
row served at the snapshot, `memory` domain, project scope term accepted,
visible to explicit search, no successor, disposition not in {`superseded`,
`contradicted`, `rejected`, `quarantined`}). "Scorer contract version V" names the constant
that embeds `ANALYSIS_CONTRACT_EPOCH` (`crates/retrieval/src/lexical/mod.rs:40`),
the fold function F, the formula, threshold, and K.

---

## S-A scorer

### cc-scorer-output-deterministic-under-contract-version

Type: safety
Reachability: explicit-config-only - the candidate pass calls the scorer
(`term_set`, `passes_gates`, and `TopCandidates`,
`crates/daemon/src/contradiction_candidates/pass.rs:300,309,325,339`) for every
publication it scores, and the pass runs only under the route's frozen
`contradiction_candidates.enabled` (#1167).
Status: active
Exercised: yes - `the_top_k_is_the_same_under_every_input_order`
(`crates/daemon/tests/contradiction_candidates_scorer.rs:276-318`) scores one
descriptor against nine summaries
(five 4-of-4 ties, a 6-of-6, a 4-of-5, a 5-of-6, and one unscorable
summary), so K + 1 ties compete for the last slots, and asserts an equal
`DescriptorScore` for nine rotations and their reversals;
`a_decision_object_id_appears_once_and_the_better_entry_wins_in_any_order`
(`:320-346`) gives two entries for one decision object id in both orders and
asserts one kept entry, the better-ranked one. No test combines a repeated
decision object id with more than K passing summaries across many orders;
the stronger oracle is a property test over random permutations of a list
that holds repeated object ids and more than K passing summaries.
Guarantee: for equal descriptor text, an equal candidate list up to order
(as `(decision object id, decision id, summary)` triples), and an equal
matcher digest, `score_descriptor` returns an equal `DescriptorScore`: at
most `CANDIDATES_PER_DESCRIPTOR` (K = 4) candidates, one per decision object
id, in the total order score descending (by cross-multiplication of
`matched_terms` and `decision_terms`), `matched_terms` descending,
`decision_terms` ascending, decision object id bytes ascending, decision id
bytes ascending, and the unscorable decision object ids in id order.
Check: `always` - for every permutation `p` of the candidate list,
`score_descriptor(d, p(c)) == score_descriptor(d, c)`, including the order of
tied candidates and the `unscorable` list; `always` because the scorer is a
pure function and every evaluation must satisfy the predicate.
Fault/timing angle: none; pure computation.
Required faults and enabling state: a candidate list with more than K
passing summaries, ties on score and `matched_terms` past position K, an
unscorable summary, a repeated decision object id, and at least two input
orders.
Confidence: high - [evidence](evidence/cc-scorer-output-deterministic-under-contract-version.md).
Read `crates/daemon/src/contradiction_candidates/scorer.rs:432-473` (the
descriptor table is sorted by hash, then folded text, and a summary
contributes only its `matched_terms` and `decision_terms` counts, so no
iteration order reaches the output), `:386-395` (`rank`, a total order
ending at both ids), `:577-610` (`record`, `offer`), `:631-648`
(`score_descriptor` sorts `unscorable`), `:652-687` (the preimage names the
order rule).
Existing check: the two tests above and `the_matcher_digest_pins_every_rule`
(`crates/daemon/tests/contradiction_candidates_scorer.rs:348-400`), which pins the order rule id in the preimage;
`each_descriptor_keeps_the_top_k_that_score_descriptor_ranks_over_the_loaded_summaries`
(`crates/daemon/src/candidate_pass_tests.rs:980`) shows the pass's single scan
keeps, per descriptor, the top K `score_descriptor` ranks. Status
unaudited.
Impact: a nondeterministic order defeats the rebuild obligation
(`cc-table-rebuildable-from-evidence-bytes-and-live-decisions`) and makes
the top K a function of SQL row order.
Open questions:
- No test runs the scorer on a second thread or process. The module holds
  no shared mutable state other than the `LazyLock` matcher digest
  (`crates/daemon/src/contradiction_candidates/scorer.rs:698-699`), so this pass judges a second-thread test low value; the
  rebuild record exercises a second run once the store lands (#1165).

### cc-oversize-text-refused-whole-never-truncated

Type: safety
Reachability: explicit-config-only - the candidate pass calls the scorer
(`term_set`, `passes_gates`, and `TopCandidates`,
`crates/daemon/src/contradiction_candidates/pass.rs:300,309,325,339`) for every
publication it scores, and the pass runs only under the route's frozen
`contradiction_candidates.enabled` (#1167).
Status: active
Exercised: partial - scorer and pass arms, one Check clause unasserted.
`an_oversize_descriptor_is_refused_whole_and_an_exact_fit_is_analyzed`
(`crates/daemon/tests/contradiction_candidates_scorer.rs:224-241`) asserts
`DESCRIPTOR_BOUNDS` is 65536 bytes
and 32769 atoms, analyzes a 65536-byte descriptor of 32768 one-letter atoms,
and gets `Err(InputTooLong { bytes: 65537, bound: 65536 })` from
`score_descriptor` for one more byte;
`an_unscorable_summary_is_reported_and_the_others_still_score` (`:243-274`)
asserts `SUMMARY_BOUNDS.max_atoms` is 4097, analyzes a 4096-atom summary,
refuses an 8193-byte summary, and reports it and two floor-emptied summaries
unscorable while an 8192-byte summary and a plain one still score;
`a_contained_summary_scores_full_and_disjoint_texts_score_nothing`
(`:100-129`) shows an analyzed zero overlap is `Below { matched_terms: 0,
.. }`, distinct from a refusal. The clause that every other summary's
result equals its result without the unscorable summary present has no
assertion: `:243-274` checks the kept ids only, and no test compares the
result against a run without that summary. Pass arm (#1167):
`an_oversize_descriptor_writes_nothing_and_an_unscorable_summary_leaves_the_others`
(`crates/daemon/src/candidate_pass_tests.rs:805`) runs `score_publication`
with a 65537-byte descriptor beside a scorable one and observes zero rows for
it, `descriptors_refused_oversize` = 1, and `summaries_unscorable` = 2 for an
over-bound and a floor-emptied summary while the scorable descriptor keeps
its row.
Guarantee: a descriptor over `DESCRIPTOR_BOUNDS` is refused whole:
`DescriptorTerms::new` and `score_descriptor` return the analyzer's
`LexicalRefusal` and no candidate; a summary over `SUMMARY_BOUNDS`, or with
no term left after the floor, is `SummaryScore::Unscorable` and its decision
object id is listed in `unscorable`, while the other summaries score;
`unscorable` lists decision object ids, not decision ids
(`crates/daemon/src/contradiction_candidates/scorer.rs:640`), so with a
repeated object id one id can appear twice in `unscorable`, or in both
`unscorable` and `candidates` when another entry with that id scores; no
analysis of a prefix of either text contributes to any score; a refusal is
an `Err` and never a score of 0.
Check: `always` - `bytes(d) > 65536 ⇒ score_descriptor(d, c) ==
Err(InputTooLong)`; `bytes(d) == 65536` with the most atoms that length
admits `⇒ Ok`; for a summary over 8192 bytes or emptied by the floor, its
object id is in `unscorable`, and when no other entry in the list carries
that object id it is absent from `candidates`; every other summary's
result equals its result without that summary present; the semantics is
`always` because every call carries the rule and the analyzer refuses
before retention (`crates/retrieval/src/lexical/analysis.rs:78-105`).
Fault/timing angle: none.
Required faults and enabling state: a descriptor one byte over the bound and
one at it with the maximum atom count; a summary one byte over its bound and
one at it; a floor-emptied summary next to scorable ones. The atom bounds are
`max_input_bytes / 2 + 1`
(`crates/daemon/src/contradiction_candidates/scorer.rs:85-96`), one past the
most atoms the byte bound admits, because n atoms need at least 2n - 1
bytes; an input with `max_atoms` atoms exceeds the byte bound first, so
`TooManyAtoms` is unreachable through the scorer and no test constructs it.
Confidence: high - [evidence](evidence/cc-oversize-text-refused-whole-never-truncated.md).
Read `crates/daemon/src/contradiction_candidates/scorer.rs:64-70`, `:85-96`,
`:103-109`, `:440-473`, `:534-566`,
`:631-648`; the compile-time assertion at `:66-68` holds the descriptor
bound at or above `MAX_SOURCE_UNIT_BYTES`
(`crates/daemon/src/harness_sources.rs:614`).
Existing check: the three tests above; the analyzer's own refusal tests
(`crates/retrieval/tests/lexical_analysis.rs:244-297`). Status unaudited.
Impact: a truncated analysis scores the wrong text and can produce a false
candidate the agent then disputes.
Open questions:
- The ticket states the atom bound two ways: `(max_input_bytes + 1) / 2`
  (32768 for the descriptor) in its Scope, and the literal values 32769 and
  4097 in OQ5-3. The code and the digest pin follow the literals. Under
  either formula the byte bound refuses first, so the behavior is the same;
  the ticket text needs one formula (needs human input).
- The pass arm (zero rows, `descriptors_refused_oversize`,
  `summaries_unscorable`) is unverifiable until #1167 lands.

### cc-case-fold-applied-identically-to-both-sides

Type: safety
Reachability: explicit-config-only - the candidate pass calls the scorer
(`term_set`, `passes_gates`, and `TopCandidates`,
`crates/daemon/src/contradiction_candidates/pass.rs:300,309,325,339`) for every
publication it scores, and the pass runs only under the route's frozen
`contradiction_candidates.enabled` (#1167).
Status: active
Exercised: yes - the case-fold test
`case_folds_identically_on_both_sides_and_diacritics_do_not_fold`
(`crates/daemon/tests/contradiction_candidates_scorer.rs:131-159`) scores the
uppercase descriptor
`"ÉCOLES CACHE EVERY KERNEL WRITER"` against the summary
`"Écoles cache every Kernel writer"` as `Candidate { matched_terms: 5,
decision_terms: 5 }`, equal to the summary scored against itself, and
scores the plain-`e` and NFD spellings of `"école école1 école2 école3"` as
`Below { matched_terms: 0, decision_terms: 4 }`;
`term_sets_follow_the_documented_fold_and_floor` (`:31-80`) folds
`"ÉCOLE École"` to one term, keeps `İSTANBUL` (lowered to `i` plus a
combining dot) distinct from `istanbul`, and applies the floor after the
fold; `a_case_change_inside_an_identifier_changes_its_parts` (`:163-171`)
gives `configLoader` the terms {configloader, config, loader} and both
`CONFIGLOADER` and `configloader` the term {configloader}.
Guarantee: one fold, `str::to_lowercase` per term, is applied to the
analyzer's output on both sides: every term of the descriptor and of every
summary is folded by the same rule in `terms` and `scan_ascii`, which fold
an ASCII term by its ASCII lowercase (equal to `str::to_lowercase` on
ASCII) and any other term by `str::to_lowercase`, and `term_set` returns
those folded terms; no diacritic removal or Unicode normalization is
applied. The analyzer splits parts on the original
case before the fold (`crates/retrieval/src/lexical/analysis.rs:193-204`:
at a lower-to-upper change and before the last capital of an uppercase run
that two lowercase letters follow), so two texts that differ only in case
have equal term sets and score as the text against itself when the case
difference leaves every part boundary in place; a case change that adds or
removes a part boundary changes the parts.
Check: `always` - `term_set(x) == term_set(y)` whenever `x` and `y` differ
only in letter case under `to_lowercase` and every atom of `x` splits into
parts at the same positions as the matching atom of `y`; a descriptor
differing from a summary only in such case changes scores
`matched_terms == decision_terms`; `term_set("configLoader") !=
term_set("configloader")`; a term that differs from another only in
diacritics, or only in NFC versus NFD form, is a distinct term, so such a
pair contributes no match. `always` because the fold is part of the pure
function's contract.
Fault/timing angle: none.
Required faults and enabling state: mixed-case pairs above the count gates;
a non-ASCII pair; a pair whose lowercase mapping changes length (`İ`); a
case-only pair whose case change crosses a part boundary (`configLoader`
and `configloader`); a diacritic-only pair and an NFC and NFD pair.
Confidence: high - [evidence](evidence/cc-case-fold-applied-identically-to-both-sides.md).
Read `crates/daemon/src/contradiction_candidates/scorer.rs:111-132` (`terms`:
an ASCII term folds as its `to_ascii_lowercase`, which equals
`str::to_lowercase` on ASCII, and any other term through `str::to_lowercase`
at `:124`), `:221-270` (`scan_ascii` floors and hashes the ASCII lowercase),
`:440-473` (`DescriptorTerms::new` takes the descriptor's terms from
`terms`), `:491-566` (`score` takes the summary's terms from `scan_ascii`
through `count_ascii`, or from `terms` when a term is not ASCII or more than
64 are distinct), `:18-25` (the module header states the fold is not full
case folding), `:77` (`FOLD_RULE_ID` enters the preimage);
`crates/retrieval/src/lexical/analysis.rs:193-204` (`splits`, the
case-driven part boundaries).
Existing check: the three tests above; `the_matcher_digest_pins_every_rule`
(`:348-400`) pins the fold rule id;
`term_sets_equal_the_analyzer_rules_on_generated_text`
(`crates/daemon/tests/contradiction_candidates_term_sets.rs:86`) compares
`term_set` with the analyzer's atoms and parts under the same fold and floor
on generated mixed-script text. Status unaudited.
Impact: a fold applied on one side only hides a real overlap whenever the
descriptor and the summary differ in case.
Open questions:
- Containment scores the decision side, so score(a, b) and score(b, a)
  differ by design when the term counts differ; the fold itself is
  symmetric. The ticket fixes the fold (#1164 OQ5-5): `to_lowercase` only,
  so `ß` and `ss` are different terms.
- The case-driven part split is inherited analyzer behavior
  (`crates/retrieval/src/lexical/analysis.rs:4`): a descriptor that writes
  `CONFIGLOADER` loses the `config` and `loader` matches that
  `configLoader` gives. Whether that is intended matcher behavior, or the
  fold should precede the split under a new `CANDIDATE_MATCHER_EPOCH`,
  needs human input (portfolio B2).

## S-B trigger

### cc-pass-starts-only-after-publication-job-terminal

Type: safety
Reachability: explicit-config-only - `capture_after_response`
(`crates/daemon/src/lib.rs:9279`) offers a pass only when the route's frozen
`contradiction_candidates.enabled` and a memory store are both present.
Status: active
Exercised: partial - #1167, `crates/daemon/src/candidate_pass_tests.rs`:
`a_pass_starts_only_after_the_job_releases_the_slot_and_the_gate` (`:284`)
holds the `before_publish` hook and observes zero offers and zero rows, then
observes the pass start with the capture slot idle and the commit gate free
and score every descriptor with the gate free, one `completed` pass, and one
row; a memo-hit job that publishes nothing offers no pass; each job ends in
one terminal count; a replayed unit is scored again with its rows' tip at or
above the earlier one. `publish_latency_with_candidates_on_stays_within_the_off_baseline`
(`:1275`) commits at least one publication beside a running pass and keeps
the median publication hold within three times the flag-off median plus
50 ms. `a_job_whose_document_capture_ends_in_deadline_still_offers_its_published_units`
(`:1618`) commits a window batch and then ends the job `deadline` in its
document batch, and observes one offered pass and one row: a kept unit's
commit landed and the memo holds it, so the pass is offered whatever count
ended the job after that commit. The job hands its kept texts out through `CaptureJob::run_settled`
(`crates/daemon/src/source_capture.rs:801`) after the blocking closure has
dropped the slot permit, and the pass runs under its own `EvalBudget`
(`crates/daemon/src/contradiction_candidates/pass.rs:175`), a fact read from
the code. Not constructed: the `between_screen_and_commit` hook held, a
cancel while the job waits in a hook, and the `refused_shutting_down`
branch.
Guarantee: The candidate pass starts only after `CaptureJob::run_settled`
(`source_capture.rs:801-825`) has produced its terminal `Count` and the
capture commit gate guard has been dropped (`source_capture.rs:1006`); it runs
under an `EvalBudget` whose origin is the pass start and whose interrupt is the
daemon cancel, never the job's budget; the response bytes were handed to the
wire writer before the `AfterResponse` closure ran (`lib.rs:9275-9324`).
Check: `always` - with the `before_publish` or `between_screen_and_commit`
test hook (`source_capture.rs:986, 1040`) blocked, the candidate table has
zero rows and no pass is offered; after the hook releases and the job
returns, the pass runs; the semantics is `always` because every publication
must respect the ordering.
Fault/timing angle: the window between `publish_screened_between` returning
and `drop(gate)`; the window between the job's terminal and the pass start;
daemon cancel during either.
Required faults and enabling state: flag on; a publication with at least one
`Published` unit; the two test hooks held; a cancel raised while the job is
blocked in a hook.
Confidence: high - [evidence](evidence/cc-pass-starts-only-after-publication-job-terminal.md)
Existing check: the tests above (#1167), unaudited.
Impact: the pass would consume the 5 s publication budget or hold the gate,
delaying publication and session deletion (KTD2 stop condition).
Open questions:
- Resolved by #1167 (OQ-C): the job carries each published unit's text and
  receipt out of its blocking closure; nothing is read back from evidence.

### cc-passes-in-flight-bounded-and-overflow-counted

Type: safety
Reachability: explicit-config-only - as `cc-pass-starts-only-after-publication-job-terminal`.
Status: active
Exercised: yes - #1167, `crates/daemon/src/candidate_pass_tests.rs`:
`a_second_offer_while_a_pass_runs_is_refused_and_a_cancel_still_ends_the_pass`
(`:400`) parks a pass, publishes again, and observes `refused_pass_busy` = 1
with `active` = 1 and no row for the refused publication; a cancel during
descriptor 1 of 3 ends the pass with `budget_exhausted`, the first
descriptor's rows, `descriptors_unscored` = 2, and `active` back at 0;
`a_panicking_pass_releases_its_permit_and_charge_and_shutdown_waits_for_a_parked_pass`
(`:1217`) shows a panicking pass counted with its permit and charge released
and shutdown waiting for a parked pass;
`a_pass_queued_past_its_budget_ends_at_the_deadline_it_was_admitted_under`
(`:467`) fills an 8-thread blocking pool so the admitted pass queues for
600 ms under a 200 ms budget, observes `active` = 1 while it queues, and
observes `deadline` = 1, `descriptors_unscored` = 1, `pass_ms_last` >= 600,
and no row once it runs.
P = 1 per store through a non-waiting permit
(`crates/daemon/src/contradiction_candidates/pass.rs:111`); the pass's start
is fixed in `run` before `spawn_blocking` (`pass.rs:147-151`) and is the origin
of both its deadline (`:196-199`) and `pass_ms_last`, so a queued pass holds
the permit for its queue wait plus one budget check and reports that wait.
Guarantee: At most P = 1 candidate pass exists at once per store; an offer
that finds the permit taken is refused and counted `refused_pass_busy`, never
queued; a daemon cancel ends a running pass at its next budget check, after
at most one unbudgeted Kernel read (the visible-decision or payload-size
read).
Check: `always` - after N rapid publications with the pass blocked on a test
hook, running passes are at most 1 and `refused_pass_busy` equals N - 1; the
semantics is `always` because every offer is subject to the bound.
Fault/timing angle: a burst of publications faster than one pass completes;
a pass blocked on a slow Kernel read.
Required faults and enabling state: flag on; a test hook that parks the pass;
N > P publications in one session.
Confidence: high - [evidence](evidence/cc-passes-in-flight-bounded-and-overflow-counted.md)
Existing check: the tests above (#1167), unaudited.
Impact: unbounded passes would hold Kernel readers and memory proportional to
publication rate.
Open questions:
- Resolved by #1167 (OQ5-11): P = 1 per store, matching the publication
  slot; the counter is `refused_pass_busy`.

## S-C candidate set bounding

### cc-candidate-set-bounded-with-counted-truncation

Type: safety
Reachability: explicit-config-only - as `cc-pass-starts-only-after-publication-job-terminal`.
Status: active
Exercised: partial - #1167, `crates/daemon/src/candidate_pass_tests.rs`:
`the_candidate_set_keeps_the_newest_scope_matched_decisions_of_the_project_and_counts_the_cut`
(`:665`) seeds N + 2 visible decisions with the cap at N = 4: the two oldest
are cut, `candidates_truncated` = 2, the other project's decision is never scored,
and stale and disputed decisions are scored while the contradicted one is
hidden by explicit search; `a_superseded_decision_is_never_scored_and_the_default_bounds_are_the_named_constants`
(`:713`) pins the production values (N = 4096) and shows a superseded
decision unread; `the_summary_byte_cut_skips_payloads_before_any_load`
(`:777`) cuts payloads before any load and counts their bytes. Not
constructed at the production N: the tests cut at a reduced cap. No test
shows the pass's `eligible` filter
(`crates/daemon/src/contradiction_candidates/pass.rs:497-501`) as the reason a
decision is dropped.
Guarantee: The pass's candidate set is built by the project scope term
prefilter (`admission.rs:3138-3144`, `project.rs:106-111`) and the
`ScopeFilter` judge, then cut to at most N visible, scope-matched decisions
in a deterministic newest-first order, counting each decision cut when more
were visible, before the eligibility filter; summary bytes are bounded before
any payload is loaded (`decision_payload_sizes_as_of`, `slice/read.rs:330`);
the pass never loads more than N summaries or more than the byte bound.
Check: `always` - with N + k scope-matched visible decisions the pass keeps
exactly the N newest for the eligibility judgment, the k oldest are the ones
omitted, and `candidates_truncated` increments by k, so the rows scored are
the eligible among those N; a
decision with an oversize payload is skipped before load and counted; the
semantics is `always` because every pass must stay inside the bound.
Fault/timing angle: a project whose `memory` decisions exceed N; a decision
with a very large summary.
Required faults and enabling state: flag on; N + 1 eligible decisions in one
project and some in another project (must be excluded by the prefilter); one
decision whose payload exceeds the byte bound.
Confidence: medium - [evidence](evidence/cc-candidate-set-bounded-with-counted-truncation.md);
the tests cut at a reduced cap; the visible-decision
read itself has no row bound or budget (the `sometimes` witness
`p5dep-kernel-domain-read-has-no-row-bound`)
Existing check: the tests above (#1167), unaudited.
Impact: pass cost grows with project size; a large project stalls publication
follow-up and holds Kernel readers.
Open questions:
- Resolved by #1167: N = 4096 and 4 MiB of summary payload; the SQL walk is
  accepted with a row-capped, budgeted Kernel read recorded as a follow-up.

### cc-partial-result-is-descriptor-granular-prefix

Type: safety
Reachability: explicit-config-only - as `cc-pass-starts-only-after-publication-job-terminal`.
Status: active
Exercised: yes - #1167, `crates/daemon/src/candidate_pass_tests.rs`:
`a_deadline_during_the_third_descriptor_leaves_the_first_two` (`:837`)
expires the budget during descriptor 3 of 5 and observes four rows for each
of descriptors 1 and 2, none for 3 to 5, `descriptors_unscored` = 3, and
`deadline` = 1; `a_budget_spent_before_the_pass_writes_nothing_and_counts_every_descriptor`
(`:869`) and the cancel case of
`a_second_offer_while_a_pass_runs_is_refused_and_a_cancel_still_ends_the_pass`
(`:400`) cover the empty prefix and a cancel at the second of three
descriptors;
`a_pass_stopped_during_the_scan_counts_the_summaries_the_analyzer_refused_before_the_stop`
(`:1438`) and `a_pass_stopped_between_payload_chunks_counts_the_summaries_already_loaded`
(`:1472`) stop the pass at a check inside the scan and between two payload
chunks and observe `summaries_unscorable` and `summaries_loaded` holding the
counts reached before the stop;
`a_restore_between_the_reads_and_the_write_ends_the_pass_and_keeps_the_held_rows`
(`:1497`) restores the Kernel after the pass's last read and observes the pass
end `kernel_unavailable` with every descriptor unscored and the held rows in
place; `a_write_behind_a_held_store_connection_ends_at_the_pass_deadline`
(`:1547`) holds the store's connection from another thread past the budget
and observes the pass end `deadline` at its write with no row; the three
`*_behind_held_kernel_readers_ends_at_the_pass_deadline` tests (`:1721`,
`:1726`, `:1731`) hold both pooled Kernel readers from a
checkpoint before the visible-decision, scope-term, and payload-size reads
and observe the pass end `deadline` within its budget;
`a_pass_queued_past_its_budget_ends_at_the_deadline_when_the_kernel_is_gone_too`
(`:481`) marks the Kernel unavailable while a pass queues past its
budget and observes `deadline`, not `kernel_unavailable`. The pass
scores every descriptor in one scan of the loaded summaries, then writes each
descriptor's rows in publish order, one store write per descriptor; a stop
during the scan writes no row (`crates/daemon/src/contradiction_candidates/pass.rs:263`),
and before each write the pass checks the Kernel incarnation it read at
(`pass.rs:372-377`); the write's wait for the store connection ends at the
pass deadline (`pass.rs:408`,
`crates/memory-store/src/contradiction_candidates.rs:513`).
Guarantee: When the pass's budget is exhausted or interrupted, the rows
written equal exactly the rows a complete pass would have written for a
prefix of the publication's descriptors in publish order; a descriptor whose
candidate set was not fully scored contributes no rows; the exhaustion is
counted once.
Check: `always` - with a deadline injected during descriptor i of n, rows
exist for descriptors 1..i-1 only, each with its full top-K, and `deadline`
increments by one; the semantics is `always` because every interrupted pass
must leave this shape.
Fault/timing angle: deadline or cancel between two descriptors; deadline
inside the summary read or the scoring loop of one descriptor; crash between
two descriptor writes; a Kernel restore between the incarnation check and the
store write of one descriptor (the check runs without a Kernel guard, so this
window is the write itself): the rows written cite decisions of the replaced
history under the incarnation id, which a restore keeps, the sweep drops the
ones the restored tip judges ineligible, and the descriptor's next
publication replaces the rest. Holding one of the Kernel's two pooled readers
through the store write would close the window at the price of that reader
held through a store wait bounded by the pass deadline; the pass leaves the
window open.
Required faults and enabling state: flag on; a batch of several `Published`
units; a budget that expires mid-pass (test clock or interrupt flag).
Confidence: high - [evidence](evidence/cc-partial-result-is-descriptor-granular-prefix.md)
Existing check: the tests above (#1167), unaudited.
Impact: a half-scored descriptor's rows are not its top-K; a reader sees a
false ranking and the rebuild obligation cannot be stated.
Open questions:
- Resolved by #1167: the pass writes per descriptor.

## S-D row semantics

### cc-row-key-unique-and-caps-enforced

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - direct-write store arm (#1165): `crates/memory-store/tests/contradiction_candidates.rs`
`each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip` (`:83`) shows one
row per key, a lower-tip rewrite that keeps the higher tip and replaces the
other values, and a newer write that removes keys it no longer names;
`a_late_write_replaces_held_keys_and_adds_none` (`:453`) shows a write below
the held tip adds no key, so a descriptor never exceeds K;
`a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole` (`:125`)
and `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` (`:694`)
show K accepted, K + 1 refused with the held rows unchanged, and a project
at `per_project` evicting the lowest `score_bp`, then oldest, then smallest
key with the count returned; `a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest`
(`:198`) shows another project's rows are untouched;
`a_new_row_the_cap_evicts_at_once_counts_as_written_and_dropped` (`:1483`)
shows a new row below the cap's floor counted written and dropped with the
held rows and rowids unchanged; and
`random_write_and_delete_sequences_match_the_reference_model` (`:1213`)
compares 24 seeded sequences of writes, late writes, incarnation changes,
evictions at caps of 2 to 4096, and as-read deletes against a reference model
of the rules, checking every row, the returned counts, rowid stability, and
the gauge after each step, and pages, cursors across writes, and the sweep
every seventh step. The store
(`crates/memory-store/src/contradiction_candidates.rs:497`) enforces K and the
project cap in the write transaction; the daemon owns the values. Pass arm
(#1167): `the_row_caps_hold_per_descriptor_per_pass_and_per_project_and_a_replay_keeps_the_rows`
(`crates/daemon/src/candidate_pass_tests.rs:889`) scores K + 1 overlaps into K rows, cuts a pass at M in publish
order with `rows_written` = M and the rest counted `dropped_at_cap`, counts
the rows one write drops at the project cap (the store tests cover evicting
held rows), rescoring at a newer tip keeps
each key with a higher `publish_tip`, and a descriptor past M keeps the rows
it already holds; the handler test at `:284` republishes a replayed unit
through the job and keeps the same keys;
`equal_texts_in_one_batch_cite_one_evidence_row_and_the_pass_scores_it_once`
(`:1375`) publishes two equal-text units, which cite one evidence row, and
observes one analyzed descriptor, `rows_written` = 1, and one row, since the
pass scores each evidence id once (`pass.rs:179-182`). The over-M count is
`dropped_at_cap`; the C13 vocabulary has no `pass_cap_reached`.
Guarantee: The table key is (project digest, descriptor evidence row id
`srcev:{key}`, decision object id); a write for an existing key replaces `decision_id`,
`score`, `matched_atoms`, `publish_tip`, `created_at_ms`; one pass writes at
most K rows per evidence id and at most M rows in total, stopping at M with a
count; the row carries no column that any command reads for admission.
Check: `always` - after any sequence of passes, `SELECT COUNT(*) GROUP BY
key` is 1 for every key; after one pass with a descriptor matching K + 1
decisions the evidence id has K rows; after one pass whose descriptors would
match more than M, exactly M rows are written and the rest are counted
`dropped_at_cap`; the semantics is `always` because the predicate holds at every
pass commit.
Fault/timing angle: a replayed publication (`Published.replayed`,
`harness_sources.rs:559`) re-scoring the same evidence id; two passes for one
project interleaving (bounded by P).
Required faults and enabling state: flag on; a descriptor overlapping K + 1
decisions; a batch whose total matches exceed M; the same unit published
twice (replay).
Confidence: high - [evidence](evidence/cc-row-key-unique-and-caps-enforced.md)
Existing check: the store and pass tests above, unaudited.
Impact: duplicate rows inflate candidate counts; uncapped rows make the
table grow with project size per pass.
Open questions:
- Resolved by #1167: K = 4 and M = 64; a replayed descriptor is re-scored.

### cc-table-rebuildable-from-evidence-bytes-and-live-decisions

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - store arm (#1165):
`truncating_and_replaying_the_writes_rebuilds_the_same_rows`
(`crates/memory-store/tests/contradiction_candidates.rs:371`) truncates through the test-support
`truncate_contradiction_candidates_for_test` and replays a write sequence,
reproducing the same rows excluding `created_at_ms`. Pass arm (#1167):
`a_rerun_from_the_retained_bytes_at_one_tip_rebuilds_the_rows_and_the_kernel_is_untouched`
(`crates/daemon/src/candidate_pass_tests.rs:1084`) publishes through the handler, checks that the Kernel retained
exactly the published texts, truncates, rescores the live descriptors' evidence
bytes at the same tip, and reproduces the pass's rows excluding
`created_at_ms`. That rebuild rescores one publication's descriptors in one
`score_publication` call, so it covers a table one publication wrote. Not
constructed: a table several publications built, which holds up to the
project cap (4096) while one call writes at most M = 64 rows; rebuilding it
takes one call per original publication in publish order, and the rows the
project cap evicted depend on that order.
Guarantee: For one Kernel snapshot, one scorer contract version V, no
budget exhaustion, and a rebuild that rescores each publication's descriptors
in publish order under the same M, truncating the table and rebuilding it
from the project's live descriptors yields the same set of (key, decision_id,
score, matched_atoms); this holds because the pass scores the evidence
object's exact bytes, so a rebuild can read the same bytes back.
Check: `always` - run a pass, snapshot the rows, truncate, run the rebuild at
the same `tip`, compare the sets excluding `created_at_ms`; the semantics is
`always` because the derivation is deterministic by construction.
Fault/timing angle: a decision revised between pass and rebuild (changes the
expected set; excluded by fixing the snapshot); a descriptor whose evidence
was retired (excluded from the rebuild input and swept from the table).
Required faults and enabling state: flag on; a pass that wrote rows; a
rebuild entry point (operator or test) that enumerates live descriptors of
the project and reads evidence bytes under a budget.
Confidence: medium - [evidence](evidence/cc-table-rebuildable-from-evidence-bytes-and-live-decisions.md);
the over-M rebuild is unconstructed.
Existing check: the store and pass tests above, unaudited.
Impact: without this the table is a log, not a derivation, and the derived
classification (index D6) is false.
Open questions:
- Resolved by #1167 (OQ-C): the pass carries the job's text, which the
  rebuild test shows equals the retained evidence bytes.
- Resolved by #1164 and #1166: V is the row's `matcher_digest`; the sweep
  drops a row whose digest differs from the current one.

## S-E sweep

### cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched

Type: safety
Reachability: explicit-config-only - the daemon starts the MemoryReviewer
cleanup loop whenever the memory store opens
(`crates/daemon/src/lib.rs:4006`), and each pass runs the sweep through
`pass_then_sweep` (`crates/daemon/src/memory_reviewer/lifecycle.rs:448`),
which returns before any table statement unless the user tier sets
`contradiction_candidates.enabled`.
Status: active
Exercised: partial - #1166, `crates/daemon/tests/contradiction_candidates_sweep.rs`:
`one_sweep_drops_every_dead_row_keeps_the_eligible_ones_and_a_repeat_drops_none`
(`:433`) seeds rows for decisions in every disposition, a decision of another
project, another domain, a `Secret` decision, a retired descriptor, and one
row each for a foreign incarnation, a stale matcher digest, and an over-age
row, each on live evidence and an eligible decision so it has one reason to
drop; one sweep keeps exactly the active, sensitive, stale, and disputed rows
of the bound project and the other project's own row, a repeat drops zero,
and the Kernel tip and the reviewer-job facts are unchanged;
`a_closed_kernel_drops_nothing_and_counts_the_skip` (`:502`) runs with no
Kernel; `a_failed_claim_read_keeps_its_rows_and_the_kernel_free_drops_still_land`
(`:765`) refuses one claim chunk and observes its live rows kept, the
over-age and retired-evidence rows dropped, and one skip counted;
`the_watermark_covers_three_pages_in_three_runs_and_the_fourth_starts_over`
(`:517`) bounds each run to one page;
`a_row_written_after_the_sweep_tip_stays_until_a_later_sweep_judges_it`
(`:994`), `a_cancelled_sweep_deletes_nothing_and_reads_no_kernel_row` (`:1018`),
`every_claim_chunk_of_a_page_is_read` (`:1047`), and
`a_row_at_the_age_bound_stays_and_one_past_it_drops` (`:486`) cover the
remaining branches. `a_busy_reader_does_not_hold_the_sweep_past_its_budget`
(`:1203`) holds one Kernel reader while the other is free and observes the
sweep return inside its budget with both rows judged: each scope-terms read
goes through `KernelStore::scope_terms_within_budget`
(`crates/kernel/src/scope.rs:1617`), which polls the whole reader pool and
stops at the sweep's deadline. The sweep is `CandidateSweep::run`
(`crates/daemon/src/contradiction_candidates/sweep.rs:69`); it reads the page
before it captures the tip, reads each chunk of decisions in ascending id
order through `KernelStore::claim_standing_at_within_budget`
(`crates/kernel/src/claim_facts.rs:270`), which returns the registry row,
decision row, own admission row, and served standing that claim facts
carry, and deletes through
`MemoryStore::delete_candidates_as_read`, so a row written above the tip or
rewritten after the read stays. Retirement observability (G4): the sweep
judges a descriptor by its evidence row through the new budgeted
`KernelStore::evidence_liveness_at_within_budget`
(`crates/kernel/src/evidence_liveness.rs:50`), which answers `Live`,
`Retired`, or `Unknown` at the target; `Unknown` drops the row because a row
at or below the tip names evidence the tip should know. Not constructed: a
concurrent pass, a failed tip capture, evidence read, or scope-terms read,
and the budget expiry.
Guarantee: One sweep call examines at most one page of rows. Among the rows
written at or below the sweep's Kernel tip it drops exactly those with a
stale matcher digest, an age past `MAX_CANDIDATE_AGE`, a foreign Kernel
incarnation, a decision with no registry row in its claim standing or failing
`eligible` at the tip, or descriptor evidence that is `Retired` or `Unknown`
at the tip; every other row is kept. A row written above the tip is spared
the Kernel judgments (standing, eligibility, evidence); the stale-digest
and over-age drops need no Kernel and apply to it as to any row, and the
foreign-incarnation drop applies once the tip capture has answered the
database identity. A restore between the tip capture and a Kernel read
keeps every row the Kernel would have judged and counts one skip. A repeat over the same page at the same tip drops zero, and a cycle of
`n` runs over an `n`-page table reaches a fixed point: the next cycle at the
same tip drops zero. The sweep writes no Kernel row.
With no Kernel the sweep reads nothing, drops nothing, and counts one skip. A
Kernel read that fails keeps the rows it would have judged and counts one
skip; the drops that need no Kernel read still land.
Check: `always` - seed one row per drop reason, each otherwise eligible on
live evidence, plus rows for active, sensitive, stale, and disputed
decisions; one sweep keeps exactly the eligible rows; a second sweep drops
zero; the Kernel tip and the reviewer-job facts are equal before and after;
with no Kernel the table is unchanged and one skip is counted; with a
refused claim chunk the chunk's rows stay and the Kernel-free drops land.
`always` because every sweep run must satisfy the predicate.
Fault/timing angle: a pass writing a row between the sweep's page read and
its delete (a row above the tip or rewritten after the read stays); the
Kernel retention sweep retiring evidence concurrently; more rows than one
page; a failed tip capture, evidence read, or scope-terms read; the 2 s
sweep budget expiring.
Required faults and enabling state: flag on; rows for each drop reason and
each kept disposition; a refused claim chunk; a page overflow; a row above
the tip. The failed tip capture, evidence read, and scope-terms read and the
budget expiry need a Kernel that fails on demand and are not constructed.
Confidence: medium - [evidence](evidence/cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched.md);
the sweep's concurrency with the pass (#1167) is reasoned, not yet exercised
Existing check: the tests above (#1166), unaudited. Bounded reads recorded
here (G5, G6): the harness sweep retires at most
`MAX_EXPIRED_CAPTURES_PER_CALL` = 64 captures per call
(`crates/kernel/src/local_file.rs:26`), and a call that retired or retained
any capture sets `advanced`
(`crates/daemon/src/memory_reviewer/lifecycle.rs:329`), so the next pass
follows at the 5 s retry interval; a backlog of `n` expired captures drains
in `ceil(n / 64)` passes while each pass advances (G5). The harness
candidate query `expired_captures_sql`
(`crates/kernel/src/local_file.rs:549`) walks every live expired row of the
class, including evidence a citation retains; a confirmed candidate's
dispute adds one such row for as long as its decision is live, so the scan
grows with confirmed candidates, not with candidate rows, and the candidate
sweep adds no pin or citation of its own (G6).
Impact: over-dropping loses candidates for live decisions; under-dropping
serves rows for dead decisions; a Kernel write would violate R5.
Open questions:
- Resolved by #1166: `stale` and `disputed` rows are kept (OQ5-4 default);
  the page is `CANDIDATE_SWEEP_PAGE` = 256 and the sweep runs once per
  MemoryReviewer cleanup pass (OQ5-9).
- Gap (liveness, queued): a table of `n` pages is covered in `n` enabled
  runs, but candidate drops do not mark the pass as advanced, so an idle
  loop covers one page per 30 s; no liveness record states this bound.
- Gap (needs human input): when the flag turns off, held rows stay until a
  later re-enable or the write-time caps reclaim them, and `retained_bytes`
  keeps its last value.

## S-F read surfaces

### cc-read-serves-only-live-decision-rows-ids-and-scores-only

Type: safety
Reachability: explicit-config-only - the read answers rows only for a route
whose frozen `contradiction_candidates.enabled` is true
(`crates/daemon/src/contradiction_candidates/read.rs:225`).
Status: active
Exercised: yes - #1168,
`review_candidates_pages_live_rows_and_answers_each_terminal`
(`crates/daemon/src/candidate_pass_tests.rs:1849`) writes 70 live rows and
four stale rows for one decision through the store and reads them back through
`Handler` dispatch: the first item's key set is exactly the eight wire names
(`:1963`), a limit of 65 clamps to 64 rows listed or filtered (`:1980`) and a
limit of 0 to one (`:2046`), the two-page walk follows (score desc, created
desc, evidence asc) and covers the 70 live rows once each (`:2025`, `:2034`),
the retired-evidence, foreign-incarnation, stale-digest, and future-tip rows
are left out and counted `read_filtered_stale` = 4 (`:2005`), a superseded
decision with rows present answers `not_live` (`:2109`), an unknown decision
answers `not_live` (`:2125`), and a foreign root answers `project_mismatch`
(`:2150`), also with a bad body (`:2159`). The tip and the reviewer job facts
are unchanged after the first read (`:1952`, `:1953`), and the table's rows
are unchanged after the walk (`:2007`). Not constructed: the
other `eligible` refusals (another domain, a foreign scope, hidden, or a
contradicted, rejected, or quarantined disposition) through the read; the
eligibility predicate's own test covers them
(`crates/daemon/tests/contradiction_candidates_sweep.rs:378`).
Guarantee: `review.candidates` and the `candidates` action answer only for the
route's bound project; the handler judges the decision with the shared
eligibility predicate at the reader's identified tip
(`crates/daemon/src/contradiction_candidates/read.rs:132` `decision_is_live`, over the Kernel's claim standing read)
and answers `not_live` with no rows when it fails; otherwise it answers a page
of at most 64 rows in (score desc, created desc, evidence id asc) order,
`as_of` the tip, whose items carry exactly `evidence_id`,
`decision_object_id`, `decision_id`, `matched_terms`, `decision_terms`,
`score`, `publish_tip`, and `created_at_ms`
(`crates/daemon/src/contradiction_candidates/read.rs:102-110` `ItemOut`): no descriptor text,
summary, or term. A row is listed only when its evidence is live at `as_of`,
its Kernel incarnation and matcher digest equal the reader's, and its
`publish_tip` is at or below `as_of`
(`crates/daemon/src/contradiction_candidates/read.rs:192` `is_current`); every
other row on the page is counted `read_filtered_stale`
(`crates/daemon/src/contradiction_candidates/read.rs:311`). The TypeScript
parser refuses an item with any other key set
(`packages/opencode-plugin/src/shared/kernel-client/wire.ts:673`).
Check: `always` - every answered page satisfies the key, order, bound, and
filter predicates above, and every ineligible decision answers `not_live`; the
semantics is `always` because every response must satisfy it.
Fault/timing angle: the decision superseded between the pass and the read
(answered `not_live`); evidence retired between the pass and the read
(filtered); a row written for a tip past the reader's target (filtered).
Required faults and enabling state: flag on; MODULE memories authority; rows
for an active and a superseded decision; more rows than the page bound;
retired evidence; a second incarnation id and matcher digest; a second project
root.
Confidence: high - [evidence](evidence/cc-read-serves-only-live-decision-rows-ids-and-scores-only.md)
Existing check: the test above (#1168) and the store paging test
`a_decision_pages_in_reader_order_with_a_stable_cursor`
(`crates/memory-store/tests/contradiction_candidates.rs:246`), unaudited.
Impact: rows for a dead decision invite a dispute the Kernel will refuse; text
in the answer would be a disclosure path for Sensitive harness capture to the
model.
Open questions:
- Resolved by #1168: the terminal is `not_live` and the method literal is
  `review.candidates` (`docs/host-wire-protocol.md:833`); the item carries
  `matched_terms` and `decision_terms` as counts, and no term list.
- G7, sensitivity of descriptor-derived rows: the egress half stays open
  (needs human input). #1165 fixes the facts: a row holds ids, integers, and
  times only (the table's columns are pinned by
  `the_table_holds_ids_integers_and_times_and_checks_its_shapes`,
  `crates/memory-store/tests/contradiction_candidates.rs:410`), so no captured
  text is stored, and the store has no sensitivity column. #1168 fixes what
  leaves the daemon: the read answers the closed key set above to the calling
  harness, and the `candidates` action returns it to the model as a tool
  result, so the evidence id, the decision id, the score, the term counts,
  and the times of a descriptor that published as `Sensitive` with
  `ProviderEgress::LocalOnly` (`crates/daemon/src/source_capture.rs:921-922`) are
  provider-visible. No repository policy states whether metadata derived from
  such a descriptor inherits its class, and the read consults neither
  classification; whether that disclosure is admitted is the open policy
  decision.

### cc-read-distinguishes-disabled-unavailable-and-empty

Type: safety
Reachability: default-production - the `disabled` branch answers at the flag's
default, and the other branches need the flag on.
Status: active
Exercised: yes - #1168: a root without MODULE memories authority answers
`disabled` (`crates/daemon/src/candidate_pass_tests.rs:1867`); a flag-off
route answers `disabled` while the table is renamed away, and the same route
with the flag on answers `store_unavailable`
(`crates/daemon/src/candidate_pass_tests.rs:2191`, `:2214`); an eligible
decision with no rows answers
`{"kind":"page","as_of":tip,"items":[],"next":null}` (`:2117`).
`review_candidates_answers_the_store_and_kernel_states_before_any_page`
(`:2562`) answers the kernel route's `unavailable`/`store_starting` state for
a Kernel not yet open (`:2577`), `unavailable`/`store_unavailable` for a
Kernel whose open failed (`:2594`), and `store_unavailable` for a daemon with
no Memory Store (`:2625`), each without `items`. The OpenCode tool renders
each terminal by name and `disabled` as a plain statement
(`packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1666`). Not
constructed: a failing authority lookup (dependency fault F18 has no seam).
Guarantee: after the scope, Kernel-readiness, and body checks, a route whose
frozen flag is false, or a root whose memories authority is not MODULE,
answers `disabled` (`crates/daemon/src/contradiction_candidates/read.rs:225`,
`:247` through `module_authority`, `:182-190`), and the page read judges the
route's authority again in the connection that reads the page, so an
authority that drains or moves after the entry check answers `disabled`
(`:274`, `:288`; `crates/daemon/src/candidate_pass_tests.rs:2342`), and a
decision that is not live answers after the authority is judged once more, so
a drain during the judgment answers `disabled` before `not_live` (`:264-265`;
`crates/daemon/src/candidate_pass_tests.rs:2383`); an uninstalled Memory
Store, or one that fails the authority lookup or the page read, answers
`store_unavailable` (`crates/daemon/src/contradiction_candidates/read.rs:229`,
`:188`, `:285`);
the read's budget runs from its admission, so a read queued behind a saturated
blocking pool past its budget answers `store_unavailable` as soon as it runs
(`:240`; `crates/daemon/src/candidate_pass_tests.rs:2448`); an unready Kernel
or a failed Kernel read answers the kernel routes' state; an eligible decision
with no rows answers an empty page; no terminal or state renders as a page.
Check: `always` - the four conditions answer four distinct shapes; the
semantics is `always` because every response must be one of them.
Fault/timing angle: Kernel starting or failed at read; Memory Store absent or
its table unreadable; the flag toggled between route bind and read (the frozen
snapshot decides).
Required faults and enabling state: a route with the flag off; a route with
the flag on and the table renamed away; a daemon with no Memory Store; a
Kernel not yet open and one whose open failed; an eligible decision with zero
rows.
Confidence: high - [evidence](evidence/cc-read-distinguishes-disabled-unavailable-and-empty.md)
Existing check: the tests above (#1168), unaudited.
Impact: an agent reading an empty page as "no contradiction" when the store
was unavailable.
Open questions: None.

## S-G reachability

### cc-flag-off-no-pass-no-write-no-table-read

Type: safety
Reachability: default-production (the flag's default is off; this record
describes the default build's behavior)
Status: active
Exercised: yes - sweep and status arm (#1166):
`a_false_flag_runs_no_table_statement_and_moves_no_counter`
(`crates/daemon/tests/contradiction_candidates_sweep.rs:553`) hides the table,
runs the sweep under a false flag with and without a Kernel, and observes
every counter at zero, then shows the same table fails the first statement an
enabled sweep runs;
`the_cleanup_pass_carries_the_watermark_and_reads_the_flag_each_pass` (`:1092`)
drives `pass_then_sweep`
(`crates/daemon/src/memory_reviewer/lifecycle.rs:448`) with the flag off and
then on. The key is `/contradiction_candidates/enabled`
(`crates/daemon/src/config.rs:1024`), user-only and default false
(`contradiction_candidates_default_off_and_only_the_user_tier_enables_them`,
`config.rs:2299`); a configuration resolved before a user-tier rewrite keeps
its value (`config.rs:2320`), and a bound route keeps the value it was bound
with
(`a_route_keeps_the_candidate_flag_it_was_bound_with_and_health_reports_the_block`,
`crates/daemon/src/lib.rs:21502`), which also shows the status block carrying
the key with every counter zero. Pass arm (#1167):
`a_route_without_consent_offers_no_pass_and_writes_no_row`
(`crates/daemon/src/candidate_pass_tests.rs:253`) sets the offer's consent off
directly, publishes three windows with source capture on, and observes every
counter at zero and no row;
`the_route_consent_reaches_the_pass_through_a_transform` (`:1167`) binds the
route against the opposite daemon value and shows the offer follows the
route's frozen value; #1168 adds
`a_publication_without_consent_runs_no_candidate_table_statement` (`:2788`),
which renames the table away, publishes three consent-off windows under the
store's statement ledger (`start_statement_work_ledger`, every statement the
store's connection runs) with every counter at zero and no recorded statement
naming a candidate table (`:2808-2827`), shows one consenting publication's
pass fails on the hidden table (`store_unavailable` = 1), and, with the table
back, shows a consenting publication's pass runs statements the ledger
records on it (`:2858-2868`). Read arm (#1168):
`review_candidates_pages_live_rows_and_answers_each_terminal`
(`crates/daemon/src/candidate_pass_tests.rs:1849`, assertions `:2187-2214`)
renames the table away, answers `disabled` on a flag-off route with
`read_filtered_stale` unchanged and no recorded statement naming a candidate
table (`:2187-2207`), shows the same route with the flag on answers
`store_unavailable` (`:2214`), and, with the table back, shows a flag-on read
runs a statement the ledger records on it (`:2228-2237`); the handler answers
`disabled` before it reaches the store
(`crates/daemon/src/contradiction_candidates/read.rs:225`). The `candidates`
action renders the terminal as a plain statement in both harnesses
(`packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1666`,
`packages/pi-plugin/src/tools/eidnara-memory.test.ts:532`).
Guarantee: With the route's frozen `contradiction_candidates.enabled` false
(default), no candidate pass is offered after any publication, no row is
written, and neither read surface opens the table; the flag is enabled only by
explicit configuration at the `/contradiction_candidates/enabled` path and is
frozen in the route's consent snapshot like `/source_capture/enabled`
(`config.rs:105,158,1024`). The sweep is daemon-global: it reads the
daemon-level value again on every cleanup pass
(`crates/daemon/src/memory_reviewer/lifecycle.rs:458`) and, while that value
is false, runs no table statement and moves no counter.
Check: `always(!X)` - X is "a pass counter increments, a row is written, or
a table read statement executes for a route whose frozen flag is false"; a
row written while the flag was on and retained after it turned off is a legal
state, so the predicate names new writes, not held rows; this is a forbidden
state with no dedicated detection point, so `always(!X)` rather than
`unreachable`; assert by publishing with the flag off and checking counters
and the row count against its value before the publication, with the table
renamed away, beside a flag-on control that fails on it.
Fault/timing angle: a config reload that turns the flag on after the route
bound (the route keeps false, while the sweep follows the reload on its next
pass); a daemon restart with the flag on (new routes see true).
Required faults and enabling state: a default config; publications with source
capture on; a `candidates` call and an owner read under the default.
Confidence: high - [evidence](evidence/cc-flag-off-no-pass-no-write-no-table-read.md); the pass
and read arms record every store statement under the flag-off run and find
none naming a candidate table, beside a flag-on control the ledger does
record; the sweep arm hides the table and pairs the flag-off run with a
flag-on control
Existing check: the sweep-arm, pass-arm, and read-arm tests named under
Exercised, unaudited.
Impact: an unmeasured matcher reaches users (KTD4) and a surprise write load
appears in deployments that did not opt in.
Open questions:
- OQ-E resolved by #1168 (OQ5-6, option a): the `candidates` action is in both
  plugins' schemas whatever the flag, and answers `disabled` when the route's
  frozen flag is off; the A1 golden
  (`packages/opencode-plugin/src/shared/prompt-surface-a1-golden.md`) carries
  the schema change.

## S-H confirm linkage

### cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm

Type: safety
Reachability: explicit-config-only - as
`cc-read-serves-only-live-decision-rows-ids-and-scores-only`.
Status: active
Exercised: yes - #1168,
`a_listed_candidate_confirms_through_dispute_while_its_evidence_lives`
(`crates/daemon/src/candidate_pass_tests.rs:2630`) publishes two windows
through the capture job and the pass, lists each decision's row, checks the
listed `evidence_id` of each decision equals the evidence id of the
descriptor of the window whose text overlaps it (`:2695-2704`),
truncates the candidate table, and files
`dispute {object_id, event: mark_disputed, evidence_id}` built from the row
through `kernel.commit`, which lands `disputed` (`:2747`). After
`expire_harness_captures` retires the evidence, the other decision's read
leaves the row out and counts it (`:2756`), and the same dispute built from
the earlier listing is refused `not_found` (`:2766`).
The dispute runs under the store's statement ledger, which records no
statement naming a candidate table (`:2725-2732`), beside the listings, which
the ledger records on the table (`:2668-2680`).
`the_confirm_path_never_reads_the_candidate_table` (`:2770`) is a lexical
check that `kernel_routes/commit.rs` and `kernel_routes/dispute.rs` name no
candidate store method. The OpenCode tool renders each row's confirm form
`{"action":"dispute","event":"mark_disputed","objectId":"<decision_object_id>","evidenceId":"<evidence_id>"}`
from the row's ids
(`packages/opencode-plugin/src/tools/eidnara-memory/tools.test.ts:1627`). The
form's fields match `dispute`'s arguments by reading
`packages/opencode-plugin/src/tools/eidnara-memory/dispute.ts`; no test feeds
a rendered form back into the tool.
Guarantee: a row's `evidence_id` is the `srcev:*` evidence row id the
publication receipt carries (`Published.evidence_id`,
`crates/daemon/src/harness_sources.rs:555`), and its `decision_object_id` is
the `object_id` P4 `dispute` takes, so a dispute copied from a row is well
formed; the read lists a row only while its evidence is live at the reader's
tip; a confirm whose evidence retired after the listing is refused `not_found`
by P4 at commit; the confirm path never reads the candidate table.
Check: `always` - for every listed row the evidence id names a published
descriptor's evidence row; a dispute built from a row with live evidence lands
`Disputed`; after the evidence retires the row leaves the read and the same
dispute is refused `not_found`; the semantics is `always` because every row
and every confirm must satisfy it.
Fault/timing angle: the `harness_capture` 30-day retention sweep retiring the
evidence between list and confirm; a session delete retiring the descriptor.
Required faults and enabling state: flag on; MODULE authority; P4 `dispute`
landed; rows written by a real pass; evidence retired by
`expire_harness_captures` between the listing and the confirm.
Confidence: high - [evidence](evidence/cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm.md);
the confirm runs under the store's statement ledger with the listings as its
control, and the lexical check names the two route files
Existing check: the tests above (#1168), unaudited; P4's
`cited-evidence-live-at-dispute-commit` owns the confirm-side refusal.
Impact: a row carrying the descriptor `object_id` instead of the evidence id
makes every confirm fail; a confirm that read the table would make the derived
table an authority.
Open questions:
- Resolved by #1168 (ticket scope, C14): the read checks evidence liveness at
  the reader's tip and leaves retired rows out, counted `read_filtered_stale`;
  a confirm built from an earlier listing still meets P4's refusal.

## Cross-cutting R5

### cc-pass-and-sweep-write-no-kernel-event-and-no-signal

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - sweep arm (#1166): every enabled sweep in
`crates/daemon/tests/contradiction_candidates_sweep.rs` runs through
`Fixture::sweep` (`:291`), which compares the Kernel tip and the
MemoryReviewer job facts before and after the run;
`the_candidate_modules_name_no_kernel_write_path` (`:695`) is a lexical proxy
that finds no Kernel write entry point, `CommitIntent`, or `CausalInputs` in
the files of `crates/daemon/src/contradiction_candidates/` and does not
follow calls out of the module; and
`every_reviewer_job_the_daemon_builds_carries_no_signal` (`:719`) scans every
`.rs` file under `crates/daemon/src` for `CausalInputs {` literals and
requires `signals: Vec::new()` in each, so a construction through a helper
or struct update would escape it. Pass arm (#1167): every direct pass in
`crates/daemon/src/candidate_pass_tests.rs` runs through `Core::score`, which compares the Kernel tip and the job
facts before and after; the handler rebuild test (`:1084`) does the same
around a parked pass; the lexical module check covers `pass.rs`, and the
signal scan covers the pass's files.
Guarantee: Across any pass and any sweep the Kernel commit log, admission
rows, and object registry are unchanged, no decision's disposition or
maturity changes, no MemoryReviewer job is reserved, and every
`CausalInputs.signals` constructed in the daemon remains empty
(`crates/memory-store/src/memory_reviewer_jobs.rs:79-82`; production
producers at `selection.rs:306` and `handoff.rs:367`).
Check: `always(!X)` - X is "a Kernel write, a reviewer reservation, or a
non-empty `signals` attributable to the candidate feature"; a forbidden state
with no dedicated detection point; assert by comparing commit-log and
reviewer-job row counts before and after a pass and a sweep, and by a
source-level check that no code path from the candidate module reaches a
Kernel write family or a `CausalInputs` constructor with a non-empty vector.
Fault/timing angle: none; the obligation is structural.
Required faults and enabling state: flag on; a pass that writes rows; a sweep
that drops rows; a candidate whose score would tempt an automatic dispute.
Confidence: medium - [evidence](evidence/cc-pass-and-sweep-write-no-kernel-event-and-no-signal.md);
the source checks are lexical
Existing check: the sweep and pass tests named under Exercised, unaudited.
Impact: the plan's three stop conditions; a signal would reserve a
receipt-charged reviewer job against the Q38 ceiling (2,048 per project per
incarnation, `docs/memory-reviewer-operations.md:263`).
Open questions: None.
