# foreign-write-lands-between-pass-loads

## Rebase status, 2026-09-13

Relocation anchors refer to the formatted working tree atop `e451a2b4`.
The rebased Emergency95 group passes all four tests, preserving six scalar reads
for Busy and four for the post-publish hook witness. Historical hook orders and
`d6060f79` results remain separate from that rebased receipt.

## Historical baseline, 2026-09-10

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation sections describe that baseline. Their source
links are pinned to it. The marker evidence below describes the live test.

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
returns through the second handle and its completion between two of the
pass's post-commit `cache_state` reads, and compare it with the `row_version`
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

[hook-field]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L2908-L2911
[prepare]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4994-L5067
[prepare-load]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5013
[floor-a]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8215-L8223
[hook]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8224-L8232
[rerun]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8264-L8271
[prepare-b]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8336-L8338
[floor-b]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8358-L8377
[t-emergency]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L34923
[t-hook-install]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L34942-L34945
[t-hook-second]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L35389-L35392
[load]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L6196-L6223
[publish]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10559
[publish-drain]: https://github.com/ahrav/eidnara/blob/9132344/crates/memory-store/src/lib.rs#L10762-L10771

## Marker evidence

Implementation base: `96709d0ef54bcfad2327878ab96e118fb8ba4969` plus the
storage units that precede it on the branch.
Preservation authority: [implementation ticket](https://github.com/ahrav/eidnara/issues/432)
and [parent specification](https://github.com/ahrav/eidnara/issues/350).

This section includes the upstream `e451a2b4` clarification of hook order.
Commit `d6060f79` temporarily used an earlier hook position. The final source
restores this upstream order and gives W11 a separate post-commit gate.

The [emergency interleave test][marker-test] now carries the marker. Inside the
`between_transform_and_prepare` hook it reads the `row_version` the transform
committed, releases the blocked producer, waits for the publish to leave the
historian idle, and records the `row_version` that publish committed; after
the pass it asserts the published version exceeds the transform's. Both values
come from the store through the test's own handle, not from the pass's
post-commit read, which is C1's subject. The hook runs after the Emergency95
[pre-floor read][pre-floor-live], so the publish lands between that read and
`prepare_historian_fire`'s load; the [final floor read][floor-live] is a
scalar read after this change and still observes the publish: the response
carries the fold.

The hook stays `#[cfg(test)]`; a campaign outside the unit-test crate still
needs its own seam, as the investigation log records.

[pre-floor-live]: https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/lib.rs#L8565-L8578
[marker-test]: https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/lib.rs#L37664-L37764
[floor-live]: https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/lib.rs#L8694-L8710

## Implementation evidence, 2026-09-13

[#438](https://github.com/ahrav/eidnara/issues/438) preserves the foreign-write
witness while placing pass work in host-joined blocking units. In the final
[first_transform][first-live], `between_transform_and_prepare` follows the
first publication-floor read, matching upstream. Abort and panic tests instead
use `after_transform_commit` before lineage and guidance bookkeeping. The
temporary earlier C5 hook position in `d6060f79` is historical, not the final seam.

`handler_emergency_refolds_when_active_run_publishes_before_live_wait_capture`
still records both row versions through the test's store reference, releases
the blocked producer, waits for the durable historian state to become idle,
and asserts `published > transform_committed`. These are observations outside
the pass's own read. The test uses an `Arc` clone of the store, not a separately
opened SQLite connection, and does not capture the publisher's return value
directly. Its response must contain `autonomous summary`, and its statement
probe requires exactly four scalar reads during the exercised pass.

The [rerun code][rerun-live] names `PassState::Reload`, never the original
pre-transform load. [Settlement][settle-live] performs a distinct final floor
check and reruns if the floor changed. After an inline publication, the rerun,
floor reload, and settlement share one final unit rather than adding a
redundant check across another await.
The four [Emergency95 tests][emergency-tests] cover inline success, busy-run
completion, the foreign publish, and inline failure. They do not establish
every possible publication schedule or a global freshness theorem. The
[execution receipt][receipt] reports four passes, separately from full gates.

[first-live]: ../../../../../crates/daemon/src/lib.rs#L8886
[rerun-live]: ../../../../../crates/daemon/src/lib.rs#L8845-L8883
[settle-live]: ../../../../../crates/daemon/src/lib.rs#L8973-L8991
[emergency-tests]: ../../../../../crates/daemon/src/lib.rs#L37952-L38185
[receipt]: ../../existing-checks.md#transform-unit-execution-receipt-2026-09-13

## Rebase check, 2026-09-13

Both source versions require four scalar reads in this witness. Upstream's
[assertion](https://github.com/ahrav/eidnara/blob/e451a2b4/crates/daemon/src/lib.rs#L37731-L37750)
explicitly distinguishes those four reads from the six-read Busy path excluded
by the hook. The rebased [Busy test][busy-live] separately requires six reads:
live-completion refold and preparation add a historian-phase read and floor
read before the inline follow-up. The [post-publish witness][post-publish-live]
still requires four. The rebased Emergency95 group reports four passes with
both oracles unchanged; no count was relaxed to accept the other path.

The final C5 hook follows the first floor read, so the passing witness again
constructs publication between two post-commit reads. C5 returns to exercised
with default-production reachability, while the separate W11 hook retains its
test-only reachability. Neither contract or scalar-read oracle is weakened.

[busy-live]: ../../../../../crates/daemon/src/lib.rs#L37967-L38023
[post-publish-live]: ../../../../../crates/daemon/src/lib.rs#L38025-L38127
