# partial-close-token-conservation

## Discovery trigger

Partial close sweeps, reentrant callbacks, repeated release/close, and environment termination conserve tokens and one-shot return authority; uncertain aliases quarantine the owning backing once. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/shm-native/src/lib.rs:408`
- `packages/shm-native/src/lib.rs:1729`
- `packages/shm-native/src/napi_buffers.rs:252`

Witness status: yes - `packages/shm-native/src/lib.rs:408` sweeps every alias and reports the first failure; `finish_close` retains alias-holding channels; `injected detach and deletion failures quarantine the backing and conserve tokens` in packages/shm-native/tests/mechanism.ts injects a detach failure into a two-lease close sweep and shows exactly one alias surviving, the channel entry retained with its mapping, and a later close completing the sweep and removing it; a deletion failure after a successful detach consumes the token (the wrapper is told), keeps the leaked reference counted, and quarantines the ring; repeated release and close are covered by the neighboring tests.

## Failure scenario

A lost token strands a block; a double return frees a newer occupant.

## Timing windows and dependencies

A detach failure mid-sweep; a callback that closes the channel reentrantly.

## What a test must construct

Injected detach failure on the second of three aliases; a `deliver` callback calling `close`.

Situation markers that must fire independently of the safety check:

- `native.partial_sweep_failure`
- `native.reentrant_close`

Check semantics: `always` - every token is released or retained exactly once across a partial sweep, and a channel with any alias outstanding is never removed from the registry.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
