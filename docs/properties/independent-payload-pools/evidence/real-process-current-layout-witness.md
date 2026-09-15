# real-process-current-layout-witness

## Discovery trigger

Every required real-process witness records current-layout activation and a completed daemon request on a supported runtime; a skip discharges nothing. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/tests/ring.rs:429`
- `packages/e2e-tests/src/rust-runner/hermetic-host.ts:275`

Witness status: partial - `crates/shm-transport/tests/ring.rs:347` records a completed real cross-process exchange at layout 4; the direct-host E2E and native suites still skip on Bun 1.3.14 (`markAsUntransferable` unimplemented) and on Node (`node_detachment_unavailable`), which are recorded limitations, not passes.

## Failure scenario

A skipped suite counted as passing hides an unexercised layout.

## Timing windows and dependencies

None.

## What a test must construct

A supported runtime.

Situation markers that must fire independently of the safety check:

- `gate.supported_runtime_present`

Check semantics: `reachable` - the code point that reports a completed real exchange (`EIDNARA_SHM_CHILD_DONE`) executes in the two-process job; `reachable` because a specific code location must execute.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548, #552, #550 add daemon-level real-process witnesses.
- Conclusion: unresolved, needs the named handoff.
