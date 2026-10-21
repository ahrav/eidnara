# config-observed-promotes-only-on-exact-lineage-trigger

## Discovery trigger

Plan KTD2: repository policy rules are admitted under `ConfigObserved` with
`trigger_object_id` naming a `config_present` observation; "`validate_trigger`
promotes only when the observation sits on the subject's exact lineage
triple". The plan's U5 execution note calls the lineage match "the part most
likely to be wrong", so the existing guarantee is cataloged with its exact
predicate.

## Evidence trail

- `crates/kernel/src/admission.rs:2726-2795` `validate_trigger`:
  - no `trigger_object_id` yields `Ok(None)` (`:2733-2735`);
  - expected kind is `code_present` for `CodeObserved`, `config_present` for
    `ConfigObserved` (`:2737-2741`);
  - the SQL (`:2749-2775`) requires `o.object_kind='observation'`,
    `o.invalidated_commit_seq IS NULL`, `o.source_kind=?3`, `o.source_id=?4`,
    `o.source_revision=?5` bound to the subject's facts,
    `observed.observation_kind=?2`, `observed.invalidated_commit_seq IS NULL`,
    and either no evidence or a live `evidence_meta` row;
  - a match returns `Some(Some(sensitivity))` where sensitivity is the
    stricter of observation and backing evidence (`:2779-2786`); a miss
    returns `Ok(None)` (`:2787`).
- `:1603-1613` `prepare_with_facts` folds the trigger sensitivity into
  `candidate_sensitivity` and keeps `trigger_object_id` only on a match.
- `:1640-1644` the event passed to the evaluator is `request.event.kind` when
  `trigger.is_some()` and `EventKind::Other` otherwise.
- `:4159` `event_effect(Other)` is `(current, None, Deny)`; a first decision
  stays `Candidate`, `visibility_row(Candidate, Active)` is `ExplicitLabeled`
  (`:4248-4251`).
- `:4199-4203` `automatic_ceiling(TrustedLocalCode, CurrentConfig)` is
  `Verified`; `:4150-4152` `event_effect(ConfigObserved)` is `(Verified, None,
  Promote)`; `:4172-4182` the event auto-admits, so no approval is needed.
- Tests on staged candidates: `crates/kernel/tests/kernel_admission.rs:1292`
  (kind mismatch denies, match promotes to `verified`), `:1323` (revision
  skew), `:1339` (invalidated observation), `:677` (another source),
  `:4211` (dead evidence), `:4150` (trigger class folds into the served
  class).

## Failure scenario

An observation on lineage `("repository_policy", "<digest>:AGENTS.md", 3)`
is accepted as the trigger for a decision at revision 4, or on another file's
lineage; one observation then verifies rules it never witnessed. Inverse: the
observation is on the exact lineage but the SQL misses (for example a
`None` evidence id compared against a dead evidence row) and every rule stays
`Candidate`, so the feature delivers nothing while reporting success.

## Timing windows and dependencies

- The observation must be committed in the same or an earlier commit; the
  query runs in the admitting transaction.
- The observation's own admission classes are not consulted; only its
  registry row, kind, lineage, and sensitivity are.
- A `Sensitive` observation or backing evidence makes the decision
  `Sensitive` and therefore `Hidden` on `AutoInject` even at `Verified`.

## What a test must construct

1. A decision `D` on lineage `(k, id, 2)` in the memory domain, `Normal`.
2. An observation `O` with `observation_kind = "config_present"` on `(k, id,
   2)`, `Normal`, no evidence.
3. `record_admission(ConfigObserved, subject D, (TrustedLocalCode,
   CurrentConfig), trigger_object_id = O)`: assert `effective_maturity ==
   Verified`, `visibility == Automatic`, stored `trigger_object_id == O`.
4. Negative controls, each a fresh decision: trigger absent; `O'` of kind
   `code_present`; `O''` on `(k, id, 3)`; `O'''` on `(k, other, 2)`; `O`
   retired before the admission; `O` citing evidence that was retired.
   Assert `outcome == Deny`, `effective_maturity == Candidate`,
   `trigger_object_id IS NULL`.
5. Sensitivity control: `O` at `Sensitive`; assert the decision's served
   class is `Sensitive` and `read_visible` on `AutoInject` omits it.

## Investigation log

### Q: Could a plugin-written observation satisfy the trigger predicate?

- Sources examined: `validate_trigger` SQL; `commit.rs` `InsertObservation`
  (`:870-900`) which admits the observation with `EventKind::Other` at model
  classes; `derive_classes`.
- Findings: the predicate does not read `admission_decisions` for the
  observation, so a `config_present` observation inserted through
  `kernel.commit` on the right lineage would be accepted as a trigger. The
  `ConfigObserved` event itself cannot be emitted through the route.
- Missing evidence: the N2 writer's ownership of the `repository_policy`
  lineage; whether a plugin can name that `source_kind` (today it cannot:
  `derive_classes` refuses it).
- Conclusion: partial: code read; unresolved for the N2 pass, which owns the
  lineage namespace decision.
