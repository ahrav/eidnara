# wit-package-carries-original-and-minimized

## Discovery trigger
Parent specification Implementation Decisions: "Witness package: original
failure (RunId, decision tape, canonical trace digest, causal trace, oracle
and checkpoint, coverage signature), minimized semantic scenario, and a
compact recipe form when multiplicity is the trigger."

## Evidence trail
- `crates/eval-core/src/witness.rs` `OriginalFailure`, `WitnessPackage`,
  `MultiplicityRecipe` (two `Generation`s and the counted kinds).
- `crates/eval-core/src/witness.rs` `count_triggered` reads the shrink
  report: a surviving aged event whose single deletion (over the final
  deletion set, under the digest of the scenario it produces) was recorded
  `Slipped` or `NotReproduced` counts toward its kind; kinds with one such
  event are dropped.
- `crates/eval-core/src/witness.rs` `regenerates` compares the declared event
  count with the minimized log plus that history's deletions before calling
  `generate_all`, then applies the deletions and compares logs.
- `crates/eval-core/tests/witness.rs` `the_package_round_trips_and_carries_the_recipe_for_a_count_triggered_failure`:
  round trip, five counted commits (the sixth is evidence and pair-invalid to
  delete), `RecipeRequired`, `RecipeDisagrees` for each history, the
  oversized config, `RecipeMultiplicitiesDisagree`, `RecipeWithoutMultiplicity`
  once the changing records are removed, a refused extra field, `Lossy`.
- `crates/eval-core/src/witness.rs` `check_minimality`: a `OneMinimal`
  report must record, for every minimized element, a single deletion over
  the final set, under the digest of the scenario it produces, whose
  verdict is neither `Reproduced` nor `Unknown`.
- `crates/eval-core/src/witness.rs` `validate` ends by checking every
  `original.coverage` name against `MARKERS`.
- `crates/eval-core/tests/witness.rs` `every_structural_refusal_names_its_cause`: outer and embedded
  schema, hex, digest, and predicate refusals through `validate` and
  `serialize` alike.
- `crates/eval-core/tests/witness.rs` `one_minimality_needs_a_rejected_record_for_every_single_deletion`:
  a report that replayed only the original refuses a `OneMinimal` claim and
  passes a `NotEstablished` one; a rejection record under a foreign digest is
  no record.
- `crates/eval-core/tests/witness.rs` `a_multiplicity_record_counts_only_under_its_own_scenario_digest`:
  a counted commit's records under a foreign digest drop out of the count
  and the recipe disagrees.
- `crates/eval-core/tests/witness.rs` `a_one_minimal_claim_names_exactly_the_transformations_the_scenario_held`:
  a dropped or padded transformation list is refused.
- `crates/eval-core/tests/witness.rs` `the_remaining_count_is_the_minimized_scenario_s_element_count`:
  a budget claim with a survivor count the minimized scenario contradicts is
  refused.
- `crates/eval-core/tests/witness.rs` `a_residue_that_no_schema_could_declare_is_refused`:
  a `Keep` entry and a second rule for one field are refused.
- `crates/eval-core/tests/witness.rs` `a_minimized_scenario_that_cannot_compile_is_refused`:
  a taskless minimized scenario with every digest agreeing is refused.
- `crates/eval-core/tests/witness.rs` `a_deleted_element_cannot_also_survive`,
  `the_failure_predicate_names_the_task_the_replay_evaluates`,
  `the_recipe_regenerates_the_original_tape_too`,
  `the_recipe_regenerates_the_original_causal_trace_too`: a survivor named as
  deleted, a predicate over a task the scenario lacks, and an original tape
  or causal trace the aged generation does not produce are refused.
- `crates/eval-core/tests/witness.rs` `an_invalid_pair_record_is_evidence_only_when_the_compiler_refuses`:
  a compiling single deletion relabelled `InvalidPair` is refused.
- `crates/eval-core/tests/witness.rs` `the_coverage_signature_names_only_registered_markers`:
  an unregistered name refuses through `validate` and `serialize`.
- `crates/daemon/tests/eval_shrink.rs` `a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`:
  the published `witness.json` parses back to the run's package and the
  manifest's `witness_digest` is its protocol digest.

## Failure scenario
A recipe recorded with the wrong seed regenerates a different aged history;
a reader following it reproduces nothing. A recipe declaring a million
commits would make the reader generate them before comparing.

## Timing windows and dependencies
None.

## What a test must construct
A count-triggered minimized scenario and a tampered recipe.

## Investigation log
### Q: When is a recipe required?
- Sources examined: `check_recipe`, `count_triggered`.
- Findings: when minimality is `OneMinimal` and some kind has more than one
  surviving aged event whose single deletion changed the outcome. An event
  whose deletion is `InvalidPair` (the evidence) does not count.
- Missing evidence: none.
- Conclusion: resolved with answer.
