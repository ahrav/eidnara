# native-alias-closure-before-transfer

## Discovery trigger

Every JavaScript alias of a block detaches before the block is published or returned; a failed detach quarantines the direction and retains the alias record (R9, KTD6). The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/shm-native/src/lib.rs:356`
- `packages/shm-native/src/napi_buffers.rs:149`
- `packages/shm-native/src/napi_buffers.rs:248`
- `packages/shm-native/tests/runtime.ts:135`

Witness status: yes - producer aliases detach before commit (`packages/shm-native/src/lib.rs:383`) and consumer aliases detach before return (`packages/shm-native/src/lib.rs:356`); `runNativeLifecycle` in packages/shm-native/tests/runtime.ts asserts subarray, DataView, and Buffer aliases read zero after release, but only when the runtime reports the detachment capability; `injected detach and deletion failures quarantine the backing and conserve tokens` in packages/shm-native/tests/mechanism.ts runs against the raw addon on every runtime and shows a refused detach leaving the alias attached, the token registered, the block held, and the ring quarantined, with a cooperating retry detaching and returning exactly once.

## Failure scenario

A live alias after return would let JavaScript write a block the peer has reused.

## Timing windows and dependencies

`napi_detach_arraybuffer` failure; alias survivors through `subarray`/`DataView`.

## What a test must construct

An external-view failpoint firing on detach; a `subarray` created before release.

Situation markers that must fire independently of the safety check:

- `native.detach_failed`
- `native.alias_survivor_before_return`

Check semantics: `always` - `commit_reservation` and `release` reach the ring only after `detach_all` succeeded; a detach failure calls `enter_quarantine` and keeps the entry.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
