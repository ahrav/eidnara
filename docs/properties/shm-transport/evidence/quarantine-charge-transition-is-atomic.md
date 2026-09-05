# quarantine-charge-transition-is-atomic

## Citation refresh, 2026-08-30

The ring-transport refactor (`0f336d3c`, `d8bde128`, `793a973e`, `ed487e11`)
renamed `crates/host-runtime/src/shm_provider.rs` to
`crates/host-runtime/src/ring_transport.rs` and deleted `provider_recovery.rs`,
`transport_negotiation.rs`, and `transport_provider.rs`. Host-side citations below
were re-anchored against `ring_transport.rs` at `e447c927`.

Where the cited construct survives, the citation names `ring_transport.rs` and a
line re-verified against that commit. Where it does not, the original reference is
kept and prefixed `former`, so it reads as pre-refactor evidence rather than a
current location. A `former` line number is never a claim about the tree today.
Every `provider_recovery.rs` reference is `former` by definition: that module has
no successor. See the refresh note in [../catalog.md](../catalog.md).

## Discovery trigger

A fallible-step lens over the accounting transitions: for each function that
moves charges between buckets, ask what state the buckets are in on every error
return. `quarantine()` performs a subtraction, then a fallible addition, with an
early return between them.

## Evidence trail

- `crates/shm-transport/src/profile.rs:508-532` is
  `AdmissionController::quarantine`. The sequence is: acquire the accounting lock
  at `:509-512`; assign `accounting.active = accounting.active.checked_sub(
  charges).ok_or(AdmissionError::AccountingUnavailable)?` at `:520-523`; call
  `accounting.release_spans(charges.spans_per_frame)` at `:530`; build `retained`
  with `pinned_workers: 0` at `:513-517`; assign `accounting.quarantined =
  accounting.quarantined.checked_add(retained).ok_or(
  AdmissionError::ChargeOverflow)?` at `:524-527`. The catalog's line citations
  for both assignments are exact.
- The mutation at `:520-523` writes through the `MutexGuard`, so it is committed
  to the shared `Accounting` before the fallible add at `:524-527` runs. When the
  add fails, `?` returns `Err(ChargeOverflow)` with `active` already reduced and
  `quarantined` never raised.
- `profile.rs:530` also matters and the catalog does not mention it.
  `release_spans` (`profile.rs:354-363`) does
  `saturating_sub(1)` on the per-span count slot and then recomputes
  `active.spans_per_frame` as the maximum over surviving slots. That side effect
  is committed before the failure too.
- `profile.rs:561-565` is `Admission::quarantine(mut self)`. It calls
  `self.controller.quarantine(self.charges)?` at `:562` and sets
  `self.state = AdmissionState::Quarantined` at `:563`. On the error path `:563`
  never runs, so `state` remains `AdmissionState::Active`.
- `profile.rs:570-577` is `impl Drop for Admission`. Because `quarantine` takes
  `self` by value, the failing call drops the `Admission` on return. `Drop` sees
  `state == Active` at `:572` and calls `self.controller.release(self.charges)`
  at `:573`. So a failed quarantine is followed immediately by a release attempt
  for the same charges. **This second-order effect is not in the catalog and it
  changes the failure shape.**
- `profile.rs:498-506` is `release`. It returns silently on a poisoned lock at
  `:499-501` and performs the subtraction inside
  `if let Some(active) = accounting.active.checked_sub(charges)` at `:502-505`
  with no `else`. So the follow-on release either double-subtracts or silently
  no-ops, depending on whether other admissions still hold enough charge.
- `profile.rs:76-90` is `ResourceCharges::checked_sub`. Every one of
  `descriptors`, `arena_bytes`, `leases`, `mappings`, and `pinned_workers` uses
  `checked_sub` with `?`, so a shortfall in any single field makes the whole
  subtraction `None`. `spans_per_frame` is passed through unchanged, with the
  comment "A maximum, not a sum: release paths recompute it from the
  per-admission span counts in `Accounting`."
- former `crates/host-runtime/src/provider_recovery.rs:183-197` is
  `CandidateCustody::quarantine`. The discard is at former `:188`:
  `_retained: admission.quarantine().ok()`. **Correction:** the catalog cites
  former `:187`, which is the `*state = CustodyState::Quarantined {` line.
- former `provider_recovery.rs:127-134` declares `CustodyState`, and the comment at
  former `:130-133` states: "The retained record proves the charges stay
  host-accounted. `None` only when aggregate accounting itself failed; the phase
  is still terminal and storage is never reused." So the code knowingly tolerates
  the accounting failure. It addresses terminality and storage reuse. It does not
  address where the charges went.
- former `provider_recovery.rs:377-378` and former `:544-546` show the intent the failure
  breaks: both comments say charges "stay visible" when readiness goes
  `Quarantined`, matching `docs/shm-transport.md:90` (source tree; not at HEAD) and former `:112`.
- Existing check: `crates/shm-transport/tests/profile.rs:50`
  `host_admission_retains_quarantined_commitments` asserts the success path only.

## Failure scenario

1. Accounting reaches a state where `quarantined + retained` overflows in at
   least one field. Because `checked_add` on `ResourceCharges` (`profile.rs:62`)
   sums `descriptors`, `arena_bytes`, `leases`, and `mappings`, any of those near
   `u64::MAX` suffices.
2. A suspect resolves as `Uncertain` or `StaleRetry`, or the deadline fires, so
   the recovery path calls `record.quarantine()` (former `provider_recovery.rs:494`,
   former `:552`, former `:571`, former `:381`, or former `:390`).
3. `CandidateCustody::quarantine` replaces the state with `Quarantined` at
   former `:185` and calls `admission.quarantine()` at former `:188`.
4. Inside, `active` is reduced at `profile.rs:520-523` and the span census is
   updated at `:530`. The add at `:524-527` fails and returns
   `Err(ChargeOverflow)`.
5. `.ok()` at former `provider_recovery.rs:188` discards it. `_retained` becomes `None`
   and the phase stays `Quarantined`, so the record is terminal.
6. The `Admission` drops with `state == Active`, so `Drop` calls `release` again
   (`profile.rs:573`). If other admissions still hold at least `charges` in every
   field, the subtraction succeeds and `active` is reduced a second time. If not,
   `checked_sub` returns `None` and the release silently no-ops.

Either branch loses the charges from `quarantined` entirely. The double-subtract
branch additionally makes `active` under-report by `charges`, and calls
`release_spans` twice for one admission, which under-counts that span class in
`active_span_counts` and can lower `active.spans_per_frame` below the true
maximum.

## Timing windows and dependencies

No concurrency is required. The whole sequence runs under one `MutexGuard` on
`profile.rs:387`'s `Mutex<Accounting>`, and the failure is arithmetic. The
enabling state is the only hard dependency: `quarantined` must be high enough
that adding `retained` overflows. That state is not reachable through ordinary
admission, because `admit` bounds `active + quarantined` against the frozen
limits at `profile.rs:452-482`, so it needs either a seeded accounting pre-state
(fault class F9 in the fault map) or an injected failure at `:524-527`. This
property is the upstream half of
`charge-release-never-silently-strands`: the failed quarantine is a verified
construction of the charge mismatch that record's open question asks for.

## What a test must construct

Construct an `AdmissionController` whose `quarantined` bucket is already near
`u64::MAX` in one field, admit one candidate, snapshot
`active + quarantined` per field through `AdmissionController::snapshot`
(`profile.rs:487-496`), then call `Admission::quarantine()` and assert it returns
`Err(AdmissionError::ChargeOverflow)`. Then snapshot again and assert the
per-field sum `active + quarantined` is unchanged. Because `quarantined` is
private and `admit` refuses to overshoot the limits, seeding requires either a
test-only constructor for `Accounting` or a limits configuration high enough that
repeated admit-then-quarantine cycles walk `quarantined` up to the boundary. A
second case should hold a second live admission across the failing quarantine so
the `Drop`-driven `release` at `profile.rs:573` succeeds, and assert `active`
was not reduced twice.

## Investigation log

The catalog records no open questions for this property. Two findings surfaced
while verifying it and are recorded here as new questions rather than left
implicit.

### Q: Is the `Drop`-driven second release after a failed `Admission::quarantine` intended?

- Sources examined: `profile.rs:561-565` (`Admission::quarantine` signature and
  body), `:570-577` (`Drop`), `:498-506` (`release`), `:76-90`
  (`ResourceCharges::checked_sub`), `:354-363` (`release_spans`).
- Findings: `quarantine` takes `mut self`, and the `?` at `:562` returns before
  `state` is updated at `:563`, so `Drop` unavoidably runs with
  `state == Active`. There is no `ManuallyDrop`, no `mem::forget`, and no
  compensating branch. The behaviour follows directly from the ownership and the
  early return.
- Missing evidence: no comment or test addresses the error path of
  `Admission::quarantine` at all.
- Conclusion: resolved as a mechanism, unresolved as intent. The second release
  is certain to occur; whether it was considered needs the author.

### Q: Is the `AccountingUnavailable` variant at `profile.rs:530` (source tree; not at HEAD) the right classification for a `checked_sub` shortfall?

- Sources examined: `profile.rs:520-523`, the variant list at `:655-665`, and
  the descriptions at `:664` ("host admission accounting unavailable").
- Findings: `:523` maps an arithmetic shortfall in `active` to
  `AccountingUnavailable`, while the structurally similar failure at `:527` maps
  to `ChargeOverflow`. A caller cannot distinguish "the lock was unusable" from
  "the charges did not match" from the error alone.
- Missing evidence: none needed for the property; this is an observability
  point, and the catalog's `charge-release-never-silently-strands` record owns
  the observability angle.
- Conclusion: resolved as an observation. It does not change this property's
  check, and it is recorded so the shared cause is not rediscovered.

## Refresh outcome, 2026-08-30

`Reaches production:` moved from `yes` to `no`; `Status:` stays `active`. The
ordering defect this record is about is untouched: `AdmissionController::quarantine`
still decrements `active` before the fallible `checked_add` on `quarantined`, with
an early return between them. Only its driver is gone. The host caller that
discarded the error, `admission.quarantine().ok()` at the former
`crates/host-runtime/src/provider_recovery.rs:188`, was deleted by `ed487e11`.

Verified at `e447c927`: `Admission::quarantine` has no non-test caller anywhere in
the tree. A search for `.quarantine()` across `crates/` and `packages/` returns
exactly two call sites, which at `9c1eb4d1` were
`crates/shm-transport/tests/contract.rs:368` (the `OwnershipMode::DirectLeased`
field, removed from the descriptor by `0f336d3c`) and
`:539`. The `quarantine` identifiers remaining in `crates/host-runtime` are unrelated:
`LeaseTracker`'s lease quarantine in `frame_channel.rs:417-433` (source tree; not at HEAD), and the
lifecycle-record and manifest quarantines in `lifecycle.rs` and `generation.rs`.

This is a reachability change rather than a supersession because the guarded code
survives and is still defective. A future host path that quarantines charges
re-exposes it with no further change.

### Q: Is the ordering defect present at HEAD? (added 2026-09-05)

- Checked: `AdmissionController::quarantine` (`crates/shm-transport/src/profile.rs:518-531`) computes `active = accounting.active.checked_sub(charges)?` and `quarantined = accounting.quarantined.checked_add(retained)?` into locals and assigns both fields only after both succeed; the comment states that a failed checked operation leaves `accounting` unchanged. `host_admission_retains_quarantined_commitments` is at `crates/shm-transport/tests/profile.rs:50` and covers the success path only.
- Conclusion: no. The record stays active as a regression contract; the trail above describes the source tree's ordering.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 30, `:527-530` now `:520-523`: At HEAD the subtraction is computed into a local `active` and mapped to `AdmissionError::ChargeUnderflow`; `accounting.active` is assigned only after the `checked_add` succeeds (`:528`).
  - line 34, `:537-540` now `:524-527`: At HEAD the addition is computed into a local `quarantined` first, so a failure returns before either field is written.
  - line 36, `:527-530` now `:520-523`: At HEAD nothing is written through the `MutexGuard` until both checked operations succeed, so a failed add leaves `active` unreduced.
  - line 40, `profile.rs:531` now `profile.rs:530`: At HEAD `release_spans` runs only after both checked operations succeed, so its side effect cannot be committed before a failure.
  - line 55, `profile.rs:512-520` now `profile.rs:498-506`: At HEAD a failed `quarantine` leaves `accounting.active` unchanged, so the `Drop`-driven `release` subtracts charges that are still counted and is one correct refund rather than a double subtraction.
  - line 93, `profile.rs:527-530` now `profile.rs:520-523`: At HEAD `active` is not reduced until both checked operations succeed, so an add failure leaves the accounting untouched.
  - line 162, `:530` now `:523`: At HEAD the `active` shortfall maps to `ChargeUnderflow`, which is distinct from `AccountingUnavailable`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 78, `docs/shm-transport.md:90` (documented charge visibility claim): `docs/shm-transport.md` no longer describes a `Quarantined` readiness state or the phrase "stay visible"; the surviving statements are that active and quarantined charges are reported separately (`:21`) and that quarantined charges stay within the process bound (`:92`).
  - line 158, `profile.rs:530` (AccountingUnavailable on the checked_sub shortfall): At HEAD the `checked_sub` shortfall maps to `AdmissionError::ChargeUnderflow` (`profile.rs:523`); no `AccountingUnavailable` classification remains on that path.
  - line 187, `frame_channel.rs:417-433` (LeaseTracker lease quarantine): `LeaseTracker` and its lease quarantine no longer exist; the `quarantine` identifiers left in `crates/host-runtime` are in `generation.rs`, `lifecycle.rs`, `instance.rs`, and `ring_transport.rs`.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
