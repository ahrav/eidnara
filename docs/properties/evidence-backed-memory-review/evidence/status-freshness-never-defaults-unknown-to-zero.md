# status-freshness-never-defaults-unknown-to-zero

## Discovery trigger

Specification U3 acceptance and owner decision OQ5 on #596: list, show, and
status defaults; exact rendering of the `disabled` terminal; one connection per
invocation; `cli` harness with a fresh `eidnara-review:` session; OQ9 for the
route-local omission of the ambient consumer identity.

## Evidence trail

`packages/cli/src/commands/review.ts` - argument parsing, the connection
lifecycle, the envelopes, the renderers.

`packages/cli/src/commands/review-wire.ts` - the closed vocabularies, the byte
caps, `decodePage`, `decodeSelected`, `decodeReviewStatus`.

`docs/host-wire-protocol.md` review operations and `metrics.memory_reviewer`
table; `crates/daemon/src/memory_reviewer/wire.rs` for the handler side.

Tests named in the catalog record.

## Failure scenario

A `starting` block's `swept_jobs: 0` prints as zero, reading as an idle store.

The decoder reads the block from the top of `metrics`, where `host.status`
never publishes it, so every real daemon reports an absent store and every
counter unavailable while exiting 0. The unit fixtures agreed with the decoder
rather than with the host, and the smoke asserted only the answer's `kind`.

An absent activation state prints as `unknown`, the wire's own literal for a
worker that has not evaluated yet, so a dropped field reads as a real state.

## Timing windows and dependencies

The sampler's first five minutes and its staleness window.

## What a test must construct

A `starting` block carrying zeros, an absent block, counters outside the
domain beside valid siblings, each placed under
`metrics.components.context.metrics` as the host publishes it, and the same
block at the top of `metrics`, which must not be read. A running daemon's
status must report a recognized store state.

## Investigation log

No open questions at authoring time.
