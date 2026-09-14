# served-default-omissions-are-bounded

## Discovery trigger

Plan R5 accepts message false-meta omission and daemon-built tool false omission.
R6 and the risk section describe a 25-byte tool change.
Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: plan R3-R6/KTD2 and issue 350's frozen-byte constraints.
Lenses: protocol contracts, unproven assumptions, version compatibility.

## Evidence trail

1. `crates/memory-store/src/lib.rs:72-89` skips false typed metadata flags.
   Its custom message serializer at lines 145-161 instead replays originals.
2. Lines 340-352 default tool flags to false but do not skip them on output.
   Typed tool objects always have other members, including `type` and `id`.
3. `packages/opencode-plugin/src/hooks/context/module-wire.ts:1012-1014`
   emits only absent/true tool flags; lines 1083-1090 explicitly emit metadata
   booleans. These two omission boundaries are different.
4. `crates/daemon/src/injection.rs:145-176` builds a synthetic call/result
   pair with false tool flags. Both are production constructors.
5. `crates/memory-store/src/lib.rs:1279-1319` persists those messages and
   clears originals on reload. They are not merely process-local cache entries.
6. `crates/daemon/src/transform.rs:13714-13793` pins old message bytes;
   `crates/daemon/src/injection.rs:794-831` compares same-input bytes and
   changed-input inequality. Despite its name, it pins no literal or baseline
   bytes. The test-strategy correction is source-confirmed; checks stay unaudited.

## Failure scenario

A broad regeneration accepts every changed byte as a default omission. It
also drops true metadata, empty signatures, provider extras, or a tool failure
variant. Semantic JSON equality after removing arbitrary fields hides the bug.

The check uses an independent edit of old canonical JSON over a bounded
domain. Remove only the three false metadata members and the applicable false
tool member. Keep the `meta` object, all true values, and all remaining data.
R3 unknown-envelope normalization has its own labeled cases.

## Timing windows and dependencies

The comparison crosses old and new serializers and persisted-pair reload.
It does not claim a power-loss protocol or a timing SLO.
Reachability is default-production because synthetic todo construction and
ordinary plugin metadata require no testing flag, although todo state is needed.

## What a test must construct

- Plugin message metadata with each false/true combination that matters.
- Plugin tool flags absent and true, with exact baseline block bytes.
- Typed false call and result, plus a loaded old frozen synthetic pair.
- Explicit-false ingress and unknown envelopes as non-P normalization cases.
- Empty strings, optional fields, provider JSON, and error output variants.
- Compare the entire served byte string, not just parsed field equality.
- Record `typed_wire_identity_default_forms` from input forms before checking.

## Investigation log

### Q: Is the claimed 25-byte change universal or corpus-specific?

- Sources examined: typed tool fields, constructor output, compact JSON syntax,
  and plan Appendix A.3.
- Findings: `"provider_executed":false` has 25 ASCII bytes. Removing it from
  a tool object also removes one comma. The exact per-block compact delta is
  26 bytes when this is the only field change; the pair delta is 52 bytes.
  Key sorting changes order, not length. Absent/true flags have no such delta.
  Unknown-field removal and message metadata omissions have other lengths.
- Missing evidence: no supplied corpus or serializer boundary exhibits a net
  25-byte tool-block change. The Appendix reports 26 differing blocks out of
  53, which is a corpus count rather than a length.
- Conclusion: resolved as arithmetic for the named block boundary; the parent
  specification needs human input to correct or replace its stated boundary.

### Q: Can unchanged frozen-pair bytes be promised across this upgrade?

- Sources examined: frozen-pair docs and deserializer at lines 1273-1319.
- Findings: the pair is durable; accepted omission changes its CK serialization.
- Missing evidence: explicit reconciliation with older exact-replay promises.
- Conclusion: needs human input from the specification owner. Test strategy
  receives the bounded omission oracle; no benchmark result is implied.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`; replay envelopes removed. `served_canonical_shell_bytes_and_segments_are_frozen` pins the served bytes of a decoded message: unknown envelope keys (`z`, `future`, a block's `unknown`) are absent, false `synthetic` is omitted, true is kept, and an edit serializes immediately. `typed_only_blocks_canonicalize_by_field_selection_and_sorted_order` pins explicit-false `provider_executed` on ingress to the omitted default, the accepted false-default omission. Unknown-envelope discard is the R3 normalization domain, separate from the two R5 omissions; no persisted synthetic-pair replay ran.

Served synthetic flag: `TransformIngress::rendered_message` writes the
pass-local effective `synthetic` flag into the served copy. With replay gone the
served bytes of a plugin-echoed synthetic todo pair that arrived unflagged carry
`"synthetic":true`, where the retained envelope used to replay the ingress
`false` while the struct already carried `true` for every decision
(`synthetic_ingress_matches_flagged_reference` pins the served bytes to the
flagged reference). The plugin reads only `native_messages`, which already
carried the effective flag through `encode_opencode`, so no plugin-visible
behavior changes; the affected daemon-side identities are that message's
`canonical_hash` and `output_identity`, which feed the native cache key and
change once for such a message. This is typed-envelope normalization of a
daemon-built value, not one of the two R5 omissions, and is listed in the PR
description for the owner.
