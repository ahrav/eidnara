# page-and-other-routes-retain-tree-semantics

## Discovery trigger

KTD5 and the plan's scope boundary retain tree decoding for pages and every
non-transform route. R3 must not move normalization ahead of page hashing.
This specializes A2 without duplicating page-collector lifecycle properties.

System: `/local/home/ahrav/scratch/eidnara`, 2026-09-13.
HEAD: `2e4433e6b511ae74944df8a9669c428e73915d29`.
Dirty daemon lib.rs is read from git show HEAD throughout this evidence.

## Evidence trail

- `crates/daemon/src/lib.rs:15811-15814,15962-15964` treats any page key
  as present, including null, and excludes it from the direct typed route.
- `crates/daemon/src/lib.rs:8078-8084` branches to page handling on the
  parsed Value. `:9575-9609` validates the six-field envelope and types.
- `crates/daemon/src/lib.rs:9647-9653` rejects mismatched content digest
  before assembly or typed conversion.
- `crates/daemon/src/lib.rs:15007-15026` separates array content digest
  from the final-page scalar digest used for replay.
- `crates/daemon/src/lib.rs:15055-15171` reconstructs continuation values,
  strips the page envelope, and concatenates array fields as Values.
- `crates/daemon/src/lib.rs:8107-8126` performs the typed conversion after
  the tree path. `:13036-13046` retains explicit echo and facade behavior.
- `packages/opencode-plugin/src/hooks/context/module-wire.ts:148-155`
  hashes the selected page-array object using its canonical JSON function.

These are default-production branches selected by body shape, not test flags.
The synthetic two-page comparison in lib.rs does not validate digests; its
values `d0` and `d1` go directly to the assembler at `:20082-20115`.

## Failure scenario

A page is normalized to typed CK before hashing. An unknown field disappears,
so its sender-provided digest fails despite valid bytes. Conversely, dropping
the field before recomputation could make a changed raw page look unchanged.
Both failures are possible even when final decoded CK is identical.

A global typed decoder can also affect echo or status bodies that the plan
leaves on the generic path. Page and non-transform behavior need explicit
controls beside the transform happy path.

## Timing windows and dependencies

Order matters: raw page digest, collection/assembly, then typed conversion.
The final-page scalar digest is a separate existing boundary. Changing it
could authorize stale replay even if all array hashes match.
Collector timeout, generation fencing, and completed-response retention stay
in the canonical handlers catalog and are not redesigned here.

## What a test must construct

Create a valid multi-page body with an unknown CK field inside a messages
array. Freeze its raw digest independently of the new wire serializer.
Change that field without updating the digest and require digest_mismatch;
update the digest and require normal assembly followed by R3 field discard.
Keep a scalar-only edit control, a continuation item, a malformed non-array
field, and partial/null page envelopes with a valid bound route.

Compare non-transform echo's full tree, discriminator precedence, and unknown
route errors before/after. Use isolated handler state for outcome comparisons.
The generated-corpus integration test is proposed execution only; it is
ignored at `crates/daemon/tests/serialized_transform_pages.rs:11-15`.

## Investigation log

### Q: Does existing page equivalence establish pre-normalization hashing?

- Sources examined: daemon lib.rs:20082-20115, :30224-30257, and the
  serialized_transform_pages integration test.
- Findings: one unit comparison bypasses validation; scalar digest has its
  own test; the integration suite needs a generated corpus.
- Missing evidence: an explicit unknown-CK raw-digest witness against final
  serde and source-bound execution of that witness.
- Conclusion: unresolved, needs the proposed page-boundary fixture.

### Q: Is the tree route a transport compatibility fallback?

- Sources examined: host-wire-protocol.md:337 and daemon dispatch above.
- Findings: this is application-body parsing within one transport profile.
- Missing evidence: none for that boundary distinction.
- Conclusion: resolved with answer: no transport downgrade or alternate wire
  profile is implied by BodyLane::Tree.

## Named handoff

`/testing:test-strategy` owns raw-page vectors and the existing generated
TypeScript/direct-host seam. `/testing:invariant-test-review` audits parity
and digest oracles. No tests or generation scripts run in this discovery.
