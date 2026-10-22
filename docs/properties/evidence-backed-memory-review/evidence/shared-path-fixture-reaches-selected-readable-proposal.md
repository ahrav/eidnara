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
- Missing evidence: none; the fixture's `review-witness` control supplies the
  publish path, and the next question records the read it drives.
- Conclusion: resolved with answer: yes, through the `review-witness` control;
  see the next question.

### Q: Does the command render a real host's selected proposal field for field?

- Sources examined: `crates/daemon/examples/direct_host_fixture/review_witness.rs`;
  `crates/daemon/tests/support/memory_reviewer_forms.rs`;
  `packages/e2e-tests/src/rust-runner/review-cli.test.ts`.
- Findings: the test raises the root to MODULE through `authority.prepare`
  begin, complete, and ack over a route. The fixture's `review-witness` control
  runs, over the host's own Kernel and Memory Store, the production selector
  over `MEMORY_CLASSES`, the selection slot's enqueue, and a production
  `Worker` pass against a scripted local TLS peer whose activation record lives
  in a private home. Two fixture forms of one decision, over evidence whose
  repository provenance the control supplies, reach completed receipts and
  selected `retain` proposals that target the decision and cite nonempty spans.
  Three production forms the materializer publishes `Sensitive` are not
  selected; enqueued anyway, they abstain `owner_sensitive` with no attempt and
  no connection. `review list --json` lists all five with their outcomes, and
  `review show --json` equals the Kernel's read of each fixture proposal with
  `policy_dependencies` withheld, which the command does not render.
- Missing evidence: none for the decoder and transport. The fixture
  provenance is test-supplied, so this is not production eligibility.
- Conclusion: resolved with answer.
