# Kernel admission and scope eligibility: implemented-behavior catalog (S1, S2)

Method: `docs/properties/METHOD.md` in the repository. Discovery pass for the
authoritative-rules plan; every record here describes behavior implemented at
HEAD, cataloged as a claim under test.

Provenance: repository `/local/home/ahrav/scratch/eidnara`, HEAD
`08de9051a735f136ff63a9dca49eaa11493b3006`, authored 2026-10-21. Every
`file:line` was checked against that commit while writing.

## Scope

Surfaces S1 (Kernel admission and authority mechanics) and S2 (scope
eligibility) from the shared brief, bounded to the behavior the settled plan
`docs/plans/2026-10-21-1902-feat-authoritative-rules-automatic-use-plan.md`
depends on or extends. New channels N1 to N5 belong to another pass; where
the plan relies on an existing guarantee, that guarantee is a record here.

Files read: `crates/kernel/src/admission.rs`, `crates/kernel/src/slice/write.rs`,
`crates/daemon/src/kernel_routes/commit.rs`, `crates/daemon/src/kernel_routes/project.rs`,
`crates/daemon/src/kernel_routes/read.rs`, `crates/kernel/src/eligibility.rs`,
`crates/kernel/src/scope.rs`, and the tests named in `existing-checks.md`.

Assumptions recorded:

- A1. Scope is the user-confirmed scope in the brief (2026-10-21). Untracked
  local research notes were not consulted.
- A2. "Production writer" means a call site compiled without `test-support`
  or `direct-host-fixture`; verified by grep and by the `cfg` gate at
  `crates/daemon/src/lib.rs:13517`.
- A3. The plan's line references were taken as leads and re-verified; each
  correction is noted in the record that uses it.

## External evidence consulted

| Reference | Why consulted | What it contributed |
| --- | --- | --- |
| Settled plan (above) and its Appendix source brief | names the guarantees the new channels depend on | the priority list of mechanisms; line references re-verified |
| `docs/properties/METHOD.md`, `AGENTS.md`, `README.md` | record contract and catalog layout | schema, semantics table, reachability labels |
| `docs/properties/canonical-positive-claim-projection/catalog.md` | existing records on scope binding, authority, and fresh-snapshot judgement | four reused records (table below) |
| `docs/properties/evidence-backed-memory-review/catalog.md` | existing records on review succession and Candidate ceiling | one reused record |
| Issue #307 (closed) via `gh issue view` | the acceptance criteria for canonical memory injection | serve-path contract: retired, superseded, other-project rows absent |
| Issue #353 (closed) | shared kernel eligibility with daemon verdicts | confirms one scope judgement (`names_project`) for route and kernel |
| Issue #311 (closed) | closed, build-checked privileged-key table | precedent for closed-table discipline; no S1/S2 defect |
| Issues #579, #657, #408, #709 (`--comments`) | settled review-path decisions the plan honors | Candidate ceiling for model output; no S1/S2 defect reported |
| Tracker-wide `gh issue list --state all --search` for "memory injection", "admission", "scope matching", "project memory", "harness parity", "packing budget", "AcceptedAdr", "revoke_approval", "supersede", "ScopeFilter" | leads for bugs or incidents | no bug or incident in S1/S2; no behavior-level regression promoted |
| `docs/host-wire-protocol.md` | wire contract for `kernel.commit` classes | the document does not specify `derive_classes`; the contract lives in the code comment at `commit.rs:237-239` |

## Reachability classes

- `default-production`: reached by a routed `kernel.commit` or a per-turn
  read with no configuration beyond a bound project.
- `explicit-config-only`: reached only behind an operator gate.
- `test-only`: the enabling state (an `AcceptedAdr` or `ConfigObserved`
  event, an approval object, a scope term outside `project`) has no
  production writer at HEAD; only tests construct it.

Per-record labels carry their evidence; no blanket claim applies.

## Index

| Slug | Type | Reachability | Status | Existing check |
| --- | --- | --- | --- | --- |
| `explicit-user-accepted-adr-self-approves-to-approved` | safety | test-only | active | `admission.rs::an_accepted_adr_roots_authority_only_from_an_accepted_decision_object`; `kernel_routes.rs::seed_approval` callers |
| `accepted-adr-requires-live-accepted-decision-subject` | safety | test-only | active | `kernel_admission.rs::only_an_accepted_decision_object_self_approves` |
| `config-observed-promotes-only-on-exact-lineage-trigger` | safety | test-only | active | `kernel_admission.rs::a_config_observation_authorizes_only_a_config_event` and three sibling tests |
| `sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority` | safety | default-production | active | `admission.rs::visibility_matrix_and_sensitivity_ceiling_are_total`; `kernel_admission.rs::a_hidden_approval_is_not_authority` |
| `authority-loss-cascades-to-latest-dependents` | safety | test-only | active | `kernel_admission.rs::approval_revocation_fans_out_support_demotion_without_rewriting_history` and four sibling tests |
| `decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse` | safety | default-production | active | `kernel_slice.rs::supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor` and three sibling tests |
| `explicit-reject-moves-to-rejected-review-only-and-bars-succession` | safety | default-production | active | `kernel_routes.rs::disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`; `kernel_admission.rs::every_non_active_disposition_has_the_contracted_surface_row` |
| `plugin-commit-classes-are-closed-at-model-inference` | safety | default-production | active | `commit.rs::a_pair_is_accepted_exactly_when_not_raised_and_legal_for_admission`; `kernel_routes.rs::a_plugin_route_cannot_declare_a_class_above_the_derived_one` |
| `policy-digest-pins-revision-one` | safety | test-only | active | `admission.rs::policy_revision_matches_policy_content` |
| `scope-filter-excludes-a-term-on-an-unresolved-dimension` | safety | test-only | active | `kernel_eligibility.rs::a_scope_names_a_project_only_through_an_exact_project_term`; `kernel_scope_algebra.rs::missing_context_value_for_constrained_dimension_is_uncertain` |
| `sql-project-prefilter-is-a-superset-of-the-daemon-verdict` | safety | default-production | active | none |
| `served-rows-exclude-objects-invalidated-at-the-snapshot` | safety | default-production | active | `transform_canonical_memory.rs::transform_composes_project_memory_from_canonical_rows_and_observes_absence` |

Semantics distribution: 11 `always`, 0 `always-or-unreached`, 0 `sometimes`,
0 `reachable`, 0 `unreachable`. Situation coverage for the `always` records
is listed in `fault-map.md` as precondition markers to add.

## Records

### explicit-user-accepted-adr-self-approves-to-approved

Origin: implemented behavior

Type: safety
Reachability: test-only - no production code records `EventKind::AcceptedAdr`
(grep over `crates/*/src`; the only daemon `AcceptedAdr`-adjacent helper is
under the `test-support` gate at `crates/daemon/src/lib.rs:13517`). The
daemon test helper `seed_approval` (`crates/daemon/tests/kernel_routes.rs:4356-4387`)
constructs it.
Status: active
Exercised: partial - the evaluator path is covered at the `EvaluationInputs`
level and the store path through `seed_approval`; no test reads the result
back on `AutoInject` through `read_visible` and asserts `Visible`.
Guarantee: Recording `AcceptedAdr` at `(explicit_user, user_explicit)` on a
live `adr_accepted` decision with `Normal` sensitivity yields effective
maturity `Approved`, disposition `Active`, visibility row `automatic`, and
`Visible` on `AutoInject`, with no `approval_object_id` cited or stored.
Check: `always` - for the admission row written by that request,
`effective_maturity == Approved && disposition == Active && visibility ==
Automatic && approval_object_id IS NULL`, and `surface_visibility(Automatic,
AutoInject, Normal) == Visible`; the outcome is `Admit` for a first decision.
`always` because every evaluation of this input must produce this row; the
plan's directive channel relies on it on every record.
Fault/timing angle: none in the evaluator; the subject must exist in the same
or an earlier commit (see the next record).
Required faults and enabling state: a live `decision` object with
`decision_kind = adr_accepted`, `sensitivity_class = normal`, admitted with
`AcceptedAdr` and classes `(ExplicitUser, UserExplicit)`.
Confidence: high - [evidence](evidence/explicit-user-accepted-adr-self-approves-to-approved.md).
Verified `automatic_ceiling(ExplicitUser, UserExplicit) == Verified`
(`admission.rs:4197-4202`), `admission_ceiling(AcceptedAdr, Verified, true)
== Approved` (`:4221-4234`), `event_effect(AcceptedAdr) == (Approved, None,
Promote)` (`:4148`), `requires_approval` false because `Approved <=
admission_ceiling` and `auto_admit_event(AcceptedAdr)` (`:324-329`,
`:4172-4182`), `visibility_row(Approved, Active) == Automatic` (`:4243-4246`),
`surface_visibility(Automatic, AutoInject, Normal) == Visible` (`:4116-4138`),
and that `prepare_with_facts` does not back-fill a stored approval for
`AcceptedAdr` (`:1569-1580`).
Existing check: `crates/kernel/src/admission.rs::an_accepted_adr_roots_authority_only_from_an_accepted_decision_object`
(`:5066`) evaluates both `Verified`-ceiling pairs; `crates/daemon/tests/kernel_routes.rs::seed_approval`
(`:4356`) constructs the store sequence and later tests use the row as an
approval; status unaudited.
Impact: a directive that fails to self-approve stays `Candidate` and
`explicit_labeled`, so no user directive is ever auto-injected; a directive
that self-approves at the wrong class could carry authority for rows it
should not.
Open questions:
- `(explicit_user, user_inferred)` also has a `Verified` ceiling and would
  self-approve an `adr_accepted` decision, but cannot act as an approval
  (`approval_row_fields` requires `user_explicit`). No writer exists for it;
  whether the directive route must refuse that taint is a design question for
  the N1 pass (needs human input).

### accepted-adr-requires-live-accepted-decision-subject

Origin: implemented behavior

Type: safety
Reachability: test-only - as for the previous record; the enabling event has
no production writer.
Status: active
Exercised: partial - a non-decision subject (a domain object) is covered; a
decision of another `decision_kind` and a missing object are read from code.
Guarantee: An `AcceptedAdr` request self-approves only when its subject
exists as a live `decision` object of kind `adr_accepted` at the time the
decision is prepared; a missing subject is refused with `NotFound`, and a
live subject of any other kind is denied and stays at its automatic ceiling.
Check: `always` - `record_admission(AcceptedAdr, (ExplicitUser, UserExplicit))`
on a missing object returns `Err(NotFound)`; on a live decision whose kind is
not `adr_accepted` or on a non-decision object the stored row has `outcome
== deny`, `effective_maturity == Candidate`, `visibility == explicit_labeled`;
on a live `adr_accepted` decision the previous record holds. `always` because
the ordering is a per-request precondition.
Fault/timing angle: the insert and the admission must land in one envelope
or in commit order; a reversed order within one envelope is a `NotFound`
because `load_subject_facts` reads the registry inside the transaction.
Required faults and enabling state: three subjects: absent, a `decision` of
kind other than `adr_accepted`, and a non-decision object, each submitted
under `AcceptedAdr` with explicit-user classes.
Confidence: high - [evidence](evidence/accepted-adr-requires-live-accepted-decision-subject.md).
Verified `load_subject_facts` returns `NotFound` for an absent or invalidated
object (`admission.rs:2126-2174`); `subject_is_accepted_decision` is the
`APPROVAL_OBJECT_PREDICATE` on the subject (`:2436-2458`, `:710-714`);
`admission_ceiling` returns the automatic ceiling when it is false
(`:4221-4234`), so `requires_approval` is true (`Approved > Verified`) and
`denied` without an approval (`:324-330`).
Existing check: `crates/kernel/tests/kernel_admission.rs::only_an_accepted_decision_object_self_approves`
(`:1740`) covers the domain-object subject;
`crates/kernel/src/admission.rs::an_accepted_adr_roots_authority_only_from_an_accepted_decision_object`
(`:5066`) covers `subject_is_accepted_decision = false` at the evaluator;
status unaudited.
Impact: an `AcceptedAdr` accepted against a look-alike subject would mint
authority for any object a caller names.
Open questions: None.

### config-observed-promotes-only-on-exact-lineage-trigger

Origin: implemented behavior

Type: safety
Reachability: test-only - no production code records `ConfigObserved`; the
plugin route admits with `EventKind::Other` (`commit.rs:462`).
Status: active
Exercised: partial - exact-lineage success, kind mismatch, revision mismatch,
another source, and an invalidated observation are covered on staged
candidates; the decision-object subject path with `ConfigObserved` and a
`config_present` observation is read from code only.
Guarantee: A `ConfigObserved` admission at `(trusted_local_code,
current_config)` reaches effective maturity `Verified` and visibility
`automatic` only when `trigger_object_id` names a live `observation` object
of kind `config_present` whose `(source_kind, source_id, source_revision)`
equals the subject's, with live backing evidence when it cites any;
otherwise the request is evaluated as `Other` and the stored row has
`outcome == deny` and `effective_maturity == Candidate`.
Check: `always` - after the admission, `effective_maturity == Verified &&
visibility == automatic && trigger_object_id == <observation id>` exactly
when the observation row satisfies the SQL predicate at
`admission.rs:2749-2775`; for every other trigger (absent, wrong kind, wrong
lineage triple, invalidated, dead evidence) `outcome == deny &&
effective_maturity == Candidate && trigger_object_id IS NULL`. `always`
because every repository-policy admission in the plan depends on it.
Fault/timing angle: the observation must be committed before or inside the
same envelope; an observation invalidated between its write and the admission
fails the `invalidated_commit_seq IS NULL` test. The observation's
sensitivity (and its evidence's) is folded into the decision's
(`:1605`, `:1631`), so a `Sensitive` observation yields a `Sensitive`
decision hidden on `AutoInject`.
Required faults and enabling state: a subject decision and a `config_present`
observation on the same lineage; negative controls: a `code_present`
observation, an observation at revision N+1, one on another `source_id`, an
invalidated one, and one whose backing evidence is invalidated.
Confidence: high - [evidence](evidence/config-observed-promotes-only-on-exact-lineage-trigger.md).
Verified `validate_trigger` (`:2726-2795`) and the fall-through to
`EventKind::Other` (`:1640-1644`); `event_effect(Other) == (current, None,
Deny)` (`:4159`); `automatic_ceiling(TrustedLocalCode, CurrentConfig) ==
Verified` (`:4199-4202`); `auto_admit_event(ConfigObserved)` (`:4172-4182`).
Existing check: `crates/kernel/tests/kernel_admission.rs::a_config_observation_authorizes_only_a_config_event`
(`:1292`), `an_observation_at_another_source_revision_cannot_authorize`
(`:1323`), `an_invalidated_observation_cannot_authorize` (`:1339`),
`observation_from_another_source_cannot_authorize_candidate` (`:677`),
`a_trigger_whose_evidence_is_gone_cannot_admit` (`:4211`),
`an_admitted_object_carries_the_class_of_its_trigger_and_the_evidence_behind_it`
(`:4150`); status unaudited.
Impact: a trigger accepted off-lineage lets one observation verify every
decision in the store; a trigger refused on-lineage leaves repository rules
`Candidate` and never injected.
Open questions:
- `validate_trigger` reads the observation's registry row, not its admission
  classes. An observation written through the plugin route at
  `model_inference` classes on the right lineage would satisfy the predicate.
  The event itself is unreachable from the route today; whether the N2 writer
  must own the observation lineage exclusively is for that pass (partial:
  code read, no test).

### sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority

Origin: implemented behavior

Type: safety
Reachability: default-production - `surface_visibility` runs on every served
read (`admission.rs:3321-3346`); the authority half needs an approval object,
which only tests create at HEAD.
Status: active
Exercised: partial - the visibility matrix is total over
`(VisibilityRow, Surface, Sensitivity)`; a hidden approval is shown to grant
nothing; a `Sensitive` self-approved `adr_accepted` decision read on
`AutoInject` is read from code only.
Guarantee: A decision whose served sensitivity is `Sensitive` is `Hidden` on
`AutoInject` and `AutoSearch` whatever its maturity, and a decision whose
object, history, or cited evidence is not `Normal` grants no authority as an
approval; maturity itself is not lowered by sensitivity.
Check: `always` - `surface_visibility(row, AutoInject | AutoSearch,
Sensitive) == Hidden` and `surface_visibility(_, _, Secret) == Hidden` for
every row; `validate_approval(x)` is false whenever `o.sensitivity_class !=
'normal'`, the strictest history class is not `normal`, or the cited
evidence is not `normal` (`approval_qualifies_predicate`); and for a
`Sensitive` self-approved `adr_accepted` decision the stored
`effective_maturity` is still `Approved`. `always` because the plan's
"sensitivity must be Normal" rests on these two checks at every read.
Fault/timing angle: a classification tightened after admission (an ingest
replay on cited evidence) changes the served class at the next read without
a new admission row (`load_subject_facts` folds `evidence_meta`,
`admission.rs:2140-2154`).
Required faults and enabling state: a self-approved `adr_accepted` decision
whose registry sensitivity is `Sensitive`, or whose trigger or cited
evidence is `Sensitive`; a read on `AutoInject`; a dependent citing it as
approval.
Confidence: high - [evidence](evidence/sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority.md).
Verified `surface_visibility` (`:4116-4138`), `approval_row_fields`
(`:843-856`), `approval_qualifies_predicate` (`:861-905`), and that
`evaluate_admission` composes sensitivity (`:353-360`) without reading it in
the maturity path.
Existing check: `crates/kernel/src/admission.rs::visibility_matrix_and_sensitivity_ceiling_are_total`
(`:4910`); `crates/kernel/tests/kernel_admission.rs::a_hidden_approval_is_not_authority`
(`:4326`), `governing_decision_sensitivity_hides_an_object_the_registry_calls_normal`
(`:3422`), `dispositions_and_sensitivity_only_reduce_visibility` (`:2735`);
status unaudited.
Impact: a sensitive directive or rule auto-injected would disclose
classified content on an automatic surface; a sensitive approval honored
would let classified content vouch for others.
Open questions: None.

### authority-loss-cascades-to-latest-dependents

Origin: implemented behavior

Type: safety
Reachability: test-only - every path (`revoke_approval`, `retire_decision`,
`supersede_decision`, a restrictive `Disposition`) is callable from the
daemon, but the enabling state (an approval object with dependents) has no
production writer at HEAD.
Status: active
Exercised: partial - revocation, retirement, quarantine, and transitive
chains are covered at the store level; supersession of an approval with
dependents is read from code (`write.rs:448-452`).
Guarantee: When a decision that granted authority stops granting it, through
`revoke_approval`, `retire_decision`, `supersede_decision`, or an admitted
restrictive disposition, every subject whose latest committed decision cites
it with `elevated_support = 1` receives an `ApprovalRevoked` row in the same
transaction that lowers effective maturity to
`min(prior.effective_maturity, automatic_ceiling)` and leaves historical
maturity unchanged; dependents beyond `MAX_AUTHORITY_DEMOTIONS` (4,096) or on
another `policy_revision` are counted as deferred and still traversed.
Check: `always` - after the invalidating commit, for each dependent `d`
selected by `load_approval_dependents(X)` before the commit: the latest row
on `d` has `event = approval_revoked`, `effective_maturity == min(previous
effective, ceiling(d.classes))`, `maturity` unchanged, and the audit on the
revocation change carries `demoted + deferred == |dependents|` for
`revoke_approval`. `always` because the plan's revoke and revise flows depend
on it per operation.
Fault/timing angle: the walk runs inside the invalidating transaction under
the single writer; a dependent past the cap keeps its stored elevated row
until a later write, while `validate_approval` already answers false for it
(the "visibility repair" comment at `admission.rs:1424-1446`).
Required faults and enabling state: an approval `A`, at least one dependent
admitted with `approval_object_id = A` and elevated support, optionally a
dependent approval `B` with its own dependents; then each of the four
invalidating paths.
Confidence: high - [evidence](evidence/authority-loss-cascades-to-latest-dependents.md).
Verified `revoke_approval_inner` (`:1359-1424`), `demote_authority_dependents`
(`:1447-1486`), `demote_one_dependent` (`:1488-1524`),
`remaining_support` (`:1617-1621`), `with_authority_cascade` (`:1055-1082`),
`retire_slice_object` (`write.rs:627-669`), and `correct_decision_inner`
(`write.rs:362-456`).
Existing check: `crates/kernel/tests/kernel_admission.rs::approval_revocation_fans_out_support_demotion_without_rewriting_history`
(`:469`), `revocation_cascades_through_dependent_approvals_and_strips_their_authority`
(`:1582`), `retiring_an_approval_demotes_its_dependents` (`:2614`),
`quarantining_an_approval_demotes_its_dependents_and_ends_its_authority`
(`:1503`), `revocation_survives_a_dependent_on_a_superseded_policy_revision`
(`:2030`), `an_unvisited_descendant_of_a_revoked_root_grants_nothing`
(`:2081`); status unaudited.
Impact: a revoked or revised directive would keep supporting rows it
approved, so withdrawn user authority keeps surfacing content.
Open questions:
- The deferred set keeps stored elevated visibility until "a later write";
  no bound on that window exists in code. Whether the N1 pass needs a
  follow-up repair step is a design question (needs human input).

### decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse

Origin: implemented behavior

Type: safety
Reachability: default-production - `kernel.commit` `supersede_decision`
(`commit.rs:787-850`) and `memory_capture.rs:1197` call it.
Status: active
Exercised: yes - fresh successor, fold into a live survivor, quarantined
predecessor, quarantined survivor, cross-domain refusal, and
sensitivity floor are each constructed by store or route tests.
Guarantee: `supersede_decision(old, spec)` succeeds only when `spec` keeps
`old`'s `domain_id`, `source_kind`, and `source_id`, carries a
`source_revision` strictly greater than `old`'s, names an `object_id` that
is not `old`'s, and neither `old`'s nor a live survivor's latest admission
disposition is `Quarantined`, `Rejected`, or `Contradicted`; on success
`old` is invalidated at the commit and linked by `superseded_by` to the
successor, the successor's sensitivity is at least `old`'s, and no admission
row is written on `old`.
Check: `always` - a spec differing in domain, source kind, or source id is
`Err(InvalidInput)`; `source_revision <= old.source_revision` is
`Err(Conflict)` (route: `revision_not_advanced`); `spec.object_id ==
old.object_id` with `old` live is `Err(InvalidInput)`; a barred predecessor
or survivor is `Err(AdmissionPolicy)`; after success `old.invalidated_commit_seq
== commit_seq && old.superseded_by == Some(new)` and `COUNT(admission_decisions
WHERE subject_object_id = old AND commit_seq = this commit) == 0`. `always`
because every directive revise and repository reconcile is a succession.
Fault/timing angle: none; single-writer transaction.
Required faults and enabling state: a live predecessor with a known lineage;
specs varying one field at a time; a predecessor whose latest disposition is
`Rejected` (currently untested) and `Quarantined`.
Confidence: high - [evidence](evidence/decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse.md).
Verified `validate_successor` (`write.rs:1271-1285`), the survivor identity
check (`write.rs:388-394`), `refuse_barred_lineage` (`admission.rs:1339-1349`),
`supersede_decision_inner` (`:1320-1337`), `set_successor` (`write.rs:434`),
and that `Correct`/`Replace` rows are written only by `supersede_domain`
(`:1252-1305`).
Existing check: `crates/kernel/tests/kernel_slice.rs::supersede_decision_replaces_a_live_predecessor_and_folds_merges_into_a_survivor`
(`:1095`), `supersede_decision_refuses_a_quarantined_predecessor` (`:1186`),
`a_fold_refuses_a_quarantined_survivor` (`:1225`),
`corrections_reject_cross_domain_replacements` (`:1002`),
`a_correction_cannot_relabel_a_predecessor_below_its_class` (`:1335`);
`crates/kernel/tests/kernel_admission.rs::superseding_a_quarantined_predecessor_is_rejected`
(`:991`); `crates/daemon/tests/kernel_routes.rs::a_merge_folds_every_predecessor_into_one_survivor_in_one_commit`
(`:1066`); status unaudited.
Impact: a succession that changed lineage would detach a rule from its
observation and from its own history; identity reuse would let a successor
masquerade as its predecessor to token holders.
Open questions:
- The kernel does not tie the successor's admission classes to the
  predecessor's: the successor is a new subject with no prior, so
  `evaluate_admission`'s class-consistency rule (`admission.rs:268-274`)
  does not apply across succession. The plan (KTD3) describes class
  continuity as a kernel property; it is a route property. Lead L2.
- The kernel requires strictly greater revision, the plan says `+1`. Lead L1.

### explicit-reject-moves-to-rejected-review-only-and-bars-succession

Origin: implemented behavior

Type: safety
Reachability: default-production - `kernel.commit` `disposition` with event
`explicit_reject` (`commit.rs:113-119`, `:856-868`).
Status: active
Exercised: partial - the fixed table and the approval-gated relaxation are
covered through the route; a supersede of a `Rejected` predecessor is read
from code (tests cover `Quarantined` only).
Guarantee: An `ExplicitReject` admitted on a live decision needs no approval,
moves the disposition to `Rejected`, lowers effective maturity to
`Candidate`, sets the visibility row to `review_only` (`Hidden` on every
surface), and from then on the subject cannot be superseded or used as a
fold survivor, and any transition toward a less restrictive disposition is
denied without a valid approval.
Check: `always` - after the disposition the latest row has `disposition ==
rejected && effective_maturity == candidate && visibility == review_only`;
`surface_visibility(ReviewOnly, s, _) == Hidden` for all three surfaces;
`supersede_decision(rejected, _)` is `Err(AdmissionPolicy)`; a later
`mark_stale` without approval is `denied == true` with the disposition
unchanged. `always` because the plan's `confirmed_from` flow rejects the
candidate on every promotion.
Fault/timing angle: when the rejected subject is itself an approval, the
authority cascade runs in the same commit (`with_authority_cascade`).
Required faults and enabling state: a live decision; an `explicit_reject`
disposition; a follow-up supersede and a follow-up relaxing disposition
without approval.
Confidence: high - [evidence](evidence/explicit-reject-moves-to-rejected-review-only-and-bars-succession.md).
Verified `event_effect(ExplicitReject) == (current, Some(Rejected), Reject)`
(`admission.rs:4145`), `auto_admit_event` includes it (`:4172-4182`),
support withdrawal for non-active dispositions (`:345-351`),
`visibility_row(_, Rejected) == ReviewOnly` (`:4252`), `surface_visibility`
(`:4116-4138`), `refuse_barred_lineage` (`:1339-1349`), and
`relaxes_disposition` (`:320-323`).
Existing check: `crates/daemon/tests/kernel_routes.rs::disposition_events_apply_the_fixed_table_and_report_outcome_and_disposition`
(`:4392`), `a_relaxation_is_denied_without_a_valid_approval_and_permitted_with_one`
(`:4452`); `crates/kernel/tests/kernel_admission.rs::every_non_active_disposition_has_the_contracted_surface_row`
(`:2890`), `relaxing_a_restrictive_disposition_requires_approval`
(`admission.rs:5334`); status unaudited.
Impact: a rejected candidate that stayed visible or could be superseded back
into service would undo the user's refusal.
Open questions:
- At HEAD the route builds a disposition request from the subject's stored
  classes (`commit.rs:589-590`), so a `model_inference` caller can reject an
  `explicit_user` decision. The plan's KTD9 guard is a proposed obligation
  (N1); this record describes the kernel semantics only. Lead L4.

### plugin-commit-classes-are-closed-at-model-inference

Origin: implemented behavior

Type: safety
Reachability: default-production - every `kernel.commit` request passes
`resolve_classes` (`commit.rs:283-316`) before any write.
Status: active
Exercised: yes - every `(source_kind, SourceClass, TaintClass)` triple over
the four kinds is enumerated in a unit test; the route refuses an asserted
`explicit_user` and an unknown kind.
Guarantee: A `kernel.commit` request is admitted under
`(ModelInference, t)` where `t` is `AssistantInference` for `assistant` and
`model`, `MemoryClassifierInference` for `memory_classifier`, and
`UserInferred` for `user`; any other `source_kind` (including
`user_directive` and `repository_policy`) is `invalid_input`; an asserted
class ranked above the derived one is `class_over_declared`; and every
insert or successor the route admits uses `EventKind::Other`, so its
effective maturity is `Candidate` and its visibility `explicit_labeled`.
Check: `always` - `derive_classes(k).map(|(s, _)| s) == Some(ModelInference)`
for the four kinds and `None` otherwise; `resolve_classes` returns
`ClassOverDeclared` whenever `source_rank(asserted) < 5` or
`taint_rank(asserted) < taint_rank(derived)`; the admission row for a route
insert has `source_class == model_inference && effective_maturity ==
candidate && visibility == explicit_labeled`. `always` because the plan's
authority hierarchy rests on this table never widening.
Fault/timing angle: none.
Required faults and enabling state: requests with `source_kind` in the four
kinds, with `user_directive`, `repository_policy`, and an unknown string;
asserted `explicit_user`, `trusted_local_code`, and `current_config`.
Confidence: high - [evidence](evidence/plugin-commit-classes-are-closed-at-model-inference.md).
Verified `derive_classes` (`commit.rs:240-252`), `source_rank` (`:255-264`),
`taint_rank` (`:267-275`), `resolve_classes` (`:283-316`), `admit`
(`:451-467`), and `automatic_ceiling` for every `ModelInference` pair
(`admission.rs:4197-4219`).
Existing check: `crates/daemon/src/kernel_routes/commit.rs::a_plugin_write_is_capped_at_inference_classes`
(`:1071`), `a_pair_is_accepted_exactly_when_not_raised_and_legal_for_admission`
(`:1117`); `crates/daemon/tests/kernel_routes.rs::a_plugin_route_cannot_declare_a_class_above_the_derived_one`
(`:1145`); status unaudited.
Impact: a widened table would let a model write a row that packs in the
required phase or self-approves.
Open questions: None.

### policy-digest-pins-revision-one

Origin: implemented behavior

Type: safety
Reachability: test-only - a `#[cfg(test)]` constant and test; the pin runs
under `cargo test`, which CI requires.
Status: active
Exercised: yes - the test runs in every CI test job.
Guarantee: `POLICY_REVISION` is 1 and the SHA-256 over the vocabularies plus
the `evaluator`, `tables`, `authority`, `chain`, and every `serving` section
of `admission.rs` equals `REVISION_1_SOURCE_DIGEST`, so any change to the
policy text fails the build until the revision is advanced.
Check: `always` - `policy_revision_matches_policy_content` passes: the digest
computed from the marked sections equals
`097ed31de1a3f3a49b83de0cedf83e7a1b9850f1cfce8d33aa4fa777d7ca0d17`. `always`
because the plan's R4 ("policy unchanged") is verified by this single test.
Fault/timing angle: none.
Required faults and enabling state: a build of the kernel test target.
Confidence: high - [evidence](evidence/policy-digest-pins-revision-one.md).
Verified the constant (`admission.rs:20-23`), the marker pairs
(`:264`, `:393`, `:437`, `:661`, `:663`, `:907`, `:2594`, `:2700`, `:2798`,
`:3038`, `:3376`, `:3960`, `:4066`, `:4299`), and the assertion (`:5819-5820`).
Existing check: `crates/kernel/src/admission.rs::policy_revision_matches_policy_content`
(`:5734-5821`); status unaudited.
Impact: an unpinned policy change would reinterpret every stored admission
row under a revision number that still reads 1.
Open questions: None.

### scope-filter-excludes-a-term-on-an-unresolved-dimension

Origin: implemented behavior

Type: safety
Reachability: test-only - `read_visible` runs the filter on every per-turn
read, but the only production scope writer emits a single `project` term
(`project.rs:81-99`), so a term on another dimension exists only in tests.
Status: active
Exercised: partial - `names_project` is tested with a `branch` term and
`scope_matches` with a missing context value; no test builds a stored scope
with an `entity` term and reads through `read_visible`.
Guarantee: `ScopeFilter::matches` returns true only when the row has a
`scope_id`, a stored scope row exists and decodes canonically, the scope
carries a `project` term, and every term matches the context: an `Exact`
term matches when the context value equals it, a `Set` term when the
context value is a member, a term on a dimension with no context value or a
redacted placeholder term is `Uncertain` and the row is excluded, and a
single `DoesNotMatch` excludes the row.
Check: `always` - for `(terms, ctx)`: `matches == (terms.is_some() &&
from_term_specs(terms).is_ok() && scope.term(Project).is_some() &&
scope_matches(scope, ctx, UnknownGraph) == Matches)`, where
`term_matches(Exact(e)) == bool_outcome(e == v)`, `term_matches(Set(s)) ==
bool_outcome(s.contains(v))`, and `term_matches(_)` with `ctx.value(d) ==
None` is `Uncertain`. `always` because the plan's fail-closed scope rule is
this function on every row of every pass.
Fault/timing angle: verdicts are memoized per `scope_id` for one request
(`project.rs:198-206`); a scope changed mid-request is not re-judged, but
scope rows are immutable once written.
Required faults and enabling state: stored scopes with (a) `project` only,
(b) `project` plus `entity` exact, (c) `project` plus `entity` set, (d)
`entity` only, (e) a redacted `project` value; contexts with and without an
`entity` value.
Confidence: high - [evidence](evidence/scope-filter-excludes-a-term-on-an-unresolved-dimension.md).
Verified `ScopeFilter::matches` (`project.rs:194-208`), `names_project`
(`eligibility.rs:86-94`), `ScopeMatchContext::new().with_value(Project, _)`
(`:81`), `scope_matches` (`scope.rs:1450-1471`), `term_matches`
(`:1392-1437`), and `from_term_specs` duplicate and unknown-dimension
refusals (`:628-659`).
Existing check: `crates/kernel/tests/kernel_eligibility.rs::a_scope_names_a_project_only_through_an_exact_project_term`
(`:148`); `crates/kernel/tests/kernel_scope_algebra.rs::missing_context_value_for_constrained_dimension_is_uncertain`
(`:325`), `placeholder_terms_never_match_or_subsume` (`:272`),
`definitive_mismatch_skips_later_graph_terms` (`:335`);
`crates/daemon/tests/kernel_routes.rs::rows_serve_only_to_the_project_their_scope_names`
(`:1803`); status unaudited.
Impact: a rule carrying a directory, branch, or environment term would be
served to a pass that cannot resolve that dimension, which is exactly the
out-of-scope injection the plan forbids.
Open questions:
- `Dimension::Entity` has no production writer at HEAD (grep: only
  `crates/kernel/tests/kernel_scope_algebra.rs:426`; the `"entity"` strings
  in `kernel_schema.rs` are object kinds). Resolved for this pass; the N3
  pass owns the `dir:` prefix collision question.

### sql-project-prefilter-is-a-superset-of-the-daemon-verdict

Origin: implemented behavior

Type: safety
Reachability: default-production - `read_visible` binds
`project.scope_term()` on every read (`read.rs:178-184`).
Status: active
Exercised: not yet - no test compares the SQL-kept set against
`names_project` over the same scopes; the route tests observe only the final
rows.
Guarantee: The single-dimension SQL prefilter in `served_classes_sql` never
drops a row that `ScopeFilter::matches` would keep: it drops only rows whose
scope carries an `exact` or `set` `project` term with a definite,
unredacted value different from the binding digest, and it keeps rows with
no scope, no `project` term, a non-`exact`/`set` operator, or a redacted
value for the daemon filter to judge.
Check: `always` - for every object `o` in the registry at the snapshot,
`kept_by_sql(o) || !names_project(terms(o.scope_id))`; equivalently the set
of object ids returned by `visible_as_of_in_scope(.., Some(project term))`
is a superset of the ids `read_visible` returns after `ScopeFilter`. `always`
because the daemon filter is the deciding judgement and the SQL may only
narrow toward it, never past it.
Fault/timing angle: both run at the same `:governing_as_of`, so no skew.
Required faults and enabling state: scopes with a matching project term, a
foreign project term, no project term, a redacted project value, a
`range` operator on `project`, and a `NULL` scope.
Confidence: medium - [evidence](evidence/sql-project-prefilter-is-a-superset-of-the-daemon-verdict.md).
Verified the predicate (`admission.rs:3548-3562`) and that both `exact_value`
and `set_values` placeholders keep the row, that a scope with no term on the
dimension is kept by the `NOT IN` branch, and that `names_project` refuses
every one of those kept classes; the superset claim is read from the SQL,
not executed.
Existing check: none.
Impact: a prefilter stricter than the filter would silently hide a row the
kernel judges in scope; a prefilter looser than the SQL comment claims costs
only performance, which the comment at `read.rs:174-177` accepts.
Open questions: None.

### served-rows-exclude-objects-invalidated-at-the-snapshot

Origin: implemented behavior

Type: safety
Reachability: default-production - every `read_visible` and every
`visible_as_of_in_scope` read runs the `served_classes_sql` predicate; the
m0 compose path takes this read on every pass with `memory_enabled`.
Status: active
Exercised: partial - `transform_canonical_memory.rs:46` retires one memory
decision and supersedes another before a pass and asserts both texts are
absent from the served block (`:70-71`, `:146-147`); no test pins the
boundary case where `:governing_as_of` equals the invalidating commit.
Guarantee: A served-class read at snapshot `governing_as_of` returns only
registry objects whose `created_commit_seq <= governing_as_of` and whose
`invalidated_commit_seq` is null or strictly greater than `governing_as_of`;
`retire_decision` and `supersede_decision` set the predecessor's
`invalidated_commit_seq` to the committing sequence, so a retired or
superseded rule disappears from every AutoInject read whose snapshot is at
or after that commit, and the successor appears in the same read.
Check: `always` - for every row returned by `read_visible(AutoInject,
as_of)`, `row.object.invalidated_commit_seq.is_none_or(|seq| seq > as_of)`;
and after one envelope that retires decision A and supersedes decision B
with B2 at commit C, a read at `as_of >= C` contains neither A nor B and
contains B2, while a read at `as_of = C - 1` contains A and B and not B2.
`always` because the predicate is part of the SQL every served read
executes; a forbidden state (a stale row served) has no detection point of
its own, so the check is `always(!served)` on the row set.
Fault/timing angle: the snapshot boundary. A pass pinned at `as_of` before
the invalidating commit serves the old row by design; the next pass at a
later snapshot drops it and the revision digest changes. No window exists
inside one read because the predicate and the row fetch run in one
statement.
Required faults and enabling state: a live decision that is Visible on
AutoInject; one envelope that retires or supersedes it; two reads, one
pinned before and one at or after that commit. Supersession additionally
needs the successor spec on the same lineage at a greater revision.
Confidence: high - [evidence](evidence/served-rows-exclude-objects-invalidated-at-the-snapshot.md).
Read the predicate at `admission.rs:3543-3546`, the registry update in
`object_write.rs:90-110`, the supersede call at `write.rs:410-415`, and the
retire entry at `write.rs:543`; read the integration test's shaping commit
and absence assertions.
Existing check: `crates/daemon/tests/transform_canonical_memory.rs:46`
`transform_composes_project_memory_from_canonical_rows_and_observes_absence`
(retire and supersede before the pass; absence asserted on served text) -
unaudited. `eligibility.rs:208-214` returns `Superseded` and `Retracted`
verdicts for the explicit egress path, a separate consumer of the same
registry columns - unaudited.
Impact: a retired or superseded directive or repository rule stays in the
agent's context after the user or a new HEAD removed it; R7 and R8 are
false at the read layer even though the write layer did its work.
Open questions:
- Whether the equality boundary (`as_of` equal to the invalidating commit)
  is meant to exclude the row, as the strict `<` in the predicate implies,
  is a claim this record states from the SQL; no test pins it.

## Reused existing records

| Slug | Catalog path | Why it applies |
| --- | --- | --- |
| `bound-project-scope-cannot-be-widened-by-candidate` | `docs/properties/canonical-positive-claim-projection/catalog.md:981` | the mandatory `project` term and `WrongScope` for a foreign digest are the first half of S2; `names_project` is the shared judgement |
| `retrieved-content-cannot-upgrade-write-authority` | `docs/properties/canonical-positive-claim-projection/catalog.md:464` | record content (summary text, category) grants no class; the directive channel's look-alike candidate stays `model_inference` by this record plus `plugin-commit-classes-are-closed-at-model-inference` |
| `supporting-authority-is-preserved-not-recomputed` | `docs/properties/canonical-positive-claim-projection/catalog.md:532` | the stored `approval_object_id` and its validity at the snapshot are what the cascade record demotes and what serving reads |
| `stale-projection-cannot-authorize-current-use` | `docs/properties/canonical-positive-claim-projection/catalog.md:862` | every use is judged at a fresh snapshot; a revoked directive is denied at the next read without a projection catching up |
| `future-correction-keeps-kernel-maturity-and-retrieval` | `docs/properties/evidence-backed-memory-review/catalog.md:393` | a review-path revise writes a fresh successor that inherits no maturity, with a different object id and `superseded_by` on the predecessor; the same succession mechanics the directive revise reuses |

## Contract-versus-code leads

- L1. Plan KTD3 and R7 say successors land at `source_revision + 1`; the
  kernel and the route require only `source_revision > predecessor`
  (`write.rs:1281`, `commit.rs:833-834`). The plan is stricter than the code.
- L2. Plan KTD3 says "succession cannot cross lineage or class" citing
  `admission.rs:268-274`; that rule compares a request against the same
  subject's prior. A successor is a new subject, so the kernel admits it
  under whatever classes the caller supplies. Class continuity is a route
  obligation.
- L3. Plan AE5 says predecessors are "superseded"; the kernel invalidates
  the registry row and sets `superseded_by`, writing no
  `Disposition::Superseded` admission row. Eligibility reports `Superseded`
  from the registry (`eligibility.rs:209-210`).
- L4. Plan KTD9's baseline: the route's `RetireDecision`, `SupersedeDecision`,
  and `Disposition` ops do not consult the caller's derived class against
  the subject's stored class (`commit.rs:589-590`, `:851-854`). The code
  agrees with the plan's motivation; the guard is proposed, not present.
- L5. Plan KTD2 cites `validate_provenance` territory; decision subjects
  carry `provenance_kind: None` into admission (`admission.rs:2156`), so
  repository provenance protects artifact sensitivity at ingest, not the
  decision's admission class.
- L6. `adr_accepted` is not in `POSITIVE_MEMORY_CATEGORIES`
  (`crates/daemon/src/memory_render.rs:67-80`); at HEAD a self-approved
  directive decision is `Visible` at the kernel and dropped by the renderer.
  Owned by the S4 pass; the plan's KTD6 addresses it.

## Relationship map

- `accepted-adr-requires-live-accepted-decision-subject` is the precondition
  of `explicit-user-accepted-adr-self-approves-to-approved`; both share
  `admission_ceiling` and `subject_is_accepted_decision`. If the first
  holds, a forged self-approval through the second is unreachable.
- `sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority`
  narrows both self-approval records and the cascade record: the same
  `approval_qualifies_predicate` decides authority grant and the cascade's
  `validate_approval` check.
- `config-observed-promotes-only-on-exact-lineage-trigger` and
  `explicit-user-accepted-adr-self-approves-to-approved` are the two
  Verified-or-above paths without an approval object; both depend on
  `automatic_ceiling` giving `Verified` to their class pair, which
  `policy-digest-pins-revision-one` freezes.
- `authority-loss-cascades-to-latest-dependents` is triggered by
  `decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse`
  (via `correct_decision_inner`), by `retire_decision`, by `revoke_approval`,
  and by `explicit-reject-moves-to-rejected-review-only-and-bars-succession`
  when the rejected subject was an approval. Suspected dominance: if the
  cascade record holds for `revoke_approval`, it holds for the other three
  entry points because all call `demote_authority_dependents` through the
  same `demote_dependents_if_authority_lost`.
- `explicit-reject-moves-to-rejected-review-only-and-bars-succession` and
  the succession record share `refuse_barred_lineage`.
- `served-rows-exclude-objects-invalidated-at-the-snapshot` is the read-side
  complement of the succession record, `retire_decision`, and the cascade:
  the write side sets `invalidated_commit_seq`, the served-class SQL reads
  it; a successor becomes the only live row of its lineage in the same
  snapshot that drops the predecessor.
- `plugin-commit-classes-are-closed-at-model-inference` is the route-side
  complement of the reused `retrieved-content-cannot-upgrade-write-authority`
  and the only reason the two Verified paths are unreachable from a plugin.
- `scope-filter-excludes-a-term-on-an-unresolved-dimension` extends the
  reused `bound-project-scope-cannot-be-widened-by-candidate` from the
  Project term to every term; `sql-project-prefilter-is-a-superset-of-the-daemon-verdict`
  guarantees the filter sees every candidate row. If the SQL record fails,
  the scope record passes vacuously for the dropped rows.
