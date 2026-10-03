# dense-order-is-total-and-keeps-distinct-identities

## Discovery trigger

RP2.6: order scores descending, then occurrence-identifier bytes ascending;
preserve distinct occurrence identities even when payload bytes match.

## Evidence trail

- `rank_order` (`crates/retrieval/src/dense/score.rs:92`) compares
  `total_cmp` descending, then identifier bytes.
- `TopK::offer` (`score.rs:160`) admits a row only when it outranks the worst
  held member, keyed on score and identifier, never on payload.
- `dense_numerics.rs` `reference_order` sorts with plain comparisons.

## Failure scenario

An order keyed on payload drops one of two equal-code occurrences; an epsilon
comparison makes the cut depend on offer order.

## Timing windows and dependencies

None.

## What a test must construct

- Equal codes under two identifiers, a cut between them, negative and zero
  scores.

## Investigation log

### Q: Does a verification tolerance change selection?

- Sources examined: #578 numerical contract.
- Findings: tolerances apply to verification only; the comparator has none.
- Missing evidence: none.
- Conclusion: resolved with answer - no.
