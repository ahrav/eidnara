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
a field that can reach provider input: native text, tool call IDs, names,
inputs, and outputs, follow-up prompts, memory example text, and the approved
example. Evaluator labels include the corpus note, case IDs and titles, every
entry ID and statement, and incident references. Native
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
scenario or source, and stage, linked into place without replacement from a
temporary file with mode `0600` inside a directory created with mode `0700`,
and only when `EIDNARA_FIDELITY_OBSERVATIONS_DIR` names that directory. A
default run asserts in memory and writes nothing. The wrapped slice's 105%
retry is recorded as unobserved; the history body and the wrapped slice are
measured separately under one named estimator.

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
at most `N` compact fixture-authored rows, `echo`, which answers with the
fixture's default segments whose bodies repeat the presented text, `echo:N`,
which repeats the presented text in at most `N` rows, and a
scenario ID with an `@p1-only` suffix, which serves the approved example
without its P2 and P3 bodies. `script-status` counts filler and echo answers
as `filled` and reports the importance each kind of fixture row carries.

`packages/e2e-tests/tests/compression-fidelity-delivery.test.ts` drives each
source through `src/compression-fidelity/campaign.ts` in one OpenCode session
per source. Each case owns its OpenCode, direct host, and mock provider, so
the cases run as concurrent tests up to the runner's `--max-concurrency`
(`test:rust` passes 6). Each turn waits until no summarizer firing is live,
so no pass commits while a firing publishes.

- A baseline of fixture-authored rows publishes first, then the scripted case.
  The next provider request serves the case's P1 body in the m1 window
  (`<session-history-since>`), a second request repeats those bytes warm, and
  a restart under the aging budget with a changed prompt surface
  rematerializes the same body into m0 on a `HARD` pass. Every source serves
  these three rows; their observations carry the source's m1 scenario when
  the corpus declares one and the source ID otherwise.
- Natural decay adds counted fixture rows after the case under a 562-token
  history budget and serves each P2, P3, P4, and P5 scenario row on a `HARD`
  pass that rebuilds m0 in the running OpenCode: the session's requests carry
  new system text, so the next pass sees a changed render config. Each served
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

`packages/e2e-tests/tests/compression-fidelity-memory.test.ts` drives the
memory, hint, and recovery rows:

- C3 ages its case row to P3, so the raw constraint is gone from history, and
  then observes C3.M1 across cold passes. `script-source` returns the memory
  example; the fixture's `memory-seed` control commits it as a
  repository-sourced verified memory into the project scope of an existing
  memory decision, and `memory-admission` records `code_observed`,
  `quarantine`, or `explicit_reject` on a decision. A pass earns memory
  credit only when the provider request's `<project-memory>` block carries
  the example's text, and each admitted pass's block omits every excluded
  decision present at that point. The candidate-only (an agent-created
  memory), the wrong-project (a verified copy in another directory's project), the
  budget-excluded (a verified copy whose body exceeds the 4000-token
  injection budget), the withheld (quarantined), and the rejected variants
  each serve the same P3 row with no memory credit. Two seeded copies show
  the admission is repeatable.
- From the budget-excluded pass, `eidnara_search` with the query `manifest cache
  resident`, every term taken from the visible request, and sources
  `["memory"]` returns the budget-excluded admitted copy, named by object id
  with the memory's text, in one call within 16 KiB, without
  the wrong-project copy's distinct suffix. Exact recovery stays
  `unavailable`.
- C4 ages its case row to P5, then publishes up to 24 distractor rows that
  share the follow-up's generic terms. With `memory.auto_search.enabled`, the
  follow-up carries an `<eidnara-search-hint>` whose first fragment is the
  case row. The fixture's `user-hint-outcome` trace shows the case row
  selected first while distractor rows also matched. A separate truncation
  row records the fragment, cut at both ends by the fragment limit, and
  whether it kept the source's qualifier. With
  auto-search off, the same request carries no hint and the host decides
  none.

### Opt-in real capture

Two paths capture real model output. Neither runs by default, and no default
test sends a request off the host.

**Real producer capture.** The ignored daemon test
`real_producer_capture_of_every_corpus_source` in
`crates/daemon/src/compression_fidelity_replay_tests.rs` folds every corpus
source once through the host's existing producer execution. It reads four
environment variables, all required:

| Variable | Meaning |
| --- | --- |
| `EIDNARA_FIDELITY_REAL_CONNECTION_FILE` | The connection file of a running host whose model execution can reach the model |
| `EIDNARA_FIDELITY_REAL_MODEL` | The model id the history summarizer runs, as the only model in its chain |
| `EIDNARA_FIDELITY_REAL_WAIT_SECONDS` | How long each source's firing may take to settle |
| `EIDNARA_FIDELITY_OBSERVATIONS_DIR` | A private directory outside the repository |

Run it with
`cargo +1.98 test -p daemon --lib --locked real_producer_capture -- --ignored`.
Each source yields one `daemon.compression_fidelity.real_capture` record,
written with mode `0600` in an owner-only `0700` directory that the test
creates or requires, outside the repository. Its terminal is `published`,
`validation_rejected` for a settled firing that drained model output but
published no rows, or `unsettled` for a firing that did not settle within
the wait, never started a producer, or drained no model output;
the test fails after writing every record when any source is unsettled. The
record names the model,
the output origin, whether the firing settled, the attempt count, and every
attempt's complete system prompt, user prompt, generation settings, start
error, and every drained output or error, followed by the published rows. The host's model
execution protocol reports no token usage, so each record carries
`usage: null` and `usage_reported: false`; its cost is unknown. The test
reads no credential: the host owns them.
`a_capture_records_the_model_attempts_usage_and_complete_input` runs the
same capture over the scripted producer in the default suite.

**Record-and-forward provider mode.** `new MockProvider({ forward })` in
`packages/e2e-tests/src/mock-provider/` forwards each request OpenCode sends
to one Messages endpoint and returns the provider's response to OpenCode.
`RustTestHarness.create({ forward })` builds it and runs OpenCode on
`forward.model` at `forward.contextLimit`, so the forwarded body already
names the selected model. Construction requires:

- an `https:` URL whose path ends in `/messages`, with no user info or query;
- the model, which every request must name, and its context limit;
- the reviewed corpus digest;
- input and output prices in USD per million tokens, with the input price
  at least the model's highest input-side price;
- four limits: `maxCalls`, `maxOutputTokens`, `timeoutMs`, and
  `spendCapUsd`;
- a credential callback.

Scripted responses and forwarding are exclusive: passing both, or scripting
a forwarding mock, throws. A forwarding mock accepts only requests carrying
its per-mock `inboundKey`, which the harness writes into OpenCode's provider
config, so no other local process can spend its budget. `forward.contextLimit`
is the context limit OpenCode is configured with. Before each send the
forwarder checks the model,
the request's `max_tokens`, the call count, and the spend cap. The spend
check reserves the body's byte length as input tokens plus `max_tokens` as
output. A response with usage charges its stated tokens; a response without
usage, or a send without a response, charges the reservation. A refused
send, a non-2xx response, a timeout, a redirect, a failed credential
callback, or a response whose usage costs more than its reservation stops
the run, and every later request is refused without a send. The input price
must cover any pricing the client's `anthropic-beta` header enables. The forwarder sends the received
bytes unchanged, adds the callback's headers to the outbound request only,
and records each exchange with redacted headers and the bounded response
bytes. `forwardingReport()` keeps attempted sends and acknowledged responses
apart, and marks the run incomplete on a stop, a send in flight, a truncated
capture, an unknown cost, an acknowledged response without a stop reason,
spend above the cap, or a tool call that no later request answers with its
`tool_result`. The limits, spend, and stop span the mock's life, across
`reset()`.
`publishForwardingReport` writes it with mode `0600` in an owner-only `0700`
directory outside the repository, refusing a shared existing directory or a
label that is not a plain file name. `tests/compression-fidelity-forwarding.test.ts`
runs the whole loop through OpenCode against an in-process provider double.

## What is unsupported

- **Consumer exact expansion.** No registered tool returns native source
  bytes. `eidnara_search` searches project memory; it can surface a decision
  or rationale but not exact transcript bytes. C6's exact read is an internal
  daemon witness, and the consumer disposition for exact bytes is
  `unavailable` in every C6 scenario.
- **Pi decay tiers.** The Pi delivery test covers C1 at P1 in m1 and m0. No
  Pi row covers P2 to P5, and Pi carries corpus tool parts as text.
- **Truncated-away memory.** A rendered memory line is capped at 64 KiB, far
  above the injection budget, so no admitted row can render with its
  decisive qualifier truncated away. The budget-excluded variant drops the
  whole row instead.
- **Agent-authored admission.** An `eidnara_memory` create stays a
  `candidate` with `explicit_labeled` visibility after `code_observed`, so
  the admitted rows use the fixture's repository-sourced seed.
- **Legacy rows and invalid recipes.** The campaign cannot store a legacy row
  through the producer, and it constructs no invalid recipe; the U2 replay
  and the plugin's own tests cover both. The delivery judge refuses a raw
  pass-through.
- **Semantic review.** No human semantic judgment exists for any row.

## `eval:compression-fidelity`

`bun run --cwd packages/e2e-tests eval:compression-fidelity` assembles one
baseline and one candidate evidence directory into a private manifest and a
per-scenario report (`scripts/eval-compression-fidelity.ts`,
`src/compression-fidelity/evaluation.ts`). It reads files and writes two;
it sends no request in either mode.

```
eval:compression-fidelity --baseline <dir> --candidate <dir>
  --out <private dir> [--corpus <path>] [--mode offline|live]
```

**Inputs.**

- `--corpus`: the corpus file, the committed one by default, read only when
  its bytes hash to the pinned digest.
- `--baseline` and `--candidate`: each an evidence directory. It holds the
  owner observations the witnesses wrote with
  `EIDNARA_FIDELITY_OBSERVATIONS_DIR` set:
  `daemon.compression_fidelity.replay`,
  `daemon.harness_sources.c6_exact_read`,
  `daemon.compression_fidelity.real_capture`, and `opencode-delivery`. It
  also holds any `forwarding-*.json` reports and an `arm.json`
  (`eidnara.compression-fidelity-arm/v1`). `arm.json` names the arm's label,
  the SHA-256 of its history summarizer system prompt, its model, provider,
  version, settings, and limits, and whether its generation origin is
  `scripted` or `real`.

**Outputs.** `manifest.json` and `report.json`, written with mode `0600` in
an owner-only `0700` directory outside the repository.

- **Manifest.** It records once:
  - the repository revision;
  - the corpus path and digest;
  - each arm's configuration;
  - every observation and forwarding report with its file SHA-256.
- **Report.** It holds, per arm:
  - identity errors;
  - reached and missing scenarios;
  - one row per corpus scenario with the columns below, where every column
    derives from that arm's observations;
  - the reasons acceptance is withheld.

  It then holds the comparison's refusals and whether the comparison is a
  treatment. It assembles no review, control, or cost evidence, so it
  accepts no arm.

| Column | Values and source |
| --- | --- |
| `execution` | `executed`, `failed`, or `missing`, with every observation's owner, stage, and terminal |
| `deterministic` | `pass`, `assertion_fail`, or `not_evaluated`: the served tier against the scenario's tier, or C6's exact read |

**Identity.** The assembler recomputes every file's SHA-256. It refuses:

- an observation bound to another corpus;
- an unknown owner;
- a case or scenario outside the corpus;
- a duplicate observation of one owner, case, source, scenario, and stage;
- a leftover temporary file, whether `.<name>.tmp` or `<name>.tmp-<hex>`;
- a system prompt the arm did not declare;
- forwarding exchange text that does not match its recorded hash. The
  hashed representation is the request body as UTF-8 bytes.

In an arm labeled `real`:
- scripted output is an identity error;
- every source needs a published real capture;
- every serving observation must name, in `detail.generation_capture_sha256`,
  the file hash of the published real capture of its source whose output it
  served.

A scenario label `<scenario>@<variant>` belongs to its scenario's case.
Observations of a variant and observations marked `detail.judge_control`,
which test the delivery judge itself, appear in the row's outcomes and judge
nothing.

**Comparison.**

- The comparison is refused when an arm is bound to another corpus or has
  identity errors, when the arms reached different scenario sets, or when
  they differ in model, provider, version, settings, or limits.
- Differing prompt hashes mark it a treatment comparison.

Missing scenarios block full acceptance, and the report names them.

**Modes.** `offline`, the default, assembles the directories as they are.
`live` additionally requires each arm to carry complete forwarding reports
from the record-and-forward provider mode whose limits equal the arm's
`limits`, so live evidence inherits that mode's limits.
