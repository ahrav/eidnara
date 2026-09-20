# review-cli-preserves-shared-kernel-refusal-shapes

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

A `project_mismatch` state body is printed as the daemon's message, or a
management error's message is echoed with peer text.

## Timing windows and dependencies

None.

## What a test must construct

Each state body and each management code, with peer text in the message.

## Investigation log

No open questions at authoring time.
