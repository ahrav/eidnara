# p5dep-cited-evidence-survives-the-harness-sweep

HEAD `3a3b916b7`. Surface S6.

## Discovery trigger

`schema.rs:136` and `local_file.rs:240`: the sweep "keeps evidence another
live row cites". Wildcard finding W1: `EVIDENCE_CITED_SQL` counts decision
events of live decisions, and a P4 `dispute` writes a decision event with an
`evidence_id`. A confirmed P5 candidate therefore changes what the sweep does
to the evidence it cites. Lenses: data integrity; wildcard.

## Evidence trail

- `crates/kernel/src/slice/write.rs:42-54` `EVIDENCE_CITED_SQL`: `EXISTS
  observations o WHERE o.evidence_id = e.evidence_id AND live` OR `EXISTS
  decisions d WHERE d.evidence_id = ... AND live` OR `EXISTS decision_events
  de JOIN decisions d ON d.decision_id = de.decision_id WHERE de.evidence_id
  = ... AND d.invalidated_commit_seq IS NULL` OR `EXISTS asserted_edges a ...
  live`.
- `write.rs:579-585` `retire_evidence` doc: "Rejects retirement while a live
  observation, decision (including its events), or asserted edge cites the
  evidence." `:589-608`: reads `cited` with that SQL; `if cited { return
  Err(Conflict) }` (`:606-608`).
- `crates/kernel/src/local_file.rs:487-501` `retire_capture`: retires the
  capture's own `source_descriptor` observations first, then `if
  cited_elsewhere(envelope, evidence_id)? { return Ok("retained") }`, else
  `retire_evidence`.
- `local_file.rs:585-602` `cited_elsewhere`: the same `EVIDENCE_CITED_SQL`
  over `evidence_meta e WHERE e.evidence_id=?1`; doc: "such a row is
  independent support the expiry must not remove. The predicate is the one
  `retire_evidence` enforces."
- `local_file.rs:549-565` candidate SQL: a capture whose owners are already
  retired and whose evidence is cited is not a candidate ("has nothing left
  to retire until that citation goes").
- `local_file.rs:28-30` `CaptureExpiry.retained`: "captures lost only their
  owning observations because another live row still cites the evidence."
- P4 spec (`/tmp/p5-spec/spec-p4-1159.md`, D8, C6, C7): `dispute` commits a
  `MarkDisputed` or `Contradict` event with `evidence_id` on the target
  decision; `crates/kernel/src/admission.rs:4234`: `MarkDisputed =>
  Disposition::Disputed, Outcome::Deny`. The event row carries the evidence
  id (the `AdmissionEvent.evidence_id` field, `kernel_scope.rs:302` shows the
  shape).
- `source_capture.rs:557-560` `record_expiry` surfaces `retained` as
  `expiry_retained` in the capture report.
- Tests: none found that place a foreign citation on a harness capture's
  evidence and sweep. `retire_evidence`'s `Conflict` is exercised elsewhere
  (`abandon_local_file_capture` path, `local_file.rs:376-380`).

## Failure scenario

Without the citation guard, the sweep would retire evidence that a live
decision's dispute event cites; the dispute would then point at retired
evidence and any later read of the dispute's support would find it gone.
With the guard (current code), the evidence of a confirmed candidate outlives
its 30-day class for as long as the disputed decision is live, while its
descriptor is retired. P5 R4's sweep rule ("drop rows whose descriptor is
retired") and P4's confirm rule ("evidence live") therefore disagree about a
confirmed candidate: the descriptor is gone, the evidence stays, and the
candidate row's fate depends on which the P5 sweep keys on. Also, the
retention promise "30 days from observation" is extended by the citation;
that is the documented behavior ("keeps evidence another live row cites").

## Timing windows and dependencies

The citation check runs inside the writer transaction after the owners are
retired in the same transaction, so a dispute committed between candidate
selection and the sweep's writer is seen. A dispute committed after the
evidence is retired is refused by P4 C6 (`not_found`).

## What a test must construct

- A Kernel fixture: publish one descriptor with an `observed_at` 31 days
  back; commit a `memory` decision; commit an admission event on that decision
  with `evidence_id` = the descriptor's evidence id (the `dispute` shape);
  call `expire_harness_captures(now)`.
- Assert: descriptor `invalidated_commit_seq` set; evidence
  `invalidated_commit_seq` NULL; `CaptureExpiry { retired: 0, retained: 1 }`;
  a second sweep call finds no candidate (the retained capture leaves the
  sweep).
- Then retire the decision and sweep again; assert the evidence is retired.

## Investigation log

### Q: Does P5 R4 intend to drop a confirmed candidate when its descriptor is
retired but its evidence is live?

- Sources examined: plan R4, P4 C6/C7, `local_file.rs:487-501`,
  `write.rs:42-54`.
- Findings: the plan keys the sweep on "descriptor is retired"; after a
  confirm, that is true while the evidence remains live and citable. Dropping
  the row loses the record that a candidate was confirmed, though the dispute
  event itself persists in the Kernel.
- Missing evidence: the P5 author's intent.
- Conclusion: needs human input.
