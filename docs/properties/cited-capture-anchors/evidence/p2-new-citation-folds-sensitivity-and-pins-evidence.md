# p2-new-citation-folds-sensitivity-and-pins-evidence

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N3, reduced to the evidence pin by the owner's OQ2-10 decision (decision-event placement), activated by the U3 ticket [#1135](https://github.com/ahrav/eidnara/issues/1135).

## Evidence trail

`append_citations` (`crates/daemon/src/memory_capture/cite.rs:358`) appends one `memory_capture_citation` event per cited block through `Envelope::append_decision_event`, which checks the evidence is live. The decision row keeps `evidence_id` empty, so `fold_cited_evidence_class` does not raise its class. `EVIDENCE_CITED_SQL` (`crates/kernel/src/slice/write.rs:42`) counts decision events of live decisions, so `expire_harness_captures` (`crates/kernel/src/local_file.rs:245`) keeps the cited evidence and `retire_evidence` refuses it with `Conflict`.

## Failure scenario

A sweep that retired a cited capture would leave the citation naming retired evidence; a row-level citation would hide the memory from auto-inject by folding `Sensitive` into it.

## Timing windows and dependencies

A sweep after the evidence's `retain_until`.

## What a test must construct

A cited decision, a sweep at a far-future instant, and a retirement attempt (`crates/daemon/src/source_capture_tests.rs:3761`); the cited decision's class and empty `evidence_id` (`crates/daemon/src/source_capture_tests.rs:3474`).

## Investigation log

### Q: Does any status or export surface read an unfolded class (OQ2-5)?

- Sources examined: served visibility through `served_rows_for` in the end-to-end test.
- Findings: Cited and uncited siblings serve with one visibility and class on every surface.
- Missing evidence: an audit of every export path.
- Conclusion: resolved with answer for serving; export paths read the stored class, which is the decision's own.
