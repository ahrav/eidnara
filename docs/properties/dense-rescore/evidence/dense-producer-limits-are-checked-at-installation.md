# dense-producer-limits-are-checked-at-installation

## Discovery trigger

RP2.6 and #620 AC3: every approved limit has an enforcement point; invalid
capacity configuration is refused.

## Evidence trail

- `CompressedLimits::capacity` derives the pool through
  `CandidateCapacity::new` and checks it and the scan page against the
  kernel's batch.
- `HandlerCore::set_dense_vectors` refuses without dense limits and runs the
  capacity check; `CompressedProducer::rank` runs it again per request.
- Byte, row, layer, pinned-byte, and read bounds are enforced in
  `select_candidates` and `rank_compressed`.

## Failure scenario

An unapproved pool size reaches a request and allocates or judges past the
kernel's batch.

## Timing windows and dependencies

None.

## What a test must construct

- Malformed alpha, an oversized pool, oversized pages, and missing dense
  limits.

## Investigation log

### Q: Are the production values approved?

- Sources examined: #825 D23 and D27.
- Findings: `k` 64 and a pool of 256 are named there; RP2.9 approval is Q6.
- Missing evidence: owner approval.
- Conclusion: unresolved, needs Q6 owner approval.
