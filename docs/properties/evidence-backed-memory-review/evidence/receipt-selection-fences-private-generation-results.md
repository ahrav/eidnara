# receipt-selection-fences-private-generation-results

## Discovery trigger

Specification U1 acceptance: takeover selects only its new generation, losing
generations remain private. Decision record KTD8 and Q26: provisional identity
derives from the job id and receipt generation.

## Evidence trail

`crates/kernel/src/review_staging.rs` - `provisional_result_identity(job_id,
generation)`; `sealed_review_reference` looks up one candidate id.

`crates/daemon/src/memory_reviewer/settlement.rs` - `adopt` and
`read_selected_proposal_inner` derive the candidate id from the receipt's
generation before touching any row.

`crates/daemon/tests/memory_reviewer_settlement.rs` -
`a_resumed_claim_adopts_the_durable_result_without_the_broker_or_a_new_request`
(second half: takeover to generation 2, adopt at 2 completes `unknown`, the
generation-1 row stays sealed),
`a_takeover_fences_the_losing_generation_and_selects_only_its_own_result`.

## Failure scenario

A successor at generation 2 finds the generation-1 row sealed and publishes it
as its own, binding a result to a marker and union it never produced.

## Timing windows and dependencies

Between the losing generation's Kernel envelope and the successor's first pass.

## What a test must construct

A sealed row and transferred hold at generation 1; a takeover to generation 2;
an adopt and a read at generation 2; assertions that the receipt is `unknown`,
the read is `not_selected`, and the generation-1 row is still readable by
identity.

## Investigation log

No open questions at authoring time.
