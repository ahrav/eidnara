# dense-invalid-query-never-enters-scoring

## Discovery trigger

RP2.6: reject non-finite and zero-norm vectors, invalid scales, and dimension
or recipe mismatches; invalid query inputs cannot enter scoring.

## Evidence trail

- `QuantizedQuery::new` (`crates/retrieval/src/dense/scalar.rs:309`) checks
  the layout and the scales' dimension, encodes through `encode`, which
  validates the query through `codec::validate`, and refuses all-zero codes.
- Scales themselves are positive and finite by construction
  (`Scales::from_values`).

## Failure scenario

A query outside the generation's predicate scores every row with a value the
contract does not define; an all-zero-code query scores every row zero and
selects candidates by identifier alone.

## Timing windows and dependencies

None.

## What a test must construct

- Each malformed query, mismatched scales, and an all-zero-code query.

## Investigation log

### Q: What does a zero-code query do?

- Sources examined: #578 Q2.
- Findings: the behavior is Q2's to freeze.
- Missing evidence: owner approval.
- Conclusion: implemented as a typed refusal; unresolved, needs Q2 owner
  approval.
