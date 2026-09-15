# unsafe-witness-selection

## Discovery trigger

Each required unsafe witness is individually selected and reported by the Miri and Valgrind gates; two-process witnesses run separately; a nonzero aggregate count discharges nothing. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `.github/workflows/ci.yml:642`
- `.github/workflows/ci.yml:734`

Witness status: yes - the Miri step greps each required witness by name (`.github/workflows/ci.yml:653`), the Valgrind step likewise (`.github/workflows/ci.yml:701`), and the `two-process` job runs the child-process witnesses without the memcheck runner.

## Failure scenario

An empty gate passes while proving nothing.

## Timing windows and dependencies

A module move that drops a witness from the prefix filter.

## What a test must construct

A renamed or moved witness.

Situation markers that must fire independently of the safety check:

- `gate.witness_renamed`

Check semantics: `always` - the CI logs contain `test <witness> ... ok` for every listed witness.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
