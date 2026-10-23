# sc-unprovenanced-text-stores-sensitive-or-refuses-secrets

## Discovery trigger

Part A record on disclosure class at ingest. R13 requires captured text
without provenance to store at least `Sensitive`, and exact retention never
rewrites bytes. Re-verified at HEAD `5a842d998`; the batch path was read
because production capture uses it.

## Evidence trail

- `crates/kernel/src/cas/ingest.rs` `PreparedArtifact::new`: with
  `PayloadFidelity::Exact`, UTF-8 text with detections returns
  `ExactBytesRewritten`; non-UTF-8 returns `UnsupportedShape`. The class is
  `Secret` for detections, the asserted class raised to `Sensitive` for
  uninspected bytes, `Sensitive` for asserted `Normal` without affirmative
  provenance (non-empty `repository_id` and `revision`), and otherwise the
  asserted class.
- `KernelStore::ingest_exact_artifact` calls the same path. In
  `ingest_exact_artifacts_with`, a `PreparedArtifact::new` error maps to
  `KernelError::InvalidInput` before the commit closure.
- `crates/daemon/src/harness_sources.rs` `evidence_request` sets
  `provenance: crate::git_sources::commit_provenance(unit.unit)`, which is
  `Some` only for `GitCommits`. `stored_sensitivity` reads the stored class,
  and `apply` writes it to the descriptor. `publish_batch` reads the class
  from `envelope.object_state`.
- Capture publisher: `CaptureJob::publish` asserts `Sensitivity::Sensitive`,
  so captured units store `Sensitive` from the assertion.
- Tests read at HEAD:
  - `crates/kernel/tests/kernel_exact_artifacts.rs`
    `a_secret_is_refused_before_persistence_with_a_reason_that_names_no_content`:
    four canaries refuse `ExactBytesRewritten` with a fixed message.
  - `oversized_and_non_utf8_payloads_are_refused_before_materialization`:
    three non-UTF-8 payloads refuse `UnsupportedShape` with nothing staged.
  - `crates/kernel/tests/kernel_cas.rs` `unproven_normal_and_non_utf8_are_clamped_sensitive`:
    redacting lane; Remote denied `SensitiveRemote`, Local allowed.
  - `crates/daemon/tests/harness_sources.rs` `refused_bytes_and_identities_retain_nothing`:
    a tool output `token={SECRET}` refuses `Artifact(ExactBytesRewritten)` with
    the tip and inventory unchanged, and the refusal names no content.
  - `mixed_sessions_create_pending_only_for_message_text`: with the `Normal`
    corpus publisher, every row's sensitivity and its evidence object's are
    both `Sensitive`.
  - `crates/daemon/src/source_capture_tests.rs`
    `an_opencode_window_publishes_its_native_sources_once_and_replays_after_memo_loss`:
    the captured tool row is `Sensitive`.

## Failure scenario

A publisher passes repository provenance for working-tree or harness text, or
the clamp is skipped, so a row stores `Normal` and becomes remotely eligible.
Or the exact lane stores redacted bytes. Falsifier: an unprovenanced `Normal`
row, or a descriptor whose sensitivity differs from its evidence object's.

## Timing windows and dependencies

None for the classification. The batch path shares one artifact transaction
across admitted units, so an artifact refusal is batch-wide.

## What a test must construct

Exists: exact secret and non-UTF-8 refusals; unprovenanced `Normal` stored
`Sensitive`; descriptor equals evidence class. Missing: a capture-path batch
with one secret-bearing tool output and one clean unit, asserting what
publishes and what the memo records on the next pass.

## Investigation log

### Q: Does a secret-bearing unit refuse only itself in the batch path?
- Sources examined: `publish_screened` (no artifact screening per unit),
  `publish_batch`, `ingest_exact_artifacts_with`, `CaptureJob::publish_locked`.
- Findings: `PreparedArtifact::new` fails before the commit closure, so
  `BatchRefusal.unit` is `None` and the error is `InvalidInput`.
  `publish_locked` records every keyed unit of the batch as pending, counts
  `kernel_error`, and the next pass offers the pending siblings ahead of
  unknown units, where the same unit fails the batch again. Read, not
  executed. Nothing is retained, so
  the safety half holds; sibling liveness is the concern.
- Missing evidence: a test.
- Conclusion: unresolved, needs a capture-path test with a secret-bearing tool
  output beside a clean unit.

### Q: Is the non-UTF-8 arm reachable from the publisher?
- Findings: `SourceUnit.text` is `String`, so the publisher offers only UTF-8.
  The arm is reached at the kernel boundary only.
- Conclusion: resolved with answer.

### Q: Do baseline citations hold?
- Findings: baseline `cas/ingest.rs:141-163, 184-186, 197-207` are inside
  `PreparedArtifact::new`; `hs-tests:1459` is
  `refused_bytes_and_identities_retain_nothing`; `hs-tests:1614-1628` is in
  `mixed_sessions_create_pending_only_for_message_text`. The record also cites
  `kernel_exact_artifacts.rs`, which the baseline omitted.
- Conclusion: resolved with answer.
