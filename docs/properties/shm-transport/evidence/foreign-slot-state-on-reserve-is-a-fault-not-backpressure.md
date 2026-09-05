# foreign-slot-state-on-reserve-is-a-fault-not-backpressure

## Discovery trigger

Round 18 review of the PR: the corrected mapping of a foreign slot state to
`InvalidSharedState` is tested on both the producer and the consumer side, and
the catalog recorded the source-tree defect but assigned the corrected contract
to no record.

## Evidence trail

- `crates/shm-transport/src/backend/ring.rs:1294-1297`: `try_reserve` computes
  `outstanding = published - completed` and returns `ProducerError::Exhausted`
  only when `outstanding >= depth`.
- `ring.rs:1300-1311`: past that check the next slot must be `SLOT_FREE`; the
  compare-exchange to `SLOT_PRODUCER_RESERVED` maps any other observed state to
  `ProducerError::Ring(self.quarantine_with(RingError::InvalidSharedState))`,
  with the comment at `:1300-1301` stating the reasoning.
- `ring.rs:1431-1438`: `try_receive_inner` moves the next slot from
  `SLOT_PUBLISHED` to `SLOT_RECEIVER_HELD` and maps any other state to
  `InvalidSharedState`; `try_receive` wraps every error in `quarantine_with`
  (`:1399-1401`).
- `ring.rs:1345-1390`: `reserve_until` parks on `Exhausted` and returns other
  errors immediately, which is why the two mappings have different downstream
  consequences.
- Tests: `foreign_slot_state_on_reserve_is_a_fault_not_backpressure`
  (`:3960-3970`) and `impossible_slot_state_quarantines_the_receiver`
  (`:4277-4295`), both writing the slot state in-process and asserting the
  error and the quarantine.

## Failure scenario

A peer writes `PRODUCER_RESERVED` into a slot the producer's depth check says is
free. Under a regression to the source tree's mapping the producer sees
`Exhausted`, `reserve_until` parks on the capacity doorbell, nobody releases the
phantom slot, and the caller receives `Deadline` after its full budget with the
ring still admitting the next connection.

## Timing windows and dependencies

None; the state is observed at the compare-exchange.

## What a test must construct

A slot forced into each non-`FREE` state before reserve and each
non-`PUBLISHED` state before receive, asserting `InvalidSharedState` and
quarantine: present for one state on each side. Missing: the remaining states,
`reserve_until` returning without parking, and a cross-process writer.

## Investigation log

### Q: Does the producer-side test reach the mapping through the public path?

- Sources examined: `ring.rs:3960-3970`, `:1267-1312`.
- Findings: the test calls `try_reserve` after storing the foreign state, so it
  exercises the public entry and the compare-exchange it guards.
- Missing evidence: `reserve_until` on the same state.
- Conclusion: Exercised is yes for the mapping; the parking contract is named in
  the Check and left to the fault map.
