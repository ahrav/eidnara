# projection-occurrence-payload-separation

## Discovery trigger

The data-integrity lens finds KTD4's two separate identities. The wildcard adds
a same-rendered-text revision, because existing rendering digests intentionally
ignore some source changes. The property protects provenance under byte sharing,
not a particular hash function or database schema.

## Evidence trail

Provenance: [source register](../_lenses/model.md#source-register), dated
2026-09-10, Eidnara HEAD `913234433ae36a80a6e22c6aac14c7f9aab74386`.
No observed collision or identity bug is asserted.

- [RP2.1 KTD4][plan] names source class, canonical object or host record identity,
  revision, representation and span. Payload IDs name collision-checked bytes.
- [Index shared contracts][index] explicitly prohibit payload deduplication
  from collapsing occurrences across projection and downstream consumers.
- [Codec fingerprints][fingerprint] remove codec identity metadata and hash
  decoded content; [stamps][stamp] carry native indexes for alignment.
  Neither defines durable retrieval occurrence identity.
- [Rendered-memory digest][rendered] sorts rendered rows and hashes category
  and rendered line. It is a rendering signal, not source revision.
- [Workspace][workspace] contains no retrieval crate; named occurrence/payload
  allocators are absent from the inspected tracked Rust tree.

Reachability: `test-only`. Production codecs create native alignment metadata,
but no production search occurrence or collision-checked payload allocator
implements KTD4. A future fixture must reach those proposed boundaries.

## Failure scenario

Two source records contain the same bytes. A payload-based primary key silently
turns them into one occurrence, losing source or revision provenance. Deleting
one then removes the other's evidence. A similar error merges complementary
spans or a raw representation with a derived one.

A separate collision case assigns one candidate digest to unequal byte buffers.
Trusting the digest alone returns the first buffer for the second occurrence.
A real cryptographic collision need not be found: deterministic injection at
the digest boundary constructs the equality the collision check must handle.

A competing explanation is valid storage sharing. Equal payload IDs for equal
bytes are allowed. The invariant counts distinct occurrence tuples and checks
payload bytes; it does not require redundant payload copies or unequal hashes.
Likewise equal rendered text at two revisions does not make revisions identical.

## Timing windows and dependencies

Tuple encoding must be stable across input ordering, replay and process restart.
Host record identity needs its namespace, not a request-local ordinal alone.
Span offsets need one defined byte coordinate system and representation.
The exact algorithm and collision response are owner decisions.
The test only constrains their observable identity and byte-preservation result.
Shared payload reclamation must not erase another live reference; this record
observes the reference result, not a new garbage-collection algorithm.

## What a test must construct

1. Equal bytes at two host records and two source classes with distinct tuples.
2. Equal bytes at two canonical revisions, including an edit outside rendered
   content so the rendered-memory digest remains unchanged.
3. Distinct representations and complementary or overlapping spans of one source.
4. Replay in a different insertion order and after reopening the local store.
5. Forced equal digest candidates for unequal bytes, including a prefix pair.
6. Deletion of one shared-payload occurrence while another remains live.
7. A fixture-owned tuple map and captured buffers; never compute expected
   identity solely by calling the production ID function twice.
8. The equal-bytes and forced-collision markers in [fault-map](../fault-map.md).

Existing codec alignment records are linked in [existing-checks](../existing-checks.md).
They are not duplicated, and their check adequacy remains unaudited.

## Investigation log

### Q: Which encoding, namespace and span convention define the tuple?

- Sources examined: [KTD4][plan], [shared identity][index], current codec
  fingerprint/stamp code and rendered-memory digest.
- Findings: Fields are prescribed, but no retrieval type or canonical encoding
  exists. Native indexes and rendered digests serve other contracts.
- Missing evidence: Namespace scope, encoding version and byte-span convention.
- Conclusion: needs human input from projection and source-adapter owners.

### Q: What happens when payload digest candidates collide?

- Sources examined: RP2.1 KTD4 and U2, workspace and named-code searches.
- Findings: Byte collision checking is required; rejection versus disambiguation
  is not settled in an implemented allocator.
- Missing evidence: A chosen response and deterministic collision seam.
- Conclusion: needs human input. Both choices must leave existing bytes intact
  and must not merge distinct occurrence identities.

[plan]: ../../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md#L79-L84
[index]: ../../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md#L35-L43
[fingerprint]: ../../../../../crates/daemon/src/codec/sidecar.rs#L169-L174
[stamp]: ../../../../../crates/daemon/src/codec/sidecar.rs#L177-L212
[rendered]: ../../../../../crates/daemon/src/canonical_memory.rs#L43-L66
[workspace]: ../../../../../Cargo.toml#L3-L17
