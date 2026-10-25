# sc-retire-evidence-conflicts-while-cited

## Discovery trigger

KTD6 in the plan said `retire_evidence` conflicts "while anything cites or
holds it". Part A checked the Kernel and found the conflict covers live
citations only. U4 expiry depends on that rule to keep independently cited
evidence, and on its own pin predicate for holds (D6, C11).

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/slice/write.rs`, `EVIDENCE_CITED_SQL`: an
  `evidence_meta` row aliased `e` is cited when a live observation, a live
  decision, a decision event of a live decision, or a live asserted edge
  names its `evidence_id`. The doc comment states every sweep that predicts
  acceptance evaluates this same text.
- `crates/kernel/src/slice/write.rs`, `Envelope::retire_evidence` and
  `retire_evidence_inner`: loads the live evidence object, reads
  `EVIDENCE_CITED_SQL`, returns `Conflict` when cited and `NotFound` when no
  live evidence object or row exists, and otherwise invalidates the row and
  pushes an `evidence_retire` change. The predicate reads no
  `capture_pins` or `capture_pin_refs` table, so pins are not consulted.
- `crates/kernel/src/local_file.rs`, `retire_expired_capture` and
  `cited_elsewhere`: the expiry sweep retires the capture's own
  observations, then asks `cited_elsewhere` (the same predicate) and returns
  `"retained"` without calling `retire_evidence` when another row cites the
  evidence. Pins are checked separately through `EXPIRED_UNPINNED_SQL`.
- `crates/daemon/src/harness_sources.rs`, `SourcePublisher::retire_evidence`
  (private): commits `retire_evidence` under `source-evidence-retire:{key}`;
  a `NotFound` on an object whose registry row is already invalidated counts
  as retired, because a replayed artifact receipt for a compensated unit
  names that object.

Tests read at HEAD:

- `crates/kernel/tests/kernel_source_descriptors.rs`,
  `retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence`:
  an uncited row retires with `invalidated_commit_seq == receipt.commit_seq`
  and an `evidence_retire` outbox change; a later descriptor citing it is
  `EvidenceMissing`; a second retirement is `NotFound`; retiring cited
  evidence is `Conflict` with `tip()` unchanged, the descriptor live, and
  `invalidated_commit_seq == None`; a descriptor object is `NotFound`.
- `crates/daemon/tests/harness_sources.rs`,
  `permanent_refusal_reports_failed_retirement_attempt`: a trigger makes the
  compensating retirement fail with `Io`; the evidence stays live and the
  error reports the failed attempt.
- `crates/daemon/tests/harness_sources.rs`,
  `harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`:
  evidence cited by an independent `note` observation keeps its evidence
  while its descriptor retires.

## Failure scenario

A retirement removes evidence that an independent observation or decision
still cites, leaving a live row whose `evidence_id` names invalidated bytes,
or a conflicting retirement commits part of its envelope.

## Timing windows and dependencies

The check and the invalidation run in one writer transaction, so no citation
can land between them. A caller that wants pin safety must check pins itself
in the same transaction, as the expiry sweeps do.

## What a test must construct

A live citing descriptor and an uncited evidence row (F13), then
retirement of each. A pinned but uncited row shows that `retire_evidence`
alone retires it; the existing tests do not construct that case, so the pin
half is a code-reading fact.

## Investigation log

### Q: Does `retire_evidence` refuse while a capture pin holds the evidence?
- Sources examined: `retire_evidence_inner`, `EVIDENCE_CITED_SQL`.
- Findings: the predicate reads only observations, decisions, decision
  events, and asserted edges. Holds are enforced by the expiry predicate
  `EXPIRED_UNPINNED_SQL` and by GC.
- Missing evidence: a test retiring pinned, uncited evidence directly.
- Conclusion: resolved with answer by code reading; pins are not consulted.

### Q: Did the baseline citations move?
- Sources examined: baseline `08de9051a` line cites in comment 1.
- Findings: the test is `retiring_evidence_...` (now from line 1895); the
  harness test cited at `hs-tests:1244` is
  `permanent_refusal_reports_failed_retirement_attempt` (now line 1298).
- Missing evidence: none.
- Conclusion: resolved with answer; citations use function names.
