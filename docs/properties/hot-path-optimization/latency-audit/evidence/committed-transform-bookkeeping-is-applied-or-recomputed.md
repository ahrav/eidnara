# committed-transform-bookkeeping-is-applied-or-recomputed

## Rebase status, 2026-09-13

Relocation anchors refer to the formatted working tree atop `e451a2b4`.
`PassIntake` and `PassStart` retain upstream's `Result<ModuleMeta, MemoryStoreError>`;
the preflight `load_meta` duration remains `pass_state_load`. Rebased focused
checks pass. The `d6060f79` negative control and gate results remain historical,
and approved policy limits remain recorded. Final Bun passes, but the earlier
rebased full workspace has three failures; focused passes do not make it green.

## Historical baseline, 2026-09-10

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation below describe that baseline. Historical source
links are pinned; the implementation evidence does not replace those claims.

## Discovery trigger

The parent's [portfolio evaluation](../../portfolio-evaluation.md#gaps-queued)
queued this record as its gap 2 after refuting the lead "a cancelled request
must not commit": HEAD does not guarantee that, and a synchronous transform
cannot be separated from its bookkeeping by a host abort on the common path.
What a `spawn_blocking` relocation opens is narrower: a committed transform
whose derived in-memory state is never applied. This record names the four
structures and the recovery each one has.

## Evidence trail

The handler is [`handle_transform_dispatch`][handler]. After admission it
takes a snapshot generation ([`:8067-8071`][snapshot-begin]) and runs the
pre-transform work at [`:8115-8132`][h-pre]. The `run_transform` closure at
[`:8139-8193`][h-run] reads `guidance_date` through
[`guidance_date_for_transform`][guidance-fn] at [`:8175`][guidance-use] and
calls [`transform_with_projection_cached`][tc-inject] with
`&self.serialized_outputs`. The first call is at [`:8202`][commit-call].
Inside that call, `apply_once` commits the store at
[`commit_transform:4939-4971`][store-commit] and then replaces the
serialized-output cache at [`:4976-4986`][output-replace]; both are inside
the closure, so the transform catalog's
[output-cache record][tc-output] governs their order.

In-memory mutations after `run_transform()` returns, in order:

- [`:8209-8214`][roots-insert] inserts `lineage_root` into
  `transform_session_roots`. Its [field doc][roots-doc] says the durable
  table is the authority and [`module_knows_transform_session`][knows]
  repopulates the map from it ([`:4501-4522`][knows-heal]) when the session
  has `cache_state`.
- [`:8215-8223`][floor-a] reads the publication floor (Emergency95 only), then
  the `#[cfg(test)]` [hook][hook] runs.
- [`prepare_historian_fire`][prepare] replaces the session's boundary-token
  snapshot at [`:5148-5151`][boundary-store] and persists no-fire reasons by
  CAS at [`record_no_fire:5462`][no-fire]; `spawn_historian_firing` at
  [`:8353`][spawn-fire] detaches the firing task.
- [`:8387-8394`][pc-store] calls [`store_projection_cache`][store-pc], which
  replaces the `projections` entry for `(session, revert_epoch)`.
- [`:8398-8403`][guidance-remove] removes the session's `guidance_dates` pin
  when `response.committed`. The pin is inserted by
  [`guidance_date_for_session`][guidance-pin] when the loaded `meta` has no
  date, and [`guidance_date_for_transform`][guidance-fn] returns it until it
  is removed. The transform copies `ctx.guidance_date` into `meta` only on a
  bust pass ([`transform.rs:3990-3991`][guidance-meta]).
- [`:8407-8419`][native-attach] updates `native_attachments`;
  [`:8436`][trace-complete] writes the completion trace;
  [`:8439`][observation] records the response observation;
  [`:8452-8461`][finish-ready] finishes the snapshot generation with the
  retained request.

On the ordinary path there is no `.await` between [`:8202`][commit-call] and
[`:8461`][finish-ready]. The three awaits at `:8263`, `:8289`, and `:8315`
sit inside the Emergency95 branch ([`:8244-8334`][emergency]), and each is
followed by another `run_transform()` call, so an abort there leaves the
first commit's roots inserted and the later bookkeeping skipped.

## Failure scenario

`run_transform` moves to `spawn_blocking`; the handler awaits the join handle.
The blocking task cannot be cancelled once started, so the store commit
completes. The handler's abort lands on the await. The lineage root is
absent from the map until the next facade call repopulates it from the table.
The projection cache keeps the previous epoch's entry, so the next pass takes
the full projection path, which is correct but not cheaper. `guidance_dates`
keeps the pin: the next pass's `ProducerContext.guidance_date` is the pinned
line although the committed `meta` now carries a date, and nothing removes it
until a later committed pass on that session. The serialized-output cache is
consistent only if the relocation moved the whole closure; a split that
commits on the worker and replaces on the handler leaves an entry from a pass
the next pass will not observe as committed.

## Timing windows and dependencies

At HEAD the window exists only in the Emergency95 branch. Under a blocking
worker it exists on every pass, between the store commit and the first line
of handler bookkeeping. The `#[cfg(test)]` [hook][hook] runs after the roots
insert, so it cannot separate the commit from that first update; it can
separate the commit from everything after it.

## What a test must construct

A committing pass on a session with a pinned guidance date (a fresh session
whose first `guidance_date_for_session` call set the pin); an abort injected
between commit and bookkeeping (W11); then a second pass on the same session
that records `ProducerContext.guidance_date`, whether the projection cache
was hit, and whether `module_knows_transform_session` repopulated the root,
and compares each against a fresh computation. No existing check covers this;
the parent's [E1][e1] and [E2][e2] cover route state and charges.

## Investigation log

### Q: What owns and joins any proposed off-worker transform work?

- Sources examined: [`:8139-8193`][h-run] (a closure over borrowed `parsed`,
  `binding`, `store`, `project_memory`, and `projection_cache_input`), the
  host cancel arm at [`dispatch.rs:938-955`][host-cancel], the route-close
  path at [`:1239-1259`][host-close].
- Findings: The closure is invoked up to four times per request over borrowed
  state; a `'static` worker needs clones or shared ownership. The host aborts
  the dispatch task on cancel and, on route close, waits two budgets and
  trips fatal for a task that never yields. Nothing at HEAD owns a detached
  worker or joins it.
- Missing evidence: A worker design.
- Conclusion: needs human input.

[handler]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7887
[roots-doc]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L2936-L2939
[store-pc]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4302-L4345
[knows]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4489-L4536
[knows-heal]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4501-L4522
[guidance-fn]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4648-L4655
[prepare]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L4994-L5324
[boundary-store]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5148-L5151
[no-fire]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L5462
[guidance-pin]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L7632-L7671
[snapshot-begin]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8067-L8071
[h-pre]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8115-L8132
[h-run]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8139-L8193
[guidance-use]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8175
[commit-call]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8202
[roots-insert]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8209-L8214
[floor-a]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8215-L8223
[hook]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8224-L8232
[emergency]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8244-L8334
[spawn-fire]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8353
[pc-store]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8387-L8394
[guidance-remove]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8398-L8403
[native-attach]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8407-L8419
[trace-complete]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8436
[observation]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8439
[finish-ready]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L8452-L8461
[tc-inject]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L1799-L1815
[guidance-meta]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L3990-L3991
[store-commit]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L4939-L4971
[output-replace]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/transform.rs#L4976-L4986
[host-cancel]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L938-L955
[host-close]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L1239-L1259
[tc-output]: ../../../daemon/transform/catalog.md#output-cache-replace-trails-the-accepted-commit
[e1]: ../../catalog.md#route-cleanup-waits-for-request-owned-physical-work
[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources

## Implementation evidence, 2026-09-13

[#438](https://github.com/ahrav/eidnara/issues/438) uses owned `Send + 'static`
pass state and the host's request-work join. [Unit one][unit-one] includes the
ordinary pass's commit and settlement on one blocking thread. Waiter abort
does not interrupt that synchronous unit. [First-transform bookkeeping][first]
inserts the lineage root and removes the guidance pin after success; each
[committing rerun][rerun-live] also removes the pin. Serialized-output replacement
stays inside the unchanged transform call after the accepted store commit.

[Emergency95 orchestration][emergency-live] retains the environment across
async historian waits without a unit permit. Initial and rerun results share
`PassContinuation`, whose last field, `env`, keeps request charges alive until
the preceding pass and action values drop. The first unit matches the prepared
action directly; no separate no-wait helper or optional action is needed.
Reruns use `PassState::Reload`; an inline publication's rerun, floor reload,
and settlement run in one final unit. The final floor
check, projection storage, native attachment, trace, observation, and snapshot
publication live in [settlement][settle-live]. If cancellation is observed at
a later unit's head, that unit may skip its work. The owner explicitly accepts
this on 2026-09-13, including a `cancelled` outcome after an earlier durable
commit. Derived projections and attachments are recomputable; observations
are advisory; a later snapshot `begin` supersedes an unfinished generation.
This is not a promise that cancellation rolls back committed state.

The [first permit acquisition][admission-live] precedes snapshot `begin` on both
unit-backed lanes and ticket acceptance on the unpaged typed path. [Paged
staging][page-accept] accepts before its Apply arm reaches typed admission;
that behavior remains unchanged to preserve paged admission semantics. Read
preflight still precedes the permit. An aborted semaphore wait therefore cannot
invalidate a Ready snapshot. This is a source-ordering guarantee, not a
Ready-snapshot assertion in the fifth-waiter test. That test checks the initially
missing snapshot remains `Missing` after the queued waiter is dropped.

[`aborted_waiter_preserves_commit_bookkeeping_and_worker_charges`][abort-test]
holds the real transform after commit and before lineage insertion, with a
known stale guidance pin. It observes the durable row and absent lineage,
aborts and joins the waiter while the physical join remains live, then releases
the worker. Lineage is present and the pin absent after completion. The test
reopens the store, verifies committed core state, and runs a new HARD pass;
its guidance date equals a fresh computation from that pass's receive time,
not the old pin. This does not independently assert every projection, native,
serialized-output, or snapshot recovery branch, and reopen is not power loss.

The controller reports an executed negative control for this path on 2026-09-13:
replacing `forget_guidance_pin_on_commit`'s `.remove` with `.get` makes the
aborted-waiter test fail at the [pin-absence assertion][negative-control]. The
removal is restored before the subsequent workspace runs. This discriminates
the guidance-pin obligation; it does not prove every derived-cache recovery
branch. The workspace runs exercise the restored code but remain failed on
the two known baseline deadline tests.

[`emergency_cancellation_between_units_preserves_commit_and_releases_scratch`][emergency-test]
observes an Emergency95 commit and a completed first unit before cancellation
during the live historian wait. After release, a second unit returns `cancelled`
without losing initialized durable state or leaking scratch. The real-host
cancel and close cases additionally retain the committed core after cleanup.
The [focused receipt][receipt] records ten passing group tests, the parent-run
child role, and the failed workspace results. The earlier five nine-pass runs
predate the synthetic failure test. Bun passes with five capability-gated skips;
isolated deadline reruns pass without changing the full workspace's exit 101.
W8 remains partially exercised.

The transform ownership question is resolved: the host joins units through
`RequestCtx::run_blocking`. The unrelated kernel-route gap remains in E1.
Fatal close after an over-budget unit is approved, but full-cap paged and
slow-disk durations remain unmeasured.

[unit-one]: ../../../../../crates/daemon/src/lib.rs#L8427-L8492
[first]: ../../../../../crates/daemon/src/lib.rs#L8846-L8903
[rerun-live]: ../../../../../crates/daemon/src/lib.rs#L8806-L8837
[emergency-live]: ../../../../../crates/daemon/src/lib.rs#L8494-L8609
[settle-live]: ../../../../../crates/daemon/src/lib.rs#L8925-L9068
[abort-test]: ../../../../../crates/daemon/src/transform_unit/tests.rs#L143-L246
[emergency-test]: ../../../../../crates/daemon/src/transform_unit/tests.rs#L504-L570
[admission-live]: ../../../../../crates/daemon/src/lib.rs#L8357-L8418
[page-accept]: ../../../../../crates/daemon/src/lib.rs#L9941-L10006
[negative-control]: ../../../../../crates/daemon/src/transform_unit/tests.rs#L205-L212
[receipt]: ../../existing-checks.md#transform-unit-execution-receipt-2026-09-13
