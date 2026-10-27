# Use-outcome record

This part catalogs the use-outcome record of the adaptive-discovery plan P3,
specified by [#1145](https://github.com/ahrav/eidnara/issues/1145) and landed
by tickets [#1154](https://github.com/ahrav/eidnara/issues/1154) (U1, the
Kernel tables, reads, and sweep), [#1155](https://github.com/ahrav/eidnara/issues/1155)
(U2, the daemon writer), [#1156](https://github.com/ahrav/eidnara/issues/1156)
(U3, the lifecycle columns), and [#1157](https://github.com/ahrav/eidnara/issues/1157)
(U4, the observational read). It carries the 34 records of the
specification's Property Catalog: 20 existing-behavior records (`uo-*`) and
14 new obligations (`uo-p-*`). #1154 lands the part and activates six
obligations; #1155 activates five; #1156 activates two; #1157 activates the last; every obligation is active.

## Evidence boundary

The specification authored its records at `a0e3f2993` and condensed the
evidence inline because no part directory existed; this part reconstructs
the records and their evidence from that catalog. Every `file:line` here was
re-verified against the #1157 branch tip, the commit that carries this
revision of the directory. A specification reference that moved or
pointed at a test rather than production code is corrected in the record's
evidence file. `Exercised` cites tests that run in the #1157 PR's recorded
gate run.

Every existing check is `unaudited` (see `existing-checks.md`). Adequacy
verdicts belong to `/testing:invariant-test-review`.

Evidence files are condensed where the cited code and tests carry the
evidence; several are below METHOD's 60-line target. The specification's
working files were not durable, so nothing beyond the verified trail was
available to restore.

## Scope

Surfaces (from the specification): S1 the composition (the Kernel read on
auto-inject, the injectable snapshot, the revision digest, the three freeze
sites); S2 the `memory.enabled` gate and the route-frozen config; S3
admission disposition events, surface visibility, and the nullable commit
sequence; S4 the capture sweeps and the live-retention index; S5
`session.delete` ordering, route teardown, and pass timestamps; S6 the Kernel
single-baseline open and the activation gate; S7 the sensitivity and egress
rule; S8 `review.*` reads and `host.status` conventions. #1154 adds S9: the
`use_outcomes` and `use_outcome_decisions` tables, the admission citing
column, the envelope insert, the two reads, and the expiry sweep.

## Reachability classes

Labels are per record, with the evidence in each record's `Confidence` line
and evidence file. Two engines are production: `DaemonConfig::default` has
an empty model chain, so the default deployment runs the additive engine
under native authority, and `eidnara_folds` (compaction on and a configured
model chain) runs the folding engine. A record whose behavior differs by
engine names both in its `Confidence` line (bias B1).

| Class | Records |
| --- | --- |
| default-production | 29 |
| explicit-config-only | 4 |
| test-only | 1 |

Check semantics: 31 `always`, 1 `always-or-unreached`, 2 `always(!X)`. No record pairs `always(!X)` with a `sometimes(X)` marker.

## Index

| Record | Kind | Type | Check | Reachability | Exercised | Status |
| --- | --- | --- | --- | --- | --- | --- |
| [`uo-served-rows-are-the-frozen-m0-rows-between-hard-folds`](#uo-served-rows-are-the-frozen-m0-rows-between-hard-folds) | existing | safety | `always` | default-production | partial | active |
| [`uo-composition-revision-digests-exactly-the-rendered-rows`](#uo-composition-revision-digests-exactly-the-rendered-rows) | existing | safety | `always` | default-production | yes | active |
| [`uo-composition-freeze-persists-at-every-hard-fold-site`](#uo-composition-freeze-persists-at-every-hard-fold-site) | existing | safety | `always` | default-production | partial | active |
| [`uo-subagent-pass-serves-no-composition-under-eidnara-folds`](#uo-subagent-pass-serves-no-composition-under-eidnara-folds) | existing | safety | `always-or-unreached` | explicit-config-only | yes | active |
| [`uo-known-as-of-is-the-tip-bound-read-sequence`](#uo-known-as-of-is-the-tip-bound-read-sequence) | existing | safety | `always` | default-production | partial | active |
| [`uo-memory-disabled-route-takes-no-canonical-read`](#uo-memory-disabled-route-takes-no-canonical-read) | existing | safety | `always` | explicit-config-only | yes | active |
| [`uo-consent-gate-source-is-the-route-frozen-config`](#uo-consent-gate-source-is-the-route-frozen-config) | existing | safety | `always` | default-production | partial | active |
| [`uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search`](#uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search) | existing | safety | `always` | default-production | partial | active |
| [`uo-admission-reads-require-a-non-null-commit-seq`](#uo-admission-reads-require-a-non-null-commit-seq) | existing | safety | `always` | test-only | yes | active |
| [`uo-disposition-events-append-rows-and-newest-by-commit-seq-wins`](#uo-disposition-events-append-rows-and-newest-by-commit-seq-wins) | existing | safety | `always` | default-production | yes | active |
| [`uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts`](#uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts) | existing | safety | `always` | default-production | yes | active |
| [`uo-expiry-keeps-pinned-and-cited-evidence`](#uo-expiry-keeps-pinned-and-cited-evidence) | existing | safety | `always` | default-production | yes | active |
| [`uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate`](#uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate) | existing | safety | `always` | default-production | yes | active |
| [`uo-route-teardown-purges-process-state-but-deletes-no-durable-row`](#uo-route-teardown-purges-process-state-but-deletes-no-durable-row) | existing | safety | `always` | default-production | partial | active |
| [`uo-per-session-pass-timestamps-die-with-the-session`](#uo-per-session-pass-timestamps-die-with-the-session) | existing | safety | `always` | default-production | yes | active |
| [`uo-kernel-opens-only-at-the-compiled-baseline-identity`](#uo-kernel-opens-only-at-the-compiled-baseline-identity) | existing | safety | `always` | default-production | yes | active |
| [`uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change`](#uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change) | existing | safety | `always` | explicit-config-only | yes | active |
| [`uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only`](#uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only) | existing | safety | `always` | default-production | yes | active |
| [`uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero`](#uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero) | existing | safety | `always` | default-production | yes | active |
| [`uo-review-reads-authorize-by-bearer-and-project-binding-not-owner`](#uo-review-reads-authorize-by-bearer-and-project-binding-not-owner) | existing | safety | `always` | default-production | yes | active |
| [`uo-p-baseline-rotation-refuses-previous-family-untouched`](#uo-p-baseline-rotation-refuses-previous-family-untouched) | new (#1154) | safety | `always` | default-production | yes | active |
| [`uo-p-served-set-and-identity-never-rewritten`](#uo-p-served-set-and-identity-never-rewritten) | new (#1154) | safety | `always` | default-production | yes | active |
| [`uo-p-classification-sensitive-local-only-no-remote-egress`](#uo-p-classification-sensitive-local-only-no-remote-egress) | new (#1154) | safety | `always` | default-production | yes | active |
| [`uo-p-disputed-after-serve-from-committed-events-only`](#uo-p-disputed-after-serve-from-committed-events-only) | new (#1154) | safety | `always` | default-production | yes | active |
| [`uo-p-sweep-retires-only-expired-uncited-rows`](#uo-p-sweep-retires-only-expired-uncited-rows) | new (#1154) | safety | `always` | default-production | yes | active |
| [`uo-p-record-survives-session-delete`](#uo-p-record-survives-session-delete) | new (#1154) | safety | `always` | default-production | yes | active |
| [`uo-p-served-set-once-per-session-revision`](#uo-p-served-set-once-per-session-revision) | new (#1155) | safety | `always` | default-production | yes | active |
| [`uo-p-served-ids-equal-frozen-composition`](#uo-p-served-ids-equal-frozen-composition) | new (#1155) | safety | `always` | default-production | yes | active |
| [`uo-p-no-row-when-memory-disabled`](#uo-p-no-row-when-memory-disabled) | new (#1155) | safety | `always(!X)` | explicit-config-only | yes | active |
| [`uo-p-no-row-for-withheld-composition`](#uo-p-no-row-for-withheld-composition) | new (#1155) | safety | `always(!X)` | default-production | partial | active |
| [`uo-p-producer-matches-request-subagent-flag`](#uo-p-producer-matches-request-subagent-flag) | new (#1155) | safety | `always` | default-production | yes | active |
| [`uo-p-deleted-at-iff-session-delete-kernel-step`](#uo-p-deleted-at-iff-session-delete-kernel-step) | new (#1156) | safety | `always` | default-production | yes | active |
| [`uo-p-last-transform-at-monotone-per-committing-pass`](#uo-p-last-transform-at-monotone-per-committing-pass) | new (#1156) | safety | `always` | default-production | yes | active |
| [`uo-p-counts-exact-and-unavailable-not-zero`](#uo-p-counts-exact-and-unavailable-not-zero) | new (#1157) | safety | `always` | default-production | yes | active |

## Records

### uo-served-rows-are-the-frozen-m0-rows-between-hard-folds

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the canonical-memory integration test runs a third,
non-HARD pass and observes the same `project_memory`; no test asserts m0 byte
equality across a disposition commit between folds
Guarantee: Between two HARD folds of a session, the served `<project-memory>`
rows are the composition frozen into session meta at the last HARD fold (m0),
not the result of a later Kernel read.
Check: `always` - on every pass whose plan is not `Hard` or `MigrateHard`, the
served m0 bytes and `meta.project_memory` equal those of the last HARD fold; the
semantics are `always` because the rule holds on every pass.
Fault/timing angle: A disposition commit that lands between a HARD fold and the
next pass.
Required faults and enabling state: A session with a HARD fold over a Canonical
composition, then a Kernel commit that changes the visible rows, then a non-HARD
pass.
Confidence: high -
[evidence](evidence/uo-served-rows-are-the-frozen-m0-rows-between-hard-folds.md).
the m0 unit is built only in `Hard | MigrateHard` arms
(`crates/daemon/src/transform.rs:3122`); the per-pass Kernel read feeds the
revision signal that requests a HARD fold
(`project_memory_revision_change_requests_a_hard_fold`
(`crates/daemon/src/transform.rs:13993`)).
Existing check:
`transform_composes_project_memory_from_canonical_rows_and_observes_absence`
(`crates/daemon/tests/transform_canonical_memory.rs:46`) (third pass keeps
`project_memory`); `project_memory_revision_change_requests_a_hard_fold`
(`crates/daemon/src/transform.rs:13993`). Status: unaudited.
Impact: A record keyed by a later read would name findings the session never
received.
Open questions: None.

### uo-composition-revision-digests-exactly-the-rendered-rows

Type: safety
Reachability: default-production
Status: active
Exercised: yes - four unit tests in `canonical_memory.rs` vary each rendered
input and each non-input
Guarantee: The composition revision is a digest over exactly the rendered
positive-category rows, sorted by object id, after the token-budget trim.
Check: `always` - two snapshots with equal rendered rows have equal revisions
and a change to any rendered row changes it; a dropped or over-budget row never
moves it.
Fault/timing angle: none
Required faults and enabling state: Snapshots that differ only in rows the
renderer drops, bytes past the render cap, or rows past the memory budget.
Confidence: high -
[evidence](evidence/uo-composition-revision-digests-exactly-the-rendered-rows.md).
read at `crates/daemon/src/canonical_memory.rs:55`; the revision is a
`DefaultHasher` `u64`, which is why #1154 keys the record by a separate
served-set digest.
Existing check: `revision_changes_only_when_the_rendered_inputs_change`
(`crates/daemon/src/canonical_memory.rs:380`);
`rows_the_renderer_drops_do_not_move_the_revision`
(`crates/daemon/src/canonical_memory.rs:438`);
`bytes_past_the_render_cap_do_not_move_the_revision`
(`crates/daemon/src/canonical_memory.rs:455`);
`rows_past_the_memory_budget_are_neither_injected_nor_digested`
(`crates/daemon/src/canonical_memory.rs:477`). Status: unaudited.
Impact: A revision that moved without a byte change would record a second serve
of the same rows.
Open questions:
- The `DefaultHasher` output is unspecified across toolchain releases; #1154
  resolves OQ3-12 by keying the record on `served_set_digest` and storing the
  revision as an informational column.

### uo-composition-freeze-persists-at-every-hard-fold-site

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the additive site and the folding fold record serves
through the handler tests, the pressure refold persists the pinned read's
composition and reports its freeze, and a handled Kernel failure injected after
the Memory Store commit leaves the composition and response equal to a
fault-free run (R13); a crash and restart between the two stores is not
constructed
Guarantee: Every pass whose plan is `Hard` or `MigrateHard` assigns the pinned
read's composition to session meta at one of three sites, made durable by the
pass's memory-store commit.
Check: `always` - after a committed HARD pass, `meta.project_memory` equals the
composition of that pass's pinned read.
Fault/timing angle: A CAS conflict that discards an attempt after the
assignment; a crash after the memory-store commit.
Required faults and enabling state: A HARD fold on each engine, a pressure
refold on the folding engine, and a CAS conflict inside one pass.
Confidence: high -
[evidence](evidence/uo-composition-freeze-persists-at-every-hard-fold-site.md).
the three assignments are `crates/daemon/src/transform.rs:3122`,
`crates/daemon/src/transform.rs:4743`, and
`crates/daemon/src/transform.rs:4898`. Reachability per site: the additive site
is default-production; the two folding sites are explicit-config-only
(`eidnara_folds`).
Existing check:
`canonical_memory_composes_the_project_memory_block_and_pins_the_snapshot`
(`crates/daemon/src/transform.rs:13886`);
`transform_composes_project_memory_from_canonical_rows_and_observes_absence`
(`crates/daemon/tests/transform_canonical_memory.rs:46`);
`a_failed_write_changes_nothing_the_harness_receives_and_a_later_fold_repairs_it`
(`crates/daemon/src/use_outcome_writer_tests.rs:253`);
`a_pressure_refold_reports_its_frozen_composition`
(`crates/daemon/src/transform.rs:20895`). Status: unaudited.
Impact: A site that skipped the assignment would leave the writer (#1155) with
no composition to record.
Open questions:
- Gap G1: does a CAS-discarded attempt leave a use-outcome serve? Owner #1155
  (AC7); the writer runs only after the attempt's memory-store commit succeeded,
  and `a_discarded_attempt_leaves_no_serve_and_the_retry_records_one` asserts
  that a discarded attempt writes none.

### uo-subagent-pass-serves-no-composition-under-eidnara-folds

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - a subagent pass under `eidnara_folds` folds no HARD, answers
with no `<project-memory>` block and no composition, and records no serve; under
native authority it records a `subagent` serve
Guarantee: Under `eidnara_folds`, a subagent pass plans `Soft` or `Defer`,
enters no HARD arm, and is served no new composition; under native authority the
additive engine applies no subagent plan rule.
Check: `always-or-unreached` - the forcing rule runs only on the folding engine;
when it runs, no subagent pass assigns a composition.
Fault/timing angle: A subagent request on each engine.
Required faults and enabling state: A route with `eidnara_folds` (compaction on,
a model chain) and a subagent request; the same request under native authority.
Confidence: medium -
[evidence](evidence/uo-subagent-pass-serves-no-composition-under-eidnara-folds.md).
the folding engine sets the subagent plan at
`crates/daemon/src/transform.rs:4239` and steps Soft with reduction units at
`crates/daemon/src/transform.rs:4478`; the additive engine reads `is_subagent`
only as scheduler inputs (`crates/daemon/src/transform.rs:2980`), so a subagent
pass on the default engine can enter a HARD arm (lead L1).
Existing check:
`a_subagent_pass_under_native_authority_reports_native_authority`
(`crates/daemon/src/fold_authority_handler_tests.rs:143`);
`subagent_first_active_render_commits_tagged_bytes_before_replay`
(`crates/daemon/src/transform.rs:23784`);
`the_session_role_follows_the_pass_subagent_flag_on_each_engine`
(`crates/daemon/src/use_outcome_writer_tests.rs:385`). Status: unaudited.
Impact: The record's `subagent` rows would be attributed to the wrong engine.
Open questions:
- OQ3-13: is the additive engine's subagent serve intended? Owner: product owner
  with #1142 (needs human input).
- Correction: a native subagent lineage switch routes to `apply_once` and
  answers a passthrough (`crates/daemon/src/transform.rs:3363`), so "no subagent
  rule applies under native authority" holds for the plan, not for routing.

### uo-known-as-of-is-the-tip-bound-read-sequence

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the integration test asserts `known_as_of` equals the tip
at two reads; no test binds the read to an earlier tip while a later commit
lands (R8)
Guarantee: The composition's `known_as_of` is the Kernel tip captured before the
visible-row read, and the read is bound at that tip.
Check: `always` - `known_as_of` equals the tip the read was bound at, and every
admission event at or below it is reflected in the read.
Fault/timing angle: A commit that lands between the tip capture and the
visible-row read.
Required faults and enabling state: A Canonical read with a concurrent Kernel
commit.
Confidence: high -
[evidence](evidence/uo-known-as-of-is-the-tip-bound-read-sequence.md).
`read_project_memory` captures the tip and passes it as the requested snapshot
(`read_project_memory` (`crates/daemon/src/canonical_memory.rs:167`)); the route
returns `known_as_of` from the visible read
(`crates/daemon/src/kernel_routes/read.rs:252`).
Existing check:
`transform_composes_project_memory_from_canonical_rows_and_observes_absence`
(`crates/daemon/tests/transform_canonical_memory.rs:46`) (assertions
`crates/daemon/tests/transform_canonical_memory.rs:159` and
`crates/daemon/tests/transform_canonical_memory.rs:196`). Status: unaudited.
Impact: `served_commit_seq` (C5) would name a sequence the served rows did not
reflect, and the strict-greater dispute join would miscount.
Open questions:
- Gap G3: commit sequences are allocated by `INTEGER PRIMARY KEY AUTOINCREMENT`
  on `commit_log`, so they are strictly increasing and the tip is their maximum;
  no test names that property for this catalog. Conclusion: resolved by the
  schema (`commit_log` DDL in `crates/kernel/src/schema.rs`); no new record.

### uo-memory-disabled-route-takes-no-canonical-read

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - the integration test binds a route with memory disabled and
asserts no block and no `project_memory` field
Guarantee: A route bound with `memory.enabled` off takes no Kernel read, so the
pass has no composition.
Check: `always` - a pass on such a route issues no canonical read and stores no
composition.
Fault/timing angle: none
Required faults and enabling state: A route bound with `/memory/enabled: false`.
Confidence: high -
[evidence](evidence/uo-memory-disabled-route-takes-no-canonical-read.md).
`project_memory_read` wraps the read in `cfg.memory_enabled.then(...)`
(`project_memory_read` (`crates/daemon/src/lib.rs:5542`)).
Existing check: `a_memory_disabled_pass_takes_no_canonical_read`
(`crates/daemon/tests/transform_canonical_memory.rs:320`). Status: unaudited.
Impact: A disabled route that still read and recorded would breach the consent
rule (C2).
Open questions: None.

### uo-consent-gate-source-is-the-route-frozen-config

Type: safety
Reachability: default-production
Status: active
Exercised: partial - a pass under a disabled frozen config records nothing; a
wrapup under a live config that differs from the binding records no serve; a
real config reload is not constructed (R14 seam)
Guarantee: A pass reads `memory.enabled` from the config frozen at `route.open`;
the wrapup fire reads the live effective config.
Check: `always` - the consent value a pass uses equals its binding's frozen
config whatever the live config says.
Fault/timing angle: A config reload between bind and a later pass or wrapup.
Required faults and enabling state: A route bound with memory on, then a reload
that turns it off (and the reverse), then a pass and a wrapup.
Confidence: medium -
[evidence](evidence/uo-consent-gate-source-is-the-route-frozen-config.md). bind
freezes `effective_config` into the binding (`crates/daemon/src/lib.rs:13555-13565`);
wrapup reads the live config (`prepare_wrapup_fire`
(`crates/daemon/src/lib.rs:6029`)). R14 applied: the config seam is
`SessionBinding.config` versus `effective_config` at the two call sites.
Existing check:
`history_summarizer_diagnostics_omit_project_memory_when_memory_is_disabled_or_not_fired`
(`crates/daemon/src/lib.rs:39960`); `only_an_available_composition_is_recorded`
(`crates/daemon/src/use_outcome_writer_tests.rs:180`);
`a_wrapup_under_a_config_that_differs_from_the_binding_records_no_serve`
(`crates/daemon/src/use_outcome_writer_tests.rs:555`). Status: unaudited.
Impact: The record could be written under a consent value the route never had.
Open questions:
- Lead L5: the pass and the wrapup read consent from different sources. Owner:
  daemon owner (needs human input).
- Gap G4: the summarizer reattach path reads `memory_enabled` from the live
  effective config (`crates/daemon/src/lib.rs:5256`, used at
  `crates/daemon/src/lib.rs:5403`), a third consent source beside the frozen
  binding and the live wrapup config; the fire path states the frozen-config
  rule in its own comment (`crates/daemon/src/lib.rs:5913`). Recorded, not
  fixed; whether the reattach should follow the frozen config is an owner
  decision (needs human input).

### uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the unit matrix proves the auto-inject cell; the
integration tests check auto-search and compare ids only
Guarantee: A decision whose newest disposition is `disputed` is hidden on
`auto_inject` and `auto_search` and served labeled on `explicit_search`.
Check: `always` - for every disputed subject, the auto-inject read omits it and
the explicit-search read labels it.
Fault/timing angle: none
Required faults and enabling state: A decision admitted, then a committed
`mark_disputed` on it.
Confidence: high -
[evidence](evidence/uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search.md).
`visibility_row` maps `Disputed` to `ExplicitLabeled` (`visibility_row`
(`crates/kernel/src/admission.rs:4248`)); `surface_visibility` hides it on every
surface but explicit search (`crates/kernel/src/admission.rs:4121`).
Existing check: `visibility_matrix_and_sensitivity_ceiling_are_total`
(`crates/kernel/src/admission.rs:4915`);
`every_non_active_disposition_has_the_contracted_surface_row`
(`crates/kernel/tests/kernel_admission.rs:2890`);
`dispositions_and_sensitivity_only_reduce_visibility`
(`crates/kernel/tests/kernel_admission.rs:2735`). Status: unaudited.
Impact: If a disputed decision were auto-injected, the strict-greater dispute
join (C12) would miss disputes the session saw before the serve.
Open questions: None.

### uo-admission-reads-require-a-non-null-commit-seq

Type: safety
Reachability: test-only
Status: active
Exercised: yes - a direct SQL update nulls `commit_seq` and the explicit-search
read omits the row
Guarantee: Every admission read counts only rows whose `commit_seq` is not null.
Check: `always` - a row with a null `commit_seq` never decides visibility,
latest disposition, or the dispute flag.
Fault/timing angle: none
Required faults and enabling state: A row whose `commit_seq` is nulled by direct
SQL.
Confidence: high -
[evidence](evidence/uo-admission-reads-require-a-non-null-commit-seq.md). the
predicates filter `commit_seq IS NOT NULL`
(`crates/kernel/src/admission.rs:675`); the production writer binds the envelope
sequence at `crates/kernel/src/admission.rs:1781`. The use-outcome dispute join
and citation predicate filter the same way
(`crates/kernel/src/use_outcome.rs:178`).
Existing check:
`malformed_stored_policy_and_sensitivity_fail_closed_without_read_error`
(`crates/kernel/tests/kernel_admission.rs:2935`);
`disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
(`crates/kernel/tests/kernel_use_outcome.rs:400`). Status: unaudited.
Impact: A null row counted as a dispute would report harm with no committed
event.
Open questions: None.

### uo-disposition-events-append-rows-and-newest-by-commit-seq-wins

Type: safety
Reachability: default-production
Status: active
Exercised: yes - two dispositions in one commit exercise the id tie-break
Guarantee: Disposition events append admission rows; the newest committed row by
(`commit_seq`, `admission_decision_id`) decides the subject's disposition.
Check: `always` - both latest-decision formulations select the same row for
every subject.
Fault/timing angle: Two disposition events in one commit.
Required faults and enabling state: A `mark_stale` and a `mark_disputed`
committed together.
Confidence: high -
[evidence](evidence/uo-disposition-events-append-rows-and-newest-by-commit-seq-wins.md).
`LATEST_SUBJECT_DECISION_PREDICATE` (`crates/kernel/src/admission.rs:675`) and
its single-row seek.
Existing check: `both_latest_decision_formulations_choose_the_same_row`
(`crates/kernel/tests/kernel_admission.rs:2252`);
`an_event_that_names_a_disposition_keeps_its_own_outcome`
(`crates/kernel/src/admission.rs:5245`). Status: unaudited.
Impact: The dispute join counts event existence (C12), so this record bounds
what a reader sees, not the join.
Open questions:
- A decision disputed then re-approved still counts as disputed after serve:
  event existence counts (resolved at #1154 ticketing).

### uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts

Type: safety
Reachability: default-production
Status: active
Exercised: yes - 65 harness captures drain as one page and a remainder; unit
tests cover receipts and rechecks
Guarantee: The capture expiry sweeps retire at most 64 captures per call, oldest
deadline first, each under a reserved-producer receipt keyed by cutoff and id
and rechecked under the writer.
Check: `always` - one call retires at most one page; a backlog of N drains in
ceil(N / 64) calls.
Fault/timing angle: A pin taken after selection; a replayed receipt.
Required faults and enabling state: More than 64 expired captures; a hold
acquired between selection and commit.
Confidence: high -
[evidence](evidence/uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts.md).
`crates/kernel/src/local_file.rs:26`, `expire_captures`
(`crates/kernel/src/local_file.rs:250`), `expiry_intent`
(`crates/kernel/src/local_file.rs:433`), `still_expired`
(`crates/kernel/src/local_file.rs:476`).
Existing check:
`harness_capture_evidence_expires_at_its_deadline_and_replay_never_renews_it`
(`crates/daemon/tests/harness_sources.rs:3356`);
`harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
(`crates/daemon/tests/harness_sources.rs:3500`);
`a_replayed_expiry_receipt_is_not_counted_as_a_retirement`
(`crates/kernel/src/local_file.rs:737`). Status: unaudited.
Impact: The use-outcome sweep (#1154) reuses this loop shape; a defect here
would recur there.
Open questions: None.

### uo-expiry-keeps-pinned-and-cited-evidence

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the harness drain test holds and cites captures; the descriptor
test refuses cited evidence
Guarantee: Capture expiry keeps evidence a live hold pins and evidence another
live row cites.
Check: `always` - after any sweep, no retired evidence row had a live pin or a
live citation at its retiring commit.
Fault/timing angle: A citation or pin that appears after selection.
Required faults and enabling state: An expired capture cited by a live
observation; one pinned by a live hold.
Confidence: high -
[evidence](evidence/uo-expiry-keeps-pinned-and-cited-evidence.md).
`crates/kernel/src/local_file.rs:523`; `crates/kernel/src/slice/write.rs:42`.
Existing check:
`harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
(`crates/daemon/tests/harness_sources.rs:3500`);
`retiring_evidence_invalidates_uncited_evidence_and_refuses_cited_evidence`
(`crates/kernel/tests/kernel_source_descriptors.rs:1895`);
`a_capture_pinned_after_it_was_selected_for_expiry_is_kept`
(`crates/kernel/src/local_file.rs:760`). Status: unaudited.
Impact: The use-outcome cited-row exception mirrors this liveness idiom; a
defect here predicts one there.
Open questions:
- Gap G5: every retention class a writer stores is a compiled constant?
  `harness_capture`, `curator_capture`, and `use_outcome_capture` are constants;
  `canonical` has no Rust constant (spec correction at #1154 ticketing).
  Unresolved, needs an owner decision on a `canonical` constant (needs human
  input).

### uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate

Type: safety
Reachability: default-production
Status: active
Exercised: yes - three source-capture tests cover the order, a Kernel that is
starting, and a stop between the stores; the use-outcome step runs in the same
Kernel step
Guarantee: `session.delete` holds the capture commit gate, retires the session's
Kernel sources in pages, then deletes the Memory Store rows; a Kernel failure
returns before the Memory Store is touched.
Check: `always` - after `ok` the Kernel step and the memory delete both ran in
that order; after `kernel_unavailable` the memory rows are intact.
Fault/timing angle: A stop between the stores, then a retry; a Kernel that is
starting.
Required faults and enabling state: A session with captured sources; the
`stop_after_retirement` hook.
Confidence: high -
[evidence](evidence/uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate.md).
handler `handle_session_delete_value` (`crates/daemon/src/lib.rs:6975`); gate
`crates/daemon/src/lib.rs:2822`; Kernel step `retire_captured_sources`
(`crates/daemon/src/lib.rs:6938`); memory delete
`crates/daemon/src/lib.rs:7020`.
Existing check:
`session_delete_retires_captured_sources_and_a_retry_recovers_a_stop_between_stores`
(`crates/daemon/src/source_capture_tests.rs:1557`);
`session_delete_with_the_kernel_starting_deletes_nothing`
(`crates/daemon/src/source_capture_tests.rs:1627`);
`session_delete_retires_only_its_own_captures_and_recovers_a_stop_between_pages`
(`crates/daemon/src/source_capture_tests.rs:1699`);
`session_delete_waits_for_the_capture_gate_off_the_runtime_worker`
(`crates/daemon/src/lib.rs:26455`);
`a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:41`). Status: unaudited.
Impact: #1156 places the `deleted_at` write in this Kernel step; a reordering
would let the record miss the delete.
Open questions: None.

### uo-route-teardown-purges-process-state-but-deletes-no-durable-row

Type: safety
Reachability: default-production
Status: active
Exercised: partial - teardown tests check process state and a retained
checkpoint; a teardown after a HARD fold leaves the use-outcome record live and
the memory rows intact; no test asserts zero durable rows deleted in every table
of both stores
Guarantee: Route teardown purges process-local session state and deletes no
durable row in either store.
Check: `always` - after `route_gone`, every durable row of the session in both
stores is unchanged.
Fault/timing angle: A teardown after a HARD fold.
Required faults and enabling state: A route closed without `session.delete`
after a HARD fold.
Confidence: high -
[evidence](evidence/uo-route-teardown-purges-process-state-but-deletes-no-durable-row.md).
`unbind_route` (`crates/daemon/src/lib.rs:4716`) calls `purge_session_state`
(`crates/daemon/src/lib.rs:4756`), which clears in-memory maps only.
Existing check: `route_teardown_keeps_an_admitted_checkpoint_alive`
(`crates/daemon/src/lib.rs:25405`);
`route_teardown_releases_the_staged_upload_while_the_session_stays_bound`
(`crates/daemon/tests/kernel_routes.rs:3849`);
`a_route_torn_down_after_a_hard_fold_leaves_its_records_live`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:208`). Status: unaudited.
Impact: A teardown that wrote `deleted_at` would mark a live session deleted
(AC10).
Open questions: None.

### uo-per-session-pass-timestamps-die-with-the-session

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the Memory Store delete test seeds every session table and
asserts each is swept; the Kernel copy `last_transform_at` survives the delete
Guarantee: The per-session pass timestamp `last_committed_pass_at_ms` lives on
Memory Store session meta and is deleted with the session.
Check: `always` - after `delete_session`, no row of the session remains in any
session-keyed table.
Fault/timing angle: none
Required faults and enabling state: A session with committed passes, then
`session.delete`.
Confidence: high -
[evidence](evidence/uo-per-session-pass-timestamps-die-with-the-session.md).
field `crates/memory-store/src/lib.rs:2512`; writes
`crates/daemon/src/transform.rs:3262`; delete `delete_session`
(`crates/memory-store/src/lib.rs:8968`).
Existing check:
`delete_session_sweeps_every_session_table_once_and_scopes_notes`
(`crates/memory-store/src/lib.rs:22343`);
`a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:41`). Status: unaudited.
Impact: The use-outcome `last_transform_at` column (#1156) is the Kernel-side
copy that survives the delete.
Open questions: None.

### uo-kernel-opens-only-at-the-compiled-baseline-identity

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the mismatch test covers epoch, user version, digest, and an
extra object; #1154 adds a family at the previous baseline
Guarantee: A Kernel family opens only when its header, marker, live digest, and
object inventory equal the compiled baseline; any other family is refused
untouched.
Check: `always` - an open of a family with any differing term returns
`IdentityMismatch` and leaves the main file and `-wal` byte-identical.
Fault/timing angle: none
Required faults and enabling state: A family at another baseline, with a valid
marker naming that baseline.
Confidence: high -
[evidence](evidence/uo-kernel-opens-only-at-the-compiled-baseline-identity.md).
`classify_existing_family` (`crates/kernel/src/open.rs:796`);
`classify_open_kernel` (`crates/kernel/src/open.rs:852`). Disposition B:
re-verified against the #1154 digest
`40cf9b503ce6458c98e352324df95e4a7ad97066f6e9b4b5e699bd62140e150b`.
Existing check: `every_conclusive_kernel_mismatch_is_refused_and_left_untouched`
(`crates/kernel/tests/kernel_open.rs:183`);
`a_family_created_under_the_previous_baseline_is_refused_untouched`
(`crates/kernel/tests/kernel_open.rs:582`);
`kernel_schema_digest_is_pinned_to_the_frozen_v1_shape`
(`crates/kernel/tests/kernel_schema.rs:599`);
`kernel_schema_object_inventory_is_pinned_by_kind_and_name`
(`crates/kernel/tests/kernel_schema.rs:1900`). Status: unaudited.
Impact: A silent in-place change would run the new code over rows the old schema
shaped.
Open questions: None.

### uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - activation unit tests vary each identity term
Guarantee: The MemoryReviewer activation gate closes when the record's
`kernel_baseline_digest` or `kernel_incarnation` differs from the live value.
Check: `always` - a record naming another Kernel digest or incarnation evaluates
`Closed::IdentityMismatch`.
Fault/timing angle: none
Required faults and enabling state: An activation record written before the
rotation.
Confidence: high -
[evidence](evidence/uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change.md).
`identity_terms` (`crates/daemon/src/memory_reviewer/activation.rs:174`);
`evaluate` (`crates/daemon/src/memory_reviewer/activation.rs:219`). Disposition
B: the live digest is `kernel::kernel_baseline_digest()`, which this rotation
changes.
Existing check: `the_gate_opens_only_for_a_matching_attested_owner_only_record`
(`crates/daemon/src/memory_reviewer/activation.rs:448`);
`the_acceptance_gate_opens_only_for_an_owner_only_record_of_the_live_pair`
(`crates/daemon/src/memory_reviewer/activation.rs:385`). Status: unaudited.
Impact: A gate that stayed open across the rotation would send work under an
approval for another deployment.
Open questions: None.

### uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only

Type: safety
Reachability: default-production
Status: active
Exercised: yes - CAS and eligibility tests cover the class matrix; a unit test
reads unknown stored classes back restrictive
Guarantee: The class rule denies Remote for `sensitive`, and an unrecognized
stored egress class reads as `local_only`.
Check: `always` - `Sensitivity::Sensitive.denies_destination(Remote)` is
`SensitiveRemote`, and any stored egress text other than `remote_allowed` is
`LocalOnly`.
Fault/timing angle: none
Required faults and enabling state: A sensitive row presented to the rule; an
out-of-vocabulary egress string by SQL.
Confidence: high -
[evidence](evidence/uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only.md).
`denies_destination` (`crates/kernel/src/envelope.rs:71`);
`crates/kernel/src/cas/mod.rs:109`.
Existing check: `eligibility_matrix_includes_secret_unknown_and_tombstone`
(`crates/kernel/tests/kernel_cas.rs:452`);
`egress_facts_carry_the_eligibility_verdict_and_the_stored_class`
(`crates/kernel/tests/kernel_cas.rs:545`);
`unrecognized_stored_classes_read_back_restrictive`
(`crates/kernel/src/cas/mod.rs:797`);
`a_serve_creates_no_registry_or_evidence_row_and_its_class_denies_remote`
(`crates/kernel/tests/kernel_use_outcome.rs:381`). Status: unaudited.
Impact: A use-outcome row presented to egress would leave the host.
Open questions: None.

### uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the daemon report test and two CLI tests
Guarantee: `host.status` blocks carry closed-name counters, and the CLI renders
the MemoryReviewer counters unavailable unless the block state is ready, through
an exact-integer decoder bounded at 2^53.
Check: `always` - a counter is rendered as an integer only when the block is
ready and the value is an exact integer in [0, 2^53].
Fault/timing angle: A non-ready block with counters present.
Required faults and enabling state: A `memory_reviewer` block in a non-ready
state with zero counters.
Confidence: medium -
[evidence](evidence/uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero.md).
`packages/cli/src/commands/review-wire.ts:379`;
`packages/cli/src/commands/review-wire.ts:434`;
`packages/opencode-plugin/src/shared/host-client/exact-json.ts:11`. Lead L4: the
`source_capture` block has no state flag
(`crates/daemon/src/source_capture.rs:675`).
Existing check: `the_capture_report_reaches_host_status_field_for_field`
(`crates/daemon/src/source_capture_tests.rs:845`);
`packages/cli/src/commands/review.test.ts:887`;
`packages/cli/src/commands/review.test.ts:938`. Status: unaudited.
Impact: A zero read as "no harm observed" when the store was not ready.
Open questions:
- Gap G6: does the status surface name no identifiers? The `source_capture` and
  `memory_reviewer` blocks carry closed counter names only; owner #1157 (AC19)
  proves it for the use-outcome read.

### uo-review-reads-authorize-by-bearer-and-project-binding-not-owner

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the wire tests list and read under MODULE authority and compare
an observational route; `review.use_outcome` answers an observational route the
same counts
Guarantee: `review.*` reads authorize by the connection bearer key and the
route's project binding; the wire has no owner principal, and an observational
`cli` route reads the same answers.
Check: `always` - two routes bound to one project answer the same bytes for the
same read, whatever their harness.
Fault/timing angle: none
Required faults and enabling state: An observational route and an ordinary route
bound to one project.
Confidence: high -
[evidence](evidence/uo-review-reads-authorize-by-bearer-and-project-binding-not-owner.md).
`kernel_route_scope` (`crates/daemon/src/kernel_routes/mod.rs:414`);
`bind_review` (`crates/daemon/src/memory_reviewer/wire.rs:203`);
`crates/daemon/src/lib.rs:275`.
Existing check:
`review_operations_are_disabled_without_module_authority_and_list_and_read_receipts_under_it`
(`crates/daemon/tests/memory_reviewer_wire.rs:15`);
`an_observational_route_reads_the_same_outcomes_as_an_ordinary_route`
(`crates/daemon/tests/memory_reviewer_wire.rs:260`);
`review_use_outcome_counts_serves_and_later_disputes_of_one_decision`
(`crates/daemon/tests/memory_reviewer_wire.rs:686`). Status: unaudited.
Impact: A per-owner read would contradict K5; the #1157 read follows the bearer
rule.
Open questions:
- Lead L3: the spec's "owner-only" read has no wire principal; restated in C17.

### uo-p-baseline-rotation-refuses-previous-family-untouched

Type: safety
Reachability: default-production
Status: active
Exercised: yes - a fixture family at the previous baseline digest with a valid
marker is opened by this binary
Guarantee: A Kernel family at the previous baseline opens as `IdentityMismatch`
with its main file and `-wal` byte-identical; a new family contains both
use-outcome tables and its digest equals the pinned literal.
Check: `always` - the previous-family open returns `IdentityMismatch` with equal
bytes before and after, and the compiled digest equals the pinned literal;
checked on every run of the Kernel tests.
Fault/timing angle: none
Required faults and enabling state: A fixture database at the `a0e3f2993` digest
(`13437d62f69c77d1f5bdad72195473d1f409c2647efecdcac660554e5c1db07b`) opened by
this binary.
Confidence: high -
[evidence](evidence/uo-p-baseline-rotation-refuses-previous-family-untouched.md).
the fixture `crates/kernel/tests/fixtures/prior_kernel_baseline.sql` is the
previous `sqlite_schema` text verbatim; the test asserts its digest before the
open (`crates/kernel/tests/kernel_open.rs:594`).
Existing check:
`a_family_created_under_the_previous_baseline_is_refused_untouched`
(`crates/kernel/tests/kernel_open.rs:582`);
`kernel_schema_digest_is_pinned_to_the_frozen_v1_shape`
(`crates/kernel/tests/kernel_schema.rs:599`);
`kernel_schema_object_inventory_is_pinned_by_kind_and_name`
(`crates/kernel/tests/kernel_schema.rs:1900`);
`kernel_schema_has_one_ordered_full_shape`
(`crates/kernel/tests/kernel_schema.rs:151`). Status: unaudited.
Impact: A silent in-place schema change, or an unreviewed rotation.
Open questions: None.

### uo-p-served-set-and-identity-never-rewritten

Type: safety
Reachability: default-production
Status: active
Exercised: yes - direct SQL attempts every forbidden transition on both tables
Guarantee: After commit, the identity columns, `served_commit_seq`,
`session_role`, `recorded_at`, the classification columns, `decision_count`, and
the decision rows never change; `deleted_at` is set once, `last_transform_at`
never decreases, retirement is set once, no row is deleted, a record is served
at a commit before the one that records it, and an admission citation names a
recorded serve when its row is written, never changes, keeps its subject and
commit sequence, and its row is never deleted.
Check: `always` - every forbidden update or delete aborts under a named trigger,
`INSERT OR REPLACE` aborts through the delete trigger on every table including
`admission_decisions`, a decision row inserted once `decision_count` rows exist
or once a later commit exists aborts (so a Kernel-written set is sealed at its
commit, and any record's set is sealed by the next commit), a row with
`served_commit_seq >= created_commit_seq` or `decision_count < 0` fails its
CHECK, a decision row naming a registry object of another kind or one that was
absent or retired at `served_commit_seq` aborts, and an admission row naming an
unrecorded serve fails its foreign key.
Fault/timing angle: A raw insert after the recording commit while that commit
is still the newest.
Required faults and enabling state: Direct SQL for each forbidden transition,
including REPLACE and a post-commit decision-row append.
Confidence: high -
[evidence](evidence/uo-p-served-set-and-identity-never-rewritten.md). triggers
in the `use_outcomes` and `use_outcome_decisions` components
(`crates/kernel/src/schema.rs:188`); the `admission_decisions_citation_no_delete`
trigger (`crates/kernel/src/schema.rs:163`); `recursive_triggers` is on for
every Kernel connection.
Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
(`crates/kernel/tests/kernel_schema.rs:2177`);
`use_outcome_classification_and_identity_shapes_are_checked`
(`crates/kernel/tests/kernel_schema.rs:2348`);
`an_admission_citation_names_a_recorded_use_outcome_and_never_changes`
(`crates/kernel/tests/kernel_schema.rs:2429`). Status: unaudited.
Impact: History rewriting defeats the witness the record exists to be.
Open questions:
- OQ3-10: a session that continues after its delete reuses its identity at an
  equal composition; resolved at #1155 ticketing as an accepted loss (gap G2).

### uo-p-classification-sensitive-local-only-no-remote-egress

Type: safety
Reachability: default-production
Status: active
Exercised: yes - CHECK refusals by SQL and a serve that creates no registry or
evidence row
Guarantee: Every serving record carries `sensitive`, `local_only`, and
`use_outcome_capture`, the DDL refuses any other value, and no serve creates a
registry object, evidence row, change event, or outbox entry.
Check: `always` - inserts with `normal`, `secret`, `remote_allowed`, or
`canonical` fail the CHECK; a serve leaves the registry, evidence, change-event,
and outbox counts unchanged; the class rule denies Remote for `sensitive`.
Fault/timing angle: none
Required faults and enabling state: Inserts with `normal` and `remote_allowed`;
a serve through the envelope.
Confidence: high -
[evidence](evidence/uo-p-classification-sensitive-local-only-no-remote-egress.md).
CHECKs in the `use_outcomes` DDL (`crates/kernel/src/schema.rs:188`); the insert
names the classes as literals (`insert_use_outcome`
(`crates/kernel/src/use_outcome.rs:293`)).
Existing check: `use_outcome_classification_and_identity_shapes_are_checked`
(`crates/kernel/tests/kernel_schema.rs:2348`);
`a_serve_creates_no_registry_or_evidence_row_and_its_class_denies_remote`
(`crates/kernel/tests/kernel_use_outcome.rs:381`). Status: unaudited.
Impact: A stop condition of the plan: a Remote-eligible serving record.
Open questions:
- Backup classification: the backup scan reads every table with a
  `sensitivity_class` column, so a Kernel holding a serving record classifies as
  at least `sensitive`. Conclusion: resolved by the scan in
  `crates/kernel/src/backup.rs`; no new record.

### uo-p-disputed-after-serve-from-committed-events-only

Type: safety
Reachability: default-production
Status: active
Exercised: yes - one serve and nine admission rows cover every case of the rule
Guarantee: For a serve at `served_commit_seq = n`, `disputed_after_serve` is
true exactly when a committed `mark_disputed` or `contradict` on the served
object and its registry lineage has a commit sequence greater than n.
Check: `always` - an event at n + k sets the flag; an event at or before n, a
`mark_stale`, a null-commit row, another lineage, or no event does not; two
events in one commit set it once.
Fault/timing angle: Two qualifying events in one commit.
Required faults and enabling state: A serve at n; dispositions at n + k, at n,
and before n; a null row and another lineage by SQL.
Confidence: high -
[evidence](evidence/uo-p-disputed-after-serve-from-committed-events-only.md).
the join is `crates/kernel/src/use_outcome.rs:190-228`, using the admission
lineage text through `crates/kernel/src/admission.rs:910`.
Existing check:
`disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
(`crates/kernel/tests/kernel_use_outcome.rs:400`);
`disputed_after_serve_is_judged_against_each_serve`
(`crates/kernel/tests/kernel_use_outcome.rs:826`). Status: unaudited.
Impact: A prior dispute or a stale mark counted as harm after the serve.
Open questions:
- A decision disputed then re-approved still counts: event existence counts
  (resolved at #1154 ticketing).

### uo-p-sweep-retires-only-expired-uncited-rows

Type: safety
Reachability: default-production
Status: active
Exercised: yes - expiry, backlog drain, cited and uncited records, and a
citation committed after selection
Guarantee: The expiry sweep retires a record only when `retain_until` is before
the cutoff and no committed admission row with a live subject cites it, at most
64 per call in one commit keyed by a digest of the page, each under its own
reserved receipt keyed by cutoff and id, rechecked inside that commit; a page
that retires nothing commits nothing; the Kernel maintenance slice calls it once
per pass whenever the Kernel is Ready.
Check: `always` - after a sweep at t, every record it retired had `retain_until
< t` and no live citation at its commit; a backlog of N uncited records drains
in ceil(N / 64) calls (R10); a page another sweep committed at the same cutoff
replays, retires nothing, and reports the cited rows its scan read; a record
kept by a citation at a cutoff retires at that cutoff once uncited; one
maintenance pass with the Kernel Ready runs one sweep (AC16, a bound in passes
rather than an unbounded eventually).
Fault/timing angle: A citation committed between selection and the retiring
commit.
Required faults and enabling state: An expired record; an expired cited record;
70 expired records; a citation inserted after selection.
Confidence: high -
[evidence](evidence/uo-p-sweep-retires-only-expired-uncited-rows.md).
`expire_use_outcomes` (`crates/kernel/src/use_outcome.rs:614`); predicate
`crates/kernel/src/use_outcome.rs:176`; recheck `retire_due_use_outcome`
(`crates/kernel/src/use_outcome.rs:751`); receipt
`crates/kernel/src/local_file.rs:438`.
Existing check: `expiry_retires_an_expired_record_under_a_reserved_receipt`
(`crates/kernel/tests/kernel_use_outcome.rs:568`);
`an_expiry_backlog_drains_one_page_per_call`
(`crates/kernel/tests/kernel_use_outcome.rs:630`);
`an_expired_record_cited_by_a_live_report_is_retained`
(`crates/kernel/tests/kernel_use_outcome.rs:700`);
`a_record_cited_after_selection_is_kept`
(`crates/kernel/src/use_outcome.rs:897`);
`a_page_another_sweep_committed_at_the_same_cutoff_replays`
(`crates/kernel/src/use_outcome.rs:925-926`);
`a_record_retired_after_selection_at_another_cutoff_is_skipped`
(`crates/kernel/src/use_outcome.rs:988`);
`a_record_kept_at_a_cutoff_retires_at_that_cutoff_once_uncited`
(`crates/kernel/src/use_outcome.rs:1009`);
`the_cited_count_covers_the_rows_read_before_the_page_fills`
(`crates/kernel/tests/kernel_use_outcome.rs:956`);
`the_sweep_and_the_reads_use_their_indexes`
(`crates/kernel/src/use_outcome.rs:1026`);
`the_maintenance_slice_expires_use_outcomes_with_the_activation_gate_closed`
(`crates/daemon/src/memory_reviewer/lifecycle.rs:829`);
`a_cancelled_slice_stops_before_the_use_outcome_sweep`
(`crates/daemon/src/memory_reviewer/lifecycle.rs:928`);
`a_cited_use_outcome_kept_by_the_sweep_is_not_progress`
(`crates/daemon/src/memory_reviewer/lifecycle.rs:859`). Status: unaudited.
Impact: A report loses the record it cites, or records never age out.
Open questions:
- OQ3-8 resolved at #1154 ticketing: "live" means the citing row's subject
  object is unretired. Until P4 U3 writes the column, no production row cites a
  record.

### uo-p-record-survives-session-delete

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the Kernel arm (no delete of either table succeeds, a retired
record stays readable) and the `session.delete` arm (every record and decision
row read before the delete reads the same after `ok`, across a stop between the
stores and a retry; the tests run no serve beside the delete)
Guarantee: After `session.delete` answers `ok`, every serving record and
decision row of the session that was readable before it is readable after it,
with the same ids. A serve whose write lands beside the delete adds a record
and removes none (OQ3-10).
Check: `always` - the record ids and decision ids of the session read before
any delete path are a subset of those read after it, and every record read
before keeps its decision ids.
Fault/timing angle: A delete with the Kernel unavailable at step 1 and a retry.
Required faults and enabling state: A session with records, then a delete
(Kernel arm: direct SQL deletes).
Confidence: high - [evidence](evidence/uo-p-record-survives-session-delete.md).
the delete triggers and the retained read hold the Kernel arm (`served_set`
(`crates/kernel/src/use_outcome.rs:468`)); the `session.delete` step only sets
`deleted_at` (`mark_use_outcomes_deleted`
(`crates/kernel/src/use_outcome.rs:358`)).
Existing check: `use_outcome_identity_and_served_set_are_never_rewritten`
(`crates/kernel/tests/kernel_schema.rs:2177`);
`disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
(`crates/kernel/tests/kernel_use_outcome.rs:400`) (a retired record stays
readable);
`a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:41`);
`a_delete_with_nothing_live_commits_nothing_and_an_unready_kernel_converges_on_retry`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:86`). Status: unaudited.
Impact: The record dies at the event it witnesses (KTD1).
Open questions:
- A serve write takes no capture gate, so a pass that committed before the
  delete can insert its record while the delete runs or after its Kernel step
  (OQ3-10); the count after the delete can then exceed the count before. The
  guarantee is preservation of the rows read before, and the cited tests
  construct no concurrent serve.

### uo-p-served-set-once-per-session-revision

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the handler records one serve per served set across folds,
replays an equal set, and records one serve across a CAS retry
Guarantee: For one (project digest, harness, session, served-set digest), at
most one serve commit exists; a later fold at an equal digest under the same
harness and session role, at any `known_as_of`, replays and writes nothing,
while another harness or role at that key conflicts.
Check: `always` - commit-log rows under the serve key number 0 or 1, and a
replayed fold leaves the count unchanged.
Fault/timing angle: A CAS conflict that re-runs the attempt within one pass.
Required faults and enabling state: Two HARD folds with no Kernel change
between; a CAS retry.
Confidence: high -
[evidence](evidence/uo-p-served-set-once-per-session-revision.md). the writer
keys its commit by the project binding, `use-outcome`, the session, and the
served-set digest (`crates/daemon/src/use_outcome_writer.rs:388-391`); the
Kernel holds one record per identity tuple.
Existing check: `a_hard_fold_records_its_frozen_composition_once_per_served_set`
(`crates/daemon/src/use_outcome_writer_tests.rs:70`);
`an_equal_digest_replays_and_another_harness_or_role_conflicts`
(`crates/daemon/src/use_outcome_writer_tests.rs:131`);
`an_equal_served_set_at_a_later_known_as_of_replays`
(`crates/daemon/src/use_outcome_writer_tests.rs:448`);
`a_discarded_attempt_leaves_no_serve_and_the_retry_records_one`
(`crates/daemon/src/use_outcome_writer_tests.rs:314`);
`the_real_failure_paths_count_their_outcomes_and_bound_the_wait`
(`crates/daemon/src/use_outcome_writer_tests.rs:610`);
`a_recorded_served_set_replays_while_another_writer_holds_the_writer`
(`crates/daemon/src/use_outcome_writer_tests.rs:708`);
`every_kernel_error_maps_to_a_closed_outcome`
(`crates/daemon/src/use_outcome_writer_tests.rs:833`);
`an_emergency_pass_that_reruns_records_one_serve`
(`crates/daemon/src/use_outcome_writer_tests.rs:852`);
`only_an_unready_kernel_leaves_the_pass_to_retry_its_serve`
(`crates/daemon/src/use_outcome_writer_tests.rs:880`);
`a_hard_fold_answers_while_another_writer_holds_the_kernel_writer`
(`crates/daemon/src/use_outcome_writer_tests.rs:967`);
`a_fold_past_the_writes_in_flight_counts_a_deadline_and_a_later_fold_repairs_it`
(`crates/daemon/src/use_outcome_writer_tests.rs:1003`); Kernel half:
`duplicate_and_malformed_serves_are_refused_without_rows`
(`crates/kernel/tests/kernel_use_outcome.rs:215`),
`a_repeated_serve_intent_replays_and_a_rebound_key_conflicts`
(`crates/kernel/tests/kernel_use_outcome.rs:790`). Status: unaudited.
Impact: Duplicates inflate served counts and P7 removal sets.
Open questions:
- Receipts last for the database incarnation and serving records for the
  retention window, so a fold at a served set whose record expired replays with
  no live record; recorded as a gap beside G2 (needs human input).
- The receipt key omits the harness while the record identity carries it, so one
  session id served under two harnesses at one served set is a lasting conflict;
  the design takes session ids as unique within a project (needs human input).
- A deadline, an unready Kernel, or a lost fence leaves the serve for a later
  write at its digest: a rerun in the same pass when the Kernel was not Ready at
  the hand-off and no write was handed off (a write handed to a Ready Kernel
  settles the pass whatever its outcome), or a later HARD fold that another
  input requests over the same served set, such as context pressure. The pass
  hands the write to a tracked blocking job and answers while the job waits for
  the Kernel writer; when every write slot is taken, the write is counted
  `deadline` and repaired the same way. This placement departs from the
  ticket's OQ3-5 resolution, an inline write within the pass's remaining
  budget: the job's own 250 ms writer wait is the budget AC6 names, and the
  measured wait the ticket asked for is recorded by
  `the_real_failure_paths_count_their_outcomes_and_bound_the_wait` (owner
  decision, taken at the #1155 review rounds). A session whose composition never folds
  HARD again at that set stays unrecorded. A conflict and a refusal are not
  repaired by a later fold.

### uo-p-served-ids-equal-frozen-composition

Type: safety
Reachability: default-production
Status: active
Exercised: yes - the recorded ids equal the snapshot rows of the fold, before
and after a composition change and a dispute, and the record a CAS retry
writes names the retry's persisted composition
Guarantee: The stored decision set equals, as a set, the object ids of the
snapshot rows whose composition the same fold persisted.
Check: `always` - for every committed serve, the stored ids equal the frozen
snapshot ids of that attempt.
Fault/timing angle: A CAS retry; a composition change between passes.
Required faults and enabling state: A read trimmed by the token budget; a CAS
retry.
Confidence: high -
[evidence](evidence/uo-p-served-ids-equal-frozen-composition.md). the ids and
digest come from the pinned snapshot the same fold froze (`served_ids`
(`crates/daemon/src/canonical_memory.rs:72`), `served_set_digest`
(`crates/daemon/src/canonical_memory.rs:82`)), and the Kernel refuses ids that
disagree with the digest.
Existing check: `a_hard_fold_records_its_frozen_composition_once_per_served_set`
(`crates/daemon/src/use_outcome_writer_tests.rs:70`);
`a_dispute_between_folds_records_the_next_served_set_without_it`
(`crates/daemon/src/use_outcome_writer_tests.rs:484`);
`the_served_set_digest_names_the_rendered_ids_and_not_their_bytes`
(`crates/daemon/src/canonical_memory.rs:342`);
`rows_past_the_memory_budget_are_neither_injected_nor_digested`
(`crates/daemon/src/canonical_memory.rs:477`);
`a_fold_whose_boundary_read_fails_after_its_commit_records_its_serve`
(`crates/daemon/src/use_outcome_writer_tests.rs:1032`); Kernel half:
`duplicate_and_malformed_serves_are_refused_without_rows`
(`crates/kernel/tests/kernel_use_outcome.rs:215`),
`session_roles_round_trip_and_the_served_set_digest_is_order_free`
(`crates/kernel/tests/kernel_use_outcome.rs:811`). Status: unaudited.
Impact: The record names findings the session never saw.
Open questions: None.

### uo-p-no-row-when-memory-disabled

Type: safety
Reachability: explicit-config-only
Status: active
Exercised: yes - a route bound with memory disabled folds HARD and attempts no
write
Guarantee: A route bound with `memory.enabled` off produces no serving record on
any pass.
Check: `always(!X)` - X is "a record exists for a session whose binding had
memory disabled"; a forbidden state with no dedicated detection point.
Fault/timing angle: none
Required faults and enabling state: A route bound with `/memory/enabled: false`,
then HARD folds on both engines.
Confidence: high - [evidence](evidence/uo-p-no-row-when-memory-disabled.md). a
disabled route takes no read, so `env.project_memory` is `None` and
`record_use_outcome` (`crates/daemon/src/lib.rs:9721`) returns before any write.
Existing check: `only_an_available_composition_is_recorded`
(`crates/daemon/src/use_outcome_writer_tests.rs:180`). Status: unaudited.
Impact: Consent violation (R5).
Open questions: None.

### uo-p-no-row-for-withheld-composition

Type: safety
Reachability: default-production
Status: active
Exercised: partial - a withheld read attempts no write, an Available read of
the same rows then records once, and an empty Available composition records a
serve with no decision rows; the withheld arm runs with the Kernel unopened for
the whole pass, so a Kernel that is Starting at the read and Ready at the write
is not constructed
Guarantee: A fold that persists a withheld composition writes no serving record;
an Available snapshot with zero rows at a tip of at least one commit attempts a
record with zero decision rows, which exists once the write lands as `written`
or as `replayed` against a live record; a receipt that outlives its expired
record replays with no live row (the open question under
`uo-p-served-set-once-per-session-revision`). At tip 0 (a Kernel with no commit) the
composition names no commit, `served_commit_seq` has no row to reference, and
the writer records nothing; the Kernel refuses `served_commit_seq = 0` as
`InvalidInput`.
Check: `always(!X)` - X is "a record whose fold persisted a withheld
composition". The positive arm is exercised, not guaranteed: an Available
zero-row composition whose write lands against a live or absent record leaves
a record whose `served_set` read answers zero decision ids, while a `deadline`, `kernel_unavailable`, or lost
fence leaves no record until a later HARD fold at the same served set writes or
replays it (`uo-p-served-set-once-per-session-revision`).
Fault/timing angle: A Kernel Ready for the write but starting at the read.
Required faults and enabling state: A Kernel starting or a refusing serving
verdict at read time, then a HARD fold.
Confidence: high - [evidence](evidence/uo-p-no-row-for-withheld-composition.md).
the writer acts only on `CanonicalMemoryRead::Available` (`record_use_outcome`
(`crates/daemon/src/lib.rs:9721`)).
Existing check: `only_an_available_composition_is_recorded`
(`crates/daemon/src/use_outcome_writer_tests.rs:180`);
`a_withheld_read_then_an_available_read_of_the_same_rows_records_once`
(`crates/daemon/src/use_outcome_writer_tests.rs:520`);
`the_real_failure_paths_count_their_outcomes_and_bound_the_wait`
(`crates/daemon/src/use_outcome_writer_tests.rs:610`); Kernel half:
`served_set_answers_records_and_distinguishes_no_record_from_an_empty_serve`
(`crates/kernel/tests/kernel_use_outcome.rs:164`). Status: unaudited.
Impact: A record claiming a serve that did not render.
Open questions:
- OQ3-11 resolved at #1155 ticketing: an empty Canonical composition writes a
  record with zero decision rows.
- Tip 0, raised in #1176 review: `UseOutcomeSpec::validated_retain_until`
  (`crates/kernel/src/use_outcome.rs:266-267`)
  requires `1 <= served_commit_seq < created_commit_seq` and the column
  references `commit_log`, so a serve at tip 0 is not recordable. The first
  commit of any kind ends that state. Resolved for this catalog by the carve-out
  in the guarantee; a genesis sentinel would be a #1155 decision.

### uo-p-producer-matches-request-subagent-flag

Type: safety
Reachability: default-production
Status: active
Exercised: yes - a subagent pass records `subagent` on the additive engine and
nothing on the folding engine; a primary pass records `primary` on each engine
Guarantee: The stored `session_role` is `subagent` exactly when the writing
pass's effective subagent flag was true.
Check: `always` - the stored role equals the flag of the pass that wrote the
serve.
Fault/timing angle: none
Required faults and enabling state: A subagent request on the additive engine
with a Canonical read; a primary on each engine.
Confidence: high -
[evidence](evidence/uo-p-producer-matches-request-subagent-flag.md). the role
comes from the pass's `is_subagent`
(`crates/daemon/src/use_outcome_writer.rs:395`). Reachability per engine (B1):
`subagent` rows arise only on the additive engine.
Existing check: `the_session_role_follows_the_pass_subagent_flag_on_each_engine`
(`crates/daemon/src/use_outcome_writer_tests.rs:385`); Kernel half:
`session_roles_round_trip_and_the_served_set_digest_is_order_free`
(`crates/kernel/tests/kernel_use_outcome.rs:811`). Status: unaudited.
Impact: Harm attributed to the wrong session kind.
Open questions:
- OQ3-9 resolved at #1154 ticketing: the column is `session_role`.

### uo-p-deleted-at-iff-session-delete-kernel-step

Type: safety
Reachability: default-production
Status: active
Exercised: yes - a delete marks the records before the memory delete; a stop
between the stores, an unknown commit outcome, and a Kernel that is not Ready
each converge on one repeat that keeps the first timestamp; a second delete
marks records written after the first; a route teardown leaves the records live
Guarantee: `deleted_at` is set exactly when a `session.delete` for the session
committed its Kernel step, before the memory-store delete; route teardown leaves
it null.
Check: `always` - after `ok` every record the Kernel step selected as live has
`deleted_at` set and the memory rows are gone; a record expiry retired before
the delete keeps its state with `deleted_at` null, and a serve whose job lands
after the step is live until the next delete (OQ3-10); after teardown it is
null; after
`kernel_unavailable` the memory rows are intact, and `deleted_at` is null when
the Kernel was not Ready and set when the step's commit ran and its answer was
an error; one repeat with the Kernel Ready completes `ok` and keeps the first
timestamp (R9).
Fault/timing angle: A stop between the stores, then a retry.
Required faults and enabling state: A delete with the Kernel available; one with
it starting; a route close without delete.
Confidence: high -
[evidence](evidence/uo-p-deleted-at-iff-session-delete-kernel-step.md). the step
runs inside the capture gate after the descriptor retirement and before the
memory delete (`delete_kernel_step` (`crates/daemon/src/lib.rs:6883`)); the
envelope selects the session's live records under the writer
(`mark_use_outcomes_deleted` (`crates/kernel/src/use_outcome.rs:358`)), and the
set-once trigger refuses a changed value (`crates/kernel/src/schema.rs:188`).
Existing check:
`a_delete_marks_the_records_before_the_memory_rows_go_and_a_retry_keeps_the_first_time`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:41`);
`a_delete_with_nothing_live_commits_nothing_and_an_unready_kernel_converges_on_retry`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:86`);
`a_delete_step_with_an_unknown_outcome_converges_on_retry`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:136`);
`a_second_delete_marks_records_written_after_the_first`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:177`);
`a_route_torn_down_after_a_hard_fold_leaves_its_records_live`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:208`);
`a_session_id_that_reads_as_a_secret_still_deletes`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:483`); Kernel half:
`a_session_delete_marks_each_live_record_once`
(`crates/kernel/tests/kernel_use_outcome.rs:1063`). Status: unaudited.
Impact: A deleted session reads as live, or a live one as deleted.
Open questions:
- A record written after the delete commits, by a pass on a route still bound to
  the session (OQ3-10), stays live until a later delete marks it; each delete
  selects the live records afresh under its own attempt key. This includes a
  pass whose Memory Store commit preceded the delete and whose serve write
  landed after it: the harness received that composition, and the serve writer
  takes no capture gate, so the record counts as a continued session.
- A fold after a delete over a composition the deleted record already holds
  replays that record, so the continued session gains no live record for it
  until its composition changes (needs human input).
- A conflict or invalid input in the step answers `source_retirement_failed` and
  counts no lifecycle name; the closed names cover the marked, nothing-live, and
  unavailable outcomes only.

### uo-p-last-transform-at-monotone-per-committing-pass

Type: safety
Reachability: default-production
Status: active
Exercised: yes - a committing pass advances the value, a non-committing pass and
a pass whose clock reads before the stored value leave it, a Kernel unavailable
or a held writer skips it with the pass `ok`, and the next committing pass
catches up; the spent-budget arm is checked at the writer function with a
spent `PassWriteBudget`, not through a pass
Guarantee: `last_transform_at` never decreases on a session's serving record,
and only a pass whose response committed updates it.
Check: `always` - after every pass the value is at least the old value, and
unchanged when the pass did not commit.
Fault/timing angle: Two passes with skewed clocks.
Required faults and enabling state: A committing then a non-committing pass; a
Kernel unavailable at the write.
Confidence: high -
[evidence](evidence/uo-p-last-transform-at-monotone-per-committing-pass.md). the
settle step checks the session's live records once per committed pass on a
memory-enabled route (`use_outcomes_to_touch`
(`crates/daemon/src/lib.rs:9815`)) and, when one needs the update, makes it on a
scoped thread beside the response build that the settle joins before the
response returns (`crates/daemon/src/lib.rs:10128`, `touch_use_outcomes`
(`crates/daemon/src/lib.rs:9849`)), within what remains of the pass's Kernel
wait (`crates/daemon/src/use_outcome_writer.rs:345`); a pass that handed its
serve to a tracked job leaves the touch to that job, which checks and updates
after its serve commits with a Kernel wait of its own (`touch_after_serve`
(`crates/daemon/src/use_outcome_writer.rs:319`)); the Kernel update moves
only live records whose value is null or earlier, in one fenced transaction
with no commit that holds only the writer (`advance_use_outcome_last_transform`
(`crates/kernel/src/use_outcome.rs:560`)).
Existing check: `last_transform_at_moves_forward_only_on_committing_passes`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:227`);
`a_held_writer_skips_the_update_and_the_next_committing_pass_catches_up`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:314`);
`a_spent_pass_budget_skips_the_touch`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:362`);
`a_serve_deadline_leaves_the_touch_its_own_kernel_wait`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:391`);
`a_touch_waiting_on_the_writer_overlaps_the_settle_and_lands_before_the_response`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:428`); Kernel half:
`last_transform_at_advances_live_records_without_a_commit`
(`crates/kernel/tests/kernel_use_outcome.rs:1118`),
`a_held_writer_bounds_the_advance_and_an_idle_session_skips_it`
(`crates/kernel/tests/kernel_use_outcome.rs:1202`),
`an_outside_file_lock_bounds_the_advance_at_begin`
(`crates/kernel/tests/kernel_use_outcome.rs:1239`),
`the_staleness_check_answers_for_null_or_earlier_values_and_writes_nothing`
(`crates/kernel/tests/kernel_use_outcome.rs:1285`),
`the_advance_takes_no_pooled_reader`
(`crates/kernel/tests/kernel_use_outcome.rs:1317`). Status: unaudited.
Impact: A wrong last-activity attribution.
Open questions:
- OQ3-6 resolved at #1156 ticketing: a fenced, receipt-free monotone update.
- The pass budget is the 250 ms Kernel wait the pass spends across the touch's
  staleness read and its write (`PassWriteBudget`); the daemon carries no other
  pass deadline. The touch bounds its connection waits, its begin, and its
  statements by that budget. A pass that handed its serve to a tracked job
  makes no touch of its own: the job touches after its serve, under a budget of
  the same size, so the serving pass's record carries that pass's time and the
  response waits on neither write.
- Cost: every committing pass of a session with a stale live record (a value
  null or before the pass's time; the staleness read skips the writer
  otherwise) makes one fenced
  commit, on a scoped thread beside the response build, so its device flush
  overlaps the memory-store flush of the pass trace. Measured on this host
  (release build, one session with one record, paired runs of 240 committing
  passes): with a fixed 1 ms flush on tmpfs the committing pass p50 is 4.86 ms
  against 5.90 ms for the touch made before the response build; on XFS over
  EBS NVMe it is 8.68 to 8.95 ms against 9.99 to 10.17 ms. A coarser stale
  predicate would skip most of these commits and changes the
  per-committing-pass rule (needs human input).
- A pass that commits and then fails its boundary read answers an error before
  the settle step; it hands off its serve, and the job's touch advances the
  record with that pass's time, because the pass committed
  (`a_fold_whose_boundary_read_fails_after_its_commit_records_its_serve`).
  When every write slot is taken, the serve counts `deadline`, no job runs, and
  the error path makes no touch of its own, so the record keeps its earlier
  value until the next committing pass; no lifecycle name counts that skip.
- A committing pass that lands while an earlier pass's serve job still waits
  for the writer finds no record at its staleness check and touches nothing;
  the job then advances the record to its own pass's time, and the later
  pass's time is recorded by the next committing pass. This is the convergence
  a skipped touch already has; whether the attribution must name that later
  pass without a further pass is an owner decision (needs human input).
- A session rebound to a route with memory disabled keeps its records untouched
  by those passes, because such a route takes no Kernel read (C2).
- A pass with an earlier clock selects no record, because the update filters on
  a stored value before `now`; the trigger never sees a decrease, and the pass
  ignores any write failure (C11).

### uo-p-counts-exact-and-unavailable-not-zero

Type: safety
Reachability: default-production
Status: active
Exercised: partial - the routed read answers (2, 1) for two serves with a
contradiction between them, skips a retired serve, answers (0, 0) for a decision
never served, and answers an unready Kernel with its state and no count; the
Kernel read answers `Deadline` under a held reader pool; the CLI accepts 2^53,
refuses 2^53 + 1, and renders every other answer unavailable. The route's answer
for a read that fails while the Kernel is Ready has no daemon test.
Guarantee: Served and disputed-after-serve counts per decision are exact
unsigned integers at one Kernel snapshot; a Kernel that is not Ready answers
its `unavailable` kernel state with no integer on the wire, which the CLI
renders as `unavailable`.
Check: `always` - k unretired records of which j have a later dispute answer (k,
j); an unready Kernel answers no integer; the client accepts 2^53 and refuses
2^53 + 1.
Fault/timing angle: none
Required faults and enabling state: A Kernel starting; one decision served to
two sessions with one later dispute.
Confidence: high -
[evidence](evidence/uo-p-counts-exact-and-unavailable-not-zero.md). one
statement counts the by-decision rows at one snapshot (`served_counts_for`
(`crates/kernel/src/use_outcome.rs:588`)); the handler answers counts or the
kernel state (`crates/daemon/src/memory_reviewer/wire.rs:308`); the client
decodes each count with `exactCount` and renders any other answer unavailable
(`packages/cli/src/commands/review-wire.ts:344`).
Existing check:
`review_use_outcome_counts_serves_and_later_disputes_of_one_decision`
(`crates/daemon/tests/memory_reviewer_wire.rs:686`);
`review_use_outcome_answers_an_unready_kernel_with_its_state_and_no_count`
(`crates/daemon/src/use_outcome_lifecycle_tests.rs:512`);
`packages/cli/src/commands/review-wire.contract.test.ts:107`;
`packages/cli/src/commands/review-wire.contract.test.ts:125`;
`packages/cli/src/commands/review-wire.contract.test.ts:137`;
`packages/cli/src/commands/review.test.ts:781`;
`packages/cli/src/commands/review.test.ts:822-847`;
`packages/cli/src/commands/review.test.ts:849`; Kernel half:
`disputed_after_serve_is_judged_against_each_serve`
(`crates/kernel/tests/kernel_use_outcome.rs:826`) and
`disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage`
(`crates/kernel/tests/kernel_use_outcome.rs:400`); the read bound:
`a_held_reader_pool_bounds_the_count_read`
(`crates/kernel/tests/kernel_use_outcome.rs:1262`) and
`review_use_outcome_reads_the_kernel_under_a_bounded_budget`
(`crates/daemon/tests/memory_reviewer_wire.rs:787`). Status: unaudited.
Impact: A zero misread as "no harm observed".
Open questions:
- OQ3-2 and OQ3-15 resolved at #1157 ticketing: a `review.use_outcome` method
  answering the Kernel state outcome when unready.
- `served_sessions` counts unretired serving records, one per (harness,
  session, served set), so a session served the decision at two served sets
  counts twice; the name follows the ticket.
- The counts cover the bound project only; another project's serves of the same
  decision are not counted.
- A failed read after the Kernel is Ready answers the kernel state its failure
  maps to. The read runs under a 5 s budget; a held reader pool stops the
  Kernel read with `Deadline` (`a_held_reader_pool_bounds_the_count_read`),
  which maps to `unavailable`/`store_busy`; no daemon test drives that
  state answer. AC19 (no identifier in logs or `host.status`) holds by
  inspection: the handler logs nothing and adds no status field.

## Relationship map

- Write path: `uo-composition-freeze-persists-at-every-hard-fold-site` and
  `uo-served-rows-are-the-frozen-m0-rows-between-hard-folds` supply the ids
  that `uo-p-served-ids-equal-frozen-composition` and
  `uo-p-served-set-once-per-session-revision` record; consent comes from
  `uo-memory-disabled-route-takes-no-canonical-read` and
  `uo-consent-gate-source-is-the-route-frozen-config`, which
  `uo-p-no-row-when-memory-disabled` and `uo-p-no-row-for-withheld-composition`
  depend on.
- Dispute join: `uo-known-as-of-is-the-tip-bound-read-sequence`,
  `uo-disputed-decision-is-hidden-on-auto-inject-and-labeled-on-explicit-search`,
  `uo-admission-reads-require-a-non-null-commit-seq`, and
  `uo-disposition-events-append-rows-and-newest-by-commit-seq-wins` are the
  conditions `uo-p-disputed-after-serve-from-committed-events-only` rests on.
- Retention: `uo-capture-expiry-retires-at-most-a-page-per-call-under-reserved-receipts`
  and `uo-expiry-keeps-pinned-and-cited-evidence` are the loop and liveness
  idiom `uo-p-sweep-retires-only-expired-uncited-rows` reuses.
- Lifecycle: `uo-session-delete-retires-kernel-sources-before-memory-rows-under-the-capture-gate`,
  `uo-route-teardown-purges-process-state-but-deletes-no-durable-row`, and
  `uo-per-session-pass-timestamps-die-with-the-session` bound
  `uo-p-deleted-at-iff-session-delete-kernel-step`,
  `uo-p-last-transform-at-monotone-per-committing-pass`, and
  `uo-p-record-survives-session-delete`; `uo-p-served-set-and-identity-never-rewritten`
  is the trigger floor under all three.
- Baseline and egress: `uo-kernel-opens-only-at-the-compiled-baseline-identity`
  and `uo-activation-gate-closes-on-kernel-baseline-or-incarnation-change`
  (disposition B) bound `uo-p-baseline-rotation-refuses-previous-family-untouched`;
  `uo-sensitive-class-denies-remote-and-unknown-egress-is-local-only` bounds
  `uo-p-classification-sensitive-local-only-no-remote-egress`.
- Observability: `uo-status-counters-are-closed-vocabulary-exact-integers-with-unavailable-not-zero`
  and `uo-review-reads-authorize-by-bearer-and-project-binding-not-owner`
  bound `uo-p-counts-exact-and-unavailable-not-zero`.
