# repo-policy-per-file-commit-atomic-and-pass-converges

## Discovery trigger

Plan U5 step 6 ("one `commit_before` per file with `operation_key =
project.operation_key("repository_policy", "<path>:<oid>")`"), KTD2 sequence
diagram ("committed under operation_key(path:oid)"), U5 skill candidate
`crash-consistency-and-failpoint-testing`. The brief asks what a crash
between files leaves behind and whether the next pass converges. Serves R2,
R8; forbidden outcome F6 in its crash form.

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/envelope.rs:1310-1330` `commit_prepared_with_writer`:
  one `IMMEDIATE` transaction; receipt lookup first; the closure's writes and
  the commit log row commit together or not at all.
- `envelope.rs:348-369` `guarded`: any failed mutation poisons the envelope so
  the commit fails; no partial envelope can commit.
- `envelope.rs:1279-1309` `stored_receipt`: an identical retry replays; a
  different digest conflicts.
- `crates/kernel/src/cas/ingest.rs:583-600`: artifact ingest runs its own
  `commit_with_writer` with the ingest request's intent; it is a separate
  commit from any envelope the daemon opens afterward. The plan's "one commit
  per file" therefore maps to two Kernel commits: ingest, then envelope.
  `ingest.rs:600-620`: a replayed ingest with the same digest returns the
  committed handle; a replay over different bytes is refused
  (`crates/daemon/tests/kernel_routes.rs:3681`).
- `crates/daemon/src/kernel_routes/project.rs:73-78` `operation_key`
  namespacing.
- `crates/kernel/src/admission.rs:1424-1430`: the store admits one writer.

Plan-only: the module's loop over files, the cache write after the loop, the
replay behavior on the next pass.

## Failure scenario

Legal witness: tree at Y has files F1 and F2. Pass P1: ingest(F1), envelope
(F1) commits under key `<digest>:repository_policy:F1:Y`; ingest(F2),
envelope(F2) commits under `...:F2:Y`; cache set to Y. Pass P2 at Y: cache
equal, no-op.

Crash traces and the required convergence:

1. Crash after envelope(F1) commits, before ingest(F2). Kernel: F1 lineage at
   Y, F2 lineage at X. Cache: must not be Y (set only after the loop). Next
   pass: cache != Y; ingest(F1) replays; envelope(F1) replays (same key, same
   digest); ingest(F2) and envelope(F2) commit; cache set to Y. Converged in
   one pass.
2. Crash between ingest(F2) and envelope(F2). Kernel: artifact for F2 at Y
   exists with no observation citing it. Next pass: ingest(F2) replays
   (`replayed: true`), envelope(F2) commits. Converged. The orphan artifact is
   retained under its retention class; not a correctness problem.
3. Crash after the loop but before the cache write. Next pass re-runs
   everything as replays; converged with zero new rows.

Smallest violating traces:

- The cache is written before the loop (or after F1): a crash leaves F2 at X
  and the next pass at Y is a no-op; F2's rules from an older HEAD stay live
  until HEAD moves again. F6.
- The envelope's `request_digest` includes a per-pass nonce: the next pass's
  envelope(F1) is `Conflict` (key reused), the module treats it as failure,
  and F1 never advances while HEAD is stable.
- The module stops the loop on the first failed file (for example a stale
  predecessor `NotFound`) and still writes the cache.

## Timing windows and dependencies

Process kill between any two Kernel commits of one pass; power loss after a
SQLite `COMMIT` returns but before the daemon's in-memory state updates. The
transform pass that triggered the reconcile may have already read the
canonical memory from the pre-reconcile snapshot; that pass serves old rules
once, which is the plan's accepted latency.

## What a test must construct

- A two-file fixture repository with two commits X and Y.
- A fault seam: the `test-support` ingest hooks exist for artifacts
  (`ingest.rs:307-360`); for the envelope boundary the module needs a hook
  after each file commit, or the test kills the daemon process between files
  (the e2e `RustTestHarness` can restart).
- Assert after the interrupted pass: per lineage, either all live decisions
  and the live observation are at Y or all at X; never mixed within one
  lineage; the cache is not Y.
- Run one complete pass; assert every in-bound lineage at Y, receipts for the
  earlier files `replayed: true`, and `COUNT(*)` of live decisions equals the
  expected unit count.
- `sometimes` marker: the campaign reached "interrupted after k of n files"
  for some `0 < k < n`.

## Investigation log

### Q: Is the plan's "one commit per file" literally achievable?
- Sources examined: `ingest.rs:259-300, 583-600`; `envelope.rs:733` and
  `KernelStore::commit_before`.
- Findings: artifact ingest is a store-level operation with its own commit and
  CAS staging; it cannot run inside another envelope's closure.
- Missing evidence: none.
- Conclusion: resolved with answer: two commits per file; the atomic unit the
  design needs (observation plus unit changes plus admissions) fits in the
  second. Recorded as a plan-versus-code clarification, not a conflict with
  any requirement.

### Q: Must a failed file stop the pass?
- Sources examined: plan U5.
- Findings: not stated.
- Missing evidence: design statement.
- Conclusion: needs human input (open-questions.md OQ-N2-6).
