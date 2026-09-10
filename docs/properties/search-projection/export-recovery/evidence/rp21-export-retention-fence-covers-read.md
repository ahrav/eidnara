# rp21-export-retention-fence-covers-read

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

The persistence and failure-recovery passes find two retention domains.
P, line 82, requires registration before S and abort if history or source
retention cannot cover S. P, line 114, requires removal of partial replacement
after fence expiry or inability to hold retention. No expiry API is claimed.

## Evidence trail

- `crates/kernel/src/outbox.rs:83-112` chooses initial consumer checkpoint
  from the oldest retained commit minus one or the pre-operation tip.
- `crates/kernel/src/outbox.rs:578-601` prunes inclusively through the
  minimum registered checkpoint and rejects an empty consumer set.
- `crates/kernel/tests/kernel_outbox.rs:156-224` checks the slow-consumer
  horizon and late registration. Status: unaudited.
- `crates/kernel/tests/kernel_retention.rs:316-391` checks that staging
  cleanup neither loses unacked rows nor advances a halted checkpoint.
- `crates/kernel/src/cas/gc.rs:437-534` uses pending purge, live references,
  capture pins, ingestion reservations, and grace periods for artifact GC.
  Its protection predicate does not consult an export snapshot fence.
- `crates/kernel/src/cas/deletion.rs:1002-1017` records the consumer set
  associated with a deletion barrier at the time of that deletion.
- `crates/kernel/src/slice/read.rs:177-191` validates the requested range
  against tip, not availability of every historical source payload.
- `crates/kernel/src/envelope.rs:395-438` can replace a domain name in place
  without changing its source revision. If export consumes that field, retained
  row identity does not prove availability of the required bytes at S.
- No production export fence or search replacement supervisor exists at HEAD.
  The composite record is `test-only`, despite existing outbox registration.

## Failure scenario

Capturing S and registering afterward leaves a gap in which pruning can
remove needed events. Registering first closes that outbox gap only if the
registration is durable and retained until its obligations are transferred.

Even with outbox rows held, canonical artifact bytes may become unavailable
through their independent retention/purge path. An exporter that interprets a
missing payload as an empty row or skips it can publish incomplete state.
This is a proposed failure scenario, not an observed product incident.
An in-place `operator_remediation` presents the same valid-S question for a
required mutable field. The fence cannot justify reversing that authorized
change. Abort if S cannot be supplied; do not infer that all projected classes
depend on domain names or that remediation creates a new occurrence generation.

## Timing windows and dependencies

Observe registration COMMIT before capture of S, then each page, catch-up
interval, and publication admission. A fence invalidated during any of those
windows makes the candidate unusable. Re-registration cannot certify the
continuity of an earlier gap; a new attempt needs its own S and oracle.
An old serving consumer and a bootstrap consumer may have distinct obligations.
Source retention must honor canonical deletion authority rather than override it.

## What a test must construct

1. Attempt prune with unread export input and a valid registered fence.
2. In a separate episode, invalidate coverage after a nonfinal page.
3. Remove a required source payload without pretending outbox loss occurred.
4. Observe explicit abort and exclusion of the partial candidate from selection.
5. Inject cleanup failure and require visible failure/pending cleanup, not a
   success claim that the partial artifact was removed.
6. Record `rp21_export_prune_with_fence` and
   `rp21_export_retention_lost_mid_build` from the independent fault controller.
7. Keep positive convergence separate: that episode has retained sources for
   the entire admitted recovery window.

## Investigation log

### Q: Does consumer registration also retain all export source bytes?

- Sources examined: Outbox pruning, artifact GC, deletion barrier creation,
  and P KTD2/U1.
- Findings: Outbox pruning and artifact retention have different predicates.
  Registration alone supplies no all-class byte-retention evidence.
- Missing evidence: Export retention ownership, lifetime, validity witness,
  and forced-purge or in-place-remediation handling for each required field.
- Conclusion: Needs human input from kernel and source-coverage owners.

### Q: What completes partial replacement cleanup after a failure?

- Sources examined: P, line 114, U5; host generation staging precedent.
- Findings: P requires removal; no search cleanup/retry owner exists at HEAD.
- Missing evidence: Bounded cleanup schedule, retained error state, and RP2.9
  resource limits for abandoned partial artifacts.
- Conclusion: Needs human input; safe non-publication is required even when
  filesystem removal fails, and immediate successful deletion is not assumed.
