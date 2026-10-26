# Review policy

Review for correctness, security, simplicity, and coherence, in that order.
Prefer the smallest change that preserves the contract, not the fewest lines.
Stay read-only. Review the diff at the exact base and head; for uncommitted local
changes, record the staged and worktree content identity. Read callers,
contracts, and tests at those states to verify impact.
Do not turn a PR review into a repository-wide redesign.

## Project contracts

- Read root `AGENTS.md` and scoped `AGENTS.md` files for affected directories.
- `docs/host-wire-protocol.md` is the normative host wire contract. The wire has
  one contract, and a change to wire names, literals, or shapes lands in one PR
  with the document and every Rust and TypeScript peer. Check both peers.
- Preserve single-baseline storage. Reject schema mismatches; do not propose
  migrations, version ledgers, or compatibility layers.
- Use `.github/workflows/ci.yml` for required checks and supported configurations.
  Use `--locked` for Cargo commands except deliberate dependency-update checks.
  Do not claim an unrun or skipped check passed. When a change adds or edits a
  shared fixture or contract input, confirm the `changed paths` filter runs every
  consumer's required job. Run the `gates` job steps from
  `.github/workflows/ci.yml` locally before opening a PR. The `:ci` validator
  entry points read GitHub event data. Locally, with `<sha>` the merge base of
  the head and the PR's target branch and `<dir>` holding
  `git archive <sha> packages/e2e-tests/incidents` extracted from the repository
  root, run `bun scripts/validate-incident-verifiers.ts --base <sha>` and
  `bun scripts/validate-incident-history.ts --accepted <dir> --base <sha>` in
  `packages/e2e-tests`; the package scripts compare against `main` or only the
  working tree.

## Review priorities

- **Correctness:** Derive the required behavior from the ticket and the production
  consumers before comparing with the old code or a dependency. Trace changed
  behavior from the shipped entry point through configuration, producers,
  consumers, and failure paths, and name one witness that uses the production
  constructor or adapter at its supported runtime version. Check state
  transitions, partial writes, recovery, retries, cancellation, shutdown, and
  bounded resource use. A bound is enforced before the work it limits and covers
  traversal, representation growth, fallback, waits, and draining work through
  physical release. Show the termination condition for input-controlled
  traversal. A value read before a blocking point or in another transaction is
  rechecked or fenced where it is acted on. A timeout does not prove rollback.
- **Peers:** When the change reimplements, replaces, or fixes behavior that
  another component also implements, such as a dependency, another crate, or the
  TypeScript side, compare the two predicate by predicate. Report each other copy
  of a fixed defect as fixed, unaffected with the reason, or a finding.
- **Security:** Check runtime validation at trust boundaries, credential handling,
  file ownership and symlink safety, unsafe Rust, FFI, and shared-memory lifetimes.
  Host bearer-key possession grants authority; caller identity fields do not.
  TypeScript annotations and casts do not validate runtime input. Validate
  decoded field values before escaping or serialization changes them, and apply
  each check on every entry path into the guarded sink, including recovery,
  replay, and fast paths. Name the facts frozen at admission and the facts
  revalidated at use.
- **Simplicity:** Look for existing code, standard-library functions, platform
  features, and installed dependencies before proposing new machinery. Challenge
  dead code, duplicated logic, speculative configuration, and pass-through layers.
  Name what can be removed and what replaces it. Preserve validation, cleanup,
  safety checks, supported behavior, and useful regression tests.
- **Coherence:** Keep invariants and mutable state with a clear owner. Check
  dependency direction, module responsibilities, naming, and public contracts.
  An abstraction should protect a present invariant, lifecycle, boundary, or
  required variation. One implementation alone is not grounds for deletion.
  Prefer local changes; extracting helpers or moving files is not inherently simpler.

## Tests and evidence

- Ask which plausible wrong implementation each changed test would reject.
  For each added branch, limit, or marker, name the edit that makes the test
  fail: delete the check, invert the condition, or drop the recorded call. A test
  that passes either way is not evidence; a second limit with the same error that
  trips first for the fixture's sizes is the common cause.
- Verify setup reaches the claimed behavior and assertions inspect the result,
  relevant state, and forbidden side effects. Record the reached branch, the
  enabling state, and an oracle independent of the code under test. For claimed
  absent effects, require a positive control showing the observer can detect
  them. Preserve per-identity associations and exact state tokens; an aggregate
  comparison does not prove a per-item claim.
- Confirm new tests are collected by the CI job and feature set that the claim
  depends on.
- Changed property, contract, or test-evidence claims in Markdown are review
  scope. After the last commit and after each merge of `main`, re-resolve every
  `path:line` and `path#Lnnn` anchor that points into a changed file.
- Request the smallest test that distinguishes the bug from correct behavior.
  Do not demand a framework, one test per input, or coverage for its own sake.
- Suggest test deletion only when a named surviving test covers the same inputs,
  invariant, failure modes, and supported configurations. Consolidation must
  preserve every original case. Keep bug-referenced regressions, unique boundary
  cases, readable usage anchors, and the last ungated baseline.
- Honor scoped verification rules: ordinary tests do not replace Miri/Valgrind
  for changed shared-memory unsafe code, native-addon tests, or shipped-TUI smoke tests.

## Findings

Report actionable issues introduced or worsened by this change, ordered by impact.
For each, give the location, trigger or evidence, consequence, and smallest fix.
For simplifications, show the maintenance cost removed and behavior preserved.
Keep optional improvements separate from blockers. Do not inflate severity for style.
Skip cosmetic preferences, speculative future needs, and formatter/linter noise.
Verify findings before reporting; state missing evidence instead of inventing certainty.
No findings is a valid result. Keep the summary short and name verification gaps.

Give every prior actionable finding a disposition: fixed with evidence, refuted
with evidence, or unresolved with its consequence and owner. A refutation that
cites parity with a dependency, an existing validator, or documentation states
that check's predicate and names the test that exercises the triggering input
against it; without that test the finding stays unresolved. An unresolved
finding may remain at merge only when the merged code cannot stall progress,
report a false state, exceed a bound the PR claims, or weaken a trust boundary.
Any other unresolved correctness or security finding keeps the readiness verdict
incomplete. Bot findings posted after merge get a fix or a recorded decline.

These recur as false positives here; check them before reporting:

- In `docs/properties/`, `METHOD.md` allows a ` - <note>` after an enumerated
  field value; per-part `portfolio-evaluation.md` files and template `### Q:`
  headings are required; a `Failure scenario` states the failure under its
  trigger, and the record's evidence trail and `Status` say whether current code
  reaches it.
- On a stacked PR, code from the parent PR and intermediate shapes that never
  reached `main` are out of scope.
- A resource finding needs an untrusted producer or a producer bound above the
  consumer's enforced budget.
- A decision the ticket settled is out of scope unless the change breaks it or
  the decision violates a project contract or the Security priority.
- A new diagnostic `message` value is a wire change only when
  `docs/host-wire-protocol.md` states its exact text.
- A "no issues", rate-limit, or incomplete-run summary from a review bot is not
  review evidence.

## Sub-agents

The ticket's Pre-PR Gate names the review set for a change. Every PR that
changes a file other than Markdown, or Markdown contract, property, or
test-evidence claims, or this file or an `AGENTS.md`, records a gate, including
fixes outside the ticket flow.
This file sets no count. Always run `ponytail-review` and
`/code-quality:reduce-complexity`. Run `/testing:test-strategy` for Rust changes;
for other languages, run the testing-approach owner the router names. Run the
language code reviewer for every changed language, including ported or copied
code; a design review does not substitute for it.

Use `/research-planning:ask-skills` against the requirements source and the
actual diff to select the additional skills the change needs. The requirements
source is the ticket, or for a fix outside the ticket flow the issue report or
contract that states the required behavior. Classify the diff by the
mechanisms it changes, not only its language or ticket topic: SQL, persisted or
wire schemas, durable publication, cross-transaction fences, spawned work and
cancellation, limits, budgets and deadlines, untrusted input, credentials and
redaction, prompt assembly, clocks, test oracles, and Markdown claims. Changed
test-evidence claims select `invariant-test-review`, and changed Markdown
contract claims select `design-doc-audit` with explicit paths and
`--no-auto-fix`. An existing trust boundary whose authority or sensitivity
changes still needs its owner. Record each selected skill, its version or
content digest, scope, and selection reason, and each signal checked for a
priority area marked not applicable. When the host blocks nested router
invocation, apply the `ask-skills` contract in-context and record
`contract-applied; nested invocation unavailable`.

Run every selected review in separate, fresh-context, read-only sub-agents
against the same diff and base; run them concurrently, in waves at the host's
sub-agent cap when one applies. Give each reviewer this file, the scoped
`AGENTS.md` files, the requirements source, and the production consumers of the
changed code. At least one applicable reviewer derives the required behavior
from that source and the consumers before reading the author's explanation.
Each returns location, severity, evidence, and rationale; none posts comments or
edits code. The main reviewer consolidates and deduplicates findings, keeps the
contributing reviewers, and verifies each finding against the implementation
before reporting it. A fix listed as applied is verified present at the head.

Record each round in the PR body as `Round N reviewed <base>..<head>: <skills>`,
with commit SHAs. The recorded scopes together cover every changed line at the
recorded head. Before opening, marking ready, and merging, classify every
commit after the last recorded head, including merges of `main` that touch files
the PR changed, against each review's scope. Rerun each review a commit touches,
and record a new round for the new head that names the reruns or states
`no rerun - <reason>`. When a commit adds a language or mechanism outside the
recorded review set, rerun skill selection for the new head and run the newly
required reviews; a gate scoped to tests or docs no longer applies once
production files enter the diff. The skill that raised a finding reviews its
fix. After a rebase, compare the old and new ranges with `git range-diff` and
classify the commits that changed. When `main` has changed a crate or package
the PR touches, one a touched crate depends on, or one that depends on a touched
crate, merge or rebase and rerun CI before merge. A gate whose latest recorded
round names an older head than the PR head, or a base other than the merge base
of the PR head and its current target, is blocked.

Merge after the bot reviews requested on the head commit finish, or record that
they were unavailable. If a required skill or the concurrent-agent capability is
unavailable, report a blocked gate; other coverage is partial and does not
replace it. Never claim that an unavailable review ran.
