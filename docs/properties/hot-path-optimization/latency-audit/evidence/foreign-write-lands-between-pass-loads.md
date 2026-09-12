# foreign-write-lands-between-pass-loads

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

C1's freshness clause is meaningful only when something commits to the
session's `cache_state` row between the pass's own commit and a later read in
the same pass. With no concurrent writer, every load in a pass returns the same
`row_version`, so a design that loads once and a design that loads per consumer
produce identical results and no test can tell them apart. The Emergency95
rerun exists for exactly this interleaving. This record asks a campaign to
witness it, not to assert C1's outcome.

## Evidence trail

- After the transform commits, the handler reads `publication_floor_ordinal`
  at [`:8215-8223`][floor-a] on the Emergency95 arm, then runs the
  [`#[cfg(test)]` interleave hook][hook] at `:8224-8232`, whose field is
  declared at [`:2908-2911`][hook-field] with the comment that it runs where
  concurrent publication otherwise cannot interleave deterministically.
- [`prepare_historian_fire`][prepare] loads again at [`:5013`][prepare-load];
  on the ordinary arm it is called at [`:8336-8338`][prepare-b]. After an
  awaited inline firing the handler reruns `run_transform` and reloads the
  floor ([`:8264-8271`][rerun]).
- The final check at [`:8358-8377`][floor-b] loads once more and reruns the
  transform when `publication_floor_ordinal != emergency_pre_floor`. The
  comments there state that a publish is the only event that advances the
  floor and that abandon also bumps `row_version`, which is why the floor
  rather than the version is compared.
- The other actor in the existing test is the historian publish:
  [`publish_historian_chunk`][publish] commits through a second handle and
  drains afterwards ([`:10762-10771`][publish-drain]).
- [`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`][t-emergency]
  installs the hook at [`:34942-34945`][t-hook-install] to release a blocked
  producer and wait until the store shows `historian.state == Idle`, so the
  publish lands inside the window; a second test installs the hook at
  [`:35389-35392`][t-hook-second] to mutate snapshots.
- Every load in the window goes through [`MemoryStore::load`][load], which
  returns `row_version`, so the marker can record the value each read observed
  and the value the transform's commit returned.

## Failure scenario

Not a violation; a coverage gap. A campaign whose passes never see a foreign
commit between `commit_transform` and the post-commit reads exercises the
rerun lines on every Emergency95 pass while the comparison always finds equal
floors. C1 then passes for both the current design and a single-load design.

## Timing windows and dependencies

The window is between the transform's commit and
[`prepare_historian_fire`][prepare]
or the [floor check][floor-b]. Entering it needs a concurrent writer with its
own store handle: a historian publish, a wrapup recut, or a state sync. The
hook is the only deterministic seam at HEAD and is compiled out of release
builds.

## What a test must construct

A pass whose transform commits, then a foreign commit through a second handle
inside the window, then the pass's post-commit read. Reuse the
[hook][hook] as the campaign marker: record the `row_version` the transform
committed, the `row_version` each later load observed, and the actor that
committed between them; assert the two versions differ. The Emergency95 arm
needs usage at the emergency threshold so the floor reads run. The
[emergency interleave test](../existing-checks.md#cache-state-load-pass-trace-side-channel-and-meta-preparation)
constructs the interleaving but records no marker.

The witness is the foreign actor's own commit: record the `row_version` it
returns through the second handle and its completion before the pass's first
post-commit `cache_state` read begins, and compare it with the `row_version`
the pass's transform committed. The pass's own post-commit read is C1's
subject and is not the witness.

## Investigation log

### Q: Is the hook the only seam, and is it usable outside `cfg(test)`?

- Sources examined: the [hook field][hook-field], the [hook call][hook], the
  two installing tests ([`:34942`][t-hook-install], [`:35389`][t-hook-second]).
- Findings: The field and the call are `#[cfg(test)]`; no `test-support`
  feature exposes it, so integration tests in `crates/daemon/tests/` and the
  benches cannot reach it. A campaign outside the unit-test crate needs a
  successor seam or a real concurrent publisher with timing control.
- Missing evidence: A seam decision; W11 in the catalog names the same gap.
- Conclusion: resolved with answer - at HEAD the hook is the only seam and it
  is unit-test-only; a campaign outside that crate needs a new seam, which is
  a specification decision.

[hook-field]: ../../../../../crates/daemon/src/lib.rs#L2908-L2911
[prepare]: ../../../../../crates/daemon/src/lib.rs#L4994-L5067
[prepare-load]: ../../../../../crates/daemon/src/lib.rs#L5013
[floor-a]: ../../../../../crates/daemon/src/lib.rs#L8215-L8223
[hook]: ../../../../../crates/daemon/src/lib.rs#L8224-L8232
[rerun]: ../../../../../crates/daemon/src/lib.rs#L8264-L8271
[prepare-b]: ../../../../../crates/daemon/src/lib.rs#L8336-L8338
[floor-b]: ../../../../../crates/daemon/src/lib.rs#L8358-L8377
[t-emergency]: ../../../../../crates/daemon/src/lib.rs#L34932
[t-hook-install]: ../../../../../crates/daemon/src/lib.rs#L34951-L34954
[t-hook-second]: ../../../../../crates/daemon/src/lib.rs#L35398-L35401
[load]: ../../../../../crates/memory-store/src/lib.rs#L6196-L6223
[publish]: ../../../../../crates/memory-store/src/lib.rs#L10559
[publish-drain]: ../../../../../crates/memory-store/src/lib.rs#L10762-L10771
