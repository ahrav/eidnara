# p6-new-attestation-stale-on-byte-change

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N8. Converted to active by [#1174](https://github.com/ahrav/eidnara/issues/1174), which lands the applicability step under OQ6-4 alternative 3, the push model inside the pass.

## Evidence trail

The witness detail the write set records (`WitnessDetail`, `crates/daemon/src/attestation_verifier/attest.rs:157`) names the compared descriptor's lineage id, its revision, and the witnessed `artifact_digest`. `Identity::of_attestation` (`crates/daemon/src/attestation_verifier/attest.rs:647`) recovers the witness observation's object id from the attestation's object id, since every id of the pair derives from one hash.

`evaluate` (`crates/daemon/src/attestation_verifier/applicability.rs:39`) reads the witness observation live at the pass sequence through `observation_for_object_as_of_within_budget` (`crates/kernel/src/slice/read.rs:799`), decodes its detail at `WITNESS_DETAIL_VERSION`, and reads the lineage's live `repository_documents` descriptor at the same sequence through `live_source_descriptor_of_lineage_within_budget` (`crates/kernel/src/source_descriptor.rs:1263`), which applies the page read's liveness predicate and row checks in the lineage's object id range (`lineage_descriptor`, `:1282`), the same read the envelope's `live_source_descriptor_of_lineage` (`:950`) makes. An absent witness, an undecodable detail, and no live descriptor are `Unavailable`; an equal digest is `Current`; another digest is `Republished`, carrying both digests, the path, the lineage, and the live descriptor's object id and revision.

`mark_republished` (`crates/daemon/src/attestation_verifier/applicability.rs:84`) commits under `(verifier, applicability:<attestation object id>:<descriptor revision>)` (`:100`) with a request digest over the attestation, the lineage, both digests, and the descriptor object id, through the write set's `run` (`crates/daemon/src/attestation_verifier/attest.rs:490`) with no opposite key. The envelope rereads the store incarnation; the attestation live with no successor, a decision, under `source_kind = "verifier"`; its latest admission `Active`, where `Stale` is `Skipped(AlreadyStale)` (`crates/daemon/src/attestation_verifier/applicability.rs:141`) and any other disposition is `Changed`; and the lineage's live descriptor still the compared object (`:150`), so a second republication between the read and the commit is `Changed`. It then records `MarkStale` under the latest admission's classes with `evidence_id: None` and a reason naming the path, the descriptor revision, and both digests, and requires the returned disposition `Stale`.

The pass runs the step after the decisions, at the same commit target (`ProjectRun::run`, `crates/daemon/src/attestation_verifier/bounded_pass.rs:399`). `ProjectRun::applicability` (`:628`) pages the project's live `verifier` decisions through `scope_decisions_of_source_after_within_budget` (`crates/kernel/src/slice/read.rs:725`) from a process-local cursor, reads their standing, passes over one whose own admission is not `Active`, and evaluates at most `MAX_VERDICTS` others (`crates/daemon/src/attestation_verifier/bounded_pass.rs:667`); an empty page returns the cursor to the start (`:642`). `ProjectRun::applicable` (`:681`) polls the pass deadline before the commit (`:695`) and counts the result. No repository is read and the Kernel writer is held only inside the envelope.

## Failure scenario

Bytes changed at the cited path, source capture published a new descriptor revision with the new digest, and the attestation is still `Active` after a pass evaluated it; or a second `MarkStale` for one revision.

## Timing windows and dependencies

Between the applicability read and the commit, the attestation can be retired, re-disposed, or superseded, and the lineage can be republished again or retired; the envelope recheck turns each into `Changed` with no write. A commit refused with `Io` can follow a durable commit: the next evaluation finds the attestation `Stale` and passes over it, and a direct retry replays the key's receipt. The step depends on `p6-document-republishes-only-on-digest-change-and-retires-on-missing`, so a new revision means changed bytes, and it never sees a change source capture did not publish.

## What a test must construct

An attestation committed by a pass, then a second descriptor revision of the cited document with other bytes, then a pass; controls with equal bytes, a retired descriptor, and an undecodable witness detail; an attestation retired from the commit poll; a repeated call under one key; more than N republished attestations.

`a_republished_document_marks_its_attestation_stale_once_and_leaves_the_model_decision_unchanged` (`crates/daemon/tests/attestation_verifier_bounded_pass.rs:1619`) attests, counts `applicability_current` on an unchanged pass, republishes revision 2 with other bytes, and finds one `MarkStale` on the attestation under `(trusted_local_code, current_code)` with no evidence, a reason holding both digests and `descriptor revision 2`, the receipt key `applicability:<attestation>:2`, and disposition `stale`; a later pass adds no row, and the model decision's registry row and admissions are unchanged. `an_equal_republish_a_retired_descriptor_and_an_undecodable_witness_write_nothing` (`:1683`) counts one `applicability_current` and two `applicability_unavailable` with no new row. `an_attestation_retired_before_its_stale_mark_commits_writes_nothing` (`:1741`) retires the attestation from the commit poll: `commit_refused_changed = 1` and no `MarkStale`. `a_replayed_stale_mark_key_writes_nothing_new` (`:1779`) calls `mark_republished` twice: `StaleCommitted`, then `Replayed` with no new admission. `the_applicability_step_evaluates_at_most_n_per_pass_and_resumes_after_its_cursor` (`:1819`) republishes N + 1 attestations: a deadline at the commit writes nothing, and the next two passes mark N and then 1. `the_source_page_walks_only_the_scopes_decisions_of_one_source_kind` (`crates/kernel/tests/kernel_scope_decisions.rs:140`) checks the page read.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record and this file, read at the #1174 branch.
- Findings: Each cited line is the first line of the named test, function, or type, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Which mechanism carries applicability?

- Sources examined: OQ6-4 and its resolution in [#1174](https://github.com/ahrav/eidnara/issues/1174).
- Findings: The owner chose alternative 3, the push model inside the pass. Alternative 1, a digest check kind in the Kernel applicability engine, is a handoff to #657, which owns the packing-path wiring of that engine.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: What leaves an attestation `Active` although its bytes changed?

- Sources examined: `evaluate`, the trigger scope in [#1174](https://github.com/ahrav/eidnara/issues/1174).
- Findings: a retired descriptor or a lineage with no live descriptor writes nothing, because absence also follows from disabled source capture; with source capture disabled nothing is republished; the `reachable_from` anchor alone stays satisfied at descendant commits; and the step runs only while the verifier is enabled. The limitation is documented in `docs/memory-reviewer-operations.md` section 9.5.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: How does the step progress under N without a cursor table?

- Sources examined: `ProjectRun::applicability`.
- Findings: the cursor is a process-local `(created_commit_seq, object_id)` pair per project; it passes an attestation once it is evaluated or passed over, stays before an attestation the deadline cut, and returns to the start when a page is empty, so each live `Active` attestation is evaluated once per cycle of passes. A restart or a store incarnation change starts the cycle over; a `Stale` attestation is passed over and a repeated stale mark replays its key, so a rescan writes nothing new.
- Missing evidence: none.
- Conclusion: resolved with answer.
