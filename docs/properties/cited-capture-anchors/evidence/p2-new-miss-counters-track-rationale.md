# p2-new-miss-counters-track-rationale

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N5 and constraint C14, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

The publish closure pushes one citation `Count` and one anchor `Count` per written decision into a caller-owned list, and one approval-refused `Count` per user-attributed decision a labeled commit writes. A publish whose approving commit the Kernel's admission policy refuses at a statement record runs the closure a second time as a labeled commit; each attempt clears the list first, so the refused attempt's reasons are not counted. `CaptureWork::publish` passes the list to `SourceCapture::record_capture_decisions` (`crates/daemon/src/source_capture.rs:676`) only after `commit_before` returned a receipt with `replayed == false`; a refused commit returns before the call, and a replayed receipt runs no closure. The eight counters join the closed `counts!` vocabulary, so `report()` emits them under `metrics.source_capture`, and the host sanitizer allowlist and the wire table carry their names (`crates/host-runtime/src/control.rs:709`, checked against the document by `:1967`). Each recorded reason also emits one log line, `memory decisions`, with the reason name and a count; a debug assertion keeps the recorded list to the eight decision counts. The approval-refused reason has no rationale field; the decision's admission event, `Other` with no approval, records the outcome. `decisions_publish_budget_expired` counts a decision whose lookup found no descriptor after its session's source publish was refused, ended on its deadline, or was still running at `accepted_at + CAPTURE_BUDGET` (`Citation::lookup`, `crates/daemon/src/memory_capture/cite.rs:301`); `a_publish_past_its_budget_releases_the_submit_at_the_deadline` (`crates/daemon/src/source_capture_tests.rs:4254`) observes it at 1, and the wire table row states the same condition.

## Failure scenario

A counter incremented inside the closure would count rolled-back publishes as misses.

## Timing windows and dependencies

A rollback after the reasons were computed; a lost reply followed by a replay.

## What a test must construct

The seven citation and anchor counters after a mixed submission (`crates/daemon/src/source_capture_tests.rs:3523`, `:3695`); a committed publish replayed on retry with every counter unchanged (`crates/daemon/src/source_capture_tests.rs:3871`); the approval-refused counter and the citation and anchor sums across a refused approving commit, the labeled commit, and its replay (`a_statement_whose_approval_the_kernel_refuses_is_stored_for_explicit_search_and_counted` in `crates/daemon/src/memory_auto_delivery_tests.rs`).

## Investigation log

### Q: Are the counters content-free?

- Sources examined: `record_capture_decisions` and `log_line`.
- Findings: Only closed names and counts reach the report and the log.
- Missing evidence: none.
- Conclusion: resolved with answer.
