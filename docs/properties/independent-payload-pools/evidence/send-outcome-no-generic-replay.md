# send-outcome-no-generic-replay

## Discovery trigger

Failure before publication is `not_sent`; failure after publication is `outcome_unknown`; no layer replays an uncertain request (R7). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/host-runtime/src/ring_transport.rs:967`
- `crates/shm-transport/src/backend/ring.rs:1007`

Witness status: partial - `crates/host-runtime/src/ring_transport.rs:1030` classifies `Deadline`/`Unreserved` as zero-byte and `Reserved` as unknown; `a_client_send_past_its_frame_deadline_publishes_nothing` in crates/host-runtime/src/ring_transport.rs. Stop/restart witnesses belong to #548, #552, #550.

## Failure scenario

A replayed uncertain request executes twice.

## Timing windows and dependencies

Quarantine between the pre-commit check and `commit`.

## What a test must construct

A quarantine landing after `write` and before `commit`.

Situation markers that must fire independently of the safety check:

- `host.quarantine_between_write_and_commit`

Check semantics: `always` - every `SendFailure` maps to exactly one of the two outcomes and no code path resubmits a frame after `commit` returned `Err`.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: partial at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548, #552, #550.
- Conclusion: unresolved, needs the named handoff.
