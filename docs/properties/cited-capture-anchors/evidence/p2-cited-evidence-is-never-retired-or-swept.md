# p2-cited-evidence-is-never-retired-or-swept

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), existing-behavior record for S2, touchpoints C12, AC12, and OQ2-6 (cited capture evidence stays pinned for the life of the memory, owner decision 2026-10-25).

## Evidence trail

`EVIDENCE_CITED_SQL` (`crates/kernel/src/slice/write.rs:42`) is true when a live observation, live decision, decision event of a live decision, or live asserted edge names the evidence. `retire_evidence` (`:585`) refuses while it holds. `expire_harness_captures` (`crates/kernel/src/local_file.rs:245`) selects expired `harness_capture` rows and skips any that are cited (`:553`), re-checking at retirement so a pin added after selection is kept.

## Failure scenario

A sweep that retired a cited message would leave every capture citation pointing at a retired row.

## Timing windows and dependencies

A citation committed after the sweep selected the row and before it retired it.

## What a test must construct

A `harness_capture` row past `retain_until` cited by a live decision event; a pin added after selection; a retirement attempt while cited.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_source_descriptors.rs:1895`; `crates/daemon/tests/harness_sources.rs:3500`; `crates/kernel/src/local_file.rs:791`, `:889`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Did the expiry test move?

- Sources examined: `crates/daemon/tests/harness_sources.rs` at `a0e3f2993` and at the branch tip.
- Findings: `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages` moved from line 3331 at `a0e3f2993` to 3482 at `a734661b6`, and sits at line 3500 at HEAD.
- Missing evidence: none.
- Conclusion: resolved with answer: the record cites the current line.
