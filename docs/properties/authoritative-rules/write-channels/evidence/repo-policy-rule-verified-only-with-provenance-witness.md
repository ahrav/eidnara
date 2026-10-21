# repo-policy-rule-verified-only-with-provenance-witness

## Discovery trigger

Plan requirement R2 ("admitted as `(trusted_local_code, current_config)`
under `ConfigObserved` citing a live `config_present` observation on the
rule's own lineage, backed by an artifact that carries
`RepositoryProvenance`"), R12 (the Kernel's own verdict), KTD2 ("`validate_
trigger` promotes only when the observation sits on the subject's exact
lineage triple"; "affirmative provenance keeps the artifact `Normal`"), U5
execution note ("the trigger lineage match is the part most likely to be
wrong") and test scenarios "a root rule is admitted Verified and Visible";
"without the observation the same admission degrades to `Other`". Forbidden
outcome F5.

## Evidence trail

Verified at HEAD:

- `crates/kernel/src/admission.rs:2726-2795` `validate_trigger`: for
  `ConfigObserved`, requires `trigger_object_id`; queries a live observation
  object whose `source_kind`, `source_id`, `source_revision` equal the
  subject's, with `observation_kind='config_present'`, live, and whose
  `evidence_id` is either null or a live `evidence_meta` row; returns the
  stricter of the observation's and evidence's sensitivity. No trigger or no
  match returns `None`.
- `admission.rs:1603-1644` `prepare_with_facts`: `trigger.is_none()` turns
  the event into `EventKind::Other`; `admission.rs:4159` `Other` has outcome
  Deny and raises nothing (`admission.rs:4184-4195`). Trigger sensitivity is
  folded into `candidate_sensitivity` (`:1604-1605, 1631`).
- `admission.rs:4197-4206` `automatic_ceiling(TrustedLocalCode,
  CurrentConfig) == Verified`; `admission.rs:4140-4150` `event_effect(
  ConfigObserved)` targets Verified; `auto_admit_event(ConfigObserved)` is
  true (`:4172-4182`), so no approval is needed up to Verified.
- `admission.rs:4243-4255` `visibility_row(Verified, Active) == Automatic`;
  `admission.rs:4116-4138` `surface_visibility(Automatic, AutoInject,
  Normal) == Visible`, Sensitive or Secret Hidden.
- `crates/kernel/src/cas/ingest.rs:184-207`: Normal survives only with
  affirmative provenance on an inspected UTF-8 payload without detections.
- `crates/kernel/src/slice/write.rs:190-192`: an observation citing evidence
  takes at least the evidence's class.
- `admission.rs:2569-2591` `validate_provenance` applies to candidate
  subjects only; decision subjects have `provenance_kind: None`
  (`admission.rs:2156`), so it does not constrain this path.
- Daemon test-support pattern for observation-then-admit:
  `crates/daemon/src/lib.rs:13853-13889` `observe_code`.
- Tests: `crates/kernel/tests/kernel_admission.rs:1292`
  `a_config_observation_authorizes_only_a_config_event`; `:1323`
  `an_observation_at_another_source_revision_cannot_authorize`; `:1339`
  `an_invalidated_observation_cannot_authorize`; `:4150`
  `an_admitted_object_carries_the_class_of_its_trigger_and_the_evidence_behind_it`;
  `:4211` `a_trigger_whose_evidence_is_gone_cannot_admit`.

## Failure scenario

Legal witness: artifact A for `AGENTS.md` at oid X, provenance `{digest, X}`,
Normal. Observation O on `("repository_policy", "<digest>:AGENTS.md", 1)`,
kind `config_present`, `evidence_id = A`. Decision rule(u1,1) on the same
triple. `record_admission(ConfigObserved, TrustedLocalCode/CurrentConfig,
trigger O)`: Verified, Active, automatic, Normal; Visible on AutoInject.

Smallest violating traces:

1. O on revision 1, decision on revision 2 (plan's N+1 off by one between
   observation and units): `validate_trigger` finds nothing; event becomes
   `Other`; Deny; the rule is Candidate and Hidden. Fail-closed; AE3 fails.
2. O inserted in the next envelope (after the admissions): at admission time
   O does not exist; same degradation.
3. O cites no evidence (`evidence_id: None`): the trigger still validates
   (null is allowed), the rule is Verified, but the "backed by an artifact
   that carries `RepositoryProvenance`" clause of R2 is not witnessed by the
   Kernel; the oracle in this record (join evidence through O) fails. The
   plan's R2 requires the citation.
4. `AGENTS.md` contains a token the redactor classifies as a secret: A is
   Secret; O is Secret; `candidate_sensitivity` is Secret; the rule is
   Hidden on every surface. Fail-closed.
5. A trigger `trigger_object_id` naming a `code_present` observation:
   `expected` mismatch; `Other`; Deny.

## Timing windows and dependencies

O and the admissions must share one envelope or O must be committed first;
the artifact must be committed before O's `fold_cited_evidence_class` runs.
Evidence retirement later invalidates the trigger for future admissions only;
the served row keeps its stored class with the trigger fold recomputed at
serve time (`admission.rs:3411-3463`).

## What a test must construct

- The U5 execution-note test: one rule admitted; assert
  `effective_maturity='verified'`, `visibility='automatic'`,
  `sensitivity_class='normal'`, `trigger_object_id = O`, and a `kernel.read`
  on `auto_inject` serves it.
- The degraded twin: identical envelope with `trigger_object_id: None`;
  assert `event_kind` stored as `other`, outcome `deny`, not served.
- A revision-skewed observation; assert deny.
- An `AGENTS.md` with a fake AWS key pattern; assert the artifact is Secret,
  the rule Hidden, and nothing panics.
- Ledger oracle: for every Verified `repository_policy` admission, the joined
  observation has matching lineage triple, kind `config_present`, live
  evidence with `sensitivity_class='normal'`, and the artifact's provenance
  fields non-empty.

## Investigation log

### Q: Does the Kernel force the observation to cite the artifact?
- Sources examined: `admission.rs:2745-2770` (evidence may be null).
- Findings: no; citation is a plan obligation enforced by the module.
- Missing evidence: the module.
- Conclusion: resolved with answer: plan obligation, checked by the oracle.

### Q: Can an observation's object id collide across revisions?
- Sources examined: `write.rs:169-200` `insert_observation`; plan U5.
- Findings: the plan does not name the observation object id; it must embed
  the revision or oid to avoid a duplicate insert.
- Missing evidence: design statement.
- Conclusion: unresolved, resolvable at implementation.
