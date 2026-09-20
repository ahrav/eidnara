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

## Timing windows and dependencies

The sampler's first five minutes and its staleness window.

## What a test must construct

A `starting` block carrying zeros, an absent block, counters outside the
domain beside valid siblings.

## Investigation log

No open questions at authoring time.
