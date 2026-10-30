# Attestation verifier: catalog

Method: `../METHOD.md`. The records come from the Property Catalog of the
deterministic attestation verifier specification
([#1169](https://github.com/ahrav/eidnara/issues/1169)), authored against
Eidnara `3a3b916b7`. The specification's working bundle is not durable
(specification N4), so the U1 ticket
([#1170](https://github.com/ahrav/eidnara/issues/1170)) reconstructs the 23
existing-behavior records and the 18 proposed obligations from the
specification body, applies the portfolio evaluation's accepted refinements
and four reference corrections, and re-verifies every reference against the
#1170 branch, based on `84743e366`. A reference `path:line` names the first
line of the function, constant, or test it cites, or the named line inside it.
Each later ticket (#1171 to #1174) converts the records its change makes
checkable and re-verifies every reference at its branch; #1171 re-verified
every reference into the files it changed at its branch, and #1172 did the
same at its branch.

This ticket set amends the specification's proposed obligations in three
ways. The reason set has no `range_outside_blob` and no `filter_unavailable`
(OQ6-16 and OQ6-8 as decided). A claim carries no range. The admission
oracle follows the write-set ticket
([#1172](https://github.com/ahrav/eidnara/issues/1172)): an attestation's
first admission yields `Outcome::Admit` at effective maturity `Verified`, so
the oracle is effective maturity `Verified` with disposition `Active`, never
the outcome.

## Scope

Existing surfaces, read at the branch tip:

1. S1, Kernel admission for witness events: `crates/kernel/src/admission.rs`
   (`evaluate_admission`, `record_admission`, `validate_trigger`,
   `validate_provenance`, `automatic_ceiling`, `event_effect`).
2. S2, decision lineage and dispositions: `crates/kernel/src/admission.rs`
   (`supersede_decision`, `refuse_barred_lineage`),
   `crates/kernel/src/slice/write.rs` (`validate_successor`,
   `retire_evidence`), and `crates/kernel/src/cas/deletion.rs`
   (`delete_artifact`).
3. S3, applicability payload and cheap checks:
   `crates/kernel/src/applicability/checks.rs`.
4. S4, the isolated bounded git open and the Kernel checkout open:
   `crates/daemon/src/git_sources.rs` and
   `crates/kernel/src/applicability/checkout.rs` (`open_isolated`).
5. S5, `repository_documents` publication:
   `crates/daemon/src/source_capture.rs` and
   `crates/daemon/src/project_docs.rs`.
6. S6, plugin route class derivation:
   `crates/daemon/src/kernel_routes/commit.rs` (`derive_classes`).
7. S7, claim causality: `crates/kernel/src/claim_causality.rs`.

New behavior: N1 verifiability; N2 verdict; N3 attestation; N4 witness; N5
stale path; N6 party; N7 bounded pass; N8 applicability. #1170 lands N2
in `crates/daemon/src/attestation_verifier.rs`: the `Claim`, `Observed`,
`Verdict`, and `Reason` domain, the pure `verdict` and `header_verdict`
functions, and the `verify` driver over `git_sources::open_with_slots`.
#1171 lands N1 in `crates/daemon/src/attestation_verifier/classify.rs`:
`classify`, the closed `Unverifiable` reasons, the `DocumentKind` path rule,
and the `VerifierInput` a write set consumes, over two Kernel reads, the
evidence-keyed descriptor read in `crates/kernel/src/source_descriptor.rs`
and the anchor-condition read in `crates/kernel/src/anchor_write.rs`. #1172
lands N3 to N5 and the write-set half of N6 in
`crates/daemon/src/attestation_verifier/attest.rs`: the deterministic
`Identity`, the `Match` envelope built by `WriteSet`, the `Mismatch` stale
path, and `attest`, which returns one `Outcome` per decision; the `verify`
driver's `Evaluation` gains the blob id the path resolved to, and
`crates/kernel/src/anchor_write.rs` gains the envelope's anchor-condition read
the recheck uses. The plugin-route half of N6 is the existing
`derive_classes` and `resolve_classes` guard in
`crates/daemon/src/kernel_routes/commit.rs`, covered by the route test at
`crates/daemon/tests/kernel_routes.rs:6778`.

## Status vocabulary

`Status:` uses METHOD's `active` for the 23 existing-behavior records, the
two N2 records #1170 made checkable, the two N1 records #1171 made
checkable, and the nine N3 to N6 records #1172 made checkable. The other 5
records use `Status: proposed`, the value the specification assigns to a
designed obligation whose code does not exist yet. A proposed record names
the ticket that converts it, carries `Exercised: not yet` with that ticket
named, and states the intended reachability. Every existing check named in
this part has status `unaudited` in `existing-checks.md`.

## Reachability classes

- `default-production`: reached with the shipped configuration, through plugin
  routes, memory capture, or every admission.
- `explicit-config-only`: reached only when source capture is enabled;
  `source_capture_enabled` defaults to off (`crates/daemon/src/config.rs:157`).
  The proposed verifier records carry this label as their intended class,
  since `verifier.enabled` will default to off (C9).
- `test-only`: no production caller at the branch tip. Each record states its
  own evidence: no production `CodeObserved` emitter, no production caller of
  the applicability engine, `read_selection`, `record_claim_causality`,
  `delete_artifact`, `attestation_verifier::verify`,
  `attestation_verifier::classify::classify`, or
  `attestation_verifier::attest::attest`. The verifier's write set emits
  `CodeObserved` and calls `record_claim_causality`, and has no production
  caller until #1173.

## Index

| Slug | Type | Surface | Status | Semantics | Reachability |
| --- | --- | --- | --- | --- | --- |
| `p6-degraded-witness-event-denies-without-error` | safety | S1 | active | `always` | test-only |
| `p6-witness-cannot-lift-an-inference-class-subject` | safety | S1 | active | `always` | test-only |
| `p6-trigger-must-match-subject-source-triple` | safety | S1 | active | `always` | test-only |
| `p6-trigger-sensitivity-composes-into-admission` | safety | S1 | active | `always` | test-only |
| `p6-validated-witness-under-trusted-classes-promotes` | reachability | S1 | active | `sometimes` | test-only |
| `p6-object-subject-provenance-passes-trusted-classes` | safety | S1 | active | `always` | default-production |
| `p6-policy-digest-pins-admission-policy` | safety | S1 | active | `always` | default-production |
| `p6-successor-keeps-identity-and-advances-revision` | safety | S2 | active | `always` | default-production |
| `p6-replaced-decision-is-invalidated-and-unserved` | safety | S2 | active | `always` | default-production |
| `p6-barred-lineage-refuses-supersession` | safety | S2 | active | `always` | default-production |
| `p6-mark-stale-withdraws-support-and-is-terminal-in-place` | safety | S2 | active | `always` | default-production |
| `p6-object-applicability-payload-fails-closed` | safety | S3 | active | `always` | test-only |
| `p6-failed-cheap-check-is-stale-and-handed-to-repair` | safety | S3 | active | `always` | test-only |
| `p6-blob-read-is-header-gated-before-decode` | safety | S4 | active | `always` | test-only |
| `p6-document-descriptor-identity-digest-and-revision` | safety | S5 | active | `always` | explicit-config-only |
| `p6-document-republishes-only-on-digest-change-and-retires-on-missing` | safety | S5 | active | `always` | explicit-config-only |
| `p6-plugin-source-kind-table-is-closed-and-classes-never-raised` | safety | S6 | active | `always` | default-production |
| `p6-plugin-cannot-replace-a-foreign-source-decision` | safety | S2, S6 | active | `always` | default-production |
| `p6-derived-reinjection-parents-live-at-write-and-existed-at-read` | safety | S7 | active | `always` | test-only |
| `p6-direct-observation-requires-exact-evidence-digest` | safety | S7 | active | `always` | test-only |
| `p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search` | safety | S5, S1 | active | `always` | explicit-config-only |
| `p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard` | safety | S2 | active | `always` | test-only |
| `p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements` | safety | S4 | active | `always` | test-only |
| `p6-new-verifiability-pure-function-of-snapshot` | safety | N1 | active | `always` | test-only |
| `p6-new-verifier-input-anchor-and-document-citation` | safety | N1 | active | `always` | test-only |
| `p6-new-attestation-identity-unique-per-decision-oid` | safety | N3 | active | `always` | test-only |
| `p6-new-attestation-classes-and-cited-fields` | safety | N3 | active | `always` | test-only |
| `p6-new-attestation-sibling-model-stays-candidate` | safety | N3 | active | `always` | test-only |
| `p6-new-attestation-verified-only-via-witness` | safety | N4 | active | `always` | test-only |
| `p6-new-witness-evidence-id-equals-attestation` | safety | N4 | active | `always` | test-only |
| `p6-new-witness-degradation-never-silent` | safety | N4 | active | `always` | test-only |
| `p6-new-attestation-write-set-one-outcome` | safety | N3 | active | `always` | test-only |
| `p6-new-mismatch-stale-unavailable-silent` | safety | N5 | active | `always` | test-only |
| `p6-new-party-distinct-and-self-refusal` | safety | N6 | active | `always` | default-production |
| `p6-new-flag-off-nothing-runs` | safety | N7 | proposed | `unreachable` | default-production |
| `p6-new-pass-bounds-and-accounting` | safety | N7 | proposed | `always` | explicit-config-only |
| `p6-new-cursor-never-skips-unresolved` | safety | N7 | proposed | `always` | explicit-config-only |
| `p6-new-pass-progress-under-transient-unavailable` | liveness | N7 | proposed | `always` | explicit-config-only |
| `p6-new-attestation-stale-on-byte-change` | safety | N8 | proposed | `always-or-unreached` | explicit-config-only |
| `p6-new-verdict-deterministic` | safety | N2 | active | `always` | test-only |
| `p6-new-blob-read-bounded-at-oid` | safety | N2 | active | `always` | test-only |

Counts: 41 records; 23 existing-behavior records with `unaudited` existing
checks, 13 records #1170 to #1172 convert to active, and 5 proposed
obligations (4 for #1173, 1 for #1174). Semantics distribution: `always` 38, `sometimes` 1,
`always-or-unreached` 1, `unreachable` 1, `reachable` 0. Types: 39 safety, 1
reachability, 1 liveness. Reachability among the 36 active records: 24
test-only, 9 default-production, 3 explicit-config-only. The specification
labelled
`p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard`
`default-production`; `delete_artifact` has no production caller at the
branch tip, so this part labels it `test-only`.

## Records

### p6-degraded-witness-event-denies-without-error

Type: safety
Reachability: test-only - the daemon emitters of `CodeObserved` are `observe_code` (`crates/daemon/src/lib.rs:14394`), inside the module the `test-support` or `direct-host-fixture` feature gates at `crates/daemon/src/lib.rs:14041`, and the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:376`), which has no production caller at this branch
Status: active
Exercised: yes - Kernel admission tests submit witness events with an unvalidated, foreign, other-revision, and invalidated trigger; the verifier write-set test rewrites its witness's kind at insert and the admission returns below `Verified` with no error
Guarantee: A `CodeObserved` or `ConfigObserved` event whose trigger id is well-formed but fails validation is evaluated as `Other` and admitted with outcome `Deny` and no error; a malformed trigger id is refused by `identity` before validation.
Check: `always` - for every witness event whose well-formed trigger fails `validate_trigger`, the recorded admission has outcome `deny`, the subject's effective maturity is unchanged, and the commit returns `Ok`; scoped to well-formed ids by refinement R3
Fault/timing angle: none
Required faults and enabling state: F1: a trigger absent, from another source, of the other kind, invalidated, at another revision, or with retired backing evidence
Confidence: high - [evidence](evidence/p6-degraded-witness-event-denies-without-error.md). Read `validate_trigger` (`crates/kernel/src/admission.rs:2807`), which returns `Ok(None)` for every failed lookup, and the call at `:1662` whose `None` evaluates `EventKind::Other` at `:1699`; `event_effect` (`:4245`) maps `Other` to `Deny`
Existing check: `crates/kernel/tests/kernel_admission.rs:648`, `:677`, `:1323`, `:1339`; `crates/daemon/tests/attestation_verifier_attest.rs:854` (unaudited)
Impact: a writer that reads success as promotion would count a `Candidate` row as verified; #1172 asserts maturity itself (`p6-new-witness-degradation-never-silent`)
Open questions: None.

### p6-witness-cannot-lift-an-inference-class-subject

Type: safety
Reachability: test-only - the daemon emitters of `CodeObserved` are `observe_code` (`crates/daemon/src/lib.rs:14394`), inside the module the `test-support` or `direct-host-fixture` feature gates at `crates/daemon/src/lib.rs:14041`, and the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:376`), which has no production caller at this branch
Status: active
Exercised: yes - admission unit tests evaluate every legal source and taint pair against its ceiling
Guarantee: A subject admitted under `(ModelInference, AssistantInference)` has an automatic ceiling of `Candidate`: a witness event that raises support above that ceiling requires valid approval, and an admission whose classes differ from the subject's prior admission is refused.
Check: `always` - for every admission of a subject with a prior admission the classes equal the prior's, and for every subject under an inference taint a witness event that raises effective maturity above `Candidate` carries valid approval
Fault/timing angle: none
Required faults and enabling state: a model-class decision followed by a `CodeObserved` event with a valid trigger; a second admission of that subject under trusted classes
Confidence: high - [evidence](evidence/p6-witness-cannot-lift-an-inference-class-subject.md). Read `automatic_ceiling` (`crates/kernel/src/admission.rs:4302`), which maps every inference taint to `Candidate`, and the prior-class refusal at `:284` in `evaluate_admission` (`:280`)
Existing check: `crates/kernel/src/admission.rs:4942`, `:4720` (unaudited)
Impact: a model-proposed decision promoted in place would reach auto-inject without a distinct party
Open questions: None.

### p6-trigger-must-match-subject-source-triple

Type: safety
Reachability: test-only - the daemon emitters of `CodeObserved` are `observe_code` (`crates/daemon/src/lib.rs:14394`), inside the module the `test-support` or `direct-host-fixture` feature gates at `crates/daemon/src/lib.rs:14041`, and the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:376`), which has no production caller at this branch
Status: active
Exercised: yes - Kernel admission tests submit triggers from another source, at another revision, of the other kind, invalidated, and with retired evidence; a verifier fixture admits one attestation identity's subject through another identity's witness and it stays at `Candidate`
Guarantee: Under trusted classes whose ceiling admits `Verified`, a witness event promotes only when its trigger observation is live, of the event's kind, carries the subject's exact `(source_kind, source_id, source_revision)`, and has live backing evidence when it cites any.
Check: `always` - every admission row with a witness kind and outcome `admit` or `promote` names a trigger observation that, at the admission's commit, was live, of kind `code_present` or `config_present` matching the event, had the subject's source triple, and had live or absent backing evidence; the trusted-class precondition is refinement R4
Fault/timing angle: the trigger or its evidence invalidated in an earlier commit or earlier in the same envelope
Required faults and enabling state: F1
Confidence: high - [evidence](evidence/p6-trigger-must-match-subject-source-triple.md). Read the lookup predicate in `validate_trigger` (`crates/kernel/src/admission.rs:2807`); it compares the source triple and liveness and never compares evidence ids
Existing check: `crates/kernel/tests/kernel_admission.rs:677`, `:1292`, `:1323`, `:1339`, `:4211`; `crates/daemon/tests/attestation_verifier_attest.rs:1132` (unaudited)
Impact: one observation could validate every attestation's trigger if the triple were shared (D1)
Open questions:
- Does `validate_trigger` compare the observation's evidence with anything on the subject? - resolved with answer: evidence equality is the verifier's own check (`p6-new-witness-evidence-id-equals-attestation`).

### p6-trigger-sensitivity-composes-into-admission

Type: safety
Reachability: test-only - the daemon emitters of `CodeObserved` are `observe_code` (`crates/daemon/src/lib.rs:14394`), inside the module the `test-support` or `direct-host-fixture` feature gates at `crates/daemon/src/lib.rs:14041`, and the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:376`), which has no production caller at this branch
Status: active
Exercised: yes - Kernel admission tests admit objects through triggers of a stricter class and through triggers backed by stricter evidence
Guarantee: An object admitted through a validated trigger serves at least as restrictively as the trigger observation and the trigger's backing evidence.
Check: `always` - for every admission with a validated trigger, the served sensitivity of the subject is at least the restrictive fold of the observation's class and its backing evidence's class
Fault/timing angle: none
Required faults and enabling state: F14: a `Sensitive` evidence row behind the trigger observation
Confidence: high - [evidence](evidence/p6-trigger-sensitivity-composes-into-admission.md). Read the class fold in `validate_trigger` (`crates/kernel/src/admission.rs:2807`) and its use as `candidate_sensitivity` in `record_admission` (`:998`) after the call at `:1662`
Existing check: `crates/kernel/tests/kernel_admission.rs:4150`, `:6378` (unaudited)
Impact: a witness citing `Sensitive` document evidence hides the attestation on `AutoInject` (K7), which is why D5 keeps the witness's evidence in its payload
Open questions: None.

### p6-validated-witness-under-trusted-classes-promotes

Type: reachability
Reachability: test-only - the daemon emitters of `CodeObserved` are `observe_code` (`crates/daemon/src/lib.rs:14394`), inside the module the `test-support` or `direct-host-fixture` feature gates at `crates/daemon/src/lib.rs:14041`, and the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:376`), which has no production caller at this branch
Status: active
Exercised: yes - the `observe_code` fixture and the route tests that build a verified decision through the sibling `code_observation` fixture; the verifier write set admits an attestation at `Verified` through its `code_present` witness
Guarantee: A fixture campaign constructs at least one subject under `(TrustedLocalCode, CurrentCode)` that a validated `CodeObserved` trigger promotes to `Verified`, the positive witness the S1 `always` records depend on.
Check: `sometimes` - a coverage floor for fixtures, reworded by refinement R7: at least one committed admission with event `code_observed`, a validated non-null trigger, outcome `admit` or `promote`, effective maturity `verified`, and disposition `active`; a subject's first admission records `admit` (`crates/kernel/src/admission.rs:381`)
Fault/timing angle: none
Required faults and enabling state: a trusted-class subject and a matching `code_present` observation in one envelope
Confidence: high - [evidence](evidence/p6-validated-witness-under-trusted-classes-promotes.md). Read `observe_code` (`crates/daemon/src/lib.rs:14394`) and the route test at `crates/daemon/tests/kernel_routes.rs:1875`, whose observation comes from `code_observation` (`:887`), a sibling fixture in the test file (reference correction 3)
Existing check: `crates/daemon/src/lib.rs:14394` fixture; `crates/daemon/tests/kernel_routes.rs:1875`, `:5120` through `code_observation` at `:887`; `crates/daemon/tests/attestation_verifier_attest.rs:431` (unaudited)
Impact: without the positive witness the S1 `always` records pass vacuously
Open questions: None.

### p6-object-subject-provenance-passes-trusted-classes

Type: safety
Reachability: default-production - `record_admission` runs `validate_provenance` on every admission
Status: active
Exercised: partial - object-subject admissions under `(TrustedLocalCode, CurrentCode)` in `crates/daemon/tests/kernel_routes.rs:1875`, `:5120`, the `observe_code` fixture (`crates/daemon/src/lib.rs:14394`), and the verifier write set (`crates/daemon/tests/attestation_verifier_attest.rs:431`) pass `validate_provenance` with `None` provenance, and the verifier's configuration test (`crates/daemon/tests/attestation_verifier_attest.rs:1092`) passes it under `(TrustedLocalCode, CurrentConfig)`; no test covers the other legal class pairs
Guarantee: `validate_provenance` refuses `TrustedLocalCode` only for `unclassified` provenance, which only staging candidates carry, so an object subject with no provenance passes under trusted classes (N2, OQ6-2).
Check: `always` - for every admission of a subject whose provenance kind is `None`, `validate_provenance` returns `Ok` for every legal class pair
Fault/timing angle: none
Required faults and enabling state: an object subject with no provenance under `(TrustedLocalCode, CurrentCode)`
Confidence: high - [evidence](evidence/p6-object-subject-provenance-passes-trusted-classes.md). Read `validate_provenance` (`crates/kernel/src/admission.rs:2650`) and its call at `:1622`
Existing check: `crates/daemon/tests/kernel_routes.rs:1875`, `:5120`; `crates/daemon/tests/attestation_verifier_attest.rs:431` (unaudited)
Impact: a refusal here would make every attestation admission fail with `AdmissionPolicy`
Open questions: None.

### p6-policy-digest-pins-admission-policy

Type: safety
Reachability: default-production - every admission and every served read evaluate under `POLICY_REVISION`
Status: active
Exercised: yes - the digest tests hash the evaluator and serving source regions
Guarantee: The admission evaluator and the serving fold are pinned by a content digest under `POLICY_REVISION`, so a change to either fails the digest test unless the revision moves.
Check: `always` - `policy_revision_matches_policy_content` and `the_served_fold_is_under_the_serving_digest` pass at every commit, and a newer stored revision fails closed
Fault/timing angle: none
Required faults and enabling state: an edit inside a `policy-digest` region
Confidence: high - [evidence](evidence/p6-policy-digest-pins-admission-policy.md). Read the digest tests at `crates/kernel/src/admission.rs:5931` and `:5900` and `POLICY_REVISION` at `:20`
Existing check: `crates/kernel/src/admission.rs:5931`, `:5900`; `crates/kernel/tests/kernel_admission.rs:3371` (unaudited)
Impact: an unpinned evaluator change could grant a class upgrade, the plan's first stop condition
Open questions: None.

### p6-successor-keeps-identity-and-advances-revision

Type: safety
Reachability: default-production - plugin `kernel.commit` replacements and memory capture call `supersede_decision`
Status: active
Exercised: yes - Kernel slice tests correct and replace across domains and without a revision bump
Guarantee: A successor keeps its predecessor's `domain_id`, `source_kind`, and `source_id` and has a larger `source_revision`; a mismatch is `InvalidInput` and an unbumped revision is `Conflict`.
Check: `always` - for every successor row, its domain and source pair equal the predecessor's and its revision is greater
Fault/timing angle: none
Required faults and enabling state: a replacement under another domain or source; a replacement at the same revision
Confidence: high - [evidence](evidence/p6-successor-keeps-identity-and-advances-revision.md). Read `validate_successor` (`crates/kernel/src/slice/write.rs:1271`)
Existing check: `crates/kernel/tests/kernel_slice.rs:1003`; the unbumped case inside `crates/kernel/tests/kernel_slice.rs:607` at `:704` to `:712` (unaudited)
Impact: succession is unavailable across parties, so the verifier writes a sibling (K8)
Open questions: None.

### p6-replaced-decision-is-invalidated-and-unserved

Type: safety
Reachability: default-production - plugin `kernel.commit` replacements and memory capture call `supersede_decision`
Status: active
Exercised: partial - `crates/kernel/tests/kernel_slice.rs:1096` exercises decision replacement, folding, and registry liveness; `crates/kernel/tests/kernel_admission.rs:393` exercises admission-history retention for a replaced domain; no cited test reads a replaced decision's admission history
Guarantee: A replaced decision is invalidated at the replacement's commit and no longer served, while its admission history stays readable.
Check: `always` - after a committed replacement the predecessor's registry row is invalidated and no serving read returns it
Fault/timing angle: none
Required faults and enabling state: a live predecessor replaced, and a merge folded into a survivor
Confidence: high - [evidence](evidence/p6-replaced-decision-is-invalidated-and-unserved.md). Read `supersede_decision` (`crates/kernel/src/admission.rs:1368`)
Existing check: `crates/kernel/tests/kernel_slice.rs:1096`; `crates/kernel/tests/kernel_admission.rs:393` (unaudited)
Impact: a served predecessor would show both the model decision and its replacement on automatic surfaces
Open questions: None.

### p6-barred-lineage-refuses-supersession

Type: safety
Reachability: default-production - plugin `kernel.commit` replacements and memory capture call `supersede_decision`
Status: active
Exercised: yes - a slice test supersedes a quarantined predecessor
Guarantee: A lineage whose latest admission disposition is `Quarantined`, `Rejected`, or `Contradicted` cannot be corrected, replaced, or used as a fold survivor.
Check: `always` - every committed supersession names a predecessor and survivor whose latest disposition is outside the barred set
Fault/timing angle: none
Required faults and enabling state: a quarantined predecessor; a barred survivor
Confidence: high - [evidence](evidence/p6-barred-lineage-refuses-supersession.md). Read `refuse_barred_lineage` (`crates/kernel/src/admission.rs:1395`)
Existing check: `crates/kernel/tests/kernel_slice.rs:1187` (unaudited)
Impact: it bounds which model decisions a later supersession can touch
Open questions: None.

### p6-mark-stale-withdraws-support-and-is-terminal-in-place

Type: safety
Reachability: default-production - plugin routes submit `MarkStale` with the shipped configuration
Status: active
Exercised: yes - admission unit and route tests apply `MarkStale` and read the disposition; the verifier's stale path applies it under the model decision's stored classes
Guarantee: `MarkStale` sets disposition `Stale` in place with outcome `Deny`; a later event that names no disposition keeps `Stale`. After `MarkStale`, a validated `CodeObserved` returns `Outcome::Promote` when the subject's automatic ceiling admits `Verified` or approval is valid, and `Outcome::Deny` otherwise, while the disposition stays `Stale`; `Outcome` alone is never the oracle.
Check: `always` - after a committed `MarkStale`, every later admission of the subject that names no disposition leaves the disposition `stale`; clause added by refinement R2
Fault/timing angle: a witness event after `MarkStale` in the same or a later envelope
Required faults and enabling state: `MarkStale` followed by a validated `CodeObserved` on the same subject
Confidence: medium - [evidence](evidence/p6-mark-stale-withdraws-support-and-is-terminal-in-place.md). Read `event_effect` (`crates/kernel/src/admission.rs:4245`): `MarkStale` maps to `(current, Some(Stale), Deny)` and `CodeObserved` to `(Verified, None, Promote)`, and a `None` disposition keeps the prior one. The effective-maturity and visibility halves of R2 are not traced here
Existing check: `crates/kernel/src/admission.rs:5437`; `crates/kernel/tests/kernel_admission.rs:1047`, `:2735`; `crates/daemon/tests/kernel_routes.rs:4398`; `crates/daemon/tests/attestation_verifier_attest.rs:597` (unaudited)
Impact: a writer that reads `Promote` as success after `MarkStale` would count a stale row as verified
Open questions:
- After `MarkStale`, does a validated `CodeObserved` leave effective maturity and visibility unchanged? - unresolved, needs a Kernel test that applies `MarkStale` then a validated `CodeObserved` and reads effective maturity and visibility.

### p6-object-applicability-payload-fails-closed

Type: safety
Reachability: test-only - the applicability engine has no production caller; no production code builds an `ApplicabilityCandidate`
Status: active
Exercised: yes - engine tests construct unreadable, oversized, unknown-kind, and unknown-field payloads
Guarantee: An applicability payload that cannot be read, exceeds its bound, or carries an unknown field is `Uncertain`, never `Current`; an unknown check kind degrades that check only.
Check: `always` - every candidate whose payload is unreadable, oversized, or carries an unknown field reports `Uncertain`
Fault/timing angle: none
Required faults and enabling state: corrupt, oversized, and unknown-field payloads; an unknown check kind
Confidence: high - [evidence](evidence/p6-object-applicability-payload-fails-closed.md). Read the cited engine tests at the branch tip
Existing check: `crates/kernel/tests/kernel_applicability_engine.rs:626`, `:703`, `:1832`, `:2703` (unaudited)
Impact: a payload-schema change for OQ6-4 alternative 1 must keep this property
Open questions: None.

### p6-failed-cheap-check-is-stale-and-handed-to-repair

Type: safety
Reachability: test-only - the applicability engine has no production caller; no production code builds an `ApplicabilityCandidate`
Status: active
Exercised: yes - engine and read-repair tests fail a cheap check and read the repair commit
Guarantee: A failed `FileExists` or `ConfigKey` check reports `Stale`, and read repair appends the observation, the event, and the job in one commit.
Check: `always` - every failed cheap check yields `Stale` and a repair commit holding all three rows
Fault/timing angle: none
Required faults and enabling state: a missing file; a missing config key
Confidence: high - [evidence](evidence/p6-failed-cheap-check-is-stale-and-handed-to-repair.md). Read the `FileExists` arm at `crates/kernel/src/applicability/checks.rs:285` and the `ConfigKey` arm at `:296` (reference correction 2: the specification's arm ranges were two lines off)
Existing check: `crates/kernel/tests/kernel_applicability_engine.rs:283`; `crates/kernel/tests/kernel_read_repair.rs:141` (unaudited)
Impact: no digest check kind exists (K3), so attestation staleness rests on OQ6-4
Open questions: None.

### p6-blob-read-is-header-gated-before-decode

Type: safety
Reachability: test-only - `read_selection` has no production caller; `GitReconciler` is constructed only by tests
Status: active
Exercised: partial - the git-source tests refuse oversized headers and an oversized delta base for commits; no test observes that no object was decoded before the header pass ended
Guarantee: `read_selection` judges every selected object's header against the per-object and total bounds before decoding any object, charges the decoded bytes again, and refuses bytes that do not hash to the id.
Check: `always` - for every selection the header pass refused, no `find_object` call ran; for every accepted object, its decoded bytes hash to its id
Fault/timing angle: none
Required faults and enabling state: F4: a header over the bound; a header under the bound with an over-budget delta base; bytes that do not hash to the id
Confidence: high - [evidence](evidence/p6-blob-read-is-header-gated-before-decode.md). Read the header loop at `crates/daemon/src/git_sources.rs:603` and the decode loop at `:617` in `read_selection` (`:551`)
Existing check: none direct; `p2-git-open-is-isolated-and-allocation-bounded` covers the open, and `crates/daemon/tests/git_sources.rs:549`, `:827` cover the refusals (unaudited)
Impact: a decode before the gate would let one object allocate past the bound
Open questions:
- Does the U1 verifier follow the same order? - resolved with answer: covered for the verifier by `p6-new-blob-read-bounded-at-oid`.

### p6-document-descriptor-identity-digest-and-revision

Type: safety
Reachability: explicit-config-only - source capture publishes documents only when `source_capture_enabled` is on, and it defaults to off (`crates/daemon/src/config.rs:157`)
Status: active
Exercised: yes - source-capture and descriptor tests publish, edit, and re-read documents
Guarantee: A `repository_documents` descriptor is identified by `(project_id, repository_id, relative_path)`, carries the sha256 of the document's bytes as `artifact_digest`, and takes the lineage head plus one as its revision; source capture reads at most 256 KiB and publishes at most 64 KiB of text.
Check: `always` - every live document descriptor's digest equals the sha256 of its retained bytes, its revision exceeds every earlier revision of its lineage, and its text is at most `MAX_SOURCE_UNIT_BYTES`; both caps stated by refinement R1
Fault/timing angle: none
Required faults and enabling state: a document edited between passes; a document between 64 KiB and 256 KiB
Confidence: high - [evidence](evidence/p6-document-descriptor-identity-digest-and-revision.md). Read the document step in `crates/daemon/src/source_capture.rs` (unit cap at `:1297`, digest comparison at `:1325`), `MAX_PROJECT_DOC_BYTES` (`crates/daemon/src/project_docs.rs:23`), and `MAX_SOURCE_UNIT_BYTES` (`crates/daemon/src/harness_sources.rs:642`, equal to `MAX_CAPTURE_SOURCE_BYTES` at `crates/memory-store/src/memory_capture.rs:19`)
Existing check: `crates/daemon/src/project_docs.rs:446`; `crates/kernel/tests/kernel_source_descriptors.rs:1180`, `:2007`, and `:3252` for the evidence-keyed read, which applies the page read's row checks to the descriptor it returns; `crates/daemon/src/source_capture_tests.rs:1992` (unaudited)
Impact: the verifier's claim is this descriptor's path, digest, and byte length (D2); a wrong digest would make every verdict meaningless
Open questions: None.

### p6-document-republishes-only-on-digest-change-and-retires-on-missing

Type: safety
Reachability: explicit-config-only - source capture publishes documents only when `source_capture_enabled` is on, and it defaults to off (`crates/daemon/src/config.rs:157`)
Status: active
Exercised: yes - source-capture tests edit, remove, rename, and fail to read documents
Guarantee: Source capture republishes a document only when its digest changes, retires the capture's own lineage when the file is missing, retires stale lineages of a path first, and retires nothing when a read fails.
Check: `always` - a publication either has no live capture-owned predecessor or changes that predecessor's digest; retirement of the current repository lineage follows a read that found the file missing, retirement of another repository identity's lineage follows a read that settles the path, and an unreadable read retires nothing
Fault/timing angle: a document edited or removed between passes
Required faults and enabling state: an unchanged document; a removed document; a renamed document; an unreadable document
Confidence: high - [evidence](evidence/p6-document-republishes-only-on-digest-change-and-retires-on-missing.md). Read the cited tests at the branch tip
Existing check: `crates/daemon/src/source_capture_tests.rs:1992`, `:2172`, `:2237`, `:2350`, `:3009` (unaudited)
Impact: a retired descriptor makes a decision unverifiable (`descriptor_retired`)
Open questions: None.

### p6-plugin-source-kind-table-is-closed-and-classes-never-raised

Type: safety
Reachability: default-production - every plugin `kernel.commit` derives its classes here
Status: active
Exercised: yes - route tests declare classes above the derived ones, including `trusted_local_code`, `current_code`, and `current_config` under `assistant`, and submit a decision, an observation, and a `mark_stale` under `source_kind = "verifier"`
Guarantee: A plugin route derives `(source_class, taint_class)` from a closed `source_kind` table (`assistant`, `model`, `memory_classifier`, `user`), refuses every other kind, and lowers but never raises the derived classes.
Check: `always` - `derive_classes(k)` is `None` for every `k` outside the table, including `verifier`, and every committed plugin row's classes are at or below the derived pair
Fault/timing angle: none
Required faults and enabling state: a plugin write naming `verifier` or asserting `trusted_local_code`
Confidence: high - [evidence](evidence/p6-plugin-source-kind-table-is-closed-and-classes-never-raised.md). Read `derive_classes` (`crates/daemon/src/kernel_routes/commit.rs:370`)
Existing check: `crates/daemon/src/kernel_routes/commit.rs:1249`; `crates/daemon/tests/kernel_routes.rs:1145`, `:6778` (unaudited)
Impact: a plugin able to write `verifier` rows could certify model output (C6)
Open questions: None.

### p6-plugin-cannot-replace-a-foreign-source-decision

Type: safety
Reachability: default-production - plugin replacements go through `supersede_decision`
Status: active
Exercised: not yet - no test submits a plugin replacement of a decision whose `source_kind` differs from the route's
Guarantee: A plugin replacement carries the route's `source_kind`, so replacing a decision of another source kind fails `validate_successor` with `InvalidInput`, and a direct `Replace` or `Correct` admission event is refused.
Check: `always` - every committed plugin replacement names a predecessor with the route's `source_kind` and `source_id`
Fault/timing angle: none
Required faults and enabling state: a plugin replacement of a `verifier` decision
Confidence: high - [evidence](evidence/p6-plugin-cannot-replace-a-foreign-source-decision.md). Read the plugin replacement at `crates/daemon/src/kernel_routes/commit.rs:1002`, `validate_successor` (`crates/kernel/src/slice/write.rs:1271`), and `refuse_succession_events` (`crates/kernel/src/admission.rs:2640`)
Existing check: none
Impact: an attestation replaced by a plugin would change party; retirement and `MarkStale` remain open to plugins (OQ6-15, gap G4)
Open questions: None.

### p6-derived-reinjection-parents-live-at-write-and-existed-at-read

Type: safety
Reachability: test-only - `record_claim_causality` has no production caller; its one daemon caller, the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:395`), has no production caller at this branch
Status: active
Exercised: yes - claim-causality tests record derived reinjections over live and mismatched parents, and retire a parent after the write to show the derivation survives (`crates/kernel/tests/kernel_claim_causality.rs:315`); the verifier write set records one naming the model decision at its revision
Guarantee: A `DerivedReinjection` record is written only when every named parent is live at the named revision. A later read requires each parent row to have existed at the requested snapshot with the named revision, so a later retirement or supersession keeps `DerivedReinjection`; a missing historical parent row or a revision mismatch yields `Unknown(ParentUnavailable)` (`parents_existed`, `crates/kernel/src/claim_causality.rs:719`).
Check: `always` - every committed derived-reinjection record names parents live at its commit, and its detail, operation, and parent list agree with the registry
Fault/timing angle: the parent superseded between selection and commit
Required faults and enabling state: F2: a parent retired or superseded before the write
Confidence: high - [evidence](evidence/p6-derived-reinjection-parents-live-at-write-and-existed-at-read.md). Read `record_claim_causality` (`crates/kernel/src/claim_causality.rs:218`) and `MAX_DERIVATION_PARENTS` (`:38`; reference correction 1: the specification cited `:36`)
Existing check: `crates/kernel/tests/kernel_claim_causality.rs:229`, `:710`; `crates/daemon/tests/attestation_verifier_attest.rs:431` (unaudited)
Impact: the attestation's link to the model decision (D4) depends on it
Open questions: None.

### p6-direct-observation-requires-exact-evidence-digest

Type: safety
Reachability: test-only - `record_claim_causality` has no production caller; its one daemon caller, the verifier write set (`crates/daemon/src/attestation_verifier/attest.rs:395`), has no production caller at this branch
Status: active
Exercised: yes - claim-causality tests record direct observations over exact, mismatched, and forged evidence
Guarantee: A `DirectObservation` record requires live, exactly retained evidence whose digest matches the recorded one; copied strings and forged records grant nothing.
Check: `always` - every committed direct-observation record names live evidence with the recorded digest
Fault/timing angle: none
Required faults and enabling state: F3: evidence invalidated; a digest that differs
Confidence: high - [evidence](evidence/p6-direct-observation-requires-exact-evidence-digest.md). Read the cited tests at the branch tip
Existing check: `crates/kernel/tests/kernel_claim_causality.rs:42`, `:364` (unaudited)
Impact: explains why D4 chooses `DerivedReinjection` (OQ6-12)
Open questions: None.

### p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search

Type: safety
Reachability: explicit-config-only - document evidence exists only when source capture is on, and it defaults to off (`crates/daemon/src/config.rs:157`)
Status: active
Exercised: partial - the row-level fold is exercised by slice tests, the publisher's class by a source-capture test, and a model decision citing document evidence by its row serves `Sensitive` in a verifier test; no test promotes through a trigger citing document evidence
Guarantee: Every `repository_documents` evidence row is stored at least `Sensitive`, and a decision or observation citing it by foreign key, or an admission whose validated trigger cites it, serves at least `Sensitive`, hidden on `AutoInject` and `AutoSearch`.
Check: `always` - every row citing document evidence through `evidence_id`, and every subject promoted through a trigger citing it, has `AutoInject` and `AutoSearch` visibility `Hidden`
Fault/timing angle: none
Required faults and enabling state: F14: document evidence cited through the row FK, the observation FK, or the admission event
Confidence: high - [evidence](evidence/p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search.md). Read the publisher's class at `crates/daemon/src/source_capture.rs:922`, the trigger fold in `validate_trigger` (`crates/kernel/src/admission.rs:2807`), and `surface_visibility` (`:4221`)
Existing check: `crates/daemon/src/source_capture_tests.rs:1992`; `crates/kernel/tests/kernel_slice.rs:1454`, `:1527`; `crates/daemon/tests/attestation_verifier_attest.rs:572`; none for the trigger fold (unaudited)
Impact: an attestation citing the evidence by FK would be `Verified` and never auto-injected (K7); D5 places the citation on a decision event
Open questions: None.

### p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard

Type: safety
Reachability: test-only - `delete_artifact` has no production caller in the daemon; its callers are tests and the eval-runner example (`crates/daemon/examples/eval_runner/fault.rs:817`), which corrects the specification's `default-production` label
Status: active
Exercised: partial - `crates/kernel/tests/kernel_slice.rs:2248` deletes cited artifact evidence and asserts the evidence is invalidated while its citing decision stays live; no test follows the deletion with a witness admission
Guarantee: `retire_evidence` refuses a cited evidence row with `Conflict`, while `delete_artifact` invalidates every live evidence row of a digest with no citation predicate, so a trigger backed by that evidence then fails validation.
Check: `always` - after `delete_artifact` on a digest a live decision cites, the evidence row is invalidated while the citing decision stays live; recorded as present behavior
Fault/timing angle: a deletion between the verifier's read and its commit
Required faults and enabling state: F3: `delete_artifact` on a cited digest
Confidence: high - [evidence](evidence/p6-artifact-deletion-invalidates-cited-evidence-without-the-citation-guard.md). Read the citation check in `retire_evidence` (`crates/kernel/src/slice/write.rs:585`, `Conflict` at `:607`) and the `UPDATE evidence_meta` in `delete_artifact` (`crates/kernel/src/cas/deletion.rs:239`, update at `:389`)
Existing check: `crates/kernel/tests/kernel_slice.rs:2248` (unaudited)
Impact: the pin on cited evidence holds for the expiry sweep only; an attestation's witness can lose its backing evidence (gap G2)
Open questions:
- Should `delete_artifact` honor the citation guard? (needs human input)

### p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements

Type: safety
Reachability: test-only - this label covers the Kernel opener and the verifier clauses: the applicability engine has no production caller, no production code builds an `ApplicabilityCandidate`, and `read_selection` and `attestation_verifier::verify` have no production caller. The shared daemon opener is default-production through `memory_capture::head::observe_head` (`crates/daemon/src/memory_capture/head.rs:32`), called after native capture output parses (`crates/daemon/src/memory_capture/native.rs:614`), as `p2-git-open-is-isolated-and-allocation-bounded` records
Status: active
Exercised: partial - the daemon opener's allocation limit is exercised by delta-base tests; no test reads a delta base or a replacement ref through the Kernel opener
Guarantee: The Kernel checkout open is `Options::isolated()` with command configuration stripped, no `gitoxide.objects.allocLimit`, and replacement refs left to the repository's configuration; under gix 0.87.1 `refs/replace` loads only when the repository sets `core.useReplaceRefs = false`, so an entry alone substitutes nothing. `git_sources::open` and `git_sources::open_with_slots` install the limit after the repository's configuration and ignore replacements.
Check: `always` - the verifier opens repositories only through `git_sources::open_with_slots` and, after a store lookup exhausts the index slots, `git_sources::open`; it reads its own packs and loose objects under the same limit; and a delta-packed blob whose base exceeds the limit is refused before allocation
Fault/timing angle: none
Required faults and enabling state: F4: an over-budget delta base; a `refs/replace` entry with `core.useReplaceRefs = false`
Confidence: high - [evidence](evidence/p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements.md). Read `open_isolated` (`crates/kernel/src/applicability/checkout.rs:639`) and `isolated_options` and `open_with_slots` (`crates/daemon/src/git_sources.rs:131`, `:148`); no `allocLimit` or `ignore_replacements` appears under `crates/kernel/src`
Existing check: `crates/daemon/tests/git_sources.rs:549`, `:827`, and `crates/daemon/tests/attestation_verifier.rs:594`, `:505`, `:1146`, `:1240` for the daemon opener; none for the Kernel opener (unaudited)
Impact: a verifier on the Kernel opener would inflate a delta chain without a bound and could read a replaced object (C4)
Open questions:
- Which opener does the U1 driver use? - resolved with answer; this resolves the open question the portfolio evaluation tied to G3.

### p6-new-verifiability-pure-function-of-snapshot

Type: safety
Reachability: test-only - `attestation_verifier::classify::classify` has no production caller at this branch; #1173 wires it behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1171](https://github.com/ahrav/eidnara/issues/1171)
Exercised: partial - a fixture store classifies four decisions twice at one commit sequence around a worktree edit, a HEAD move to an orphan commit from which the anchor oid is unreachable, and later Kernel changes that retire a cited descriptor, invalidate an anchor, and delete cited evidence cited at event and at row level, each changing one decision's classification at the tip; a target of another incarnation is refused; no test restores the store between two of the classifier's reads, so the position of the recheck after the last read is not discriminated
Guarantee: Classification is a function of the Kernel snapshot at one commit sequence, the bound roots the caller passes, and the verifier's identity; every read runs at that sequence, the result carries it, and the classifier reads no worktree, clock, or network and opens no repository.
Check: `always` - two classifications of one decision at one `CommitReadTarget` with equal bound roots are equal, the result carries that target, and a store that is not the target's incarnation at the first read or after the last is refused with `IncarnationMismatch`
Fault/timing angle: a worktree edit, a HEAD move, or a Kernel commit between two classifications at one sequence; a restore between two of the classifier's reads
Required faults and enabling state: F8 at the classification step, and later commits that retire the cited descriptor, invalidate the anchor, and delete the cited evidence, so the tip classifications differ (F3)
Confidence: high - [evidence](evidence/p6-new-verifiability-pure-function-of-snapshot.md). Read `classify` (`crates/daemon/src/attestation_verifier/classify.rs:295-296`), whose incarnation comparison after the last read is at `:306`, and `class_of` (`:314`): its reads are `claim_standing_at_within_budget` (`:324`), `anchor_condition_as_of` (`:351`), `evidence_liveness_at_within_budget` (`:411`), `linked_decision_evidence_within_budget` (`:435`), and `source_descriptor_citing_evidence` (`:473`), each at `target.through_commit` under one `EvalBudget`
Existing check: `crates/daemon/tests/attestation_verifier_classify.rs:950`, `:805`, `:713` (unaudited)
Impact: a classification that read HEAD or the clock would change between a pass's selection and its write set, so the write set's recheck at the classification's target would compare against another snapshot
Open questions:
- Is a restore between two of the classifier's reads detected? - resolved with answer: yes; the standing read and `evidence_liveness_at_within_budget` compare the incarnation under their reader guards, and `classify` compares it again after the last read. A restore only advances the restore generation, so the last comparison detects a restore between any two reads. No test restores between two reads, because `classify` has no seam between them; `a_target_of_another_incarnation_is_refused` checks the refusal.

### p6-new-verifier-input-anchor-and-document-citation

Type: safety
Reachability: test-only - `attestation_verifier::classify::classify` has no production caller at this branch; #1173 wires it behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1171](https://github.com/ahrav/eidnara/issues/1171)
Exercised: yes - a fixture store in the memory-capture shape constructs every reason, a verifiable decision citing a `messages` and a `repository_documents` descriptor, and decisions outside the input set
Guarantee: A decision is verifier input only when live, `Active`, with a live `reachable_from` anchor and a row- or event-level citation resolving to a live `repository_documents` descriptor with a digest; otherwise it carries exactly one reason from {`no_anchor`, `anchor_not_git`, `no_citation`, `citation_not_document`, `evidence_retired`, `descriptor_retired`, `digest_missing`, `self_source`}, or, when not live or not `Active`, no reason. A classification that returns an error, `InvalidDocumentPath` or a Kernel refusal, carries no class and counts under no reason. The resolved claim carries the descriptor's path and digest, the evidence row's byte length, and the anchor's oid, and no range, and binds the first bound root whose repository identity equals the descriptor's, or `repository_unbound` when none does.
Check: `always` - `verifiable XOR exactly one reason` for every live `Active` decision whose classification succeeds; a decision citing only `messages` evidence is never verifiable; a verifiable claim's root is the first bound root with the descriptor's repository identity, else `repository_unbound`
Fault/timing angle: an anchor, evidence row, or descriptor invalidated before the classification's sequence (F3)
Required faults and enabling state: a `messages`-only citation, no anchor, an `Exact`, a `ReachableBetween`, and a non-git anchor, a deleted evidence artifact, a retired descriptor, a descriptor without a digest, no citation, two reasons on one decision in citation order, a path outside the document path rule cited before a valid document, a row-level document citation, a deleted row-level citation, and event citations at the cap, one past the cap, and one past the cap with the row's own evidence among them
Confidence: high - [evidence](evidence/p6-new-verifier-input-anchor-and-document-citation.md). Read `class_of` (`crates/daemon/src/attestation_verifier/classify.rs:314`): the `Active` filter at `:334`, then the self-source check at `:340`, the anchor match after `:351`, the citation list from `citations` (`:401`), which reads the row-level `evidence_id` first and then at most `MAX_EVENT_CITATIONS` (`:53`, 32) event-level citations, and the loop over `resolve_citation` (`:464`), which returns at the first live document descriptor with a digest and a path inside the document path rule, otherwise `InvalidDocumentPath` when a cited live document descriptor with a digest has a path outside the rule, and otherwise the first cited row's reason; and the Kernel reads `source_descriptor_citing_evidence` (`crates/kernel/src/source_descriptor.rs:1213`) and `anchor_condition_as_of` (`crates/kernel/src/anchor_write.rs:469`)
Existing check: `crates/daemon/tests/attestation_verifier_classify.rs:355`, `:572`, `:667`, `:747`, `:805`, `:872`, `:1080`; `crates/kernel/tests/kernel_source_descriptors.rs:3252`, `:3446`; `crates/kernel/tests/kernel_anchor_write.rs:869` (unaudited)
Impact: falsifier: a `messages`-citing decision classified verifiable
Open questions:
- Is `ReachableBetween.start_oid` a legal input? - resolved with answer: no; the #1171 ticket records OQ6-10 as decided, `ReachableBetween` is `anchor_not_git` because no producer emits it.
- Which reason names a decision whose cited document descriptor was superseded by a revision citing other evidence? - resolved with answer: `descriptor_retired`; the evidence-keyed read finds descriptors of the class that cited the evidence, none live at the sequence. The reason covers a cited revision that is no longer live, including one whose lineage has a newer live revision, because C8 requires a live descriptor citing the cited evidence.
- Should `descriptor_retired` stay the reason when a newer revision of the cited lineage is live, given the ticket defines it as "a document lineage with no live revision"? This includes a revert: the new revision has the cited digest but a new evidence id, because the evidence id hashes the occurrence and its revision, so the evidence-keyed read does not reach it. (needs human input)
- Which citations does the classification inspect? - resolved with answer: the row-level `evidence_id` and at most `MAX_EVENT_CITATIONS` event-level citations drawn from the newest `LINKED_EVENT_WINDOW` × (`MAX_EVENT_CITATIONS` + 1) decision events; a decision whose only document citation lies past either bound classifies `no_citation` or `citation_not_document`, and the reasons name the inspected citations.
- Which descriptor resolves when several live document descriptors cite one evidence row? - resolved with answer: the first in object id order; equal texts share one evidence row, so the digest and byte length agree.

### p6-new-attestation-identity-unique-per-decision-oid

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - a fixture store commits an attestation, replays its key, conflicts the key under another request digest and the opposite verdict's key for the same decision and oid, and admits a second identity's subject through the first identity's witness, which stays below `Verified`
Guarantee: Every attestation id, its decision id, its witness observation and object ids, and its `source_id` are fixed hashes of `(model object id, oid)` under the prefixes `verattest:`, `verattestdec:`, `verwitness:`, and `verwitnessobj:`, outside the Kernel's reserved descriptor, causality, and local-file namespaces; `source_kind` is `verifier` and `source_revision` is 1; the operation key is `(verifier, attest:<model object id>:<oid>)`, so at most one attestation exists per pair. The Kernel derives the causality record's ids from the attestation's object id.
Check: `always` - live verifier decisions grouped by `(model object id, oid)` count at most one; a repeat of the key replays its receipt, the key under another request digest is `Conflict`, and a recorded opposite-verdict key for the pair is `Conflict`
Fault/timing angle: a retry after a crash or a replayed pass; a changed claim at the same oid
Required faults and enabling state: F10: the operation key replayed with the same and with a different verdict
Confidence: high - [evidence](evidence/p6-new-attestation-identity-unique-per-decision-oid.md). Read `Identity::of` (`crates/daemon/src/attestation_verifier/attest.rs:65`), `intent` (`:610`), whose request digest covers the frozen claim, the compared evidence, the blob, and the verdict, and `run` (`:494`), which refuses a recorded opposite key at `:506` before any write
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:895`, `:938`, `:1132` (unaudited)
Impact: falsifier: an id containing pass time or randomness, or two live attestations for one pair
Open questions:
- Can a retired attestation's pair be attested again? - resolved with answer: no; the registry keeps the retired ids, and OQ6-7 accepts that re-attesting a pair after retirement is out of scope.

### p6-new-attestation-classes-and-cited-fields

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: partial - a fixture store with an anchored model decision citing a `Sensitive` document descriptor by a decision event, one citing it by its row, one admitted under a `Personal` taint that the Kernel serves `Sensitive` over a `Normal` registry class, and one citing a `config.toml` descriptor commits attestations and reads their rows, admissions, citation events, and served standing; the `config.toml` attestation is admitted `ConfigObserved` under `(TrustedLocalCode, CurrentConfig)` on a `config_present` witness; no test reads the committed scope, decision kind, summary, or rationale, and every fixture model decision has `scope_id: None`
Guarantee: The attestation's classes are exactly `(TrustedLocalCode, CurrentCode|CurrentConfig)` by document kind; it carries the model decision's `anchor_id`, scope, domain, and kind and the sensitivity the Kernel serves the model decision at, copies the summary, names the oid and digest in its rationale, carries `evidence_id: None`, and cites the compared evidence through exactly one decision event of kind `verifier_citation`.
Check: `always` - `anchor_id` equals the model decision's, the one citation event's evidence equals the resolved and compared evidence, the decision row and the witness admission cite no evidence, and the classes are in the allowed pairs
Fault/timing angle: none
Required faults and enabling state: F14: the compared evidence is `Sensitive`; F15: a prose document classified `CurrentCode`
Confidence: high - [evidence](evidence/p6-new-attestation-classes-and-cited-fields.md). Read `WriteSet::new` (`crates/daemon/src/attestation_verifier/attest.rs:181`), which builds the decision, witness, and citation specs, and the writes at `:373` and `:421`; the admission at `:376` takes the classes from `DocumentKind::taint_class` and its event from `DocumentKind::event_kind` (`crates/daemon/src/attestation_verifier/classify.rs:530`); `WriteSet::check` (`crates/daemon/src/attestation_verifier/attest.rs:289`) requires the model decision's anchor, domain, and served sensitivity before the commit, and `WriteSet::write` rereads the served sensitivity in the envelope (`:366`); `classify` takes it from the Kernel's served standing (`crates/daemon/src/attestation_verifier/classify.rs:343`)
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:431`, `:572`, `:1092`, `:1399`, `:1434` (unaudited)
Impact: falsifier: no citation event, or a different evidence row, at `Verified`
Open questions:
- Is a configuration document's attestation exercised? - resolved with answer: yes; `a_match_on_a_configuration_document_commits_under_current_config` commits a `config.toml` attestation and reads `(TrustedLocalCode, CurrentConfig)`, `ConfigObserved`, `config_present`, `admit`, `verified`, and `active`.

### p6-new-attestation-sibling-model-stays-candidate

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - after a match the fixture compares the model decision's registry row and own admission with their values before the write, and reads the attestation's causality class; a superseded, retired, quarantined, or stale model decision, a retired anchor, a retired descriptor, and a deleted evidence row abort the set as `changed`; a second model decision classified at the same sequence still commits after the first one's attestation
Guarantee: The attestation is a new lineage linked by a `DerivedReinjection` record naming the model decision at its revision; the model decision stays live at `Candidate` with its classes and receives no `Replace`, `Correct`, or promoting event; a model decision superseded, retired, or re-disposed after the classification aborts the set.
Check: `always` - after a match the model decision's registry row and own admission are unchanged, and the attestation is live with a causality record naming `(model object id, model revision)`; after a change to the model decision between the classification and the commit, no verifier row exists
Fault/timing angle: the model decision, its anchor, its descriptor, or its evidence row changed between the classification and the commit; an unrelated attestation committed in that window
Required faults and enabling state: F2, F3
Confidence: high - [evidence](evidence/p6-new-attestation-sibling-model-stays-candidate.md). Read `recheck` (`crates/daemon/src/attestation_verifier/attest.rs:543`), which compares the store's incarnation (`:552`), the model decision's registry row and latest admission, its anchor (`:580`) at the sequence the envelope extends, and its descriptor lineage (`:589`), whose live read requires the cited evidence row live, all read in the envelope's transaction, and the causality write at `:395`
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:431`, `:965`, `:1067`, `:1542`, `:1585`, `:1613` (unaudited)
Impact: falsifier: a `Replace` on the model decision, or an attestation without causality
Open questions:
- What does the causality read report after the model decision is superseded? - resolved with answer: `DerivedReinjection` naming `(model object id, model revision)` while the attestation keeps `Verified`; `parents_existed` (`crates/kernel/src/claim_causality.rs:719`) checks only that the parent row existed at the requested snapshot with the named revision, and supersession leaves that row and revision in place. OQ6-12 expected `Unknown(ParentUnavailable)`, which the Kernel reports only for a missing historical parent row or a revision mismatch; no test reads it.

### p6-new-attestation-verified-only-via-witness

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - a committed attestation's own admission is a `CodeObserved` event whose trigger is its witness; a witness whose kind a storage fault rewrites at insert leaves the admission below `Verified` and the set aborts; a second identity's subject admitted through the first identity's witness stays at `Candidate`
Guarantee: An attestation reaches `Verified` only through a `CodeObserved` or `ConfigObserved` event validated against an observation with the attestation's source triple; no model-class decision changes maturity through the verifier.
Check: `always` - every `Verified` attestation has a witness admission row with a non-null trigger naming its own witness, and model maturities are unchanged across a write set
Fault/timing angle: none
Required faults and enabling state: F1: a trigger of another kind; a trigger carrying another attestation's source triple
Confidence: high - [evidence](evidence/p6-new-attestation-verified-only-via-witness.md). Read the admission request at `crates/daemon/src/attestation_verifier/attest.rs:376`, the only admission of the attestation, whose trigger is the witness inserted at `:374`; the Kernel's trigger lookup compares the source triple (`crates/kernel/src/admission.rs:2807`)
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:431`, `:854`, `:1132` (unaudited)
Impact: falsifier: `Verified` from insert alone
Open questions: None.

### p6-new-witness-evidence-id-equals-attestation

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - mutated write sets with a witness at another revision, another source id, a detail naming other evidence, a detail at another `witness_version`, another blob id, a detail naming another commit oid or artifact digest, a citation naming other evidence, the other witness kind, another sensitivity or domain on either row, no anchor, an attestation object, decision, or source id or a witness observation or object id other than the pair's `Identity`, and a row-level `evidence_id` on either row are each refused before the commit with nothing written; a `Match` or `Mismatch` without a blob id is `invalid_write_set`
Guarantee: Before the write, the observation detail's evidence id, the citation event's evidence id, and the compared evidence row are equal, the detail is at `WITNESS_DETAIL_VERSION` and names the write set's blob id and the claim's commit oid, digest, path, descriptor, lineage, and descriptor revision, the citation is a `verifier_citation` event, the observation's source triple equals the attestation's under `verifier` at revision 1, every id is the `Identity` of the model decision and the claim's oid, both rows carry the model decision's domain and served sensitivity and one scope and the attestation its anchor, the observation kind matches the document kind, and neither row cites evidence by its own `evidence_id`.
Check: `always` - `WriteSet::check` holds for every write set `WriteSet::commit` submits, and a write set that fails it returns `invalid_write_set` before the commit
Fault/timing angle: none
Required faults and enabling state: F1, F3: a detail or citation naming another live evidence row
Confidence: high - [evidence](evidence/p6-new-witness-evidence-id-equals-attestation.md). Read `WriteSet::check` (`crates/daemon/src/attestation_verifier/attest.rs:289`), called first in `WriteSet::commit` (`:342`), and `recheck` (`:543`), which also requires the cited descriptor to still cite the compared evidence
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:749`, `:685`, `:1469` (unaudited)
Impact: falsifier: a `Verified` attestation citing E whose witness detail names E'
Open questions: None.

### p6-new-witness-degradation-never-silent

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - a storage fault rewrites the witness's observation kind as the Kernel inserts it, so its trigger fails validation and the admission returns below `Verified`; the envelope rolls back, the call returns `witness_degraded`, and no verifier row exists; the committed path's admission is `admit` at `verified` and `active`
Guarantee: The envelope aborts unless the returned admission has effective maturity `Verified` and disposition `Active`; the first admission of a subject yields `Outcome::Admit`, so the outcome is not the oracle, and a degraded (`Other`) event never commits and returns `witness_degraded`.
Check: `always` - a committed attestation's own admission has effective maturity `verified` and disposition `active`, and every other returned admission rolls the envelope back with `witness_degraded`; oracle amended by the write-set ticket
Fault/timing angle: the witness changed between its insert and the admission in one envelope
Required faults and enabling state: F1: a witness whose trigger fails validation inside the envelope
Confidence: high - [evidence](evidence/p6-new-witness-degradation-never-silent.md). Read the assertion at `crates/daemon/src/attestation_verifier/attest.rs:389` after the admission at `:376`, whose failure stops the envelope through `run` (`:494`)
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:854`, `:431` (unaudited)
Impact: falsifier: a committed attestation at `Candidate`
Open questions:
- Should repeated degradation stop the task? (needs human input)

### p6-new-attestation-write-set-one-outcome

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: partial - a storage fault refuses the citation event, the last write after the witness admission and the causality record, and the fixture finds no attestation, observation, causality, citation, or admission row and the model decision unchanged; after a commit, a clean close, and a fresh open every companion row exists and the key replays its receipt at the same commit; a write set built at a sequence before the model decision existed is `changed`; no test terminates the process after the commit
Guarantee: Attestation, causality record, citation event, witness observation, witness admission, and receipt commit in one transaction or none, and each call returns one `Outcome`.
Check: `always` - a live attestation always has all five companions and its receipt, before and after a reopen
Fault/timing angle: a fault after the admission inside the envelope; a restart after the commit
Required faults and enabling state: F11: a fault-injected rollback after the admission, constructed; a process termination after the commit followed by a fresh open, not yet constructed (a clean close and fresh open shows durability across an open only)
Confidence: high - [evidence](evidence/p6-new-attestation-write-set-one-outcome.md). Read `WriteSet::write`'s order, insert at `crates/daemon/src/attestation_verifier/attest.rs:373` through the citation at `:421`, all inside one `commit_within_budget` call in `run` (`:494`)
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:1178`, `:727` (unaudited)
Impact: falsifier: an attestation at `Candidate` with no observation after a crash
Open questions:
- Does the reopen model a crash? - resolved with answer: partly; the test records a fresh open after close. A live store holds its data home's writer lock, so an abandoned store (`std::mem::forget`) makes the second open fail with "held by another writer"; the test closes the store and opens it again, which shows the commit is durable across a fresh open and does not kill the process mid-write.

### p6-new-mismatch-stale-unavailable-silent

Type: safety
Reachability: test-only - `attestation_verifier::attest::attest` and `WriteSet::commit` have no production caller at this branch; #1173 wires them behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - a fixture whose commit holds other bytes than the cited document drives a `Mismatch` through the write set; a second run classifies the stale decision outside the input and the old classification replays; a decision already `Stale` at the commit is skipped; a decision quarantined after its classification is `changed` with no `MarkStale`; `Unavailable` verdicts with and without a blob id write nothing
Guarantee: On `Mismatch` exactly one `MarkStale` is written on the model decision under its latest admission's classes, with `evidence_id: None` and a reason naming the oid, the cited sha256 digest, and the git blob id observed at the path, keyed `(verifier, stale:<model object id>:<oid>)`, and nothing else; a decision already `Stale` is skipped before any write; on `Unavailable` no Kernel write occurs.
Check: `always` - after `StaleCommitted` the model decision is `Stale` with one new admission row and no verifier object exists for `(decision, oid)`; `Replayed`, `Conflict`, `Skipped(AlreadyStale)`, and `Changed` add no row; after `Unavailable` the tip is unchanged
Fault/timing angle: the model decision marked `Stale` between the classification and the commit
Required faults and enabling state: a commit holding other bytes at the cited path; `Unavailable` verdicts with and without a blob id: the fixture supplies `RepositoryUnbound`, `PathAbsent` (F6), and `OverBudget` (F4), and no F5 verdict; `attest` treats every `Unavailable` reason alike
Confidence: high - [evidence](evidence/p6-new-mismatch-stale-unavailable-silent.md). Read `attest` (`crates/daemon/src/attestation_verifier/attest.rs:125`), which returns `skipped(unavailable)` for every `Unavailable` verdict before any commit, and `mark_stale` (`:428`), whose skip is at `:454` and whose one admission is at `:459`
Existing check: `crates/daemon/tests/attestation_verifier_attest.rs:597`, `:660`, `:685`, `:1585` (unaudited)
Impact: falsifier: an attestation on `Mismatch`, or `MarkStale` on `Unavailable` or on the attestation
Open questions:
- Which two digests does the reason name? - resolved with answer: the cited sha256 and the git blob id at the path; the driver reports no content digest for a mismatch the header size decides, so the blob id is the observed value the reason carries.

### p6-new-party-distinct-and-self-refusal

Type: safety
Reachability: default-production - the plugin-route half: `kernel.commit` refuses `verifier` on every request; the `self_source` half and the model-client clause are test-only until #1173 wires the verifier behind `verifier.enabled`
Status: active - the `self_source` half since [#1171](https://github.com/ahrav/eidnara/issues/1171), the plugin-route half and the model-client clause by [#1172](https://github.com/ahrav/eidnara/issues/1172)
Exercised: yes - the plugin route refuses `source_kind = "verifier"` as `invalid_input` for a decision, an observation, and a `mark_stale`, and refuses `trusted_local_code`, `current_code`, and `current_config` asserted under `assistant` as `class_over_declared`, with no write; a fixture store classifies a committed attestation `self_source` and `attest` returns `skipped(not_verifiable)` with no commit; a source scan of every `.rs` file under `src/attestation_verifier/` and `src/attestation_verifier.rs`, listed at test time, finds no model-client module or network crate, with a positive control
Guarantee: `verifier` is outside the plugin table; every live `Active` `source_kind == "verifier"` decision is classified `self_source`, and every other `verifier` decision is outside the input set, so the write set is never entered for one; the verifier module names no model client.
Check: `always` - `derive_classes("verifier")` is `None` and the route answers `invalid_input`; live `Active` attestations are `self_source` on the next pass, and other attestations are `NotInput`; every `crate::` path the verifier sources name is in a closed allowlist of model-free modules
Fault/timing angle: none
Required faults and enabling state: a store holding an attestation; a plugin write under `verifier`
Confidence: high - [evidence](evidence/p6-new-party-distinct-and-self-refusal.md). Read the `self_source` check at `crates/daemon/src/attestation_verifier/classify.rs:340`, after the standing read (`:324`) and its `Active` filter (`:334`) and before the anchor read (`:351`); `derive_classes` (`crates/daemon/src/kernel_routes/commit.rs:370`) through `resolve_classes` (`:413`); `attest` (`crates/daemon/src/attestation_verifier/attest.rs:125`) returns before any commit for a classification that is not verifiable
Existing check: `crates/daemon/tests/attestation_verifier_classify.rs:590`; `crates/daemon/tests/kernel_routes.rs:6778`; `crates/daemon/tests/attestation_verifier_attest.rs:431`, `:1246` (unaudited)
Impact: falsifier: the verifier attests its own attestation
Open questions:
- Does anything beyond the plugin route reserve `verifier`? - resolved with answer: no other daemon route has a source-kind table; `kernel.artifact.ingest` (`crates/daemon/src/kernel_routes/ingest.rs:142`) accepts any `source_kind` for an evidence object, which cannot be a decision or a witness trigger. A plugin can still `mark_stale` or retire an attestation through `kernel.commit` under its own source kind (OQ6-15, documented in #1174).

### p6-new-flag-off-nothing-runs

Type: safety
Reachability: default-production - intended; the behavior has no code at this branch
Status: proposed - converts in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Exercised: not yet - the code lands in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Guarantee: With `verifier.enabled` false, the default, no pass runs, no repository opens, and no `verifier` row exists; a reload to false stops the next pass.
Check: `unreachable` for the pass body, a code location that must not execute while the flag is false; `always(!X)` for the store: `COUNT(object_registry WHERE source_kind='verifier') = 0` in a default store; a project-tier value is refused
Fault/timing angle: see the fault map
Required faults and enabling state: F12: flag absent, reload true to false, a project-tier flag
Confidence: low - [evidence](evidence/p6-new-flag-off-nothing-runs.md). designed obligation from the specification ([#1169](https://github.com/ahrav/eidnara/issues/1169)); no code to read
Existing check: none
Impact: falsifier: a pass before the flag is read, or a verifier row in a default store
Open questions: None.

### p6-new-pass-bounds-and-accounting

Type: safety
Reachability: explicit-config-only - intended; the behavior has no code at this branch
Status: proposed - converts in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Exercised: not yet - the code lands in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Guarantee: A pass considers at most N decisions per project, reads at most N x 256 KiB, stops at the deadline, puts each considered decision in exactly one verdict bucket, and increments effect counters only after commit.
Check: `always` - `attempted <= N`, `bytes_read <= budget`, `sum(verdict buckets) == considered`, `attestation_committed <= match`
Fault/timing angle: see the fault map
Required faults and enabling state: F7 between decisions; F13: more than N candidates
Confidence: low - [evidence](evidence/p6-new-pass-bounds-and-accounting.md). designed obligation from the specification ([#1169](https://github.com/ahrav/eidnara/issues/1169)); no code to read
Existing check: none
Impact: falsifier: a pass looping to exhaustion, double counting, or an effect counted before commit
Open questions:
- `Evaluation::bytes_read` counts blob content only; commit and tree payloads and delta materialization are excluded. Does the #1173 pass budget cover blob content or all decoded-object work? (needs human input)

### p6-new-cursor-never-skips-unresolved

Type: safety
Reachability: explicit-config-only - intended; the behavior has no code at this branch
Status: proposed - converts in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Exercised: not yet - the code lands in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Guarantee: The cursor passes a decision only after a terminal outcome (attested, stale, unverifiable, terminal `Unavailable`, or K transient attempts); revisiting an attested or `Stale` decision writes nothing, including after a restart.
Check: `always` - every decision below the cursor has a terminal outcome, and revisits write nothing
Fault/timing angle: see the fault map
Required faults and enabling state: F7, F13
Confidence: low - [evidence](evidence/p6-new-cursor-never-skips-unresolved.md). designed obligation from the specification ([#1169](https://github.com/ahrav/eidnara/issues/1169)); no code to read
Existing check: none
Impact: falsifier: a `deadline` decision passed and never revisited, or a second `MarkStale`
Open questions:
- What are N and K? - resolved with answer: N = 8 and K = 3 ([#1173](https://github.com/ahrav/eidnara/issues/1173), OQ6-10).

### p6-new-pass-progress-under-transient-unavailable

Type: liveness
Reachability: explicit-config-only - intended; the behavior has no code at this branch
Status: proposed - converts in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Exercised: not yet - the code lands in [#1173](https://github.com/ahrav/eidnara/issues/1173)
Guarantee: While passes run at the retry interval, a head decision reaches a terminal outcome within K = 3 passes.
Check: `always` - every head decision that is transient-unavailable has a terminal outcome recorded within K = 3 passes of its first transient attempt; paired with `sometimes(head_decision_transient_unavailable)` as precondition coverage
Fault/timing angle: see the fault map
Required faults and enabling state: F13: a permanently transient head decision
Confidence: low - [evidence](evidence/p6-new-pass-progress-under-transient-unavailable.md). designed obligation from the specification ([#1169](https://github.com/ahrav/eidnara/issues/1169)); no code to read
Existing check: none
Impact: falsifier: a head decision consuming every pass
Open questions: None.

### p6-new-attestation-stale-on-byte-change

Type: safety
Reachability: explicit-config-only - intended; the behavior has no code at this branch
Status: proposed - converts in [#1174](https://github.com/ahrav/eidnara/issues/1174)
Exercised: not yet - the code lands in [#1174](https://github.com/ahrav/eidnara/issues/1174)
Guarantee: When a pass evaluates an attestation whose lineage's live document descriptor has a digest different from the witnessed digest, the pass marks the attestation `Stale`; a lineage with no live descriptor, or an undecodable witness detail, writes nothing (a documented limitation).
Check: `always-or-unreached` - whenever a pass evaluates such an attestation the result is `Stale`; unreached until #1174 lands
Fault/timing angle: see the fault map
Required faults and enabling state: source capture republishing the cited document's lineage with a different digest after the attestation
Confidence: low - [evidence](evidence/p6-new-attestation-stale-on-byte-change.md). designed obligation from the specification ([#1169](https://github.com/ahrav/eidnara/issues/1169)); no code to read
Existing check: none
Impact: falsifier: a live descriptor with a different digest and the attestation left `Active`
Open questions:
- Which mechanism carries applicability? - resolved with answer: OQ6-4 alternative 3, the push model inside the pass ([#1174](https://github.com/ahrav/eidnara/issues/1174)).

### p6-new-verdict-deterministic

Type: safety
Reachability: test-only - `attestation_verifier::verify` has no production caller at this branch; #1173 wires it behind `verifier.enabled` and relabels the record `explicit-config-only`
Status: active - converted by [#1170](https://github.com/ahrav/eidnara/issues/1170)
Exercised: yes - bare fixtures without a worktree drive every verdict and every reason except `repository_unbound`, which `verify` never produces since it takes a resolved root and which only an `Observed` double reaches; a non-bare root shows a worktree edit and a staged index leave the verdict unchanged; one test repeats claims across a worktree edit, a HEAD move, and a repack
Guarantee: The verdict is a function of object-store contents, the claim's path, digest, byte length, and oid, and the budgets; `Match` asserts only that the sha256 of the bytes git stores for the path in the commit's tree equals the digest, up to line-ending normalization.
Check: `always` - equal claims against equal object-store contents give equal verdicts, except when either evaluation returns a transient reason (`deadline`, `object_unreadable`, `repository_unopenable`, or `repository_unbound`, which environmental failures such as descriptor exhaustion can produce) or `over_budget` from the allocation limit; for an `Observed::Content` admitted by the header gate, `Match` holds iff the header size equals the claimed length and the raw digest equals the claim, or the exact-length normalized digest equals it; the verdict function takes no repository handle and no clock
Fault/timing angle: a worktree edit, a HEAD move, or a repack between two evaluations of one claim
Required faults and enabling state: F8: worktree edit, HEAD move, repack, excluding allocation-limit refusals; F9: an LF blob with a CRLF-form digest, and the reverse
Confidence: high - [evidence](evidence/p6-new-verdict-deterministic.md). Read `verdict` (`crates/daemon/src/attestation_verifier.rs:329`), `Digests::of` (`:245`), and `lookup` (`:538`), which `verify` (`:381`) calls: the driver reads the commit, trees, header, and blob by object id and passes the observation to `verdict`, which decides every verdict
Existing check: `crates/daemon/tests/attestation_verifier.rs:724`, `:202`, `:212`, `:276`, `:505`, `:805`, `:879`, `:951`, `:970`, `:1071` (unaudited)
Impact: a verdict that depended on worktree state would mark a correct finding stale after an unrelated edit
Open questions:
- How are line endings normalized? - resolved with answer.
- Does the length shortcut hold as AC3 words it, a header size different from the claimed length being `Mismatch` with zero bytes read? AC1 requires a CRLF-form claim of a different length to `Match`, so the implementation reads the blob whenever a line-ending form can reach the claimed length, a deviation from AC3's literal wording. (needs human input)
- Is `over_budget` from the allocation limit terminal? It depends on how the pack stores the commit, a tree on the path, or the blob, and a repack can change it, so F8's repack clause excludes it; the ticket's mapping and terminal split are kept for the #1173 (U3) owner to decide. (needs human input)

### p6-new-blob-read-bounded-at-oid

Type: safety
Reachability: test-only - `attestation_verifier::verify` has no production caller at this branch; #1173 wires it behind `verifier.enabled`
Status: active - converted by [#1170](https://github.com/ahrav/eidnara/issues/1170)
Exercised: partial - the per-claim clauses are exercised over bare fixtures; the per-pass budget lands with #1173
Guarantee: The driver checks the blob's header against the 256 KiB per-blob budget before reading any content, reads at most one blob per claim, opens through the daemon opener with the budget as allocation limit, polls the deadline before the open and before each object read, and opens no worktree path.
Check: `always` - every claim's `bytes_read <= MAX_BLOB_BYTES`; every claim's poll trace is `Open`, `Commit`, one `Tree` per path component, `Header`, then at most one `Blob`, and ends at the first passed poll, each poll preceding its read; an over-budget header yields `Unavailable(over_budget)` with `bytes_read == 0` and no blob poll; a passed deadline yields `Unavailable(deadline(stage))` with `bytes_read == 0`; a missing object, a failed full decode, kind, or hash check, or an id whose every holding own pack revisits an entry in its delta walk, names a base outside the pack, or does not decode, and which has no valid loose copy, yields `object_unreadable(class)`, an absent entry `path_absent`, and a gitlink or symlink at or above the path or a tree at the final component `not_a_blob(kind)`; no verification writes to the repository; the per-pass clause `bytes_read_pass <= budget` converts with #1173
Fault/timing angle: the deadline crossing between object reads
Required faults and enabling state: F4: header over budget, an over-budget delta base, bytes that do not hash to the id; F5: missing and corrupt commit, tree, and blob, including a valid prefix with a malformed remainder and a self-referencing ref-delta; F6: absent path, gitlink, symlink, tree, a path beneath a gitlink; F7: deadline before the open and before the header poll
Confidence: high - [evidence](evidence/p6-new-blob-read-bounded-at-oid.md). Read `lookup` (`crates/daemon/src/attestation_verifier.rs:538`), `Objects::read` (`:485`), `Objects::blob_header` (`:468`), and `header_verdict` (`:313`); `MAX_BLOB_BYTES` (`:37`) is both the header budget and the allocation limit of the store, the own packs, and the loose store, and `OwnPacks::decode` (`crates/daemon/src/git_sources.rs:329`) bounds each ref-delta walk through `entry_header` (`:445`) before it decodes the object
Existing check: `crates/daemon/tests/attestation_verifier.rs:541`, `:594`, `:678`, `:704`, `:366`, `:324`, `:485`, `:212`, `:253`, `:475`, `:505`, `:651`, `:1012`, `:1037`, `:782`, `:1115`, `:1146`, `:1197`, `:1224`, `:1240`, `:1269`, `:1307`, `:1358`, `:1378` (unaudited)
Impact: an unbounded read would let one claim allocate a delta chain or read a large blob inside the pass
Open questions:
- Does the allocation limit admit a blob of exactly 256 KiB? - resolved with answer.
- Which reason does a commit or tree above the allocation limit take? - resolved with answer: `over_budget`; a tree above 256 KiB is covered loose and packed, and no verifier test covers an oversized commit or either object on an oversized delta base.

## Relationships

- `p6-new-verdict-deterministic` and `p6-new-blob-read-bounded-at-oid` rest on
  `p6-kernel-checkout-open-has-no-allocation-limit-and-honors-replacements`:
  the driver uses the daemon opener, which installs the allocation limit and
  ignores replacements, and header-gates its one blob read, the blob-only
  analogue of the order `p6-blob-read-is-header-gated-before-decode` records
  for `read_selection`; its commit, tree, and blob reads decode from the
  repository's own packs after the bounded delta-chain walk, or read from its
  loose store, under the allocation limit, and any other object reads through
  the object store (`try_find_header` for the blob header, `find_object` for
  content) under the same limit.
- `p6-new-verifier-input-anchor-and-document-citation` builds the claim from
  the descriptor `p6-document-descriptor-identity-digest-and-revision`
  describes, read by evidence id, and `p6-new-verifiability-pure-function-of-snapshot`
  fixes the snapshot whose facts the write set (#1172) compares with the
  envelope's current state; `WriteSet::new` reads the model decision's row at
  that snapshot's sequence.
- `p6-new-verdict-deterministic` consumes the claim
  `p6-document-descriptor-identity-digest-and-revision` describes: path,
  digest, and byte length of a document descriptor, plus the anchor's oid.
- `p6-new-attestation-verified-only-via-witness` and
  `p6-new-witness-degradation-never-silent` depend on
  `p6-trigger-must-match-subject-source-triple`,
  `p6-degraded-witness-event-denies-without-error`, and the positive witness
  `p6-validated-witness-under-trusted-classes-promotes`.
- `p6-new-attestation-sibling-model-stays-candidate` follows from
  `p6-witness-cannot-lift-an-inference-class-subject` and
  `p6-successor-keeps-identity-and-advances-revision`, and uses
  `p6-derived-reinjection-parents-live-at-write-and-existed-at-read` for the
  link.
- `p6-new-attestation-classes-and-cited-fields` avoids the fold
  `p6-document-evidence-cited-by-fk-serves-no-wider-than-explicit-search`
  and `p6-trigger-sensitivity-composes-into-admission` describe, by citing
  through a decision event.
- `p6-new-mismatch-stale-unavailable-silent` uses
  `p6-mark-stale-withdraws-support-and-is-terminal-in-place`.
- `p6-new-party-distinct-and-self-refusal` depends on
  `p6-plugin-source-kind-table-is-closed-and-classes-never-raised` and
  `p6-plugin-cannot-replace-a-foreign-source-decision`.
- `p6-new-attestation-stale-on-byte-change` needs a check kind that
  `p6-object-applicability-payload-fails-closed` and
  `p6-failed-cheap-check-is-stale-and-handed-to-repair` would carry.
