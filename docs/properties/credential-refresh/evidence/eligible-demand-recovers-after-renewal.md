# eligible-demand-recovers-after-renewal

## Discovery trigger

Specification #860, record N9: separately for the normal, emergency, wrapup,
and reattach paths, `sometimes(valid fold after source failure and recovery
under the same daemon and routes)`, plus 24 rotations with one external login.
The specification grants no idle or outage-unconditional progress; the witness
needs cooldown expiry, a usable source response, and an eligible operation.
The M4 ticket (#872) lands the record with its runnable checks.

## Evidence trail

Path witnesses in `crates/daemon/src/source_recovery_tests.rs`, each injecting
a `CredentialSource`-scoped transient failure with `retry_after_secs: 120`
through `fail_next_run_on_the_source`:

- `:68` normal: the first transform fires and fails on the source, nothing
  advances, a second transform is refused as `backoff`, then
  `fire_and_settle` publishes a model fold with exactly two producer starts.
- `:93` emergency: after the failed firing, `expire_history_summarizer_backoff`
  moves the backoff into the past and the next usage-driven transform fires;
  the witness waits for the second start and for the summarizer to reach idle
  at `firing_seq >= 2`.
- `:120` wrapup: the first `session.wrapup` returns `ok: false` with
  `disposition: "retryable"`; after the backoff expires the next wrapup,
  awaited under `tokio::time::timeout(TEST_WAIT_BUDGET, ..)`, returns
  `ok: true` and a model fold is published. The handler's own wrapup budget is
  `MAX_WRAPUP_REQUEST_BUDGET` less a margin (`crates/daemon/src/lib.rs:6160`,
  3,800 seconds at `crates/daemon/src/history_summarizer.rs:1580`), so the
  test's timeout is the bound that applies.
- `source_recovery_tests.rs:153` reattach: a seeded awaiting state answers
  `no_fire: "reattaching"`, the failure starts no model, then
  `fire_and_settle` publishes.

Shared assertions: `assert_the_source_failure_advanced_nothing` (`:28`) checks
no segment, no `chunk_retry`, an unchanged `coverage_ordinal`, a backoff at
least 115 seconds out, and a recorded failure; `assert_a_model_fold_published`
(`:46`) requires a non-archive segment with no "Unsummarized messages" title
and no `chunk_retry`.

The bound comes from the wait helpers in `crates/daemon/src/lib.rs`:
`TEST_WAIT_BUDGET` is 10 seconds and `TEST_WAIT_POLL` 2 ms (`:36613`);
`wait_for_idle` (`:36616`), `wait_for_count` (`:36641`), and
`wait_for_history_summarizer_state` (`:36652`) panic at the budget.
`fire_and_settle` (`:41620`) expires the backoff, calls transform, accepts only
`fired` or `no_fire: "busy"`, retries `busy` only until a deadline of
`TEST_WAIT_BUDGET` from its first attempt, and on `fired` waits within the
budget for a higher `firing_seq` at idle.

Rotation witness: `crates/host-runtime/tests/model_execution_subprocess.rs:4266`
`a_day_of_rotations_and_an_external_login_reuses_one_adapter_and_owner` runs
49 Bedrock runs on a shifted wall clock, 48 at 1,760-second steps and one at
the 24-hour mark, through one OpenCode adapter and one owner. The 13th refresh
observes an external login (`TransactionFailure::Withdrawn`); only slot 24
fails, as `Transient`. The test asserts the exact `ASIAROW<n>` sequence and 26
physical refreshes. It is registered at `:328`.

## Failure scenario

The source answers again after the cooldown, an eligible operation arrives,
and the summarizer still refuses to fire or fires without publishing. Folding
stalls on that path until the daemon restarts, while the other paths may
continue.

## Timing windows and dependencies

The window is the cooldown boundary: the backoff recorded at the source
failure must have expired at the next eligible operation. The recovery bound
is one firing after expiry inside two consecutive 10-second windows: the
emergency path waits for the second start, then for idle; the normal and
reattach paths retry `busy` under one deadline, then wait for idle under
another; the wrapup path has one window around its request. Each window is
`TEST_WAIT_BUDGET`, so the end-to-end bound is 20 seconds. The rotation
witness depends on the shifted clock seam and a scripted dispatch; the path
witnesses depend on the scripted producer.

## What a test must construct

A typed source failure on each of the four paths; backoff expiry under test
control; a usable producer response on the retry; an eligible operation on the
same handler and store; an assertion that the fold is a model fold, not an
archive or placeholder; a producer start count that proves exactly one retry.

## Investigation log

### Q: Is the recovery bound stated in units the code bounds?

- Sources examined: the four path tests, `fire_and_settle`, the three wait
  helpers, `TEST_WAIT_BUDGET`.
- Findings: attempts are bounded to one firing after expiry (the start counts
  and `firing_seq` assertions); time is bounded by the 10-second budget per
  wait. An earlier revision of `fire_and_settle` retried `busy` without a cap,
  so a summarizer that stayed busy would have run the normal and reattach
  witnesses to the runner's timeout instead of failing them; the helper now
  asserts a `TEST_WAIT_BUDGET` deadline across those retries. The wrapup
  witness awaited its recovered request under the handler's 3,800-second
  budget alone; it now awaits under `TEST_WAIT_BUDGET`.
- Missing evidence: none for the five checks read.
- Conclusion: resolved with answer - one firing, inside two consecutive
  10-second windows, 20 seconds end to end; the wrapup path uses one window.
  Each helper opens its own deadline, so the record states the cumulative
  bound rather than a single shared one.
