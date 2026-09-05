# charge-release-never-silently-strands

## Discovery trigger

A silent-failure lens over the accounting code: find every place a fallible
accounting step has no `else`, no error return, and no counter. `release()` has
two such steps in five lines, and its caller marks the admission `Released`
regardless of whether either one did anything. The catalog held this at medium
confidence because the `release()` body had not been read directly. This file
reads it.

## Evidence trail

- `crates/shm-transport/src/profile.rs:498-506` is
  `AdmissionController::release`, verified by direct read. Its whole body is:

  ```rust
  fn release(&self, charges: ResourceCharges) {
      let Ok(mut accounting) = self.accounting.lock() else {
          return;
      };
      if let Some(active) = accounting.active.checked_sub(charges) {
          accounting.active = active;
          accounting.release_spans(charges.spans_per_frame);
      }
  }
  ```

  The signature returns `()`, so neither failure can be reported. The poisoned
  lock returns at `:500` and the arithmetic shortfall falls off the end of the
  `if let` at `:505` with no `else`. This confirms the catalog's reported basis.
- `profile.rs:553-556` is `Admission::release(mut self)`. It calls
  `self.controller.release(self.charges)` at `:554` and unconditionally sets
  `self.state = AdmissionState::Released` at `:555`.
- `profile.rs:570-577` is `impl Drop for Admission`. It calls `release` at `:573`
  and sets `Released` at `:574`, again unconditionally. So `AdmissionState`
  records intent, not outcome. The catalog's citation of `:570-577` is exact.
- `profile.rs:387` declares `accounting: Mutex<Accounting>`, a `std::sync::Mutex`,
  so poisoning is possible in principle.
- `profile.rs:76-90` is `ResourceCharges::checked_sub`. It applies `checked_sub`
  with `?` to `descriptors`, `arena_bytes`, `leases`, `mappings`, and
  `pinned_workers`, so a shortfall in any single field makes the whole call
  `None` and the release a complete no-op, not a partial one.
- `profile.rs:354-363` is `release_spans`. It only runs inside the successful
  branch, so a silent no-op also leaves `active_span_counts` and
  `active.spans_per_frame` stale, holding the span charge high.
- The host-side amplifiers of this defect are gone. At `9c1eb4d1`,
  `crates/host-runtime/src/provider_recovery.rs:167-179` `CandidateCustody::release`
  won the phase transition with `mem::replace` at `:169`, called
  `admission.release()` at `:171`, and returned a `true` that meant "this call won
  the terminal transition", not "the charges came back"; `shm_provider.rs:365`
  discarded even that boolean; and `provider_recovery.rs:487` still minted a new
  provider incarnation on a silent no-op. `ed487e11` deleted `CandidateCustody`,
  `provider_recovery.rs`, and provider incarnations outright. The surviving host
  caller is the unconditional `admission.release()` at
  `crates/host-runtime/src/ring_transport.rs:360`, whose result is also discarded, so a
  silent no-op is still invisible to the host. The transport-side defect below is
  unchanged.
- Existing check: none for either failure. The custody test
  `custody_releases_exactly_once_and_rejects_stale_releases`
  (`provider_recovery.rs:811` at `9c1eb4d1`) had the right oracle shape but never
  perturbed the lock or the arithmetic, and `ed487e11` deleted it with its subject.

## Failure scenario

The verified route to a charge mismatch runs through a failed quarantine.

1. Accounting is in a state where `quarantined + retained` overflows in some
   field.
2. Something calls `Admission::quarantine`. At `9c1eb4d1` the callers were the
   recovery episode's `record.quarantine()` (`provider_recovery.rs:494`, `:552`,
   `:571`), all deleted by `ed487e11`.
3. `AdmissionController::quarantine` reduces `active` at `profile.rs:520-523`,
   updates the span census at `:530`, then fails the add at `:524-527` and
   returns `Err`.
4. The caller discards the error. At `9c1eb4d1` that was
   `provider_recovery.rs:188`'s `.ok()`, after which the custody phase was already
   `Quarantined` and the record terminal. `ed487e11` deleted that code, so the
   discard now depends on whatever calls `quarantine`.
5. Because `profile.rs:563` never ran, the `Admission` drops with
   `state == Active`, and `Drop` calls `release` at `:573` for charges that were
   already subtracted.
6. If other live admissions still hold at least `charges` in every field,
   `checked_sub` succeeds and `active` is reduced a second time. `active` now
   under-reports by `charges` permanently, and `release_spans` has run twice for
   one admission. Admission subsequently accepts candidates the host cannot
   afford.
7. If they do not, `checked_sub` returns `None`, the release silently no-ops, and
   `active` is merely correct-by-accident while `quarantined` never received the
   charges at all.

The poisoning route is separate. A panic while the accounting guard is held
poisons the mutex, after which every later `release` returns at `:500` and every
`Admission::drop` still records `Released`. `active` then only ever grows, and
`snapshot()` (`profile.rs:487-496`) starts returning
`Err(AdmissionError::AccountingUnavailable)` at `:491`, so the operator loses the
counters at the same moment they stop being maintained.

## Timing windows and dependencies

Neither route needs a race. The arithmetic route needs the accounting pre-state
that `quarantine-charge-transition-is-atomic` describes, which is why these two
records are coupled: the quarantine defect is the enabling fault for this one.
The poisoning route needs a panic inside a critical section on
`profile.rs:387`'s mutex. That window is narrow by inspection: the guard is held
only inside `admit` (`:421-439`), `snapshot` (`:487-496`), `release`
(`:498-506`), and `quarantine` (`:508-532`); all arithmetic in those bodies is
`checked_*`, `release_spans` uses `saturating_sub`, and the one index into
`active_span_counts` is bounds-filtered by `span_slot`
(`profile.rs`, `.filter(|slot| *slot < MAX_SPANS)`). Note that custody's own lock
behaved differently at `9c1eb4d1`: `provider_recovery.rs:158`, `:168`, and `:184`
all used `.expect("custody lock")`, so a poisoned custody lock panicked rather
than silently degrading. `ed487e11` deleted that second lock with
`provider_recovery.rs`, leaving `profile.rs:387`'s mutex as the only one.

## What a test must construct

Two separate cases, both needing access to `AdmissionController` internals that
are private today.

For the arithmetic case, do not try to invent a mismatch directly. Drive the
verified sequence: seed `quarantined` near `u64::MAX` in one field, hold two live
admissions, call `Admission::quarantine()` on one and let it fail, then assert
`snapshot().active` still equals the charges of the one remaining admission. That
assertion fails today because of the second subtraction at `profile.rs:573`.

For the poisoning case, wrap a call that holds the guard in
`std::panic::catch_unwind` and force a panic inside the critical section. Since
no such panic is reachable through the current bodies, this needs a test-only
hook, for example a closure invoked while the guard is held, or a
`ResourceCharges` field whose `Debug`/`Drop` panics. Then assert that dropping
every `Admission` leaves `snapshot()` reporting an error rather than reporting
`active == ZERO`, and assert some diagnostic fires. There is no counter to assert
against today, which is the finding.

## Investigation log

### Q: Under what conditions can `checked_sub` actually fail here?

- Sources examined: `profile.rs:498-506` (`release`), `:76-90`
  (`ResourceCharges::checked_sub`), `:421-439` (`admit`, which bounds
  `active + quarantined` against the frozen limits at `:455-482`), `:508-532`
  (`quarantine`), `:553-565` (`Admission::release` and `Admission::quarantine`),
  `:570-577` (`Drop`); and, at `9c1eb4d1`, `provider_recovery.rs:167-197`
  (custody transitions), `:487` and `:494` (the episode's release and quarantine
  calls), and `shm_provider.rs:365`, all deleted by `ed487e11`.
- Findings: one reachable construction exists, and it is not the double-release
  the catalog hypothesised. `Admission::quarantine` returning `Err` leaves
  `state == Active` (`profile.rs:563` unreached), so `Drop` at `:573` issues a
  second `release` for charges `quarantine` already subtracted at `:520-523`.
  That is a genuine charge mismatch reaching `checked_sub`. Ordinary
  double-release through the custody record is not reachable: `mem::replace` at
  `provider_recovery.rs:169` made the phase transition exactly-once at `9c1eb4d1`,
  so `admission.release()` at `:171` ran at most once per record. That record type
  is gone; `Admission::release` still consumes `self`, which is what carries the
  argument now.
- Missing evidence: whether the seeding state itself is reachable in a real
  deployment. `admit` bounds committed charges against the frozen per-profile
  limits, so reaching `u64::MAX` in `quarantined` requires limits set absurdly
  high plus a long run of quarantine events, or direct injection.
- Conclusion: resolved for the mechanism. The arithmetic is reachable, through a
  failed `Admission::quarantine` followed by `Admission::drop`. The catalog's
  parenthetical "no path constructing one has been identified" should be updated,
  and the check semantics can stay `always` rather than moving to
  `always-or-unreached`. Whether the enabling accounting state is reachable
  without injection is still open and needs the intended limit ranges.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 56, `crates/host-runtime/src/ring_transport.rs:291` now `crates/host-runtime/src/ring_transport.rs:360`: The call is no longer unconditional: the endpoint thread quarantines at `:358` when either ring is quarantined and releases at `:360` otherwise, and `Admission::release` returns `()`, so there is no result to discard.
  - line 73, `profile.rs:527-530` now `profile.rs:520-523`: At HEAD `quarantine` (`:508-532`) computes both the reduced active total (`:520-523`) and the increased quarantined total (`:524-527`) before storing either at `:528-529`, so a failed add leaves the accounting untouched.
  - line 74, `:537-540` now `:524-527`: The add now runs before any store, so its failure returns `ChargeOverflow` with `active` unreduced.
  - line 125, `profile.rs:584` now `profile.rs:573`: There is no second subtraction at HEAD: a failed `Admission::quarantine` stores nothing, so the `Drop` release at `:573` is the only subtraction and the proposed assertion would pass.
  - line 150, `:527-530` now `:520-523`: `quarantine` stores nothing until both totals succeed, so a failed quarantine subtracts no charges and `Drop` cannot double-subtract; the reachable mismatch this log recorded is closed.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
