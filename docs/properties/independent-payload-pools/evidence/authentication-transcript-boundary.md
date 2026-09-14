# authentication-transcript-boundary

## Discovery trigger

Grants are decoded only after the authenticated transcript verifies, and the grant carries no profile id of its own: the profile is checked by the setup layer before any grant byte is interpreted. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/shm-native/src/setup.rs:116`
- `crates/shm-transport/src/backend/ring.rs:286`

Witness status: yes - `crates/shm-transport/src/setup_auth.rs` vector tests and `crates/host-runtime/src/setup_socket.rs` activation tests; carried forward unchanged from the host-runtime setup-identity catalog.

## Failure scenario

A grant decoded before authentication would let an unauthenticated peer drive geometry validation.

## Timing windows and dependencies

None new; the transcript is unchanged by this replacement.

## What a test must construct

A proof mismatch before the grant message.

Situation markers that must fire independently of the safety check:

- `setup.proof_mismatch_before_grant`

Check semantics: `always` - `begin_connect` and `run_connection` verify the proof before `receive_grant`, and `PoolGrant` has no profile field.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
