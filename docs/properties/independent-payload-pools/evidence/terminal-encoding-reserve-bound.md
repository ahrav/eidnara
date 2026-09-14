# terminal-encoding-reserve-bound

## Discovery trigger

The worst-escaped 128-byte code, 4,096-byte message, and maximum retry hint fit one 32 KiB terminal block unchanged, and dedicated encoding bytes never consume the maximum-frame egress floor. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/tests/contract.rs:217`
- `crates/host-runtime/src/dispatch.rs:71`

Witness status: partial - `crates/shm-transport/tests/contract.rs:187` checks the 32 KiB terminal block holds 25,406 body and 25,427 frame bytes; the serializer-level fit is #548's.

## Failure scenario

A terminal that does not fit its reserve is truncated or replaced, hiding the real error.

## Timing windows and dependencies

None; boundary arithmetic.

## What a test must construct

The maximum-length worst-escaped terminal serialized with ordinary egress exhausted.

Situation markers that must fire independently of the safety check:

- `host.terminal_worst_case_serialized`

Check semantics: `always` - `terminal.body_capacity() >= 25_406` and the serializer output for the worst case is `<= 25_406` bytes.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548.
- Conclusion: unresolved, needs the named handoff.
