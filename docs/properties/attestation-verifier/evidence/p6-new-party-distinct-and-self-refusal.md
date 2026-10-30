# p6-new-party-distinct-and-self-refusal

## Discovery trigger

The specification's Property Catalog ([#1169](https://github.com/ahrav/eidnara/issues/1169)), surface N6. An obligation with two halves: the `self_source` half, checkable since [#1171](https://github.com/ahrav/eidnara/issues/1171), and the plugin-route half with the model-client clause, which [#1172](https://github.com/ahrav/eidnara/issues/1172) converts.

## Evidence trail

`self_source` half: `class_of` (`crates/daemon/src/attestation_verifier/classify.rs:305`) reads the registry row, decision row, and latest own admission through the standing read (`:315`), returns `NotInput` for a decision not live or whose latest own admission is not `Active` (`:325`), and then returns `Unverifiable::SelfSource` when `source_kind` is `VERIFIER_SOURCE_KIND` (`:331`), before the anchor read (`:338`) and the citation reads (`:388`). A `verifier` decision marked `Stale` is outside the input set. The test `a_verifier_decision_is_self_source_before_any_anchor_or_citation_read` (`crates/daemon/tests/attestation_verifier_classify.rs:590`) writes a `verifier` decision and two assistant decisions that cite one document, corrupts the `verifier` decision's anchor and one assistant decision's anchor so `validate_anchor_row` refuses them, and overwrites the cited descriptor's payload so the descriptor read refuses it. The assistant decisions fail with `CorruptCanonicalRow`, one at the anchor read and one at the citation read, as positive controls that both reads would fail; the `verifier` decision classifies `self_source`; and a second `verifier` decision marked `Stale` classifies `NotInput`.

Plugin-route half: `derive_classes` (`crates/daemon/src/kernel_routes/commit.rs:370`) has no `verifier` entry, and `resolve_classes` (`:413`) answers `invalid_input` for a `source_kind` outside the table before any transaction runs. `a_plugin_route_cannot_write_as_the_verifier_or_assert_trusted_local_code` (`crates/daemon/tests/kernel_routes.rs:6778`) submits a decision, an observation, and a `mark_stale` under `source_kind = "verifier"`, each refused as `invalid_input`, and asserts `trusted_local_code`, `current_code`, and `current_config` under `assistant`, each refused as `class_over_declared`; the write counts and the tip are unchanged. No other daemon route has a source-kind table: `kernel.artifact.ingest` takes a free `source_kind` for an evidence object (`crates/daemon/src/kernel_routes/ingest.rs:142`), which cannot be a decision, and `validate_trigger` admits only an `observation` object with an `observations` row as a trigger.

Write-set half: `attest` (`crates/daemon/src/attestation_verifier/attest.rs:119`) returns `Skipped(NotVerifiable)` before any commit for a classification other than `Verifiable`. `a_match_commits_the_attestation_with_every_companion_row` (`crates/daemon/tests/attestation_verifier_attest.rs:429`) classifies the committed attestation `self_source` and calls `attest` on it with the tip unchanged.

Model-client clause: `no_model_client_is_reachable_from_the_verifier_module` (`crates/daemon/tests/attestation_verifier_attest.rs:1244`) lists `src/attestation_verifier.rs` and every `.rs` file under `src/attestation_verifier/` at run time and scans each: every `crate::` path must start with an allowlisted model-free module or function (`confined_path`, `git_reconcile::identity_field`, `git_sources`, `project_docs`, `memory_capture::cite::digest_bytes`, `now_ms`), and no model-client module or network crate name (`memory_reviewer`, `memory_classifier`, `history_summarizer`, `embedding_dispatch`, `embedding_supervisor`, `hyper`, `reqwest`, `tokio`, `bedrock`) may appear. A positive control shows the scanner flags a source naming one, and every allowlisted path is used, so the allowlist is not vacuous. The scan checks direct references; the modules on the allowlist import no model client.

## Failure scenario

The verifier attests its own attestation: a `verifier` decision classified verifiable would let a later pass write an attestation of an attestation.

## Timing windows and dependencies

None for the `self_source` half: the check reads one registry column from the standing read.

## What a test must construct

A store holding a `verifier` decision whose anchor and cited descriptor the reads refuse, and a `verifier` decision marked `Stale`; a plugin write under `verifier`; a committed attestation passed to `attest`.

## Investigation log

### Q: Do the cited references exist at the branch tip under the cited names?

- Sources examined: every `path:line` in the record, read at the #1172 branch tip.
- Findings: Each cited line is the first line of the named test, function, or constant, or the named line inside it.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Does anything beyond the plugin route reserve `verifier`?

- Sources examined: OQ6-15 as resolved in the #1172 ticket; every `source_kind` field the daemon's routes deserialize (`crates/daemon/src/kernel_routes/commit.rs`, `crates/daemon/src/kernel_routes/ingest.rs`).
- Findings: The plugin route's closed table is the reservation; no other route has a source-kind table. A plugin can still `mark_stale` or retire an attestation under its own source kind (gap G4), which the ticket documents in #1174.
- Missing evidence: none.
- Conclusion: resolved with answer.
