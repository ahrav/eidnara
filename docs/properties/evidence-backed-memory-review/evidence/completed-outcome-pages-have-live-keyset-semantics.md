# completed-outcome-pages-have-live-keyset-semantics

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

The command walks every page on each invocation, or presents the cursor as a
stable snapshot, so an outcome completing behind the cursor is missed silently.

## Timing windows and dependencies

A receipt completing between two invocations with an identity ordering before
the cursor.

## What a test must construct

A page with a cursor followed by an empty page with `next: null`; one request
per invocation asserted at the connection.

## Investigation log

No open questions at authoring time.
