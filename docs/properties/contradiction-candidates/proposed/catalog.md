# Proposed obligation catalog: P5 write-time contradiction candidates

Part of `docs/properties/contradiction-candidates/`. Companion part:
`../dependencies/` (discovered facts about implemented surfaces).

These records were authored at `3a3b916b7` as proposed design obligations
for unbuilt behavior. #1164 builds the scorer
(`daemon::contradiction_candidates::scorer`,
`crates/daemon/src/contradiction_candidates/scorer.rs`) and converts the
three S-A records to `Status: active` with `Exercised: yes`; their tests are
in `crates/daemon/tests/contradiction_candidates_scorer.rs`, and their
references are verified at the #1164 branch tip. The other 12 records keep
`Status: active - PROPOSED`: each is a specification input and a test target
for the unit that builds its behavior, and no claim is made that the
implementation exists. The store (#1165), sweep (#1166), pass (#1167), and
read (#1168) tickets make them checkable. #1166 builds the sweep and
converts the sweep record to `Status: active`. #1167 builds the pass and
converts the trigger, single-flight, candidate-set, partial-result, row-key,
rebuild, and no-Kernel-write records; the flag-off record keeps its read arm
for #1168. Source: the
`/software-design:invariant-driven-domain-modeling` pass whose certificate is
`certificate.md`; its open items are listed in `open-questions.md`. The
proposed records below are reproduced from that pass unchanged in
substance.

Reachability labels on proposed records name the intended class once the
unit lands, per the preamble reproduced below, and are re-verified at
implementation time. The three active records carry the class verified at
the branch tip, `test-only`, because nothing in the daemon calls the scorer.

Companion files: [existing-checks.md](existing-checks.md),
[fault-map.md](fault-map.md),
[portfolio-evaluation.md](portfolio-evaluation.md), `evidence/<slug>.md`
for the active records, [certificate.md](certificate.md),
[open-questions.md](open-questions.md).

## Index

| Slug | Surface | Type | Semantics | Reachability | Status |
| --- | --- | --- | --- | --- | --- |
| cc-scorer-output-deterministic-under-contract-version | S-A | safety | always | test-only (intended explicit-config-only) | active, exercised yes |
| cc-oversize-text-refused-whole-never-truncated | S-A | safety | always | test-only (intended explicit-config-only) | active, exercised partial (scorer arm) |
| cc-case-fold-applied-identically-to-both-sides | S-A | safety | always | test-only (intended explicit-config-only) | active, exercised yes |
| cc-pass-starts-only-after-publication-job-terminal | S-B | safety | always | explicit-config-only | active, exercised yes (#1167) |
| cc-passes-in-flight-bounded-and-overflow-counted | S-B | safety | always | explicit-config-only | active, exercised yes (#1167) |
| cc-candidate-set-bounded-with-counted-truncation | S-C | safety | always | explicit-config-only | active, exercised partial (#1167) |
| cc-partial-result-is-descriptor-granular-prefix | S-C | safety | always | explicit-config-only | active, exercised yes (#1167) |
| cc-row-key-unique-and-caps-enforced | S-D | safety | always | explicit-config-only | active, exercised yes (#1165, #1167) |
| cc-table-rebuildable-from-evidence-bytes-and-live-decisions | S-D | safety | always | explicit-config-only | active, exercised yes (#1165, #1167) |
| cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched | S-E | safety | always | explicit-config-only | active, exercised partial (#1166) |
| cc-read-serves-only-live-decision-rows-ids-and-scores-only | S-F | safety | always | intended explicit-config-only | proposed (#1168) |
| cc-read-distinguishes-disabled-unavailable-and-empty | S-F | safety | always | intended default-production | proposed (#1168) |
| cc-flag-off-no-pass-no-write-no-table-read | S-G | safety | always(!X) | intended default-production | proposed, exercised partial (sweep #1166, pass #1167; reads #1168) |
| cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm | S-H | safety | always | intended explicit-config-only | proposed (#1168) |
| cc-pass-and-sweep-write-no-kernel-event-and-no-signal | R5 | safety | always(!X) | explicit-config-only | active, exercised partial (#1166, #1167) |

15 records: 3 active, 12 proposed. Semantics distribution: 15 `always`
(13 plain and 2 written `always(!X)` for forbidden states), 0
`always-or-unreached`, 0 `sometimes`, 0 `reachable`, 0 `unreachable`; all 15
are safety. Reachability: 3 `test-only` at the branch tip; intended classes
13 `explicit-config-only`, 2 `default-production`. The ticket mapping in the
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
fixes (proposed: registry row live at the snapshot, `memory` domain, project
scope term accepted, disposition not in {`superseded`, `contradicted`,
`rejected`, `quarantined`}). "Scorer contract version V" names the constant
that embeds `ANALYSIS_CONTRACT_EPOCH` (`crates/retrieval/src/lexical/mod.rs:40`),
the fold function F, the formula, threshold, and K.

---

## S-A scorer

### cc-scorer-output-deterministic-under-contract-version

Type: safety
Reachability: test-only - the scorer
(`crates/daemon/src/contradiction_candidates/scorer.rs`) is the public module
`daemon::contradiction_candidates::scorer` (`crates/daemon/src/lib.rs:17`,
`crates/daemon/src/contradiction_candidates/mod.rs:4`) and nothing in the
daemon calls it; its only callers are the tests in
`crates/daemon/tests/contradiction_candidates_scorer.rs`. The intended class is
`explicit-config-only` once the pass (#1167) calls it behind
`contradiction_candidates.enabled`.
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
Read `crates/daemon/src/contradiction_candidates/scorer.rs:102-110` (terms are a
`BTreeSet`, so no
hash iteration order reaches the output), `:139-148` (`rank`, a total order
ending at both ids), `:217-250` (`record`, `offer`), `:271-288`
(`score_descriptor` sorts `unscorable`), `:292-327` (the preimage names the
order rule).
Existing check: the two tests above and `the_matcher_digest_pins_every_rule`
(`:348-400`), which pins the order rule id in the preimage. Status
unaudited.
Impact: a nondeterministic order defeats the rebuild obligation
(`cc-table-rebuildable-from-evidence-bytes-and-live-decisions`) and makes
the top K a function of SQL row order.
Open questions:
- No test runs the scorer on a second thread or process. The module holds
  no shared mutable state other than the `LazyLock` matcher digest
  (`:338-339`), so this pass judges a second-thread test low value; the
  rebuild record exercises a second run once the store lands (#1165).

### cc-oversize-text-refused-whole-never-truncated

Type: safety
Reachability: test-only - the scorer
(`crates/daemon/src/contradiction_candidates/scorer.rs`) is the public module
`daemon::contradiction_candidates::scorer` (`crates/daemon/src/lib.rs:17`,
`crates/daemon/src/contradiction_candidates/mod.rs:4`) and nothing in the
daemon calls it; its only callers are the tests in
`crates/daemon/tests/contradiction_candidates_scorer.rs`. The intended class is
`explicit-config-only` once the pass (#1167) calls it behind
`contradiction_candidates.enabled`.
Status: active
Exercised: partial - scorer arm only, and one Check clause unasserted.
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
result against a run without that summary. The pass arm, zero rows and a
counted `descriptors_refused_oversize` for a refused descriptor, lands with
#1167.
Guarantee: a descriptor over `DESCRIPTOR_BOUNDS` is refused whole:
`DescriptorTerms::new` and `score_descriptor` return the analyzer's
`LexicalRefusal` and no candidate; a summary over `SUMMARY_BOUNDS`, or with
no term left after the floor, is `SummaryScore::Unscorable` and its decision
object id is listed in `unscorable`, while the other summaries score;
`unscorable` lists decision object ids, not decision ids
(`crates/daemon/src/contradiction_candidates/scorer.rs:280`), so with a
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
(`crates/daemon/src/contradiction_candidates/scorer.rs:84-95`), one past the
most atoms the byte bound admits, because n atoms need at least 2n - 1
bytes; an input with `max_atoms` atoms exceeds the byte bound first, so
`TooManyAtoms` is unreachable through the scorer and no test constructs it.
Confidence: high - [evidence](evidence/cc-oversize-text-refused-whole-never-truncated.md).
Read `crates/daemon/src/contradiction_candidates/scorer.rs:63-69`, `:84-95`,
`:102-110`, `:175-207`,
`:271-288`; the compile-time assertion at `:65-67` holds the descriptor
bound at or above `MAX_SOURCE_UNIT_BYTES`
(`crates/daemon/src/harness_sources.rs:612`).
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
Reachability: test-only - the scorer
(`crates/daemon/src/contradiction_candidates/scorer.rs`) is the public module
`daemon::contradiction_candidates::scorer` (`crates/daemon/src/lib.rs:17`,
`crates/daemon/src/contradiction_candidates/mod.rs:4`) and nothing in the
daemon calls it; its only callers are the tests in
`crates/daemon/tests/contradiction_candidates_scorer.rs`. The intended class is
`explicit-config-only` once the pass (#1167) calls it behind
`contradiction_candidates.enabled`.
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
summary passes through the one function `term_set`; no diacritic removal or
Unicode normalization is applied. The analyzer splits parts on the original
case before the fold (`crates/retrieval/src/lexical/analysis.rs:193-205`:
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
Read `crates/daemon/src/contradiction_candidates/scorer.rs:102-110` (the one fold
site), `:175-181`
(`DescriptorTerms::new` calls `term_set`), `:182-207` (`score` calls
`term_set` on the summary), `:18-25` (the module header states the fold is
not full case folding), `:76` (`FOLD_RULE_ID` enters the preimage);
`crates/retrieval/src/lexical/analysis.rs:193-205` (`splits`, the
case-driven part boundaries).
Existing check: the three tests above; `the_matcher_digest_pins_every_rule`
(`:348-400`) pins the fold rule id. Status unaudited.
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
(`crates/daemon/src/lib.rs:9246`) offers a pass only when the route's frozen
`contradiction_candidates.enabled` and a memory store are both present.
Status: active
Exercised: yes - #1167, `crates/daemon/src/candidate_pass_tests.rs`:
`a_pass_starts_only_after_the_job_releases_the_slot_and_the_gate` (`:282`)
holds the `before_publish` hook and observes zero offers and zero rows, then
observes the pass start with the capture slot idle and the commit gate free
and score every descriptor with the gate free, one `completed` pass, and one
row; a memo-hit job that publishes nothing offers no pass; each job ends in
one terminal count; a replayed unit is scored again with its rows' tip at or
above the earlier one. `publish_latency_with_candidates_on_stays_within_the_off_baseline`
(`:1041`) commits every second publication beside a running pass and keeps
the median publication hold within three times the flag-off median plus
50 ms. The job hands its kept texts out through `CaptureJob::run_settled`
(`crates/daemon/src/source_capture.rs:675`) after the blocking closure has
dropped the slot permit, and the pass runs under its own `EvalBudget`
(`crates/daemon/src/contradiction_candidates/pass.rs:165`).
Guarantee: The candidate pass starts only after `CaptureJob::run`
(`source_capture.rs:653-676`) has produced its terminal `Count` and the
capture commit gate guard has been dropped (`source_capture.rs:800`); it runs
under an `EvalBudget` whose origin is the pass start and whose interrupt is the
daemon cancel, never the job's budget; the response bytes were handed to the
wire writer before the `AfterResponse` closure ran (`lib.rs:9219-9238`).
Check: `always` - with the `before_publish` and `between_screen_and_commit`
test hooks (`source_capture.rs:781, 839`) blocked, the candidate table has
zero rows and the pass counter `admitted` is zero; after the hooks release and
the job returns, the pass runs; the pass's budget object is not the job's; the
semantics is `always` because every publication must respect the ordering.
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
(`:398`) parks a pass, publishes again, and observes `refused_pass_busy` = 1
with `active` = 1 and no row for the refused publication; a cancel during
descriptor 1 of 3 ends the pass with `budget_exhausted`, the first
descriptor's rows, `descriptors_unscored` = 2, and `active` back at 0;
`a_panicking_pass_releases_its_permit_and_charge_and_shutdown_waits_for_a_parked_pass`
(`:983`) shows a panicking pass counted with its permit and charge released
and shutdown waiting for a parked pass. P = 1 per store through a
non-waiting permit (`crates/daemon/src/contradiction_candidates/pass.rs:112`).
Guarantee: At most P candidate passes exist at once (P a named constant,
proposed 1 per project binding); an offer that finds the bound full is refused
and counted, never queued without bound; daemon cancel ends every running
pass within its budget.
Check: `always` - after N rapid publications with the pass blocked on a test
hook, running passes are at most P and `refused_busy` equals N - P; the
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
`the_candidate_set_keeps_the_newest_eligible_decisions_of_the_project_and_counts_the_cut`
(`:560`) seeds N + 1 visible decisions with the cap at N = 4: the oldest is
cut, `candidates_truncated` = 1, the other project's decision is never read,
and stale and disputed decisions are scored while the contradicted one is
hidden by explicit search; `a_superseded_decision_is_never_scored_and_the_default_bounds_are_the_named_constants`
(`:603`) pins the production values (N = 4096) and shows a superseded
decision unread; `the_summary_byte_cut_skips_payloads_before_any_load`
(`:667`) cuts payloads before any load and counts their bytes. Not
constructed at the production N: the tests cut at a reduced cap. The
`eligible` filter in the pass (`crates/daemon/src/contradiction_candidates/pass.rs:363`) rejects nothing explicit search
serves at HEAD, so no test shows it as the reason a decision is dropped.
Guarantee: The pass's candidate set is built by the project scope term
prefilter (`admission.rs:3138-3144`, `project.rs:106-111`) and the
`ScopeFilter` judge, then cut to at most N decisions in a deterministic
newest-first order with a counted truncation when more were eligible; summary
bytes are bounded before any payload is loaded
(`decision_payload_sizes_as_of`, `slice/read.rs:330`, or
`decision_page_within_budget`'s `max_payload_bytes`, `:126-133`); the pass
never loads more than N summaries or more than the byte bound.
Check: `always` - with N + 1 eligible decisions the pass scores exactly N, the
oldest is the one omitted, and `candidate_set_truncated` increments by one; a
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
`a_deadline_during_the_third_descriptor_leaves_the_first_two` (`:727`)
expires the budget during descriptor 3 of 5 and observes four rows for each
of descriptors 1 and 2, none for 3 to 5, `descriptors_unscored` = 3, and
`deadline` = 1; `a_budget_spent_before_the_pass_writes_nothing_and_counts_every_descriptor`
(`:759`) and the cancel case of
`a_second_offer_while_a_pass_runs_is_refused_and_a_cancel_still_ends_the_pass`
(`:398`) cover the empty prefix and a cancel between descriptors. The pass
writes each descriptor's rows in one store write after its full candidate
set is scored (`crates/daemon/src/contradiction_candidates/pass.rs:243`).
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
two descriptor writes.
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
`each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip` (`:72`) shows one
row per key, a lower-tip rewrite that keeps the higher tip and replaces the
other values, and a newer write that removes keys it no longer names;
`a_late_write_replaces_held_keys_and_adds_none` (`:454`) shows a write below
the held tip adds no key, so a descriptor never exceeds K;
`a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole` (`:114`)
and `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` (`:560`)
show K accepted, K + 1 refused with the held rows unchanged, and a project
at `per_project` evicting the lowest `score_bp`, then oldest, then smallest
key with the count returned; `a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest`
(`:187`) shows another project's rows are untouched. The store
(`crates/memory-store/src/contradiction_candidates.rs:230`) enforces K and the
project cap in the write transaction; the daemon owns the values. Pass arm
(#1167): `the_row_caps_hold_per_descriptor_per_pass_and_per_project_and_a_replay_keeps_the_rows`
(`crates/daemon/src/candidate_pass_tests.rs:779`) scores K + 1 overlaps into K rows, cuts a pass at M in publish
order with `rows_written` = M and the rest counted `dropped_at_cap`, keeps a
project at its cap with the eviction counted, rescoring at a newer tip keeps
each key with a higher `publish_tip`, and a descriptor past M keeps the rows
it already holds; the handler test at `:282` republishes a replayed unit
through the job and keeps the same keys. The over-M count is
`dropped_at_cap`; the C13 vocabulary has no `pass_cap_reached`.
Guarantee: The table key is (project digest, descriptor evidence object id,
decision object id); a write for an existing key replaces `decision_id`,
`score`, `matched_atoms`, `publish_tip`, `created_at_ms`; one pass writes at
most K rows per evidence id and at most M rows in total, stopping at M with a
count; the row carries no column that any command reads for admission.
Check: `always` - after any sequence of passes, `SELECT COUNT(*) GROUP BY
key` is 1 for every key; after one pass with a descriptor matching K + 1
decisions the evidence id has K rows; after one pass whose descriptors would
match more than M, exactly M rows are written and `pass_cap_reached`
increments; the semantics is `always` because the predicate holds at every
pass commit.
Fault/timing angle: a replayed publication (`Published.replayed`,
`harness_sources.rs:530`) re-scoring the same evidence id; two passes for one
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
Exercised: yes - store arm (#1165):
`truncating_and_replaying_the_writes_rebuilds_the_same_rows`
(`crates/memory-store/tests/contradiction_candidates.rs:376`) truncates through the test-support
`truncate_contradiction_candidates_for_test` and replays a write sequence,
reproducing the same rows excluding `created_at_ms`. Pass arm (#1167):
`a_rerun_from_the_retained_bytes_at_one_tip_rebuilds_the_rows_and_the_kernel_is_untouched`
(`crates/daemon/src/candidate_pass_tests.rs:850`) publishes through the handler, checks that the Kernel retained
exactly the published texts, truncates, rescores the live descriptors' evidence
bytes at the same tip, and reproduces the pass's rows excluding
`created_at_ms`.
Guarantee: For one Kernel snapshot, one scorer contract version V, and no
budget exhaustion, truncating the table and rebuilding it from the project's
live descriptors yields the same set of (key, decision_id, score,
matched_atoms); this holds because the pass scores the evidence object's
exact bytes, so a rebuild can read the same bytes back.
Check: `always` - run a pass, snapshot the rows, truncate, run the rebuild at
the same `tip`, compare the sets excluding `created_at_ms`; the semantics is
`always` because the derivation is deterministic by construction.
Fault/timing angle: a decision revised between pass and rebuild (changes the
expected set; excluded by fixing the snapshot); a descriptor whose evidence
was retired (excluded from the rebuild input and swept from the table).
Required faults and enabling state: flag on; a pass that wrote rows; a
rebuild entry point (operator or test) that enumerates live descriptors of
the project and reads evidence bytes under a budget.
Confidence: high - [evidence](evidence/cc-table-rebuildable-from-evidence-bytes-and-live-decisions.md)
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
(`:432`) seeds rows for decisions in every disposition, a decision of another
project, another domain, a `Secret` decision, a retired descriptor, and one
row each for a foreign incarnation, a stale matcher digest, and an over-age
row, each on live evidence and an eligible decision so it has one reason to
drop; one sweep keeps exactly the active, sensitive, stale, and disputed rows
of the bound project and the other project's own row, a repeat drops zero,
and the Kernel tip and the reviewer-job facts are unchanged;
`a_closed_kernel_drops_nothing_and_counts_the_skip` (`:501`) runs with no
Kernel; `a_failed_claim_read_keeps_its_rows_and_the_kernel_free_drops_still_land`
(`:765`) refuses one claim chunk and observes its live rows kept, the
over-age and retired-evidence rows dropped, and one skip counted;
`the_watermark_covers_three_pages_in_three_runs_and_the_fourth_starts_over`
(`:516`) bounds each run to one page;
`a_row_written_after_the_sweep_tip_stays_until_a_later_sweep_judges_it`
(`:816`), `a_cancelled_sweep_deletes_nothing` (`:842`),
`every_claim_chunk_of_a_page_is_read` (`:855`), and
`a_row_at_the_age_bound_stays_and_one_past_it_drops` (`:485`) cover the
remaining branches. The sweep is `CandidateSweep::run`
(`crates/daemon/src/contradiction_candidates/sweep.rs:71`); it reads the page
before it captures the tip and deletes through
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
incarnation, a decision missing from claim facts or failing `eligible` at the
tip, or descriptor evidence that is `Retired` or `Unknown` at the tip; every
other row is kept, and so is every row written above the tip; a repeat at the
same tip drops zero; the sweep writes no Kernel row. With no Kernel the sweep
reads nothing, drops nothing, and counts one skip. A Kernel read that fails
keeps the rows it would have judged and counts one skip; the drops that need
no Kernel read still land.
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
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U4 read route and tool action do not exist
Guarantee: The owner read and the `candidates` action answer only for the
route's bound project; they check the decision's standing in the Kernel at
read time and answer a terminal (proposed `not_live`) with no rows when the
decision is not eligible; otherwise they answer a bounded page of rows
ordered (score desc, created desc, evidence id asc) whose fields are
`evidence_id`, `decision_id`, `score`, `matched_atoms`, `publish_tip`,
`created_at_ms`, and nothing else: no descriptor text, no summary, no atom
list.
Check: `always` - a read for a `superseded` decision answers the terminal
even when rows exist; a read for an active decision with 70 rows and limit 64
answers 64 rows and a `next`; the response JSON key set equals the six names;
a read bound to project Q for a decision of project P answers the kernel
routes' scope refusal; the semantics is `always` because every response must
satisfy the predicate.
Fault/timing angle: the decision superseded between the pass and the read; a
row written between the standing read and the table read (legal omission).
Required faults and enabling state: flag on; rows for an active and a
superseded decision; more rows than the page bound; two project bindings.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (`review.list` paging contract at
`docs/host-wire-protocol.md:792` is the model)
Impact: rows for a dead decision invite a dispute the Kernel will refuse;
text in the answer is a new disclosure path for Sensitive harness capture
(`source_capture.rs:710-716`) to the model.
Open questions:
- Terminal name and the section 7.8 method literal (API handoff); whether
  `matched_atoms` as a count is acceptable where the atom list is not (yes
  proposed: a count is not text).
- G7, sensitivity of descriptor-derived rows: resolved by #1165. A row holds
  ids, integers, and times only (the table's columns are pinned by
  `the_table_holds_ids_integers_and_times_and_checks_its_shapes`,
  `crates/memory-store/tests/contradiction_candidates.rs:415`), the memory
  store is local, and the read answers carry the same closed key set, so a
  derived row inherits local-only egress by construction and carries no
  sensitivity column.
Falsifier: the read answers rows for a `superseded` decision, or a row
includes `summary`.
Legal witness: active decision with 3 rows answers 3 rows and `next: null`;
the same call after the decision is superseded answers `not_live`.

### cc-read-distinguishes-disabled-unavailable-and-empty

Type: safety
Reachability: default-production (the terminal branches are reachable with
the flag at its default)
Status: active - PROPOSED
Exercised: not yet - U4 read route and tool action do not exist
Guarantee: Flag off answers `disabled` without touching the table; a
memory-store that cannot be read answers `store_unavailable`; a Kernel that is
not Ready answers the kernel routes' state; an eligible decision with no rows
answers an empty page; no terminal is ever rendered as an empty page.
Check: `always` - the four conditions answer four distinct shapes; the
semantics is `always` because every response must be one of them.
Fault/timing angle: Kernel Starting at read; memory-store file unreadable;
flag toggled between route bind and read (the frozen snapshot decides).
Required faults and enabling state: a route with the flag off; a route with
the flag on and the store removed; a Kernel in Starting; an eligible decision
with zero rows.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (`review.list` terminal vocabulary at
`docs/host-wire-protocol.md:790`)
Impact: an agent reading an empty page as "no contradiction" when the store
was unavailable.
Open questions: None.
Falsifier: Kernel Starting answers `{items: [], next: null}`.
Legal witness: flag off: `disabled`; flag on, store missing:
`store_unavailable`; flag on, no rows: `{items: [], next: null}`.

## S-G reachability

### cc-flag-off-no-pass-no-write-no-table-read

Type: safety
Reachability: default-production (the flag's default is off; this record
describes the default build's behavior)
Status: active - PROPOSED
Exercised: partial - sweep and status arm (#1166):
`a_false_flag_runs_no_table_statement_and_moves_no_counter` (`crates/daemon/tests/contradiction_candidates_sweep.rs:552`)
hides the table, runs the sweep under a false flag with and without a
Kernel, and observes every counter at zero, then shows the same table fails
the first statement an enabled sweep runs;
`the_cleanup_pass_carries_the_watermark_and_reads_the_flag_each_pass`
(`:900`) drives `pass_then_sweep`
(`crates/daemon/src/memory_reviewer/lifecycle.rs:448`) with the flag off and
then on. The key is `/contradiction_candidates/enabled`
(`crates/daemon/src/config.rs:1024`), user-only and default false
(`contradiction_candidates_default_off_and_only_the_user_tier_enables_them`,
`config.rs:2299`); a configuration resolved before a user-tier rewrite keeps
its value (`config.rs:2320`), and a bound route keeps the value it was bound
with (`a_route_keeps_the_candidate_flag_it_was_bound_with_and_health_reports_the_block`,
`crates/daemon/src/lib.rs:21340`), which also shows the status block
carrying the key with every counter zero. Pass arm (#1167):
`a_route_without_consent_offers_no_pass_and_writes_no_row` (`crates/daemon/src/candidate_pass_tests.rs:251`)
publishes three windows with source capture on and consent off and observes
every counter at zero and no row; `the_route_consent_reaches_the_pass_through_a_transform`
(`:933`) binds the route against the opposite daemon value and shows the
offer follows the route's frozen value. No statement trace exists, so the
pass arm's "no table statement" clause rests on zero offers. The read arm
lands with #1168.
Guarantee: With the route's frozen `contradiction_candidates.enabled` false
(default), no candidate pass is offered after any publication, no row is
written, and neither read surface opens the table; the flag is enabled only by
explicit configuration at the `/contradiction_candidates/enabled` path and is
frozen in the route's consent snapshot like `/source_capture/enabled`
(`config.rs:102,157,1023`). The sweep is daemon-global: it reads the
daemon-level value again on every cleanup pass
(`crates/daemon/src/memory_reviewer/lifecycle.rs:448`) and, while that value
is false, runs no table statement and moves no counter.
Check: `always(!X)` - X is "a pass counter increments, a row exists, or a
table read statement executes for a route whose frozen flag is false"; this
is a forbidden state with no dedicated detection point, so `always(!X)`
rather than `unreachable`; assert by publishing with the flag off and
checking counters, row count, and a statement-trace hook.
Fault/timing angle: a config reload that turns the flag on after the route
bound (the route keeps false, while the sweep follows the reload on its next
pass); a daemon restart with the flag on (new routes see true).
Required faults and enabling state: a default config; publications with
source capture on; a `candidates` call and an owner read under the default.
Confidence: low - only the sweep and status arm exist; the pass and read
arms are unbuilt
Existing check: the sweep-arm tests named under Exercised (#1166),
unaudited.
Impact: an unmeasured matcher reaches users (KTD4) and a surprise write load
appears in deployments that did not opt in.
Open questions:
- Whether the TS plugin hides the `candidates` action from the tool schema
  when the daemon reports the flag off, since the schema itself is a visible
  behavior change (OQ-E) (needs human input).
Falsifier: flag off, a `candidates` call opens the table and answers an empty
page instead of `disabled`.
Legal witness: default config, five publications: zero pass admissions, zero
rows, `candidates` answers `disabled`.

## S-H confirm linkage

### cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U3 table and U4 surfaces do not exist
Guarantee: A row's `evidence_id` is byte-identical to the
`Published.evidence_object_id` of the descriptor's publication
(`harness_sources.rs:521-532`) and its decision object id is the `object_id`
P4 `dispute` takes, so `dispute {object_id, event, evidence_id}` copied from a
row is well formed; the list performs no evidence-liveness check; a confirm
whose evidence has been retired is refused `not_found` by P4 C6 at commit;
the confirm path never reads the candidate table.
Check: `always` - for every row the evidence id equals the publication
receipt's evidence object id and names an `evidence_meta` row at write time;
a `dispute` built from a row with live evidence lands `Disputed`; after the
evidence is retired the same row is still listed and the same `dispute` is
refused `not_found`; the semantics is `always` because every row and every
confirm must satisfy it.
Fault/timing angle: the `harness_capture` 30-day retention sweep retiring the
evidence between list and confirm (`schema.rs:136`); a session delete retiring
the descriptor.
Required faults and enabling state: flag on; P4 D8 `dispute` landed; a row
for a live descriptor; evidence retired by the Kernel sweep or
`retire_evidence`; a confirm after retirement.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (P4 proposed record
`cited-evidence-live-at-dispute-commit` owns the confirm-side check)
Impact: a row carrying the descriptor `object_id` instead of the evidence id
makes every confirm fail; a list-side liveness check would hide the refusal
the confirm must own and cost a Kernel read per row.
Open questions:
- Under P4 OQ4-3's recommended visibility definition, a retired descriptor
  leaves its evidence uncited and the confirm is `not_found` even if the
  evidence row is live; the record's expected refusal name is the same.
Falsifier: a row whose `evidence_id` is the `srcdesc:*` object id; or the
list omits rows with retired evidence.
Legal witness: row (E, O1); `dispute {object_id: O1, event: mark_disputed,
evidence_id: E}` lands `Disputed`; after retiring E the row is listed and the
confirm is `not_found`.

## Cross-cutting R5

### cc-pass-and-sweep-write-no-kernel-event-and-no-signal

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: partial - sweep arm (#1166): every enabled sweep in
`crates/daemon/tests/contradiction_candidates_sweep.rs` runs through
`Fixture::sweep` (`:290`), which compares the Kernel tip and the
MemoryReviewer job facts before and after the run;
`the_candidate_modules_name_no_kernel_write_path` (`:694`) is a lexical proxy
that finds no Kernel write entry point, `CommitIntent`, or `CausalInputs` in
the files of `crates/daemon/src/contradiction_candidates/` and does not
follow calls out of the module; and
`every_reviewer_job_the_daemon_builds_carries_no_signal` (`:718`) scans every
`.rs` file under `crates/daemon/src` for `CausalInputs {` literals and
requires `signals: Vec::new()` in each, so a construction through a helper
or struct update would escape it. Pass arm (#1167): every direct pass in
`crates/daemon/src/candidate_pass_tests.rs` runs through `Core::score`, which compares the Kernel tip and the job
facts before and after; the handler rebuild test (`:850`) does the same
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
incarnation, `docs/memory-reviewer-operations.md:257`).
Open questions: None.
