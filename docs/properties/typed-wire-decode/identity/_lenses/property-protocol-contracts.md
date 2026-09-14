# Property lens: protocol contracts

Date: 2026-09-13. HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Baseline: `e451a2b470ae8663b4613ca04f019a30b6d7df53`.
External leads: normative host protocol and plan R3-R5.

## Findings

`docs/host-wire-protocol.md:12-14,763` freezes host wire literals while
leaving routed bodies to handlers. Its channel-0 and LocalEmbeddings validation rules
must not be imposed on CK identity by analogy.
The plugin emits absent/true tool flags at
`packages/opencode-plugin/src/hooks/context/module-wire.ts:1012-1054`.
HEAD typed tool fields always serialize false when not replaying originals
(`crates/memory-store/src/lib.rs:340-352`).

## Candidates

`plugin-block-canonical-identity-preserved` covers actual emitter shapes.
`served-default-omissions-are-bounded` permits only declared false omissions
within its compatibility domain. Explicit-false ingress and unknown envelope
fields are separate accepted normalization cases, not byte-preserving cases.

## Limits

Decode lane/refusal semantics belong to the sibling decode catalog. Preserve
known fields and retained opaque JSON rather than interpreting R3 as permission
to drop arbitrary nested provider data.
