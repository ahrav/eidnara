# Export and recovery: system-model passes

Date: 2026-09-10. Repository: `/local/home/ahrav/scratch/eidnara`.
Verified HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
External scope is supplied by the user: the RP2.1 plan, its linked
parent/index/research, and the local repository. No incident logs are supplied.
Source locators and source roles are collected in [the catalog](../catalog.md).

These are separate attention passes in one context. They are not independent
replications. The original nested assessment hit the harness depth limit.
Independent central analyst `ses_f7623dcccffe3Y09nW2wVoABif` has completed all
four evaluation lenses; [dispositions](../portfolio-evaluation.md) record this
author's edits and remaining implementation questions. Non-wildcard discovery
passes precede the wildcard file.
Colgrep was attempted first; its index failed to load during another update.
Dedicated search and bounded sandbox extraction supplied the fallback evidence.
During disposition, another colgrep attempt panicked while loading its index;
the cited files were inspected directly rather than treating search failure
as evidence of absence.

## Architecture and data flow

The settled plan assigns export to kernel and orchestration to daemon. The
proposed retrieval crate and search database have no implementation at this
HEAD. Source search finds no `search.sqlite`, `pending_outbox`, registration, or
acknowledgement call in daemon source. Kernel has a publisher-facing reader at
`crates/kernel/src/outbox.rs:431-477`, not a per-consumer replay reader.
Its SQL filters `published_at IS NULL`; published-but-unconsumed events need a
different replay admission contract. Lead: complete commit catch-up.

## State and persistence

Registration chooses the oldest retained commit minus one, or the pre-operation
tip on an empty outbox (`crates/kernel/src/outbox.rs:83-112`). Pruning is
inclusive through the minimum registered checkpoint (`:578-601`). Neither
operation enumerates canonical payloads. `slice_as_of` materializes decision
and observation vectors (`crates/kernel/src/slice/read.rs:162-173`).
Lead: separate fixed-S input, retention ownership, and replacement state.

## Concurrency model

Kernel connections have mutex guards; ordinary writer acquisition blocks
(`crates/kernel/src/open.rs:410-418`). Acknowledgement takes that guard and a
fenced SQLite transaction (`crates/kernel/src/outbox.rs:540-570`). The plan
requires local COMMIT and release before this acquisition. Existing staging
maintenance explicitly drops its writer before artifact GC reacquires it
(`crates/kernel/src/retention.rs:203-211`), a precedent, not the proposed
two-database lock-order implementation.

## Claimed safety guarantees

RP2.1 KTD2 and U1 promise fixed-S, exactly-once bounded export, a retention
fence before S, and explicit oversize failure. U2 and U5 promise ack after
local durability, complete commit catch-up, and prior-or-complete replacement
selection. These establish obligations, not successful implementation.
The source plan's current-code map expressly marks other APIs proposed.

## Claimed liveness guarantees

U5 requires reconstruction after search-database deletion and outbox pruning.
RP2.9 owns numeric acceptance. No recovery duration or retry ceiling is set.
The catalog must bind convergence to a finite, approved recovery window and
finite input envelope. Existing 10,000-position and 60,000-ms serving
thresholds (`crates/daemon/src/kernel_routes/serving.rs:12-15`) are not recovery
budgets. Lead: a parameterized, unexercised liveness record.

## Bug history and density

Read-only history for the four focal modules includes `3b817ad8` (canonical
memory composition) and `c51de495` (restored artifacts and pending outbox).
Commit titles only locate mechanisms; they establish no incident or root cause.
Existing claim-mirror records are invalidated. Their omitted-effect and reset
stories cannot become observed RP2.1 bugs. No supplied incident logs exist.

## Existing test strategy

`crates/kernel/tests/kernel_outbox.rs` inspects SQLite directly, with the file
gated on `test-support` at line 5. It checks boundaries, retention, lifecycle,
fencing, identity validation, and publisher replay. Slice tests check historical
visibility and decision size lookup. These are useful lower seams, not a search
rebuild campaign. No test or build is run during this discovery.

The source-reference audit also follows the existing kernel proof suite.
`crates/kernel/tests/kernel_proofs/model.rs:183-245` compares clean and
perturbed histories, and `crates/kernel/tests/support/canonical_state.rs:1-20`
defines reusable digest profiles. `Proof` restart is explicitly a clean close,
not process termination (`crates/kernel/tests/kernel_proofs/harness.rs:1-13`).
Its three-kind object model needs all-class identity/byte enrichment for export.

## Failure and degradation

Deregistration refuses a lagging consumer (`crates/kernel/src/outbox.rs:133-135`).
Abandonment records operator facts and then removes the consumer (`:172-283`).
Pause keeps it registered. Removing the last consumer makes pruning return
`NoRequiredConsumers`, not resume freely (`:577-588`). Gated route reads become
unavailable; direct canonical tip reads permit an empty consumer set
(`crates/daemon/src/kernel_routes/serving.rs:41-63`). Lead: preserve these
distinctions in default disable behavior, with the pending-consumer decision open.

## Dependencies

SQLite and filesystem durability are separate evidence domains. Recovery
research supplies the two-WAL-database atomicity warning; local source supplies
transaction ordering. No power-loss evidence is supplied or executed.
Artifact GC checks live evidence, capture pins, reservations, and grace periods
(`crates/kernel/src/cas/gc.rs:437-534`), not the proposed export fence. Holding
outbox rows does not establish that source bytes remain readable at S.

## Product context

An incomplete replacement can omit required context or resurrect a deleted
occurrence. A paused projection consumer can also withhold existing canonical
memory. The latter already has a transform property and a real-kernel
integration fixture at `crates/daemon/tests/transform_canonical_memory.rs:230-302`.
Reuse its property rather than create another withheld-versus-empty record.

## Unproven assumptions

The following are not facts at HEAD: expiring export-fence API, stable export
cursor, bounded all-class decoder, complete per-consumer history reader,
search selector, and recovery supervisor. `decision_payload_sizes_as_of`
already exists (`crates/kernel/src/slice/read.rs:140-158`); do not propose a
duplicate size-query helper. It does not bound arbitrary decoded allocations.
`known_as_of` masks invalidation, while `object_history_as_of` retains only
invalidation visible by S (`crates/kernel/src/envelope.rs:686-711`). Which
history feeds rebuild tombstones must be settled with the source-coverage owner.
