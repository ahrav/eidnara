# kd-e02-native-memory-epoch-hard

## Discovery trigger

Existing-behavior record KD-E02 of #1060's Property Catalog section A
(`kd-e-native-memory-epoch-hard`). Ticket #1061; PR #1068 (head
`f04c54e4d`). End-to-end delivery: #1064; PR #1071 (head `a15da13a4`).
Surface: daemon and store.

Exercised status: yes - a real canonical write in a Pi-under-native
sequence, the witness the specification assigned to U1.

## Evidence trail

All references are verified at `a15da13a4`, the head of #1071, which holds
#1068 to #1071.

- The additive path reads the rendered-row revision
  (`crates/daemon/src/transform.rs:1855` `revision_signal_for_context`) and
  compares it with the stored one (`:2907-2909`); a pending epoch is
  `project_memory_epoch_pending` (`:2910`).
- HARD reason precedence: first render, legacy migration, epoch change, idle
  TTL, then `project_memory_epoch` (`transform.rs:3205-3218`).
- A HARD commit clears the pending flag (`transform.rs:3108`). Only state
  sync sets it (`crates/memory-store/src/lib.rs:12330-12332`).
- `a_canonical_memory_write_rerenders_native_pi_m0_as_hard`
  (`crates/daemon/src/pi_native_serving_tests.rs:843`): a first pass under a
  native binding renders m0 with no `<project-memory>`; a kernel
  `commit_verified_memory` write follows; the next pass is HARD with
  `project_memory_epoch`, its recipe inserts one m0 and keeps m1 and every
  submitted row from the retained output, m0 holds one block, the memory
  line appears once in the whole request, and `m1_external_revision`
  changed. A state-sync epoch bump then sets the pending flag; the next pass
  is HARD with `project_memory_epoch` and clears it. The pass after that is
  `SOFT+` with `committed: false`.
- `packages/e2e-tests/tests/pi-rust-delivery.test.ts:109` (#1071): a memory
  seeded between turns two and three reaches turn three's m0 once.

## Failure scenario

A native pass that kept the old m0 after a memory write would withhold the
memory until a later render; a pass that also left it in m1 would serve it
twice.

## Timing windows and dependencies

A canonical commit after the pass's pinned read belongs to the next pass.
The kernel write reaches the pass through the rendered-row revision, not
through the pending flag.

## What a test must construct

An initialized native session; a verified, admitted, budget-fitting memory
row written between passes; a state-sync epoch bump.

## Investigation log

### Q: Is the pending flag cleared non-vacuously?

- Sources examined: `pi_native_serving_tests.rs:843-950`;
  `memory-store/src/lib.rs:12330-12332`.
- Findings: Yes. The kernel write leaves the flag false, so the witness adds
  a state-sync epoch bump, asserts the flag true, and asserts the next pass
  clears it.
- Missing evidence: None.
- Conclusion: resolved with answer.
