# raw-native-attach-rejects-hostile-descriptors-without-effects

## Discovery trigger

Re-verification of `native-boundary-not-weaker-than-its-wrapper` against HEAD
found that `NativeChannel.attach` (`packages/shm-native/index.ts:537-540`)
forwards the descriptor to `native.attach` unchanged, so the TypeScript grant
decoder that record compared against does not exist in this tree. The raw
native boundary's own rejection set, pinned by four suites in
`packages/shm-native/tests/mechanism.ts`, had no record.

## Evidence trail

- `packages/shm-native/index.ts:537-540`:
  `static attach(descriptor: NativeDescriptor): NativeChannel { const native =
  capableAddon(); return new NativeChannel(native, native.attach(descriptor)); }`.
  No decoding, no field checks.
- `packages/shm-native/src/lib.rs:525` is `pub fn attach(env: &Env, descriptor:
  Unknown<'_>) -> Result<u32>`. `:532-533` reject a non-object with
  `descriptor_error()`; `:536` casts to `Object` with the same error; `:539`
  returns "shared-memory profile is unavailable" when the profile is absent;
  later field reads map to `descriptor_error()` (`:584`, `:594`).
- `DESCRIPTOR_ERROR: &str = "invalid shared-memory descriptor"` at `lib.rs:32`;
  `fn descriptor_error() -> Error` at `:150`.
- `tests/mechanism.ts:387` is `expectRejectedWithoutEffects(descriptor, pattern =
  DESCRIPTOR_ERROR)`: it snapshots `activeChannelCount()`,
  `activeExternalRefCount()`, and `nativeLeakDiagnostics()`, asserts
  `addon.attach(descriptor)` throws `pattern`, then compares the three again.
- Suites: "rejects non-object and structurally hostile arguments" (`:401`);
  "rejects every unsafe numeric representation before narrowing" (`:424`, with
  `hostileFds = [-1, -0, 2 ** 31, 3.5, NaN, "10"]` at `:427`); "rejects
  malformed, non-ASCII, and aliased grant text" (`:446`); "accessor objects and
  proxies get one bounded redacted error" (`:476`, asserting the message equals
  `invalid shared-memory descriptor` at `:493`, with a flipping `Proxy` at
  `:500`).
- `existing-checks.md` inventories `tests/mechanism.ts` as one file-level row
  that self-skips when the addon is absent or the platform is not Linux.

## Failure scenario

A validation step that reads a descriptor field twice through an accessor or
proxy returning different values admits geometry that was never checked; an
early return after registering a channel leaks the entry and its external
references. Either maps attacker-chosen shared memory into the process or
exhausts the channel table.

## Timing windows and dependencies

None; the hazard is single-call. The dependency is that every read of the
descriptor object happens once and before any registration side effect.

## What a test must construct

Descriptor objects of each hostile shape, built without the wrapper, driven at
the raw addon with the leak counters snapshotted around the call.

## Investigation log

### Q: Does any wrapper-level check exist between the caller and `native.attach`?

- Sources examined: `packages/shm-native/index.ts:537-540` and a search of
  `index.ts` for `candidateId`, `stale`, `aliased`, `geometry`, `decodeGrant`.
- Findings: the forwarding is unconditional; none of the searched terms appear.
- Missing evidence: none.
- Conclusion: the raw boundary is the only descriptor validator.

### Q: Is the direction-binding gap still open?

- Sources examined: the invalidated record's Impact; `lib.rs:525-600` field
  reads.
- Findings: fields are read by name, and nothing checks that two lanes are not
  both producers.
- Missing evidence: whether any caller can present swapped lane fields.
- Conclusion: carried as this record's open question.

### Q: Do all six suites expect the same message, and where do they run?

- Sources examined: `tests/mechanism.ts:403`, `:426`, `:448`, `:478`, `:511`,
  `:515`, `:521`, `:525`; `packages/shm-native/src/lib.rs:539`, `:689`, `:694`.
- Findings: the four shape suites expect `invalid shared-memory descriptor`;
  the wrong-profile suite expects `shared-memory profile is unavailable`
  (`:515`, emitted at `lib.rs:539`) and the unresolvable-descriptor suite expects
  `shared-memory attachment failed` (`:525`, emitted at `lib.rs:689` and
  `:694`). Every suite returns before any assertion unless the addon loaded and
  the platform is Linux or Darwin.
- Missing evidence: none.
- Conclusion: the single-message claim holds for four of six; the no-effects
  half spans all six; the Exercised label is conditional on a built addon.
