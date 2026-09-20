# shared-path-fixture-reaches-selected-readable-proposal

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

The decoder expects a field the Kernel serializes under another name or shape,
so every real proposal refuses as malformed while the unit tests, written from
the same expectation, pass.

## Timing windows and dependencies

None.

## What a test must construct

MODULE authority on a hermetic host's root, a completed receipt, a staged
proposal with nonempty spans, and the command reading it in one process.

## Investigation log

### Q: Can the positive read be driven from TypeScript against the hermetic host?

- Sources examined: `packages/e2e-tests/src/rust-runner/hermetic-host.ts`; the
  `authority.*` control operations in `crates/daemon/src/lib.rs`; the Rust
  `publish` fixture in `crates/daemon/tests/support/memory_reviewer_publish.rs`.
- Findings: the hermetic host exposes the wire but no path to publish a
  proposal; the Rust fixture writes the stores directly. The Rust wire test
  reaches the selected read over the real handler, and the command's decoder
  is exercised on the same shape in unit tests. The joint is the wire
  document, which both sides cite.
- Missing evidence: a TypeScript-driven positive read in one process.
- Conclusion: needs human input; the direct-host fixture would need a control
  that publishes a proposal, or the e2e lane would need the Rust fixture's
  store writes.
