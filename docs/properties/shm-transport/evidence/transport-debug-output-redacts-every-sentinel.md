# transport-debug-output-redacts-every-sentinel

## Discovery trigger

Three tests (`crates/shm-transport/tests/contract.rs:446`, `:714`,
`crates/shm-transport/tests/profile.rs:22`) and the `redacted_debug!` macro
assert that peer-echoed sentinels never appear in `Debug` output, and
`docs/shm-transport.md:84` states the host-level consequence. No record named the
property.

## Evidence trail

- `crates/shm-transport/src/lib.rs:13-21` defines `macro_rules! redacted_debug`,
  which implements `core::fmt::Debug` for each listed type as
  `formatter.write_str(concat!(stringify!($ty), "(<redacted>)"))`. The doc
  comment at `:11-12` gives the reason: the values are sentinels a peer must
  echo back, so they stay out of logs. `:22` re-exports it `pub(crate)`.
- Invocation sites (thirteen types): `profile.rs:228` (`TargetProfile`), `:568`
  (`Admission`), `:585` (`QuarantineRecord`); `backend/sample.rs:101`
  (`SamplePrefix`), `:127` (`ValidatedSample`); `descriptor.rs:88`
  (`HardwareProfileId`), `:117` (`TransportDescriptor`), `:145` (`Incarnation`),
  `:200` (`ReleaseIdentity`), `:337` (`FrameDescriptor`), `:392`
  (`ValidatedFrame`); `arena.rs:67` (`ArenaSpan`), `:181` (`SpanPlan`).
- `debug_and_errors_redact_every_sentinel` (`contract.rs:446`) formats a
  transport descriptor, an incarnation, a release identity, a frame descriptor,
  and `DescriptorError::WrongIncarnation`, and asserts the joined output does
  not contain `SENTINEL`, the sentinel value, or `0x` (`:468-470`).
- `sample_errors_redact_every_sentinel` (`contract.rs:714`) repeats the pattern
  for the sample types and their errors.
- `debug_redacts_profile_admission_and_quarantine_record` (`profile.rs:22`)
  formats `TargetProfile`, `Admission`, and `QuarantineRecord` (`:36-39`).
- Formatter search: `{:?}` sites in `crates/host-runtime/src` and
  `packages/shm-native/src` outside tests (41 in total) format `auth` stages,
  generic errors, and client values; none formats a ring, lease, descriptor,
  profile, admission, or sample type.

## Failure scenario

A new field type or error variant that derives `Debug` instead of routing
through the macro renders its payload. If a future host or addon log line
formats such a value, the sentinel it carries becomes a replayable credential
for whoever reads the log.

## Timing windows and dependencies

None; a static formatting property. Its reach depends on whether any shipped
code path formats these types, which none does today.

## What a test must construct

For each type in the macro's invocation list, a value with a known sentinel,
formatted with `Debug` directly and inside each error variant that embeds it,
asserting the output equals `TypeName(<redacted>)` and contains no substring of
the sentinel.

## Investigation log

### Q: Does any shipped log line format one of these types?

- Sources examined: ripgrep for `:?}` over `crates/host-runtime/src` and
  `packages/shm-native/src`, filtered for transport type names.
- Findings: 41 sites, none on a transport type outside tests.
- Missing evidence: derived `Debug` on host-side error enums that embed
  transport errors was not enumerated; if one exists and is logged, delegation to
  the redacted impl still holds, but the label would move.
- Conclusion: `test-only`, with the label flip condition stated in the record.

### Q: Which of the thirteen types do the tests format?

- Sources examined: `tests/contract.rs:446-470`, `:714-735`;
  `tests/profile.rs:22-47`.
- Findings: `TransportDescriptor`, `Incarnation`, `ReleaseIdentity`,
  `FrameDescriptor`, `SamplePrefix`, `TargetProfile`, `Admission`, and
  `QuarantineRecord` are formatted (eight); `HardwareProfileId`,
  `ValidatedFrame`, `ValidatedSample`, `ArenaSpan`, and `SpanPlan` are not.
  `profile.rs` asserts the exact `TypeName(<redacted>)` string and no
  `SENTINEL` but not the `0x` clause.
- Missing evidence: a set-coverage check over every sentinel-bearing type.
- Conclusion: Exercised is partial (8 of 13) and the Check is restated as a
  coverage oracle, since the rendering holds by construction of the macro.
