# committed-transform-bookkeeping-is-applied-or-recomputed

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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
takes a snapshot generation ([`:8142-8146`][snapshot-begin]) and runs the
pre-transform work at [`:8181-8198`][h-pre]. The `run_transform` closure at
[`:8195-8261`][h-run] reads `guidance_date` through
[`guidance_date_for_transform`][guidance-fn] at [`:8261`][guidance-use] and
calls [`transform_with_projection_cached`][tc-inject] with
`&self.serialized_outputs`. The first call is at [`:8288-8291`][commit-call].
Inside that call, `apply_once` commits the store at
[`commit_transform:4950-4982`][store-commit] and then replaces the
serialized-output cache at [`:4987-4997`][output-replace]; both are inside
the closure, so the transform catalog's
[output-cache record][tc-output] governs their order.

In-memory mutations after `run_transform()` returns, in order:

- [`:8297-8302`][roots-insert] inserts `lineage_root` into
  `transform_session_roots`. Its [field doc][roots-doc] says the durable
  table is the authority and [`module_knows_transform_session`][knows]
  repopulates the map from it ([`:4531-4552`][knows-heal]) when the session
  has `cache_state`.
- [`:8303-8308`][floor-a] reads the publication floor (Emergency95 only), then
  the `#[cfg(test)]` [hook][hook] runs.
- [`prepare_historian_fire`][prepare] replaces the session's boundary-token
  snapshot at [`:5197-5200`][boundary-store] and persists no-fire reasons by
  CAS at [`record_no_fire:5511`][no-fire]; `spawn_historian_firing` at
  [`:8429`][spawn-fire] detaches the firing task.
- [`:8444-8451`][pc-store] calls [`store_projection_cache`][store-pc], which
  replaces the `projections` entry for `(session, revert_epoch)`.
- [`:8474-8479`][guidance-remove] removes the session's `guidance_dates` pin
  when `response.committed`. The pin is inserted by
  [`guidance_date_for_session`][guidance-pin] when the loaded `meta` has no
  date, and [`guidance_date_for_transform`][guidance-fn] returns it until it
  is removed. The transform copies `ctx.guidance_date` into `meta` only on a
  bust pass ([`transform.rs:4001-4002`][guidance-meta]).
- [`:8481-8510`][native-attach] updates `native_attachments`;
  [`:8512`][trace-complete] writes the completion trace;
  [`:8514-8517`][observation] records the response observation;
  [`:8527-8538`][finish-ready] finishes the snapshot generation with the
  retained request.

On the ordinary path there is no `.await` between [`:8288-8291`][commit-call] and
[`:8527-8538`][finish-ready]. The three awaits at `:8263`, `:8289`, and `:8315`
sit inside the Emergency95 branch ([`:8329-8433`][emergency]), and each is
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

- Sources examined: [`:8195-8261`][h-run] (a closure over borrowed `parsed`,
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

[handler]: ../../../../../crates/daemon/src/lib.rs#L7936
[roots-doc]: ../../../../../crates/daemon/src/lib.rs#L2949-L2952
[store-pc]: ../../../../../crates/daemon/src/lib.rs#L4332-L4375
[knows]: ../../../../../crates/daemon/src/lib.rs#L4519-L4566
[knows-heal]: ../../../../../crates/daemon/src/lib.rs#L4531-L4552
[guidance-fn]: ../../../../../crates/daemon/src/lib.rs#L4697-L4704
[prepare]: ../../../../../crates/daemon/src/lib.rs#L5043-L5373
[boundary-store]: ../../../../../crates/daemon/src/lib.rs#L5197-L5200
[no-fire]: ../../../../../crates/daemon/src/lib.rs#L5511
[guidance-pin]: ../../../../../crates/daemon/src/lib.rs#L7681-L7720
[snapshot-begin]: ../../../../../crates/daemon/src/lib.rs#L8142-L8146
[h-pre]: ../../../../../crates/daemon/src/lib.rs#L8181-L8198
[h-run]: ../../../../../crates/daemon/src/lib.rs#L8195-L8261
[guidance-use]: ../../../../../crates/daemon/src/lib.rs#L8261
[commit-call]: ../../../../../crates/daemon/src/lib.rs#L8288-L8291
[roots-insert]: ../../../../../crates/daemon/src/lib.rs#L8297-L8302
[floor-a]: ../../../../../crates/daemon/src/lib.rs#L8303-L8308
[hook]: ../../../../../crates/daemon/src/lib.rs#L8308-L8316
[emergency]: ../../../../../crates/daemon/src/lib.rs#L8329-L8433
[spawn-fire]: ../../../../../crates/daemon/src/lib.rs#L8429
[pc-store]: ../../../../../crates/daemon/src/lib.rs#L8444-L8451
[guidance-remove]: ../../../../../crates/daemon/src/lib.rs#L8474-L8479
[native-attach]: ../../../../../crates/daemon/src/lib.rs#L8481-L8510
[trace-complete]: ../../../../../crates/daemon/src/lib.rs#L8512
[observation]: ../../../../../crates/daemon/src/lib.rs#L8514-L8517
[finish-ready]: ../../../../../crates/daemon/src/lib.rs#L8527-L8538
[tc-inject]: ../../../../../crates/daemon/src/transform.rs#L1810-L1826
[guidance-meta]: ../../../../../crates/daemon/src/transform.rs#L4001-L4002
[store-commit]: ../../../../../crates/daemon/src/transform.rs#L4950-L4982
[output-replace]: ../../../../../crates/daemon/src/transform.rs#L4987-L4997
[host-cancel]: ../../../../../crates/host-runtime/src/dispatch.rs#L938-L955
[host-close]: ../../../../../crates/host-runtime/src/dispatch.rs#L1239-L1259
[tc-output]: ../../../daemon/transform/catalog.md#output-cache-replace-trails-the-accepted-commit
[e1]: ../../catalog.md#route-cleanup-waits-for-request-owned-physical-work
[e2]: ../../catalog.md#request-work-accounting-covers-retained-resources
