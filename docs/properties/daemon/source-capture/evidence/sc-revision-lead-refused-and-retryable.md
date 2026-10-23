# sc-revision-lead-refused-and-retryable

Abbreviations follow [`../catalog.md`](../catalog.md): `hs.rs`, `hs-tests`,
`sc.rs`, and `sc-tests` name the harness-source and source-capture modules
and their tests.

## Discovery trigger

Part A gap record added by the portfolio evaluation: production capture
depends on clock agreement between harness and daemon. U2 listed F25 against
this record. Re-verified at HEAD `5a842d998`.

## Evidence trail

- `crates/daemon/src/harness_sources.rs`:
  - `MAX_REVISION_LEAD_MS = 60 * 60 * 1_000`.
  - `SourcePublisher::prepare` returns `PublishError::RevisionAhead { revision,
    observed_at }` when the rule is not inherited, the class is not
    `RepositoryDocuments`, and `encoded.revision > observed_at.saturating_add(MAX_REVISION_LEAD_MS)`.
    It runs before any receipt preview or retention.
  - `PublishError::is_permanent` returns `false` for `RevisionAhead`.
  - `publish_screened` turns a `prepare` error into
    `UnitDisposition::Refused(refusal)` for that unit alone.
- `crates/daemon/src/source_capture.rs`:
  - `CaptureJob::publish_locked` passes `crate::now_ms()` as `observed_at`.
  - `classify` maps a non-permanent refusal to `UnitsRefusedRetryable` and
    `Disposition::Pending { since_ms }`.
  - `CaptureJob::publish` offers memo-pending units first on the next pass.
  - The report exposes `oldest_deferred_age_ms` from `Memo::oldest_pending_since`.
- Tests read at HEAD:
  - `crates/daemon/tests/eval_ingestion.rs`
    `generated_observations_never_lead_and_a_boundary_fixture_refuses`: a unit
    published with `observed_at = revision - MAX_REVISION_LEAD_MS` is
    accepted (it replays, so the lead alone is judged), and
    `revision + MAX_REVISION_LEAD_MS + 1` with `observed_at = revision` returns
    `RevisionAhead` with the tip unchanged.
  - `crates/daemon/tests/harness_sources.rs`
    `equal_text_stays_distinct_and_revisions_replay_or_succeed_atomically`:
    `i64::MAX` refuses with the tip and staged count unchanged.
  - `crates/daemon/src/source_capture_tests.rs` `a_revision_lead_leaves_the_unit_pending`
    (F25): a user message two hours ahead counts `units_refused_retryable == 1`,
    reports `oldest_deferred_age_ms`, then counts 2 on the next pass with
    `units_memo_skipped == 0`, and no `messages` descriptor is live.
  - `hs.rs` `pins::revision_lead_matches_the_evaluator_pin` and
    `crates/daemon/tests/eval_seam_probe.rs` pin the constant to
    `eval_core::MAX_REVISION_LEAD_MS`.

## Failure scenario

A unit far in the future is accepted and pins its lineage at a revision no
later native record can advance. Or the refusal is classified permanent and
the memo marks the unit terminal, so it is never captured after the clocks
agree. Falsifier: a live descriptor with a revision more than one hour past
its `observed_at`, or `units_refused_terminal` for a lead refusal.

## Timing windows and dependencies

The window is the clock skew: while `revision - lead > now_ms()` every pass
refuses and re-offers the unit. It publishes on the first pass after the
daemon clock passes `revision - lead`, provided the record is still in the
window the plugin sends.

## What a test must construct

The boundary at exactly the lead and one millisecond past it, and a capture
pass with a skewed native timestamp followed by a second pass. All exist. No
test advances the clock past the lead to show the pending unit then publishes.

## Investigation log

### Q: Is the boundary exercised?
- Sources examined: `eval_ingestion.rs` scenario and its `#[test]` wrapper
  `generated_observations_never_lead_and_a_boundary_fixture_refuses` (run by
  `run` through `scenarios()`).
- Findings: yes, both sides of the bound.
- Conclusion: resolved with answer.

### Q: Does the lead apply to every production publisher?
- Sources examined: `prepare`; `claim_sources.rs`.
- Findings: inherited units (canonical classes) and `repository_documents`
  are exempt; the latter exemption arrived with U9/U10.
- Conclusion: resolved with answer; reachability is the capture path.

### Q: Should a unit pending for a long time ever become terminal?
- Sources examined: `classify`, `Memo::record`, `report`.
- Findings: the unit stays pending indefinitely and is re-offered every pass;
  only its age is reported.
- Missing evidence: a product decision.
- Conclusion: needs human input.
