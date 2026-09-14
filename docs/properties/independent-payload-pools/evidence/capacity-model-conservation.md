# capacity-model-conservation

## Discovery trigger

The capacity model conserves leases across every scenario, classifies refusals disjointly, and drains completely; its results are sizing input, not performance evidence. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `docs/properties/independent-payload-pools/capacity-model/simulate.py`
- `docs/properties/independent-payload-pools/capacity-model/results.md`

Witness status: yes - `python3 docs/properties/independent-payload-pools/capacity-model/simulate.py` passes its self-checks and prints the ten recorded scenarios; the outputs are recorded in `capacity-model/results.md` and labeled uncalibrated.

## Failure scenario

A model that loses leases misleads the initial sizing.

## Timing windows and dependencies

None.

## What a test must construct

A model run.

Situation markers that must fire independently of the safety check:

- `model.self_checks_run`

Check semantics: `always` - self-checks pass and every scenario reports `unfinished_after_drain == 0` and `descriptor_refusals + sum(class_refusals) == would_block`.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
