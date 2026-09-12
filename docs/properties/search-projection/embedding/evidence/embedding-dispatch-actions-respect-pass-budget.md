# embedding-dispatch-actions-respect-pass-budget

## Discovery trigger

The first bounded dispatcher stopped after selecting `max_jobs` valid rows, but
terminal candidates were collected separately and could exceed that bound. A
page of stale or retracted work could therefore perform 1,024 writes even when
`max_jobs` was one. Zipping kernel verdicts with candidates also made a verdict
cardinality mismatch silently drop work.

## Evidence trail

- `EmbeddingDispatcher::pass` uses one `actions` counter.
- `EligibilityVerdict::Ok` increments the counter when a job is selected.
- Retracted, superseded, stale, hidden, provider-sensitive, and malformed
  identity candidates increment it when queued for terminal disposition.
- WrongScope before the first action advances the safe-prefix cursor without
  incrementing `actions`; actionable rows freeze that cursor for the next pass.
- Candidate identity fields are validated through the public kernel candidate
  validator before a kernel batch is built.
- Invalid identity is converted into an `invalid_identity` terminal request and
  does not poison valid candidates in the same page.
- `exact_verdicts` returns `KernelError::AdmissionPolicy` unless the kernel
  verdict count exactly matches the number of valid candidates.
- Selected rows are hydrated and driven one at a time after candidate reads and
  kernel judgments, so one pass never retains every selected payload at once.
- Missing or newly ineligible selected jobs are skipped; missing generation or
  occurrence relationships remain `ProjectionError::CorruptRow`.
- Terminal requests are passed to one `SearchProjection::write_within` closure.
- The closure applies every `obsolete_embedding` call in one transaction.
- Stop events are emitted only for `Obsoletion::Marked`; `AlreadyTerminal` and
  `NoJob` are no-ops.
- If the terminal write reply is lost, reconciliation uses durable state only to
  decide whether the pass remains blocked. It emits no stop event because the
  durable state cannot attribute another writer's transition to this pass.
- A write-lock deadline returns `Blocked::SearchDeadline` without quarantine.

## Failure scenario

Set `max_jobs` to one and make the first two due rows retracted. Separate selected
and terminal counters obsolete both rows in one pass, violating the configured
action bound. If each obsoletion uses its own transaction, a failure on the
second row can also leave a partial terminal batch with unclear observer events.

A malformed source identity before valid work creates another failure. Sending
the whole page to the kernel makes one invalid candidate reject every valid row.
Silently zipping a short verdict vector instead drops the unmatched suffix.

## Timing windows and dependencies

Candidate reads and kernel judgment complete before the terminal transaction
starts. Storage failures in that phase cannot leave a terminal disposition
uncertain. Once terminal writing starts, the transaction and its deadline own
the uncertainty boundary. Selected rows are then hydrated and driven one at a
time after confirmed terminal writes.

Projection obsoletion does not cancel an already held host job. Synapse job,
byte, and retention limits bound that local residue until normal eviction.

## What a test must construct

1. Set `max_jobs` to one with at least two terminal candidates.
2. Assert exactly one row changes state and one stop event is emitted.
3. Run another pass and assert the second row changes then.
4. Corrupt one candidate's source identity and place valid work after it.
5. Assert the malformed row becomes `invalid_identity` and valid work completes
   in a later bounded action.
6. Select a row, then close or delete it before hydration and assert it is
   skipped without corruption.
7. Break a selected row's generation relationship and assert `CorruptRow`.
8. Hold the search write lock through the terminal deadline and assert no row
   changes, no stop event appears, and no quarantine is entered.
9. Supply a mismatched verdict count to the release-path helper and assert
   `AdmissionPolicy`.
10. Lose a successful terminal write reply and assert reconciliation observes
    the durable `obsolete` state without emitting an attributed stop event.

## Investigation log

### Q: What consumes action budget?

- Sources examined: every dispatcher verdict arm and terminal write call.
- Findings: Selected work and every requested terminal disposition consume one;
  WrongScope consumes only scan budget.
- Missing evidence: None for the enumerated verdict set.
- Conclusion: Resolved with answer.

### Q: Does a terminal race emit a false event?

- Sources examined: `obsolete_embedding`, `Obsoletion`, and event emission.
- Findings: Only `Marked` emits. `AlreadyTerminal` and `NoJob` emit nothing.
- Missing evidence: A direct concurrent `NoJob` integration race is not
  isolated, though the outcome branch is explicit.
- Conclusion: Resolved for confirmed writes. Unknown writes reconcile progress
  without claiming which writer performed the transition; direct `NoJob` race
  coverage remains useful.
