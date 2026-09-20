# attempt-ledger-preserves-cross-generation-ceilings

## Discovery trigger

Specification U4 acceptance and owner decision OQ10 on #584: two nullable
attempt columns written with the terminal; NULL, unterminated, and `unknown`
count as the whole ceiling; the remainder is computed in the marker
transaction across every generation; the collector and decoder charge against
the handed remainder; a failed terminal write leaves the row fully consumed;
the Memory Store baseline digest changes and the prior activation record
fails closed (OQ12 on #596).

## Evidence trail

`crates/memory-store/baseline.sql` - the two columns, their bounds, the
in-flight check, and the terminal-immutable trigger.

`crates/memory-store/src/memory_reviewer_ledger.rs` - `ResponseUsage`,
`ResponseAllowance`, `response_allowance`, the exhaustion refusal in
`commit_memory_reviewer_attempt_in_tx`, `record_attempt_terminal_in_tx`,
`DispatchOutcome::Handed::allowance`.

`crates/daemon/src/memory_reviewer/model_request.rs` - `ResponseAllowance`,
`InFlight::complete` with caller-owned `ResponseAccounting`, `collect_body`.

`crates/daemon/src/memory_reviewer/model_response.rs` -
`decode_message_within`.

`crates/daemon/src/memory_reviewer/disclosure.rs` - the allowance threaded to
`complete`, `consumed`, `end` and `finish` with usage.

Tests named in the catalog record.

## Failure scenario

A successor generation reads the ceilings afresh; a crashed attempt whose
terminal never wrote is read as having consumed nothing; a rewritten terminal
lowers a recorded consumption.

## Timing windows and dependencies

Crash between the response and the terminal write; takeover; reopen.

## What a test must construct

Completed and failed attempts with usage; an unterminated attempt at takeover;
an `unknown` terminal with bytes; a `failed` terminal with NULL bytes; direct
SQL updates of the columns; a reopen with the rows compared.

## Investigation log

### Q: Does the unterminated-is-full rule change a live run?

- Sources examined: the coordinator's round loop; `disclose`'s terminal writes;
  the resumed-run decision.
- Findings: a live run terminates every attempt before its next round, so an
  unterminated row exists only after a crash or a failed terminal write, and
  the resumed run adopts or completes unknown without sending. The ledger
  tests that committed successive markers without terminals now close each
  attempt first, as a live run does.
- Missing evidence: none.
- Conclusion: resolved with answer.
