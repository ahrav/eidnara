# rebuild-after-pruning-converges

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

P U5, line 163, requires deleting search state after outbox pruning, rebuilding
from fenced canonical state, catching up concurrent commits, and comparing
complete coverage. The liveness pass asks when that success must be observable.
P, lines 120-121, leaves values unset; L assigns approval to RP2.9.

## Evidence trail

- `crates/kernel/src/outbox.rs:578-601` actually deletes rows at/below the
  minimum consumer checkpoint. A test must confirm this loss of replay input.
- `crates/kernel/tests/kernel_outbox.rs:156-224` checks survivors after prune.
- `crates/kernel/src/envelope.rs:686-728` reads canonical live/history rows
  independently of outbox publication. The readers are not bounded export.
- `crates/kernel/src/slice/read.rs:194-215` and `:298-341` read typed rows
  rather than reconstructing them from surviving outbox events.
- `crates/kernel/tests/kernel_outbox.rs:333-414` discards an internal alignment
  projection and checks receipts/checkpoints. It does not delete/rebuild search.
- `crates/kernel/tests/kernel_proofs/obligations/o8_restart_backup.rs:323-378`
  checks outbox-position continuity after actual pruning and clean reopen.
  The reusable `Proof` fixture explicitly excludes interrupted-write durability
  (`crates/kernel/tests/kernel_proofs/harness.rs:1-13`).
- `crates/daemon/src/kernel_routes/serving.rs:12-15` has serving lag limits.
  They do not define a recovery deadline or a bound on bootstrap work.
- L, lines 131-136, requires a versioned limit/approval protocol, units, timing,
  identities, and limits before capacity enablement.
- Search database, export supervisor, and recovery driver are absent at HEAD.
  This property is `test-only` and has no claimed execution evidence.
- `crates/kernel/tests/cas_fault_injection.rs:1-6`, `:1046-1094` supplies an
  existing process-crash pattern with an explicit power-loss exclusion.
  Reuse its child/barrier/kill/reap structure; search hooks and oracles are
  missing, not a general process-control mechanism. Status: unaudited.

## Failure scenario

A repair process relies exclusively on outbox replay. Once acknowledged rows
are pruned, deleting local search state makes older canonical objects invisible
forever. A superficially successful empty rebuild can hide the omission.

Another implementation repeatedly restarts export whenever new commits arrive.
It can preserve safety but never become current, even after pressure stops.
An unbounded eventual assertion or a timeout reset on every retry misses the
intended recovery obligation. Neither failure has been observed in production.

## Timing windows and dependencies

The positive episode starts at t0 after a fault/load phase, with healthy
dependencies, retained sources, a finite target T, and approved input caps.
New canonical writes stop for the acceptance window so T is not a moving goal.
The rebuild captures a fresh S after registration; S need not predate pruning.
History/source loss during the attempt belongs to abort safety, not this
positive convergence case. A kept-disabled candidate is not successful recovery.

## What a test must construct

1. Create nonempty canonical state including corrections and deletions.
2. Advance consumers legitimately and confirm historical outbox rows are gone.
3. Remove all local search state, not merely its in-memory handle.
4. Begin recovery and introduce bounded concurrent commits during bootstrap.
5. Stop faults and pressure; admit one finite episode with fixed t0 and T.
6. Require complete canonical coverage, compatible selection, and the declared
   ack completion boundary within `B_recovery_ms` and approved work/attempt caps.
7. Use `search_projection_rebuild_missing_db_after_prune` and
   `search_projection_rebuild_quiet_window_admitted` as independent situation witnesses.
8. Report absence of approved bounds as blocked evaluation, never a pass.

## Investigation log

### Q: Which numeric bound makes this finite recovery claim executable?

- Sources examined: P Bounds/U5; I approval rules; L KTD1 and U1.
- Findings: No recovery duration, retry ceiling, or finite work envelope is
  approved. Existing serving thresholds and test fixture sizes are unrelated.
- Missing evidence: `B_recovery_ms`, rows/bytes/commits, attempt/work caps,
  dependency assumptions, and retention lifetime spanning the recovery window.
- Conclusion: Needs human input through RP2.9. These symbolic quantities are
  oracle parameters, not proposed production field names or fabricated defaults.

### Q: Is ack reconciliation inside this window or a separately bounded phase?

- Sources examined: P state sequence, U2 ack order, U5 publication requirement.
- Findings: Canonical progress must follow durable local progress. The plan
  does not give an independently timed final-ack phase.
- Missing evidence: RP2.9's precise end-of-recovery observation boundary.
- Conclusion: Needs human input. The catalog's complete episode includes ack
  reconciliation pending that decision; a later split needs an explicit bound
  for both phases, not an unbounded tail.

The separate [catch-up and authorized recovery record](../catalog.md#catchup-and-authorized-recovery-converge)
owns ordinary finite-backlog progress and recovery from Disabled. This record
keeps deletion after pruning as its required situation. Both use `always`
per admitted episode and remain RP2.9-blocked rather than inventing timeouts.
