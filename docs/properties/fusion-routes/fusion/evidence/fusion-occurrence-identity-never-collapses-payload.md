# fusion-occurrence-identity-never-collapses-payload

## Discovery trigger

The RP2.7 identity and fusion contract fixes the occurrence identifier as the
ranking unit and forbids payload identity from collapsing occurrences. The
data-integrity lens adds the admission side: a second spelling of one
identifier, or two lanes stamped with different encoding versions, would
present one occurrence as two ranking units without touching payload bytes.

## Evidence trail

- `crates/kernel/src/source_identity.rs` encodes the occurrence tuple with
  length-delimited fields and mints the identifier as lowercase SHA-256 hex.
  `payload_id` is the digest of exact bytes only.
- `crates/retrieval/src/identity.rs` `OccurrenceId::parse` admits exactly the
  64-character lowercase hex spelling; `LaneRanking::consolidate` keys hits by
  `OccurrenceId`, keeps the lane's own best score per occurrence, orders by
  score then identifier bytes, and assigns positions once;
  `DeclaredLanes::admit` refuses a duplicate lane and mixed encoding versions.
- `crates/retrieval/tests/identity.rs` constructs both halves against the real
  kernel encoder.

## Failure scenario

Two occurrences carry the same bytes at different revisions. A ranking keyed by
payload digest merges them, and the later revision's eligibility verdict is
applied to the earlier bytes. Or a lexical probe set matches one occurrence
three times and a per-probe vote ranks it above a single stronger match.

## Timing windows and dependencies

None. Consolidation and admission are pure functions over values.

## What a test must construct

- Occurrences sharing `BUFFER` that differ in one tuple component each, with
  the payload digest held equal.
- Hits for one occurrence from several probe ordinals and several generations
  with differing raw scores, asserting the retained score and origin.
- A fixed-seed random hit list, shuffled and partly duplicated, compared
  bit-for-bit with the unshuffled ranking.
- Uppercase, prefixed, truncated, extended, and non-hex spellings.
- Two rankings for one lane and two lanes stamped with different versions.

## Investigation log

### Q: Should an uppercase identifier be normalized rather than refused?

- Sources examined: `source_identity.rs` revision canonicalization; the
  parent Q2 wording.
- Findings: the kernel refuses a non-canonical revision spelling instead of
  normalizing it, so one number has one spelling.
- Missing evidence: none.
- Conclusion: resolved with answer - refuse, matching the kernel discipline.
