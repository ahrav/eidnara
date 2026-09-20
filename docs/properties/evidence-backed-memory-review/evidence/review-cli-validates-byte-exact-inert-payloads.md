# review-cli-validates-byte-exact-inert-payloads

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

A proposal text with an escape sequence clears the terminal line; a revision
past 2^53 prints rounded; a 40 KiB text is printed whole.

## Timing windows and dependencies

None.

## What a test must construct

Oversize text, oversize identifiers, an inverted span, escape and control
sequences, unsafe integers in every integer field.

## Investigation log

No open questions at authoring time.
