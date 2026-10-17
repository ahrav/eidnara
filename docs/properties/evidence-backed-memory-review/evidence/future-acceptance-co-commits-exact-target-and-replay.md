# future-acceptance-co-commits-exact-target-and-replay

## Discovery trigger

#709 U5 and #597 acceptance: action, durable dependencies, and settlement
co-commit in one Kernel envelope; proposal-level replay across caller operation
keys returns the same durable settlement after crash or cleanup and is checked
before expiry or payload access; changed input under a reused key conflicts;
changed target, cross-project request, purged or reclassified evidence, and a
changed owner refuse atomically and without enumeration. #579 Q32 and Q33 fix
the settlement contents and identity.

## Evidence trail

`crates/kernel/src/review_acceptance.rs` - `settlement_intent` derives the
reserved-producer key from the project digest, proposal id, payload digest,
generation, and both incarnations, each length-prefixed, and the request digest
from the key; it refuses a reference whose Kernel incarnation is not the run's.
`accept_review_proposal` maps a malformed caller intent to `invalid` through
`intent_refusal`, then under the writer lock checks the live Kernel incarnation
against the run's, reads the caller's receipt and the settlement receipt, and
lets `commit_intent_with_writer` run `apply`. A caller
receipt replays only when it shares its `commit_seq` with the settlement
receipt, and the replay answers the settlement receipt; any other caller
receipt conflicts. A refusal inside `apply` aborts the commit. `apply` reads the
row through `bound_review_row_in`, validates the review hold and every
disclosed input through `validate_review_evidence_in` into `Disclosed`, checks
every union member in `member_stands`, checks the target in `target_stands`,
applies the action, and records the settlement through
`Envelope::record_reserved_receipt`. `member_stands` requires the question
template member at `ProposalAcceptance::question_template_revision`, and a
capture member to be a disclosed input of the MemoryReviewer capture retention
class at its digest with `retain_until` after the hold check's clock, matching
the daemon's revalidation in `crates/daemon/src/memory_reviewer/dependencies.rs`
(`revalidate`, `expectation`) and
`crates/daemon/src/memory_reviewer/broker.rs` (`revalidate_under`).

`crates/kernel/src/envelope.rs` - `record_reserved_receipt` and
`recorded_receipt`. `crates/kernel/src/slice/write.rs` -
`retire_decision_with_cascade` returns the cascade counts that
`demote_dependents_if_authority_lost` now reports.

`crates/kernel/tests/kernel_review_acceptance.rs` - the tests named in the
catalog record.

## Failure scenario

A retry after a crash between an application and its settlement applies the
proposal a second time; or a second caller key reaches the envelope after the
review hold ended and is refused as expired although the proposal was settled.

## Timing windows and dependencies

The writer lock spans both replay checks and the envelope, so no other writer
commits between them. Replay never reads the hold, the row, or its deadline.

## What a test must construct

A sealed proposal transferred to its review hold, with a dependency record whose
union names the descriptor and its decision owner. Accept it, accept it again
under another key, release the hold and accept again, reopen and accept again.
For refusals: change the target, redirect it, swap the run's project or job,
release the hold, delete or tighten a disclosed input, retire an owner; assert
the tip and the settlement lookup are unchanged.

## Investigation log

### Q: Should settlement release the review hold?

- Sources examined: `crates/kernel/src/memory_reviewer_hold.rs`
  (`release_review_hold`), `crates/daemon/src/memory_reviewer/lifecycle.rs`.
- Findings: the reconciler keeps the hold of a selected result until it
  expires; the decision events keep supporting evidence cited after it ends.
- Missing evidence: an owner decision on whether a settled proposal stays
  readable through `review.read` until its hold expires.
- Conclusion: needs human input; the envelope leaves the hold in place.
