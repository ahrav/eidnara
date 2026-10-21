# Property catalog: existing packing primitives, m0 injection path, and harness surfaces (S3, S4, S5)

## Scope and provenance

- System: `/local/home/ahrav/scratch/eidnara` at
  `08de9051a735f136ff63a9dca49eaa11493b3006` (the plan's baseline). Date:
  2026-10-21. Method: `docs/properties/METHOD.md` and
  `/testing:property-discovery-and-catalog`.
- Scope (user-confirmed through the shared brief, 2026-10-21): implemented
  behavior on S3 (`crates/retrieval/src/packing/required.rs`, `scan.rs`), S4
  (`crates/daemon/src/canonical_memory.rs`, `m0_compose.rs`,
  `memory_render.rs`, `transform.rs` diagnostics and tool-call blocks,
  `crates/memory-store/src/lib.rs` `ProjectMemoryComposition`,
  `crates/daemon/src/lib.rs` read call sites), and S5 (memory-mark command
  runners, Pi `index.ts` registration, pass-input builders, the shared
  transform client, e2e `memorySeed` and `extractM0`). Bounded to behavior
  the settled plan depends on or extends. N1..N5 are another pass's scope;
  guarantees the plan relies on are cataloged here as claims under test.
- Assumption recorded: "existing behavior" means the code at HEAD. Where the
  plan or a catalog describes a different behavior, both are cited and the
  disagreement is a lead, never resolved toward the document.
- No repository file was edited; no cargo or bun command was run; no tracker
  write occurred.

## External evidence consulted (with why)

| Reference | Why consulted | Outcome |
| --- | --- | --- |
| `docs/plans/2026-10-21-1902-feat-authoritative-rules-automatic-use-plan.md` | Names the surfaces, KTD5..KTD8, and the existing guarantees U2 and U6 reuse | Nine dependency claims extracted (`_lenses/claimed-guarantees-and-leads.md` P1..P9) |
| `docs/properties/METHOD.md`, `AGENTS.md`, `README.md` | Record schema, reachability rule, prose rules | Applied |
| `docs/properties/selection-packing/required/catalog.md` | Owns `admit_required`, `reserve_required`, `OverBudget`, integer budget | Three records reused, none duplicated |
| `docs/properties/selection-packing/grouping/catalog.md` | Owns `skip_and_continue` and names the memory trim as its production caller | One record reused |
| `docs/properties/daemon/transform/catalog.md` | Owns the withheld-versus-empty composition record | One record reused |
| `docs/properties/daemon/decisions/catalog.md` | Owns the injection budget key and its `.max(1.0)` clamp | One record reused |
| `docs/properties/daemon/rendering/catalog.md` | Checked for memory-block records | None specific to the block; `is_subagent` gates for hints and reclaim noted as related |
| `docs/properties/compression-fidelity/delivery/catalog.md` | Owns memory credit at the provider boundary and the 64 KiB cut | One record reused |
| `docs/properties/opencode-plugin/transform-edit-responses/catalog.md` | Checked for response-type records | Not applicable to `project_memory`; not reused |
| `docs/host-wire-protocol.md:951-966, 1032` | Transform request fields, unknown-field rule, response diagnostics list | Unknown fields ignored is documented and tested; `project_memory` shape is undocumented (lead L7) |
| `docs/memory-reviewer-operations.md` | Named by the brief | Out of this pass's surfaces; nothing extracted |
| Tracker #307, #314, #453 (`gh issue view --comments`) | Contracts for m0 composition, the host memory-mark command, and serving order | Acceptance rows match the integration tests; no defect to promote |
| Tracker #579, #657, #408, #709 | Named by the brief as settled design | Bound N1..N5; nothing extracted for S3..S5 |
| Tracker-wide `gh issue list --state all --search` for memory injection, project memory, admission, scope matching, packing budget, harness parity, injection budget, is_subagent, memory-mark, trim_memories, canonical memory | Bugs or incidents as leads | No open bug or incident about these surfaces; search results listed in `_lenses/tests-history-assumptions-wildcard.md` |
| `git log` over S4 and S5 files | Bug history | Performance and compaction-handoff commits only |

## Reachability classes

Labels are per record, with the evidence at each record.

- `default-production`: reached by a transform pass with the default
  configuration (`memory.enabled = true`, `compaction.enabled = true`,
  `memory.injection_budget_tokens = 4000`).
- `explicit-config-only`: reached only with `compaction.enabled = false`
  (Pi `compactionOff`, daemon native fold authority) or a non-default budget.
- `test-only`: reached only through the test fixture control plane
  (`memory-seed`) or test entry points.

## Index

| Slug | Type | Reachability | Status | Existing check |
| --- | --- | --- | --- | --- |
| [injectable-rows-require-visible-verdict-and-positive-decision-kind](#injectable-rows-require-visible-verdict-and-positive-decision-kind) | safety | default-production | active | `canonical_memory.rs:282`, `memory_render.rs:619`, `tests/transform_canonical_memory.rs:46` (unaudited) |
| [memory-trim-admits-in-serving-order-not-category-order](#memory-trim-admits-in-serving-order-not-category-order) | safety | default-production | active | `canonical_memory.rs:412`, `m0_compose.rs:300`, `:337` (unaudited) |
| [memory-trim-cost-model-mirrors-rendered-block-structure](#memory-trim-cost-model-mirrors-rendered-block-structure) | safety | default-production | active | none for the rendered-byte identity; `m0_compose.rs:300` (unaudited) |
| [memory-trim-clamps-malformed-budget-and-floors-the-integer](#memory-trim-clamps-malformed-budget-and-floors-the-integer) | safety | explicit-config-only | active | `m0_compose.rs:300`, `:337` (unaudited) |
| [snapshot-revision-digests-only-rendered-rows](#snapshot-revision-digests-only-rendered-rows) | safety | default-production | active | `canonical_memory.rs:315`, `:373`, `:390`, `:412`; `transform.rs:13958`; `tests/transform_canonical_memory.rs:46`, `:356` (unaudited) |
| [project-memory-truncated-flag-excludes-budget-trimming](#project-memory-truncated-flag-excludes-budget-trimming) | safety | default-production | active | none asserting the flag under budget pressure |
| [subagent-pass-is-never-hard-and-serves-no-m0](#subagent-pass-is-never-hard-and-serves-no-m0) | safety | default-production | active | none naming the block |
| [additive-only-pass-still-composes-project-memory](#additive-only-pass-still-composes-project-memory) | safety | default-production | active | none |
| [pi-context-handler-is-gated-by-compaction-on](#pi-context-handler-is-gated-by-compaction-on) | safety | explicit-config-only | active | none |
| [pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false](#pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false) | safety | default-production | active | `hook.test.ts:1803` for OpenCode only (unaudited) |
| [memory-mark-command-is-model-free-and-confirms-before-writing](#memory-mark-command-is-model-free-and-confirms-before-writing) | safety | default-production | active | `memory-mark-command.test.ts:92-290`, `eidnara-memory-mark.test.ts:63-150`, `command-handler.test.ts:750-850`, `index-env-guard.test.ts:38` (unaudited) |
| [memory-seed-fixture-writes-repo-source-kind-under-code-observed](#memory-seed-fixture-writes-repo-source-kind-under-code-observed) | safety | test-only | active | `compression-fidelity-memory.test.ts:200-240` (unaudited) |

Semantics distribution: twelve `always`, zero `always-or-unreached`, zero
`sometimes`, zero `reachable`, zero `unreachable`. Type distribution: twelve
safety. Reachability: eight `default-production`, three
`explicit-config-only`, one `test-only`.

## Records

### injectable-rows-require-visible-verdict-and-positive-decision-kind

Origin: implemented behavior

Type: safety
Reachability: default-production - `read_project_memory` runs on every pass
with `memory.enabled = true`, the default (`config.rs:146`), through
`Handler::project_memory_read` (`lib.rs:5447-5461`) at `lib.rs:9345`.
Status: active
Exercised: partial - `canonical_memory.rs:282`
`injectable_rows_are_visible_decisions_in_a_positive_category` covers
Labeled, REJECTED_APPROACH, and an observation row;
`tests/transform_canonical_memory.rs:46` covers quarantined, retired,
superseded, other-project, other-domain, and `decision_kind = "memory"`
rows over a real store. No test seeds an `adr_accepted` decision kind.
Guarantee: A canonical row enters the injectable set only when its
`auto_inject` visibility is `Visible`, its typed decision row is present at
the snapshot, and its `decision_kind` is one of the twelve names in
`POSITIVE_MEMORY_CATEGORIES`; every other row, including a Labeled row and
a decision with `decision_kind = adr_accepted`, is absent from the block
and from the revision digest.
Check: `always` - for every `ReadResponse` handed to `injectable_snapshot`
(`canonical_memory.rs:184-211`), the returned rows equal the filter
`visibility == Visible && decisions.contains(object_id) &&
is_positive_memory_category(decision_kind)` applied before the budget trim;
a row with `decision_kind = "adr_accepted"` and `Visible` visibility is
absent. `always` because the filter runs on every served read; there is no
optional path.
Fault/timing angle: none for the filter itself. The visibility verdict is
the kernel's at `known_as_of`; a disposition change after the read lands on
the next pass (`tests/transform_canonical_memory.rs:159-200`).
Required faults and enabling state: a Visible row in a non-positive kind
(`adr_accepted`, `REJECTED_APPROACH`, `memory`); a Labeled row in a positive
kind; a Visible decision-kind row whose typed decision is missing (the read
fails closed with `CorruptCanonicalRow` at `read.rs:212-217`, so the filter
never sees it).
Confidence: high - [evidence](evidence/injectable-rows-require-visible-verdict-and-positive-decision-kind.md).
Read the filter chain at `canonical_memory.rs:195-204`, the vocabulary at
`memory_render.rs:67-80`, and the three tests.
Existing check: `canonical_memory.rs:282`, `memory_render.rs:619`
`render_boundary_drops_non_positive_categories`, `memory_render.rs:714`
`positive_category_vocabulary_matches_the_frozen_typescript_arrays`,
`tests/transform_canonical_memory.rs:46`; status unaudited.
Impact: A row the kernel labels or withholds would reach a block with no
label renderer, which is the authority leak #657 forbids; conversely the
plan's directive rows (`adr_accepted`) are invisible until KTD6 lands.
Open questions:
- KTD6 derives the category from `source_kind` for directives while keeping
  `decision_kind` for every other row; does the frozen vocabulary fixture
  `testdata/memory-category-vocabulary.json` stay the inclusion oracle when a
  category no longer equals a `decision_kind`? (needs human input)

### memory-trim-admits-in-serving-order-not-category-order

Origin: implemented behavior

Type: safety
Reachability: default-production - same path as the first record; the trim
runs inside `injectable_snapshot` (`canonical_memory.rs:206-210`).
Status: active
Exercised: partial - `canonical_memory.rs:412`
`rows_past_the_memory_budget_are_neither_injected_nor_digested` serves a
newer oversized row then an older short row and keeps the short one;
`m0_compose.rs:300` and `:337` compare the trim with the retained loop over
mixed categories. No test places a newer non-rule row ahead of an older
`PROJECT_RULES` row at a budget that admits only one.
Guarantee: The trim visits injectable rows in the order the read returned
them (`created_commit_seq` descending, `object_id` ascending), charges each
row's rendered line and, once per category, that category's wrapper, skips
a row whose marginal cost exceeds the remainder, and continues; category
rank plays no part in which rows survive, only in where the renderer places
them.
Check: `always` - for every input, `trim_memories_to_budget(rows, b, est)`
returns a subsequence of the positive rows of `rows` in the same order;
with rows `[newer NAMING, older PROJECT_RULES]` and a budget that fits one
row, the NAMING row is kept and the PROJECT_RULES row is skipped; the
rendered block then lists the surviving rows by category rank. `always`
because the rule is the baseline on every read.
Fault/timing angle: none; pure function of rows, budget, estimator.
Required faults and enabling state: at least two positive rows of different
categories with the newer row in a lower-ranked category; a budget equal to
the wrapper plus the first row's marginal cost (byte estimator makes the
boundary exact).
Confidence: high - [evidence](evidence/memory-trim-admits-in-serving-order-not-category-order.md).
Read `m0_compose.rs:74-112`, `read.rs:99`, `:127`, `:197`, and
`memory_render.rs:99-113`.
Existing check: `m0_compose.rs:300` `budget_boundaries_match_the_replaced_loop`,
`:337` `skip_and_continue_delegation_matches_the_replaced_loop`,
`canonical_memory.rs:412`; status unaudited.
Impact: Under budget pressure a rule competes with every other memory on
recency alone, which is the starvation the plan's required phase removes;
a test that assumes rules win today would be wrong.
Open questions: None.

### memory-trim-cost-model-mirrors-rendered-block-structure

Origin: implemented behavior

Type: safety
Reachability: default-production - same path.
Status: active
Exercised: not yet - every trim test uses `|text| text.len()`; none compares
the summed estimate with `estimate(render_memory_block(kept))`, and none
runs the production estimator `token_cache::cached_estimate_tokens`.
Guarantee: Under a byte-length estimator the trim's charged total (outer
wrapper plus one wrapper per category plus one line per row) equals the
byte length of `render_memory_block(kept, "project-memory")` exactly;
under the production tokenizer the two counts may differ only by token
boundary effects at the concatenation points, so the block's token count
is bounded by the budget up to that boundary slack and never by less than
the wrapper.
Check: `always` - for every kept set `K` from `trim_memories_to_budget(rows,
b, bytes)`, `bytes("<project-memory>\n</project-memory>") + sum over
categories in K of bytes("<CAT>\n</CAT>\n") + sum over rows in K of
bytes(render_memory_line(row) + "\n") ==
render_memory_block(K, "project-memory").len()`, and `render_memory_block(K).len()
<= floor(b)` when `K` is non-empty; under
`cached_estimate_tokens`, record the signed difference between the charged
total and the block's count as a measurement, and assert it stays within
the number of concatenation points. `always` because the reservation
semantics of KTD5 (wrapper and disclosure reserved before packing) assume
the cost model is additive.
Fault/timing angle: none.
Required faults and enabling state: category names from the fixed
vocabulary (no attribute escaping difference), rows whose lines contain `&`,
`<`, newlines, and a 64 KiB content so the escaping and the cut are in the
rendered bytes the estimate also sees.
Confidence: medium - [evidence](evidence/memory-trim-cost-model-mirrors-rendered-block-structure.md).
The byte identity is derived by reading `m0_compose.rs:93-98` against
`memory_render.rs:184-213`; it is not asserted by any test, and the
tokenizer behavior is unmeasured.
Existing check: none for the identity; `m0_compose.rs:300` pins the trim
against its own reference, not against the renderer.
Impact: A cost model that undercounts the block lets the rendered
`<project-memory>` exceed the configured budget and, after KTD5, lets a
reserved disclosure line become a victim; one that overcounts starves rows.
Open questions:
- How large is the tokenizer boundary slack for realistic rows under
  `cached_estimate_tokens`? (partial: the byte identity is exact; the token
  difference is unmeasured)

### memory-trim-clamps-malformed-budget-and-floors-the-integer

Origin: implemented behavior

Type: safety
Reachability: explicit-config-only - the budget comes from
`memory.injection_budget_tokens` or the deprecated `memory.budget_tokens`,
user tier only (`config.rs:1198-1214`), already clamped to `>= 1.0`; a
non-finite value cannot pass `number_at`, so the NaN and negative arms are
reachable only through direct callers (tests and the `.max(1.0)` fallback).
Status: active
Exercised: yes - `m0_compose.rs:300` `budget_boundaries_match_the_replaced_loop`
runs NaN, -3, 0, wrapper-1, wrapper, wrapper+10.5, wrapper+40.5, 1e30, and
+inf; `:337` runs NaN and -50..400 under proptest.
Guarantee: `trim_memories_to_budget` maps a NaN, negative, or zero budget to
one token and therefore to an empty set (the wrapper alone exceeds it),
floors a fractional budget, saturates an infinite budget to `u64::MAX`, and
never panics or underflows; a budget below the wrapper cost admits nothing
even when the row list is empty.
Check: `always` - `trim(rows, NaN) == trim(rows, -3.0) == trim(rows, 0.0) ==
[]`; `trim(rows, wrapper + 10.5) == trim(rows, wrapper + 10.0)`;
`trim(rows, +inf)` equals the positive rows in order; no call panics.
`always` because the clamp runs on every call.
Fault/timing angle: none.
Required faults and enabling state: direct calls with NaN, negative, zero,
fractional, and infinite budgets; a byte estimator so `wrapper` is 34.
Confidence: high - [evidence](evidence/memory-trim-clamps-malformed-budget-and-floors-the-integer.md).
Read `m0_compose.rs:79-90` and both tests.
Existing check: `m0_compose.rs:300`, `:337`; status unaudited.
`packing-budget-is-an-integer-never-clamped` (required catalog) names this
clamp as the precedent the packer must not inherit.
Impact: The plan's U2 says "convert the budget to an integer once" and
reuses `admit_required`/`reserve_required`; two conversions (a clamp here,
a typed refusal there) would make the required phase and the optional phase
disagree on the same configured value.
Open questions:
- Under U2, does the memory trim keep the `.max(1.0)` clamp or adopt the
  typed `ClaudeTokens::from_budget` refusal? (needs human input)

### snapshot-revision-digests-only-rendered-rows

Origin: implemented behavior

Type: safety
Reachability: default-production - `CanonicalMemorySnapshot::new` runs on
every served read; the revision is folded into the m1 signal
(`m1_compose.rs:75-84`) and compared on every pass.
Status: active
Exercised: yes - `canonical_memory.rs:315`, `:373`, `:390`, `:412`;
`transform.rs:13958` `project_memory_revision_change_requests_a_hard_fold`;
`tests/transform_canonical_memory.rs:46` (quarantine moves the revision and
requests `project_memory_epoch`) and `:356` (an edit past the budget holds
the revision and the frozen m0).
Guarantee: The snapshot revision is a function of the set of positive rows'
`(category, render_memory_line)` pairs sorted by `object_id`, independent of
`known_as_of`, `truncated`, read order, rows the renderer drops, rows the
budget drops, and content bytes past the 64 KiB cut; a different rendered
set yields a different revision up to hash collision, and a changed
external revision requests a HARD on the next pass with
`materialize_reason = "project_memory_epoch"`.
Check: `always` - `Snapshot::new(a, t, rows).revision() ==
Snapshot::new(b, !t, permute(rows) ++ dropped).revision()` for any
`a, b, t`, permutation, and `dropped` set of non-positive rows; a one-byte
edit inside the cut of one kept row changes it; and on a pass whose
`project_memory_revision()` differs from the stored
`m1_external_revision`, the response carries `materialize_reason:
"project_memory_epoch"` and `action: "HARD"`. `always` because the digest
is the only m0 refold trigger for memory changes.
Fault/timing angle: the digest is computed once per read; two passes see
different revisions only through a kernel commit between their reads.
Required faults and enabling state: two reads differing only in
`known_as_of`; a quarantine between passes; an edit to a budget-excluded
row; an edit past the 64 KiB cut.
Confidence: high - [evidence](evidence/snapshot-revision-digests-only-rendered-rows.md).
Read `canonical_memory.rs:50-67`, `m1_compose.rs:61-92`,
`transform.rs:2907-2909`, `:12436`.
Existing check: the six tests named above; status unaudited.
Impact: A digest that read `known_as_of` would refold m0 on every commit; one
that missed a rendered byte would serve a stale block as if unchanged. KTD5
adds the omitted count `N` to this digest.
Open questions:
- `DefaultHasher::new()` fixes its keys but std documents the algorithm as
  unspecified across releases; the revision is persisted in
  `ModuleMeta.m1_external_revision` and compared across daemon versions. Is a
  one-time refold of every session on a toolchain upgrade acceptable, or
  should the digest use a pinned hasher? (needs human input)
- KTD5 adds `N` under the unchanged tag `eidnara-project-memory-revision-v1`;
  should the tag move to `-v2` when the input layout changes? (needs human
  input)

### project-memory-truncated-flag-excludes-budget-trimming

Origin: implemented behavior

Type: safety
Reachability: default-production - `truncated` is set only in `read_visible`
(`read.rs:197`, `:233`) and carried unchanged into
`ProjectMemoryComposition::Canonical` on every served read.
Status: active
Exercised: not yet - `tests/transform_canonical_memory.rs:356` drops a row
by budget and asserts rows and revision but not `truncated`;
`canonical_memory.rs:412` constructs responses with `truncated: false` and
does not assert the field after trimming.
Guarantee: `project_memory.truncated` is `true` exactly when the kernel read
dropped rows past `MAX_READ_ROWS` or past the `MAX_READ_ROW_BYTES` payload
prefix, and stays `false` when the token budget dropped rows; no response
member today counts the rows the budget dropped.
Check: `always` - for a project with two Visible positive rows and a budget
that fits one, the response's `project_memory` is `{kind: "canonical",
truncated: false, ...}` and the block carries one row; for a project with
`MAX_READ_ROWS + 1` rows and an unbounded budget, `truncated` is `true`.
`always` because the flag is written on every served read from one source.
Fault/timing angle: none.
Required faults and enabling state: a budget that excludes at least one
row; separately, more than 8192 visible memory rows, or decision payloads
whose cumulative size passes 8 MiB.
Confidence: high - [evidence](evidence/project-memory-truncated-flag-excludes-budget-trimming.md).
Read `read.rs:70-71`, `:197`, `:229-234`, `canonical_memory.rs:100-106`,
`memory-store/src/lib.rs:2170-2173`.
Existing check: none asserting the flag under budget pressure.
Impact: An operator reading `truncated: false` concludes the block is the
whole injectable set while the budget silently dropped rows; the plan's
`rules_omitted` adds the first disclosure of a budget drop and must not be
confused with this flag.
Open questions: None.

### subagent-pass-is-never-hard-and-serves-no-m0

Origin: implemented behavior

Type: safety
Reachability: default-production - OpenCode sends `is_subagent: true` for
host-reported child sessions (`opencode-transform-adapter.ts:458, 507`;
`hook.test.ts:1803`); the wire default is `false`
(`docs/host-wire-protocol.md:966`). The guarantee holds on the fold path
(`eidnara_folds` true: compaction on and a summarizer model configured).
Status: active
Exercised: not yet - the subagent tests in `transform.rs` (`:12595`,
`:17235`, `:19282`, `:23710`, `:26990`) exercise TTL, emergency, reasoning,
and tagging; none asserts the absence of `<project-memory>` or of the `m0`
frozen unit in a subagent response on either path.
Guarantee: On the fold path a transform request with `is_subagent: true`
never plans a HARD (the plan is forced to `Soft` or `Defer` at
`transform.rs:4216-4222`) and the served output omits the `m0` and `m1`
frozen units (`:11723-11733`), so the response carries no
`<project-memory>` block even when the pinned read holds injectable rows;
on the additive-only path (`apply_additive_only`, entered when
`!authority.eidnara_folds` and the request is not a `lineage_switched`
subagent request, `transform.rs:3341`) the only `is_subagent` gate is that
`lineage_switched && is_subagent` exclusion, which routes the request to
`lineage_protocol_passthrough` (`:3355`); otherwise the plan comes from
`classify` and the served output always begins with the frozen `m0` and
`m1` (`:3178-3196`), so a subagent request with `lineage_switched: false`
receives the block there.
Check: `always` - for every response on the fold path to a request with
`is_subagent: true`, `action != "HARD"` and the served messages contain no
`<project-memory>` and no `<session-history>`; the same request with
`is_subagent: false` on the same store and `ProducerContext` carries the
block; and for a request with `is_subagent: true` and
`lineage_switched: false` on a context with `eidnara_folds: false`, the
served messages begin with the frozen m0 (recording the asymmetry, not
endorsing it). `always` because both fold gates are unconditional on the
flag and the additive serve step is unconditional on it once the
`lineage_switched && is_subagent` exclusion at `:3341` has not fired.
Fault/timing angle: the gate is per request; a session that alternates the
flag between passes serves m0 only on its non-subagent fold passes. Fold
authority is per binding (`config.rs:182-186`).
Required faults and enabling state: an Available read with at least one
positive row; `memory_enabled = true`; a request with `is_subagent: true`;
both `eidnara_folds` values.
Confidence: high - [evidence](evidence/subagent-pass-is-never-hard-and-serves-no-m0.md).
Read both fold gates, the request field (`transform.rs:671`), and the
additive serve step (`:3178-3196`); no `is_subagent` test between
`:2834` and `:3200` touches the plan or the output.
Existing check: none naming the block; `hook.test.ts:1803` covers only the
OpenCode flag.
Impact: The plan's parity contract (R13, AE8) is stated "for the same
`is_subagent` value" and KTD8 says "the daemon serves PASSTHROUGH plus m0
when the fold is off"; today an OpenCode subagent receives project memory
when the fold is off and none when it is on, so rule delivery to subagents
depends on fold authority, not only on the flag. A regression that dropped
the fold-path guard would inject project memory into prompts the design
keeps bare.
Open questions:
- Is the additive-only path's delivery of m0 to subagent requests intended,
  or a gap the fold path's guard was meant to cover? (needs human input)

### additive-only-pass-still-composes-project-memory

Origin: implemented behavior

Type: safety
Reachability: default-production - `apply_additive_only` runs when
`!authority.eidnara_folds` and the request is not a `lineage_switched`
subagent request (`transform.rs:3341`); `eidnara_folds()` is true
only with an admitted config, `compaction_enabled`, and a non-empty
`model_chain` (`config.rs:182-186`). `compaction_enabled` defaults to
`true` (`:145`) but `model_chain` defaults to empty (`:143`), so a
deployment that configures no `history_summarizer.model` takes this path on
every pass; `compaction.enabled = false` is the other way in.
Status: active
Exercised: not yet - the `eidnara_folds = false` tests at `transform.rs:16293`,
`:16438`, `:16521`, `:16586` assert reasons, retries, and activation, none
the block; the daemon transform catalog's
`canonical-read-staleness-is-distinguishable-from-emptiness` describes
`transform.rs:13894` as the "compaction-off engine", but that test's
`pctx` uses `FoldAuthorityIntent::default()` with `eidnara_folds: true`
(`fold_authority.rs:11-19`, `transform.rs:13803`).
Guarantee: When Eidnara does not hold fold authority, the additive-only
pass still composes an m0 whose bytes end with the `<project-memory>` block
rendered from the pass's pinned rows (`compose_additive_m0`,
`transform.rs:2790-2832`), gated by `memory_enabled`, and still refolds on
a project-memory revision change (`:2907-2910`, `:2987`).
Check: `always` - for a request on a session whose `fold_authority.eidnara_folds`
is `false`, with `memory_enabled` and an Available read of one positive
row, the served messages contain `<project-memory>` with that row and the
response's `project_memory` is `canonical`; with `memory_enabled = false`
the block is absent. `always` because the composition runs on every
additive-only materialization.
Fault/timing angle: the additive path refolds only on a scheduler or
configuration event (`transform.rs:2988-2992`); a revision change arriving
between such events is served on the next materialization.
Required faults and enabling state: an empty `model_chain` or
`compaction.enabled = false` (or a `ProducerContext` with `eidnara_folds:
false`); an Available read with one positive row.
Confidence: medium - [evidence](evidence/additive-only-pass-still-composes-project-memory.md).
Read the composition and the entry condition; no test witnesses the block
on this path.
Existing check: none.
Impact: KTD8 and OQ4 assume the daemon already serves m0 when the fold is
off; without a witness, U6's Pi change could register a handler that
receives passthrough without the block. The catalog wording in the
transform part overstates current coverage.
Open questions: None.

### pi-context-handler-is-gated-by-compaction-on

Origin: implemented behavior

Type: safety
Reachability: explicit-config-only - requires `compaction.enabled = false`
in the user tier at Pi boot, or a project whose resolved configuration
disables compaction.
Status: active
Exercised: not yet - no Pi test asserts that `pi.on("context", ...)` is not
registered under `compactionOff`, or that the handler returns `undefined`
when `isCompactionEnabled(deps.config)` is false; `index.test.ts:70-75`
covers `handlePiSessionBeforeCompact` only.
Guarantee: Pi registers its `context` transform handler only when
`compactionOff` is false at boot (`index.ts:383`, `:565`), and the
registered handler returns `undefined` for a project whose resolved
configuration disables compaction (`:573`); a compaction-off Pi process
sends no transform request and receives no `<project-memory>`.
Check: `always` - with a config whose `compaction.enabled` is `false`, the
extension factory records no `context` registration on the `ExtensionAPI`
stub; with it `true`, exactly one `context` handler is registered and
returns `undefined` for a `ctx` whose resolved project config disables
compaction, sending no module call. `always` because the gate is evaluated
once per process and once per event.
Fault/timing angle: `compactionOff` is fixed at boot (`index.ts:381`); a
later `/cd` into a compaction-off project is caught by the per-event gate
at `:573`, a `/cd` from off to on is not (no handler exists).
Required faults and enabling state: a Pi boot with `compaction.enabled =
false`; a project-tier config that disables compaction after a
compaction-on boot.
Confidence: high - [evidence](evidence/pi-context-handler-is-gated-by-compaction-on.md).
Read `index.ts:381-384`, `:565-609`.
Existing check: none.
Impact: This is the asymmetry R13 and OQ4 remove; U6 must lift both gates,
and a test that lifts only the boot gate still loses rules after `/cd`.
Open questions:
- Should the per-event gate at `index.ts:573` survive U6 as a fold-only
  gate inside `piTransform.run`, or move daemon-side? (needs human input)

### pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false

Origin: implemented behavior

Type: safety
Reachability: default-production - every Pi and OpenCode transform request
passes through `transformPassFields`.
Status: active
Exercised: partial - `hook.test.ts:1803` asserts `is_subagent: true` on an
OpenCode restored child; `pi-pass-inputs.test.ts` asserts usage and
completion fields, never `is_subagent`.
Guarantee: Both harnesses emit their pass fields through one builder
(`transform-pass-fields.ts:55-100`), which coerces `is_subagent` to a
strict boolean (`:66`); Pi supplies `is_subagent: false` on every pass
(`pi-pass-inputs.ts:110`) while OpenCode supplies
`isSubagentSession(sessionId)` (`opencode-transform-adapter.ts:458, 507`).
Check: `always` - `piPassFields(args).is_subagent === false` for every
`args`, including a session created by an in-process child;
`transformPassFields({passInputs: {is_subagent: "true"}}).is_subagent ===
false`; and the key sets of the two harnesses' `transformPassFields` outputs
are equal for the same `passInputs` shape. `always` because the builder runs
on every request.
Fault/timing angle: none.
Required faults and enabling state: a Pi child session (pi-subagents
in-process) and a top-level session, both captured; a non-boolean
`is_subagent` input.
Confidence: high - [evidence](evidence/pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false.md).
Existing check: `hook.test.ts:1803` (OpenCode side only); status unaudited.
Impact: Parity (R13) holds only "for the same `is_subagent` value"; a Pi
request can never be a subagent request, so a Pi child receives rules and
an OpenCode child does not. A future Pi change that infers the flag would
silently change which sessions receive rules.
Open questions:
- Does the parent Pi process's single `context` handler observe
  pi-subagents in-process child sessions, so that children receive m0 under
  `is_subagent: false`? (partial: `index.ts:338-348` shows the child factory
  call is skipped; handler scope per session is not visible in this
  repository)

### memory-mark-command-is-model-free-and-confirms-before-writing

Origin: implemented behavior

Type: safety
Reachability: default-production - both commands register at boot
(`index.ts:550`; `command-handler.ts:493-510`) whenever the plugin is
enabled.
Status: active
Exercised: yes - `memory-mark-command.test.ts:93-290` (tightening without
confirmation, confirmation left open writes nothing, decline writes
nothing, denied relaxation, replay by identity, replay after the object
moved, commit-side denial, session ending after preview, separator in
actor, refused preview, foreign verdict); `eidnara-memory-mark.test.ts:64-150`
(dialog, replaced session, no-UI fallback); `command-handler.test.ts:757-850`
(no dialog, `--yes` re-run, toast cut, disabled client); `index-env-guard.test.ts:38`
(subagent child registers nothing).
Guarantee: The command runner commits a disposition only after a preview
that reports no visibility change, or after the user confirmed (dialog or
`--yes`), or when the preview returned a replay receipt; it commits
nothing when the preview is refused or denied, when the confirmation is
declined or unavailable, or when the session is gone; its identity is
`[sessionId, event, objectId]`; it is reachable only from the Pi
`registerCommand` surface in the top-level entry and from OpenCode's
`command.execute.before`, which aborts the model turn with the sentinel,
and never from a model tool.
Check: `always` - `client.commit` is invoked at most once per run and only
when `previewed.receipt !== undefined`, or `!preview.visibility_changes`,
or `args.confirmed`, or `confirm(preview)` resolved `true`
(`memory-mark-command.ts:130-142`); `isCancelled()` true before the commit
yields `refused/cancelled` with no commit (`:140`); the Pi registration
exists only in `index.ts` (`:550`) and `subagent-entry.ts` contains no
`registerCommand`; the OpenCode path ends in `throwSentinel`
(`command-handler.ts:411`, `:420`, `:713`). `always` because every run goes
through the same runner.
Fault/timing angle: the object may change between preview and commit; the
commit's own verdict is reported (`:148-150`). The session may end during
the dialog (`eidnara-memory-mark.ts:48-56`).
Required faults and enabling state: a preview with `visibility_changes`,
a preview with a replay receipt, a deleted session between preview and
commit, a daemon that answers the preview with a non-available state.
Confidence: high - [evidence](evidence/memory-mark-command-is-model-free-and-confirms-before-writing.md).
Existing check: the tests named above; status unaudited.
Impact: KTD1 and U4 copy this runner for `/eidnara-directive`; a runner
that committed before confirmation would let queued text write authority
rows, which R1 forbids.
Open questions:
- U4 says "Refuse to run without a UI confirm (guard against
  `sendUserMessage`-style queued text)"; the memory-mark runner accepts
  `--yes` without a UI (`eidnara-memory-mark.test.ts:117`). Does the
  directive command keep the flag? (needs human input)

### memory-seed-fixture-writes-repo-source-kind-under-code-observed

Origin: implemented behavior

Type: safety
Reachability: test-only - `memory_seed` lives in the fixture control plane
(`crates/daemon/examples/direct_host_fixture.rs:757-783`) behind the
`memory-seed` command (`hermetic-host.ts:768-777`), reachable only from the
e2e harness.
Status: active
Exercised: yes - `compression-fidelity-memory.test.ts:200-240` seeds
admitted, budget-excluded, and wrong-project rows and asserts
`effectiveMaturity: "verified"`, `visibility: "automatic"`.
Guarantee: `memorySeed(anchor, objectId, category, text)` inserts a memory
decision with `decision_kind = category`, `source_kind = "repo"`,
`source_id = "<objectId>-lineage"`, `source_revision = 1`, in the anchor's
scope, and admits it with `CodeObserved` on a `code_present` observation
that shares that lineage, at classes `(TrustedLocalCode, CurrentCode)`; the
row is Verified and `automatic`; the seed cannot produce `source_kind =
user_directive` or `repository_policy`, an `AcceptedAdr` event, or a
non-project scope term, and its text frame is bounded at 64 KiB
(`MAX_CONTROL_LINE`, `direct_host_fixture.rs:66`).
Check: `always` - after `memorySeed`, a `kernel.read` on `explicit_search`
for `objectId` shows `source_kind: "repo"`, `source_id: "<objectId>-lineage"`,
and the admission's classes `trusted_local_code`/`current_code`; a text of
64 KiB or more is refused by the control frame. `always` because the seed
is one code path.
Fault/timing angle: none.
Required faults and enabling state: a seeded row and a kernel read of it;
a text at the frame cap.
Confidence: high - [evidence](evidence/memory-seed-fixture-writes-repo-source-kind-under-code-observed.md).
Read `memory_decision_spec` (`lib.rs:13785-13816`), `observe_code`
(`:13853-13889`), and the fixture.
Existing check: `compression-fidelity-memory.test.ts:200-240`; status
unaudited.
Impact: U6 proposes `memorySeed` "for the directive row"; under KTD5 a seeded
row has `source_kind = repo` and would pack in the optional phase, so a
parity test built on it would not exercise the required phase. The seam
needs a `source_kind` parameter or the directive route.
Open questions: None.

## Reused existing records

| Slug | Catalog path | Why it applies |
| --- | --- | --- |
| `packing-required-phase-precedes-optional-work` | `docs/properties/selection-packing/required/catalog.md` | `admit_required` stage order and the seven failure classes, including `over_budget`; KTD5 charges rules through it. Reachability `test-only` at that base; U2 makes the memory trim its first production caller, so the label needs re-verification after U2. |
| `packing-budget-is-an-integer-never-clamped` | same | Integer `TokenCount`; names the `trim_user_profile_to_budget` `.max(1.0)` clamp as a precedent not to inherit; the memory trim carries the same clamp (record above). |
| `packing-required-bytes-are-charged-untruncated` | same | `reserve_required` exactly-at-limit succeeds, one above fails with `OverBudget { limit, charged }`; KTD5's "stop at first OverBudget". |
| `packing-optional-scan-skips-and-continues` | `docs/properties/selection-packing/grouping/catalog.md` | `skip_and_continue` is the memory trim's rule today (`m0_compose.rs:88`) and KTD5's optional phase; `default-production` through the trim. |
| `canonical-read-staleness-is-distinguishable-from-emptiness` | `docs/properties/daemon/transform/catalog.md` | Withheld read composes no block and records `Withheld { state }`; memory-disabled pass records `None`; covers the `memory_enabled` gating the brief names. One wording issue noted in the additive-only record. |
| `dec-a-memory-injection-budget-documented-range-has-no-implementing-code` | `docs/properties/daemon/decisions/catalog.md` | `memory.injection_budget_tokens` default 4000 (`config.rs:30-31`, test `:1807`) and `.max(1.0)` only; user-tier only. |
| `cf-memory-credit-requires-admitted-visible-content` | `docs/properties/compression-fidelity/delivery/catalog.md` | Memory credit requires an included object from the pinned read; the 64 KiB cut and the `memory-seed` frame cap; the e2e variants that exclude candidate, wrong-project, budget-excluded, withheld, and rejected rows. |
| `bound-project-scope-cannot-be-widened-by-candidate` | `docs/properties/canonical-positive-claim-projection/catalog.md` | Project term stays mandatory in `read_visible` (`read.rs:177-178`); S2 owns the scope mechanics, cited here because U1 names it. |

## Relationship map

Grouped by shared mechanism; dominance is a hypothesis.

- **One filter chain, one digest.**
  `injectable-rows-require-visible-verdict-and-positive-decision-kind` is
  upstream of `memory-trim-admits-in-serving-order-not-category-order` and
  of `snapshot-revision-digests-only-rendered-rows`: the trim and the digest
  see only rows the filter kept, so a filter defect (a Labeled row kept)
  would be charged, rendered, and digested downstream with every downstream
  check passing. Only the first record detects it.
- **Cost model under the trim.**
  `memory-trim-cost-model-mirrors-rendered-block-structure` sits beside the
  serving-order record: the order says which row is priced next, the cost
  model says how much; both are inputs to `skip_and_continue`
  (`packing-optional-scan-skips-and-continues`). A wrong order with a right
  cost model still fits the budget; a wrong cost model with the right order
  overruns it. Neither dominates.
- **Budget entry.** `memory-trim-clamps-malformed-budget-and-floors-the-integer`
  is upstream of every trim record and sits beside
  `dec-a-memory-injection-budget-documented-range-has-no-implementing-code`
  (the config clamp) and against `packing-budget-is-an-integer-never-clamped`
  (the typed refusal). If U2 routes the trim through `required`, the two
  conversions must agree or the required phase and the optional phase see
  different budgets for one configured value.
- **Diagnostics tell different stories.**
  `project-memory-truncated-flag-excludes-budget-trimming` and
  `snapshot-revision-digests-only-rendered-rows` both describe
  `ProjectMemoryComposition::Canonical`; the flag says what the read
  dropped, the revision says what the block rendered, and neither says what
  the budget dropped. KTD5's `rules_omitted` fills that gap and must not be
  derived from either.
- **Three gates on the same block.**
  `subagent-pass-is-never-hard-and-serves-no-m0` (request flag),
  `additive-only-pass-still-composes-project-memory` (fold authority), and
  `pi-context-handler-is-gated-by-compaction-on` (harness registration) each
  decide whether a session receives `<project-memory>`. R13 parity is the
  conjunction: Pi must register regardless of compaction, the daemon must
  compose on the additive path, and the subagent flag must agree across
  harnesses, which
  `pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false` shows
  it does not. If the additive-only record fails, lifting the Pi gate
  delivers nothing.
- **Model-free write surfaces.**
  `memory-mark-command-is-model-free-and-confirms-before-writing` is the
  template for N5; `memory-seed-fixture-writes-repo-source-kind-under-code-observed`
  is the only test-side writer of Verified rows. Both are upstream of any
  parity e2e: the first for the directive channel, the second for what a
  seeded "directive" is. If the seed record holds, a seeded row is never a
  `user_directive`, so AE8 needs the directive route.
- **Downstream of this part.** `cf-memory-credit-requires-admitted-visible-content`
  consumes the pinned read these records describe; a change to the filter,
  trim, or digest moves its enabling state. `canonical-read-staleness-is-distinguishable-from-emptiness`
  is upstream of every record here: a withheld read produces no rows for any
  of them to judge.
