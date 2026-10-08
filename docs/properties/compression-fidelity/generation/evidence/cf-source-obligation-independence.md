# cf-source-obligation-independence

## Discovery trigger

Date: 2026-09-19. Inspected code HEAD:
`99f68bd37516ca5351f8aadb8b0b51cb0f13dcc8`.
The [settled contract][contract], "Requirements", "Materiality and minimum
meaning by tier", and "Implementation Decisions" KTD1/KTD2 (historical plan
lines 54-56,66-68,153-154), requires human material obligations, one corpus owner,
and private replay orchestration. It is a contract lead, not evidence that
those checks exist or pass. The catalog records the original plan's digest
and drift from `1555f00c`.

External scope is the plan and local code/docs/history. No additional incident
logs or related repositories are supplied. No test or external model runs here.

## Evidence trail

- The [settled contract][contract], "Six acceptance cases" and "Milestone
  boundaries and dependencies" U1 (historical plan lines 91-106,185-197),
  separates native sources and reviewed artifacts
  from generated output and defines C1-C6, forbidden transitions, and losses.
- [Implementation Decisions][contract] KTD1 (historical plan line 153) assigns
  source/span/revision validation to Rust and requires
  Rust and TypeScript to bind to the same corpus file-byte SHA-256. The
  evaluation part's [cf-corpus-byte-identity][corpus-owner] owns that validator;
  this record consumes its result rather than restating the hash check.
- [Evidence and review gate][contract] (historical plan lines 270-271)
  requires two initial human approvals and artifact-bound
  review. It does not authorize a runtime claim classifier.
- [citations_golden.rs:22-44,88-145][golden] contains native messages and
  independently listed expected identities/presented bytes. This is an existing
  deterministic fixture pattern, not the new semantic corpus.
- [memory_reviewer_corpus.rs:44-70][curator] separates source/script/expectation fields,
  but `judge` recognizes fixed substrings. Its implementation is not reusable
  as a general test of temporal state, polarity, scope, or evidence fidelity.
- [lib.rs:18252-18274][lib] provides the existing private test ancestry.
  At inspection the corpus and its module were absent; U1 (#718) adds
  `crates/daemon/testdata/compression-fidelity.json` and registers
  `compression_fidelity_corpus.rs` under that test module.

Reachability is `test-only`: this is the planned offline source-obligation
admission surface. Its private test boundary exists; its check does not.
Confidence concerns that placement and the inspected fixture patterns, not
successful source validation or human approval. Existing checks are unaudited.

## Failure scenario

A candidate drops a prohibition. The expected tier is regenerated from that
candidate, so the report succeeds without evaluating the original material
obligation even when both languages use identical corpus bytes.
Alternatively, answer-key text leaks into the prompt and tells the producer
which conclusion to emit. These are synthetic controls, not incidents.

A competing explanation is that changing expected wording merely accepts a
valid paraphrase. Artifact identity cannot decide that question. The source
obligation and recorded human judgment must distinguish permitted rewording
from changing the requirement itself.

## Timing windows and dependencies

Annotations and allowed losses precede candidate inspection. Consume the
identity owner's observation binding source validation, replay, and review.
A reviewed baseline update cannot discard a failure. Record separate
prompt hashes when prompt wording is the treatment; KTD1 does not require
baseline and candidate prompts to be identical. Do not add a separate blinding
gate beyond the plan's source-annotation independence requirement.

## What a test must construct

Construct valid C1-C6 source references and duplicate-ID, wrong-revision,
broken-span, candidate-derived-obligation, and answer-key-contamination
controls. Missing or failed identity-owner evidence must block admission, but
the generation check does not recompute that owner's digest comparison. Verify
provider input by allowed source/prompt origin, not by banning words that may
also occur legitimately in native source text. Machine checks validate this
linkage and recorded approval, not whether human annotations are true.
The future harness is missing; explicitly none found for these combined checks.

## Investigation log

### Q: Can existing fixtures replace independent human obligations?
- Sources examined: The golden and Curator corpus functions cited above.
- Findings: They provide deterministic patterns; the Curator classifier uses
  fixture-specific words and does not implement general semantic reasoning.
- Missing evidence: A reviewed C1-C6 corpus and an implemented admission check.
- Conclusion: Unresolved, needs the planned corpus; reuse patterns, not a judge.

### Q: Who approves which native revisions and material spans?
- Sources examined: Plan:66-68,91-106,270-271.
- Findings: Materiality depends on a named follow-up; two initial approvals
  are required. Code cannot choose that human judgment or its reviewers.
- Missing evidence: Named reviewers and approved source annotations.
- Conclusion: Needs human input.

[contract]: https://github.com/ahrav/eidnara/issues/707
[golden]: ../../../../../crates/daemon/src/history_summarizer_citations_golden.rs
[curator]: ../../../../../crates/daemon/tests/support/memory_reviewer_corpus.rs
[lib]: ../../../../../crates/daemon/src/lib.rs
[corpus-owner]: ../../evaluation/catalog.md#cf-corpus-byte-identity
