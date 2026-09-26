# unknown-envelope-fields-do-not-erase-payload-values

## Discovery trigger

Accepted R3 discards unknown envelope fields. KTD3 explicitly retains every
classified payload Value. The data-integrity and version lenses find that
these are different boundaries even when the same key appears at both.

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
The settled plan is worktree-only accepted evidence, as cataloged.

## Evidence trail

- `crates/memory-store/src/lib.rs:114-123,243-247` identifies the known
  message and block envelope fields whose serde attributes KTD1 preserves.
- `crates/memory-store/src/lib.rs:326-356` retains ToolCall.input as Value
  and defines closed BlockKind variants.
- `crates/memory-store/src/lib.rs:372-447` retains JSON/error-JSON outputs,
  opaque source/raw/optional arc, media source, and nested provider extras.
- `crates/daemon/src/transform.rs:700-708` retains optional native Values
  and tail_delta. `crates/daemon/src/lib.rs:4328-4337` reads delta leniently.
- `crates/memory-store/src/lib.rs:145-152,266-273` currently replays whole
  originals. `:16096-16112` deliberately preserves an unknown sibling key.
- `crates/daemon/src/wire.rs:1708-1745` discards an unknown message field
  during reattachment while preserving an unknown block field.
- Worktree-only [TE08](../../../transform-edit-responses/catalog.md#cached-canonical-prefix-preserves-exact-ingress)
  requires serialized expanded CK to equal exact captured CK, including
  nested unknown fields. This is conflicting proposed work, not HEAD behavior.

The default-production entry reaches these definitions on both decode lanes.
Unknown fields inside typed meta/origin/kind/output objects also lose their
enclosing replay preservation. That does not permit stripping nested keys
from arbitrary Values held by known fields.

## Failure scenario

A recursive unknown-field scrub deletes a tool argument or opaque native
part. A different implementation keeps the original envelope to preserve
those payloads, violating the accepted representation change.

Another false failure comes from comparing lexical JSON rather than Values:
key order and whitespace are not retained payload semantics. Explicit null
in an Option<Value> field becomes None, but null nested inside a retained
Value must remain null. These need distinct fixture positions.

## Timing windows and dependencies

Typed conversion is the discard point. Page arrays must still include unknown
fields in their digest before conversion; the page record owns that ordering.
Unknown enum variants remain typed errors. Missing, optional null, explicit
false, and wrong known-field types retain their existing serde meanings.

## What a test must construct

Use a sentinel such as `future_payload` at both discarded and retained paths.
Build independent typed expectations with every kept Value present: tool
input, output JSON/error JSON, opaque source/raw/arc, media source, provider
namespace maps, native messages, and tail_delta.
Include nested arrays, empty collections, null leaves, escapes, and numbers.
Run direct and tree conversion. Observe known-field equality and discarded
key absence without using the new serializer to generate both expectations.

Exercise invalid known enum tags separately. Their refusal must not be
mistaken for the expected disappearance of an unknown envelope key.
No test runs. Existing payload and replay checks remain unaudited.

## Investigation log

### Q: Does retaining a Value field promise retaining explicit null there?

- Sources examined: memory-store lib.rs:431-438 and transform.rs:700-708.
- Findings: arc, native_messages, and tail_delta are optional fields. Null
  at the field boundary is not a retained Some(Value::Null) in these decodes.
- Missing evidence: none for declared types; final placement cases remain.
- Conclusion: resolved with answer: preserve decoded payload semantics,
  including existing Option/default normalization, not arbitrary field text.

### Q: Is sender exclusivity enforced by the host contract?

- Sources examined: plan assumption and host-wire-protocol.md:291-292,337.
- Findings: routed application bodies are opaque to transport. The plan's
  producer inventory is a scope assumption, not a transport admission guard.
- Missing evidence: external/custom CK producer inventory or usage capture.
- Conclusion: unresolved, no additional sender evidence is supplied.

### Q: How does this specification handle TE08's exact-ingress requirement?

- Sources examined: the worktree-only TE08 record, accepted R3, and the user
  amendment accompanying independent analyst `ses_f6756093fffeVjNp36S3E8pKrM`.
- Findings: preserving captured unknown CK fields conflicts with discarding
  them at typed conversion. The 2026-09-13 disposition keeps R3 authoritative.
- Missing evidence: the integration owner's recorded cross-work reconciliation.
- Conclusion: unresolved, needs integration reconciliation before combining
  the work. This specification's owner preserves R3 and records that outcome;
  no compatibility exception or edit to the other catalog is authorized here.

## Named handoff

`/testing:test-strategy` owns placement-sensitive payload witnesses.
The identity agent owns changed byte/hash bases for discarded fields and
default omissions. The specification and integration owners record TE08
reconciliation before integration while preserving R3. The independent
portfolio pass is complete on 2026-09-13; this property remains unexercised.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`, `cargo test -p daemon --locked
--features test-support` (1,489 tests pass; `lifecycle_cli` is platform-unsupported
on the aarch64 host). Unknown message and block envelope
fields are discarded on both lanes; retained payload `Value`s keep their keys.
Witnesses: `a_block_edit_leaves_its_sibling_unchanged_and_envelope_unknowns_are_discarded`
(memory-store), `reattach_shares_the_decoded_shell_and_unknown_envelope_fields_are_discarded`
and the reattach sharing test (wire.rs), `overlay_canonicalizes_only_the_mutated_block`
(the opaque payload survives; `sentinel_unknown_field` does not), and
`decoded_envelope_charges_only_typed_fields`. A nested unknown key inside a
retained payload `Value` is not separately witnessed.

Update, 2026-09-26: [#828](https://github.com/ahrav/eidnara/issues/828) deletes
`reattach_shares_the_decoded_shell_and_unknown_envelope_fields_are_discarded`
with prefix reattachment; decode-side unknown-field handling is unchanged. The
daemon projects every request from its full input. Citations of these symbols
here are historical at their stated baseline.
