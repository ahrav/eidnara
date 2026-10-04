# claim-sources-bootstrap-closes-the-registration-race

## Discovery trigger

Ticket #891: memories committed before the claim consumer existed never
reached the search projection, and a consumer that registered after them
acknowledged past their commits. The record covers the registration, the
reconciliation and scan that follow it, replay in commit order, and the
consumer's release when the maintenance loop stops.

## Evidence trail

`crates/daemon/src/claim_sources.rs` - `register_within` commits the
registration under a key naming the tip read while the consumer is absent;
`run_slice` dispatches one bounded page per slice through `reconcile_page`,
`bootstrap_page`, and `change_page`; `bootstrap_page` publishes a scanned
decision only when its `lineage_commit_seq` is at or before the slice's
checkpoint; `change_item` retires every predecessor a commit invalidates before
publishing its successor; `Batch::admits` starts a new batch when a commit
touches a decision already in it; `commit_batch` applies one batch in one
kernel commit under per-item receipts and reports a refused batch for
item-by-item application; `release` applies retained commits and deregisters
the caught-up consumer. `crates/daemon/src/search_lifecycle_owner.rs` -
`run_slices` runs one claim slice before each lifecycle slice and
`release_claim_sources` releases the consumer at shutdown.
`crates/kernel/src/slice/read.rs` - `decision_page_within_budget` bounds each
scan page by rows and payload bytes and returns registry rows as they stood at
the requested sequence. Tests: `crates/daemon/tests/claim_sources.rs`,
`crates/daemon/tests/search_lifecycle_owner.rs`,
`crates/kernel/tests/kernel_exact_artifacts.rs`,
`crates/kernel/tests/kernel_slice.rs`, as listed in the record's
`Existing check` field.

## Failure scenario

A scan that publishes a successor before replay retires its predecessor serves
two revisions; a registration that acknowledges past unpublished memories hides
them from search; a registration that acknowledges past a retirement keeps a
retired memory searchable; a refusal that blocks forever freezes the checkpoint
that freshness gates and outbox retention read.

## Timing windows and dependencies

A correction whose successor sorts ahead of the scan cursor; a fold that keeps
the survivor's creation commit; a process restart before acknowledgement; a
budget exhausted mid-page; a deleted outbox row; a consumer removed while
descriptors are live; a stored payload that no longer parses. The slice's
kernel reads and writes wait within its `EvalBudget`, so a held reader pool
ends the slice at the budget's bound and the maintenance loop keeps its cadence.

## What a test must construct

`EpisodeFault::SkipAcknowledgement`, `ExhaustBudgetAfter`,
`LoseAcknowledgementReply`, and `FailAcknowledgement` through
`run_slice_with_fault_for_test`; a cancelled `EvalBudget`; an outbox row
removed under a registered consumer; a consumer deregistered through the
envelope; a decision payload overwritten in SQLite; a domain id longer than the
artifact store's field limit; a pooled reader held through `KernelStore::preview`
while a slice runs.

## Investigation log

No open questions at authoring time. The record's `Type` is `safety`: every
check asserts a state that must hold whenever evaluated after a bounded number
of slices, and the drain and release checks bound their slice counts
explicitly.
