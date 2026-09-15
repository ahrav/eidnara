# malformed-fixture-valid-baseline

## Discovery trigger

Every malformed-input fixture passes unmutated under the current layout before its mutation cases count. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/shm-native/src/lib.rs:277`
- `crates/shm-transport/tests/fuzz_corpus.rs:60`

Witness status: yes - `packages/shm-native/tests/mechanism.ts:1198` proves the unmutated fixture decodes before mutation cases; `crates/shm-transport/tests/fuzz_corpus.rs:78` asserts each `valid` seed is accepted.

## Failure scenario

A stale fixture makes every rejection assertion pass for the wrong reason.

## Timing windows and dependencies

None.

## What a test must construct

A fixture built from the current geometry.

Situation markers that must fire independently of the safety check:

- `gate.fixture_baseline_checked`

Check semantics: `always` - `grantDecodes(fixture) == true` and each corpus `valid` seed is accepted.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
