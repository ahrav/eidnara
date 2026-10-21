# Output certificate: proposed read side (N3, N4, N5)

Pass: invariant-driven domain modeling, new-behavior pass.
Plan: docs/plans/2026-10-21-1902-feat-authoritative-rules-automatic-use-plan.md
at HEAD 08de9051a735f136ff63a9dca49eaa11493b3006.
Every writer inventory below is proposed. Nothing in this pass is
implementation evidence; where a line says "verified" it names code that
exists at HEAD and that the design reuses, and the verification is of the
mechanism's current contract, never of the proposed behavior.

## SCOPE AND AUTHORITY

Deployable: the daemon process with its embedded Kernel store. It owns the
injection scope context, the required and optional packing phases, the m0
`<project-memory>` block, the `project_memory` response diagnostics, and the
`directive.*` route family's admission. The TypeScript harness plugins (Pi,
OpenCode) are callers at a caller-visible crossing: they supply transform
request fields (`cwd`, `is_subagent`, the window) and invoke the directive
command; they decide nothing about eligibility, order, or budget.

Owned semantic state: none new on the read side. The read side consumes
Kernel decision rows (authority: the Kernel's AutoInject visibility verdict
through `read_visible`, `crates/daemon/src/kernel_routes/read.rs:163-258`)
and produces derived state only: the per-pass scope context, the packed row
set, the omitted count, the rendered block, and the diagnostics.

Actors:
- the user, through a host command surface (`user:pi`, `user:opencode`);
- the harness plugin, supplying pass fields on every context event;
- the daemon pass, building the context and packing;
- the model, which has no path to any of the above except as the author of
  tool-call `input` JSON that the daemon parses for directory paths.

Caller-visible crossings: transform request (7.10.3; unknown fields ignored,
verified at docs/host-wire-protocol.md:951), transform response diagnostics
(7.10.5 `project_memory`), `directive.*` kernel-route methods (7.8, to be
added to the method list whose header currently counts eleven literals).

Routed decisions (not decided here): the admission policy and class pairs
(S1); storage representation of the omitted count and diagnostics (wire
enrichment pass); the directive route body shapes (N1 pass); the repository
rule writer (N2 pass).

Verified repository facts the read side relies on:
- `ScopeFilter::matches` answers `false` for a row without a scope id and
  otherwise defers to `ProjectScope::names_project`
  (`crates/daemon/src/kernel_routes/project.rs:181-208`).
- `names_project` requires a decodable scope carrying a Project term whose
  `scope_matches` outcome is `Matches`; `Uncertain` fails
  (`crates/kernel/src/eligibility.rs:85-94`).
- `scope_matches` is the conjunction of present terms; absent dimensions
  match; an absent context value for a present term is `Uncertain`
  (`crates/kernel/src/scope.rs:1392-1468`), pinned by
  `crates/kernel/tests/kernel_scope_algebra.rs:325`.
- `ScopeMatchContext` holds exactly one value per dimension
  (`scope.rs:869-873`); a set of directory ancestors cannot be expressed as
  one context, so Entity matching over ancestors is daemon iteration.
- `CanonicalScope` holds at most one term per dimension
  (`scope.rs:613-660`).
- `Dimension` is the closed ten-variant enum (`scope.rs:472-484`).
- `read_visible` prefilters by the Project scope term in SQL, judges every
  returned row with `ScopeFilter`, keeps the newest `MAX_READ_ROWS` (8192) by
  `created_commit_seq` desc then `object_id`, then cuts at
  `MAX_READ_ROW_BYTES` (`read.rs:26-29, 176-232`).
- `injectable_snapshot` keeps only rows whose AutoInject `visibility` is
  `Visible` and whose `decision_kind` is a positive category, then trims
  (`crates/daemon/src/canonical_memory.rs:183-211`).
- `trim_memories_to_budget` floors the f64 budget once, charges the wrapper
  first, and packs with `skip_and_continue` (`m0_compose.rs:74-111`).
- `CanonicalMemorySnapshot::new` digests `(category, rendered line)` of the
  positive rows in object-id order; `truncated` is not digested
  (`canonical_memory.rs:49-67`).
- `render_memory_block` orders by category rank then object id;
  `USER_DIRECTIVES` is a positive category at rank 5 (after `NAMING`)
  (`memory_render.rs:58-109, 157-208`).
- `admit_required` and `reserve_required` are all-or-nothing over the set
  they receive; `OverBudget` is an `Err` with nothing reserved
  (`crates/retrieval/src/packing/required.rs:114-205`).
- `skip_and_continue` admits what fits and records skips
  (`crates/retrieval/src/packing/scan.rs:18-43`).
- `is_subagent: true` suppresses the served m0 and m1 slots
  (`crates/daemon/src/transform.rs:11723-11733`); the additive path composes
  m0 on `PassPlan::Hard` (`transform.rs:3052`) from the same pinned read.
- One canonical read per pass feeds every memory surface
  (`crates/daemon/src/lib.rs:9338-9345`).
- Pi registers `createPiTransform` only when `!compactionOff` and the handler
  returns early when `!isCompactionEnabled(deps.config)`
  (`packages/pi-plugin/src/index.ts:382-383, 565-574`); Pi sends
  `is_subagent: false` (`pi-pass-inputs.ts:110`); the subagent child entry
  returns before any registration (`index.ts:332-337`).
- OpenCode sends `is_subagent` from host pass inputs
  (`transform-pass-fields.ts:66`); its command path has no dialog
  (`command-handler.ts:389`).
- Pinned Pi 0.80.2: `sendUserMessage` calls `prompt` with
  `expandPromptTemplates: false`, so it never dispatches an extension
  command; queued input naming an extension command throws
  (`node_modules/.bun/@earendil-works+pi-coding-agent@0.80.2+.../dist/core/
  agent-session.js:1043-1044, 963-969`).

## BEHAVIOR

Commands (authoritative change requested):
- C-N5-1 `directive add|confirm|revise|revoke` from a harness command surface:
  previews through `directive.preview`, requires a UI confirmation, then
  calls `directive.record|revise|revoke` with actor `user:pi` or
  `user:opencode` and an `operation_key`. Authoritative change happens in the
  Kernel (N1 pass). This pass owns reachability and the confirm gate.

Queries (no authoritative change):
- Q-N3-1 build the pass scope context from binding, gix HEAD, config, `cwd`,
  and tool-call paths.
- Q-N4-1 read injectable rows at one snapshot, partition, order, pack, count
  omissions, render, and report diagnostics.
- Q-N5-1 the parity query: for identical request fields the daemon answers
  identical rule object-id sets and disclosure lines.

Legal witnesses:
- A project-only rule is served on every pass of the bound project.
- A `dir:crates/storage` rule is served when a tool call names
  `crates/storage/src/lib.rs` and absent when the window names only
  `packages/cli/src/index.ts`.
- Nine rules over the reserve: served in order to the first that does not
  fit, `<omitted-rules count="N"/>` closes the block, optional rows fill the
  remainder, `rules_omitted == N`.
- One Pi pass and one OpenCode pass with the same `cwd` and window yield
  equal object-id sets and disclosure lines.

Forbidden outcomes:
- a dir-scoped rule served without a matching ancestor;
- a branch-scoped rule served on detached or unborn HEAD;
- a rule served because a dimension it names is absent from the context;
- a rule without a Project term served;
- a rendered row whose Kernel AutoInject verdict is not `Visible`;
- a required row skipped while a later required row is served;
- an omitted rule with no disclosure line or a disclosure count that differs
  from `rules_omitted`;
- two passes with identical inputs ordering rules differently;
- an `adr_accepted` row whose `source_kind` is not `user_directive`
  rendered;
- a directive recorded by a model tool call, a queued user message, a
  subagent child session, or without a UI confirmation;
- Pi and OpenCode receiving different rule sets for identical request fields.

## RULES

Each rule: temporal kind; observation dependencies; validity point.

- R-N3-a Context is resolved before any predicate is evaluated and from
  named sources only: state predicate at the read; depends on current
  authoritative state (binding), mutable context (HEAD referent, config
  key), and intrinsic request input (`cwd`, window). Valid for one pass.
- R-N3-b Every term the rule carries must match; a term naming an
  unresolvable dimension excludes the rule: state predicate at the
  per-row judgement; depends on the row's stored scope (current
  authoritative state) and the pass context (derived observation).
- R-N3-c Project term mandatory: initial admissibility of a row into the
  candidate set; intrinsic to the stored scope plus the binding digest.
- R-N3-d Directory ancestors derive only from paths inside `project_root`,
  posix-normalized, with `.` for the root: intrinsic to the request input;
  valid at context construction.
- R-N4-a Required partition is `source_kind in {user_directive,
  repository_policy}` over rows already `Visible` and scope-matched:
  state predicate; depends on current authoritative state at the snapshot.
- R-N4-b Required order is the total order (channel rank, specificity,
  `created_commit_seq` desc, `object_id` asc): state predicate over the
  candidate list; intrinsic to the rows and the context; deterministic.
- R-N4-c Reserve arithmetic: integer conversion once, wrapper and disclosure
  reserved first, limit `min(rules_reserve, remaining)`: intrinsic to the
  budget inputs; valid at packing.
- R-N4-d Required phase is prefix-only: a row that fails to fit ends the
  phase; no later required row is admitted: transition safety over the
  packing steps.
- R-N4-e Minimum service: the first required row is served when it fits
  `budget - wrapper`: state predicate at packing.
- R-N4-f Optional phase packs the remainder with skip-and-continue: state
  predicate at packing.
- R-N4-g `rules_omitted` equals the required candidates not served, is
  disclosed as `<omitted-rules count="N"/>` when positive, appears in
  diagnostics, and joins the revision digest: state predicate at render.
- R-N4-h Render category: `source_kind == user_directive` renders under
  `USER_DIRECTIVES`; every other row keeps `decision_kind`; an
  `adr_accepted` row with another `source_kind` is not rendered: state
  predicate at render; intrinsic to the row.
- R-N4-i Selection never admits: every rendered row's AutoInject verdict is
  `Visible` at the pass snapshot: state predicate; current authoritative
  state at the snapshot.
- R-N5-a The directive command is dispatched only by host user input, never
  by a model tool call, queued text, or a subagent child session: initial
  admissibility of the command; depends on harness mechanics (mutable
  external authority, version-pinned).
- R-N5-b No write without a UI confirmation: transition safety on the
  command; intrinsic to the invocation.
- R-N5-c Parity: identical request fields yield identical object-id sets and
  disclosure lines across harnesses: state predicate over the daemon's
  answer; depends only on the request and the Kernel snapshot.
- R-N5-d Pi registers the context handler whenever the project binds;
  compaction gates only the fold: initial admissibility of the handler.
- R-N5-e `is_subagent: true` yields no m0; Pi always sends `false`; the
  asymmetry is documented: state predicate at serving; intrinsic to the
  request.

## IDENTITY AND VALUES

- Scope context: value semantics. Equality is field-wise over (project
  digest, branch option, environment option, ancestor set). No identity, no
  lifetime beyond one pass. EARNED as a value.
- Directory ancestor: value, a posix relative path with `.` as root.
  EARNED as a value with a canonical form (owner of the form:
  representation pass).
- Required order key: value (rank, specificity tuple, seq, object id).
  EARNED as a value because equality and total order are the whole
  meaning.
- Omitted count: value (u64). EARNED as a value that joins the digest.
- Entity identity for rules, directives, decisions: owned by the Kernel
  (N1, N2 passes). NOT EARNED here.
- Aggregate, repository, domain service: NOT EARNED. The read side is a
  pure function of (snapshot rows, context, budgets) with no owned mutable
  state.

## TRANSITIONS

Packing is a two-phase procedure with an explicit history rule:
- Phase 1 (required): state `(served_prefix, charged)`; step admits the
  next required row when `charged + cost <= limit`; otherwise the phase
  ends. Illegal history: admitting row k+1 after row k failed to fit.
- Phase 2 (optional): skip-and-continue over the remainder.
- Minimum service is a guard on the first step of phase 1 against a wider
  limit (`budget - wrapper`).
Directive command: `parsed -> previewed -> confirmed -> recorded`; illegal
histories: `parsed -> recorded`, `previewed -> recorded` without `confirmed`.
Other transitions: NOT EARNED (no stateful read-side object).

## PRESERVATION

Enforcement sets (proposed participants):

- Scope exclusion (R-N3-b, R-N3-c): winning authority is the stored scope
  terms plus the pass context; mechanism is one daemon function evaluating
  Project through `names_project` and every other present term through
  `scope_matches` or ancestor iteration; writers are the single injection
  read path (m0 call site and the history-summarizer call site). Bypass to
  close: any other reader of memory rows that composes a provider-visible
  block without the context. Commit point: the pass's pinned read.
- Visibility (R-N4-i): winning authority is `read_visible` on
  `Surface::AutoInject`; the `Visible` filter in `injectable_snapshot` is
  the mechanism; selection code must add no row from any other source.
  Exceptional writers: test seeding (`memorySeed`) writes through the
  Kernel and so is inside the mechanism.
- Order and reserve (R-N4-b..g): intrinsic computation; mechanism is a
  pure function with one integer conversion; writers: the packer only.
  Bypass to close: the renderer must not re-order rows across the
  required/optional boundary in a way that hides a required row (see open
  question on render order).
- Command reachability (R-N5-a, R-N5-b): winning authority is the harness
  dispatch mechanism (Pi `prompt()` with `expandPromptTemplates`, OpenCode
  `command.execute.before`) plus the daemon route method as the trust
  boundary (N1); the confirm gate is the runner's own step. Bypass to
  record: another Pi extension calling `prompt("/eidnara-directive ...")`
  or a future Pi that lets `sendUserMessage` expand commands; the confirm
  gate is the covering control. OpenCode has no dialog, so the gate as
  written is unsatisfiable there (open question).
- Parity (R-N5-c): mechanism is daemon ownership of selection; writers:
  none in the plugins. Bypass to close: any plugin-side filtering or
  reordering of m0 rows.

## COMMAND OUTCOMES

`directive add|confirm|revise|revoke` (harness runner):
- Definitely not applied; cause domain rejection; stop: parse failure,
  no session, preview refused, no UI available, user declined.
- Definitely not applied; cause cancelled; stop: session replaced before the
  commit (`isCancelled`).
- Definitely applied; replay-safe: commit answered with a receipt and
  `replayed` flag; the harness reports applied or replayed.
- Unknown; cause unavailable; reconcile: commit answered without a verdict
  for the operation sent (`outcome_unknown`), or transport loss after send;
  caller re-issues the same `operation_key` to learn the outcome.
- Defect; certainty separate: a preview that does not answer the operation
  sent is reported as refused with `invalid("internal")` and nothing was
  sent to commit.

## QUERY CONTRACTS

Scope context (Q-N3-1): source is the binding (authoritative), the gix HEAD
referent read at the pass (mutable, may be absent), the config key (mutable,
may be absent), the request `cwd` and the live window (intrinsic). Freshness:
resolved once per pass, never cached across passes except the Branch cell
inside the pass. Completeness: a dimension is either resolved or absent;
absent is a first-class state that excludes terms naming it. Failure: a gix
error leaves Branch absent; it never fails the pass. Consumers: the injection
read only. Prohibited decisions: the context must not admit a row the Kernel
hides and must not widen the project.

Injection read (Q-N4-1): source `read_visible` at one tip; provenance
`known_as_of`; completeness bounded by `MAX_READ_ROWS` and
`MAX_READ_ROW_BYTES` with `truncated` reported; order is the total order of
R-N4-b for required rows and the existing newest-first order for optional
rows; result meaning: `rules_served`, `rules_omitted`, `context_resolved`;
empty block when nothing packs; withheld when the serving verdict is not
available (`CanonicalMemoryRead::Withheld`). Prohibited decisions: the block
and its diagnostics authorize nothing.

Parity (Q-N5-1): the daemon's answer for identical `(project, cwd, window,
is_subagent, snapshot)`; failure meaning: a difference is a daemon defect or a
plugin field divergence, distinguished by `context_resolved`.

## PERSISTENCE AND CONCURRENCY

No authoritative write on the read side. Required topology: the pass reads
one Kernel snapshot (`known_as_of`) and computes the block from it; a
concurrent directive or repository-policy commit lands at a later
`commit_seq` and is seen on the next pass. Two concurrent passes for one
session are serialized by the session lane (7.10.1). Two passes for two
sessions of one project may read different snapshots; parity is defined per
snapshot. A process crash between kernel writes concerns N1 and N2; the read
side observes whatever snapshot is committed. The directive command's
unknown-outcome window is the commit send; `operation_key` replay resolves
it (N1).

## DERIVED STATE

- Scope context: base authority as above; derivation per pass; freshness
  exact for the pass; consumers: the matcher; prohibited: admitting a row.
  Mismatch detection: `context_resolved` diagnostics against the harness's
  own knowledge of `cwd`. Rebuild: every pass.
- Packed row set and omitted count: derived from the snapshot and budgets;
  consumers: renderer, digest, diagnostics; prohibited: authority decisions.
  Mismatch detection: `rules_served + rules_omitted == required candidate
  count`; the disclosure count equals `rules_omitted`.
- Revision digest: derived from rendered rows plus N; consumer: m1 revision
  signal and composition metadata; prohibited: none beyond cache keying.
- `context_resolved`: display and test oracle only.
- Read-cap truncation: `truncated` remains a separate derived flag; rules
  dropped by the read cap before partition are invisible to `rules_omitted`
  (open question).

## STRUCTURE INPUTS

Hidden decisions: how directory ancestors are derived from JSON (field
names `path`, `file_path`, `paths[]`, `cwd`); the specificity key; the
reserve arithmetic. Each belongs in one daemon function with a fixture
oracle. Semantic edges: injection read depends on (uses) `read_visible`,
`names_project`, `scope_matches`; packer depends on `skip_and_continue`;
reuse of `admit_required`/`reserve_required` is a correctness `uses` only if
the prefix semantics are implemented around the all-or-nothing primitive.
Harness plugins have a dataflow edge (request fields) and no decision edge.

## VARIATION / ASSEMBLY

Harness selection is an existing variation point (serializer profile);
parity requires that no rule logic vary by it. Config keys
`/memory/injection_rules_reserve_tokens` and `/memory/injection_environment`
are runtime inputs, not variation points. NOT EARNED otherwise.

## FALSIFIERS

See catalog.md and evidence/<slug>.md for each record. Analysis bounds: all
falsifiers are single-pass, single-snapshot traces except the parity and
concurrent-pass traces; none proves writer completeness for the implemented
code.

## HANDOFF

- N1 pass: `directive.*` route bodies, admission sequence, operation_key
  replay, explicit-user subject guard.
- N2 pass: repository rule writer, `dir:` term authoring, PROJECT_RULES.
- Wire enrichment: `cwd` bounds, diagnostics member types, 7.8 literal count.
- Representation pass: canonical posix form of `dir:` values; `.` root.
- Implementation (U1, U2, U4, U6): resolve open-questions.md items tagged
  resolvable at implementation; the four items tagged needs human input
  block U4 (OpenCode confirm gate), U2 (render order), U1/U2 (read-cap
  interaction), and U6 (Pi version drift policy).
- Test review (`/testing:invariant-test-review`): the oracles in
  evidence/*.md "What a test must construct".
