# clean-reclamation-is-reachable

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

`docs/shm-transport.md:87` (source tree; not at HEAD) states "These are distinct outcomes and
distinct test experiments:" and then describes clean reclamation (line 89) and
quarantine (line 90). A pair of outcomes presented as distinct experiments
implies both are reachable, so the shipped backend's cleanup was traced to see
which branches it can select.

## Evidence trail

former `crates/host-runtime/src/shm_provider.rs:137-152` is the only production
`RecoveryBackend` implementation:

```rust
fn cleanup(&self, _candidate_id: u64) -> CleanupOutcome {
    self.cleanups.fetch_add(1, Ordering::AcqRel);
    CleanupOutcome::Uncertain
}
```

Lines 138-141. The candidate id is discarded, no state is examined, and
`Uncertain` is returned for every input. `probe` (lines 143-147) returns `true`
unconditionally. `admission_fits` (lines 148-150) is the only method whose result
depends on anything.

The unreachability is deliberate and stated in code. The struct's doc comment at
lines 128-130 reads: "Recovery primitives for the thread-confined ring endpoint.
The rings die with their endpoint thread, so a suspect close leaves alias state
uncertain: cleanup isolates instead of reclaiming." The `probe` body carries the
matching rationale: "No shared state outlives the endpoint thread, so isolation
alone proves the provider side is clean."

The consumer of that outcome is former `crates/host-runtime/src/provider_recovery.rs`, at the
`match outcome` beginning line 482:

- `CleanupOutcome::Reclaimed` (arm at lines 483-490) calls `record.release()` and,
  on success, `state.incarnation += 1` at line 488 — the charge return and the
  incarnation mint that `docs:89` (source tree; not at HEAD) describes.
- `CleanupOutcome::StaleRetry | CleanupOutcome::Uncertain` (arm at lines 493-495)
  calls `record.quarantine()`.

The catalog cites this region as `481-490`; the `Reclaimed` arm itself spans
483-490, with `match outcome {` at 482 and `state.inflight = None;` at 481.

Every producer of `CleanupOutcome::Reclaimed` in the tree is a test double.
There are three `impl RecoveryBackend` blocks in all:

| Impl | Location | Nature |
| --- | --- | --- |
| `ShmRecoveryBackend` | former `shm_provider.rs:137` | production; returns `Uncertain` only |
| `FakeBackend` | former `provider_recovery.rs:684`, inside `#[cfg(test)]` at line 578 | unit-test double, scripted |
| `MatrixBackend` | `crates/host-runtime/tests/shm_transport.rs:450` (source tree; not at HEAD) | integration-test double |

`CleanupOutcome::Reclaimed` appears as a value at lines 876, 894, 926, 1015,
1054, and 1103 of former `provider_recovery.rs`, all inside the `#[cfg(test)]` module.
`clean_reclamation_returns_charges_once_and_mints_a_new_incarnation` (line 889)
reaches the branch by pushing `Scripted::Return(CleanupOutcome::Reclaimed)` at
line 894.

A second documented branch is unreachable for the same reason.
`docs:90` (source tree; not at HEAD) names two triggers for provider-wide `Quarantined` readiness: "failed
probe" and "admission-cap exhaustion". `resolve_readiness`
(former `provider_recovery.rs:524-534`) computes `ready = probe() && admission_fits()`
at line 530 under a panic boundary. Since `ShmRecoveryBackend::probe()` returns
`true` and does not panic, only admission-cap exhaustion can set that readiness
on the shipped backend.

## Failure scenario

The scenario below was derived against the source tree this record was written from; where the investigation log's post-merge entry records a changed mechanism, the sentences marked "At HEAD" above and that entry carry the current behavior, and the scenario reads as the regression this record guards against.

There is no misbehaviour to trigger; the gap is that one documented outcome
never occurs. Any suspect close on the shipped provider takes the
`Uncertain` path: `record.quarantine()` at line 494, charges stay visible, and no
new incarnation is minted. Over a long-running process every suspect
accumulates quarantined charges, and `admission_fits` is the only thing that can
subsequently change readiness. The documented behaviour "the record's active
charges return exactly once, a new provider incarnation is minted" is proven
only against `FakeBackend`, so a regression in the `Reclaimed` arm would be
caught by the unit test and would have no production consequence either way.

## Timing windows and dependencies

None. `cleanup` is a constant function of its argument, so no interleaving, race,
or fault changes the outcome. The property is a static reachability question
about production code.

The dependency worth naming is that this property bounds what
`dead-peer-charges-are-reclaimed-or-declared` can achieve: as long as the shipped
cleanup returns `Uncertain` unconditionally, no reclamation path exists for a
dead peer's charges regardless of how the surrounding controller behaves.

## What a test must construct

A reachability assertion, and it is expected to fail or to be recorded as
scoped:

1. Assert that some production path reaches
   former `provider_recovery.rs:483-490`. With `ShmRecoveryBackend` as the only
   production backend, no construction achieves this.
2. Failing that, the property is discharged by scoping the documentation:
   assert that `docs:89` (source tree; not at HEAD) names the backends for which clean reclamation is
   reachable, and assert that the ring backend is excluded. That converts an
   unreachable branch from a silent gap into a stated limitation.
3. Independently, a case asserting that `resolve_readiness` cannot reach
   provider-wide `Quarantined` via a failed probe on the shipped backend, so the
   two triggers at `docs:90` (source tree; not at HEAD) are not presented as equally live.

Both 2 and 3 are assertions about documentation scope rather than about
behaviour, which is the correct shape when the code's stated intent is that the
branch should not be reachable.

## Investigation log

The catalog records no open question for this property. The question resolved
during the trail is logged because it determines whether this is a defect or a
scoping gap.

### Q: Is the unconditional `Uncertain` return a gap in the shipped backend, or the intended behaviour of a thread-confined ring endpoint?

- Sources examined: former `crates/host-runtime/src/shm_provider.rs:121-152`, including the
  struct doc comment and the `probe` rationale comment;
  former `crates/host-runtime/src/provider_recovery.rs:96-103` (`CleanupOutcome`),
  former `:478-496` (the outcome match), former `:508-545` (`after_record_resolved` and
  `resolve_readiness`); repository-wide search for `impl RecoveryBackend` and
  `CleanupOutcome::Reclaimed`; former `provider_recovery.rs:889-901`;
  `docs/shm-transport.md:85-90` (source tree; not at HEAD).
- Findings: the intent is recorded in code, not merely inferable. The struct doc
  comment states that cleanup isolates instead of reclaiming because the rings
  die with their endpoint thread, and the `probe` comment states that isolation
  alone proves the provider side clean. So `Uncertain` is a deliberate
  consequence of the ring backend's confinement model, not an unfinished
  implementation.
- Missing evidence: none for the reachability question. What the tree does not
  record is whether `docs:89` (source tree; not at HEAD) is meant to describe the ring backend at all, or a
  future backend whose resources outlive their thread. The section header
  "Clean reclamation versus quarantine exhaustion" presents both as live for the
  provider it documents.
- Conclusion: resolved with answer. Clean reclamation is unreachable on
  production code and is unreachable by design, per the rationale recorded at
  former `shm_provider.rs:128-130`. The residual defect is one of documentation scope:
  `docs:87` (source tree; not at HEAD) calls the two outcomes "distinct test experiments" without noting
  that only one has a production experiment. A second instance of the same shape
  was found independently — of the two `Quarantined` triggers at `docs:90` (source tree; not at HEAD), only
  admission-cap exhaustion is reachable, because `probe()` is constant.

## Refresh outcome, 2026-08-30

Status moved to `superseded-by-refactor`. The record asked whether the shipped
backend could ever reach clean reclamation. The refactor answered it by deleting
both outcomes. `0f336d3c` removed `ShmRecoveryBackend` and `ed487e11` removed
`crates/host-runtime/src/provider_recovery.rs`, so `RecoveryBackend`, `CleanupOutcome`,
`ProviderReadiness`, recovery episodes, provider incarnations, and the fake-backend
test that was this record's only evidence are all gone. None has a successor at
`e447c927`.

What now owns the obligation: nothing. `crates/host-runtime/src/ring_transport.rs:360`
calls `admission.release()` unconditionally once the endpoint thread's
`catch_unwind` returns, with no cleanup probe, no reclaim-versus-isolate decision,
and no proof that stale resources are gone. That is neither of the two documented
outcomes. It is strictly weaker than the quarantine path the record found to be
the only reachable one, because charges are now returned as clean capacity even
after an unclean close.
At HEAD: The release is no longer unconditional: a ring that latched quarantine and whose peer has not released it moves its charges to the quarantined bucket through `admission.quarantine()` (`:353-358`), and only an otherwise clean close reaches `admission.release()`.

`docs/shm-transport.md:87-90` (source tree; not at HEAD) still presents clean reclamation and
quarantine as two distinct outcomes with distinct test experiments. As of this
commit it describes no code at all, which is a larger documentation defect than
the scoping problem this record originally recorded.

Status note, 2026-08-31: the catalog status for this record is now
`invalidated` (vocabulary normalization); the `superseded-by-refactor` wording
above is retained as history.

### Q: What did the post-merge re-anchor find at HEAD?

- Sources examined: every file this trail cites, at the merged HEAD.
- Findings:
  Mechanisms whose citation moved and whose surrounding claim needed restating:
  - line 172, `crates/host-runtime/src/ring_transport.rs:291` now `crates/host-runtime/src/ring_transport.rs:360`: The release is no longer unconditional: a ring that latched quarantine and whose peer has not released it moves its charges to the quarantined bucket through `admission.quarantine()` (`:353-358`), and only an otherwise clean close reaches `admission.release()`.
  Constructs with no counterpart at HEAD; their citations above are marked "source tree; not at HEAD":
  - line 20, `docs/shm-transport.md:87` (the statement that the two outcomes are distinct test experiments): `docs/shm-transport.md` was rewritten and has no clean-reclamation-versus-quarantine section at HEAD.
  - line 55, `docs:89` (the documented clean-reclamation outcome): No such passage survives in `docs/shm-transport.md`.
  - line 69, `crates/host-runtime/tests/shm_transport.rs:450` (MatrixBackend integration-test double): `crates/host-runtime/tests/shm_transport.rs` does not exist at HEAD; the surviving shared-memory integration tests are shm_failure_modes.rs and shm_soak.rs, neither of which defines a RecoveryBackend.
  - line 78, `docs:90` (the two documented triggers for provider-wide Quarantined readiness): The passage is gone from the document.
  - line 117, `docs:89` (the documented clean-reclamation outcome): The passage is gone from the document.
  - line 122, `docs:90` (the two documented Quarantined triggers): The passage is gone from the document.
  - line 142, `docs/shm-transport.md:85-90` (the clean-reclamation-versus-quarantine section): The section is gone from the document.
  - line 150, `docs:89` (the documented clean-reclamation outcome): The passage is gone from `docs/shm-transport.md`, so the question of whether it describes the ring backend has no anchor at HEAD.
  - line 157, `docs:87` (the distinct-test-experiments wording): The wording is gone from the document.
  - line 159, `docs:90` (the two documented Quarantined triggers): The passage is gone from `docs/shm-transport.md`.
  - line 180, `docs/shm-transport.md:87-90` (the two outcomes presented as distinct test experiments): The document no longer presents them at all, so the documentation defect this paragraph records has changed shape.
- Missing evidence: none beyond what the record's Exercised field states.
- Conclusion: the claims above are read against the source tree where marked and against HEAD elsewhere; the catalog record carries the HEAD disposition.
