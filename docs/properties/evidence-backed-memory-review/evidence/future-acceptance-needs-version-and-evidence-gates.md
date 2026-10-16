# future-acceptance-needs-version-and-evidence-gates

## Discovery trigger

#597 and #709 U5: acceptance uses a separately approved addition to the context
application protocol, and an actual-evidence activation record with an
exercised stop path is owner-reviewed; disclosure `activation_state=open` alone
cannot activate acceptance. Owner decisions for this work: the operation is
`review.accept`, a closed gate answers `disabled`, and U4 is waived for coding
only, with the acceptance record shipping absent.

## Evidence trail

`crates/daemon/src/memory_reviewer/activation.rs` - `read_acceptance_gate`
reads `acceptance.json` through `read_owner_only_record` and admits only schema
1 with a nonempty evidence reference of at most 256 bytes and the live store
pair.

`crates/daemon/src/memory_reviewer/wire.rs` - `handle_review_accept` reads the
gate on every request before the job lookup and maps every refusal to a closed
terminal; `crates/daemon/src/memory_reviewer/settlement.rs` -
`accept_selected_proposal` copies the receipt and releases the Memory Store
before the Kernel call.

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
