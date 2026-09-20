# review-cli-owns-one-replay-free-connection

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

A thrown request leaves the connection open, or a managed call replays the
read after an unknown outcome.

## Timing windows and dependencies

A request that throws after publication.

## What a test must construct

A connection whose request throws each error kind; a close counter; the real
client's `isClosed` after success and refusal.

## Investigation log

No open questions at authoring time.
