# guarded-request-handoff-precedes-network-polling

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

The remainder is read after the marker commits and a concurrent attempt slips
between; a store lock is held while the provider answers; a per-chunk write
fails mid-response and leaves a partial charge.

## Timing windows and dependencies

Between the marker transaction and the first poll of the connection.

## What a test must construct

A peer that answers only after both store owners release; a ledger that
refuses the terminal; the existing ownership tests unchanged by the usage
parameter.

## Investigation log

No open questions at authoring time.
