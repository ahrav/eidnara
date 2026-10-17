# future-acceptance-needs-version-and-evidence-gates

## Discovery trigger

#597 and #709 U5: acceptance uses a separately approved addition to the context
application protocol, and an actual-evidence activation record with an
exercised stop path is owner-reviewed; disclosure `activation_state=open` alone
cannot activate acceptance. Owner decisions for this work: the operation is
`review.accept`, a closed gate answers `disabled`, and U4 is waived for coding
only, with the acceptance record shipping absent.

## Evidence trail

`crates/daemon/src/memory_reviewer/activation.rs` - `read_acceptance_record`
reads `acceptance.json` through `read_owner_only_record_held`, which keeps the
record open between requests while its path names the same unchanged inode and
reads its bytes anew on every request. It accepts only schema 1 with a nonempty
evidence reference of at most 256 bytes; `AcceptanceRecord::admits` and
`AcceptanceRecord::admits_memory_store` then require the live store pair.

`crates/daemon/src/memory_reviewer/wire.rs` - `handle_review_accept` reads the
record on every request before any store read, then reads the route's
authority, the Memory Store incarnation, and the job's receipt in one Memory
Store read. A record that does not name the live Memory Store incarnation
answers `disabled`. `crates/daemon/src/memory_reviewer/settlement.rs` -
`accept_selected_proposal` copies the receipt, and the Kernel refuses
`unavailable` before it reads anything else when its live incarnation is not the
record's; every refusal reads the live Kernel incarnation and answers `disabled`
when the record does not name it, so a closed gate answers before any other
terminal and moves no Kernel tip.

`docs/host-wire-protocol.md` 7.8 - the `review.accept` paragraph. The current
document carries no numbered protocol line, so the addition is an edit to 7.8
under that section's own rule, and the envelope keeps no version field.

## Failure scenario

The disclosure record opens the worker's gate, and acceptance follows it, so a
deployment approved only to disclose writes canonical memory on request.

## Timing windows and dependencies

The record is read per request, so removing it stops the next acceptance; a
request already inside the Kernel envelope completes.

## What a test must construct

A daemon with MODULE authority, a published proposal over a staged subject, a
disclosure record, and an acceptance record written, rewritten for another
pair, and removed around acceptances.

## Investigation log

### Q: Does a protocol number need to change?

- Sources examined: `docs/host-wire-protocol.md` sections 1 and 7.8.
- Findings: the wire carries no version field, numbered revision, or
  negotiation; 7.8's literals change by an edit to the section. #579 Q37
  predates that text.
- Missing evidence: none.
- Conclusion: resolved with answer; `review.accept` is added to 7.8's method
  list and unknown methods still answer `unrecognized_request_shape` on older
  daemons.
