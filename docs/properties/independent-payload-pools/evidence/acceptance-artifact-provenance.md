# acceptance-artifact-provenance

## Discovery trigger

Acceptance runs load a wrapper and addon built from the tested source at the current layout. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- `packages/shm-native/tests/mechanism.ts:57`
- `.github/workflows/ci.yml:785`
- `packages/shm-native/index.ts:38`

Witness status: yes - the native job builds the addon from source before every test run (`.github/workflows/ci.yml:785`); `nativeWireConstants` (`packages/shm-native/index.ts:12`) compares the loaded addon's identifiers with the wrapper's; `nativeArtifactIdentity` (`packages/shm-native/index.ts:38`) reports the loaded addon's build profile, target, N-API version, schema, and profile, and the capability witness (`packages/shm-native/tests/capability.ts:20`) records them beside the runtime's own version on every run and asserts them under `EIDNARA_SHM_NATIVE_CLAIMED_TARGET=1`, so a recorded limitation names the exact artifact and runtime it applies to.

## Failure scenario

A stale artifact tests the old layout while reporting the new one.

## Timing windows and dependencies

A stale prebuilt `shm_native.node`.

## What a test must construct

A run against a stale artifact.

Situation markers that must fire independently of the safety check:

- `gate.stale_artifact_present`

Check semantics: `always` - the addon's `descriptorSchemaVersion()` and `qualifiedTestProfile()` equal the wrapper constants in every run.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: yes at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: none for this task
- Conclusion: resolved with answer.
