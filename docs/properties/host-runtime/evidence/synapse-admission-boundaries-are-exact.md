# synapse-admission-boundaries-are-exact

## Discovery trigger

`jobs.rs:1` describes `JobTable` as retaining "bounded process-local batch
jobs" with "ephemeral retention". Two caps bound live work (`max_queued_jobs`,
`max_queued_request_bytes`) and two bound completed work (`max_retained_jobs`,
`max_retained_result_bytes`), all defaulted at `mod.rs:91-94`. The hazard is
an off-by-one at either live boundary, or an eviction routine that reclaims a
queued or running job to make room, which would lose a caller's result while
the caller still holds a valid `job_id`. The audit traced the admission
predicate, the eviction filter, and the expiry sweep.

## Evidence trail

All references are at `e16e39e`, paths relative to `crates/host-runtime/`.

Admission. `admit_charged` (`src/synapse/jobs.rs:305-398`) takes the table
lock, sweeps expired jobs (`:322`), resolves an existing key (`:326-343`),
checks the result-byte cap (`:345-350`), then counts live jobs as
`!job.is_completed()` (`:352-356`). The predicate at `:357-360` is
`admitted >= max_queued_jobs || queued_text_bytes + text_bytes >
max_queued_request_bytes`, returning `AdmitOutcome::Full` (`:362`). Both
comparisons are exact: a count equal to the cap rejects the next job, and a
byte sum equal to the cap admits. `is_completed` is `Ready | Failed`
(`:125-127`), so `Queued` and `Running` are the live set. `handle_batch` maps
`Full` to `queue_full` (`src/synapse/mod.rs:685`).

Live work is never evicted. Every removal site filters on completion:
`enforce_retention` selects only `is_completed()` jobs (`:639`, `:649`);
`sweep_expired` requires `completed_at`, which is set only at `:451` and `:486`
(`:623-624`); the retryable-failure replacement at `:366-368` removes a
`Failed` job. The exception is shutdown: `close_admission` removes every
`Queued` job (`:676-684`), called from `shutdown` at `src/synapse/mod.rs:984`.

Eviction order. `enforce_retention` (`:634-661`) loops while the completed
count exceeds `max_retained_jobs` or retained result bytes exceed the cap
(`:641-643`), picking `min_by_key(completed_at)` among completed jobs other
than `keep` (`:646-651`). If only the protected job remains, it is evicted
(`:652-657`). `publish_ready` calls it with `Some(seq)` at `:458`.

Charges. `publish_ready` moves `text_bytes` to zero (`:443`), subtracts it
from `queued_text_bytes` (`:456`), and releases the input charge down to the
retained metadata (`:453-455`). `remove` (`:663-669`) subtracts both byte
counters on any removal.

Expiry. `sweep_expired` removes jobs whose `completed_at` is at least
`retention` old (`:617-631`). It runs at admission (`:322`), at poll (`:499`),
at `key_is_retained` (`:288`), and from `sweep` (`:610-615`), which
`handle_result` and `handle` call only after a reservation failure
(`mod.rs:765`, `:894`). There is no timer. `poll` returns `Restarted` for an
unknown seq (`:503-504`), mapped to `module_restarted` at `mod.rs:780-783`.

Existing checks, all in `tests/synapse_jobs.rs`, a default-harness binary
that CI runs via `cargo test --workspace --all-targets`
(`.github/workflows/ci.yml:118`):

- `admission_count_boundary_is_exact_and_never_evicts_live_work` (`:41-89`):
  `max_queued_jobs: 2` with a 300 ms engine delay; two batches admit, the
  third is `queue_full` (`:74`), and after 900 ms it admits (`:86`).
- `queued_byte_boundary_is_exact_and_releases_on_completion` (`:131-178`):
  `max_queued_request_bytes: 8`; an eight-byte text admits, a one-byte text
  is `queue_full` (`:163`), and after completion it admits (`:175`).
- `completed_jobs_evict_oldest_first_under_count_pressure` (`:181-236`):
  `max_retained_jobs: 1`; the second completion evicts the first, and polling
  it returns `module_restarted` (`:233`).
- `expired_jobs_return_module_restarted` (`:239-275`): `retention: 100ms`;
  a poll 250 ms after readiness returns `module_restarted` (`:272`).

`boundary_waiters_with_maximal_texts_are_all_admitted` is in
`tests/synapse_protocol.rs:415-468`, not `synapse_jobs.rs`. It is `#[ignore]`
(`:412-414`) because it opens 33 ring clients and `MAX_RING_RESIDENT_BYTES`
(`src/ring_transport.rs:58`, 1 GiB) admits at most eight rings per process
(`:60-67`). It also concerns query-waiter admission, not the job table.

## Failure scenario

1. A caller admits job A, then B, filling `max_queued_jobs`.
2. A third caller submits C while A is still running.
3. A defect that evicted the oldest job regardless of state would remove A,
   and A's caller would later poll into `module_restarted` with no signal
   that the result was discarded rather than never produced.

As written, `enforce_retention` cannot see A because of the `is_completed`
filter, and `admit_charged` rejects C at `:357-362` before any insertion.

## Timing windows and dependencies

Expiry is lazy. A job whose retention has elapsed stays in the table until
some request or reservation failure triggers a sweep, so `retention` is a
lower bound on visibility, not an exact lifetime. This also means expired
jobs can hold `max_retained_jobs` slots until the next sweep; `admit_charged`
sweeps first (`:322`), so admission itself is unaffected.

`min_by_key(completed_at)` iterates a `HashMap` (`:130-137`), so two jobs
completing within the same `Instant` tick evict in map order. The tests
complete jobs sequentially and do not exercise a tie.

The count test relies on real time (300 ms delay, 900 ms sleep) rather than a
paused clock; it is not flaky by construction but is slow.

## What a test must construct

1. Direct `JobTable` tests at both boundaries with a `Running` job (via
   `start`, `:400-409`) plus completed jobs at `max_retained_jobs`, asserting
   after `publish_ready` that the running job's seq is still present and the
   oldest completed seq is gone. The host-level tests infer this from wire
   codes only.
2. A tie case: two jobs published in the same tick, asserting which survives,
   or asserting that the property holds regardless of which is chosen.
3. A shutdown case asserting that queued jobs removed by `close_admission`
   are the only live jobs ever removed, and that their callers observe
   `module_restarted` after restart rather than a silent gap.

## Investigation log

### Q: Rewrite or drop the ignored `boundary_waiters` test for the ring cap?

- Sources examined: `tests/synapse_protocol.rs:378-468`;
  `src/ring_transport.rs:57-66`; the record's `Confidence` line.
- Findings: the test is ignored with the ring-cap reason (`:412-414`). Its
  subject is `max_waiting_queries` (`WAITER_BOUNDARY`, `:385`) and the query
  admission semaphore, not the job table's count or byte caps. The adjacent
  non-ignored test at `:387-409` covers the configuration boundary for
  waiters. Whether a multiplexed single-ring rewrite is feasible depends on
  whether one client can hold 33 concurrent in-flight queries.
- Missing evidence: a decision on whether the waiter runtime path needs a
  host-level test at all, given the configuration-time check.
- Conclusion: needs human input. The catalog's `Confidence` line attributes
  the test to `synapse_jobs.rs`; it lives in `synapse_protocol.rs:415`.

### Q: Is the byte boundary exact for the sum, not just the single item?

- Sources examined: `src/synapse/jobs.rs:313`, `:358-359`, `:376`, `:456`.
- Findings: `text_bytes` sums every item (`:313`); the predicate adds it to
  the running `queued_text_bytes` with `saturating_add` and rejects on strict
  `>` (`:358-359`). Admission adds to the counter (`:376`); completion and
  failure subtract (`:456`, `:491`). The test exercises one item per batch.
- Missing evidence: a multi-item batch that lands exactly on the cap.
- Conclusion: resolved by code reading; the multi-item case is untested.
