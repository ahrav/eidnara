# fusion-parent-groups-are-not-voters

## Discovery trigger

RP2.7 KTD1 makes parent groups presentation units, not fusion voters, and
RP2.8 Q1 assigns the parent identity freeze to RP2.7.U1: the group key is
parent identity plus canonical representation revision, never parent alone.

## Evidence trail

- `crates/kernel/src/source_identity.rs` `whole_buffer_lineage_digest` re-derives the
  lineage digest of the tuple's source and representation with no span. A
  lineage-role digest never equals an occurrence-role digest.
- `crates/retrieval/src/fusion/` `ParentGroupKey::derive` pairs that
  parent with the occurrence's own revision and refuses a tuple whose derived
  columns disagree with its bytes.
- `crates/retrieval/tests/identity.rs` shows spans of one source sharing a
  key, a revision or representation change deriving another, the parent
  identifier absent from the occurrence set, and a lane ranking over the spans
  holding one entry per span.

## Failure scenario

A ranking keyed by parent would let a source with many matching spans collapse
to one vote, or let a parent key stand in for an occurrence and carry a span's
eligibility to its siblings. A group that ignored revision would merge bytes
from two versions of one message.

## Timing windows and dependencies

None for the identity clauses. The fusion clause depends on RP2.7.U2's fused
scoring never reading a parent key.

## What a test must construct

- One canonical claim at a whole-buffer span and two ranges, at one revision.
- The same ranges at a later revision and at another representation.
- A tuple presented with an altered revision, representation, or span.
- After U2: a fused ranking whose input includes parent keys and whose scores
  are unchanged by them.

## Investigation log

### Q: What is the canonical representation revision per parent?

- Sources examined: RP2.8 #629 KTD2 and Q1; kernel lineage semantics.
- Findings: each span lineage carries its own revision; no parent-level
  revision exists.
- Missing evidence: none for the identity freeze.
- Conclusion: resolved with answer - the group key uses the occurrence's own
  revision, so spans at different revisions form different groups.
