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
read (#1168) tickets make them checkable. Source: the
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
| cc-pass-starts-only-after-publication-job-terminal | S-B | safety | always | intended explicit-config-only | proposed (#1167) |
| cc-passes-in-flight-bounded-and-overflow-counted | S-B | safety | always | intended explicit-config-only | proposed (#1167) |
| cc-candidate-set-bounded-with-counted-truncation | S-C | safety | always | intended explicit-config-only | proposed (#1167) |
| cc-partial-result-is-descriptor-granular-prefix | S-C | safety | always | intended explicit-config-only | proposed (#1167) |
| cc-row-key-unique-and-caps-enforced | S-D | safety | always | intended explicit-config-only | proposed (#1165) |
| cc-table-rebuildable-from-evidence-bytes-and-live-decisions | S-D | safety | always | intended explicit-config-only | proposed (#1165, #1167) |
| cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched | S-E | safety | always | intended explicit-config-only | proposed (#1166) |
| cc-read-serves-only-live-decision-rows-ids-and-scores-only | S-F | safety | always | intended explicit-config-only | proposed (#1168) |
| cc-read-distinguishes-disabled-unavailable-and-empty | S-F | safety | always | intended default-production | proposed (#1168) |
| cc-flag-off-no-pass-no-write-no-table-read | S-G | safety | always(!X) | intended default-production | proposed (#1167, #1168) |
| cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm | S-H | safety | always | intended explicit-config-only | proposed (#1168) |
| cc-pass-and-sweep-write-no-kernel-event-and-no-signal | R5 | safety | always(!X) | intended explicit-config-only | proposed (#1166, #1167) |

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
Read `crates/daemon/src/contradiction_candidates/scorer.rs:424-465` (the
descriptor table is sorted by hash, then folded text, and a summary
contributes only its `matched_terms` and `decision_terms` counts, so no
iteration order reaches the output), `:386-395` (`rank`, a total order
ending at both ids), `:573-606` (`record`, `offer`), `:627-644`
(`score_descriptor` sorts `unscorable`), `:648-683` (the preimage names the
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
(`crates/daemon/src/contradiction_candidates/scorer.rs:636`), so with a
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
`:103-109`, `:432-465`, `:526-562`,
`:627-644`; the compile-time assertion at `:66-68` holds the descriptor
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
`:432-465` (`DescriptorTerms::new` takes the descriptor's terms from
`terms`), `:483-562` (`score` takes the summary's terms from `scan_ascii`
through `count_ascii`, or from `terms` when a term is not ASCII or more than
64 are distinct), `:18-25` (the module header states the fold is not full
case folding), `:77` (`FOLD_RULE_ID` enters the preimage);
`crates/retrieval/src/lexical/analysis.rs:193-204` (`splits`, the
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
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2 trigger does not exist
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
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: the pass would consume the 5 s publication budget or hold the gate,
delaying publication and session deletion (KTD2 stop condition).
Open questions:
- How the `Published` list and the descriptor text leave the gate: carried
  from the job or read back from the evidence object (OQ-C) (needs human
  input).
Falsifier: a candidate row exists while `between_screen_and_commit` is held.
Legal witness: hooks held, zero rows; hooks released, job `Completed`, pass
writes rows, publish latency counters `PublishMsLast` unchanged by the pass.

### cc-passes-in-flight-bounded-and-overflow-counted

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2 trigger does not exist
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
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: unbounded passes would hold Kernel readers and memory proportional to
publication rate.
Open questions:
- P and whether the bound is per project or per daemon (needs human input).
Falsifier: 1,000 rapid publications create 1,000 concurrent passes.
Legal witness: P = 1, three publications while parked: one pass, two refusals
counted.

## S-C candidate set bounding

### cc-candidate-set-bounded-with-counted-truncation

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U1/U2 read path does not exist
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
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (`kernel.read` uses the same pattern at
`read.rs:170-230` for a different read)
Impact: pass cost grows with project size; a large project stalls publication
follow-up and holds Kernel readers.
Open questions:
- N; whether the SQL walk of `visible_domain_decisions_as_of_in_scope`
  (no budget, no limit, `admission.rs:3165-3176`) is acceptable or a budgeted
  Kernel variant is required (OQ5-1) (needs human input).
Falsifier: a project with 100,000 `memory` decisions makes the pass load
100,000 summaries.
Legal witness: N = 256, 300 eligible decisions: 256 scored, 44 omitted,
truncation counted once.

### cc-partial-result-is-descriptor-granular-prefix

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U2 budget handling does not exist
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
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: a half-scored descriptor's rows are not its top-K; a reader sees a
false ranking and the rebuild obligation cannot be stated.
Open questions:
- Whether a pass writes per descriptor or once at the end (persistence
  handoff); per descriptor is the proposal.
Falsifier: a deadline inside descriptor 3 of 5 leaves two of descriptor 3's
five rows.
Legal witness: deadline during descriptor 3: rows for descriptors 1 and 2
complete, none for 3, 4, 5; `deadline` = 1.

## S-D row semantics

### cc-row-key-unique-and-caps-enforced

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: partial - direct-write store arm (#1165): `crates/memory-store/tests/contradiction_candidates.rs`
`each_key_has_one_row_and_a_rewrite_keeps_the_greater_tip` (`:73`) shows one
row per key, a lower-tip rewrite that keeps the higher tip and replaces the
other values, and a newer write that removes keys it no longer names;
`a_late_write_replaces_held_keys_and_adds_none` (`:446`) shows a write below
the held tip adds no key, so a descriptor never exceeds K;
`a_descriptor_over_its_cap_or_with_a_malformed_row_is_refused_whole` (`:115`)
and `the_caps_hold_at_their_boundaries_and_refusals_leave_held_rows` (`:559`)
show K accepted, K + 1 refused with the held rows unchanged, and a project
at `per_project` evicting the lowest `score_bp`, then oldest, then smallest
key with the count returned; `a_project_at_its_cap_evicts_the_lowest_score_then_the_oldest`
(`:188`) shows another project's rows are untouched;
`a_new_row_the_cap_evicts_at_once_counts_as_written_and_dropped` (`:1287`)
shows a new row below the cap's floor counted written and dropped with the
held rows and rowids unchanged; and
`random_write_and_delete_sequences_match_the_reference_model` (`:1038`)
compares 24 seeded sequences of writes, late writes, incarnation changes,
evictions at caps of 2 to 4096, and keyed deletes against a reference model
of the rules, checking every row, the returned counts, rowid stability, and
the gauge after each step, and pages, cursors across writes, and the sweep
every seventh step. The store
(`crates/memory-store/src/contradiction_candidates.rs:488`) enforces K and the
project cap in the write transaction; the daemon owns the values. The pass
arm (M per pass, `rows_written`, replayed units) lands with #1167.
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
Confidence: low - proposed; the direct-write store checks above exercise the
key, the caps, the eviction order, and the counts at the #1165 tip; the pass
arm (M per pass, `rows_written`, replay) lands with #1167
Existing check: the store checks named under Exercised, `unaudited`
(`existing-checks.md` S-D); none for the pass arm
Impact: duplicate rows inflate candidate counts; uncapped rows make the
table grow with project size per pass.
Open questions:
- K and M (OQ5-3); whether a replayed descriptor is re-scored or skipped
  (both satisfy the record) (needs human input).
Falsifier: two rows with equal (project, evidence id, decision object id), or
K + 1 rows for one evidence id after one pass.
Legal witness: a descriptor matching 12 decisions with K = 5 yields 5 rows;
publishing it again yields the same 5 rows with a newer `publish_tip`.

### cc-table-rebuildable-from-evidence-bytes-and-live-decisions

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: partial - store arm (#1165):
`truncating_and_replaying_the_writes_rebuilds_the_same_rows`
(`crates/memory-store/tests/contradiction_candidates.rs:364`) truncates through the test-support
`truncate_contradiction_candidates_for_test` and replays a write sequence,
reproducing the same rows excluding `created_at_ms`. The pass arm, which
re-scores retained evidence bytes at one tip, lands with #1167.
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
Confidence: low - proposed; `truncating_and_replaying_the_writes_rebuilds_the_same_rows`
exercises truncate and replay of a recorded write sequence at the #1165 tip;
the rebuild from evidence bytes lands with #1167
Existing check: `truncating_and_replaying_the_writes_rebuilds_the_same_rows`
(`crates/memory-store/tests/contradiction_candidates.rs:364`), `unaudited`;
none for the pass arm
Impact: without this the table is a log, not a derivation, and the derived
classification (index D6) is false.
Open questions:
- Whether the pass carries text from the job or reads the evidence object
  (OQ-C); a carried copy must be byte-equal to the evidence (needs human
  input).
- Where V is recorded so a version change triggers drop-and-rebuild.
Falsifier: truncate + rebuild at the same snapshot yields a different set
because the pass scored a normalized or redacted copy of the text.
Legal witness: 9 rows before truncate; 9 equal rows after rebuild at the
same `tip`.

## S-E sweep

### cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched

Type: safety
Reachability: explicit-config-only
Status: active - PROPOSED
Exercised: not yet - U3 sweep does not exist
Guarantee: One sweep call examines at most one page of rows and drops exactly
those whose decision object is invalidated at the sweep's Kernel snapshot,
whose disposition is `superseded` or `contradicted` (or otherwise not
eligible under OQ-B), or whose descriptor object or evidence row is
invalidated; every other row is kept; a repeat at the same tip drops zero; the
sweep writes no Kernel row; a Kernel that is not readable yields zero drops
and an unavailable result.
Check: `always` - seed rows for decisions in states {active, disputed, stale,
superseded, contradicted, invalidated} and for one retired descriptor; one
sweep drops the superseded, contradicted, invalidated, and retired-descriptor
rows and keeps active, disputed, stale; a second sweep drops zero; the Kernel
commit log row count is equal before and after; with the Kernel closed the
sweep returns unavailable and the table is unchanged; the semantics is
`always` because every sweep step must satisfy the predicate.
Fault/timing angle: a pass writing a row between the sweep's judgment and its
delete (the row survives to the next sweep); the Kernel retention sweep
retiring evidence concurrently; more rows than one page.
Required faults and enabling state: flag on; rows in each state; a Kernel
store closed during the sweep; a page overflow.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed (precedent sweeps: `expire_memory_reviewer_work`
`memory_reviewer_jobs.rs:1629`, `prune_terminal_memory_captures`
`memory_capture.rs:484-491`)
Impact: over-dropping loses candidates for live decisions; under-dropping
serves rows for dead decisions; a Kernel write would violate R5.
Open questions:
- Whether `stale` and `disputed` rows are kept (proposed yes: both are
  confirmable targets under P4 C2 lateral moves) (needs human input).
- Page size and who schedules the sweep (daemon-called, as the precedents).
Falsifier: a sweep drops a row whose decision is `disputed` and live, or a
sweep with the Kernel unavailable drops every row.
Legal witness: six seeded rows, one sweep drops three, a repeat drops zero,
commit log unchanged.

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
- G7, sensitivity and egress class of descriptor-derived rows: unresolved,
  needs a policy decision. #1165 fixes the facts the decision rests on: a row
  holds ids, integers, and times only (the table's columns are pinned by
  `the_table_holds_ids_integers_and_times_and_checks_its_shapes`,
  `crates/memory-store/tests/contradiction_candidates.rs:403`), so a derived
  row carries none of the captured text, and the store has no sensitivity
  column. The descriptor it derives from publishes as `Sensitive` with
  `ProviderEgress::LocalOnly` (`crates/daemon/src/source_capture.rs:844-845`);
  whether the term counts, the score, and the evidence-to-decision link
  inherit that class is the open question, and a column set does not answer
  it. The egress half waits on #1168: the store's `candidates_for_decision`
  returns rows to its caller, so what leaves the daemon is decided by the U4
  read route. That route also owns the six-name key-set check above, and the
  store's `StoredCandidate` carries `project_digest`, `kernel_incarnation_id`,
  `matcher_digest`, and `decision_terms` beyond that set, so the read projects
  the row rather than returning it (needs human input).
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
Exercised: not yet - U2 flag and U3 table do not exist
Guarantee: With the route's frozen `contradiction_candidates.enabled` false
(default), no candidate pass is offered after any publication, no row is
written, and neither read surface opens the table; the flag is enabled only by
explicit configuration at the `/contradiction_candidates/enabled` path and is
frozen in the route's consent snapshot like `/source_capture/enabled`
(`config.rs:102,154,993`).
Check: `always(!X)` - X is "a pass counter increments, a row exists, or a
table read statement executes for a route whose frozen flag is false"; this
is a forbidden state with no dedicated detection point, so `always(!X)`
rather than `unreachable`; assert by publishing with the flag off and
checking counters, row count, and a statement-trace hook.
Fault/timing angle: a config reload that turns the flag on after the route
bound (the route keeps false); a daemon restart with the flag on (new routes
see true).
Required faults and enabling state: a default config; publications with
source capture on; a `candidates` call and an owner read under the default.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
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
Status: active - PROPOSED
Exercised: not yet - U2/U3 do not exist
Guarantee: Across any pass and any sweep the Kernel commit log, admission
rows, and object registry are unchanged, no decision's disposition or
maturity changes, no MemoryReviewer job is reserved, and every
`CausalInputs.signals` constructed in the daemon remains empty
(`memory_reviewer_jobs.rs:79-82`; producers at `selection.rs:314`,
`handoff.rs:374`, `lifecycle.rs:526`).
Check: `always(!X)` - X is "a Kernel write, a reviewer reservation, or a
non-empty `signals` attributable to the candidate feature"; a forbidden state
with no dedicated detection point; assert by comparing commit-log and
reviewer-job row counts before and after a pass and a sweep, and by a
source-level check that no code path from the candidate module reaches a
Kernel write family or a `CausalInputs` constructor with a non-empty vector.
Fault/timing angle: none; the obligation is structural.
Required faults and enabling state: flag on; a pass that writes rows; a sweep
that drops rows; a candidate whose score would tempt an automatic dispute.
Confidence: low - proposed; no implementation evidence
Existing check: none; proposed
Impact: the plan's three stop conditions; a signal would reserve a
receipt-charged reviewer job against the Q38 ceiling (2,048 per project per
incarnation, `docs/memory-reviewer-operations.md:263`).
Open questions: None.
Falsifier: a non-empty `signals` vector built from candidate rows, or a
commit-log row attributed to the pass.
Legal witness: commit-log count 412 before the pass, 412 after; reviewer job
count unchanged; rows written 5.
