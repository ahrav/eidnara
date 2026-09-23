# Evaluator property catalog

METHOD-ordered records for the properties the long-horizon evaluator exercises,
promoted here once their checks were re-verified at the then-current HEAD. The
field order and check semantics come from [`../METHOD.md`](../METHOD.md); the
executed checks by phase and test name stay in [`README.md`](README.md). A test
(`crates/eval-core/tests/method_records.rs`) reads this file and refuses a
record whose fields are out of order, whose semantics are outside the closed
set, whose cited test is missing or ignored, or whose evidence file is missing.

## Scope

Phase 5: shrinking, the replay-effect protocol, the witness package, and the
gaps the ticket names (ingestion reachability, the Pi adapter, coverage
witnesses). Each record names the entry point that reaches it in its
`Reachability` field.

## Part artifacts

This part carries `catalog.md` and `evidence/`. The `existing-checks.md`,
`fault-map.md`, and `portfolio-evaluation.md` files METHOD lists per part are
not written yet; [`README.md`](README.md) lists the executed checks by phase
without per-check audit status, and no independent portfolio evaluation has
run over this part. Evidence files are shorter than the METHOD target where
the verified trail is short; none was padded.

## Index

| Slug | Type | Check | Exercised |
| --- | --- | --- | --- |
| [`flt-shrink-preserves-precise-failure-predicate`](#flt-shrink-preserves-precise-failure-predicate) | safety | `always` | yes |
| [`flt-shrinker-unknown-never-not-reproduced`](#flt-shrinker-unknown-never-not-reproduced) | safety | `always` | yes |
| [`flt-paired-worlds-shrunk-together`](#flt-paired-worlds-shrunk-together) | safety | `always` | yes |
| [`flt-hard-case-carries-witness-triple`](#flt-hard-case-carries-witness-triple) | safety | `always` | partial |
| [`rid-replay-equality-semantic-trace-digest`](#rid-replay-equality-semantic-trace-digest) | safety | `always` | yes |
| [`mtr-method-records-cite-executed-check`](#mtr-method-records-cite-executed-check) | safety | `always` | yes |
| [`wit-package-carries-original-and-minimized`](#wit-package-carries-original-and-minimized) | safety | `always` | yes |
| [`wit-live-evidence-never-replayable`](#wit-live-evidence-never-replayable) | safety | `always` | yes |
| [`wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims`](#wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims) | safety | `always` | yes |
| [`wit-residue-drift-refuses`](#wit-residue-drift-refuses) | safety | `always` | yes |
| [`ing-adapter-ingested-no-production-caller`](#ing-adapter-ingested-no-production-caller) | reachability | `always` | partial |
| [`ing-pi-adapter-unexercised-by-evaluator`](#ing-pi-adapter-unexercised-by-evaluator) | reachability | `reachable` | not yet |
| [`flt-coverage-witnesses-fire-only-on-observed-behaviour`](#flt-coverage-witnesses-fire-only-on-observed-behaviour) | safety | `always` | yes |

## Records

### flt-shrink-preserves-precise-failure-predicate

Type: safety
Reachability: test-only - `crates/eval-core/tests/` integration tests over the
  sans-I/O crate
Status: active
Exercised: yes -
  `crates/eval-core/tests/shrink.rs::shrink_preserves_the_predicate_and_rejects_slipped_candidates`,
  `crates/eval-core/tests/shrink.rs::the_shrinker_invariants_hold_under_arbitrary_replay_answers`,
  and
  `crates/daemon/tests/eval_shrink.rs::a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`
Guarantee: Every candidate the shrinker accepts reproduces the pinned failure
  predicate (oracle, cut, profile digest, witness class) field by field; a
  candidate that fails differently is rejected as slipped even though a failure
  remains, and the report states 1-minimality only under the transformations
  actually tried.
Check: `always` - for every `CandidateRecord` with verdict `Reproduced`,
  replaying `original.without(deleted)` yields `Failed { predicate }` equal to
  the pinned predicate; every `Slipped { observed }` record has `observed !=
  predicate` and was not accepted as a reduction; and
  `minimality` is `OneMinimal { transformations }` only when every single
  deletion of the minimized scenario was recorded with a non-`Reproduced`,
  non-`Unknown` verdict. The property must hold on every shrink, so `always`.
Fault/timing angle: The class slips at a count boundary: the planted oracle
  reports `interference` from six required commits and `durable_state` below it,
  so the candidate one commit past the boundary still fails and must be
  rejected.
Required faults and enabling state: A scenario whose original replay fails under
  the pinned predicate; an oracle whose failure class changes under some
  deletion; the pair compiler admitting the candidate.
Confidence: high -
  [evidence](evidence/flt-shrink-preserves-precise-failure-predicate.md). Ran
  the eval-core and daemon tests at HEAD; read `classify_replay` and
  `one_minimality` against the check; the seeded loop covers replay answers the
  fixture alone never produces.
Existing check: `crates/eval-core/src/shrink.rs` `classify_replay`
  (field-by-field comparison), `shrink` (refuses `OriginalNotReproduced`),
  `one_minimality` (restarts until a full pass rejects); tests above.
Impact: A minimized witness would name a different defect than the campaign
  observed, and a reader would debug the wrong failure.
Open questions: None.

### flt-shrinker-unknown-never-not-reproduced

Type: safety
Reachability: test-only - `crates/eval-core/tests/` integration tests over the
  sans-I/O crate
Status: active
Exercised: yes -
  `crates/eval-core/tests/shrink.rs::classify_keeps_unknown_unknown_for_every_reason`,
  `crates/eval-core/tests/shrink.rs::an_unknown_replay_is_kept_and_never_becomes_not_reproduced`,
  `crates/eval-core/tests/shrink.rs::replay_effects_are_bounded_and_a_premature_verdict_is_refused`,
  `crates/daemon/tests/eval_shrink.rs::a_child_that_dies_before_its_barrier_is_retried_then_unknown_and_kept`,
  `crates/daemon/tests/eval_shrink.rs::a_child_that_never_answers_is_cancelled_and_unknown`
Guarantee: A replay that did not answer is `Unknown` for every reason and never
  `NotReproduced`; the element whose deletion answered `Unknown` stays in the
  scenario; outstanding replay effects are bounded and a verdict cannot be read
  before the effect resolves.
Check: `always` - `classify_replay(_, Unknown { reason })` is `Unknown { reason
  }` for every `UnknownReason`; every record whose deletion set includes an
  unanswered element is `Unknown` or `InvalidPair`; `ReplayEffects::issue`
  refuses at `MAX_OUTSTANDING_REPLAY_EFFECTS`; `ReplayEffects::outcome` on an
  outstanding key is `Outstanding`. Must hold on every call, so `always`.
Fault/timing angle: A child exits before its barrier, or never answers: the
  effect is retried once under the same receipt key, then resolved `Unknown`; a
  timeout kills the child and resolves `Unknown { cancelled }`.
Required faults and enabling state: A spawn that produces a child exiting before
  the barrier or sleeping past the replay timeout, keyed to a chosen element so
  the outcome is attributable.
Confidence: high -
  [evidence](evidence/flt-shrinker-unknown-never-not-reproduced.md). Ran the
  tests at HEAD; counted child deaths in the death log against distinct unknown
  candidates.
Existing check: `crates/eval-core/src/shrink.rs` `classify_replay`,
  `ReplayEffects`; shell `crates/daemon/examples/eval_runner/shrink.rs`
  `Replayer::replay` and `attempt`; tests above.
Impact: A crashed or hung replay would be read as "the failure went away" and
  the shrinker would delete the element that caused the crash, hiding a second
  defect inside the witness.
Open questions: None.

### flt-paired-worlds-shrunk-together

Type: safety
Reachability: test-only - `crates/eval-core/tests/` integration tests over the
  sans-I/O crate
Status: active
Exercised: yes -
  `crates/eval-core/tests/shrink.rs::pair_validity_is_recomputed_and_both_worlds_are_shrunk_together`
  and
  `crates/eval-core/tests/shrink.rs::the_shrinker_invariants_hold_under_arbitrary_replay_answers`
Guarantee: Every candidate is compiled into a pair set from its own logs, so the
  fresh arm and the pair mapping are recomputed rather than carried over; a
  candidate the compiler refuses is `InvalidPair` and no replay is issued; a
  deletion names its history and touches only that log.
Check: `always` - for every record, `InvalidPair` exactly when
  `Scenario::compile` refuses `original.without(deleted)`; for every replayed
  record, no deleted natural-fresh event (tagged `~natural-fresh`) appears in
  any pair's fresh arm; deleting a natural-fresh event leaves the aged log
  byte-identical and vice versa on a shared id. Must hold for every candidate,
  so `always`.
Fault/timing angle: Both histories reuse raw event ids, so a deletion keyed by
  id alone would remove one event from each world at once.
Required faults and enabling state: A natural-fresh history whose entity names
  collide with the aged history's (the generator's defaults), and a deletion of
  a shared id from one history.
Confidence: high - [evidence](evidence/flt-paired-worlds-shrunk-together.md).
  Ran the test at HEAD; probed the id collision by hand before `History` was
  added.
Existing check: `crates/eval-core/src/shrink.rs` `History`, `Scenario::without`,
  `Scenario::compile`, `Driver::replay_candidate`;
  `crates/eval-core/src/pairs.rs` `compile_pair_set`; tests above.
Impact: A carried-over mapping would let a candidate keep a fresh arm that no
  longer follows from its aged log, and the minimized witness would replay a
  pair the compiler would refuse.
Open questions:
- When an aged-only event is the sole cause, the natural-fresh history shrinks
  independently to whatever the compiler still admits; no rule pins it to the
  natural-fresh prefix. (needs human input)

### flt-hard-case-carries-witness-triple

Type: safety
Reachability: test-only - `crates/daemon/tests/eval_fault.rs` and the
  `eval_runner` fault shell
Status: active
Exercised: partial -
  `crates/eval-core/tests/fault.rs::a_missing_receipt_is_incomplete_coverage_not_pass`
  (coverage) and
  `crates/daemon/tests/eval_fault.rs::liveness_bounds_are_met_with_outside_core_faults_armed`
  (progress); the safety count is refused by `FaultReport::validate`
  (`SafetyNeverChecked`); the three are checked per report, not per hard case.
Guarantee: Every hard case carries a coverage witness, a safety check evaluated
  while faults are armed, and a bounded healthy-progress check; a missing
  coverage entry is `IncompleteCoverage`, never `Pass`.
Check: `always` - for every fault report, `cuts` receipts every declared cut
  and `safety_checks_while_armed > 0`, both refused by `FaultReport::validate`
  when missing; `liveness.verdict(bounds)` is met when the member is present.
  `FaultReport::validate` accepts `liveness: None` (`if let Some(liveness)`;
  `crates/eval-core/tests/fault.rs` validates a `liveness_less` report), so a
  missing progress member is not refused by validation: the drive asserts it
  per campaign, not the report. The three are report-wide today, so the check
  is per report, not per hard case.
Fault/timing angle: The safety check runs while a fault is armed; a check that
  only runs after healing proves nothing about the armed window.
Required faults and enabling state: A fault campaign with declared cuts, at
  least one armed episode, and the liveness lanes declared.
Confidence: medium -
  [evidence](evidence/flt-hard-case-carries-witness-triple.md). Read
  `FaultReport::validate` and the fault tests at HEAD; the triple is not grouped
  per hard case.
Existing check: `crates/eval-core/src/fault.rs` `FaultReport::validate`
  (`SafetyNeverChecked`, cut receipts), `LivenessReport::verdict`;
  `crates/eval-core/src/markers.rs` `Coverage::complete`; tests above.
Impact: A hard case that was never actually reached, never checked while armed,
  or never made progress would be reported as passing.
Open questions:
- Is the safety check owed per hard case or shared per checkpoint? Today it is
  shared per report. (needs human input)
- Should `FaultReport::validate` require `liveness` once every campaign
  declares its lanes? Today it accepts `None`, so the progress member of the
  triple is asserted by the drive, not refused by the report. (needs human
  input)

### rid-replay-equality-semantic-trace-digest

Type: safety
Reachability: test-only - `crates/eval-core/tests/two_process.rs` and
  `crates/daemon/tests/eval_shrink.rs`
Status: active
Exercised: yes -
  `crates/eval-core/tests/two_process.rs::two_process_same_identity_yields_equal_manifest_and_trace_digests`,
  `crates/eval-core/tests/two_process.rs::two_process_planted_map_order_leak_fails_the_equality_test`,
  `crates/daemon/tests/eval_shrink.rs::a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`
Guarantee: Replay equality is equality of a canonical semantic trace digest over
  declared `Keep` fields, never of log bytes; the same inputs in two fresh OS
  processes yield equal digests, and a `Keep` field perturbed or a hash-order
  leak yields different ones.
Check: `always` - two fresh processes over the same identity print equal
  manifest and trace digests; two fresh processes over the same scenario and
  oracle print equal `Replayed` values; the original scenario replays to the
  trace digest the witness recorded; a planted `HashMap` iteration in an
  observation makes the two-process equality fail. Must hold on every replay, so
  `always`.
Fault/timing angle: Process identity, pids, and wall clock differ between the
  two processes and are classified `Drop` by the observation schema.
Required faults and enabling state: Two fresh processes over one input; a third
  with a `Drop`-classified field perturbed or an order leak planted.
Confidence: high -
  [evidence](evidence/rid-replay-equality-semantic-trace-digest.md). Ran both
  tests at HEAD. The shrink replay's trace keeps only the scenario digest and
  the outcome, so its digest agreement follows from outcome agreement; the
  two-process manifest test is the discriminating evidence for the digest
  itself.
Existing check: `crates/eval-core/src/residue.rs` `SemanticTrace::digest`;
  `crates/daemon/examples/eval_runner/shrink.rs` `replay_schema` and
  `child_main`; tests above.
Impact: A replay would be declared equal on byte-identical logs that differ
  semantically, or unequal on semantically identical runs whose pids differ.
Open questions: None.

### mtr-method-records-cite-executed-check

Type: safety
Reachability: test-only - `crates/eval-core/tests/method_records.rs` over the
  committed catalog
Status: active
Exercised: yes -
  `crates/eval-core/tests/method_records.rs::every_evaluator_record_is_method_ordered_and_cites_an_executed_check`
Guarantee: Every evaluator property record uses the METHOD fields in order with
  one of the five check semantics, every `Exercised: yes|partial` names an
  un-ignored `#[test]` that exists in the workspace, and every record has an
  evidence file; a record that fails any of these is refused by a test that
  reads the catalog.
Check: `always` - the test splits `catalog.md` on `### ` headings, joins wrapped
  continuation lines, asserts the twelve field heads in METHOD order, a
  semantics token in the closed set, a reachability class with its entry point,
  a confidence level, every backticked `path::test` under `Exercised:
  yes|partial` resolving to a file whose `fn test` carries `#[test]` and no
  `#[ignore`, `evidence/<slug>.md` with the METHOD headings, and index rows
  equal to the record set.
Fault/timing angle: None; the check is mechanical over committed documents.
Required faults and enabling state: The catalog and evidence files committed at
  HEAD.
Confidence: high -
  [evidence](evidence/mtr-method-records-cite-executed-check.md). Ran the test
  at HEAD.
Existing check: `crates/eval-core/tests/method_records.rs` reads
  `docs/properties/evaluator/catalog.md` through a `CARGO_MANIFEST_DIR`-relative
  path; precedent `crates/kernel/tests/search_projection_construction_inputs.rs`
  reads `witness-matrix.md`.
Impact: A record could claim exercise by a test that was renamed, deleted, or
  ignored, and the catalog would overstate the evidence.
Open questions: None.

### wit-package-carries-original-and-minimized

Type: safety
Reachability: test-only - `crates/eval-core/tests/` integration tests over the
  sans-I/O crate
Status: active
Exercised: yes -
  `crates/eval-core/tests/witness.rs::the_package_round_trips_and_carries_the_recipe_for_a_count_triggered_failure`,
  `crates/eval-core/tests/witness.rs::every_structural_refusal_names_its_cause`,
  `crates/eval-core/tests/witness.rs::one_minimality_needs_a_rejected_record_for_every_single_deletion`,
  `crates/eval-core/tests/witness.rs::the_coverage_signature_names_only_registered_markers`,
  `crates/eval-core/tests/witness.rs::a_multiplicity_record_counts_only_under_its_own_scenario_digest`,
  `crates/eval-core/tests/witness.rs::a_one_minimal_claim_names_exactly_the_transformations_the_scenario_held`,
  `crates/eval-core/tests/witness.rs::the_remaining_count_is_the_minimized_scenario_s_element_count`,
  `crates/eval-core/tests/witness.rs::a_residue_that_no_schema_could_declare_is_refused`,
  `crates/eval-core/tests/witness.rs::a_minimized_scenario_that_cannot_compile_is_refused`,
  `crates/eval-core/tests/witness.rs::a_deleted_element_cannot_also_survive`,
  `crates/eval-core/tests/witness.rs::the_failure_predicate_names_the_task_the_replay_evaluates`,
  `crates/eval-core/tests/witness.rs::the_recipe_regenerates_the_original_tape_too`,
  `crates/eval-core/tests/witness.rs::the_recipe_regenerates_the_original_causal_trace_too`,
  and
  `crates/daemon/tests/eval_shrink.rs::a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`
Guarantee: A witness package carries the original failure (RunId, decision tape,
  canonical trace digest, causal trace, oracle and checkpoint, coverage
  signature), the minimized semantic scenario, the shrink report, and a compact
  multiplicity recipe required when the scenario is 1-minimal and a count
  triggers the failure; the recipe regenerates the minimized logs.
Check: `always` - `WitnessPackage::validate` refuses a recipe missing when
  required (`RecipeRequired`), present without a count-triggered kind or
  without established minimality (`RecipeWithoutMultiplicity`), counting other kinds
  (`RecipeMultiplicitiesDisagree`), or not regenerating the minimized logs
  (`RecipeDisagrees { history }`); a kind is count-triggered when more than one
  aged event of it survives and each one's single deletion was recorded
  `Slipped` or `NotReproduced` under the deleted scenario's digest; a `OneMinimal` claim without a rejected
  record, under the digest of the scenario that deletion produces, for some
  single deletion from the minimized scenario is refused
  (`MinimalityUnsupported { element }`); a claim naming other
  transformations than the original held elements for is refused
  (`TransformationsDisagree { expected }`); a `remaining` count other than
  the minimized scenario's element count is refused
  (`ShrinkReport(Inconsistent { remaining })`); a residue holding a `Keep`
  rule or two rules for one field is refused (`ResidueContradiction`); a
  minimized scenario the pair compiler or episode validator refuses is
  refused (`MinimizedNotReplayable`); a failure predicate naming another task
  than the first pair's is refused (`PredicateNamesAnotherTask`); a deleted
  element the minimized scenario still holds is refused
  (`ShrinkReport(Inconsistent { deleted })`); the aged generation must
  regenerate the original's decision tape and causal trace
  (`RecipeDisagrees`); a
  coverage name outside the registry is refused (`UnregisteredMarker { name }`); an embedded report that
  `ShrinkReport::validate` refuses is refused (`ShrinkReport(..)`);
  `parse_witness(serialize(package)) == package`; the published bytes parse
  back to the run's package.
Fault/timing angle: None.
Required faults and enabling state: A shrink whose minimized scenario keeps six
  commits, and a recipe with a wrong seed, an oversized declared world, or a
  wrong count.
Confidence: high -
  [evidence](evidence/wit-package-carries-original-and-minimized.md). Ran both
  tests at HEAD.
Existing check: `crates/eval-core/src/witness.rs` `WitnessPackage`,
  `OriginalFailure`, `MultiplicityRecipe`, `check_recipe`, `count_triggered`,
  `regenerates`, `check_minimality`, `parse_witness`; shell `run` assembles
  and publishes it.
Impact: A witness without its original identity or with a recipe that
  regenerates a different world could not be replayed against the failure it
  claims.
Open questions: None.

### wit-live-evidence-never-replayable

Type: safety
Reachability: test-only - `crates/eval-core/tests/` integration tests over the
  sans-I/O crate
Status: active
Exercised: yes -
  `crates/eval-core/tests/witness.rs::live_model_evidence_is_never_relabelled_replayable`
Guarantee: A witness whose slice is live is never labelled replayable; the
  serializer refuses the relabel.
Check: `always` - `WitnessPackage::validate` and `serialize` return
  `LiveRelabelledReplayable` whenever `slice == Live && replayable`. Must hold
  on every package, so `always`.
Fault/timing angle: None.
Required faults and enabling state: A package with `slice: live` and
  `replayable: true`.
Confidence: high - [evidence](evidence/wit-live-evidence-never-replayable.md).
  Ran the test at HEAD through both `validate` and `serialize`.
Existing check: `crates/eval-core/src/witness.rs` `validate`;
  `crates/eval-core/src/failure_class.rs` `Slice`.
Impact: Live-model evidence would be presented as deterministic replay evidence,
  which the claim boundary excludes.
Open questions:
- Should the shell refuse to shrink a live-slice failure at all, rather than
  package it as non-replayable? (needs human input)

### wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims

Type: safety
Reachability: test-only - `crates/eval-core/tests/` integration tests over the
  sans-I/O crate
Status: active
Exercised: yes -
  `crates/eval-core/tests/witness.rs::the_serializer_requires_the_verbatim_claim_boundary_and_rejects_forbidden_claims`
Guarantee: The shared serializer refuses a witness whose `claim_boundary` is not
  the verbatim `claim-boundary/v1` block and one whose free text names an
  excluded claim outside that block.
Check: `always` - `serialize` returns `ClaimBoundaryMismatch` when
  `claim_boundary != ClaimBoundary::pinned()` and `ForbiddenClaim { path, phrase
  }` when any string leaf outside the root `claim_boundary` key contains one of
  `CLAIM_BOUNDARY_EXCLUSIONS` after ASCII lowercasing, naming the JSON path with
  array indices. Must hold on every package, so `always`.
Fault/timing angle: None.
Required faults and enabling state: A package with a dropped exclusion; a
  package whose oracle name says "live-model quality"; a capitalized phrase in
  an array leaf.
Confidence: high -
  [evidence](evidence/wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims.md).
  Ran the test at HEAD.
Existing check: `crates/eval-core/src/witness.rs` `validate` and `check_claims`;
  `crates/eval-core/src/report.rs` `check_claims` pins the boundary block for
  Suite B reports (it does not scan phrases).
Impact: A witness could claim scheduler-order independence or live-model quality
  in prose while carrying the boundary block that excludes them.
Open questions:
- The match is an ASCII-lowercased substring; a phrase split by a different
  separator or written with non-ASCII letters passes. (needs human input)

### wit-residue-drift-refuses

Type: safety
Reachability: test-only - `crates/eval-core/tests/witness.rs` and
  `crates/daemon/tests/eval_shrink.rs`
Status: active
Exercised: yes -
  `crates/eval-core/tests/witness.rs::residue_drift_refuses_and_limits_apply_before_publication`,
  `crates/daemon/tests/eval_shrink.rs::a_child_whose_residue_drifted_refuses_the_run`,
  and
  `crates/daemon/tests/eval_shrink.rs::a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`
Guarantee: A replaying process whose declared residue rules differ from the
  witness's recorded residue is refused with the missing and unexpected entries;
  the redaction scan and the artifact byte bound apply before a byte is
  published.
Check: `always` - `residue_drift` returns `ResidueDrift { missing, unexpected }`
  on any difference; the shell refuses the run when a child's reported residue
  differs and publishes nothing; `serialize` refuses `TooLarge` above the bound
  and `RedactionRefused` on a detection. Must hold on every replay and every
  publication, so `always`.
Fault/timing angle: A build whose observation schema reclassifies a field
  between the recording and the replay.
Required faults and enabling state: A child that reports one residue entry
  fewer; a byte bound below the package size; a planted key-shaped token in a
  free-text field.
Confidence: high - [evidence](evidence/wit-residue-drift-refuses.md). Ran the
  three tests at HEAD; the drifting child is a re-executed entrypoint that drops
  one entry.
Existing check: `crates/eval-core/src/witness.rs` `residue_drift`,
  `serialize`; shell `Replayer::replay` refuses drift;
  `crates/eval-core/src/cassette.rs` `scan_for_secrets`.
Impact: A replay under different residue rules would compare digests computed
  over different fields and report drift or agreement for the wrong reason; a
  leaked credential would be published.
Open questions: None.

### ing-adapter-ingested-no-production-caller

Type: reachability
Reachability: test-only - `crates/eval-core/tests/manifest.rs` and every
  `eval_runner` shell manifest
Status: active
Exercised: partial -
  `crates/eval-core/tests/manifest.rs::manifest_consistency_refusals_name_their_cause`
  pins the wire name `adapter-ingested, production caller: none`; no production
  caller exists to exercise, so the gap is recorded, not closed.
Guarantee: Every manifest names how its worlds were ingested; no world is
  labelled "validated real ingestion" until an ingestion entry point has a
  production caller.
Check: `always` - every manifest carries `ingestion`; every Suite C shell
  manifest at HEAD carries `adapter-ingested, production caller: none` and the
  Suite B shell's carries `transform-route, turn by turn`, the daemon's own
  route; no manifest claims validated real ingestion until a production caller
  exists.
Fault/timing angle: None.
Required faults and enabling state: A manifest; the ingestion label.
Confidence: medium -
  [evidence](evidence/ing-adapter-ingested-no-production-caller.md). Read the
  manifest field and the README gap at HEAD; the gap remains open.
Existing check: `crates/eval-core/src/manifest.rs`
  `Ingestion::AdapterIngestedNoProductionCaller`; `README.md` "Gaps recorded
  here".
Impact: A reader would take generated-world results as evidence about a
  production ingestion path that does not exist.
Open questions:
- Which adapter becomes production-live first, and when, is outside this
  specification. (needs human input)

### ing-pi-adapter-unexercised-by-evaluator

Type: reachability
Reachability: test-only - no evaluator entry point reaches
  `crates/daemon/src/harness_sources.rs` `pi_units`
Status: active
Exercised: not yet - the Pi adapter (`crates/daemon/src/harness_sources.rs`
  `pi_units`) has no evaluator arm; no campaign renders a Pi session, so no
  executed check exists.
Guarantee: The evaluator states which harness adapters its worlds exercise; the
  Pi adapter is not among them until a campaign renders Pi sessions and accounts
  `expected == published + refused` for them.
Check: `reachable` - a campaign whose rendered world reaches `pi_units` and
  records the adapter's expected-versus-published accounting; today no such
  location is executed, so the gap is recorded rather than claimed.
Fault/timing angle: None.
Required faults and enabling state: A Pi session fixture rendered by the
  evaluator and ingested through `pi_units`.
Confidence: medium -
  [evidence](evidence/ing-pi-adapter-unexercised-by-evaluator.md). Searched the
  shells and tests at HEAD for a Pi arm; none exists.
Existing check: none
Impact: A reader would assume the evaluator's ingestion evidence covers every
  supported harness when it covers the OpenCode-shaped session only.
Open questions:
- Whether a Pi arm belongs to a later phase or is out of scope. (needs human
  input)

### flt-coverage-witnesses-fire-only-on-observed-behaviour

Type: safety
Reachability: test-only - `crates/daemon/tests/eval_shrink.rs` and the
  `eval_runner` example under the `eval-runner` feature
Status: active
Exercised: yes -
  `crates/daemon/tests/eval_shrink.rs::a_fresh_process_reproduces_the_predicate_and_the_minimized_witness_is_published`,
  `crates/daemon/tests/eval_shrink.rs::a_child_that_dies_before_its_barrier_is_retried_then_unknown_and_kept`,
  `crates/eval-core/tests/render.rs::the_marker_registry_is_unique_and_incomplete_until_every_marker_fires`
Guarantee: A coverage marker is recorded only after the behaviour it names was
  observed in the run, its name is registered and globally unique, and the
  witness's coverage signature is the set of fired markers.
Check: `always` - `flt_shrink_slipped_candidate_rejected` fires only when a
  `Slipped` record exists, `flt_shrink_unknown_effect_preserved` only when
  `unknown_candidates > 0`, `flt_shrink_fresh_process_reproduced` only after the
  original replayed `Failed`; a run without unknown candidates does not fire the
  unknown marker; the dying-child run passes `Coverage::complete` for the
  suite, so every marker the suite owns fired; `Coverage::record` refuses an
  unregistered name; the registry has no duplicate names.
Fault/timing angle: None.
Required faults and enabling state: A shrink run with and without slipped and
  unknown candidates.
Confidence: high -
  [evidence](evidence/flt-coverage-witnesses-fire-only-on-observed-behaviour.md).
  Ran the shell tests at HEAD; each asserts a marker present and one absent.
Existing check: `crates/eval-core/src/markers.rs` `MARKERS`, `Coverage::record`;
  `crates/daemon/examples/eval_runner/shrink.rs` `run` records the three shrink
  markers; `crates/eval-core/tests/render.rs` asserts name uniqueness.
Impact: A marker fired unconditionally would report coverage of a behaviour the
  run never showed.
Open questions: None.

## Relationship map

- `flt-shrink-preserves-precise-failure-predicate` depends on
  `flt-paired-worlds-shrunk-together` (a candidate is judged only after it
  compiles) and on `flt-shrinker-unknown-never-not-reproduced` (an unanswered
  replay is not a rejection).
- `wit-package-carries-original-and-minimized` carries the shrink report the
  first three records describe and the trace digest
  `rid-replay-equality-semantic-trace-digest` compares.
- `wit-residue-drift-refuses`, `wit-live-evidence-never-replayable`, and
  `wit-serializer-requires-claim-boundary-and-refuses-forbidden-claims` are the
  serializer's refusals over that package.
- `flt-coverage-witnesses-fire-only-on-observed-behaviour` supplies the
  package's coverage signature; `flt-hard-case-carries-witness-triple` is the
  Phase 4 counterpart it extends.
- `mtr-method-records-cite-executed-check` is the check over this file.
- `ing-adapter-ingested-no-production-caller` and
  `ing-pi-adapter-unexercised-by-evaluator` bound what every witness may claim
  about ingestion.
