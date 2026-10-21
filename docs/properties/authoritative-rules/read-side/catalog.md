# Catalog: proposed read side (N3 scope context, N4 packing, N5 harness)

## Scope

Proposed behavior of the settled plan
`docs/plans/2026-10-21-1902-feat-authoritative-rules-automatic-use-plan.md`
at HEAD `08de9051a735f136ff63a9dca49eaa11493b3006` on three surfaces:

- N3 the per-pass injection scope context and the match rule;
- N4 two-phase rules packing, disclosure, diagnostics, and the
  `USER_DIRECTIVES` mapping;
- N5 the harness directive command surface and cross-harness parity.

The deployable is the daemon with its embedded Kernel store. Every record
describes behavior that does not exist at HEAD; `Exercised` is therefore
`not yet` throughout and `Existing check` names only tests that pin a
mechanism the obligation reuses. Reachability is the class the path will
have once built. Line references are verified at HEAD.

Writer inventories here are proposed. Conflicts between the plan and the
code at HEAD are recorded in the records' open questions and in
`open-questions.md`; none is repaired here.

## Reachability classes

- `default-production`: on the m0 injection path of every transform pass of
  a bound project with `memory_enabled` (the default), or on the harness
  command surfaces that ship enabled.
- `explicit-config-only`: requires `/memory/injection_environment` to be
  set, or a non-default reserve.
- `test-only`: none in this pass.

## Index

| slug | type | reachability | status | existing check |
| --- | --- | --- | --- | --- |
| scope-context-is-resolved-per-pass-from-named-sources | safety | default-production | active | none |
| scope-rule-serves-only-when-every-carried-term-matches | safety | default-production | active | `crates/kernel/tests/kernel_scope_algebra.rs:325` |
| scope-directory-ancestors-derive-only-from-paths-inside-project-root | safety | default-production | active | none |
| packing-selection-never-admits-every-rendered-row-is-kernel-visible | safety | default-production | active | `crates/daemon/src/canonical_memory.rs:282` |
| packing-required-order-is-total-and-identical-across-passes | safety | default-production | active | none |
| packing-reserve-is-computed-once-after-wrapper-and-disclosure | safety | default-production | active | `crates/daemon/src/m0_compose.rs:300` |
| packing-required-phase-is-a-prefix-ending-at-first-overbudget | safety | default-production | active | `crates/retrieval/tests/packing_required.rs` |
| packing-first-rule-is-served-when-it-fits-budget-minus-wrapper | safety | default-production | active | none |
| packing-omitted-count-is-disclosed-reported-and-digested | safety | default-production | active | `crates/daemon/src/canonical_memory.rs:315` |
| packing-user-directive-renders-under-user-directives-by-source-kind | safety | default-production | active | `crates/daemon/src/canonical_memory.rs:282` |
| parity-directive-command-dispatch-is-host-user-only | safety | default-production | active | none |
| parity-directive-command-writes-only-after-ui-confirmation | safety | default-production | active | none |
| parity-identical-request-fields-yield-identical-rule-sets | safety | default-production | active | none |
| parity-pi-context-handler-registers-whenever-project-binds | reachability | default-production | active | none |
| parity-is-subagent-true-suppresses-m0-and-pi-sends-false | safety | default-production | active | none |

## Records

### scope-context-is-resolved-per-pass-from-named-sources

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - every transform pass of a bound project
with `memory_enabled` takes one canonical read
(`crates/daemon/src/lib.rs:9338-9345`); the context is built for that read.
Status: active
Exercised: not yet - no `InjectionScope` or `ScopeMatchContext` builder
exists on the injection path; `read_project_memory` takes only the binding
(`crates/daemon/src/canonical_memory.rs:141-146`).
Guarantee: The pass scope context is a value resolved once per pass from
exactly four sources: Project from the binding digest, Branch from the gix
HEAD symbolic referent of `project_root` (absent on detached or unborn HEAD
or any gix error), Environment from `/memory/injection_environment` (absent
when unset), and the Entity ancestor set from `cwd` and tool-call paths of
the live window; no ambient state and no value from a prior pass enters it,
and `context_resolved` reports exactly the resolved values.
Check: `always` - for every pass, `context_resolved.branch` equals the HEAD
referent short name at the pass or is null; `context_resolved.environment`
equals the config value or is null; `context_resolved.paths` equals the
derived ancestor set; and a second pass with a changed HEAD, config, `cwd`,
or window reflects the change with no residue from the first. `always`
because the predicate is evaluated at every read.
Fault/timing angle: HEAD moves between two passes (checkout, detach); config
reload between passes; the Branch cell cached within a pass must not outlive
it.
Required faults and enabling state: a bound project that is a git
repository; a pass on a branch, then `git checkout --detach`, then a pass;
the environment key set then unset between passes.
Confidence: medium - [evidence](evidence/scope-context-is-resolved-per-pass-from-named-sources.md).
`ScopeMatchContext` resolution-before-evaluation contract verified at
`crates/kernel/src/scope.rs:866-873`; the per-pass single read verified at
`lib.rs:9338-9345`; no existing daemon code resolves a symbolic branch name
(the cited pattern `git_reconcile.rs:434-491` resolves ref tips and object
traversal only).
Existing check: none.
Impact: A stale or ambient Branch or Environment lets a rule for another
branch or environment serve, or hides a rule that applies; a stale ancestor
set serves a directory rule for a directory no longer touched.
Open questions:
- Branch value form: full ref name (`refs/heads/main`) or short name
  (`main`)? Rule writers (deferred `--branch`) and the context must agree.
  (resolvable at implementation)
- Does the history-summarizer read (`lib.rs:6006`) build a `cwd`-only context
  and report its own `context_resolved`, or none? Plan OQ7 assumes
  `cwd`-only. (resolvable at implementation)

### scope-rule-serves-only-when-every-carried-term-matches

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - every row of the injection read is judged
(`crates/daemon/src/kernel_routes/read.rs:188-195`).
Status: active
Exercised: not yet - the read judges only `names_project`
(`project.rs:205`); Branch, Environment, and Entity terms are never compared
against a context on the injection path.
Guarantee: A rule row is a candidate for the block only when its scope
decodes, carries a Project term equal to the binding digest, and every other
term it carries matches the pass context with outcome `Matches`; a term whose
dimension the context did not resolve, a redacted term, or a term the
daemon cannot evaluate excludes the row exactly as `DoesNotMatch` does.
Check: `always` - for every row the read returned, `served(row)` implies
`names_project(terms)` and, for each present non-Project term, the daemon's
match outcome is `Matches`; the three forbidden outcomes (dir-scoped rule
served with no matching ancestor; branch-scoped rule served while
`context_resolved.branch` is null; any rule served whose term names a
dimension absent from the context) are asserted as `always(!X)` over the
served set because no dedicated code point marks them.
Fault/timing angle: detached HEAD at the pass; environment key unset; the
Entity set empty because `cwd` is absent and the window has no tool calls.
Required faults and enabling state: seeded rows with (a) Project only, (b)
Project plus Entity `dir:crates/storage`, (c) Project plus Branch `main`,
(d) Project plus Environment `prod`; passes under matching and
non-matching contexts for each.
Confidence: high - [evidence](evidence/scope-rule-serves-only-when-every-carried-term-matches.md).
Conjunction and `Uncertain` on an absent context value verified at
`crates/kernel/src/scope.rs:1417-1419, 1450-1468`; `names_project` requiring
`Matches` and a Project term verified at `crates/kernel/src/eligibility.rs:
85-94`; one term per dimension verified at `scope.rs:613-660`.
Existing check: `crates/kernel/tests/kernel_scope_algebra.rs:325`
`missing_context_value_for_constrained_dimension_is_uncertain` pins the
`Uncertain` outcome; status unaudited. It does not cover the daemon's
Entity ancestor iteration or the injection path.
Impact: A branch-, environment-, or directory-scoped rule reaches the model
as authoritative outside its scope, which is the "out-of-scope rules cannot
acquire authority" verification of the source brief.
Open questions:
- `ScopeMatchContext` holds one value per dimension (`scope.rs:869-873`),
  so Entity ancestors are matched by daemon iteration outside
  `scope_matches`. The `ScopeFilter` docstring claim that "a route and the
  kernel's own eligibility judgement cannot disagree about a scope"
  (`project.rs:177-180`) ceases to hold for Entity. Which function is the
  single oracle for a scope under a multi-valued dimension? (resolvable at
  implementation)
- Entity terms of operator `Range`, `VersionRange`, or `GitReachable` on a
  `dir:` value: excluded as unevaluable, or evaluated per ancestor?
  (resolvable at implementation)

### scope-directory-ancestors-derive-only-from-paths-inside-project-root

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - `cwd` and tool-call `input` JSON are
present on ordinary passes; `FlatBlock` carries `BlockKind::ToolCall {
input, .. }` (`crates/daemon/src/transform.rs:6874-6889`).
Status: active
Exercised: not yet - no path parsing of tool-call input or `cwd` exists; the
transform request has no `cwd` field (`docs/host-wire-protocol.md:953-976`).
Guarantee: The Entity ancestor set is exactly the set of `dir:<p>` values
where `p` ranges over `.` and every proper directory prefix (posix
separators, no `.` or `..` segments, no trailing slash) of each input path
made relative to `project_root`, for inputs drawn only from the request
`cwd` and from the JSON members `path`, `file_path`, `paths[]`, and `cwd`
of tool-call blocks in the live window; a path outside `project_root`, a
non-string member, or an unparseable value contributes nothing and is not an
error; an absent `cwd` contributes nothing.
Check: `always` - `derive_ancestors(project_root, cwd, window)` equals the
expected set on a fixture table including: `cwd` = root gives `{.}`;
`crates/storage/src/lib.rs` gives `{., crates, crates/storage,
crates/storage/src}`; `/elsewhere/x.rs` gives `{}`; `a/../b` normalizes to
`{., b}` or is dropped (one choice, pinned); Windows separators normalize;
duplicate paths add nothing. `always` because the derivation is total and
runs on every pass.
Fault/timing angle: none for the derivation itself; the window a pass sees
is the live window, so a path dropped by an earlier fold no longer
contributes.
Required faults and enabling state: a window whose tool-call input is not an
object, is nested, or exceeds normal size; a `cwd` that is a symlink into
or out of `project_root`.
Confidence: medium - [evidence](evidence/scope-directory-ancestors-derive-only-from-paths-inside-project-root.md).
Tool-call input is a `serde_json::Value` on `BlockKind::ToolCall`
(`crates/daemon/src/wire.rs:478-494` constructs one); the sanitizer's
file-path extraction is not reused by the plan, and no existing helper
derives ancestors.
Existing check: none.
Impact: A path outside the project that is accepted would let a rule of
this project be chosen by a foreign path; a missing ancestor hides a
directory rule for a touched directory; a non-canonical form breaks the
`Exact`-value match against `dir:` terms written by N1 and N2.
Open questions:
- Symlink policy: is `cwd` canonicalized before the relative computation,
  as `ProjectBinding.root` is (`project.rs:22-26`)? (resolvable at
  implementation)
- Are paths inside tool results or text ever read? The plan says tool-call
  input only; a test must pin that a path appearing only in a tool result
  contributes nothing. (resolvable at implementation)

### packing-selection-never-admits-every-rendered-row-is-kernel-visible

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - the injection read runs on every pass.
Status: active
Exercised: not yet - the two-phase packer does not exist; the existing
single-phase trim is pinned by tests but the required phase is new.
Guarantee: Every row the block renders, required or optional, was returned
by `read_visible` on `Surface::AutoInject` at the pass snapshot with
`visibility == Visible`, so the required phase narrows the Kernel's set and
never adds to it; a `model_inference` row that is Hidden on AutoInject is
absent whatever its `source_kind` or text, and a label-bearing row never
appears in a block that has no label renderer.
Check: `always` - the set of object ids in the rendered block is a subset of
`{row.object.object_id : row in read.rows, row.visibility == Visible}`; a
seeded `(model_inference, user_inferred)` row with directive-shaped text is
absent; `rules_served` counts only rows in that subset. `always` because the
subset relation is checked per pass.
Fault/timing angle: a row admitted between the read and the render is not
in the snapshot and is absent; a row revoked after the read is still
rendered for this pass (snapshot semantics, disclosed by `known_as_of`).
Required faults and enabling state: a Hidden row with `source_kind`
`user_directive` (seeded by a test through the Kernel with model classes);
a `review_only` labeled row.
Confidence: high - [evidence](evidence/packing-selection-never-admits-every-rendered-row-is-kernel-visible.md).
`injectable_snapshot` keeps only `Visible` rows
(`crates/daemon/src/canonical_memory.rs:189-198`); `judge` returns `Hidden`
when the served visibility is Hidden (`crates/kernel/src/eligibility.rs:
200-238`).
Existing check: `crates/daemon/src/canonical_memory.rs:282`
`injectable_rows_are_visible_decisions_in_a_positive_category`; status
unaudited. Covers today's single-phase filter, not the required phase.
Impact: A selection that admits is a second admission authority; a Candidate
rule would reach the model as authoritative (issue 657: a batch verdict
never grants AutoInject).
Open questions: None.

### packing-required-order-is-total-and-identical-across-passes

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no partition by `source_kind` and no specificity key
exist; `ObjectRow` exposes `source_kind` and `created_commit_seq`
(`crates/kernel/src/envelope.rs:155-158`) but `CanonicalMemory` carries only
`object_id`, `category`, `content`.
Guarantee: Required rows are ordered by the strict total order (channel
rank: `user_directive` before `repository_policy`; specificity: count of
non-Project terms matched desc, then `dir:` depth desc; `created_commit_seq`
desc; `object_id` asc), so two passes, on either harness, over the same
snapshot and context produce the same sequence and the same served prefix.
Check: `always` - the comparator is antisymmetric and transitive on a
generated set (property test), never returns `Equal` for distinct object
ids, and the served prefix of two packs of the same inputs is identical;
`sometimes` a user_directive row younger than a repository rule still
precedes it. `always` for the order; the campaign marker records that the
rank-over-recency case occurred.
Fault/timing angle: none; the order is intrinsic to the rows and context.
`HashMap` iteration order in the read (`decisions` map) must not leak into
the order.
Required faults and enabling state: rows with equal `created_commit_seq`
(one commit inserting several rules); rows of equal specificity across
channels.
Confidence: high - [evidence](evidence/packing-required-order-is-total-and-identical-across-passes.md).
The existing serving order is `created_commit_seq` desc then `object_id`
(`read.rs:102-111`); `decisions` is a `HashMap` (`read.rs:239-243`).
Existing check: none.
Impact: Nondeterministic order makes the served prefix differ between passes
and harnesses, breaking parity and causing a rule to flicker in and out of
the block across turns.
Open questions:
- Specificity when the Branch or Environment term matched: does one matched
  non-directory term outrank a `dir:` term of depth 3, or is depth compared
  first within the same count? The plan says count then depth.
  (resolvable at implementation)

### packing-reserve-is-computed-once-after-wrapper-and-disclosure

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - reserve default is proposed as 1600 of
4000 (plan OQ6); a zero or absent reserve still runs the arithmetic.
Status: active
Exercised: not yet - `trim_memories_to_budget` reserves only the wrapper
(`crates/daemon/src/m0_compose.rs:79-83`).
Guarantee: The f64 budget is converted to an integer exactly once (floor,
minimum 1); the wrapper tokens and the disclosure-line tokens are subtracted
first with saturating arithmetic; the required limit is
`min(rules_reserve, remaining)`; the optional phase receives `remaining -
charged_required`; no later step re-reads the f64 or re-floors.
Check: `always` - for budget `B`, wrapper `w`, disclosure `d`, reserve `r`,
`required_limit == min(r, saturating_sub(floor(max(B,1)), w + d))` and
`optional_budget == floor(max(B,1)) - w - d - charged_required`; a budget
below `w` yields an empty block; a budget below `w + d` yields no rules and
no disclosure line. `always` because the arithmetic runs every pass.
Fault/timing angle: none.
Required faults and enabling state: budgets of `0`, `NaN`, `w - 1`, `w + d
- 1`, `w + d`; reserve larger than the budget; reserve zero.
Confidence: high - [evidence](evidence/packing-reserve-is-computed-once-after-wrapper-and-disclosure.md).
Existing conversion `budget.floor() as u64 - wrapper as u64` after a
`wrapper > budget` guard verified at `m0_compose.rs:79-90`; config key
clamps to `max(1.0)` at `config.rs:1198-1201`.
Existing check: `crates/daemon/src/m0_compose.rs:300`
`budget_boundaries_match_the_replaced_loop` and `:337` proptest; status
unaudited. They pin today's wrapper-only reservation.
Impact: A second conversion or a reserve applied before the disclosure makes
the disclosure line a victim of the budget, or over-admits by the rounding
difference.
Open questions:
- Is `d` reserved even when no rule exists (so an all-optional block loses
  `d` tokens), or only when at least one required candidate exists?
  (resolvable at implementation)

### packing-required-phase-is-a-prefix-ending-at-first-overbudget

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no required phase exists on the memory path.
Guarantee: The served required rows are a prefix of the ordered required
list: the phase admits rows in order while the cumulative untruncated cost
stays within the required limit and ends at the first row that does not
fit; no later required row is admitted in the required phase, and omitted
required rows do not enter the optional phase.
Check: `always` - `served_required == ordered_required[..k]` for the largest
`k` with `sum(cost[..k]) <= limit`; `unreachable` for a code point that
admits a required row at index `j > k` (the required loop has no `skip`
branch, so a skip-and-continue call over required rows is the forbidden
location). `always` for the prefix relation; `unreachable` for the skip
branch because the design names the branch that must not exist.
Fault/timing angle: none.
Required faults and enabling state: nine rules whose costs exceed the limit
with a small rule after the first oversize one (the skip-and-continue
temptation).
Confidence: medium - [evidence](evidence/packing-required-phase-is-a-prefix-ending-at-first-overbudget.md).
`reserve_required` is all-or-nothing: `OverBudget` is an `Err` and nothing
is reserved (`crates/retrieval/src/packing/required.rs:176-205`); a direct
call over the whole required set therefore serves zero rules when any one
overflows, which contradicts the plan's prefix semantics unless the packer
calls it per prefix or implements the loop itself.
Existing check: `crates/retrieval/tests/packing_required.rs`
`every_verdict_maps_to_exactly_one_failure_class` and
`the_load_bound_is_checked_over_the_whole_set_before_any_request_fault`;
status unaudited. They pin the primitive's all-or-nothing contract, not the
prefix rule.
Impact: Skipping in the required phase is a hidden priority inversion: a
lower-ranked rule displaces a higher-ranked one, so the model receives the
less authoritative rule while the disclosure count hides which one was
lost.
Open questions:
- The plan says required rows are charged "through `admit_required` /
  `reserve_required`"; those functions take retrieval
  `SelectedOccurrence`/`RequiredFact` with `OccurrenceId` and `Disposition`,
  not memory `DecisionRow`s, and reserve all-or-nothing. Does U2 adapt rows
  to those types and call per prefix, or write a memory-specific loop that
  shares only `TokenCount`? `crates/retrieval/AGENTS.md:71-74` is slated for
  update either way. (resolvable at implementation)

### packing-first-rule-is-served-when-it-fits-budget-minus-wrapper

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - binds whenever the first rule's cost
exceeds `min(rules_reserve, remaining)` but not `budget - wrapper`.
Status: active
Exercised: not yet.
Guarantee: When the ordered required list is non-empty and its first row's
untruncated cost is at most `floor(budget) - wrapper`, that row is served
even if it exceeds the required limit; a first row larger than
`floor(budget) - wrapper` is not served and the block renders only the
disclosure line (count 1 plus any further required rows).
Check: `always` - with `limit < cost(first) <= floor(B) - w`, `served ==
[first]` and `rules_omitted == n - 1`; with `cost(first) > floor(B) - w`,
`served == []`, `rules_omitted == n`, and the block is exactly the wrapper
around `<omitted-rules count="n"/>`. `always` because the guard runs every
pass with a non-empty required list.
Fault/timing angle: none.
Required faults and enabling state: one rule of about 3,900 tokens under a
4,000 budget and a 1,600 reserve; one rule of 5,000 tokens.
Confidence: medium - [evidence](evidence/packing-first-rule-is-served-when-it-fits-budget-minus-wrapper.md).
The existing minimum-service analogue is absent: `trim_memories_to_budget`
returns an empty set when the wrapper alone exceeds the budget
(`m0_compose.rs:81-83`) and otherwise skips the oversize row.
Existing check: none.
Impact: Without minimum service a reserve smaller than one long directive
hides the highest-ranked rule forever; without the upper bound the block
overruns the memory budget.
Open questions:
- When the first rule is served under minimum service and consumes more than
  the reserve, does the optional phase receive `remaining - cost(first)`
  (possibly zero) or is the disclosure line's reservation also consumed?
  (resolvable at implementation)

### packing-omitted-count-is-disclosed-reported-and-digested

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - `render_memory_block` emits only category elements
(`crates/daemon/src/memory_render.rs:157-208`); the digest covers rows only
(`canonical_memory.rs:49-67`); `project_memory` diagnostics carry
`known_as_of`, `truncated`, `revision` (`crates/memory-store/src/lib.rs:
2165-2181`).
Guarantee: `rules_omitted` equals the number of required candidates not
served; when it is positive the block contains exactly one line
`<omitted-rules count="N"/>` with `N` the decimal count, placed after the
last category element and before `</project-memory>`; when it is zero no
such line appears; `project_memory.rules_omitted` equals `N`,
`project_memory.rules_served + rules_omitted` equals the required candidate
count, and the snapshot `revision` changes when `N` changes with the same
rows.
Check: `always` - the block matches the grammar
`<project-memory>(\n<CAT>...\n</CAT>)*(\n<omitted-rules count="[1-9][0-9]*"/>)?\n</project-memory>`;
the count in the line equals `rules_omitted`; `revision(rows, N1) !=
revision(rows, N2)` for `N1 != N2`; `revision(rows, 0)` equals today's
digest only if the design chooses that (open question). `always` because
each pass renders once.
Fault/timing angle: none in the pass; across passes, the m1 revision signal
must move when `N` changes so a cached block is re-rendered.
Required faults and enabling state: required candidates over the limit; a
context change that moves a rule from served to omitted without changing
the row set.
Confidence: medium - [evidence](evidence/packing-omitted-count-is-disclosed-reported-and-digested.md).
Digest inputs and the `truncated` exclusion verified at
`canonical_memory.rs:42-67`; both composers render the block from the same
pinned rows (`m0_compose.rs:239-243`, `transform.rs:2826-2830`) so the line
must be produced by the shared renderer or by both callers.
Existing check: `crates/daemon/src/canonical_memory.rs:315`
`revision_changes_only_when_the_rendered_inputs_change` and `:373`
`rows_the_renderer_drops_do_not_move_the_revision`; status unaudited. They
pin the current digest inputs, which the design extends.
Impact: Silent omission violates "omission is disclosed, never silent"; a
digest that ignores `N` lets a block with a changed disclosure be served
from cache as unchanged.
Open questions:
- Is the disclosure line escaped and parsed by any consumer, or display
  only? If a harness strips unknown elements, parity on "disclosure lines"
  must compare daemon output, not provider-visible text. (resolvable at
  implementation)
- Does the digest version string change (`eidnara-project-memory-revision-v1`
  to `v2`) so that `N == 0` also yields a new revision for existing rows, or
  is `N` appended only when positive to keep existing revisions stable?
  (resolvable at implementation)

### packing-user-directive-renders-under-user-directives-by-source-kind

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - `injectable_snapshot` maps `category` from
`decision_kind` only (`canonical_memory.rs:193-199`); an `adr_accepted` row
is dropped today because `adr_accepted` is not a positive category
(`memory_render.rs:67-80`).
Guarantee: A Visible decision row with `source_kind == user_directive`
renders under the `USER_DIRECTIVES` heading regardless of its
`decision_kind`; every other row renders under its `decision_kind` and only
when that kind is a positive category; an `adr_accepted` row whose
`source_kind` is not `user_directive` is not rendered; repository rules
render under `PROJECT_RULES` because N2 writes `decision_kind =
PROJECT_RULES`.
Check: `always` - `category(row) == USER_DIRECTIVES` iff `row.source_kind ==
"user_directive"`; a row with `decision_kind == adr_accepted` and
`source_kind != user_directive` is absent from the block; a
`model_inference` row with `source_kind == "user"` and directive-shaped text
is absent (it is Hidden, so this record's rule is not even reached).
`always` because the mapping is applied per row per pass.
Fault/timing angle: none.
Required faults and enabling state: seeded Visible rows: (`adr_accepted`,
`user_directive`), (`adr_accepted`, `repository_policy`), (`PROJECT_RULES`,
`repository_policy`), (`CONSTRAINTS`, `user_directive`).
Confidence: high - [evidence](evidence/packing-user-directive-renders-under-user-directives-by-source-kind.md).
`USER_DIRECTIVES` is already a positive category at index 5
(`memory_render.rs:67-80`); render order places it after the five ordered
categories (`memory_render.rs:85-109`).
Existing check: `crates/daemon/src/canonical_memory.rs:282`
`injectable_rows_are_visible_decisions_in_a_positive_category`; status
unaudited. It pins the `decision_kind` filter the design partly replaces.
Impact: A directive rendered under a model category, or a non-directive
`adr_accepted` row rendered as a directive, misstates authority to the
model; the category text carries no authority but the heading is what the
model reads.
Open questions:
- A `user_directive` row whose `decision_kind` is a positive category other
  than `adr_accepted` (none is written by N1, but the Kernel does not forbid
  it): rendered under `USER_DIRECTIVES` by `source_kind` per the plan. Should
  the required partition also require `decision_kind == adr_accepted` for
  the directive channel? (needs human input)

### parity-directive-command-dispatch-is-host-user-only

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production - the command registers on both harnesses
when the plugin loads in a top-level session.
Status: active
Exercised: not yet - no `/eidnara-directive` command exists; the pattern
`/eidnara-memory-mark` exists on both harnesses
(`packages/pi-plugin/src/commands/eidnara-memory-mark.ts:28`,
`packages/opencode-plugin/src/hooks/context/command-handler.ts:392-422`).
Guarantee: The directive command handler runs only from host user input: on
Pi through `AgentSession.prompt()` with command expansion (interactive or
RPC prompt), never from `pi.sendUserMessage` (pinned 0.80.2 passes
`expandPromptTemplates: false`), never from queued steer or follow-up text
(the queue throws for an extension command), and never in a subagent child
process (the child entry returns before registration); on OpenCode only
through `command.execute.before`, which the host fires for user-typed
commands; no model tool exposes command dispatch, and the model tool
client never constructs a `directive.*` method.
Check: `unreachable` - the directive route handler is never entered with a
caller whose derived class is `model_inference` or whose request did not
originate from the host command hook; asserted at the daemon as a marker on
the `directive.*` dispatch path keyed by the route's bound harness and the
absence of any tool-client code path that names `directive.`; `always` at
the plugin level that `grep` over the model tool client finds no
`directive.` literal. `unreachable` because the design names a code point
(directive route entered from a tool path) that must not execute.
Fault/timing angle: a Pi upgrade past 0.80.2 (1.3.0 adds an
`expandPromptTemplates` option to `sendUserMessage`) lets another extension
dispatch the command programmatically; the confirm gate is the covering
control.
Required faults and enabling state: a model tool call `eidnara_memory` with
directive text (AE2); a Pi extension calling
`pi.sendUserMessage("/eidnara-directive add x")`; a `@gotgenes/pi-subagents`
in-process child session; an `EIDNARA_PI_SUBAGENT=1` child.
Confidence: medium - [evidence](evidence/parity-directive-command-dispatch-is-host-user-only.md).
Pi 0.80.2 dispatch verified at
`node_modules/.bun/@earendil-works+pi-coding-agent@0.80.2+72022b8d6e175469/node_modules/@earendil-works/pi-coding-agent/dist/core/agent-session.js:708-722,
839-845, 963-969, 1020-1048`; child-entry early return at
`packages/pi-plugin/src/index.ts:332-337`; OpenCode hook at
`command-handler.ts:487-500`.
Existing check: none.
Impact: A model-reachable directive command is a second admission authority
for `explicit_user` rows; the whole trust argument of R1 rests on this.
Open questions:
- In-process Pi child sessions (`@gotgenes/pi-subagents`) re-run the
  extension factory (`index.ts:338-340`); does the directive command register
  in those children, and if so is a child's `ctx.ui.confirm` a user dialog?
  (needs human input)

### parity-directive-command-writes-only-after-ui-confirmation

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the memory-mark runner accepts a `--yes` flag in place
of a dialog (`memory-mark-command.ts:30-32, 135-137`); the plan requires the
directive runner to refuse without a dialog.
Guarantee: The directive runner calls `directive.record`, `directive.revise`,
or `directive.revoke` only after `directive.preview` succeeded and a UI
confirmation callback returned `true` for that preview; with no UI
(`ctx.hasUI` false on Pi, or any OpenCode path lacking a dialog) the runner
records nothing and answers that confirmation is unavailable; a textual
confirm flag never substitutes for the dialog; the actor string is exactly
`user:pi` or `user:opencode`.
Check: `always` - in the runner's unit tests, every call sequence that
reaches `record|revise|revoke` contains a prior `confirm -> true` for the
same preview; an invocation without `confirm` yields a `needs_ui` outcome
and zero kernel-client calls past `preview`; `actor` on every wire body is
one of the two literals. `always` because the guard is evaluated on every
invocation.
Fault/timing angle: session replaced between confirm and commit
(`isCancelled`); the preview and the confirm are separated by user time
during which the candidate row (for `confirm <object_id>`) can move.
Required faults and enabling state: Pi with `ctx.hasUI == false`; OpenCode
(no dialog on the command path); a `--yes`-style argument.
Confidence: medium - [evidence](evidence/parity-directive-command-writes-only-after-ui-confirmation.md).
Pi passes `confirm` only when `ctx.hasUI`
(`eidnara-memory-mark.ts:66-75`); OpenCode's `executeMemoryMark` passes no
`confirm` and relies on the flag (`command-handler.ts:389, 411-417`).
Existing check: none.
Impact: Without the gate, a queued or scripted text could record an
`explicit_user` row with no human in the loop.
Open questions:
- OpenCode's command path has no dialog at HEAD (`command-handler.ts:389`).
  The plan's U4 step 4 ("refuse to run without a UI confirm") is
  unsatisfiable there with the existing pattern. Options: accept the
  two-step reply with a confirm flag on OpenCode (weaker), find an OpenCode
  dialog API, or scope directive authoring to Pi. (needs human input)

### parity-identical-request-fields-yield-identical-rule-sets

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - no `cwd` field, no rules, no `rules_served`.
Guarantee: For one bound project, one Kernel snapshot, and two transform
requests whose `cwd`, window tool-call paths, and `is_subagent` are equal,
the daemon's m0 rule object-id set, disclosure line, `rules_served`, and
`rules_omitted` are equal whichever `serializer_profile` (`pi`,
`opencode-aisdk`) carried them; neither plugin filters or reorders rule
rows after receipt.
Check: `always` - `extractM0(pi_pass).rule_ids == extractM0(oc_pass).rule_ids`
and the disclosure lines are byte-equal and `project_memory.rules_served`
equal; `sometimes` the negative control (one pass without `cwd`) differs and
the difference is attributable in `context_resolved.cwd`. `always` for the
equality; `sometimes` so the campaign proves the oracle can see a
difference.
Fault/timing angle: a commit between the two passes changes the snapshot;
the test pins `known_as_of` equality or seeds before both passes.
Required faults and enabling state: one directive and one repository rule
seeded; both harness fixtures bound to the same root; identical `cwd` and
one tool call naming a nested path.
Confidence: medium - [evidence](evidence/parity-identical-request-fields-yield-identical-rule-sets.md).
Selection lives in the daemon (`canonical_memory.rs:141-212`) and both
composers read the same pinned rows; the profiles differ only in codec
derivation (7.10.3 "Native derivation").
Existing check: none.
Impact: Divergent rule delivery means one harness's agent works under a rule
the other never sees, which is the source brief's "consistent eligibility
and delivery" verification.
Open questions:
- The Pi codec and the OpenCode codec derive tool-call `input` differently
  for some block shapes (`codec-b-opencode-hides-four-part-types-from-every-
  transform-decision`, `docs/properties/daemon/decisions/catalog.md:1242`);
  if a path member lives in a part one codec drops, the ancestor sets differ
  for "the same" window. Parity must be stated over the derived CK window.
  (resolvable at implementation)

### parity-pi-context-handler-registers-whenever-project-binds

Origin: proposed design obligation (not implemented at HEAD)

Type: reachability
Reachability: default-production - Pi with `PI_TRANSFORM_AVAILABLE` and a
bound project.
Status: active
Exercised: not yet - `createPiTransform` is constructed only when
`!compactionOff` (`packages/pi-plugin/src/index.ts:565`) and the handler
returns early when `!isCompactionEnabled(deps.config)` (`index.ts:572`).
Guarantee: Pi registers the `context` handler and runs a transform pass for
every top-level session whenever `PI_TRANSFORM_AVAILABLE` and the project
binds; a configuration that withdraws fold authority leaves the daemon on
its additive path, which still composes m0 with `<project-memory>` on a
`Hard` pass and serves it on later passes; compaction gates only the fold.
Check: `reachable` - with compaction off, the Pi provider request of a
top-level session contains `<project-memory>` carrying the seeded rule;
`always` that `tool_present` sent on such a pass reflects whether the reduce
tool is registered. `reachable` because this is location coverage of the
registration under the compaction-off configuration.
Fault/timing angle: `/cd` config changes after boot (Pi registers once per
process, `index.ts:381`).
Required faults and enabling state: `compaction.enabled: false` in Pi
config; a seeded Visible rule; a top-level session.
Confidence: medium - [evidence](evidence/parity-pi-context-handler-registers-whenever-project-binds.md).
Additive composition verified at `crates/daemon/src/transform.rs:3341-3342,
3052`; served m0 slot at `transform.rs:11723-11728`.
Existing check: none.
Impact: Compaction-off Pi users receive no rules, so parity fails for a whole
configuration class (plan OQ4).
Open questions:
- `compactionOff` also drives `setEidnaraReduceRegisteredGlobally` and the
  handler sends `reduceRegistered: true` unconditionally (`index.ts:383,
  593`); after the split the pass must send `tool_present: false` when the
  reduce tool is not registered, or the daemon's CC-U1 and nudge logic sees a
  tool that does not exist. (resolvable at implementation)

### parity-is-subagent-true-suppresses-m0-and-pi-sends-false

Origin: proposed design obligation (not implemented at HEAD)

Type: safety
Reachability: default-production
Status: active
Exercised: not yet - the suppression exists (`transform.rs:11723-11733`) and
Pi sends `false` (`pi-pass-inputs.ts:110`), but the documented asymmetry and
the parity statement conditioned on `is_subagent` do not exist in
`docs/host-wire-protocol.md`.
Guarantee: A pass with `is_subagent: true` serves no m0 slot and so no rule
block; Pi sends `is_subagent: false` on every pass, so in-process Pi child
sessions receive rules while OpenCode host-reported subagents do not; the
wire document states this beside the `is_subagent` request field, and
parity (previous record) is asserted only for equal `is_subagent`.
Check: `always` - a pass with `is_subagent: true` has no served slot whose
payload contains `<project-memory>`; every Pi pass body carries
`is_subagent: false`; the wire document's 7.10.3 table row for `is_subagent`
names the Pi asymmetry. `always` because the serving gate runs every pass.
Fault/timing angle: none.
Required faults and enabling state: an OpenCode subagent session (host
reports `is_subagent: true`); a Pi `@gotgenes/pi-subagents` child.
Confidence: high - [evidence](evidence/parity-is-subagent-true-suppresses-m0-and-pi-sends-false.md).
Suppression verified at `transform.rs:11723-11733`; Pi constant verified at
`pi-pass-inputs.ts:110`; OpenCode sources it from host pass inputs
(`transform-pass-fields.ts:66`).
Existing check: none found for the m0 suppression under `is_subagent`
(grep for `is_subagent` in `crates/daemon/tests` finds only fixtures).
Impact: An undocumented asymmetry is read as a parity bug; a Pi child that
should not see project rules (if the user expects subagent isolation) sees
them.
Open questions:
- Is the Pi asymmetry acceptable for rules carrying `Normal` sensitivity
  only, or should Pi derive `is_subagent` at `newSession` via
  `parentSession` and carry it per session? The plan documents rather than
  fixes. (needs human input)

## Relationship map

- `scope-context-is-resolved-per-pass-from-named-sources` feeds
  `scope-rule-serves-only-when-every-carried-term-matches` (the context is
  the right-hand side of every match) and
  `parity-identical-request-fields-yield-identical-rule-sets` (equal fields
  give equal contexts).
- `scope-directory-ancestors-derive-only-from-paths-inside-project-root` is
  the Entity component of the context; its canonical form must equal the
  `dir:` values N1 and N2 write (handoff to the representation pass).
- `packing-selection-never-admits-every-rendered-row-is-kernel-visible` is
  upstream of every packing record: the required partition is taken over the
  Visible, scope-matched set only.
- `packing-required-order-is-total-and-identical-across-passes` and
  `packing-reserve-is-computed-once-after-wrapper-and-disclosure` are inputs
  to `packing-required-phase-is-a-prefix-ending-at-first-overbudget`, whose
  guard exception is
  `packing-first-rule-is-served-when-it-fits-budget-minus-wrapper`.
- `packing-omitted-count-is-disclosed-reported-and-digested` consumes the
  prefix length; its digest extension is what makes a changed disclosure
  observable to the m1 revision signal.
- `packing-user-directive-renders-under-user-directives-by-source-kind`
  relies on N1 writing `source_kind = user_directive` and on the
  `adr_accepted` decision kind being excluded for other source kinds.
- `parity-directive-command-dispatch-is-host-user-only` and
  `parity-directive-command-writes-only-after-ui-confirmation` are the
  harness half of R1; the daemon half is in the N1 pass.
- `parity-pi-context-handler-registers-whenever-project-binds` and
  `parity-is-subagent-true-suppresses-m0-and-pi-sends-false` bound the
  domain over which `parity-identical-request-fields-yield-identical-rule-
  sets` is asserted.
- Existing catalogs touched by the plan:
  `docs/properties/selection-packing/required/catalog.md`
  (`packing-required-phase-precedes-optional-work`,
  `packing-budget-is-an-integer-never-clamped`,
  `packing-required-bytes-are-charged-untruncated`) are the primitive-level
  siblings of the packing records here;
  `docs/properties/compression-fidelity/delivery/catalog.md`
  (`cf-memory-credit-requires-admitted-visible-content`) is the
  provider-boundary sibling of the visibility record;
  `docs/properties/canonical-positive-claim-projection`
  (`bound-project-scope-cannot-be-widened-by-candidate`) is the Project-term
  sibling of the scope records.
