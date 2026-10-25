# p2-new-head-observed-once-and-frozen

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N2, placement resolved by OQ2-4 (read at
admission and freeze in the plan, owner decision 2026-10-25). Activated by
the U2 ticket [#1134](https://github.com/ahrav/eidnara/issues/1134); the U3
ticket [#1135](https://github.com/ahrav/eidnara/issues/1135) adds the clause
that each anchor's oid equals its plan's frozen head.

## Evidence trail

`accept_native_output` (`crates/daemon/src/memory_capture/native.rs:549`)
parses the submission's output, then calls `observe_head` once
(`:563`) on the binding's canonical project root before the loop that
builds and freezes one `PreparedCapture` per job. Each plan carries the
same `head` value (`crates/daemon/src/memory_capture.rs:926`), serialized
as `{"commit":"<40 hex>"}` or `"head_unavailable"`
(`crates/daemon/src/memory_capture/head.rs:10`). The store freezes a plan
once (`prepare_memory_capture`, `crates/memory-store/src/memory_capture.rs:373`,
returns the stored text to a later call), and `publish` reads the stored
text, so a retry publishes the value frozen at submit. The commit's request
digest is the SHA-256 of the frozen text
(`crates/daemon/src/memory_capture.rs:1191`), so the receipt covers the head.

The observation runs inside the `kernel_routes::blocking` hop of the submit
handler (`crates/daemon/src/memory_capture/native.rs:363`), before `publish`
takes the capture commit gate.

A plan frozen with a `head` field is refused by a daemon built before the
field existed, because `PreparedCapture` denies unknown fields; rolling back
the daemon with prepared plans in flight is unsupported. The derived
`Deserialize` accepts any string in `Commit`; the U3 consumer passes it to
`insert_anchor`, whose decoder refuses anything but a lowercase hex oid.

## Failure scenario

A read per job, or a read at publish time, would let a checkout between two
jobs of one submission, or between a submit and its retry, give the jobs
different heads and a retry a head its plan never recorded.

## Timing windows and dependencies

A commit or checkout on the project between the submit and a retry. The
plan is frozen before the first publish, so the window cannot reach it.

## What a test must construct

A two-job submission on a repository fixture with completion refused by a
store trigger, so both frozen plans stay readable; a new commit on the
checked-out branch; a retry while the trigger still refuses completion, after
which both plans still hold the submit-time head; then the trigger dropped and
a drain that publishes the frozen plans. `a_submission_freezes_one_checkout_head_in_every_plan_and_a_retry_keeps_it`
(`crates/daemon/src/lib.rs:24619`) does this for a repository with
commits, a detached HEAD, an unborn HEAD, a plain directory, and a
repository declaring SHA-256.

## Investigation log

### Q: Is the head read once per submission?

- Sources examined: `accept_native_output` and its callers at the branch
  that adds U2.
- Findings: one call precedes the per-job loop; `settle` calls
  `accept_native_output` once per submit. The route test sees two identical
  heads, which a per-job read would also produce because nothing moves HEAD
  between jobs; the single read rests on the code structure.
- Missing evidence: a test that moves HEAD between two jobs of one
  submission, which needs a hook inside the loop.
- Conclusion: resolved with answer.

### Q: Does the plan schema version change?

- Sources examined: `CAPTURE_SCHEMA_VERSION` and `PreparedCapture`.
- Findings: the version stays 1; `head` is `#[serde(default)]`, so a plan
  without it reads as `HeadUnavailable`
  (`crates/daemon/src/memory_capture.rs:1947`).
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does the anchor use the frozen value?

- Sources examined: `anchor_spec` (`crates/daemon/src/memory_capture/cite.rs:341`)
  and the publish closure at the U3 branch.
- Findings: the anchor's oid is the plan's `FrozenHead::Commit`; the U3
  tests compare each anchor row's oid with the fixture HEAD at submit
  (`crates/daemon/src/source_capture_tests.rs:3474`, `:3624`).
- Missing evidence: none.
- Conclusion: resolved with answer.
