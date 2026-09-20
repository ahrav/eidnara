# provider-response-budget-is-cumulative-per-job

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

Four responses of 900 KiB each pass the per-response cap and deliver 3.6 MiB
to one job; four texts of 60 KiB deliver 240 KiB.

## Timing windows and dependencies

None.

## What a test must construct

Bodies padded to 600 KiB with JSON whitespace; 40 KiB texts; explicit
remainders at the collector; assertions on the usage recorded per row, the
refusal at the crossing, and the request count after exhaustion.

## Investigation log

### Q: Where is the crossing refused, and what does the refusal record?

- Sources examined: `collect_body`, `Parser::string`, the content-length check.
- Findings: the chunk that crosses is not appended and `transport_bytes` is
  set to the bound; the text that crosses is not allocated and `text_bytes` is
  set to the bound; a declared length past the bound sets `transport_bytes`
  to the bound without reading. In each case the recorded consumption is the
  whole remainder, which is the conservative reading of a body whose true
  length is not known to have been smaller.
- Missing evidence: none.
- Conclusion: resolved with answer.
