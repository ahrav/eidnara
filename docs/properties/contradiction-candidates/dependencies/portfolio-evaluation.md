# Portfolio evaluation: P5 dependency catalog (`p5dep-*`)

HEAD `3a3b916b7`, evaluated 2026-10-24 with fresh context. The evaluator read
`docs/properties/METHOD.md`, the skill's "Evaluate the portfolio separately"
section and `references/coverage-vacuity-and-portfolio.md`, then
`catalog.md`, `existing-checks.md`, `fault-map.md`, `system-model.md`, and
every file under `evidence/`, plus `/tmp/p5-spec/context.md` and
`plan-p5.md` for the consumer's risk model. No `_lenses/` material and no
discovery reasoning were read. Every `file:line` cited below was read at HEAD
by the evaluator. The repository was not modified.

Verdict (one line): the set is fit as specification input for the P5 spec
once refinements R1 to R8 are applied, with gap G1 (scope soundness of the
composite read) run before the spec fixes R2's read path; G2 to G7 can be
carried as spec-time open questions.

Counts: 7 gaps, 8 refinements, 3 biases. Spot-check: 41 references checked,
37 confirmed, 4 corrected (all line drift of one to seven lines inside the
named function; no wrong file, no wrong claim).

## 1. Spot-check of `file:line` references against HEAD

| Reference as cited | Result |
| --- | --- |
| `analysis.rs:78-105` (byte check `:82-88`, atom check `:92-96`) | confirmed |
| `analysis.rs:15-20`, `:22-28`, `:33-36` | confirmed |
| `lexical/mod.rs:40` `ANALYSIS_CONTRACT_EPOCH` | confirmed |
| `identity.rs:13-18` (`LazyLock`), `:36-65` (`preimage`) | confirmed |
| `admission.rs:3165-3176` signature; `:3138-3144` superset doc | confirmed |
| `admission.rs:3207-3220` `Vec<VisibleRow>` materialization | confirmed |
| `admission.rs:3473-3476` `ScopeTermFilter` | confirmed |
| `admission.rs:3574-3579` `ids_predicate` | corrected: `:3575-3580` |
| `admission.rs:3654-3672` WHERE tail; `:3668` NULL-scope branch | corrected: tail is `:3650-3664`; `OR {scope_expr} IS NULL` is `:3661` |
| `read.rs:26` `MAX_READ_ROWS` | confirmed |
| `read.rs:177` `scope_term()`; `:189` LIMIT comment; `:190`/`:191` probe point | corrected: `:178`, `:190`, and the loop over `visible.rows` is `:192` |
| `read.rs:185` domain-decisions call (plan pattern) | confirmed |
| `read.rs:204-230`, `:240` payload sizes then payloads | confirmed (`:205`, `:216`, `:226`, `:241`, `:249`) |
| `project.rs:106-111` `scope_term`; `:212-214` `None => false` | confirmed |
| `slice/read.rs:126-133`, `:134`, `:139-146`, `:236-241`, `:330` | confirmed |
| `claim_facts.rs:240-257`, `:248`, `:309`, `:313`, `:146-156`, `:90-97` | confirmed; `ClaimDecisionFacts` carries ids only (D1 holds) |
| `source_capture.rs:3`, `:54`, `:269`, `:616-619`, `:623-626`, `:814-879` | confirmed |
| `source_capture.rs:796-825` `CaptureJob::run`, `run_settled` | confirmed at this tip |
| `lib.rs:9279-9325` `capture_after_response` | confirmed |
| `lib.rs:13653`, `:13660` `settle_prepared`, `:13662-13664` | confirmed at this tip |
| `lib.rs:4000-4008`, `:274-281`, `:413-417`, `:35931` | confirmed |
| `local_file.rs:64-70`, `:245-247`, `:487-501` | confirmed |
| `slice/write.rs:42-54`, `:569`, `:585`, `:606-608` Conflict | confirmed |
| `schema.rs:136` retention comment | confirmed |
| `kernel_routes/mod.rs:461` `ENVELOPE_KEYS` includes `kind` | confirmed (D6 holds) |
| `wire.rs:131-139` `authority_project` | confirmed |
| `host-wire-protocol.md:795-799`, `:819-823` | confirmed |
| `types.ts:6`, `:24`, `:27-29`; `tools.ts:72-74`, `:125-138` | confirmed at `3a3b916b7`; at the #1164 tip the allowlist default is `tools.ts:103-106` and `isEidnaraMemoryMutation` is `types.ts:26-29` (#1161) |
| pi `eidnara-memory.ts:155-169`, no `allowedActions` | confirmed at `3a3b916b7` (grep count 0); at the #1164 tip #1161 added `allowedActions` (`eidnara-memory.ts:146-151`) and D7 is resolved |
| `config.rs:154`; `memory-store/src/lib.rs:335-340`; `storage/src/lib.rs:2631` | confirmed |
| `activation.rs:174`; `lifecycle.rs:16-18`, `:327` | confirmed |
| `tool-registry.ts:58`; `subagent-entry.ts:63` | `tool-registry.ts:58` now passes `EIDNARA_MEMORY_DEFAULT_ACTIONS` (#1161); confirmed at `packages/opencode-plugin/src/plugin/tool-registry.ts:58` and `packages/pi-plugin/src/subagent-entry.ts:63`; the catalog gives bare file names |
| 22 test function locations (`source_capture_tests.rs:312,335,442,666,783,931,2231`; `source_capture.rs:1797`; `kernel_routes.rs:1958,2080,2112,1803`; `kernel_scope.rs:235`; `kernel_canonical_read.rs:227,448`; `kernel_claim_facts.rs:466`; `claims.rs:993`; `embedding_dispatch.rs:1102`; `search_catchup.rs:1876`; `memory_reviewer_wire.rs:15,260`; `baseline.rs:492`; `lifecycle.rs:704`; `local_file.rs:710,722,791,889`; `tools.test.ts:1185,1193`) | all confirmed |
| `query_route.rs:1055`; `lexical/index.rs:50`; `daemon/Cargo.toml:45`; `admission.rs:4261` | confirmed |
| `harness_sources.rs:614-618`, `:1233`; `open.rs:614` `hold_readers_for_test` | confirmed |

## 2. Check-semantics audit against METHOD.md

- Distribution: 19 `always`, 1 `sometimes`, 0 `always-or-unreached`, 0
  `reachable`, 0 `unreachable`, 0 liveness. Recorded in the catalog index.
- No `always(!X)` is paired with `sometimes(X)`. The one coverage pairing,
  `p5dep_cov_kernel_rows_exceed_daemon_cap` (`sometimes(input rows > 8192)`)
  against `p5dep-daemon-bounds-rows-after-the-scope-filter`
  (`always(output rows <= 8192)`), names input and output sets, so the marker
  fires on a correct system. Correct.
- Every coverage check in `fault-map.md` asserts a precondition
  (`RefusedQueueFull` counted, two batches in one job, page full, cited
  capture expired, observer newest on root). None asserts a violation.
- `p5dep-kernel-domain-read-has-no-row-bound` uses `sometimes` for a
  situation, which is the right choice under METHOD's `reachable` versus
  `sometimes` rule. Its Guarantee field describes the implementation rather
  than the reachable situation (see R6).
- `p5dep-baseline-digest-change-closes-the-activation-gate` writes the check
  as a biconditional `open Ok ⇔ marker == digest`. The reverse direction is
  false in general (an I/O or corruption failure refuses a matching file).
  See R3.
- `p5dep-budget-exhaustion-stops-a-budgeted-kernel-read` states a wall-clock
  bound ("within `WRITER_ACQUIRE_POLL` of the deadline", 1 ms at
  `open.rs:29`). METHOD asks for bounds in the units the code bounds; the code
  bounds in polls, not milliseconds. See R8.
- S6 records carry two reachability labels. METHOD rule 4 and the schema
  allow exactly one. See R1.

## 3. Lens: harness fit (routing for `/testing:test-strategy`)

| Record | Needs | Routing recommendation |
| --- | --- | --- |
| analyzer-refuses-oversize-input | none; pure fn | unit or property test over generated segments; count retained atoms at refusal through `Analysis` or a probe |
| analyzer-keeps-original-bytes-and-is-pure | none | property test: byte-substring and repeat-call equality oracles; a thread-pair run for the "across threads" clause |
| analysis-epoch-and-identity-pin | out-of-tree toolchain change | existing pinned test; `/testing:invariant-test-review` only |
| domain-decision-read-matches-domain-and-kind | two domains, raw SQL corruption | Kernel integration fixture (existing pattern `kernel_canonical_read.rs:448`) |
| scope-prefilter-is-a-superset | five scope shapes | Kernel integration fixture with `names_project` as oracle; add `set` operator and `Dimension::Project` cases |
| daemon-bounds-rows-after-the-scope-filter | >8192 rows, large payloads | existing daemon integration tests; `/testing:invariant-test-review` |
| kernel-domain-read-has-no-row-bound | >8192 rows | coverage assertion inside the existing over-cap fixture, calling the Kernel API directly (R6); no production marker needed |
| claim-fact-bounds-refused-before-any-row | over-bound ids, exhausted budget | unit-level Kernel test with the acquisition counter |
| budget-exhaustion-stops-a-budgeted-kernel-read | held pool (`test-support`), mid-statement deadline, parent interrupt | two tests per API family; the mid-statement clause routes to `/testing:deterministic-simulation-testing` or a deliberately slow statement; the acquire-only clause is a plain `test-support` test |
| decision-page-returns-an-oversize-decision-alone | sized payloads | existing Kernel integration test |
| response-never-waits-on-the-capture-job | kernel held/unavailable, busy slot | existing integration suite; byte-identity oracle; the timing clause needs the ordering oracle in R4 |
| capture-slot-is-single-and-released | panic injection (`cfg(test)` hooks), cancel | existing integration suite |
| capture-batch-is-bounded-per-batch | 33 units, >64 KiB, pending documents | existing suite plus one two-batch assertion |
| capture-job-ends-in-one-closed-vocabulary-count | every exit path | exit-matrix integration test with counter deltas; `always` fits |
| baseline-digest-change-closes-the-activation-gate | frozen prior baseline fixture | existing tests; operator step is out of test scope |
| harness-sweep-retires-owners-before-evidence | `now` argument, pin race, refused capture | in-module Kernel tests (existing seams); `now` is a parameter so no clock control needed |
| cited-evidence-survives-the-harness-sweep | foreign `decision_events` citation | one new Kernel fixture; cheapest high-value test in the set (fault-map rank 4 is right) |
| review-reads-parse-strict-bodies-after-the-envelope | mismatched root, unknown field, NotModule; `store_unavailable` has no seam (F18) | wire tests; the `store_unavailable` clause is `always-or-unreached` material until a seam exists (R8 note) |
| observational-routes-read-and-take-no-background-part | two routes on a root | existing daemon tests |
| memory-tool-actions-are-closed-and-allowlisted | fake transport | existing TS tests plus one `context-researcher` call and one pi disallowed action |

No record needs timing, faults, or state exploration beyond what unit or
integration fixtures construct, except the mid-statement budget clause.
Semantics match the test modes except where noted in section 2.

## 4. Lens: coverage balance against the consumer's risk model

The consumer (plan R1 to R7) depends on: bounded work per pass, deterministic
scoring, project-scoped candidate sets, publish-then-trigger ordering,
evidence liveness at confirm time, a deliberate baseline rotation, and two
surface contracts. The catalog covers bounded work (S1, S2 bound and witness,
S3), determinism of the analyzer (S1), publication timing (S4), evidence
retention (S6), the rotation (S5), and both surfaces (S7, S8).

Uncovered high-risk areas:

- Scope soundness. The S2 records prove the Kernel prefilter drops nothing
  the algebra would keep (completeness) and that the daemon bounds rows.
  No record states the other direction: every row the composite read serves
  has a scope that names the bound project (`ScopeFilter::matches` implies
  `names_project`; `None` scope is dropped at `project.rs:212-214`). The
  system model's own product lens names "cross-project leakage" as the S2
  failure, and `existing-checks.md` lists
  `rows_serve_only_to_the_project_their_scope_names`
  (`kernel_routes.rs:1803`) without a record. P5 R2 calls the Kernel API
  directly, so it must re-implement this filter; the property it must
  preserve is absent. Gap G1, high.
- Snapshot stability. Plan U3 requires that the table "can be truncated and
  rebuilt from live sources with identical results". That rests on
  `visible_domain_decisions_as_of_in_scope(requested)` returning the same set
  for the same `requested` after later commits, and on `tip` semantics. No
  record. Gap G2, medium.
- Publication identity. `Published.replayed` and `replaced_object_id`
  (`harness_sources.rs:521-532`): a replayed unit is the same descriptor
  (P5 would re-score it), and a new revision invalidates its predecessor in
  the same commit (P5 rows keyed on the predecessor are "descriptor retired"
  at once). No record. Gap G3, medium.
- Retirement observability. P5 R4's sweep must learn that a descriptor or
  evidence object is retired. No record states which read exposes
  `invalidated_commit_seq` for observations and evidence at tip, or what
  `visible_as_of_in_scope(Some(ids))` returns for a retired object. Gap G4,
  medium.
- Liveness. 0 liveness records. L1 (sweep progress) is folded into a safety
  record's open question. The consumer's confirm path depends on retirement
  happening on schedule only weakly, so this is low. Gap G5, low.

Dense coverage of lower-risk areas: three S4 records and three S3 records sit
on well-tested code (Exercised yes or partial for all six).

## 5. Lens: implementability

- `p5dep-kernel-domain-read-has-no-row-bound` names `read.rs:190` as the
  observation point. `visible.rows` is consumed by the `for` loop at `:192`
  with no intermediate binding, so observing its length in production needs
  a marker. The fixture at `kernel_routes.rs:1958` already builds the state;
  calling the Kernel API directly there and asserting `rows.len() > 8192`
  needs no production change. R6.
- `p5dep-response-never-waits-on-the-capture-job` asserts
  `t(after.run()) >= t(settle_prepared returned)`. Both are inside one async
  function (`lib.rs:13652-13665`); the ordering is sequential code, so the
  clock-based oracle is both trivially true and not the property. The
  property is that `after.run()` cannot wait: `into_job` uses
  `try_acquire_owned` (`source_capture.rs:728`), `spawn` is non-blocking, no
  kernel call occurs. R4 restates the check as a non-waiting clause plus the
  byte-identity oracle already tested.
- `p5dep-budget-exhaustion-stops-a-budgeted-kernel-read` depends on
  `hold_readers_for_test` (`open.rs:614`, `test-support`), so a default
  build can never exercise it; Exercised stays `partial` by construction.
  Fine, but the record should say so.
- `p5dep-cited-evidence-survives-the-harness-sweep` asserts
  `retire_evidence` returns `Conflict` on a cited row. From the sweep path
  this branch is unreachable: `retire_capture` returns `"retained"` at
  `local_file.rs:496-498` before `retire_evidence` is called. The clause is a
  guard property for other writers. R5.
- `p5dep-harness-sweep-retires-owners-before-evidence` asserts an
  intra-transaction ordering. The observable consequence is that an uncited
  capture ends `retired` (if `cited_elsewhere` ran first, the capture's own
  live descriptors would count as citations and every capture would be
  `retained`). The record should name that observable. R9 (folded into R5's
  edit list).
- F18 (`store_unavailable`) has no seam. The clause is currently untestable;
  record it as such rather than under `always`.
- All other records have constructible preconditions with existing seams
  (`now` parameter, `cfg(test)` hooks, raw SQL, fake transport).

## 6. Lens: wildcard

- W1 x W2 coupling (new). `p5dep-cited-evidence-survives-the-harness-sweep`
  shows a confirmed candidate keeps its evidence past 30 days. The sweep
  candidate query (`local_file.rs:551-562`) scans every live expired row of
  the class on every call and excludes those whose descriptors are retired
  and whose evidence is cited. Each confirmed candidate therefore adds a
  permanently scanned row for as long as the disputed decision lives.
  `expiry_candidate_cost_does_not_grow_with_retired_history`
  (`local_file.rs:1138`) covers retired history, not retained-and-cited
  history. Gap G6, low to medium: "sweep candidate cost under accumulated
  retained captures".
- Sensitivity inheritance. Harness descriptors publish as `Sensitive` and
  `LocalOnly` (context.md; `source_capture.rs:724-730`). P5 rows carry no
  text, but the `candidates` action tells the agent which descriptor
  overlapped which decision. No record states the egress class a derived row
  inherits from a `Sensitive` source. Gap G7, low; the new behavior is out
  of scope, but the implemented fact it depends on has no record.
- Surface visibility. P5 reads at `Surface::ExplicitSearch`, the widest
  surface (`admission.rs:3321`); rows `Hidden` there are excluded at `:3211`,
  `Labeled` rows are served with `labeled: true`. No record states which
  visibility the candidate set inherits. Folded into G2 as a clause.
- Framing check. The catalog treats "the P5 mechanism" as out of scope and
  catalogs only dependencies. That is the stated contract and it is kept;
  the records do not drift into specifying the new behavior. Seven records
  carry plan-correction leads (D1 to D7). That is useful spec input but it
  tilts the set toward plan review (B3).

## 7. Finding disposition

### Gaps (targeted discovery to run, then deduplicate)

| Id | Priority | Gap | Discovery target |
| --- | --- | --- | --- |
| G1 | high | Scope soundness of the composite read | `read_visible` and `ScopeFilter::matches` (`project.rs:186-222`): every served row's scope `names_project`; `None` scope dropped; `set` operator and `Dimension::Project` exact-match semantics; link `kernel_routes.rs:1803` |
| G2 | medium | Snapshot stability and visibility of the candidate read | `visible_domain_decisions_as_of_in_scope(requested)` set equality across later commits; `tip`/`known_as_of`; `Surface::ExplicitSearch` visibility rules (`admission.rs:3211`, `:3321`) |
| G3 | medium | Publication identity: replay and revision | `Published.replayed`, `replaced_object_id` (`harness_sources.rs:521-532`); same-commit predecessor invalidation; memo dedupe interaction (`source_capture.rs:5`) |
| G4 | medium | Retirement observability for a derived-row sweep | which Kernel read exposes `invalidated_commit_seq` for observations and evidence at tip; behaviour of `visible_as_of_in_scope(Some(ids))` on retired ids |
| G5 | low | Bounded liveness of the harness sweep (L1) | `ceil(pending_ahead / 64)` passes at `SAMPLE_RETRY_INTERVAL`; fault-free window with the lifecycle loop |
| G6 | low-medium | Sweep candidate scan cost under retained-and-cited captures (W1 x W2) | `expired_captures_sql` (`local_file.rs:558-572`) step count versus retained rows |
| G7 | low | Sensitivity and egress class of descriptor-derived data | `publish_batch` admission classes (`source_capture.rs:724-730`); what S7/S8 reads may carry |

Dispositions recorded by the unit tickets:

- G1 (#1167, bounded read): the pass applies the project scope term in SQL
  and then `ScopeFilter::matches`
  (`crates/daemon/src/kernel_routes/project.rs:207`), which drops a `None`
  scope, so every candidate names the bound project; recorded on
  `cc-candidate-set-bounded-with-counted-truncation`. The `set` operator
  semantics were not probed.
- G2 (#1167, bounded read): every pass read uses the one tip captured at the
  pass start, so the candidate set is one snapshot; no test commits between
  the pass's reads. Recorded on the same record.
- G3 (#1167, bounded read): a replayed unit is scored again (tested through
  the job); a revision corrects only the predecessor descriptor
  (`crates/kernel/src/source_descriptor.rs:546`), so the predecessor's rows
  stay until its evidence retires or the sweep's age bound removes them. No
  test publishes a revision. Recorded on `cc-row-key-unique-and-caps-enforced`.
- G4 to G6 (#1166): recorded on
  `cc-sweep-drops-only-dead-targets-bounded-idempotent-kernel-untouched`.
- G7 (#1165, #1168): the facts the decision rests on are recorded on the read
  record (a row holds ids, integers, and times only; the descriptor publishes
  `Sensitive` and `LocalOnly`; the read returns the row's closed key set to
  the model as a tool result); the inheritance decision stays open there
  (needs human input).

### Refinements (apply directly)

R1. S6 records (`p5dep-harness-sweep-retires-owners-before-evidence`,
`p5dep-cited-evidence-survives-the-harness-sweep`) and their index rows:
replace `default-production (path) / explicit-config-only (rows)` with
`Reachability: explicit-config-only - the sweep path runs in every default
build (lib.rs:4000-4008, lifecycle.rs:327) but the property's precondition,
a harness_capture row, exists only when source_capture_enabled was true
(config.rs:154).`

R2. `p5dep-decision-page-returns-an-oversize-decision-alone`: change
`Reachability` to `test-only - public Kernel API with no non-test caller
found at HEAD (kernel_slice.rs:1637-1753 only); P5 would be its first
production caller.` Update the index row.

R3. `p5dep-baseline-digest-change-closes-the-activation-gate`: replace
`open(file) is Ok ⇔ marker(file) == baseline_digest()` with
`marker(file) != baseline_digest() ⇒ open refused as StoreError::Baseline
with bytes unchanged; open Ok ⇒ marker(file) == baseline_digest()`.

R4. `p5dep-response-never-waits-on-the-capture-job`: Guarantee: replace
"encoded and handed to the wire writer before the capture job starts" with
"settled (encoded) before the synchronous handoff runs; the handoff
(`into_job` with `try_acquire_owned`, `admission.spawn`) performs no await,
no kernel call, and no waiting lock (`lib.rs:9297-9320`,
`source_capture.rs:616-619`); the job body runs on a spawned task". Check:
replace the `t(...) >= t(...)` clause with "the `AfterResponse` closure
contains no blocking or kernel operation" and keep the byte-identity clause.
Fix `:13391` to `:13392`.

R5. `p5dep-cited-evidence-survives-the-harness-sweep`: remove "and
`retire_evidence` on a cited row returns `Conflict`" from Check; add to
Existing check "guard `write.rs:606-608`, unreachable from the sweep path
because `retire_capture` returns `retained` first (`local_file.rs:496-498`);
route to `/low-level-systems:defensive-assertions-and-invariant-guards`".
Scope `count == "retained"` to "the first sweep that selects the capture;
later sweeps exclude it as a candidate (`local_file.rs:556-559`)". In
`p5dep-harness-sweep-retires-owners-before-evidence`, add to Check: "observed
through outcome: an uncited capture ends `retired`, which is possible only if
its own descriptors were retired before `cited_elsewhere` ran".

R6. `p5dep-kernel-domain-read-has-no-row-bound`: Guarantee: "The situation
'the Kernel returned more rows than the daemon keeps' is reachable at HEAD:
`visible_domain_decisions_as_of_in_scope` has no `max_rows`, `max_bytes`, or
`EvalBudget` parameter and materializes every matching row
(`admission.rs:3207-3220`)". Check: observe `rows.len() > MAX_READ_ROWS` on
the Kernel API's return value inside the over-cap fixture
(`kernel_routes.rs:1958`), not at `read.rs:190`. Fix the evidence file's
`read.rs:190-195`/`:191` to `:191-192`.

R7. Line corrections from section 1: `admission.rs:3668` to `:3661` and the
WHERE tail to `:3650-3664` (catalog and evidence for
`p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict`, system-model S2,
D5); `admission.rs:3574-3579` to `:3575-3580`; `read.rs:177` to `:178`,
`:190` to `:191`; `source_capture.rs:666` to `:667`; give full paths for
`tool-registry.ts` and `subagent-entry.ts`.

R8. `p5dep-budget-exhaustion-stops-a-budgeted-kernel-read`: split Check into
two labelled clauses: (A) `claim_facts_at_within_budget`: `Deadline` at the
next poll point after exhaustion, where poll points are acquisition retries,
`limit.check()` between claims, and the SQLite progress handler; (B)
`decision_page_within_budget` and `decisions_for_objects_as_of_within_budget`:
`Deadline` at the next acquisition retry only. Replace "within
`WRITER_ACQUIRE_POLL` of the deadline" with "at the first poll after the
deadline" (units: polls). Add to Exercised: "the held-pool seam is
`test-support` only, so a default build cannot exercise this record". In
`p5dep-review-reads-parse-strict-bodies-after-the-envelope`, mark the
`store_unavailable` clause "no seam at HEAD (F18); untestable until one
exists".

### Biases (present to a human with evidence)

B1. Existing-test gravity. 18 of 20 records sit on code that already has a
test (Exercised: yes 10, partial 8, not yet 2). The two `not yet` records
are the highest-information ones in the set, and the consumer risks with no
record (G1 to G4) are the ones with no or only indirect existing tests.
Judgment required: is this catalog a dependency inventory (then it is fit as
is) or a risk-driven guarantee set (then G1 to G4 run before the spec cites
it)?

B2. Safety-only orientation. 19 safety, 1 reachability, 0 liveness; no
`always-or-unreached`, `reachable`, or `unreachable`. The consumer's R1 and R4
depend on progress (a job reaches a terminal; a sweep retires on schedule).
Judgment required: accept zero liveness for a dependency catalog, or add L1
and "every `CaptureJob::run` returns one terminal within budget plus join"
as bounded liveness records.

B3. Plan-correction orientation. 7 of 20 records carry a D-lead and an open
question addressed to the P5 plan rather than to the implemented surface.
This is useful spec input and the leads are correct (D1, D5, D6, D7
re-verified here). Judgment required: confirm the spec resolves OQ5-1 to
OQ5-3 on its own evidence and does not treat these open questions as
resolved by the catalog.

## 8. What is already good

- Every lead D1 to D7 checked here is accurate at HEAD. D1 (`ClaimFacts`
  carries no text) and D2 (`decision_page_within_budget` is store-wide and
  its budget bounds only the acquire wait, `slice/read.rs:134`, `:139-146`)
  are the two facts the P5 spec most needs and the plan gets wrong.
- The one `sometimes` record is correctly a situation, not a location, and
  it is paired with the daemon bound without asserting the violation.
- The S6 pair (`retires-owners-before-evidence`, `cited-evidence-survives`)
  and the W1 finding are the catalog's best contribution: a cross-mechanism
  coupling (P4 `dispute` changes P5 retention outcomes) that neither plan
  names, with the exact SQL cited (`write.rs:42-54`).
- Evidence files are schema-complete (every file has the six sections and
  an investigation log), and `existing-checks.md` records explicit "none
  found" per surface.
- Fault-map leverage ranking is sound; rank 4 (the cited-evidence fixture)
  is the right first test to write.

## 9. Disposition (bundle at HEAD `3a3b916b7`)

Refinements applied in this bundle: R1 (both S6 records, their index rows,
and the reachability-classes note), R2 (applied with a correction: the bundle
pass found a production caller of `decision_page_within_budget` at
`crates/daemon/src/claim_sources.rs:802`, reached from
`crates/daemon/src/search_lifecycle_owner.rs:570`, so the record keeps
`default-production` and names that caller; the `test-only` label proposed
by R2 rests on a claim that does not hold at HEAD), R3, R4, R5 (both S6
records), R6 (catalog record and evidence file), R7 (catalog, system-model
S2 and D5, system-model and catalog full paths for `tool-registry.ts` and
`subagent-entry.ts`, scope-prefilter evidence; the `source_capture.rs:666`
correction had no remaining occurrence, `system-model.md` already reads
`:665-691`), R8 (budget record and review-reads record). `fault-map.md:42`
still names a probe at `read.rs:190`; R6 and R7 did not name the fault map,
so that row is left as written and is superseded by the R6 text in the
catalog record.

Gaps queued for the implementing ticket: G1 to G7. G1 facts verified in the
bundle pass: `ScopeFilter::matches` returns `false` for a `None` scope and
otherwise defers to `ProjectScope::names_project`
(`crates/daemon/src/kernel_routes/project.rs:207-220`); the record itself is
queued for the implementing ticket.

Biases surfaced to the specification owner: B1, B2, B3.
