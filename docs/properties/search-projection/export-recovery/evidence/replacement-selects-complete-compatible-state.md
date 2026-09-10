# replacement-selects-complete-compatible-state

Repository: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`. Date: 2026-09-10.
User-supplied scope: plan, linked parent/index/research, and local repo.
No incident logs or runtime evidence are supplied. Source aliases resolve in
[catalog sources](../catalog.md#sources).

## Discovery trigger

P U5, lines 158-163, requires staging, fenced coverage comparison, atomic
selection, then release of old consumer state. R5 at P, line 41, lists schema,
tokenizer, embedding-model, projection-policy, and identity-contract mismatch.
The lifecycle pass separates generic selector validity from search completeness.

## Evidence trail

- `crates/host-runtime/src/generation.rs:438-469` reads a profile as absent,
  selected, quarantined, or error; it does not validate search coverage.
- `crates/host-runtime/src/generation.rs:473-546` validates manifest bytes,
  canonical encoding, listed file content, and complete directory membership.
- `crates/host-runtime/src/generation.rs:600-616` stages, promotes, and then
  replaces the profile. This is host payload publication, not `search.sqlite`.
- `crates/host-runtime/src/generation.rs:848-911` protects selected and
  caller-supplied digests during pruning.
- `crates/daemon/src/bin/eidnara-host.rs:964-993` calls the generation store
  in the supplied-payload CLI path. The old catalog's no-caller rationale is
  stale; this does not make search replacement implemented.
- `crates/kernel/src/outbox.rs:133-156` prevents pending-consumer removal.
  `crates/kernel/src/cas/deletion.rs:1002-1017` records deletion consumers.
- `crates/retrieval` and its search selector are absent. This new record is
  `test-only` for that production absence, not for the host store's reachability.

## Failure scenario

A candidate database has valid files and schema but contains only some export
pages, or lacks a later deletion commit. Selecting it can pass a generic file
validation test while serving incomplete coverage. A schema-valid old database
can also be incompatible with the active tokenizer, model, policy, or identity.

An interrupted selector switch must not combine rows from old state with the
new checkpoint. Releasing the old consumer before a durable verified selection
can permit loss of history while the new candidate is still unusable.

## Timing windows and dependencies

Distinguish stage completion, local durable catch-up through T, coverage
verification, selector durability, reader acquisition, and old-consumer release.
A SQLite publication unit must include all state needed to reopen it, not an
unexamined assumption that copying its main file includes pending WAL state.
This file does not prescribe that unit or import a new filesystem protocol.
Generic directory identity remains an existing property to reuse.

## What a test must construct

1. Give old/new projections different rows, checkpoint, and compatibility tuple.
2. Read during switching and require one coherent selected generation per read.
3. Interrupt at every declared stage/select boundary, then reopen independently.
4. Compare the selected candidate to `O(T)`, not its own manifest claim alone.
5. Observe that the old consumer is released only after durable verification.
6. Make the prior projection incompatible; require explicit unavailability if
   the new one is incomplete, not an unconditional old-state fallback.
7. Record `search_projection_replacement_switch_interrupted` independently of selection success.

## Investigation log

### Q: Does an existing catalog already own the generic filesystem invariant?

- Sources examined: Host-runtime catalog and generation source/tests above.
- Findings: `current-profile-never-names-an-unvalidatable-generation` owns
  generic selector validity; `validation-and-enumeration-address-one-directory-object`
  owns descriptor identity. Their exact claims are linked, not duplicated.
- Missing evidence: Search-specific completeness and five-identity comparison.
- Conclusion: Resolved with answer: this record keeps only the search delta.

### Q: What is the durable SQLite publication unit after an ambiguous error?

- Sources examined: P U5, I shared lifecycle design, host generation precedent.
- Findings: The plan requires atomic selection; no search publication unit or
  selector error-reconciliation path exists in production.
- Missing evidence: Journal/connection closure contract and durable selector witness.
- Conclusion: Needs human input from the publication owner.

### Q: When may the replacement release the old consumer's obligations?

- Sources examined: Kernel deregistration and barrier capture/completion.
- Findings: A later registered consumer does not automatically replace the
  consumer named in an older barrier; a moving tip can reject deregistration.
- Missing evidence: Cutover target and safe old-consumer completion protocol.
- Conclusion: Needs human input; early release is not justified by selection intent.
