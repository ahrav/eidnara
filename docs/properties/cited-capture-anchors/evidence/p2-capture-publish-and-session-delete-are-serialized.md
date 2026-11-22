# p2-capture-publish-and-session-delete-are-serialized

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S3, touchpoint C7. Refinement R5 added the gate-hold observation method.

## Evidence trail

`capture_commit_gate` is one `Mutex<()>` per daemon handler. `publish` holds it for the frozen-plan read and the Kernel commit (`crates/daemon/src/memory_capture.rs:1815`); the checkpoint and the explicit capture route hold it while enqueuing; the source-capture job receives it through `capture_after_response` (`crates/daemon/src/lib.rs:9330`); session deletion holds it for the whole sweep (`crates/daemon/src/lib.rs:7068`). The failed-command recorder's detached task reads each command result ahead of the gate and holds the gate once per result, for the session-epoch check, the Kernel reads, the commit, and the memo update (`crates/daemon/src/memory_capture/failures.rs:2174`); `a_result_is_judged_under_the_capture_publication_gate` (`crates/daemon/src/memory_auto_delivery_tests.rs:6335`) pauses a judgment under the gate and observes that the gate is held and that a `session.delete` of the session waits for the judgment's commit, and `a_failed_command_of_a_pi_request_is_recorded_and_a_deleted_session_writes_nothing` (`crates/daemon/src/memory_auto_delivery_tests.rs:5539`) hands a task off, runs a `session.delete` that stops between its two stores, observes that the handed-off task writes nothing, completes the delete on a retry, and then observes that a fresh pass writes. Every holder runs on a blocking thread, so the gate is never held across an await.

## Failure scenario

A publish that committed after the delete sweep would leave a deleted session's memory live.

## Timing windows and dependencies

A checkpoint or publish admitted before a delete that runs after it.

## What a test must construct

Hold the gate from a test thread, issue the competing operation, assert it has not committed, and release. The outcome follows the order of the gate's holders: a capture or source publish that takes the gate after a completed delete commits nothing for the deleted session; a failed-command judgment that holds the gate first commits its fact ahead of the delete, and a failed-command task handed off before the delete, or while it runs, commits nothing once it takes the gate. A delete that ends between its two stores fences the failed-command task alone: a source publish handed off before it may still commit, and the retry of the delete retires what it published.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/src/lib.rs:25550`, `:25888`, `:26100`, `:26753`, `:25275`; `crates/daemon/src/source_capture_tests.rs:1062`, cited at their current lines and first read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which spec lines moved?

- Sources examined: `crates/daemon/src/lib.rs` at `a0e3f2993` and at the branch tip.
- Findings: The specification cited 24820, 25158, 25363, 25911, and 24569; at the #1133 branch the same tests started at 24851, 25189, 25394, 25942, and 24600; the record cites their current lines.
- Missing evidence: none.
- Conclusion: resolved with answer.
