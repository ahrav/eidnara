# Typed wire decode without intermediate JSON trees

## Problem

The direct transform lane decodes a request without an outer JSON tree, but
each `WireMessage` and `WireBlock` still builds a `serde_json::Value` envelope,
clones it for typed decoding, and retains the original for serialization.
This duplicates text and container storage on every turn.

The settled plan reports 3,283 allocation events and 381,087 peak live bytes
for a 40-message, 43,780-byte messages array. Its derive-only comparison uses
427 events and 84,356 peak live bytes. These are debug-build allocation
observations from the plan, not reproduced measurements or latency evidence.
Whole-request and projection costs are separate measurement populations.

## Outcome

Decode message and block envelopes directly into owned typed fields, with no
intermediate or retained envelope `Value`. Reduce measured decode cost while
preserving plugin-emitted block identity, served block bytes, cache behavior,
stored history readability, and the frozen decode contract.

This is one specification for the settled Typed Wire Decode plan dated
2026-09-13. The repository owner is asked to approve its publication, including
the testing seams and explicit unresolved conflicts below. It does not create
implementation tickets or authorize implementation. Publication does not
declare the implementation questions resolved.

## User Stories

1. As an OpenCode user with a long conversation, I want transform decoding to
   avoid repeated envelope allocations without changing my messages or tools.
2. As a user continuing an established session, I want the same plugin blocks
   to keep their identities and stored history to remain readable after an
   upgrade.
3. As a host operator, I want both decode lanes to retain their documented
   admission and refusal behavior, with memory charges covering the data they
   actually hold.
4. As a maintainer, I want one canonical block-byte producer, explicit
   compatibility exceptions, reusable property records, and measured evidence
   that the optimization pays for itself.
5. As a reviewer, I want each PR to be cohesive, independently verifiable, and
   small enough to review without weakening milestone dependencies.

## Constraints and Invariants

### Requirements retained from the plan

These are obligations, not claims that the replacement already satisfies them.
Requirement identifiers retain traceability to the settled plan.

- **R1:** `WireMessage` and `WireBlock` decode from bytes into typed fields in
  one pass. Neither builds nor retains an envelope `Value`. Only the
  explicitly retained payload fields below remain `Value`.
- **R2:** Direct unpaged transform decode and the tree lane give the same
  acceptance, refusal code, and typed result for the same body, including
  duplicates, unknown fields, nulls, and malformed JSON. Invalid typed decode
  still falls back to the original bytes through the tree lane. Admission
  refusal does not become fallback or dispatch. A duplicate recognized field
  inside a message or block may change its reported lane to `BodyLane::Tree`,
  with the same accepted last-value result and code.
- **R3:** Unknown message and block envelope fields are discarded on typed
  conversion in every lane. The typed model is the contract. This is an
  accepted narrowing of original-JSON replay, not permission to recursively
  discard keys inside retained payload `Value` fields.
- **R4:** `FlatBlock.bytes` and `FlatBlock.content_hash` remain byte-for-byte
  equal to the baseline for every block shape the OpenCode emitter produces.
  Freeze emitter-faithful projection examples before changing serde or hashes.
- **R5:** Plugin-shaped served block bytes do not change. The only accepted
  served-byte exceptions are omission of false message metadata flags
  `synthetic`, `summary`, and `errored`, and omission of false
  `provider_executed` on daemon-built tool calls/results. True values remain.
  Both readers treat absence as the existing default. These fields, their
  defaults, and their wire names remain supported.
- **R6:** Earlier-release `raw_chunk_messages` remain decodable. Production
  history_summarizer fingerprints retain their `id:kind:bytes.len()` basis and remain
  unchanged for unchanged plugin ingress. The plan's daemon-built tool-item
  exception is retained as an unresolved reachability claim, not a blanket
  authorization to change history_summarizer fingerprints; see Q3.
- **R7:** Projection bytes and content hashes, fresh served fingerprint
  fallback, and decoded sidecar fingerprints use one canonical block-byte
  producer. Sidecar hashing excludes only `_eidnara_codec`. Equality-indexed
  receipt reuse remains distinct from fresh byte hashing.
- **R8:** Byte-cap, footprint, and resident-capacity refusal codes remain
  unchanged for their triggers. Preserve the original A1-A3 corpus outcomes.
  Re-derive string charges from actual retention on both lanes and record the
  new text-heavy admission ceiling. Both lanes' decode heap peaks must fit
  their footprint; full decode plus projection must fit the declared pool.
  The plan's witness-retuning language conflicts with its frozen-corpus stop
  condition and remains unresolved in Q6.
- **R9:** Retained-size accounting charges surviving ownership, not removed
  originals. Retained payload JSON, canonical text, metadata, capacities,
  shared backing, and owners surviving eviction remain accounted.
- **R10:** A counting-allocation regression test bounds message decode at
  16 allocation events per message and strictly less than three times the
  messages JSON bytes at peak. For 40 messages this means at most 640 events,
  without integer-division slack. This corrects U4's `events / 40 <= 16`
  expression, which would admit 679 events despite the per-message limit.
  The gate must detect reintroduced envelope trees; source inspection
  independently checks their absence.
- **R11:** Criterion measures actual typed request decode plus projection on
  frozen mixed 40- and 200-message corpora. Preserve both before/after legs and
  the plan's single W1 grouping requirement. W1's invalidated catalog status
  requires explicit reconciliation; a new run does not reactivate it.

### Property-derived preservation constraints

- R3 and R4 rely on the OpenCode plugin being the only producer of ingress
  wire messages reaching this transform path. The Pi codec is not wired into
  that path. Re-verify this assumption before U1 acceptance if another emitter
  is introduced; a new producer is not automatically covered by the golden.
- Page digests cover the original canonical page arrays before typed envelope
  normalization. Paging and non-transform routes remain on the tree lane.
  Ignored envelope data must not disappear before digest validation.
- Decoded requests and retained projections remain owned, `Send + 'static`.
  No field borrows from the request buffer. Dropping input bytes or another
  cache owner cannot invalidate a surviving request, projection, or prefix.
- Unchanged prefixes share their `Arc` shells. Projection shares the ingress
  shell when the effective synthetic flag already matches; otherwise it
  rebuilds only the shell needed for that override.
- Typed edits are visible on serialization without invalidation calls.
  No-op mutable access does not change identity. Copy-on-write edits leave
  shared peers and untouched siblings unchanged after R3 normalization.
- `block_identity_digest` indexes typed equality; a digest match still requires
  an equality recheck. `IdentityFormatter` aligns signed-zero equality
  digests, not canonical bytes. Preserve positional precedence and first
  candidate selection. A reused receipt carries the selected candidate's
  hash and length; it need not equal a fresh hash of signed-zero-equivalent
  output. Receipt reuse must not change served payload bytes.
- Preserve plugin-derived durable ingress identities, hygiene baselines,
  lineage anchors, and token/fold/protected-tail decisions. Process cache
  reset is not evidence that durable hash-derived fields reset. Accepted
  default omissions do not authorize unrelated semantic decision changes.
- Each fault or rare-state check has an independent construction witness.
  Each fixed `sometimes` marker must pass separately; aggregate coverage
  cannot conceal a missing marker. Existing checks remain unaudited until an
  adequacy review or execution establishes more.

### Inherited boundaries

The host wire contract is normative. Add, rename, or remove no wire-visible
field, literal, or error code. An accepted default omission is not field
removal. Neither store changes schema. Every fenced path retains
`synchronous=FULL`; redaction of persisted text and request-scoped panic
payloads remains intact. Crates forbidding unsafe code continue to forbid it.
No process-local cache is added; any resized cache remains accounted.
Regenerate differential goldens only where they pin an accepted R5 omission;
an unrelated golden difference is not an accepted update.

Rust is pinned to the repository toolchain. Cargo commands use `--locked`
where the subcommand resolves dependencies. No comment-lint override markers
are permitted. Benchmarks stay excluded from nextest unless they implement
the list protocol. The CI workflow determines all applicable repository gates.

### Stop conditions

Stop and report if any of these occurs:

1. A projection golden changes bytes for any plugin-emitted block.
2. Decode admission codes or outcomes change for an original A1-A3 corpus body.
3. U1's measured payoff is within noise. U2 and U4 exist to preserve identity
   and prove U1; they do not justify proceeding without the saving.
4. The change requires a wire-visible field, literal, or error-code change.

Missing evidence is not a pass. Do not resolve these stops by replacing a
frozen witness, broadening a byte exception, or silently relaxing a threshold.
Material contract conflicts below require disposition before the affected
implementation result can be accepted.

## Implementation Decisions

### KTD1: Owned typed wire envelopes

Derive `Serialize` and `Deserialize` directly on `WireMessage` and `WireBlock`,
using the existing typed field attributes. Remove the custom serde
implementations, `WireMessageData`, `WireBlockData`, `original`, `original()`,
`mark_modified`, `mark_fully_typed`, and their callers.

Keep `from_parts`, `bare`, `with_provider_extras`, `content()`, `content_mut()`,
`kind()`, and `kind_mut()`. Accessors become plain field access, not a hidden
replay-invalidation mechanism. Keep fields private rather than expanding the
public surface. Rewrite documentation and tests that promise removed replay
or invalidation behavior. `WireBlock` equality becomes `(kind, provider_extras)`;
message equality follows all typed message fields.

### KTD2: One canonical block-byte producer

Add crate-visible `pub(crate) fn canonical_block_bytes(&WireBlock) -> String`
in `served_json` over the
existing canonical serializer. Route `flatten_block`, fresh fallback in
`ServedMessage::from_message_reusing`, and
`codec::sidecar::decoded_block_fingerprint` through it. Validate UTF-8 when
converting an encoded byte vector, or write through `String`; do not introduce
unchecked conversion. Hash raw UTF-8 bytes, not a JSON-encoded string of them.
The plan's infallible signature needs reconciliation with the underlying
serializer's `Result` and projection's `WireError::UnsupportedBlock` error
arm before U2. The decode owner must justify that error disposition; this
specification does not authorize a new panic path.

On `ToolCall` and `ToolResult`, deserialize `provider_executed` with its false
default and omit it when false during serialization. The plugin emits this
member only when true. Canonical object ordering must remain equivalent to
sorted `Value` serialization, with `serde_json/preserve_order` disabled.

Daemon-built typed blocks may move from declaration-order hash input to
canonical ordering. Explicit-false and unknown-envelope ingress may change
identity under the accepted normalization; these are not plugin-emitted
shapes. Neither exception permits an unreviewed durable-state transition.
The sidecar's old typed normalization also means ordinary absent-flag tool
fingerprints change there. Preserve correct native alignment and namespace
isolation rather than claiming all old sidecar hashes remain equal.

### KTD3: Keep the remaining payload Values

No request-side `RawValue`, `Cow`, or lifetime-parameterized request model is
introduced. Requests, projections, ingress prefixes, and native attachments
survive across turns through owned `Arc` values. Owned decoding also preserves
compatibility with blocking-pool work and the separate private-input lease
design, without implementing either here.

| Field or representation | Decision and reason |
| --- | --- |
| `WireMessage.original`, `WireBlock.original` | Remove complete replay envelopes; typed state and canonical serialization replace their consumers. |
| `ToolCall.input` | Keep `Value`; readers inspect keys and fingerprint the whole value. |
| `OutputKind::Json.value`, `ErrorJson.value` | Keep `Value`; validated JSON is serialized by existing consumers. |
| `OpaqueBlock.raw` | Keep `Value`; consumers inspect `ignored` and splice the value into native parts. |
| `OpaqueBlock.source`, `.arc` | Keep `Value`; small opaque objects have no measured conversion benefit. |
| `MediaBlock.source` | Keep `Value`; codecs inspect type/data/url, and typing it does not remove the owned data string. |
| `ProviderExtras` | Keep nested ordered maps and `Value`; codec namespaces are inspected and other namespaces fingerprinted. |
| `TransformRequest.native_messages` | Keep `Vec<Arc<Value>>`; codecs inspect and rewrite native envelopes and share them across turns. |
| `TransformRequest.tail_delta` | Keep `Option<Value>`; malformed/nonobject values retain lenient full-sync fallback rather than becoming typed-decoding errors. |
| `FlatBlock.bytes` | Keep canonical `Arc<str>`; token estimation, hygiene, history_summarizer items, and divergence consume it. |
| `ServedMessage.canonical_bytes` | Keep `Arc<[u8]>`; response egress work owns its redesign. |
| Ingress/request envelope scalars | Keep their existing typed representations. |

Borrowed/raw replacement would add conversion obligations at every retention
point and complicate raw-token metering equivalence. Serde's internal tagged
enum buffering remains permitted; it is not a retained wire-envelope tree.

### KTD4: Re-derive accounting without hiding simultaneous storage

Start `RETAINED_STRING_COPIES` at one: direct decode owns typed
strings; consuming tree conversion moves strings from `Value` into typed
fields. Keep `RETAINED_NODE_COPIES` at two for tree containers coexisting
with the typed request; the direct lane may conservatively overcharge nodes.
Measure actual peaks, including escapes, failure prefixes, and fallback. If
either lane exceeds its footprint, raise the string coefficient to the
smallest passing value. Do not claim the starting value is proven.

Record the per-lane argument and largest admissible text-heavy body from the
declared resident capacity in the constant's comment. The plan estimates
roughly a threefold ceiling
increase; the final number requires measurement. Confirm that full decode
plus projection fits the declared resource envelope, including serializer
workspace and retained-owner handoff, rather than measuring two disjoint
peaks. Requested allocation sizes are not an exact process-RSS bound.

Remove only original-envelope terms from retained-size estimates. Preserve
the accounting rules for remaining payloads, canonical text, capacities,
shared backing, and concurrent holders.

### KTD5: Keep paging on the tree lane

Pages remain staged as `Vec<Value>` and validated with the existing canonical
page digest before typed conversion. Other non-transform routes retain their
generic tree path. The route probe, lane gate, and invalid-typed fallback stay
the existing mechanisms. A discovered parity gap is a stop or an explicit
design question, not permission to rewrite those mechanisms silently.

### Alternatives retained as rejected or deferred

- Decoding from `&original` removes a clone but retains both costly trees.
- Replacing originals with owned raw text still requires canonicalization and
  more machinery, without an established saving over typed fields.
- A handwritten `BlockKind` deserializer is rejected unless U4 evidence makes
  serde's tagged buffering material; the plan measured 14 events for one
  derive-only two-block assistant message.
- Typing `tail_delta`, native OpenCode messages, or media source is excluded.
- Rebuilding `TransformRequestWire` is deferred; preserve explicit-null
  `serializer_profile` semantics rather than folding in adjacent cleanup.

### Milestone boundaries, dependencies, and landing policy

Preserve the plan's units **U1, U2, and U4**; there is no U3 in this plan.

| Unit | Cohesive result | Dependency and completion boundary |
| --- | --- | --- |
| U1 | Typed envelopes, removal of replay state/callers, and justified decode/retained accounting. | Cannot land removal before canonical block identity protection is present. Original-dependent tests and comments change with their contract. |
| U2 | One canonical producer, routed consumers, frozen plugin-shape preservation, typed-only oracles, and basis agreement. | Capture the expanded before-image on eligible `main` before U1. Complete the identity-safe boundary with U1. |
| U4 | Allocation regression gate, 40/200 decode-plus-projection evidence, and property-record updates. | Before leg precedes the change; after leg uses the final tree. Update affected property records last. U4 cannot be omitted from overall completion. |

U1 also removes the originals-only JSON round trip from the benchmark corpus
builder. Remove the redundant reattached-projection benchmark and preserve its
`Arc` shell-sharing assertion in the surviving projection benchmark. These
changes retain the existing benchmark coverage rather than creating a second
implementation of projection measurement.

Implementation starts from `main` at or after `e451a2b4` (PR #523), with the
landed direct lane, meter, and canonical serializer present. The old
`search-replacement-build` branch is not a valid base. Check the actual
revision rather than trusting a stale local branch name.

The owner's amendment supersedes any one-PR-per-unit rule:

- Milestones or units may split into multiple independently landable tickets
  and PRs when needed for reviewability. This specification creates none.
- Every PR must deliver a cohesive and independently verifiable result.
- Target at most **500 changed production-source lines** per PR; the hard
  maximum is **1,000**, measured as additions plus deletions, not net change.
  Tests and documentation do not count. Do not invent additional exclusions.
- Preserve milestone boundaries and dependency ordering. In particular, no
  landable intermediate state may remove replay while still deriving ingress
  identity from declaration-order serialization. Identity-safe preparation
  may precede removal, or the coupled result may land together. A size cap
  does not authorize an unsafe intermediate state.
- Every PR independently passes its applicable repository gates.
- Before opening every PR, run independent **architecture, complexity,
  testing, over-engineering, and language-design reviews in parallel**.
  Consolidate and verify findings, fix all applicable findings, rerun affected
  checks, and leave no blocking finding unresolved. Spec enrichment does not
  satisfy these future implementation reviews.

The implementer owns verification, the U4 property updates, and eventual PRs
against `main`. Each PR description names the two R5 byte exceptions, the R2
duplicate-key lane change, relevant hash/persistence consequences, and the
interaction with the egress plan.

## Testing Seams

1. **Body entry and direct-host fixture.** Reuse the literal A2 corpus,
   module-local dispatch observations, and host fixture for exact outcomes,
   lane selection, one terminal, and absence of dispatch-side effects. Retain
   literal duplicate keys rather than constructing them with a JSON map.
   Cover raw-token first/nonfirst positions, nulls, malformed inputs, unknown
   envelopes versus payloads, and real held-pool pressure. No new framework.
2. **Projection and canonical serialization.** Freeze emitter-faithful bytes
   before serde changes. Cover every `BlockKind` and `OutputKind`, absent/true
   tool flags, typed-only false, optional media filename, opaque arc, and
   reasoning signature. The existing canonical serializer covers scalar and
   escape forms. Classify nonplugin fixtures separately, without blessing
   unlisted golden changes.
3. **Fresh fingerprints, equality reuse, and sidecar alignment.** Force fresh
   fallback and reuse separately. Compare raw hashes to frozen/literal bytes.
   Use signed zeros, positional mismatch, repeated candidates, two provider
   namespaces, and changed stamps. Agreement among consumers checks routing,
   not correctness of their shared producer. Sorted `to_value` serialization
   independently checks ordering, but shares serde field-selection logic and
   cannot replace a frozen field-set oracle.
4. **Owned prefixes, mutation, and durable history.** Reuse `Arc` sharing and
   copy-on-write checks, real stored old message rows, history_summarizer assembly, and
   preserved-store/cold-cache fixtures. Observe durable served receipts,
   hygiene baselines, lineage validation, and token/fold decisions, not only
   successful deserialization. No storage schema or restart framework is added.
5. **Allocation and resident accounting.** Reuse existing event counters and
   live/peak allocation instrumentation with isolated process-global counting.
   Measure both real decode lanes continuously through conversion/fallback;
   separately cover decode plus projection at the new ceiling. The messages
   gate uses the actual production types and a documented scope certificate.
   Never subtract unrelated peak maxima or compare a whole-request numerator
   to a messages-only denominator. The exact attribution method remains Q7.
6. **Performance evidence.** Add the planned decode-plus-projection Criterion
   group, not a projection-only or derive-mirror substitute. Freeze corpora,
   artifact identities, timing/destruction boundaries, build configuration,
   run ordering, replication, and noise rule before interpreting results.
   Preserve all four size/artifact cells. No production-latency claim follows
   from this narrower operation alone.

No new public production seam is required by the specification. The common
canonical block entry is the accepted implementation surface. Extend existing
test-support observations only where the required state is otherwise hidden.
Recommended negative controls include restoring one false-field emission,
routing one consumer around the canonical helper, changing one frozen byte,
and restoring an envelope under a different name. None has been executed.

## Acceptance Criteria

- [ ] R1-R11 and KTD1-KTD5 are satisfied, with every material open conflict
  dispositioned before accepting its affected result.
- [ ] No retained envelope, replacement replay cache, custom wire serde, or
  removed invalidation API remains. The dead-reference search is empty.
- [ ] Every plugin-shaped projection entry has zero byte/hash drift from its
  frozen before-image. Typed-only cases match literal expectations and the
  sorted typed oracle. Any nonplugin normalization diff has an explicit
  disposition; it cannot be hidden in regeneration.
- [ ] Served bytes differ only within R5 and the separately classified R3/KTD2
  nonplugin normalization domain. True/default/null distinctions are covered.
- [ ] Fresh block-byte consumers agree on the correct raw-byte basis, sidecar
  excludes exactly its namespace, and equality-based receipt selection passes
  independent expectations, including signed-zero cases.
- [ ] Old stored messages decode through actual recovery/expansion paths;
  plugin history_summarizer items, durable identities, hygiene, lineage anchors, and
  downstream policy outcomes preserve their contracts with cold caches.
- [ ] Original A1-A3 bodies retain outcomes and codes; added duplicate cases
  observe successful tree fallback; added unknown-field cases normalize
  equally on both lanes. Page digest and non-transform behavior are unchanged.
- [ ] Both decode peaks fit the re-derived footprint. Full decode plus
  projection fits the declared pool. The coefficient argument and measured
  admission ceiling are recorded with boundary witnesses.
- [ ] The 40-message, 2 KiB-result allocation gate measures at most 640 events
  and a peak strictly below `3 * messages_json_bytes` in the declared scope.
  A derive-only mirror is not the tested implementation.
- [ ] Before/after 40/200 evidence clears the predeclared payoff rule and is
  recorded under the plan's required grouping, with W1 status resolved rather
  than silently changed. A result within noise stops the whole plan.
- [ ] Relevant property records and evidence relationships are updated last;
  missing or unfired witnesses are investigated, not reported as coverage.
- [ ] Every applicable verification gate passes independently for every PR,
  production-source counts meet the landing limits, and all five parallel
  pre-PR reviews are dispositioned with no blocking finding left open.

### Verification contract

The plan's checks remain required. Commands below include the missing lock
flag and the existing benchmark feature prerequisite; these are invocation
corrections, not weakened gates. Toolchain-qualified CI commands remain the
source of truth when the workflow is stronger.

| Check | Command or required evidence |
| --- | --- |
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Workspace tests | `cargo test --workspace --locked` |
| Daemon test support, U1/U2/U4 | `cargo test -p daemon --locked --features test-support` |
| Projection golden, U2 | `cargo test -p daemon --locked -- wire_golden_projects_to_flat_blocks`; zero plugin-shaped before/final diff |
| Common bases, U2 | `cargo test -p daemon --locked --features test-support --test block_bases_agree` |
| Differential goldens, U2 | `cargo test -p daemon --locked -- differential_goldens` |
| Decode footprint, U1 | `cargo test -p daemon --locked --features test-support --test parse_charge_covers_typed_decode`; direct and tree peak cases |
| Allocation bound, U4 | `cargo test -p daemon --locked --features test-support --test typed_wire_decode_allocations` |
| Decode benchmark, both legs, U4 | `cargo bench -p daemon --locked --features bench-internals --bench hot_path -- decode` |
| Bench list protocol, U4 | `cargo bench --workspace --locked --bench '*' -- --list`; benches excluded by their required features do not prove the hot-path target's list behavior, so verify it with `bench-internals` too |
| Repository gate | `bun run check:repo` |
| Dead references, U1 | `rg 'original\(\) | mark_modified | mark_fully_typed | WireMessageData | WireBlockData' crates` returns no matches; search errors are not success |
| Full resident envelope | Continuous allocation/charge evidence through decode, projection, and owner handoff, including boundary and pressure cases |
| Property evidence | A1/A3, A2 with the two added cases, B1, history_summarizer/identity witnesses, allocation evidence, and reconciled W1 grouping |

Also pass CI's applicable all-feature Clippy, warning-free rustdoc,
no-default-feature checks, nextest shards, bench test mode, doctests,
stable-toolchain checks when distinct, native-addon/package/E2E checks, and
repository guard scripts. Path filters decide applicability; the table above
does not replace them. No implementation tests or benchmarks run during this
specification task, and planned test target names do not claim those targets
already exist.

## Domain Skill Inputs

- Routing mode: `ask-skills` invoked. The inspected installed contracts select
  the following three compatible read-only domain owners.
- `/testing:property-discovery-and-catalog` was invoked separately for decode,
  identity/served/history, and resource behavior. Each part retains its system
  model, all lens passes, check inventory, exact property records, fault map,
  evidence files, relationships, and named handoffs. A fresh-context analyst
  evaluated harness fit, risk balance, implementability, and wildcard gaps;
  targeted discovery and refinements follow that evaluation.
- `/design-review:rust-design-review` supplied ownership, equality versus
  byte identity, persistence, canonical-helper, and safe landing constraints.
  Its recommendations do not override accepted product decisions.
- `/testing:test-strategy` selected existing boundaries and discriminating
  oracles, separated golden field-set proof from common-helper agreement,
  and identified allocation-attribution and measurement-evidence gaps.
- No performance campaign ran. The evidence gate names statistics,
  experiment-design, and benchmark-execution owners for later work, without
  claiming those skills ran or inventing a noise threshold.

The reusable portfolio contains 24 active properties, one invalidated
measurement-category record retained for history, and one required payoff
evidence gate. All replacement properties remain unexercised; inventoried
existing checks remain unaudited. The payoff obligation is preserved despite
its correction from a runtime property to an evidence gate.

## Out of Scope

- Response-envelope pre-encoding, `PreparedSegment` collapse, direct-frame
  egress, and the response `to_value` step; these belong to the canonical-output
  egress plan and HP1 M4.3/#441.
- Moving paging or non-transform routes off `Value` trees, replacing the route
  gate/fallback architecture, or changing page digest semantics.
- Blocking-pool relocation (#438), shared-memory payload leases (#524), or
  implementing early private-input release.
- Request-side `RawValue`, `Cow`, lifetimes, a typed native-message model,
  typed `tail_delta`, typed media source, or public `content`/`kind` fields.
- A store schema change, durability downgrade, new cache, new transport,
  broad codec rewrite, or unrelated `TransformRequestWire` cleanup.
- Implementation tickets, code changes, PR creation, or tracker updates to
  existing items during specification publication.

## Open Questions

These questions retain contradictory evidence rather than silently changing
the settled plan. Publishing the specification does not waive their gates.

1. **Q1: Golden partitions and the existing pin.** The projection golden is
   generated from a SHA-256-pinned fixture. Ten existing tool entries contain
   explicit false; absent/true tool flags and media are missing. KTD2 permits
   normalization of nonplugin false, but the plan's final-diff wording allows
   only newly anchored typed-only differences. The specification owner must
   choose an emitter-faithful before-image and explicitly disposition those
   ten entries before U2 fixture work. Do not silently change the pin or
   relabel old ingress as daemon-built. The plugin subset's zero-diff stop
   remains unconditional.
2. **Q2: Durable state affected by normalization.** Served-output receipts,
   hygiene baseline hashes/signatures, and lineage anchors persist. Synthetic
   exclusion from ingress identity does not make all derived state ephemeral.
   The transform/store owner must demonstrate compatible behavior for old
   rows, including legacy unknown/false ingress and stamped blocks, before
   accepting U1/U2. Any new upgrade exception or schema mechanism requires
   owner approval; none is granted here.
3. **Q3: HistorySummarizer exception and byte count.** Removing
   `,"provider_executed":false` removes 26 compact bytes per tool block, or
   52 across a call/result pair, from that omission alone. The plan says 25.
   Production history_summarizer assembly excludes the synthetic todo pair, so its
   proposed changed chunk-item example does not establish a reachable path.
   The history_summarizer owner must identify any eligible daemon-built item and its
   exact expected fingerprint, or explicitly dispose of the example, before
   R6 acceptance. A test-only fabricated item cannot prove production reachability.
4. **Q4: Raw-token compatibility.** Removing sorted envelope re-decodes may
   change treatment of `$serde_json::private::RawValue` in discarded fields
   or retained payloads. Source evidence identifies the risk, not an executed
   regression. The decode owner must compare literal first/nonfirst-key cases
   against the frozen baseline and tree answer before U1 acceptance. R2 and
   the stop conditions apply; no new acceptance exception is authorized.
5. **Q5: W1 ownership and noise rule.** W1 is invalidated in the existing
   catalog, while U4 requires updating it and retaining one W1 marker. The
   property owner must reconcile the update target/status before record
   updates. The performance owner must fix artifact ownership, timing scope,
   replication, and noise/proceed rule before collecting verdict-bearing
   results. Preserve the plan-specific evidence gate meanwhile; do not
   reactivate W1 or choose a favorable rule after seeing results.
6. **Q6: Frozen refusal witnesses and wider ceiling.** KTD4 accepts a wider
   live string-charge ceiling; the plan also permits retuning threshold
   witnesses while requiring unchanged A1-A3 bodies. Preserve original bytes,
   numeric capacities, schedules, and outcomes, then add new ceiling witnesses.
   If an original case changes, stop and obtain the plan owner's disposition
   before accepting it. Length caps, node-floor behavior, error mapping,
   one-terminal behavior, and effect-free refusal are not relaxed.
7. **Q7: Allocation attribution and peak coverage.** The messages-only limits
   cannot be applied to the plan's larger native-inclusive whole-request
   measurement. The testing owner must choose a production-decode scope or
   certified isolation method before writing the U4 gate. The 640-event and
   strict `3 * messages_json_bytes` limits apply only to that declared messages
   scope. Whole-request `decode_metered::<TransformRequest>` integration
   coverage remains a separate obligation, without reusing that messages-only
   denominator. The resource owner must also explain projection workspace and
   the above-1-MiB pre-meter probe, which contradicts the broad existing A1
   charge-before-probe claim. Source ordering is verified; allocation size
   and final coverage still require measurement. Do not use spare old string
   charges as an unproved substitute for a resource argument.
8. **Q8: Adjacent exact-ingress contract.** The separate transform-edit
   specification/catalog expects cached ingress to preserve unknown fields.
   That conflicts with R3. Its integration owner must reconcile the adjacent
   contract before combining the work; this task preserves R3 and does not
   edit the neighboring artifact or expand the decode scope.

## Further Notes

### Reusable catalog and traceability

The retained repository input is `docs/properties/typed-wire-decode/README.md`.
It links the decode, identity, and resources catalogs, per-property evidence,
fault maps, independent evaluation dispositions, and the payoff evidence gate.
Its traceability table maps R1-R11 and U1/U2/U4 to stable property slugs. Keep
these artifacts available to later implementation and test authors; do not
copy their unexercised state into a completion claim.

The settled source is
`docs/plans/2026-09-13-0104-perf-typed-wire-decode-plan.md`, SHA-256
`badf0d718366bd627d453498576935ba2fd3292cfe5701b7020fa09a07c52f32`.
Source inspection is pinned to
`2e4433e6b511ae74944df8a9669c428e73915d29`; the plan's measurement baseline is
`e451a2b470ae8663b4613ca04f019a30b6d7df53`. The relevant core sources agree
between those revisions apart from the recorded unrelated line insertion.
The supplied plan is accepted source material, not a reproduced experiment.

Apply the owner's landing amendment first. Within that amendment, preserve
the plan's authority order: the settled plan; the property method and
latency-audit A1-A3/B1/W1 obligations; HP1 constraints; scoped and root agent
instructions. Repository instructions and the normative host wire contract
remain binding. Conflicting evidence is recorded above rather than resolved
by assuming a document or a green test is infallible.

Related work: [HP1 #350](https://github.com/ahrav/eidnara/issues/350),
[direct lane #435](https://github.com/ahrav/eidnara/issues/435),
[meter #436](https://github.com/ahrav/eidnara/issues/436),
[canonical serializer #426](https://github.com/ahrav/eidnara/issues/426),
[blocking pool #438](https://github.com/ahrav/eidnara/issues/438),
[egress #441](https://github.com/ahrav/eidnara/issues/441), and
[payload pools #524](https://github.com/ahrav/eidnara/issues/524).
Read issue state separately from landed-code evidence.

The egress plan's borrowed wire-view workaround for cloned originals becomes
unnecessary after derived serialization. Its fingerprint-basis concern is
addressed by the common producer here; response-envelope conversion remains
its own work. No egress, blocking, or transport milestone moves into this spec.

Work item: typed-wire-decode-83443140-3a3e-4e03-a486-ce46b1405d3f
