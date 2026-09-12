# meta-json-preparation-scans-every-persisted-byte

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The single-pass evidence below describes the live code.

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
[content-sig]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L1308
[bim]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L1515
[json-content]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L2116-L2126
[record-scan]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L2133-L2143
[policy]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3077-L3098
[prepare-collecting]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3109-L3287
[keys]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3157-L3170
[prepare-value]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3181-L3276
[refuse-identity]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3213-L3217
[refuse-container]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3226-L3233
[substitute]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3243-L3259
[walk-keys]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3266-L3272
[clean-branch]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3282-L3286
[unique-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3289-L3290
[unique]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L3291-L3372
[load]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6196-L6223
[commit-meta]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L8306-L8315
[recomp]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10057-L10150
[t-keydir]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L15029
[t-dup]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L15112
[t-container]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L15129
[t-preserved]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L15166
[t-cache-redact]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/tests/production_redaction.rs#L606
[value-compare]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L3184
[serde-features]: https://github.com/ahrav/eidnara/blob/9132344/Cargo.toml#L45

## Single-pass evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the units
that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/433)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

[`prepare_json_content_single_pass`][single-pass] keeps the unique-name parse
ahead of one walk, and the walk does the rest: it validates every object key it
descends through, refuses or substitutes protected values, records detections,
and sets a [`changed` flag][changed] when a value is replaced by different
text. The separate key-validation pass is gone. The clone-and-compare is gone
from release builds; a debug build keeps the [clone][debug-clone] and asserts
that the flag agrees with the [structural compare][debug-compare], so a
mutation site that forgets the flag fails every test run. The
[clean branch][clean-branch-live] returns the input when nothing changed and
re-serializes otherwise. The walk judges a subtree whole without descending in
two cases, an integrity-named value and an identity-named value under a policy
that does not preserve identities, and runs the [key validation][keys-live]
over that subtree, so a secret-bearing key under `{"signature": {...}}` is
refused there. Under the `DurablePreserveIdentities` policy that `meta` uses,
an identity-named scalar is [preserved][preserved-live] and an identity-named
container falls through to the ordinary walk, whose [object arm][walk-keys-live]
validates each key, so a secret-bearing key under `{"id": {...}}` is refused
by the walk itself. The
[wrapper][collecting-live] that callers use asserts in debug builds that a
change left a detection for the receipt; byte identity of clean output is the
clean branch's construction rather than an assertion.

The order in which a document with two independent faults reports its error
can differ from the baseline, where every key was validated before any value;
the outcome on one fault is unchanged and detections gathered before a refusal
are discarded with the refused write, as before. `serde_json::Value` walks
object members in `serde_json::Map` order, so a value precedes a refusing key
only when its own key sorts first; a fixture that plants the value under a
later-sorting key never reaches the refusal with a detection in hand.

The [unit test][unit-live] shows clean input returned byte for byte with
`changed` false, a substitution with `changed` true and one recorded detection,
and refusals for a secret-bearing key under an integrity-named and under an
identity-named container. The [refusal-ordering test][refusal-live] serializes
the store test's `keyed` fixture, a `block_identity_by_mid` map whose `a-mid`
entry carries a value secret and whose second key is itself a secret, and shows
the caller's vector holding one detection when `prepare_json_content_collecting`
returns the refusal. The [store test][store-live] shows, through `commit`,
clean `meta` stored equal to `serde_json::to_string` of the value with a
`meta` receipt whose `finding_count` is zero, a secret planted in a
`block_identity_by_mid` entry's value substituted with a `meta` receipt whose
`finding_count` is one (`scan_detections` deduplicates labels, so the count
is the oracle), and the same `keyed` fixture refused with no row stored and
every scan-audit table count unchanged from before the refused `commit`, so
the detection the walk gathered before the refusal was discarded rather than
recorded. Duplicate object names remain
refused by [`parse_json_with_unique_names`][unique-live] and its
[existing test][t-dup-live].

### Focused execution, 2026-09-12

At the merged tree `39f706b6`, `cargo test -p memory-store --locked` passed
every test in each binary: 144 in the library, 5 in `baseline.rs`, 8 in
`dreamer_ledger.rs`, and 22 in `production_redaction.rs`, the three above
among them. `cargo test -p daemon --locked` passed 1021; the two
`dreamer_run_task_bounds_*` tests fail under full-suite load on the base
branch as well and pass in isolation.

[single-pass]: ../../../../../crates/memory-store/src/lib.rs#L3412-L3604
[changed]: ../../../../../crates/memory-store/src/lib.rs#L3567-L3570
[debug-clone]: ../../../../../crates/memory-store/src/lib.rs#L3590-L3591
[debug-compare]: ../../../../../crates/memory-store/src/lib.rs#L3596-L3597
[clean-branch-live]: ../../../../../crates/memory-store/src/lib.rs#L3598-L3603
[keys-live]: ../../../../../crates/memory-store/src/lib.rs#L3462-L3475
[preserved-live]: ../../../../../crates/memory-store/src/lib.rs#L3500-L3510
[walk-keys-live]: ../../../../../crates/memory-store/src/lib.rs#L3577-L3583
[collecting-live]: ../../../../../crates/memory-store/src/lib.rs#L3396-L3406
[unique-live]: ../../../../../crates/memory-store/src/lib.rs#L3672-L3674
[t-dup-live]: ../../../../../crates/memory-store/src/lib.rs#L16283-L16295
[unit-live]: ../../../../../crates/memory-store/src/lib.rs#L16103-L16156
[refusal-live]: ../../../../../crates/memory-store/src/lib.rs#L16158-L16195
[store-live]: ../../../../../crates/memory-store/tests/production_redaction.rs#L611-L725
