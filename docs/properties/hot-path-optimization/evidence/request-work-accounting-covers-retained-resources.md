# request-work-accounting-covers-retained-resources

## Rebase status, 2026-09-13

Relocation anchors refer to the formatted working tree atop `e451a2b4`.
The merged admission and transfer checks below preserve upstream accounting
guards. Rebased meter and blocking groups pass; the historical `d6060f79`
receipt is separate, and no green full-workspace gate is claimed.

## Historical baseline, 2026-09-10

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.
The discovery and investigation below describe that baseline, not the working
tree. Their source links are pinned; the dated implementation section follows.

## Discovery trigger

Execution relocation can release a waiter's permit while its work or retained
result still occupies a different bounded resource.

## Evidence trail

[Parse admission][parse] precedes decode with a [three-copy bound][footprint].
[Decode][decode] and [finish][finish] move guards with work and retained results.

The current ledger separates classes by actual units and ownership boundaries.
Host class capacities are the configured pools after startup reservations, not
a new per-transform worker limit. Fixed ingest ceilings are [defined here][caps].

| Class and units | Admission | Current owner | Transfer | Release | Capacity/source |
| --- | --- | --- | --- | --- | --- |
| Pending requests use a count. | Dispatch acquires before spawning. | The outer settlement task holds the permit. | It moves into outer dispatch. | Dropping that task returns it after settlement or early exit. | The route's general/reserved pending pool is [selected here][admission]. |
| Handler tasks use a count. | Dispatch acquires beside the pending permit. | The callback task holds it during execution. | Dispatch moves it into the callback. | Callback completion/drop returns it. | The route's general/reserved task pool is [selected here][owners]. |
| Ingress uses resident frame/body bytes. | Frame admission charges before handler ownership. | InputBuffer holds bytes and charge. | Moving InputBuffer moves the charge with retained bytes. | Dropping the owner returns its ByteCharge. | The ingress pool and [InputBuffer contract][input] apply. |
| Parse residency uses bytes. | The handler reserves footprint before JSON decode. | `_parse_charge` remains in the async handle call. | No transform-worker transfer exists here. | Handler completion/drop releases the charge. | The scratch pool exposed by [resident_capacity][resident] applies. |
| Staging uses declared bytes and upload count. | Begin calls StagingBudget admission. | The coordinator or finish reservation holds ownership. | Retry restoration keeps the reservation with the upload. | Final release, discard, or guard drop returns both units. | [StagingBudget][staging] defaults to MAX_STAGED_BYTES and MAX_PENDING_UPLOADS. |
| Decode uses encoded/decoded bytes and job count. | reserve_decode checks both caps before work. | DecodeReservation follows work and its retained result. | It moves through decode to staging/refusal. | Guard drop returns bytes and one job. | [Decode admission][decode-admission] uses PAGE_DECODE_BYTES_MAX and PAGE_DECODE_JOBS_MAX. |

## Failure scenario

Cancellation releases the only charge while a worker retains bytes. Another
request is then admitted against understated usage. A second failure mode
releases a transferred reservation twice or never releases an abandoned result.

## Timing windows and dependencies

Pending capacity describes settlement, not every retained resource. Handler
task capacity describes callback work; a future physical worker needs explicit
accounting ownership. Decode, staging, and ingress charges are not interchangeable.
Health and pending counters need not all remain elevated until physical finish.

## What a test must construct

Track resource owners independently by class across admission, work start,
waiter cancellation, retained result, retry transfer, and final release.
Assert
`observed_live_units[c] <= reserved_units[c] <= configured_capacity[c]`.
Obtain live units from independently observed execution/ownership and allocation
lifetimes, not by reading back the candidate's reservation counter. For byte
classes, count resident allocations covered by that class, not just wire length.
Include refusal and ownership transfer, not equality between unrelated counters.
[Existing checks][checks] remain unaudited; no resource-lifetime campaign runs.

## Investigation log

### Q: Which capacity class and byte definition cover proposed worker work?

- Sources examined: [Host permit owners][owners] and [ingest reservations][guards].
- Findings: The existing implementation already distinguishes settlement from
  retained resource ownership. The ingest examples do not specify transform jobs.
- Missing evidence: Future queued jobs, running jobs, and retained results need
  a complete resource-to-owner mapping, numerical capacities selected by their
  owner, and transfer/completion observations. None is supplied for transform work.
- Conclusion: Worker-specific test readiness is BLOCKED pending this owner gate.
  Existing pending/health counters need not remain held to physical completion;
  actual work and retained bytes need their own coverage, not an invented limit.

[admission]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L823-L855
[owners]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/dispatch.rs#L876-L934
[guards]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/kernel_routes/ingest.rs#L513-L550
[decode]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/kernel_routes/ingest.rs#L692-L763
[finish]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/kernel_routes/ingest.rs#L787-L815
[checks]: ../existing-checks.md#execution-lifecycle
[parse]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L11805-L11826
[footprint]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/lib.rs#L15408-L15454
[input]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L277-L295
[resident]: https://github.com/ahrav/eidnara/blob/9132344/crates/host-runtime/src/handler.rs#L474-L490
[caps]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/kernel_routes/ingest.rs#L43-L82
[staging]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/kernel_routes/ingest.rs#L87-L130
[decode-admission]: https://github.com/ahrav/eidnara/blob/9132344/crates/daemon/src/kernel_routes/ingest.rs#L406-L425

## Implementation evidence, 2026-09-13

[#438](https://github.com/ahrav/eidnara/issues/438) moves transform units through
the host-owned runner. Relocation anchors describe the rebased working tree,
not the discovery baseline. [The execution receipt][receipt] separates historical
focused results from failed workspace gates and the passing Bun gate. All checks
remain unaudited.

| Resource | Owner and boundary in the implementation |
| --- | --- |
| Ingress bytes | `RequestCtx.body` remains on the async side. Units own decoded values, not the input buffer. The read-only test observer measures the held body charge and its pool's availability without retaining the charge. Raw input can release when the handler aborts while decoded scratch remains with the worker. |
| Decode scratch | The meter retains charges during semaphore admission. After admission, `ResidentMeter::take_charges` transfers them once into `PassIntake.held`, then `PassEnv._held`. The environment drops request fields before the hold. Transfer permanently disables further charging by that meter, including after restart or release. |
| Transform unit count | Four semaphore permits bound submitted units across the daemon. Abortable acquisition precedes snapshot `begin` and submission on both unit-backed lanes. Unpaged typed ticket acceptance follows the permit; paged staging already accepts before the Apply arm reaches it. Read preflight remains before the permit. The closure owns its permit. No unit permit crosses an Emergency95 historian await. |
| Emergency continuation and pending rerun result | Both use `PassContinuation`. Its last field, `env`, retains the environment and its scratch charges until the preceding pass and action fields drop. An aborted waiter cannot leave a queued continuation retaining pass data without its environment. |
| Paged apply | A shared `PageApplyGuard` keeps `Applying`, staged bytes, and the pending upload count until completion or the final owner's drop. `finish_apply` and guard drop share `release_applying`, which checks the transform identity before releasing. |
| Shared-memory accounting | The host's existing accounting is unchanged. Before/after status equality is a regression observation, not a per-allocation ingress or scratch ledger. |

The ownership definitions are [PassIntake and PassEnv][env], [charge transfer][take],
[submission and admission][admit], [carry and guard types][holds], and
[identity-checked release][release]. This mapping resolves the transform-worker
capacity question; it does not replace the historical ingest budgets or prove
that the assembled paged request has a new whole-request footprint reservation.
The [paged staging acceptance][page-accept] remains before typed admission to
preserve the page lane's acceptance semantics; only snapshot `begin` is
uniformly after the permit.

Same-ID guard safety also depends on [stage refusing every Applying phase][stage]
and [collector eviction excluding Applying][evict]. A sender cannot replace a
live apply with the same ID while its guard remains active. These existing
state-machine rules, not a new epoch, protect the identity-checked release.

`aborted_waiter_preserves_commit_bookkeeping_and_worker_charges` and
`aborted_page_waiter_keeps_applying_and_staged_bytes_until_worker_finishes`
hold a post-commit worker, abort its waiter, and check retained scratch and a
held permit. The page case also checks exact staged bytes, one pending upload,
refusal of overlapping input, and successful paged and unpaged retries.
`stale_apply_release_preserves_newer_attempt_and_matching_release_runs_once`
keeps a second session charged so a duplicate decrement cannot hide at zero.
`four_parked_units_keep_fifth_waiter_off_the_blocking_pool` explicitly polls the
fifth request, proves no fifth submission, drops it, then admits a replacement
only after one permit returns. Source ordering places snapshot `begin` after
this wait, so aborting a queued request cannot invalidate an existing Ready
snapshot. After dropping the queued fifth request, the test requires snapshot
lookup to remain `Missing`, not `InFlight`. It does not seed a Ready snapshot
as a separate oracle.
`cancelled_unit_releases_its_charges_without_store_work`
checks no cache row or receive trace. These [unit tests][tests] reacquire the
entire scratch pool and refuse one additional byte, proving exact return for
the tested pool rather than merely an empty counter.

`emergency_cancellation_between_units_preserves_commit_and_releases_scratch`
observes all four permits available while the live historian is awaited, but
scratch still held. The next unit sees cancellation and releases it. The two
[meter transfer tests][meter-tests] separately test one-time release and spent
meter refusal. [Real-host tests][host-tests] repeat scratch reacquisition after
request cancel, route close, and panic. They also measure exact ingress return:
at the post-commit gate, available bytes equal the recorded baseline minus the
request's held body charge; after server Error publication or route-gone,
availability equals that baseline. The panic child checks the baseline too.

[`InputBuffer::ingress_charge_for_test`][ingress-observer] exposes the held-byte
count and a closure from [`ByteCharge::observe_pool`][pool-observer]. The closure
captures the semaphore, not its charge, and exposes no mutation. Thus the
observer cannot keep the request charge alive or return extra bytes. These
`test-support` observations close the former private-ingress visibility gap.
They are not an allocator-RSS measurement or coverage of every pending-output
cancellation schedule, so E2 remains partial.

[`synthetic_unit_failures_map_to_internal_error_and_release_resources`][failed-unit-test]
checks all three `BlockingWorkFailed` variants at submissions one and two.
The fake runner drops the failed submission's closure without executing it.
Submission-two cases first run a real Emergency95 transform and inline historian
publication, then inject the error delivery. Every case returns `internal_error`,
all four unit permits, and exact scratch capacity. These are synthetic delivery
tests, not actual runtime-stop or route-closing races.

[receipt]: ../existing-checks.md#transform-unit-execution-receipt-2026-09-13
[env]: ../../../../crates/daemon/src/lib.rs#L3545-L3584
[take]: ../../../../crates/daemon/src/metered_decode.rs#L211-L245
[admit]: ../../../../crates/daemon/src/lib.rs#L8381-L8492
[page-accept]: ../../../../crates/daemon/src/lib.rs#L9941-L10006
[holds]: ../../../../crates/daemon/src/transform_unit.rs#L89-L160
[release]: ../../../../crates/daemon/src/lib.rs#L1405-L1425
[stage]: ../../../../crates/daemon/src/lib.rs#L1517-L1529
[evict]: ../../../../crates/daemon/src/lib.rs#L1427-L1445
[tests]: ../../../../crates/daemon/src/transform_unit/tests.rs#L143-L570
[meter-tests]: ../../../../crates/daemon/src/metered_decode.rs#L1188-L1310
[host-tests]: ../../../../crates/daemon/src/transform_unit/host_tests.rs#L245-L439
[ingress-observer]: ../../../../crates/host-runtime/src/handler.rs#L301
[pool-observer]: ../../../../crates/host-runtime/src/wire.rs#L468
[failed-unit-test]: ../../../../crates/daemon/src/transform_unit/tests.rs#L600

## Upstream accounting correction, e451a2b4

The upstream audit corrects the parse-admission description: allocation charges
are taken inside metered decode against the three-copy bound, not reserved in
full before decode. The pre-decode sentence above belongs to the pinned
discovery baseline. The [upstream inventory](https://github.com/ahrav/eidnara/blob/e451a2b4/docs/properties/hot-path-optimization/evidence/request-work-accounting-covers-retained-resources.md)
and its metered-decode source anchors preserve that correction. The transferred
charge ownership and exact-pool results from `d6060f79` remain historical evidence;
the five-case rebased meter run separately includes both transfer tests.

## Rebased admission and transfer, 2026-09-13

The merged [admit_body][body-admission] first checks meter refusal, before the
already-admitted shortcut. It rejects bodies whose value-count footprint floor
already exceeds capacity before parsing, then reserves the longest escaped
string's unescape scratch. Admission runs once before the lane probe. Thus the
upstream escape-scratch guard and early dense-body refusal remain in place;
the old uncharged-unescape and doomed-dense-decode scenarios are not claims
about this implementation.

The unpaged direct lane still requires [direct_lane_gate][direct-gate], a metered
tree-parse walk whose successful result precedes typed decode. Restart reuses
held bytes; it does not authorize a spent meter. `take_charges` sets `TAKEN`,
which maps to permanent refusal and survives restart and release. Consequently
an already-admitted body cannot use the latch to bypass terminal charge transfer.
`spent_meter_cannot_reenter_admitted_body` directly tests this refusal-before-latch
guard: reentry fails without another reserve call, and dropping the value and
charges restores the exact pool capacity. The [rebased receipt][rebased-receipt]
reports five meter passes, eleven blocking passes, and a final Bun pass. The
earlier full workspace still has three failures; no workspace pass is claimed.

[body-admission]: ../../../../crates/daemon/src/lib.rs#L16302
[direct-gate]: ../../../../crates/daemon/src/lib.rs#L15976
[rebased-receipt]: ../existing-checks.md#rebased-working-tree-verification-2026-09-13
