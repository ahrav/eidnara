# raw-native-attach-rejects-hostile-descriptors-without-effects

## Discovery trigger

Re-verification of `native-boundary-not-weaker-than-its-wrapper` against HEAD
found that `NativeChannel.attach` (`packages/shm-native/index.ts:686-689`)
forwards the descriptor to `native.attach` unchanged, so the TypeScript grant
decoder that record compared against does not exist in this tree. The raw
native boundary's own rejection set, pinned by four suites in
`packages/shm-native/tests/mechanism.ts`, had no record.

## Evidence trail

- `packages/shm-native/index.ts:686-689`:
  `static attach(descriptor: NativeDescriptor): NativeChannel { const native =
  capableAddon(); return new NativeChannel(native, native.attach(descriptor)); }`.
  No decoding, no field checks.
- `packages/shm-native/src/lib.rs:592` is `pub fn attach(env: &Env, descriptor:
  Unknown<'_>) -> Result<u32>`. `:597-598` reject a non-object with
  `descriptor_error()`; `:601` casts to `Object` with the same error; `:604`
  returns "shared-memory profile is unavailable" when the profile is absent;
  later field reads map to `descriptor_error()` (`:649`, `:659`).
- `DESCRIPTOR_ERROR: &str = "invalid shared-memory descriptor"` at `lib.rs:31`;
  `fn descriptor_error() -> Error` at `:150`.
- `tests/mechanism.ts:758` is `expectRejectedWithoutEffects(descriptor, pattern =
  DESCRIPTOR_ERROR)`: it snapshots `activeChannelCount()`,
  `activeExternalRefCount()`, and `nativeLeakDiagnostics()`, asserts
  `addon.attach(descriptor)` throws `pattern`, then compares the three again.
- Suites: "rejects non-object and structurally hostile arguments" (`:772`);
  "rejects every unsafe numeric representation before narrowing" (`:795`, with
  `hostileFds = [-1, -0, 2 ** 31, 3.5, NaN, "10"]` at `:798`); "rejects
  malformed, non-ASCII, and aliased grant text" (`:817`); "accessor objects and
  proxies get one bounded redacted error" (`:847`, asserting the message equals
  `invalid shared-memory descriptor` at `:864`, with a flipping `Proxy` at
  `:871`).
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

- Sources examined: `packages/shm-native/index.ts:686-689` and a search of
  `index.ts` for `candidateId`, `stale`, `aliased`, `geometry`, `decodeGrant`.
- Findings: the forwarding is unconditional; none of the searched terms appear.
- Missing evidence: none.
- Conclusion: the raw boundary is the only descriptor validator.

### Q: Is the direction-binding gap still open?

- Sources examined: the invalidated record's Impact; `lib.rs:592-659` field
  reads.
- Findings: fields are read by name, and nothing checks that two lanes are not
  both producers.
- Missing evidence: whether any caller can present swapped lane fields.
- Conclusion: carried as this record's open question.

### Q: Do all six suites expect the same message, and where do they run?

- Sources examined: `tests/mechanism.ts:774`, `:797`, `:819`, `:849`, `:880`,
  `:886`, `:890`, `:896`; `packages/shm-native/src/lib.rs:604`, `:276`, `:283`.
- Findings: the four shape suites expect `invalid shared-memory descriptor`;
  the wrong-profile suite expects `shared-memory profile is unavailable`
  (`tests/mechanism.ts:886`, emitted at `lib.rs:604`) and the unresolvable-descriptor suite expects
  `shared-memory attachment failed` (`tests/mechanism.ts:896`, emitted at `lib.rs:276` and
  `:283`). Every suite returns before any assertion unless the addon loaded and
  the platform is Linux or Darwin.
- Missing evidence: none.
- Conclusion: the single-message claim holds for four of six; the no-effects
  half spans all six; the Exercised label is conditional on a built addon.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 25, `tests/mechanism.ts:387` now `tests/mechanism.ts:758`: At HEAD the helper takes the addon as its first parameter: `expectRejectedWithoutEffects(addon, descriptor, pattern = DESCRIPTOR_ERROR)`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
