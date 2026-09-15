# fuzz-adapter-current-contract

## Discovery trigger

Fuzz adapters decode the current descriptor, grant, and completion encodings, accept at least one valid seed each, and no dormant old reader remains. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/fuzz/fuzz_targets/{pool_descriptor,provider_grant,payload_completion}.rs`
- `crates/shm-transport/tests/fuzz_corpus.rs:80`

Witness status: yes - `crates/shm-transport/src/harness.rs:31`, `crates/shm-transport/src/harness.rs:79`, and `provider_grant` decode the current contract; the corpus `valid` seeds decode and the dormant sample reader is deleted.

## Failure scenario

A fuzz target that rejects everything finds nothing.

## Timing windows and dependencies

None.

## What a test must construct

A corpus replay.

Situation markers that must fire independently of the safety check:

- `gate.fuzz_corpus_replayed`

Check semantics: `always` - `cargo check --bins` of the fuzz workspace succeeds and every `valid` seed is accepted.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
