# sc-sensitive-denied-remote-hidden-from-auto-surfaces

## Discovery trigger

Part A of the #1066 catalog (comment 1) recorded the Kernel disclosure gates
that captured text relies on under R13. Captured message text and tool output
carry no repository provenance, so the store classifies them `Sensitive`
(`sc-unprovenanced-text-stores-sensitive-or-refuses-secrets`). This record
asks whether every policy layer then keeps that class away from the `Remote`
destination and from the automatic surfaces. The source left the
`EligibilityVerdict::ProviderSensitive` tests located but unread.

## Evidence trail

Verified at HEAD `5a842d998`.

- `crates/kernel/src/envelope.rs`, `Sensitivity::denies_destination`:
  `Secret` returns `Some(Secret)` for every destination; `Normal` and
  `(Sensitive, Local)` return `None`; `(Sensitive, Remote)` returns
  `Some(SensitiveRemote)`. The doc comment states every egress judgement
  derives its class rule from this function.
- `crates/kernel/src/cas/read.rs`, `eligibility_for`: an artifact with no
  stored class is `Allowed` locally and `Denied(UnknownSensitive)` remotely;
  otherwise it applies `denies_destination`, then `ProviderRestricted` for a
  `LocalOnly` egress class on `Remote`.
- `crates/kernel/src/admission.rs`, `surface_visibility`: `Secret` is
  `Hidden` on every surface; `Sensitive` is `Hidden` on `AutoInject` and
  `AutoSearch`; otherwise the visibility row decides.
- `crates/kernel/src/eligibility.rs`, `judge`: after the lifecycle, revision,
  and scope checks, the served sensitivity (or the registry class when no
  admission decision serves the object) is passed to `denies_destination`; a
  denial returns `ProviderSensitive`. A cited artifact whose eligibility is
  not `Allowed` also returns `ProviderSensitive`.
- `crates/daemon/src/kernel_routes/egress.rs`, `evaluate`: the artifact
  class and the owner's citation are read in one `egress_candidates`
  snapshot. An `Allowed` decision becomes `Refused(OwnerSensitive)` when the
  served owner is `Secret`, or when the destination is `Remote` and the
  served owner is not `Normal`. The route is dispatched from
  `crates/daemon/src/lib.rs` as `"kernel.egress.decide"`.

Tests read at HEAD:

- `crates/kernel/tests/kernel_cas.rs`,
  `unproven_normal_and_non_utf8_are_clamped_sensitive`: unprovenanced text
  and non-UTF-8 bytes are `Denied(SensitiveRemote)` remotely and `Allowed`
  locally.
- `crates/kernel/src/admission.rs` unit test
  `visibility_matrix_and_sensitivity_ceiling_are_total`: `Secret` is
  `Hidden` for every maturity, disposition, and surface; `Sensitive` is
  `Hidden` on `AutoInject` and `Visible` on `ExplicitSearch`.
- `crates/kernel/tests/kernel_admission.rs`,
  `dispositions_and_sensitivity_only_reduce_visibility`: `AutoSearch`
  returns no rows; `ExplicitSearch` returns the sensitive object unlabeled.
- `crates/kernel/tests/kernel_eligibility.rs`,
  `every_verdict_class_and_precedence_pair_matches_the_expected_table`: the
  independently authored `expected` table gives `secret` `ProviderSensitive`
  for both destinations and `sensitive` `ProviderSensitive` only for
  `Remote`, including the artifact-citing case.
- `crates/daemon/tests/kernel_routes.rs`,
  `egress_gate_judges_the_owner_by_its_served_class`: an owner admitted under
  a `personal` taint is refused `owner_sensitive` for `remote` with zero
  provider requests, and allowed for `local`.
- `crates/daemon/src/source_capture_tests.rs`,
  `captured_units_create_no_decision_and_stay_hidden_from_auto_inject_after_reopen`:
  live `messages` and `raw_tool_spans` descriptors from a real pass are
  absent from `visible_as_of_in_scope(Surface::AutoInject, ...)` after a
  reopen.

## Failure scenario

A captured tool error stored `Sensitive` reaches a remote model because one
layer (artifact eligibility, candidate eligibility, the egress route, or a
surface view) applies a weaker class rule than `denies_destination`, or
because the egress route reads the class and citation at different
snapshots and an interleaved ingest reclassifies the digest between them.

## Timing windows and dependencies

The egress route closes the reclassification window by reading class and
citation in one snapshot. The other layers are pure functions of stored
class. The record depends on the stored class being `Sensitive`, which
`sc-unprovenanced-text-stores-sensitive-or-refuses-secrets` owns.

## What a test must construct

A `Sensitive` object and artifact; reads at `Remote` and `Local` through
`artifact_eligibility`, `judge_eligibility`, and `kernel.egress.decide`; and
`AutoInject` and `AutoSearch` views. Fault F21 supplies the remote request.
The assertion is the verdict table above for every row, with zero provider
requests on a refusal.

## Investigation log

### Q: Do the `ProviderSensitive` eligibility tests cover `Sensitive` at `Remote`?
- Sources examined: `crates/kernel/tests/kernel_eligibility.rs` `expected`
  and its driver test; `crates/kernel/tests/eligibility_spec.rs`.
- Findings: the table maps `sensitive` to `ProviderSensitive` for `Remote`
  and `Ok` for `Local`, and `secret` to `ProviderSensitive` for both.
- Missing evidence: none for this question.
- Conclusion: resolved with answer; the source's "located, not read" note
  is closed and Confidence rises to high.

### Q: May a future read tool disclose captured tool errors to the model (OQ2)?
- Sources examined: spec.md OQ2; ticket-1090.md.
- Findings: OQ2 is an owner decision for #708 and the disclosure policy
  owner; ticket-1090 records it as open.
- Missing evidence: an owner decision.
- Conclusion: needs human input.
