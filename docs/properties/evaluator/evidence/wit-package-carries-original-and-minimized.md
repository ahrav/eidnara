# wit-package-carries-original-and-minimized

## Discovery trigger
Parent specification Implementation Decisions: "Witness package: original
failure (RunId, decision tape, canonical trace digest, causal trace, oracle
and checkpoint, coverage signature), minimized semantic scenario, and a
compact recipe form when multiplicity is the trigger."

## Evidence trail
- `crates/eval-core/src/witness.rs:30` `OriginalFailure`; `:62`
  `WitnessPackage`; `:149` `check_recipe` regenerates both worlds through
  `generate_all` and applies the deletions.
- `crates/eval-core/tests/witness.rs:201` round trip, `RecipeRequired`, `RecipeDisagrees`, and a refused
  extra field.
- `crates/daemon/tests/eval_shrink.rs:175` the published `witness.json` parses back to the run's
  package and the manifest's `witness_digest` is its protocol digest.

## Failure scenario
A recipe recorded with the wrong seed regenerates a different aged history;
a reader following it reproduces nothing.

## Timing windows and dependencies
None.

## What a test must construct
A count-triggered minimized scenario and a tampered recipe.

## Investigation log
### Q: When is a recipe required?
- Sources examined: `check_recipe`.
- Findings: when minimality is `OneMinimal` and a payload kind survives more
  than once in the aged log; `InvalidPair` can also force events to survive,
  so the rule is over-inclusive and the form is always valid.
- Missing evidence: none.
- Conclusion: resolved with answer - over-inclusive by design.
