# dense-failures-keep-their-classification-at-the-route

## Discovery trigger

RP2.6 and #620 AC6: canonical-validation failure, corruption, cancellation,
and exhaustion are not relabeled as successful degraded completion.

## Evidence trail

- `compressed_refusal` maps budget refusals to the request's terminal,
  corruption to `DenseRefusal::Corruption`, which the route answers as
  `dense_corruption`, and bounds, quarantine, and failed reads to typed
  unavailable lanes.
- `lane_status` serves a complete pool and a coverage shortfall and refuses
  every other incomplete pool.

## Failure scenario

A corrupt generation answered as a degraded but successful dense lane hides
the corruption.

## Timing windows and dependencies

Corruption between selection and the original reads.

## What a test must construct

- A served ranking, a one-byte read bound, and a rows file cut after
  selection.

## Investigation log

### Q: Is a quarantined view a corruption or an unavailable lane?

- Sources examined: `compressed_refusal`.
- Findings: the first request that finds corruption ends typed; later
  requests see the quarantined view as an unavailable lane.
- Missing evidence: none.
- Conclusion: resolved with answer - both, in that order.
