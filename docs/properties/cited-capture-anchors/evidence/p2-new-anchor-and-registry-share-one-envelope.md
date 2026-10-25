# p2-new-anchor-and-registry-share-one-envelope

## Discovery trigger

Specification [#1130](https://github.com/ahrav/eidnara/issues/1130), proposed obligation for N1, activated by the U1 ticket [#1133](https://github.com/ahrav/eidnara/issues/1133).

## Evidence trail

`insert_anchor_inner` (`crates/kernel/src/anchor_write.rs:320`) validates, requires a live domain, inserts the `object_registry` row of kind `anchor`, records its redaction fields, inserts the `anchors` row at the envelope's `commit_seq`, and pushes one `anchor_insert` change. `Envelope::guarded` (`crates/kernel/src/envelope.rs:348`) poisons the envelope on any refusal, so the commit returns that error and rolls back every row of the closure. A duplicate `object_id` fails the registry insert and a duplicate `anchor_id` fails the `anchors` primary key; `map_sqlite` maps both to `Conflict`. The claim materializer filters outbox rows to `object_kind == "decision"` (`crates/daemon/src/claim_sources.rs:1117`), so anchor changes reach no decision parser. The daemon read routes read through `KernelStore::visible_as_of_in_scope` (`read_visible`, `crates/daemon/src/kernel_routes/read.rs:163`, call at `:180`), and an object serves only after its own committed admission (`crates/kernel/src/admission.rs:3042`); an anchor has none, so it is never a served row.

## Failure scenario

A registry row committed without its anchor row would leave an orphan object; a decision committed before its anchor would fail the parent check.

## Timing windows and dependencies

A refusal after the anchor insert inside one envelope.

## What a test must construct

An anchor insert followed by a refused decision insert in one closure; a duplicate anchor id within one envelope and across two commits with distinct operation keys; a materializer run over anchor-only and anchor-plus-decision commits, followed by a served-rows read on the explicit-search and auto-inject surfaces.

## Investigation log

### Q: Do the cited checks exist at the branch tip under the cited names?

- Sources examined: `crates/kernel/tests/kernel_anchor_write.rs:399`, `:429`, `:654`; `crates/daemon/tests/claim_sources.rs:1508`, read at the branch that adds this part (base `3a3b916b7`).
- Findings: Each cited line is the first line of the named test or function.
- Missing evidence: none.
- Conclusion: resolved with answer.
