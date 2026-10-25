# sc-readmission-blocked-after-purge-tombstone

## Discovery trigger

Part A found that exact ingest refuses `ReAdmissionBlocked` for a purged
digest and that the daemon classifier marks it permanent. Under finite
retention, the concern (OQ21) was that identical tool bytes observed after
their first capture expired would be refused forever, and (OQ20) that the
pending-unlink arm is transient yet classified permanent.

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/cas/ingest.rs`, `artifact_is_blocked`: true when the
  digest is in `artifact_purge_tombstones` or `artifact_pending_unlinks`.
  It is checked at reservation (`ReAdmissionBlocked`), in the batch
  reservation path (`Conflict`), and in `insert_reference`
  (`ReAdmissionBlocked`). A replayed receipt returns before these checks.
- `crates/kernel/src/cas/deletion.rs`, the `ArtifactDeletionKind::Purge`
  branch of `delete_artifact`: the only `INSERT INTO
  artifact_purge_tombstones`, written together with an
  `artifact_pending_unlinks` row and pin degradation.
- `crates/kernel/src/cas/gc.rs`: the only other pending-unlink insert is in
  `prepare_startup_cas_recovery`, which re-arms a digest that already has a
  tombstone. `reclaim_candidate` deletes the pending-unlink row after the
  unlink and writes no tombstone.
- `crates/kernel/src/schema.rs`: triggers
  `artifact_purge_tombstones_no_update` and
  `artifact_purge_tombstones_no_delete` make tombstones immutable and
  permanent.
- `crates/daemon/src/harness_sources.rs`, `PublishError::is_permanent`:
  `Artifact(ReAdmissionBlocked)` is permanent;
  `crates/daemon/src/source_capture.rs`, `classify`: a permanent refusal is
  `Disposition::Terminal` (`UnitsRefusedTerminal`).
- `crates/kernel/src/local_file.rs`, `expire_captures`: expiry invalidates
  rows; it calls no deletion API.

Tests read at HEAD: `crates/kernel/tests/kernel_deletion.rs`,
`purge_tombstones_unlinks_degrades_pins_and_is_idempotent` (re-ingest of
purged bytes under a new key is `ReAdmissionBlocked`; tombstone and
eligibility `Tombstoned`) and
`purge_intent_failure_has_no_commit_and_commit_before_unlink_recovers_on_reopen`;
`crates/kernel/tests/kernel_cas.rs`,
`eligibility_matrix_includes_secret_unknown_and_tombstone` (a seeded
tombstone blocks re-ingest); `crates/kernel/tests/kernel_backup.rs`,
`a_backup_that_does_not_carry_a_purge_the_target_committed_cannot_be_restored`.

## Failure scenario

Purged bytes re-enter the store under a new evidence key, or a transient
condition makes a later identical capture fail permanently.

## Timing windows and dependencies

A purge commits tombstone and pending unlink together; GC later unlinks and
clears the pending row, and the tombstone stays. Ordinary GC of expired,
invalidated capture bytes after the 14-day grace writes neither row.

## What a test must construct

F24: an operator purge of a captured digest, then identical bytes under a
new unit key, expecting `ReAdmissionBlocked` and a terminal disposition.
F27: an expired capture reclaimed by ordinary GC, then identical bytes under
a new unit key; by code reading this admits. No test constructs F27.

## Investigation log

### Q: Is `ReAdmissionBlocked` permanent by intent for the pending-unlink arm (OQ20)?
- Sources examined: ticket-1082 "Resolved OQ20"; pr-1082; the insert sites.
- Findings: U2 resolved it as terminal for the process lifetime, expecting
  "a cleared pending-unlink then admits" after restart. By code reading every
  pending-unlink row is created with, or re-armed from, a permanent
  tombstone, so the digest stays blocked after the pending row clears.
- Missing evidence: a test of a pending unlink with no tombstone.
- Conclusion: resolved in U2; the stated recovery path does not occur at
  HEAD, recorded for the lead.

### Q: Are identical bytes refused after their capture expired and GC reclaimed them (OQ21)?
- Sources examined: ticket-1084, pr-1084 "Recorded conditions";
  `gc.rs`, `deletion.rs`, `local_file.rs`.
- Findings: pr-1084 states the purge after expiry tombstones the digest.
  Code shows expiry plus GC never tombstones; only an operator purge does.
  Re-observing the same unit replays its receipts by design.
- Missing evidence: an F27 test; the Kernel owner's OQ21 decision.
- Conclusion: needs human input (OQ21); the loss condition as written in
  pr-1084 disagrees with code, unresolved, needs an F27 test.
