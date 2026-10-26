# p2-new-miss-counters-track-rationale

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N5 and constraint C14, activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

The publish closure pushes one citation `Count` and one anchor `Count` per written decision into a caller-owned list. `CaptureWork::publish` passes the list to `SourceCapture::record_capture_decisions` (`crates/daemon/src/source_capture.rs:659`) only after `commit_before` returned a receipt with `replayed == false`; a refused commit returns before the call, and a replayed receipt runs no closure. The seven counters join the closed `counts!` vocabulary, so `report()` emits them under `metrics.source_capture`, and the host sanitizer allowlist and the wire table carry their names (`crates/host-runtime/src/control.rs:709`, checked against the document by `:1960`). Each recorded reason also emits one log line, `memory decisions`, with the reason name and a count; a debug assertion keeps the recorded list to the seven decision counts. `decisions_publish_budget_expired` counts a decision whose lookup found no descriptor after its session's source publish was refused, ended on its deadline, or was still running at `accepted_at + CAPTURE_BUDGET` (`Citation::lookup`, `crates/daemon/src/memory_capture/cite.rs:334`); `a_publish_past_its_budget_releases_the_submit_at_the_deadline` (`crates/daemon/src/source_capture_tests.rs:4205`) observes it at 1, and the wire table row states the same condition.

## Failure scenario

A counter incremented inside the closure would count rolled-back publishes as misses.

## Timing windows and dependencies

A rollback after the reasons were computed; a lost reply followed by a replay.

## What a test must construct

The seven counters after a mixed submission (`crates/daemon/src/source_capture_tests.rs:3496`, `:3646`); a committed publish replayed on retry with every counter unchanged (`crates/daemon/src/source_capture_tests.rs:3822`).

## Investigation log

### Q: Are the counters content-free?

- Sources examined: `record_capture_decisions` and `log_line`.
- Findings: Only closed names and counts reach the report and the log.
- Missing evidence: none.
- Conclusion: resolved with answer.
