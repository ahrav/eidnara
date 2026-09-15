# integration-gate-dependency-selection

## Discovery trigger

Every CI gate is selected by the inputs it consumes: host-runtime changes select the native/E2E job, and the protocol document selects the transport gates that read it. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `.github/workflows/ci.yml:95`
- `crates/shm-transport/tests/contract.rs:366`

Witness status: yes - `.github/workflows/ci.yml:95` adds host-runtime to the `native` filter and `.github/workflows/ci.yml:71` adds the protocol document, which `crates/shm-transport/tests/contract.rs:364` reads.

## Failure scenario

A change to a consumed input that skips its gate is an untested change.

## Timing windows and dependencies

None.

## What a test must construct

A host-runtime-only pull request.

Situation markers that must fire independently of the safety check:

- `gate.host_runtime_only_change`

Check semantics: `always` - each path a gate's tests read appears in that gate's filter.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
