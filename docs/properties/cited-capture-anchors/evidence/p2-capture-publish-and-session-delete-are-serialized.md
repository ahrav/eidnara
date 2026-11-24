# p2-capture-publish-and-session-delete-are-serialized

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S3, touchpoint C7. Refinement R5 added the gate-hold observation method.

## Evidence trail

`capture_commit_gate` is one `Mutex<()>` per daemon handler. `publish` holds it for the frozen-plan read and the Kernel commit (`crates/daemon/src/memory_capture.rs:2071`); the checkpoint and the explicit capture route hold it while enqueuing; the source-capture job receives it through `capture_after_response` (`crates/daemon/src/lib.rs:9334`); session deletion holds it for the whole sweep (`crates/daemon/src/lib.rs:7068`). The failed-command recorder's detached task reads each command result ahead of the gate and holds the gate once per result, for the session-epoch check, the Kernel reads, the commit, and the memo update (`crates/daemon/src/memory_capture/failures.rs:1829`); `a_result_is_judged_under_the_capture_publication_gate` (`crates/daemon/src/memory_auto_delivery_tests.rs:8510`) pauses a judgment at a hook under the gate, after its read of the Kernel history and ahead of its reads of the log and the lineage and its commit, and observes that the gate is held and that a `session.delete` of the session has not finished while the hook blocks, then observes at a second hook between the judgment's reads and its commit that the gate is still held and the delete still unfinished, and `a_failed_command_of_a_pi_request_is_recorded_and_a_deleted_session_writes_nothing` (`crates/daemon/src/memory_auto_delivery_tests.rs:7040`) hands a task off, runs a `session.delete` that stops between its two stores, observes that the handed-off task writes nothing, completes the delete on a retry, and then observes that a fresh pass writes. The delete's fence (`DeleteFence`, `crates/daemon/src/memory_capture/failures.rs:328`) retires the session's memo entry when it is raised and again as it releases the gate, and `a_delete_fence_retires_the_session_s_entry_at_both_ends` (`crates/daemon/src/memory_capture/failures.rs:4676`) observes both retirements over a memo and a gate of its own. Every holder runs on a blocking thread, so the gate is never held across an await.

## Failure scenario

A publish that committed after the delete sweep would leave a deleted session's memory live. A failed-command task that committed after the delete of its session, having been handed off before it or while it held the gate, would write a fact after the delete's fence retired the session's place in the memo. A fact committed before the delete stands, and a pass handed off after the delete writes as before.

## Timing windows and dependencies

A checkpoint, a publish, or a failed-command task admitted before a delete that runs after it; a failed-command task handed off while the delete holds the gate; a delete that stops between its Kernel step and its memory-store delete, and its retry.

## What a test must construct

Hold the gate from a test thread, issue the competing operation, assert it has not committed, and release. The outcome follows the order of the gate's holders: a capture publish handed off before a completed delete commits nothing for the deleted session once it takes the gate, and a pass handed off after the delete publishes as before; a source publish commits only while the delete epoch its pass sampled equals the daemon's when it compares them under the gate, and the epoch is one counter for the daemon, so a pass publishes nothing when a completed delete of any session advanced the epoch between the sample and that comparison, also when it hands off after the delete, and a pass that samples after it publishes as before; a failed-command judgment that holds the gate first commits its fact ahead of the delete, and a failed-command task handed off before the delete, or while it holds the gate, commits nothing once it takes the gate. A delete that ends between its two stores fences the failed-command task alone: a source publish handed off before it may still commit, and the retry of the delete retires the deleted session's descriptors among what it published.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/daemon/src/lib.rs:25554`, `:25892`, `:26104`, `:26757`, `:25279`; `crates/daemon/src/source_capture_tests.rs:1062`, cited at their current lines and first read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which checks cover the failed-command recorder as a holder of the gate?

- Sources examined: `crates/daemon/src/memory_capture/failures.rs:1829`, `crates/daemon/src/memory_capture/failures.rs:328`, `crates/daemon/src/memory_capture/failures.rs:1839`, `crates/daemon/src/memory_capture/failures.rs:4676`; `crates/daemon/src/memory_auto_delivery_tests.rs:8510`, `crates/daemon/src/memory_auto_delivery_tests.rs:7040`.
- Findings: The gate test pauses a judgment at a hook under the gate and observes the gate held and a `session.delete` of the session unfinished, there and again at a second hook between the judgment's reads and its commit. The delete test hands a task off, stops a delete between its two stores, and observes that the task writes nothing and that a fresh pass writes after the retry. The fence test observes both retirements of the session's memo entry over a memo and a gate of its own.
- Missing evidence: a test of a task that starts its gate wait while the delete holds the gate, a test that observes the fence's last retirement and its gate release as one step, and a test that observes the gate held through the commit itself and the memo update after it.
- Conclusion: resolved with answer.

### Q: Which spec lines moved?

- Sources examined: `crates/daemon/src/lib.rs` at `a0e3f2993` and at the branch tip.
- Findings: The specification cited 24820, 25158, 25363, 25911, and 24569; at the #1133 branch the same tests started at 24851, 25189, 25394, 25942, and 24600; the record cites their current lines.
- Missing evidence: none.
- Conclusion: resolved with answer.
