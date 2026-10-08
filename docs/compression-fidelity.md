# Compression fidelity evaluation

This document describes the boundary of the compression fidelity evaluation
as it exists today. The contract is the
[Compression Fidelity Contract](https://github.com/ahrav/eidnara/issues/707);
its milestones and tickets live in the issue tracker. The property catalog is
[`docs/properties/compression-fidelity/`](properties/compression-fidelity/README.md).

## What exists

### The corpus

`crates/daemon/testdata/compression-fidelity.json` is the oracle. It holds six
synthetic cases, C1 to C6, one per acceptance case in the contract:

| Case | Subject |
| --- | --- |
| C1 | A rejected design returns |
| C2 | An unconfirmed deployment, paired with an otherwise identical source that has a success receipt |
| C3 | A task-scoped constraint mentioned once, with an independently eligible project memory example |
| C4 | Repeated misleading recaps, with an auto-hint truncation scenario |
| C5 | A final state that differs from the plan, with an observed error and an inferred cause |
| C6 | Exact source bytes with a same-length successor revision |

Each case carries:

- **Sources.** Native OpenCode `MessageV2` records for one session each,
  restricted to `info.{id, sessionID, role, time}` and text or settled tool
  parts. A span addresses a block by its message ID and part index; the
  OpenCode source adapter identifies a text block the same way and a tool
  output by its parent message, call ID, result revision, and output block 0.
  A block's revision is the decimal millisecond timestamp the adapter
  publishes: `time.completed` for assistant text, `time.created` for user
  text, and `state.time.end` for a tool output. C6 adds a successor record
  that keeps the message ID and part layout, completes later, and changes
  text without changing its byte length.
- **Approved example.** The authored producer output for each source, pending
  two-person corpus approval: one `<history_segment>` over the whole source
  with P1 to P3 bodies and an empty P4 whose title carries the capsule.
  Scripted runs return it; it stays separate from candidate outputs and
  observed results. A witness that needs a prior publication, such as the m1
  window, authors that baseline itself; the baseline earns no fidelity
  credit and stays out of the corpus.
- **Obligations.** Material decisions, constraints, status, uncertainty,
  rationale, and evidence, each with a statement written in evaluator voice
  and either native evidence spans or a memory example. A span names its
  source, message, block, revision, UTF-8 byte range (end-exclusive), and
  the quoted bytes.
- **Forbidden conclusions** classed `false-authoritative` (a false state
  transition or authority) or `recall` (lost material meaning), and
  **allowed losses** marked nonmaterial with the native detail they permit
  losing.
- **Follow-ups and scenarios.** Each scenario names a source, a follow-up
  prompt, the serving path (`natural`, `pressure`, `omission`,
  `hint_truncated`, `memory_admitted`, `memory_excluded`, or `exact_read`),
  the served tier and stage, and per obligation the dispositions it accepts
  (`visible`, `discoverable`, `unavailable`). `abstention` states whether a
  safe abstention is acceptable. Every case has natural P1 at m1, P2 to P4
  at m0, and a pressure or omission scenario.

Materiality and dispositions are authored before any candidate output
exists. Candidate outputs, observed baseline results, and review verdicts
never enter the corpus. Corpus approvals live outside the file, in the
tracker, and name the corpus SHA-256 they cover.

### Identity and validation

The corpus identity is the SHA-256 of the complete file bytes. Two pins hold
it, and both must move together when the corpus is re-authored:

- `CORPUS_SHA256` in `crates/daemon/src/compression_fidelity_corpus.rs`, a
  child of the daemon's library test module. It compares the digest of the
  bytes compiled in with `include_bytes!`. The module depends only on
  `aho-corasick`, `serde`, `serde_json`, and `sha2`, so an integration test
  can include it by path.
- `COMPRESSION_FIDELITY_CORPUS_SHA256` in
  `packages/e2e-tests/src/compression-fidelity/corpus.ts`. Its test also
  reads the Rust pin and requires equality.

Missing, unreadable, or mismatched bytes reject the run in both languages.
No normalization applies, so whitespace-only, same-length, and
scenario-only edits are rejected like any other edit.

The Rust corpus module owns source, span, and revision validation, and
`compression_fidelity_tests.rs` holds its negative controls. It rejects
duplicate or malformed IDs, spans that do not resolve to the exact bytes of
their native block at the named revision, successors that change identity
or byte length, successors that give one block two byte strings at one
revision, missing natural P1 at m1, P2 to P4 at m0, or pressure/omission
coverage, unexercised obligations, and any answer key or evaluator label in
a field that can reach provider input: native text and tool fields,
follow-up prompts, memory example text, and the approved example. Evaluator
labels include the corpus note, case titles, and incident references. Native
records decode through a closed schema, so an extra field on a message or
part fails to decode.

The same test file holds two further checks. Each approved example must
author its `<p1>` to `<p3>` bodies and a `<p4 />` capsule, because the
history parser fills a missing P2 or P3 from a denser tier. Every native
record also runs through the production adapter
`harness_sources::opencode_units`, and each block's revision and bytes must
equal the corpus module's own resolution.

The TypeScript reader `readCompressionFidelityCorpus` exposes only case,
source, scenario, follow-up, obligation, and forbidden-conclusion IDs,
follow-up prompts, reviewed outputs, each scenario's serving path, tier,
and stage, accepted dispositions, and abstention. It exposes no native
text, spans, obligation statements, or forbidden conclusion statements.

## What is unsupported

- **Consumer exact expansion.** No registered tool returns native source
  bytes. `eidnara_search` searches project memory; it can surface a decision
  or rationale but not exact transcript bytes. C6's exact read is an internal
  daemon witness, and the consumer disposition for exact bytes is
  `unavailable` in every C6 scenario.
- **Pi fidelity.** The Pi plugin folds history through the window protocol,
  but the corpus is OpenCode-native and no fidelity scenario replays Pi. The
  evaluation makes no Pi P1 to P5 claim.
- **Record-and-forward provider mode.** The e2e Messages mock serves scripted
  responses, and its cassette modes record or replay those scripted
  exchanges. No mode forwards a captured request to a real provider.
- **Replay, delivery, and semantic review.** No test yet drives the approved
  examples through the producer, publication, and serving, captures an
  OpenCode invocation, or records human semantic judgments.

## Planned `eval:compression-fidelity` command

This command is planned, not delivered. It will be a script in
`packages/e2e-tests` with this contract:

- **Inputs.** The corpus path, verified against the pinned digest; baseline
  and candidate prompt identities; a mode, either scripted or forwarding,
  with scripted as the default; for forwarding, one explicitly selected
  HTTPS provider and model with fixed call count, maximum output, timeout,
  and total spend limits covering the whole tool loop.
- **Outputs.** A private output directory outside the repository, written
  through the existing atomic JSON publisher with restrictive permissions.
  The run manifest records the repository revision, the corpus digest, the
  prompt hashes, the model, provider, version, and settings, and the limits
  once. Each case and scenario row references the owner observations it was
  derived from and keeps execution, deterministic result, preservation,
  recovery, consumer safety, semantic review, and cost as separate columns.
- **Limits.** Scripted mode makes no outbound provider call, including when
  scripts exhaust or setup fails. Forwarding stops at its first exhausted
  limit and treats an ambiguous send as spent.
- **Review prerequisites.** Two people approve the corpus before any
  candidate output is inspected, and the first semantic baseline before it
  is accepted. Without an authorized provider or reviewers, semantic
  evidence stays unverified.
