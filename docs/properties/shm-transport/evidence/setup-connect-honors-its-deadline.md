# setup-connect-honors-its-deadline

## Discovery trigger

Round 17 review of the PR: `connect_honors_the_deadline_when_the_backlog_is_full`
fills a listener backlog and asserts a bounded timeout, and no liveness record
owned the setup-connect deadline.

## Evidence trail

- `packages/shm-native/src/setup.rs:97-108`: `begin_connect` rejects a zero
  timeout, derives one `deadline`, and calls `connect_until` before
  `authenticate`.
- `setup.rs:143-146`: the doc comment records the hazard: Linux parks a blocking
  `AF_UNIX` connect for `SO_SNDTIMEO`, which std never sets.
- `setup.rs:147-186`: `connect_until` returns `TimedOut` when no budget remains
  (`:149-151`), floors the budget to whole microseconds (`:152-155`), creates
  the socket with `CLOEXEC` (`:156-161`), sets `SO_SNDTIMEO` to the remaining
  budget before each attempt (`:173`), maps `EAGAIN` to `TimedOut` (`:185`),
  and recomputes the budget on `EINTR` (`:176-184`).
- Tests: `connect_honors_the_deadline_when_the_backlog_is_full` (`:785-829`)
  fills a real backlog with non-blocking connects until `EAGAIN` (`:796-811`),
  then asserts `TimedOut` within five seconds of a 200 ms deadline
  (`:814-828`); `connect_succeeds_against_an_accepting_listener` (`:831`)
  covers the positive path.

## Failure scenario

The host's accept loop wedges with its backlog full. A client using a plain
blocking connect parks indefinitely inside setup, past its own timeout, with no
error and nothing for the caller to observe.

## Timing windows and dependencies

The kernel enforces the deadline through `SO_SNDTIMEO`; the test allows
scheduling slack by asserting under five seconds for a 200 ms budget.

## What a test must construct

A full backlog and a bounded `TimedOut`, present. Missing: a signal delivered
during the blocking connect, asserting the re-armed timeout equals the remaining
budget.

## Investigation log

### Q: Does anything pin the `EINTR` re-arm?

- Sources examined: `setup.rs:170-186`, both tests.
- Findings: the re-arm is implemented and commented; no test interrupts the
  connect.
- Missing evidence: an interrupted-connect test.
- Conclusion: Exercised stays yes for the deadline itself; the re-arm is the
  open question.
