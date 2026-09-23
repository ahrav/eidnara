# mtr-judge-calls-blinded-order-swapped-versioned

## Discovery trigger
Issue #768 acceptance criteria: "Freeze a human-anchor calibration set,
rubric, judge identity, and sampling plan before judging. Blind prompts with
canary scans and arm-identification permutation checks; execute both
presentation orders and record per-pair lengths to expose length leakage."
and "Wrong-arm leakage and omitted order-swap fixtures fail the blinding
checks."

## Evidence trail
- `crates/eval-core/src/judge.rs` `screen` and `blind`: a non-empty canary
  found in either response is `CanaryInPrompt`; an `ARM_TOKENS` entry matched
  as whole words after `fold_words` is `ArmIdentifiable`; `BlindedPrompt`
  serializes `first` and `second` only.
- `crates/eval-core/src/judge.rs` `judge_pairs`: refuses a malformed judge
  digest, a duplicated pair, an unblinded pair, an unknown pair, another
  judge, and a second call for one pair and order; requires both orders and
  unswaps them; disagreement is `Preference::Inconsistent`; lengths are the
  byte lengths of `a` and `b`.
- `crates/eval-core/src/judge.rs` `PermutationCheck::validate`: refuses when
  `max(correct, trials - correct) * 100 > trials * 60`.
- `crates/eval-core/src/judge.rs` `CalibrationSet::validate`: refuses a schema
  other than `eval-judge/v1`, a digest that is not 64 lowercase hex, an empty
  label map, and an `Inconsistent` human label.
- `crates/eval-core/tests/judge.rs`: the seven cited tests. `rename` as a
  canary and `as the Fresh Arm I answer` refuse; `harm bound`, `warm bread`,
  `firearm` do not; first-in-both-orders is `Inconsistent`; one order is
  `OrderMissing`; a rerolled `BThenA` call is `DuplicateCall`; 25 of 40 and
  15 of 40 refuse, 24 and 16 do not.

## Failure scenario
A response that names its arm, or a judge asked in one order only, lets
position or vocabulary decide the residual instead of the content.

## Timing windows and dependencies
None.

## What a test must construct
A pair with an arm name or canary; call sets missing an order, repeating an
order, or under two judges; permutation counts on both sides of the ceiling.

## Investigation log
### Q: Is the ceiling pre-registered?
- Sources examined: `ARM_IDENTIFICATION_CEILING_PERCENT`, the pull request
  description for #806.
- Findings: 60 percent is a stand-in; no trial-count floor exists, so a
  two-trial check validates.
- Missing evidence: an approved bound.
- Conclusion: unresolved, needs human input.
### Q: Does the token screen cover role descriptions such as `aged answer`?
- Sources examined: `ARM_TOKENS`, `fold_words`, `contains_words`.
- Findings: only six tokens, all naming an arm; natural language is left to
  the permutation check.
- Missing evidence: none; the design is recorded as an open question.
- Conclusion: resolved with answer - no, by design.
