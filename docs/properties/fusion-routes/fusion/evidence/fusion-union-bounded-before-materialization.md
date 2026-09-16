# fusion-union-bounded-before-materialization

## Discovery trigger

RP2.7 requires the fused union bounded before any materialization by an
RP2.9-approved limit, with the refusal naming the bound.

## Evidence trail

- `fuse` checks the union size before inserting a new occurrence and returns
  `FusionRefusal::UnionExceeds { bound }`.
- The bound is a caller-supplied `NonZeroUsize` with no default, matching the
  other retrieval bounds.

## Failure scenario

A wide query could allocate a union larger than the approved limit before any
refusal.

## Timing windows and dependencies

None. Fusion is a pure function over values.

## What a test must construct

- Overlapping lanes whose union exceeds the bound by exactly one; a union that
  fits exactly; a lane that only repeats occurrences already present.

## Investigation log

### Q: Does the check run before or after the allocation it protects?

- Sources examined: `fuse`'s loop.
- Findings: `contains_key` and `len` are read before `entry().or_default()`.
- Missing evidence: none.
- Conclusion: resolved with answer - before.
