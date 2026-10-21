# Portfolio evaluation: existing-behavior catalogs (S1..S5)

Evaluator: fresh context, no access to the discovery reasoning. HEAD
`08de9051a`. Read-only against the repository; no cargo, bun, or tracker
command was run. Inputs: `docs/properties/METHOD.md`, the skill reference
`coverage-vacuity-and-portfolio.md`, `BRIEF.md`, and the two catalogs with
their companions and evidence files.

## Scope evaluated

- `discovery-kernel-scope/`: 11 records (S1 admission and authority, S2
  scope eligibility), `existing-checks.md`, `fault-map.md`, `handoff.md`,
  11 evidence files, 5 lens files.
- `discovery-packing-injection/`: 12 records (S3 packing primitives, S4 m0
  injection path, S5 harness surfaces), the same companions, 12 evidence
  files, 3 lens files.
- Risk model from the brief: a model-authored row reaching authority; a
  stale, revoked, unverified, or out-of-scope rule served; silent
  omission; harness divergence.

## Mechanical verification

Run with grep and diff over the two directories and the repository.

- Records equal index rows equal evidence files: kernel-scope 11 = 11 = 11;
  packing-injection 12 = 12 = 12. Slug sets match exactly.
- Every `evidence/<slug>.md` link in both catalogs resolves. Every index
  anchor in the packing catalog resolves to a `###` heading.
- Reused-record links: the five `docs/properties/.../catalog.md:<line>`
  references in the kernel catalog land on the named `###` heading at
  HEAD. The eight reused records in the packing catalog name a catalog
  path without a line; each file exists.
- Schema: all 23 records carry the 12 METHOD.md fields in the prescribed
  order plus the brief's `Origin:` line. No field is missing. Every
  evidence file carries the six required section headings.
- Check-semantics distribution: kernel-scope 11 `always`; packing-injection
  12 `always`. Portfolio total 23 `always`, 0 `always-or-unreached`, 0
  `sometimes`, 0 `reachable`, 0 `unreachable`. Type distribution: 23
  safety, 0 liveness, 0 reachability. Reachability labels: kernel 5
  `default-production`, 6 `test-only`; packing 8 `default-production`, 3
  `explicit-config-only`, 1 `test-only`.

Spot-checked file:line references (about 70 lines read at HEAD). All of
these land on the named symbol or statement:
`admission.rs:20`, `:268`, `:710`, `:1339`, `:1424`, `:1447`, `:1640`,
`:2156`, `:2436`, `:2726`, `:2749`, `:3548`, `:4116`, `:4148`, `:4197`,
`:4910`, `:5066`, `:5734`, `:5819`; `write.rs:388`, `:434`, `:627`,
`:1271`, `:1281`; `commit.rs:237`, `:240`, `:283`, `:462`, `:589`, `:833`,
`:1071`, `:1117`; `project.rs:81`, `:194`, `:198`; `eligibility.rs:86`,
`:209`; `scope.rs:628`, `:1392`, `:1450`; `read.rs:26`, `:29`, `:70`,
`:174`, `:178`, `:197`, `:210`, `:233`; `lib.rs:305`, `:5447`, `:9345`,
`:13517`, `:13785`, `:13853`; `canonical_memory.rs:50`, `:184`, `:206`,
`:282`, `:315`, `:373`, `:390`, `:412`, `:457`; `m0_compose.rs:74`, `:79`,
`:88`, `:300`, `:337`; `memory_render.rs:67`, `:99`, `:116`, `:184`,
`:446`, `:619`, `:714`; `transform.rs:671`, `:2790`, `:2907`, `:3178`,
`:3341`, `:4216`, `:11723`, `:13803`, `:13851`, `:13894`, `:13958`,
`:16293`, `:17437`; `config.rs:30`, `:143`, `:146`, `:182`, `:1198`;
`fold_authority.rs:11`; `m1_compose.rs:75`; `direct_host_fixture.rs:66`,
`:757`; `index.ts:332`, `:338`, `:343`, `:381`, `:383`, `:550`, `:565`,
`:573`; `pi-pass-inputs.ts:110`; `transform-pass-fields.ts:55`, `:66`;
`opencode-transform-adapter.ts:458`, `:507`; `memory-mark-command.ts:130`,
`:140`, `:146`; `command-handler.ts:411`, `:420`, `:493`, `:713`;
`hook.test.ts:1803`; `eidnara-memory-mark.test.ts:117`;
`index-env-guard.test.ts:38`; `host-wire-protocol.md:966`, `:1032`; the
test functions named in both `existing-checks.md` files that I opened
(`kernel_admission.rs:469`, `:1292`, `:1740`; `kernel_slice.rs:1095`;
`kernel_eligibility.rs:148`; `kernel_scope_algebra.rs:325`, `:426`;
`kernel_routes.rs:1145`, `:1803`, `:4356`, `:4392`;
`transform_canonical_memory.rs:46`, `:320`, `:356`).

References that are off:

- `discovery-packing-injection/existing-checks.md`, production guards
  table, `read.rs:250-253` "same check after the payload cutoff". Line 250
  is a closing brace; the second `CorruptCanonicalRow` check is
  `read.rs:245-252`. Off by about five lines; the claim itself is correct.
- `discovery-packing-injection/catalog.md`,
  `project-memory-truncated-flag-excludes-budget-trimming`, cites
  `canonical_memory.rs:98-102` for the carry into
  `ProjectMemoryComposition::Canonical`. Line 98 is blank and 99 is the
  `impl` line; the `composition` method body that copies `truncated` is
  `:100-106`. Off by two lines; harmless.

No other checked reference was off.

## Harness fit

The handoff files route every record to a skill and leave the test form
to the receiver, which is the contract. Observations:

- Kernel handoff, `authority-loss-cascades-to-latest-dependents`, routes
  the deferred branch to `/testing:deterministic-simulation-testing`. The
  branch needs a generated graph of 4,097 dependents inside one SQLite
  transaction (`admission.rs:1447-1486`). No schedule control, time
  control, fault injection, or replay is involved. This is a generator
  problem for `/testing:test-strategy` (bulk fixture or proptest), and the
  packing handoff states the same rule for its own records ("none needs
  schedule control"). Refinement R4.
- Packing handoff, `memory-seed-fixture-writes-repo-source-kind-under-code-observed`,
  routes primary to `/testing:stateful-workload-design` for one e2e
  readback of one seeded row. A single readback is not a workload; the
  workload skill fits only if AE8 parity becomes a multi-step e2e.
  Refinement R6 swaps primary and secondary.
- Packing handoff routes the `DefaultHasher` persistence question to
  `/low-level-systems:defensive-assertions-and-invariant-guards`. The
  question is a cross-version compatibility decision on a persisted
  `u64`, and the record already marks it `needs human input`. A guard
  skill can add a pin test; it cannot decide whether a one-time refold is
  acceptable. The routing is harmless because the record already escalates
  to a human; no change required.
- Every other routing is sound: pure functions and per-request gates go to
  `test-strategy`, well-covered records go to `invariant-test-review`
  first, the policy digest goes to mutation testing.
- No semantics is mismatched to a test mode. Every `always` record is
  evaluable on a correct implementation with the enabling state the fault
  map names.

## Coverage balance

Against the brief's four risks:

- Model-authored row reaching authority: dense and correct.
  `plugin-commit-classes-are-closed-at-model-inference`,
  `accepted-adr-requires-live-accepted-decision-subject`,
  `config-observed-promotes-only-on-exact-lineage-trigger`,
  `sensitive-decision-...`, the reused
  `retrieved-content-cannot-upgrade-write-authority`, and the memory-mark
  runner record cover the write side from route to evaluator. Lead L4 (the
  KTD9 guard is absent at HEAD) is recorded and handed to N1 rather than
  disguised as an existing guarantee. Good.
- Revoked, stale, or unverified rule served: revocation is covered by the
  cascade record plus the reused `stale-projection-cannot-authorize-current-use`;
  unverified by `injectable-rows-require-visible-verdict-...`. Stale in
  the sense of superseded or retired is covered only by an existing test
  (`transform_canonical_memory.rs:46`, which retires, quarantines, and
  supersedes rows) and by lead L3; no record names the registry-level
  exclusion (`eligibility.rs:209` `superseded_by`, `invalidated_commit_seq`
  at the snapshot) as a serving guarantee. Gap G2.
- Out-of-scope rule served: `scope-filter-excludes-a-term-on-an-unresolved-dimension`
  and `sql-project-prefilter-is-a-superset-of-the-daemon-verdict` cover
  the two layers, with the vacuity relation between them stated in the
  relationship map. Good.
- Silent omission: the `truncated` flag record is the only record on the
  read layer. Both the kernel read caps (`MAX_READ_ROWS` 8192 newest-first,
  `MAX_READ_ROW_BYTES` payload prefix, `read.rs:26-29`, `:197`, `:229-234`)
  run before `injectable_snapshot` and before any packing. A rule row older
  than the newest 8192 visible rows of its project is absent from the
  required phase and from `rules_omitted`; `truncated: true` is the only
  trace. The plan does not mention the caps (grep over the plan for
  `MAX_READ_ROWS`, `8192`, "cap" finds only the maturity cap). Gap G1,
  the most important coverage finding.
- Harness divergence: three gate records plus the pass-field record cover
  it well; the catalog is candid that parity holds only for equal
  `is_subagent` and that Pi cannot send `true`.

Over-representation: none material. Four kernel records describe evaluator
tables that one exhaustive test already pins (`policy-digest-pins-revision-one`
is the meta-record for all of them); that is acceptable because each
record names a distinct enabling state.

Duplication with existing catalogs: none found. The packing catalog reuses
eight records instead of restating them; the kernel catalog reuses five.
`sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority`
is adjacent to `served-sensitivity-and-artifact-policy-govern-egress`
(canonical-positive-claim-projection:511) but states a different claim
(matrix and authority predicate versus reader-equals-serving).

Missing types: 0 liveness, 0 reachability across 23 records. See bias B1.

## Implementability

Each record's observation point, enabling state, and faults were checked
against the fault maps and the code.

- No record asserts the violation itself. No fault-map marker is the
  negation of its record's invariant. The kernel fault map's
  `sometimes(prefilter_kept_a_row_the_filter_then_dropped)` fires on a
  correct system (a scope with no `project` term is kept by SQL and refused
  by `names_project`), as the file states.
- One marker conjoins a precondition with the asserted outcome:
  `m0_budget_dropped_row_with_truncated_false` (packing fault map). It
  fires on a correct system, so it is not vacuous, but it duplicates the
  record's assertion inside the coverage check. Refinement R3.
- `subagent-pass-is-never-hard-and-serves-no-m0` and
  `additive-only-pass-still-composes-project-memory` state that the
  additive-only path has no `is_subagent` gate. `transform.rs:3341` reads
  `if !authority.eidnara_folds && !(req.lineage_switched && req.is_subagent)`
  and `:3355` returns `lineage_protocol_passthrough` (plan `Defer`, no
  m0) for a `lineage_switched` subagent request. The evidence file quotes
  the full condition; the catalog text drops the qualifier. A test built
  from the catalog check with `lineage_switched: true` would fail on a
  correct system. Refinements R1 and R2.
- `project-memory-truncated-flag-excludes-budget-trimming`, `true` arm:
  needs 8,193 visible rows or 8 MiB of payload; the fault map marks it
  "costly; a smaller cap is not injectable". Constructible but expensive;
  the record is honest about it.
- `pass-field-builder-...`, child arm: the in-process pi-subagents child is
  marked "partial: handler scope not observable here". Not constructible
  from this repository alone; correctly left open.
- `memory-trim-cost-model-mirrors-rendered-block-structure`: the byte
  identity is an exact unit oracle; the tokenizer arm is declared a
  measurement first. Sound.
- Kernel `test-only` records: every enabling state is constructible through
  the store API or `seed_approval`; the fault map names the helper per
  class. Sound.
- The kernel fault map asserts "every mechanism runs in one SQLite
  transaction under a single writer" without a citation. The claim is
  plausible (envelope commit path) but is stated as fact. Refinement R7
  asks for the citation or an "assumption" label; gap G3 covers the
  two-pass concurrency question the plan introduces.

## Wildcard

Hunting outside the catalogs' frames:

- Kernel read caps before packing (G1, above). This is the one place where
  the new feature's "disclosed omission" promise can be false while every
  cataloged record passes.
- Persistence of superseded rows: `supersede_decision` invalidates the
  registry row and links `superseded_by`; `read_visible` reads
  `visible_as_of`, so the predecessor disappears at the next snapshot and
  the digest moves. Covered by test, not by a record (G2). The reconcile
  path will supersede on every content edit, so this becomes the most
  frequent lineage event in production.
- Concurrency between two passes: two sessions bound to one project (or Pi
  and OpenCode on one checkout) both reach the reconcile step on the m0
  compose path. Receipts keyed by `(producer, operation_key)` exist
  (`envelope.rs:85-123`) and the plan keys each file commit by
  `<path>:<oid>`, so the second writer should replay. No record in either
  catalog describes receipt replay or the writer lock as a guarantee the
  plan relies on. Gap G3 for the N2 pass.
- `DefaultHasher` across Rust releases: found by the packing lens, recorded
  as an open question with `needs human input`, and routed. No further
  action here beyond bias B1's note that the revision is compared across
  daemon versions with no pin test.
- Sensitive artifacts: `config-observed-...` records that the trigger's
  and evidence's sensitivity fold into the decision (`admission.rs:1605`,
  `:1631`) and `validate_provenance` (`:2569-2591`) constrains repository
  provenance. No record covers how an ingested `AGENTS.md` artifact
  acquires `Sensitive` or `Secret`, or what the envelope's secret
  detection (`envelope.rs:85`) does to a rule unit that contains a
  credential-shaped string. A repository file that leaks a token would
  either be hidden (good) or served verbatim under `PROJECT_RULES` (bad);
  which one happens is undetermined by these catalogs. Gap G4 for N2.
- Pi in-process children: the pass-field record carries the question and
  `index.ts:338-348` is cited. Nothing to add.
- Wrapper element validity: the packing lens notes `render_memory_block`
  does not validate its wrapper name and that `<omitted-rules count="N"/>`
  must respect the same escaping if rendered through that function. The
  item did not become a record. It is a proposed-side obligation (N4) and
  is noted here so the N4 evaluator sees it.

## Findings disposition table

| # | Finding | Class | Record(s) | Evidence | Proposed action |
| --- | --- | --- | --- | --- | --- |
| G1 | Kernel read caps drop rule rows before packing; `rules_omitted` cannot count them | gap | none (adjacent: `project-memory-truncated-flag-excludes-budget-trimming`) | `read.rs:26-29`, `:197`, `:229-234`; `canonical_memory.rs:163-166`; plan has no mention of the caps | Targeted discovery in the N4 pass: guarantee "required rows are selected before or independently of `MAX_READ_ROWS` newest-first truncation, or a cap-dropped rule is counted in `rules_omitted`"; look at `read_visible` and the plan's KTD5 and KTD7 |
| G2 | Superseded and retired rows absent from the next AutoInject read is an existing test with no record | gap | none | `eligibility.rs:209`; `write.rs:434`; `transform_canonical_memory.rs:46`, `:119` | Add one S2 record: "serving excludes rows invalidated or superseded at the snapshot"; existing check is the integration test; reuse L3 |
| G3 | Two passes reconciling one project concurrently; receipt replay and writer lock uncited | gap | kernel `fault-map.md` single-writer statement | `envelope.rs:85-123`; `transform.rs` compose path; plan `operation_key` design | N2 discovery: record receipt dedup on `(producer, operation_key)` and the writer serialization as guarantees; cite the lock |
| G4 | Sensitivity and secret handling of ingested `AGENTS.md` units undetermined | gap | `config-observed-promotes-only-on-exact-lineage-trigger` (folding only) | `admission.rs:1605`, `:1631`, `:2569-2591`; `envelope.rs:85`; `redaction.rs` | N2 discovery: how an artifact's class is assigned at ingest and what the envelope does with a detected secret in a rule unit |
| R1 | Subagent record says the additive path has no `is_subagent` gate | refinement | `subagent-pass-is-never-hard-and-serves-no-m0` | `transform.rs:3341`, `:3355` | Guarantee: replace "no `is_subagent` gate exists" with "the only `is_subagent` gate is the `lineage_switched && is_subagent` exclusion at `:3341`, which routes to `lineage_protocol_passthrough` (`:3355`)". Check: qualify the third clause with "and `lineage_switched: false`" |
| R2 | Additive-only record states the entry condition incompletely | refinement | `additive-only-pass-still-composes-project-memory` | same | Reachability line: "`apply_additive_only` runs when `!authority.eidnara_folds` and the request is not a `lineage_switched` subagent request (`transform.rs:3341`)" |
| R3 | Coverage marker conjoins the outcome | refinement | packing `fault-map.md` | marker `m0_budget_dropped_row_with_truncated_false` | Rename to `m0_budget_dropped_row` and define it as "the trim dropped at least one row on a served read"; the record asserts the flag |
| R4 | DST routing for a generated-graph construction | refinement | kernel `handoff.md`, `authority-loss-cascades-to-latest-dependents` | `admission.rs:1447-1486`; packing handoff's own rule | Replace "`/testing:deterministic-simulation-testing` for the deferred branch" with "`/testing:test-strategy` (generated 4,097-dependent fixture) for the deferred branch" |
| R5 | Off-by-five line reference | refinement | packing `existing-checks.md` | `read.rs:245-252` | Change `read.rs:250-253` to `read.rs:245-252` |
| R6 | Workload skill as primary for a single readback | refinement | packing `handoff.md`, `memory-seed-fixture-...` | handoff row | Swap: primary `/testing:test-strategy`, secondary `/testing:stateful-workload-design` "if AE8 becomes a multi-step e2e" |
| R7 | Single-writer claim stated without citation | refinement | kernel `fault-map.md` | "every mechanism runs in one SQLite transaction under a single writer" | Cite the envelope commit path and lock, or label the sentence "assumption, verified by N2" |
| B1 | Portfolio is 23/23 `always` safety; no reachability or liveness record; situation coverage lives only in fault-map markers | bias | all | semantics distribution above; cascade record's unbounded "until a later write" | Human decides whether to require `sometimes`/`reachable` records for the enabling states that N1 and N2 turn into production states, and whether the deferred-set repair needs a bounded liveness record or a design change |
| B2 | Record selection is filtered by "what the plan relies on"; items the wildcard lenses found but the plan does not name (read caps, wrapper validity, `memory_enabled` reload) stayed in `_lenses/` | bias | both catalogs' Scope sections | kernel Scope paragraph; packing Scope bullet 2; packing lens Wildcard section | Human decides whether the scope boundary is "plan dependencies" or "existing behavior the plan changes"; G1 is the cost of the current boundary |
| B3 | Six of eleven kernel records are `test-only` because the plan's writers do not exist yet; labels flip en masse after N1/N2 | bias | kernel records labeled `test-only` | METHOD.md rule 4; kernel Reachability section | Human decides whether to annotate "post-plan reachability" now or relabel at disposition; either way the relabel is one coordinated edit, not eleven independent ones |

Totals: 4 gaps, 7 refinements, 3 biases.

## Biases for a human

B1. Every record in both catalogs is `always` and `safety`. The method
permits situation coverage to live in fault-map markers, and both fault
maps supply them. The cost is that nothing in the catalogs will fail when
the new channels never produce a Visible row in production, which is the
plan's stated motivation ("no production writer produces a row that
reaches Verified"). The judgment required: accept the marker delegation,
or require at least one `reachable` or `sometimes` record per channel
("an `AcceptedAdr` row written by the directive route was Visible on
AutoInject in a production-configured daemon"). Related: the cascade
record's deferred set is repaired "until a later write" with no bound;
METHOD.md forbids unbounded liveness, so the choice is a bounded repair
in code or an explicit acceptance of the window.

B2. Both catalogs scope themselves to "behavior the settled plan depends on
or extends". The lens files show the discoverers saw the read caps, the
wrapper validity precondition, and the `memory_enabled` reload pairing,
and none became a record because the plan does not name them. G1 is the
direct consequence: the plan's disclosure promise can be false at a layer
the plan did not look at. The judgment required: whether discovery of
existing behavior should be bounded by the plan's own citations.

B3. METHOD.md rule 4 labels reachability by the enabling state at HEAD, so
six kernel records are `test-only` for a feature whose purpose is to add
the writers. The labels are correct today and wrong the day U1 lands. The
judgment required: carry a forward-looking annotation now, or schedule a
single relabel pass at disposition and accept that the catalogs read as
"test-only" during implementation.

## Disposition (applied after the evaluation, by the specification author)

Refinements applied directly to the catalog files before publication:

- R1, R2: the `subagent-pass-is-never-hard-and-serves-no-m0` guarantee and
  check and the `additive-only-pass-still-composes-project-memory`
  reachability line now carry the `lineage_switched && is_subagent`
  exclusion (`transform.rs:3341`, `:3355`).
- R3: the coverage marker is renamed `m0_budget_dropped_row` and asserts only
  that the trim dropped a row; the record asserts the flag.
- R4: the deferred cascade branch routes to `/testing:test-strategy` with a
  generated 4,097-dependent fixture.
- R5: `read.rs:250-253` corrected to `read.rs:245-252`; the
  `canonical_memory.rs:98-102` reference corrected to `:100-106`.
- R6: the memory-seed record's primary handoff is `/testing:test-strategy`;
  the workload skill is secondary.
- R7: the kernel fault map labels the single-writer sentence an assumption
  that the N2 pass verifies and cites.

Gaps queued:

- G1 (kernel read caps before packing): recorded as specification open
  question OQ11, to be resolved before U2 starts; the N4 record
  `packing-omitted-count-is-disclosed-reported-and-digested` carries the
  dependency. The targeted record is added when the decision names the
  selection.
- G2 (superseded and retired rows absent from the next read): closed by the
  new S2 record `served-rows-exclude-objects-invalidated-at-the-snapshot`
  with its evidence file, existing-check row, fault-map row, and handoff.
  Counts in the kernel part are now 12 = 12 = 12.
- G3 (two passes reconciling one project): recorded as OQ16; the N2 record
  `repo-policy-cached-head-oid-is-advisory-and-reconverges` names the
  overlapping-pass window, and the U5 ticket cites the writer lock.
- G4 (sensitivity and secret handling of ingested `AGENTS.md` units):
  recorded as OQ22, to be resolved before U5 starts.

Biases surfaced to the owner in the specification's Open Questions (OQ23,
OQ24, OQ25): the all-`always` safety portfolio, the plan-dependency scope
boundary, and the `test-only` labels that flip when U3 and U5 land. The
proposed parts contain one `reachability` record
(`parity-pi-context-handler-registers-whenever-project-binds`) and one
`sometimes` record (write channels), which partially answers B1 for the new
channels.
