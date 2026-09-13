# RP2.1 construction contracts

This document freezes the construction inputs that RP2.1 ticket P1
([#352](https://github.com/ahrav/eidnara/issues/352)) owes the later tickets of
the specification ([#347](https://github.com/ahrav/eidnara/issues/347)). Each
contract is a construction input: it says what a product path must accept,
refuse, identify, or record. None of them approves a number, a campaign, or an
enablement. RP2.9 approves numeric limits; the release owner reconciles the
release gate; the coordinator closes witnesses.

The machine-readable form is
[`crates/kernel/tests/fixtures/search-projection/construction-contracts.json`](../../../crates/kernel/tests/fixtures/search-projection/construction-contracts.json),
checked by the kernel test `search_projection_construction_inputs` together with the
[independent identity fixtures](../../../crates/kernel/tests/fixtures/search-projection/source-identity-fixtures.json)
and the [witness matrix](witness-matrix.md). The test pins the fixture's
enumerations: the five classes and their dense defaults, the tuple field order
and version byte, the admission dimensions, every capability disposition on
both harnesses, the hook set with its shared gate defaults, and the complete
named-limit set with no value. The tables in this document restate the fixture
for readers. A product encoder that disagrees with the reference encoder in
that test on any fixture record, including its golden occurrence and payload
identifiers, is wrong until this contract is revised under a new
identity-contract version.

Contract identifiers are `CC1` through `CC12`. The adoption table at the end
maps each later ticket to the contracts it consumes. Changing any contract
changes `identity_contract_version`
(`search-projection-identity-v3`), which is one of the invalidation identities every hook
carries (CC10) and one of the five rebuild triggers in the specification's C7.

## CC1. Source classes and stable identifiers

RP2.1 projects exactly five source classes. Their codes are constant ASCII and
appear first in every occurrence tuple:

| Class code | Source | Dense by default |
| --- | --- | --- |
| `messages` | Harness message text blocks | yes |
| `canonical_claims` | Canonical claim objects, every RP2.4-qualified kind | yes |
| `promoted_memory` | Positive stable-memory-domain decisions | yes |
| `git_commits` | Retained commit evidence | yes |
| `raw_tool_spans` | Selected spans of exact native tool output | no |

Identity uses stable identifiers only. Domain names, display names, titles, and
any other in-place mutable field are neither identity nor text input (CC7).
`user_memories` is not a source; promoted memory comes from positive
stable-memory-domain decisions and their summaries.

## CC2. Per-class native identity

Each class has a fixed, ordered set of identity fields. Every field is a UTF-8
string. A record with a missing field, an unknown field, or a non-string value
is refused before any identity is minted.

| Class | Identity fields, in tuple order | Representations |
| --- | --- | --- |
| `messages` | `project_id`, `harness`, `session_id`, `message_id`, `block_index` | `text` |
| `canonical_claims` | `object_id` | `decision_summary`, `rationale` |
| `promoted_memory` | `decision_object_id` | `summary` |
| `git_commits` | `repository_id`, `object_format`, `oid` | `commit_message` |
| `raw_tool_spans` | `project_id`, `harness`, `session_id`, `parent_message_id`, `tool_call_id`, `result_revision`, `block_index` | `tool_output`, `tool_error` |

Field semantics:

- `harness` is `opencode` or `pi`. Any other value is refused.
- `session_id` is the durable harness session identity, `message_id` the
  native message identity, and `block_index` the ordered position of the text
  block. Text blocks exclude tool blocks; tool blocks belong to
  `raw_tool_spans` under their parent message.
- Claims and promoted memory carry the canonical object identity and the
  canonical revision. Decision summary and rationale are separate
  representations and therefore separate occurrences. A claim that is also a
  positive stable-memory-domain decision appears in both classes; the two
  occurrences share payload bytes and remain distinct (dual membership).
- `repository_id` is a stable identifier assigned under the frozen source
  policy. `object_format` is `sha1` or `sha256`. `oid` is the full lowercase
  hex object identifier for that format; short or mixed-case identifiers are
  refused. Permitted refs and the traversal boundary are frozen by the source
  policy version and recorded with the descriptor, not carried in the tuple.
  The evidence is the exact commit message, never a synthesized diff or
  concatenated metadata.
- Tool identity names the parent message, the tool call or native result, the
  result revision, and the output block. Evidence is the exact UTF-8 string the
  Rust codec received before reduction, normalization, rendering, or
  truncation. Envelopes and harness-removed bytes are not evidence. Output and
  error strings are separate representations.

Every identity value is a stable identifier, not content: it is nonempty, at
most 512 bytes, and holds no control character. A value outside those bounds
is refused, and each class field appears exactly once.

Revision is the canonical decimal spelling of a nonnegative integer that fits
a signed 64-bit value: the canonical revision for kernel objects, the host
record revision for messages, and the descriptor revision for retained git
evidence. An empty revision is refused as missing; any other spelling (a
leading zero, a sign, letters) is refused as malformed, so one number has one
identifier.

## CC3. Representation and span convention

The default selection is the complete block: the whole payload of one
representation. A narrower selection is a half-open byte range `[start, end)`
within one buffer whose bounds both fall on UTF-8 character boundaries. A
reversed range, a range past the end of the buffer, or a bound inside a
multibyte sequence is refused.

Blocks are never joined. A selection never spans two buffers, and no separator
is invented between parts. A multipart native result keeps its parts as
separate blocks with their own `block_index`. Unsupported shapes are refused,
not converted lossily: a payload that is not a string, a span that is not a
two-element array of unsigned offsets, or any malformed bound refuses before
identity is minted.

Refusals are checked in a fixed order, and the first failing check names the
refusal: unknown class; missing or non-string identity field; unknown or
repeated identity field; malformed identity value; unknown harness; malformed
object identifier; missing revision; malformed revision; unknown
representation; payload not a string; malformed span; reversed span; span past
the end of the buffer; span bound inside a multibyte sequence. The fixtures
include multi-fault records that pin this order.

A span that selects every byte of its buffer is the whole-block selection and
is normalized to it before an identifier is minted, so one occurrence has one
identifier however the producer spelled the selection.

An empty buffer with a whole-block selection is a valid occurrence. Empty
output is evidence that a tool produced nothing, and the fixtures keep it.

## CC4. Occurrence tuple encoding and collision response

The occurrence tuple is `(class, namespaced_identity, revision,
representation, span)`. Its byte encoding is length-delimited so no value can
imitate a field boundary:

```text
0x02                                  encoding version
0x00                                  role: occurrence
len32(class) class
count32                               number of identity fields, in CC2 order
  len32(name) name len32(value) value  once per field
len32(revision) revision
len32(representation) representation
0x00                                  whole block
| 0x01 start64 end64                  half-open byte range
```

`len32`, `count32`, `start64`, and `end64` are big-endian unsigned integers.
The occurrence identifier is SHA-256 over the encoding. The encoding bytes are
retained beside the identifier; on a digest collision the full tuple bytes are
compared, and unequal tuples are refused. No suffix, counter, or rename
resolves a collision.

The lineage of an occurrence is every revision of one source at one
representation and span: the same encoding with role byte `0x01` and the
revision omitted. The lineage identifier is SHA-256 over that encoding. The
role byte keeps a lineage identifier from ever equalling an occurrence
identifier. A newer revision supersedes the live descriptor of its lineage;
two occurrences with different representations or spans are different
lineages and never supersede each other.

Two identity values that differ only in where a separator character sits, such
as `session_id = "a|b", message_id = "c"` against `session_id = "a",
message_id = "b|c"`, produce different encodings. The fixtures include this
case.

## CC5. Payload identity

The payload identifier is SHA-256 over the exact selected bytes. Occurrences
with equal selected bytes may share storage under one payload identifier.
Equal digests are not equal bytes: storage compares the full byte strings and
refuses unequal bytes with equal digests rather than replacing or suffixing
either. Payload identity never replaces occurrence identity and never collapses
multiplicity; two occurrences over the same bytes remain two occurrences.

## CC6. Exact-or-refused scanning before persistence

Raw evidence passes the existing bounded secret scanning in
`crates/secret-scanner` before persistence. The outcome is exact or refused.
Persistence stores the exact admitted bytes or nothing. Scanning never
rewrites bytes into storage, and a redacted form is never persisted as
evidence. A scan failure or a scan that would rewrite bytes refuses with a
non-content reason that names no secret. Existing redacting ingestion paths
keep their established output; this contract adds no secret-retention lane and
does not discover rewritten digests after persistence.

## CC7. Excluded inputs and remediation applicability

`domains.name` is excluded from every projection input: it is not identity and
not text. Existing `operator_remediation` changes that field without a source
revision change, so under this contract a name-only remediation changes no
projection input and creates neither a new occurrence generation nor an
erasure deadline.

This is a recorded applicability decision for the conditional records that
depend on the approved mapping. The witness matrix carries it as the
`conditional applicability` cell of
`search_projection_remediation_without_revision_change`, satisfied by the
decision plus a name-only control that reconstructs mapped input bytes before
and after a remediation and finds them equal. Any other in-place mutable
selected field needs historical bytes at the fixed sequence S or an approved
snapshot mechanism; without one, export aborts rather than reading current
bytes.

## CC8. Both-harness capability dispositions

The supported harnesses are OpenCode and Pi. Each capability has one
disposition per harness:

| Disposition | Meaning |
| --- | --- |
| `required` | Acceptance on that harness needs a witness for this capability. An unsupported required capability fails acceptance; simulating it is not evidence. |
| `optional_disabled` | Not required; stays disabled and produces no work. Its refusal never substitutes for a required path. |
| `not_applicable` | Outside RP2.1. |

| Capability | Classes | OpenCode | Pi |
| --- | --- | --- | --- |
| `message_text_block_capture` | messages | required | required |
| `durable_session_identity` | messages, raw tool spans | required | required |
| `tool_result_native_string_capture` | raw tool spans | required | required |
| `tool_result_revision_identity` | raw tool spans | required | required |
| `multipart_error_result_capture` | raw tool spans | required | required |
| `git_repository_scope_declaration` | git commits | required | required |
| `projection_hook_activation` | all five | required | required |
| `dense_raw_tool_embedding` | raw tool spans | optional_disabled | optional_disabled |
| `host_mural_rendering` | none | not_applicable | not_applicable |

Evidence status for every capability is `unwitnessed`. A disposition is a
requirement, not a claim that the harness supports the capability today. The
retained end-to-end suite runs OpenCode in Rust mode and a Pi load smoke that
makes no stored-data assertion; neither is a capability witness.

## CC9. RP2.9 limit-manifest interface

Every numeric bound RP2.1 uses is a named limit in a versioned manifest owned
by RP2.9. This contract freezes the interface, not any value.

Required manifest fields: `protocol_version`, `workload`, `hardware`,
`corpus_snapshot`, `metric`, `sampling`, `limits`, `approvers`, and
`approval_digest`.

Named limits: `export_page_rows`, `export_page_encoded_bytes`,
`export_row_standalone_encoded_bytes`, `export_live_decoded_bytes`,
`catchup_batch_commits`, `catchup_batch_encoded_bytes`,
`catchup_batch_source_bytes`, `local_transaction_rows`,
`local_transaction_bytes`, `pending_count`, `pending_bytes`,
`embedding_input_bytes`, `embedding_input_tokens`, `supervisor_slice_ms`,
`retry_attempts`, `lease_duration_ms`, `capture_reference_count`,
`capture_hold_expiry_ms`, `capture_disk_bytes`, `catchup_lag_commits`,
`B_catchup_ms`, `B_recovery_ms`, `B_authorized_recovery_ms`,
`embedding_recovery_attempts`, `query_service_opportunities`,
`physical_drain_ms`, and `decoded_heap_high_water_bytes`.

Rules:

- A missing, null, or unapproved limit fails closed at startup, reload, and
  dispatch. Test controls are explicit fixture inputs and never defaults.
- Approved product limits cannot enlarge a model's exact supported window.
- Changing a limit after a candidate run requires a new `protocol_version`;
  it cannot retroactively pass that run.
- Logical admission charges are not measured decoded-heap high water. Resource
  acceptance requires the separately approved observation mechanism and a
  budget that counts model memory, every tokenizer instance, SQLite caches,
  buffers, staging, and overlapping generations.

## CC10. Hook-to-gate map

Every RP2.1-owned hook is disabled by default and passes the same fail-closed
evaluation at every entry point: `startup`, `reload`, `dispatch` (a supervisor
slice), and `explicit` (an operator action). The evaluation checks five gates,
`class_coverage`, `freshness`, `resource`, `capability`, and `both_harness`,
against evidence bound to the hook's invalidation identity. Evidence for a
different identity is inapplicable and does not pass.

The invalidation identity is the tuple `(schema_version,
tokenizer_fingerprint, embedding_model, projection_policy_version,
identity_contract_version, limit_manifest_protocol_version,
vector_dimension, generation_epoch)`: every projection identity field except
the kernel incarnation, which changes on every kernel restart while the
evidence about the projection's content, model, and limits stays valid. A new
generation epoch or vector dimension invalidates coverage evidence, since the
vectors it counted belong to the earlier generation.

| Hook | Classes | Built by |
| --- | --- | --- |
| `search_projection.message_cleanup` | messages | #377 |
| `search_projection.embedding.bootstrap` | messages, claims, promoted memory, git | #368 |
| `search_projection.embedding.routing` | messages, claims, promoted memory, git | #368 |
| `search_projection.embedding.registry` | messages, claims, promoted memory, git | #368 |
| `search_projection.embedding.backfill` | messages, claims, promoted memory, git | #371 |
| `search_projection.embedding.identity_gc` | messages, claims, promoted memory, git | #370 |
| `search_projection.promoted_memory.embeddings` | promoted memory | #374 |
| `search_projection.git.ingest` | git | #375 |
| `search_projection.git.durable_rows` | git | #375 |
| `search_projection.git.jobs` | git | #375 |
| `search_projection.git.sweeps` | git | #376 |
| `search_projection.git.leases` | git | #376 |

The gate manifest that implements this map is U4h's work (#380). The witness
matrix requires a missing, a failed, an unsupported, and an inapplicable
evidence scenario for every hook at every entry point: 48 cells each for
missing and failed evidence, and 96 cells for unsupported and inapplicable
evidence. A valid supported activation control is required once the gate
exists; the matrix records it as a control beside the gate cells, not as a
marker cell, because markers record preconditions and never a successful
activation of disabled work.

## CC11. Storage residue

Staged, selected, and retired projection files are owner-only. Payloads are
never logged; refusal reasons and diagnostics name identities and sizes, not
content. Deletion is not secure erasure and no document or interface claims
otherwise. Restricted corpora and large crash images live only in approved
external storage; the repository and the witness records carry references,
integrity metadata, and access requirements.

## CC12. Admission dimensions

Admission is checked before materialization or decode on these dimensions:
`row_count`, `encoded_bytes`, `source_bytes`, `payload_size`,
`identity_and_range_bounds`, `logical_decoded_memory_charge`,
`local_mutation_charge`, `pending_count`, `pending_bytes`,
`embedding_input_bytes`, `embedding_input_tokens`, `capture_reference_count`,
and `capture_work`. Refusal leaves cursor, checkpoint, acknowledgement, and
inventory unchanged. A single row or commit that exceeds its standalone bound
fails explicitly; it is never split, skipped, truncated after load, or
accommodated by enlarging the bound. A row that fits alone but not in the
remaining page moves intact to the next page.

## Adoption by later tickets

| Ticket | Contracts consumed |
| --- | --- |
| P2 #353 | CC8 (capability), CC10 (invalidation identity) |
| U1a #355 | CC9, CC12 |
| U1b #356 | CC2, CC3, CC5, CC6, CC8, CC11, CC12 |
| U1c #357 | CC1-CC5, CC7 |
| U1d #358, U1e #359 | CC7, CC9, CC12 |
| U1f #360 | CC1-CC7, CC9, CC11, CC12 |
| U2a #361 | CC1-CC5, CC11 |
| U2b #362, U2c #363 | CC9, CC12 |
| U3 #364, #365, #367, #368, #369, #370, #371 | CC9, CC10 |
| U4 #372, #373, #374, #375, #376, #377, #379, #380 | CC1-CC3, CC8, CC10 |
| U5 #381-#388 | CC9, CC10, CC11 |
