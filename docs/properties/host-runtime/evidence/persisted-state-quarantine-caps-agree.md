# persisted-state-quarantine-caps-agree

Scope note: the two persisted objects are the lifecycle record and the payload
generation manifest, plus the current-profile selector, which shares the
generation cap. "Generation" is the on-disk content-addressed payload generation,
not the in-process connection generation of `connection.rs`.

## Discovery trigger

A comment that asserts an equality. Reading the generation store's constant block
and following the claim to the constant it names.

## Evidence trail

Both constants, quoted at HEAD:

`crates/host-runtime/src/lifecycle.rs:51`

```rust
const MAX_EVIDENCE_BYTES: usize = 65_536;
```

`crates/host-runtime/src/generation.rs:148-149`

```rust
/// Manifest and lifecycle-evidence readers each cap input at 1 MiB.
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
```

The claim is the words "Manifest and lifecycle-evidence readers each cap input at
1 MiB" at `generation.rs:148`. The values are 65,536 and 1,048,576. The ratio is
exactly 16: 1,048,576 / 65,536 = 16. The comment is false as written: the
lifecycle-evidence readers cap input at 64 KiB.

The more consequential half is what each cap gates, because the two are not
symmetric. `MAX_EVIDENCE_BYTES` has three uses, and exceeding it is conservative
at all three. In `quarantined_record_present` (`lifecycle.rs:232`, read at `:253`) a
failing or oversize read returns `Ok(true)`, quarantined, refusing the start. In
`remove_lifecycle_record` (`lifecycle.rs:416`, read at `:431`) the same failure
returns early, so the record is not unlinked. In `read_evidence_file`
(`lifecycle.rs:729`, read at `:749`) it yields `EvidenceFile::Insecure`, which the
comment at `:737-739` explains is what stops a hostile shape from downgrading a
`wedged` verdict. Oversize means "cannot decide, so hold" everywhere.

`MAX_MANIFEST_BYTES` gates every generation-side read in `generation.rs`, and at
HEAD each of the three execution-deciding paths quarantines an oversize file
before reading it. `is_quarantined_schema` tests `st_size` against the cap at
`:1533` and returns quarantined. `read_profile` tests the selector's `st_size` at
`:907` and returns `CurrentProfile::Quarantined`, with the comment at `:905-906`
naming the manifest cap in `validate_in_dir` as what it matches. `read_manifest_in_dir`
tests the manifest's `st_size` at `:1031` and returns
`GenerationError::UnsupportedStateSchema`, with the comment at `:1030` naming
`is_quarantined_schema` as what it matches. The capped reads that follow (`:910`,
`:1034`, `:1537`) can still fail for other reasons and then report
`invalid(...)`, which `crates/daemon/src/bin/eidnara-host.rs:347` maps to
`native_payload_invalid`, but a file whose only fault is size never reaches them.
The pre-write refusal at `:1255` stops this release writing such a manifest at
all.

So exceeding the cap means quarantine on both sides. The two sides still use two
values.

## Failure scenario

Two distinct consequences follow, and only the first is the one the catalog
states.

A future release writing a 100 KiB lifecycle record is quarantined by this
release: the gate at `lifecycle.rs:253-255` returns `Ok(true)` and the start is
refused with `unsupported_state_schema` and `align_versions`, which is the
intended forward-compatibility behaviour. A 100 KiB manifest sits well under
1 MiB, so it is decoded normally and is quarantined only if its schema is unknown

- a different mechanism entirely. The size threshold that is supposed to be one
value is two, and the maintainer who moves one on the comment's authority moves
only one.

The second consequence is the threshold split inside one release. A future
manifest above 1 MiB is preserved by `prune`, which classifies it undecidable at
`:1533`, and `read_manifest_in_dir` refuses it as `UnsupportedStateSchema` at
`:1031-1033`, so the generation is retained on disk and unusable with an
`align_versions` remediation. The same applies to the selector itself:
`current-profile.json` is read under `MAX_MANIFEST_BYTES` (`:907-910`), so the
profile's threshold is 1 MiB while the lifecycle record's is 64 KiB, even though
both are trusted persisted selectors.

One aggravator is verified and worth naming: `MAX_MANIFEST_BYTES` is an overloaded
identifier inside `host-runtime`. `crates/host-runtime/src/local_embeddings/bundle.rs:12` declares
`const MAX_MANIFEST_BYTES: u64 = 64 * 1024`, and
`crates/host-runtime/src/harness_closure.rs:25` declares
`const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024`. Three constants share one
name in one crate with values 64 KiB, 1 MiB, and 16 MiB, so "the manifest cap" is
ambiguous before the equality question is even asked.

## Timing windows and dependencies

No fault and no window; both values are compile-time constants and the whole
record is statically checkable. Depends on
`an-undecidable-quarantine-witness-fails-closed`, whose single closed arm is the
`:1513` size test against this cap.

## What a test must construct

Nothing to inject. Either an equality assertion over the two constants, or -
since the two caps gate different decisions - separate assertions that an
over-cap record quarantines and an over-cap manifest quarantines on every read
path rather than only in `prune`. Both generation-side halves are exercised:
`undecodable_generations_survive_prune_after_a_successor` (`generation.rs:2253`)
plants an oversized manifest and `unknown_schemas_are_quarantined_and_block_mutation`
(`:2572`) writes an oversized profile (`:2628-2633`). No test asserts the two
constants agree.

## Investigation log

### Q: None recorded in the catalog. Verified here instead: what does each cap actually gate, and is the stated consequence the real one?

- Sources examined: `lifecycle.rs:64-65`, `:257-284`, `:467-491`, `:803-830`;
  `generation.rs:148-149`, `:894-898`, `:1027-1031`, `:1270-1273`, `:1544-1556`;
  `crates/host-runtime/src/instance.rs:852-865` for `read_all_fd`'s over-cap error;
  `eidnara-host.rs:344-350` for the error-to-reason mapping; every workspace
  occurrence of both identifiers.
- Findings: the values and the false comment are as quoted, ratio 16. The
  asymmetry the source catalog recorded, generation read paths reporting corrupt
  payload for an oversize file, is closed at HEAD: `read_profile` (`:887-889`)
  and `read_manifest_in_dir` (`:1011-1013`) quarantine on `st_size` before the
  capped read, matching `is_quarantined_schema` (`:1513`). `read_all_fd` still
  returns `InvalidData` when the accumulated length passes the cap, so a read
  path that reaches it cannot distinguish oversize from any other read error; at
  HEAD only a file that grows between `fstat` and the read can reach it.
- Missing evidence: none for the mismatch. Not established is which of the two
  values is the intended one, or whether the profile's threshold was ever meant
  to be the manifest's - that is a design question, not a readable fact.
- Conclusion: resolved as to fact, and broader than stated. The caps differ 16x,
  the comment asserting otherwise is false, and the two thresholds also differ in
  what exceeding them means, so making the numbers equal would not by itself make
  the property true.
