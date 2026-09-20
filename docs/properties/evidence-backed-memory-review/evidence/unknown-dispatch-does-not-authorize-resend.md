# unknown-dispatch-does-not-authorize-resend

## Discovery trigger

Specification constraint: interrupted dispatched work without an admissible
result becomes unknown, not permission to rerun for a conclusion. Decision
record Q14: `not_dispatched` is proof of no dispatch; Q20: the marker-derived
unknown scalar.

## Evidence trail

`crates/daemon/src/memory_reviewer/coordinator.rs` - `resumes_dispatched_work`
reads the markers at the receipt's generation; `prepare` returns early when it
is true; `investigate` routes to `adopt`.

`crates/daemon/src/memory_reviewer/settlement.rs` - `adopt` completes `unknown`
when no sealed row exists or when any attempt is unterminated or unknown.

`crates/daemon/tests/memory_reviewer_coordinator.rs` - the three marker-class
tests named in the catalog record; each asserts the peer's connection count.

## Failure scenario

A resumed run treats a lost answer as a spent round and sends again, exceeding
the attempts and bytes the first marker already charged.

## Timing windows and dependencies

Crash between marker commit and terminal write; owner cancellation while a
request is in flight; a recheck clock past the attempt deadline.

## What a test must construct

A planted unterminated marker, a cancelled first run, and a recheck-lapsed
`not_dispatched` marker; a live peer; assertions on `Settled`, connections, and
attempt counts.

## Investigation log

### Q: Should a failed or cancelled marker with no row complete the receipt as unknown?

- Sources examined: ticket #727 acceptance; `memory_reviewer_jobs.rs` sweep
  terminal mapping; `complete_without_content`.
- Findings: the ticket admits a new attempt only after a proven
  `not_dispatched` terminal. A failed or cancelled marker's request left the
  host, and the transcript that preceded it is gone, so continuing is not
  possible; `unknown` is the content-free completion the settlement offers. The
  sweep maps a cancelled receipt to `cancelled`.
- Missing evidence: an owner statement on the preferred terminal literal for
  these two cases.
- Conclusion: needs human input; `unknown` is recorded.
