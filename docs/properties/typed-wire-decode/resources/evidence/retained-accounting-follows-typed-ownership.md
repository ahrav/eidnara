# retained-accounting-follows-typed-ownership

System: retained request and projection holders.
HEAD `2e4433e6b511ae74944df8a9669c428e73915d29`, 2026-09-13.
[Source register](../source-register.md) defines P and baseline B.
No new test run establishes the proposed accounting.

## Discovery trigger

P:L52 removes original-envelope terms. P:L95-L110 explicitly retains payload
Values and canonical text. The change reduces ownership, not the need to
account for all surviving representations.

## Evidence trail

1. `crates/daemon/src/retained_size.rs:96-134` recursively estimates JSON and
   provider extras. Strings and arrays use capacity; maps use entry storage
   plus a declared node-overhead approximation.
2. `:157-221` covers opaque source/raw/arc, media source, JSON/error JSON,
   content/error content, tool input, and nested provider extras.
3. `:228-260` adds the original block/message trees. These are the terms P
   removes. Inline block capacity is counted once in the message.
4. `:263-271` explicitly charges a whole shared ingress allocation per holder,
   including Arc counters. This conservative policy is not global dedup.
5. `crates/daemon/src/wire.rs:243-365` includes canonical block text, replay
   shells, identities, and frontiers. A wire block handle points into a shell;
   the shell, not each handle, owns the block backing.
6. `crates/daemon/src/lib.rs:1745-1799` retains native Values, messages, and
   tail_delta in snapshot accounting. `:1803-1812` holds the typed request.
7. `crates/daemon/src/lib.rs:23039-23072` distinguishes shared native raw
   Values from equal-but-distinct allocations. `:23340-23353` keeps active
   lease charge after cache removal.

These are ordinary production holders; tests need not enable an optional
feature to make the ownership mechanism relevant. Tests themselves remain
unaudited, including original-dependent assertions slated for replacement.

## Failure scenario

Deleting all Value charges, rather than only original charges, omits tool
inputs and native payloads. Using serialized length instead of capacity
omits spare heap. Counting the block handle and shell backing separately
duplicates ownership. Deduplicating across independently bounded holders
can release the last budget charge while an Arc still retains the payload.

Competing explanation: a remaining double charge is intentional per-holder
conservatism, rather than an estimator bug. Identify each allocation's owners
and the policy boundary before removing a term.

## Timing windows and dependencies

Observe before and after projection, reattachment, snapshot publication,
copy-on-write, eviction, and last-owner drop. Physical allocation identity and
logical equality differ. Pointer equality is supporting ownership evidence;
payload semantics still require an independent value/byte observer.

The estimate includes declared map overhead, not exact allocator size classes.
Equality with an independent estimator ledger does not alone establish RSS.
Canonical bytes remain retained after the wire representation becomes typed.

## What a test must construct

- Every surviving Value-bearing payload family with nonempty data.
- Strings and vectors whose capacity exceeds length.
- Tool input, media/opaque data, nested extras, native messages, and tail_delta.
- Canonical text surviving alongside its typed source.
- Shared shells, shared native Values, and equal distinct allocations.
- A lease that outlives cache eviction and a copy-on-write edited clone.
- An ownership ledger built from type layout, capacities, and allocation
  identity rather than calls back into the production estimator.

## Investigation log

### Q: Can all Value accounting disappear with original envelopes?

- Sources examined: P:L95-L110; `retained_size.rs:157-260`;
  `crates/daemon/src/lib.rs:1745-1766`.
- Findings: Surviving payload Values and native/tail data have real owners.
- Missing evidence: A candidate-wide ledger for all variants after removal.
- Conclusion: resolved with answer on scope. Only original-envelope terms
  disappear; candidate correctness remains unresolved.

### Q: Does sharing allow removal of a holder's charge?

- Sources examined: `crates/daemon/src/retained_size.rs:263-271`;
  `crates/daemon/src/lib.rs:23039-23072,23340-23353`.
- Findings: Per-holder conservative charges coexist with local alias-aware
  accounting. Equal content is insufficient evidence of one allocation.
- Missing evidence: Candidate observations of unchanged holder/release policy.
- Conclusion: resolved on design scope. Preserve conservative cross-holder
  charges; remove only original-envelope ownership. Candidate compliance is
  unmeasured. Independent finding 8 does not authorize a new transfer or
  global deduplication policy.

## Typed-wire U1 execution, 2026-09-13

`WireMessage` and `WireBlock` in `crates/memory-store/src/lib.rs` derive their
serde implementations and hold no `Value` outside the typed payload fields;
`source_has_no_envelope_tree_or_replay_entry` in
`crates/daemon/tests/typed_wire_decode_allocations.rs` checks the struct
definitions, the absence of handwritten serde and the `*Data` mirrors, and the
absence of `original()`, `mark_modified`, `mark_fully_typed`, `WireMessageData`,
and `WireBlockData` anywhere under `crates/`. `retained_size.rs` dropped the two
original-envelope terms; `decoded_envelope_charges_only_typed_fields` shows a
decoded message or block with unknown envelope fields charging the same bytes as
one built from parts, and the independent ledgers in `wire.rs:1024-1060` and
`transform.rs` (served retained bytes) no longer add an envelope tree. Existing
alias and cross-holder rules are unchanged. Not run: an eviction sequence with a
surviving owner under the new model.
