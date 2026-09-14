# envelope-decode-has-no-retained-tree

## Discovery trigger

Settled plan R1 and KTD1 require derived wire serde without envelope Values.
This is an accepted prospective contract, not an assertion about HEAD.
The architecture and resource lenses find the same construction mechanism.
Their agreement is not independent corroboration.

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
The plan is worktree-only; its hash and external leads are in the catalog.

## Evidence trail

- `crates/memory-store/src/lib.rs:99-124` separates WireMessage from its
  serde mirror and gives the public type `original: Option<Value>`.
- `crates/memory-store/src/lib.rs:126-143` first decodes Value, clones it
  into WireMessageData, and stores the first tree.
- `crates/memory-store/src/lib.rs:232-264` repeats that structure for blocks.
- `crates/memory-store/src/lib.rs:145-160,266-278` serializes retained
  original when present and otherwise clones typed fields into a mirror.
- `crates/daemon/src/lib.rs:12924` decodes TransformRequest directly, but
  that does not bypass the nested custom deserializers.
- `crates/daemon/src/lib.rs:8116` uses from_value for the tree entry.
- The compared wire definitions are unchanged from `e451a2b4`.

The owning path is default-production: ordinary transform bodies reach these
types. The no-envelope representation does not exist at the inspected HEAD.
Allowed Values remain the plan's classified payload fields, not a renamed
original cache or an envelope reconstructed for later replay.

## Failure scenario

An implementation deletes `original` but retains an equivalent envelope in
another wrapper, or serializes typed data through an intermediate envelope
Value. Semantic output tests can still pass while R1 remains unmet.

The competing explanation for high allocations is serde enum buffering or
legitimate payload Values. Neither alone violates this structural condition.
Allocation counts cannot discriminate those causes without source evidence.

## Timing windows and dependencies

The condition applies at wire construction and serialization on both entries.
It does not require the complete handler to scan input only once. The probe
and compatibility walk remain at `crates/daemon/src/lib.rs:15733-15765`.
The page and non-transform tree lane remains intentionally materialized.
Internally tagged enums may buffer serde content under accepted KTD1.

## What a test must construct

Inspect final field definitions and serde implementations, then exercise a
mixed decoded request containing plain text, tool input, media, opaque data,
and provider extras through direct bytes and from_value conversion.
Require each resulting envelope to have only typed state, while payload
Values remain usable. A constructor-only fixture misses ingress decoding.

The accounting agent owns the proposed allocation-count witness and numerical
thresholds. This record does not reproduce those budgets or run measurements.
No test or build runs here. Existing old-original assertions are unaudited.

## Investigation log

### Q: Does direct TransformRequest decode already remove the envelope tree?

- Sources examined: memory-store custom serde and daemon direct entry above.
- Findings: nested WireMessage and WireBlock each allocate and retain Value.
- Missing evidence: none for the current source mechanism.
- Conclusion: resolved with answer: direct outer decode still builds both
  envelope trees at HEAD; the accepted replacement is prospective.

### Q: What final check establishes representation rather than only cost?

- Sources examined: KTD1, R10, and the current sibling-replay tests.
- Findings: no scoped check asserts derived types without envelope caches.
- Missing evidence: final source and a source-bound structural witness.
- Conclusion: unresolved, needs the implementation artifact and test-strategy
  choice; an allocation threshold alone is insufficient evidence.

## Named handoff

`/testing:test-strategy` owns the structural and behavioral witness choice.
The accounting agent owns the allocation witness. Independent analyst
`ses_f6756093fffeVjNp36S3E8pKrM` completes the supplied portfolio evaluation
on 2026-09-13. This property remains unexercised; existing enforcement and
test-adequacy checks remain unaudited.

## Typed-wire U1 execution, 2026-09-13

Branch `perf/typed-wire-u1-owned-decode`, `cargo test -p daemon --locked
--features test-support` (1,489 tests pass; `lifecycle_cli` is platform-unsupported
on the aarch64 host). `WireMessage` and `WireBlock` derive serde in
`crates/memory-store/src/lib.rs` with no `Value` field outside the typed
payloads; `source_has_no_envelope_tree_or_replay_entry` and
`message_decode_stays_within_the_allocation_budget` in
`crates/daemon/tests/typed_wire_decode_allocations.rs` are the structural and
allocation witnesses (434 events, 109,432-byte peak for 74,934 message bytes;
the restored-envelope control shows 2,185 events and 313,025 bytes).
