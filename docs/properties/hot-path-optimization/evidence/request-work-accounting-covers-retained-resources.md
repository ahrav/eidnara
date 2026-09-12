# request-work-accounting-covers-retained-resources

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

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

[admission]: ../../../../crates/host-runtime/src/dispatch.rs#L823-L855
[owners]: ../../../../crates/host-runtime/src/dispatch.rs#L876-L934
[guards]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L513-L550
[decode]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L692-L763
[finish]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L787-L815
[checks]: ../existing-checks.md#execution-lifecycle
[parse]: ../../../../crates/daemon/src/lib.rs#L11856-L11877
[footprint]: ../../../../crates/daemon/src/lib.rs#L15459-L15505
[input]: ../../../../crates/host-runtime/src/handler.rs#L277-L295
[resident]: ../../../../crates/host-runtime/src/handler.rs#L474-L490
[caps]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L43-L82
[staging]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L87-L130
[decode-admission]: ../../../../crates/daemon/src/kernel_routes/ingest.rs#L406-L425
