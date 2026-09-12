# meta-json-preparation-scans-every-persisted-byte

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

`commit_transform` serializes `ModuleMeta`, parses the text back into a
`Value`, walks and redacts a clone, and re-serializes only when the walk
changed something. The audit treats the parse and the clone as removable and
proposes a streaming single-pass redaction that returns the original bytes for
an unchanged prefix. The parse is load-bearing: it refuses duplicate object
names, and the comment on that parser states the bypass a `Value`-only parse
would allow. The memory-store catalog's
[preserved-identity][ms-preserved] and [refused-write][ms-refused] records own
the policy; this record fixes the byte and receipt contract for the `meta`
column specifically.

## Evidence trail

- [`commit_transform`][commit-meta] serializes `meta` with
  `serde_json::to_string`, then hands the text to
  [`json_content("meta", .., DurablePreserveIdentities)`][json-content],
  which calls [`prepare_json_content_collecting`][prepare-collecting] and
  records the returned detections with [`record_observed_scan`][record-scan]
  under action `substitute`.
- [`prepare_json_content_collecting`][prepare-collecting] bounds the input,
  parses with [`parse_json_with_unique_names`][unique], runs
  [`validate_json_keys`][keys] (bound and secret scan on every object key at
  every depth), clones the tree, walks the clone with
  [`prepare_value`][prepare-value], and at the [clean branch][clean-branch]
  returns `input.to_string()` when `value == original`, else
  `serde_json::to_string(&value)`.
- The [comment][unique-doc] on the unique-name parser states that
  `serde_json::Value` keeps only the last duplicate, letting an earlier
  secret-bearing value bypass `prepare_value` and persist when the unchanged
  input is returned.
- [`prepare_value`][prepare-value]: a detected value under an identity or
  integrity key refuses ([`:3213-3217`][refuse-identity]); a protected key
  holding a container with text refuses ([`:3226-3233`][refuse-container]); a
  protected scalar substitutes `<REDACTED:label>` and records a synthetic
  detection when the scanner found none ([`:3243-3259`][substitute]); object
  keys are bound-checked and scanned again during the walk
  ([`:3266-3272`][walk-keys]).
- The [policy enum][policy] distinguishes durable from transaction and
  reject-protected from preserve-identities; the `meta` column uses durable
  preserve-identities.
- `ModuleMeta` fields that carry the named fault shapes exist:
  [`block_identity_by_mid: BTreeMap<String, ..>`][bim] (a secret can be a
  key) and [`tail_hygiene_baseline.content_signature`][content-sig] (an
  integrity name).
- No reader compares `meta` bytes: [`load`][load] deserializes,
  [`reset_session_for_recomp`][recomp] deserializes and rewrites, and the
  transform compares values ([`next_meta != loaded.meta`][value-compare]).
  The workspace `serde_json` has only [`raw_value`][serde-features], so a
  re-serialized `Value` orders keys alphabetically.

## Failure scenario

A streaming redaction that emits the original bytes for an unchanged prefix
and only rewrites from the first substitution lets an earlier duplicate name's
value persist while the later duplicate is what the walk saw; that is the
bypass the parser comment names. A redaction that substitutes without pushing
a detection leaves the audit receipt with fewer findings than the stored bytes
show. A redaction that scans values but not keys stores a secret inside a
`BTreeMap` key.

## Timing windows and dependencies

None in time. The dependencies are the unique-name parser, the key walk, and
the clean branch's `value == original` test. The alphabetical key order of a
redacted `meta` is a consequence of the feature set, not a contract any reader
depends on.

## What a test must construct

`meta` text with duplicate names at top level and nested; a secret inside a
`block_identity_by_mid` key; a secret under
`tail_hygiene_baseline.content_signature`; a protected key holding an object;
a clean `meta` compared byte-for-byte with the stored column against
`serde_json::to_string(meta)`. Assert refusal, refusal, refusal, refusal, and
byte identity respectively, and that the recorded `meta` scan carries the
detections the walk observed. The
[state checks](../existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
list [`t-dup`][t-dup], [`t-keydir`][t-keydir], [`t-container`][t-container],
[`t-preserved`][t-preserved], and [`t-cache-redact`][t-cache-redact]; none
covers byte identity of a clean stored `meta` or a `BTreeMap`-key secret.

## Investigation log

### Q: Must a redacted `meta` keep today's alphabetical key order?

- Sources examined: the [clean branch][clean-branch],
  [`Cargo.toml`][serde-features],
  [`load`][load], [`value_compare`][value-compare], [`recomp`][recomp].
- Findings: The order is an artifact of `preserve_order` being off. Every
  reader deserializes and compares values; none depends on byte order. A
  streaming design that preserves source order would change stored bytes for
  redacted rows only.
- Missing evidence: A statement of whether any deserializable form is
  acceptable.
- Conclusion: needs human input.

[ms-preserved]: ../../../memory-store/catalog.md#preserved-identity-name-does-not-exempt-its-value
[ms-refused]: ../../../memory-store/catalog.md#refused-durable-write-leaves-no-row-and-no-receipt
[content-sig]: ../../../../../crates/memory-store/src/lib.rs#L1302
[bim]: ../../../../../crates/memory-store/src/lib.rs#L1509
[json-content]: ../../../../../crates/memory-store/src/lib.rs#L2110-L2120
[record-scan]: ../../../../../crates/memory-store/src/lib.rs#L2127-L2137
[policy]: ../../../../../crates/memory-store/src/lib.rs#L3071-L3092
[prepare-collecting]: ../../../../../crates/memory-store/src/lib.rs#L3103-L3281
[keys]: ../../../../../crates/memory-store/src/lib.rs#L3151-L3164
[prepare-value]: ../../../../../crates/memory-store/src/lib.rs#L3175-L3270
[refuse-identity]: ../../../../../crates/memory-store/src/lib.rs#L3207-L3211
[refuse-container]: ../../../../../crates/memory-store/src/lib.rs#L3220-L3227
[substitute]: ../../../../../crates/memory-store/src/lib.rs#L3237-L3253
[walk-keys]: ../../../../../crates/memory-store/src/lib.rs#L3260-L3266
[clean-branch]: ../../../../../crates/memory-store/src/lib.rs#L3276-L3280
[unique-doc]: ../../../../../crates/memory-store/src/lib.rs#L3283-L3284
[unique]: ../../../../../crates/memory-store/src/lib.rs#L3285-L3366
[load]: ../../../../../crates/memory-store/src/lib.rs#L6236-L6263
[commit-meta]: ../../../../../crates/memory-store/src/lib.rs#L8459-L8468
[recomp]: ../../../../../crates/memory-store/src/lib.rs#L10210-L10303
[t-keydir]: ../../../../../crates/memory-store/src/lib.rs#L15182
[t-dup]: ../../../../../crates/memory-store/src/lib.rs#L15265
[t-container]: ../../../../../crates/memory-store/src/lib.rs#L15541
[t-preserved]: ../../../../../crates/memory-store/src/lib.rs#L15608
[t-cache-redact]: ../../../../../crates/memory-store/tests/production_redaction.rs#L606
[value-compare]: ../../../../../crates/daemon/src/transform.rs#L3195
[serde-features]: ../../../../../Cargo.toml#L45
