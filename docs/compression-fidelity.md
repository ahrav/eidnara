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
  `serde`, `serde_json`, and `sha2`, so an integration test can include it by
  path.
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
or byte length, missing P1 to P4 or pressure/omission coverage, unexercised
obligations, and any answer key or evaluator label in a field that can reach
provider input: native text and tool fields, follow-up prompts, memory
example text, and the approved example. Native records decode through a
closed schema, so an extra field on a message or part fails to decode.

The TypeScript reader `readCompressionFidelityCorpus` exposes only case,
source, scenario, follow-up, obligation, and forbidden-conclusion IDs,
follow-up prompts, reviewed outputs, each scenario's serving path, tier,
and stage, accepted dispositions, and abstention. It exposes no native
text, spans, obligation statements, or forbidden conclusion statements.

### Replay and exact evidence

`crates/daemon/src/compression_fidelity_replay_tests.rs`, another child of the
daemon's library test module, drives every source's approved example through
real prompt and alias assembly, a scripted producer, real validation, and
accepted publication. It records the producer's complete system, user, and
model input per attempt, and the exposure of every material span there:
exact, transformed by the presenter, or absent, with omitted tool outputs
flagged and a constructed oversized lead showing absence after truncation. It then observes covered input leaving the live
messages, P1 in the m1 window after a test-authored baseline, P1 at m0,
P2 to P4 and P5 omission under natural decay against test-authored newer
rows, and positive-budget pressure with a generous-budget control. Controls
cover a rejected primary with fallback, inherited P2 and P3, a healed tier
close, a discarded final segment, a rejected fact set, a same-length drift,
legacy and tier-sparse rows, and nonpositive budgets and disabled models.
Expected tiers are the approved bodies; the renderer is only observed.

The C6 witness in `crates/daemon/tests/harness_sources.rs` publishes C6's
records through the OpenCode source adapter, publishes the same-length
successor, reopens the kernel, selects the original descriptor at the saved
commit sequence by its complete identity and revision, and reads its exact
bytes through the guarded artifact reader. Its controls are the successor's
bytes, an equal-text other occurrence, a normalized substitute, an empty
block, and deleted evidence, which refuses rather than substituting.

Both write owner-attributed JSON observations through
`crates/daemon/src/compression_fidelity_observation.rs`: one record per case,
scenario or source, and stage, renamed into place from a temporary file with
mode `0600` inside a directory created with mode `0700`, and only when
`EIDNARA_FIDELITY_OBSERVATIONS_DIR` names that directory. A default run
asserts in memory and writes nothing. The wrapped slice's 105% retry is
recorded as unobserved; the history body and the wrapped slice are measured
separately under one named estimator.

### Fixture qualification

The direct-host fixture example (`crates/daemon/examples/direct_host_fixture.rs`
with `direct_host_fixture/case_script.rs`) includes the corpus module by path
and resolves scenario IDs through the same digest check. Its `script-cases`
control queues up to eight scenario IDs or `filler` entries. Each later
admitted summarizer request consumes one entry: a scenario binds its source's approved
example to the ordinals that request presents, covering any earlier presented
records with one fixture-authored lead-in segment and leaving later ones to
`<unprocessed_from>`. A request that does not present the source's messages
as one contiguous run, or that arrives after the queue is empty, fails typed
with no default text. `script-status` reports each delivered binding, and
`script-source` returns a scenario source's native records for the harness
to seed into OpenCode, with leak probes taken from its text blocks and tool
outputs.

`packages/e2e-tests/tests/compression-fidelity-qualification.test.ts` runs
ungated in the manifest-selected Rust lane. It seeds C1's records into a real
OpenCode session, drives the session until the bound case publishes, and
judges the next provider request for that session and follow-up: an applied
recipe served from the transform on the pass line, the reviewed P1 body in the
served history, and covered native text only inside the `<session-history>`
wrapper. A missing capture, an empty capture, a raw pass-through, or a leak
is a refusal, not a pass.

### Delivery campaign

The fixture's queue also takes `filler:N`, which answers one request with
exactly `N` compact fixture-authored rows, `echo`, which answers with the
fixture's default segments whose bodies repeat the presented text, and a
scenario ID with an `@p1-only` suffix, which serves the approved example
without its P2 and P3 bodies. `script-status` counts filler and echo answers
as `filled`.

`packages/e2e-tests/tests/compression-fidelity-delivery.test.ts` drives each
source through `src/compression-fidelity/campaign.ts` in one OpenCode session
per source:

- A baseline of fixture-authored rows publishes first, then the scripted case.
  The next provider request serves the case's P1 body in the m1 window
  (`<session-history-since>`), a second request repeats those bytes warm, and
  a restart with a changed prompt surface rematerializes the same body into
  m0 on a `HARD` pass.
- Natural decay adds counted fixture rows after the case under a 562-token
  history budget and serves each P2, P3, P4, and P5 scenario row. Each served
  tier must equal the tier `src/compression-fidelity/decay-oracle.ts` computes
  from the row count, the importances, and the budget the pass line reports,
  so a guard demotion or a retry fails the row. P4 rows serve the title-only
  heading, and P5 rows serve no segment text.
- C1.S5 and C3.S5 publish newer rows with large bodies under a 225-token
  budget. The served tier must be sparser than the curve tier at that budget,
  which the curve alone would still render. The served tier is recorded
  beside the corpus tier.
- A `C1.S2@p1-only` publication serves its P1 body at a P2 curve position,
  the parser's fallback.
- The first m1 request pins the capability surface
  (`src/compression-fidelity/capabilities.ts`): the four Eidnara tools,
  `eidnara_search` sources `["memory"]`, no exact-expansion tool, and
  `PI_TRANSFORM_AVAILABLE`. Its unit test shows an added expansion tool, a
  widened source enum, a missing tool, and a disabled Pi transform each
  report drift.

Every row is judged as in the qualification test: an applied transform pass,
a reviewed body, and leak probes only inside the `<session-history>` and
`<session-history-since>` wrappers. The OpenCode pass line now records the
final-array admission branch (`fits`, `shrinks`, `limit_unknown`, or
`declined`), the candidate's canonical bytes and heuristic charged tokens, and
the history budget the request carried; the rows record them with the
estimator name.

`packages/e2e-tests/tests/compression-fidelity-pi.test.ts` drives C1 through
the Pi plugin's `context` handler against the direct-host fixture: a seeded
baseline, the scripted publication, P1 in m1, and P1 in m0 after a restart.
Pi carries C1's tool part as assistant text.

## What is unsupported

- **Consumer exact expansion.** No registered tool returns native source
  bytes. `eidnara_search` searches project memory; it can surface a decision
  or rationale but not exact transcript bytes. C6's exact read is an internal
  daemon witness, and the consumer disposition for exact bytes is
  `unavailable` in every C6 scenario.
- **Pi decay tiers.** The Pi delivery test covers C1 at P1 in m1 and m0. No
  Pi row covers P2 to P5, and Pi carries corpus tool parts as text.
- **Record-and-forward provider mode.** The e2e Messages mock serves scripted
  responses, and its cassette modes record or replay those scripted
  exchanges. No mode forwards a captured request to a real provider.
- **Memory, hint, and recovery rows.** The admitted-memory and exclusion
  pairs (C3.S6, C3.S7), the hint rows (C4.S6), and the `eidnara_search`
  recovery loop are not yet driven through a provider request.
- **Legacy rows and invalid recipes.** The campaign cannot store a legacy row
  through the producer, and it constructs no invalid recipe; the U2 replay
  and the plugin's own tests cover both. The delivery judge refuses a raw
  pass-through.
- **Semantic review.** No human semantic judgment exists for any row.

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
