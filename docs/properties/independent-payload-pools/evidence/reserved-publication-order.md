# reserved-publication-order

## Discovery trigger

Cross-class bypass preserves increasing consumer Request correlations, ordinary FIFO, per-stream data before terminal, and drain before Goodbye; a channel-0 Request is not a bypass control. The specification (#524) names this record under its
acceptance section and ties it to the requirements and decisions the
`catalog.md` relationship map lists.

## Evidence trail

Resolved against the tree of this catalog's introducing commit:

- docs/payload-pool-protocol.md section 11
- `docs/host-wire-protocol.md:314`

Witness status: not yet - eligibility selection (Request correlation order, stream prefix before terminal, drain before Goodbye, no channel-0 Request bypass) is #548's host publisher and #552/#550's client publishers; this layer promises FIFO descriptor consumption only.

## Failure scenario

A reordered terminal or correlation breaks the application contract.

## Timing windows and dependencies

A blocked ordinary ticket first in queue with eligible Ping and unrelated terminal behind it.

## What a test must construct

Ordinary exhaustion with a Ping and a terminal queued behind a data frame.

Situation markers that must fire independently of the safety check:

- `host.bypass_eligible_frame_behind_blocked_data`

Check semantics: `always` - the sequence of published headers per direction satisfies the four order predicates.

## Investigation log

### Q: Is the enabling state actually reached by the named witnesses at HEAD?

- Sources examined: the files listed under the evidence trail, the test names
  in `Exercised`, and the CI workflow where the record is a gate property.
- Findings: not yet at the tree of this catalog's introducing commit; see `Exercised` for what each
  witness constructs and what it leaves unconstructed.
- Missing evidence: #548, #552, #550.
- Conclusion: unresolved, needs the named handoff.
