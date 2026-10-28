# Portfolio evaluation: P5 proposed obligations

Evaluated at `446c7363f` (#1164 scorer commits `05acd0107` and
`446c7363f` on base `979fb814e`) plus uncommitted docs; citations are
corrected to HEAD `400efc860`, which adds the test
`a_case_change_inside_an_identifier_changes_its_parts` and shifts later
lines of the scorer test file by 12. Independent fresh-context pass
over the three active S-A records in `catalog.md`, `existing-checks.md`,
`fault-map.md`, and the three evidence files, read against
`crates/daemon/src/contradiction_candidates/scorer.rs`,
`crates/daemon/tests/contradiction_candidates_scorer.rs`, and
`crates/retrieval/src/lexical/analysis.rs`. The 12 proposed records are out
of scope. Short names: determinism, oversize, case fold.

Sampled citations confirmed: `scorer.rs:66-68`, `:77`, `:85-96`,
`:103-109`, `:386-395`, `:432-562`, `:590-606`, `:627-644`, `:694-695`
(line numbers re-verified after the term pipeline moved to hashed
spellings);
`analysis.rs:78-105`; `lib.rs:17`; `contradiction_candidates/mod.rs:4`;
`lexical/mod.rs:40`; every test range in `existing-checks.md`. A grep of
`crates/` finds no caller of the scorer outside its test file, so
`test-only` holds for all three. Two ranges are off (R6).

## 1. Harness fit

All three records are pure-function properties. Each one runs as an
in-crate integration test with no fixture beyond strings, and that is where
the tests already live.

| Record | Route | Why |
|---|---|---|
| determinism | daemon integration test; property test fits | `proptest` is a daemon dev-dependency (`crates/daemon/Cargo.toml:112`) |
| oversize | daemon integration test at exact and over-bound sizes | refusal precedes retention (`analysis.rs:82-87`) |
| case fold | daemon integration test over literal pairs | one fold rule in `terms` (`scorer.rs:111-132`) |

The pass arm of oversize (zero rows, `descriptors_refused_oversize`) routes
to #1167 and is correctly labelled unverifiable.

## 2. Coverage balance

All three records are `always` safety records on S-A. That fits a pure
scorer. The scorer's own surface has two uncatalogued obligations.

First, the determinism Guarantee holds "under an equal matcher digest". The
digest preimage is a list of rule ids and constants (`scorer.rs:648-683`).
A change to `kept`, `scan_ascii`, `terms`, or `rank` that keeps those ids leaves the
digest unchanged, so the version stops identifying the behavior. Only the
hand-written term-set test (`tests:31-80`) would notice some such changes.
Gap G1.

Second, the candidate predicate (three gates, compared by
cross-multiplication, `scorer.rs:547-561`) has tests (`tests:187-222`) and
no record. `existing-checks.md` marks those tests "none directly". Gap G2.

The three `sometimes` markers in `fault-map.md` have no owner until #1167
(B1).

## 3. Implementability

Every required fault the fault map lists as available is constructed in
the tests. A6 is unconstructible as stated: n atoms need 2n - 1 bytes, and
the atom check at `analysis.rs:92` refuses only the atom after
`max_atoms`. Under either ticket formula the byte bound binds first.

Three clauses lack an oracle:

- Case fold: the first Check clause is false for some inputs (R1).
- Oversize: "every other summary's result equals its result without that
  summary present" is not asserted. `tests:267-273` checks the kept ids
  only (R4).
- Determinism: repeated decision object ids are exercised in two orders
  with no K pressure (`tests:320-346`), apart from the 18-order
  permutation test (R5).

## 4. Wildcard

- The analyzer splits parts on the original case. It splits at
  lower-to-upper (`analysis.rs:198`) and before the last capital of an
  uppercase run (`:199-201`), and the scorer folds afterwards (`:107`). So
  `configLoader` yields {configloader, config, loader} and `configloader`
  yields {configloader}. The two differ only in case and have unequal term
  sets. The case test uses whole-word case changes only
  (`"ÉCOLES CACHE ..."`, `tests:134`), which never cross a part boundary.
  The record states the fold correctly. It overstates the consequence
  (R1, B2).
- `unscorable` holds decision object ids, not decision ids
  (`scorer.rs:636`). With a repeated object id, one id can appear twice in
  `unscorable`, or in both `unscorable` and `candidates`. The determinism
  Guarantee admits repeated ids, so the oversize Check clause "in
  `unscorable` and absent from `candidates`" is false for such inputs (R3).
- The determinism evidence says "only identical triples tie". `rank`
  ignores the summary, so two entries with equal ids and equal counts tie
  with different summaries. Their outputs are equal, so the conclusion
  stands and only the wording is wrong (R6).

## Finding disposition

| Id | Class | Finding and action | Disposition |
|---|---|---|---|
| R1 | refinement | case fold: Guarantee and Check claim equal term sets for case-only differences; false when the case change crosses a part boundary (`analysis.rs:198`). Restate as "one fold over the analyzer's output on both sides", limit the equality clause to case changes that keep part boundaries, add a `configLoader`/`configloader` pair to the record and to fault class A8 | applied: Guarantee and Check restated in `catalog.md`; `a_case_change_inside_an_identifier_changes_its_parts` (`tests:163-171`) constructs the pair and pins `configLoader` to {configloader, config, loader} and `CONFIGLOADER` and `configloader` to {configloader}; added to `existing-checks.md`, fault class A8, and the evidence file |
| R2 | refinement | case fold: "a diacritic-only or NFC-versus-NFD difference gives disjoint terms" holds per term, not per text; reword | applied: the Check states distinctness per term; the evidence file notes that every term of the tested pairs differs |
| R3 | refinement | oversize: qualify the unscorable clause for repeated object ids, and note that `unscorable` lists object ids and can repeat one | applied: Guarantee and Check qualified for repeated object ids; record and evidence note that `unscorable` lists object ids (`scorer.rs:636`) |
| R4 | refinement | oversize: the "result unchanged without the unscorable summary" clause has no assertion; mark it `partial` in `Exercised` or name a test that compares against a run without it | applied: `Exercised: partial` with the unasserted clause named; index, `existing-checks.md`, and evidence updated |
| R5 | refinement | determinism: no test combines a repeated object id with more than K passing summaries across many orders; name a property test over random permutations with repeated ids as the stronger oracle | applied: `Exercised`, fault class A3, `existing-checks.md`, and evidence name the property test as the stronger oracle |
| R6 | refinement | citations: `harness_sources.rs:612-616` is `:612`; `identity_pins_the_manifest` is `lexical_analysis.rs:389-413`; the determinism evidence's "only identical triples tie" should say equal ids and counts tie with equal outputs | applied: `harness_sources.rs:612` and `lexical_analysis.rs:389-413` corrected; the evidence's tie wording restated |
| G1 | gap | no record ties `matcher_digest()` to scorer behavior; one record whose oracle pins a fixed corpus's scores in the same test as the digest literal | queued: needs one record and a test that pins a fixed corpus's scores beside the digest literal (see Gaps queued) |
| G2 | gap | the three-gate candidate predicate has tests and no record; one record | queued: needs one record for the three gates and the exact-threshold boundary over `tests:187-222` (see Gaps queued) |
| B1 | bias | the scorer has no runtime caller, and the pass arm, markers, and digest recording all wait on #1165 and #1167 | surfaced for a human: whether #1165 and #1167 must convert the markers, counters, and digest recording |
| B2 | bias | the case-dependent part split is inherited analyzer behavior; whether a case-only identifier difference should match is a design decision | surfaced for a human: intended behavior or fold before split under a new epoch; recorded as a case-fold open question |

## Gaps queued

- G1: read `matcher_preimage` (`scorer.rs:648-683`) and the digest pin
  (`tests:348-400`). Write one record: for a fixed corpus of descriptor and
  summary pairs, a change in any score requires a changed preimage. The
  corpus scores are pinned next to the digest literal so both change
  together.
- G2: read `DescriptorTerms::score` (`scorer.rs:526-562`). Write one record
  covering the three gates and the exact-threshold boundary, citing
  `tests:187-222`.

## Biases for a human

- B1. The three records are `test-only` until #1167 calls the scorer
  behind `contradiction_candidates.enabled`. The coverage markers, the
  oversize counters, and the recording of `matcher_digest()` with each row
  have no owning ticket text yet. Decide whether #1165 and #1167 must
  convert them.
- B2. `analysis.rs:4` documents case-driven part splits and the scorer
  folds afterwards (`scorer.rs:111-132`). A descriptor that writes
  `CONFIGLOADER` loses the `config` and `loader` matches that
  `configLoader` would give. Decide whether that is intended matcher
  behavior or whether the fold should precede the split. A change needs a
  new `CANDIDATE_MATCHER_EPOCH`.

## Verdict

The three records are fit as tests of the scorer once R1 and R3 correct
the two Check clauses that are false for valid inputs. R2, R4, R5, and R6
are wording and coverage fixes. G1 is the gap that matters for the
rebuild obligation, because the determinism record assumes the digest
identifies the behavior.
