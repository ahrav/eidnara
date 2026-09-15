# validated-setup-geometry

## Discovery trigger

Every block offset and capacity comes from geometry both peers validated; the grant's total must equal the computed layout; unsealed, resized, or non-regular objects are refused before mapping (KTD2). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `crates/shm-transport/src/backend/ring.rs:321`
- `crates/shm-transport/src/backend/retained.rs:766`
- `crates/shm-transport/src/backend/retained.rs:293`

Witness status: yes - `crates/shm-transport/src/backend/ring.rs:2761`, `crates/shm-transport/tests/ring.rs:103`, `crates/shm-transport/src/pool.rs:598`, and `crates/shm-transport/tests/fuzz_corpus.rs:92`.

## Failure scenario

A peer-supplied offset or size would let a forged descriptor address bytes outside its block.

## Timing windows and dependencies

Mutated grant fields; an unsealed or wrongly sized object; a lifecycle page that disagrees with the grant.

## What a test must construct

Each grant field mutated one at a time; an unsealed memfd of the right size; a lifecycle lane rewritten after creation.

Situation markers that must fire independently of the safety check:

- `setup.grant_field_mutated`
- `setup.unsealed_object_presented`

Check semantics: `always` - `PoolGrant::decode` accepts only a geometry `PoolGeometry::new` accepts whose layout total matches, and `Ring::attach` refuses a mapping whose lifecycle page disagrees with the grant in any field.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
