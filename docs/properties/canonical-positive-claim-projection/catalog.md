# Canonical positive-claim projection: catalog

Method: `../METHOD.md`. Records were verified against the kernel crate in the
change that introduced `crates/kernel/src/claim_causality.rs` and
`crates/kernel/src/claim_facts.rs`. References name functions and tests rather
than line numbers because both files were authored in the same change.

## Scope

The kernel side of RP2.4: the claim facts reader (`KernelStore::claim_facts_as_of`),
the trusted causality write path (`Envelope::record_claim_causality`), and the
snapshot-bound causal classifier (`claim_causality::causal_class_at`); and the
projection side: `retrieval::claims`, which classifies live claim occurrences
into Current, Superseded, Retracted, Hidden, or Stale from those facts at one
kernel snapshot and stores no state. Eligibility per surface, checkout
applicability, and harness delivery are separate implementation boundaries;
their records join this catalog with the ticket that lands them.

## Reachability classes

- `default-production`: reached by the kernel store API with no configuration.
  The claim facts reader and the causality writer are library calls; no daemon
  route calls them yet, so a production request cannot reach them today. Every
  record below is therefore `test-only` until a caller lands.
- `test-only`: constructed only by `crates/kernel/tests/kernel_claim_facts.rs`.

## Index

Every slug the specification and its three implementation tickets assign. The
`Record` column says whether this file holds a record for it yet.

| Slug | Ticket | Record |
| --- | --- | --- |
| `bound-project-scope-cannot-be-widened-by-candidate` | #466 | pending |
| `candidate-validation-preserves-surface-policy` | #466 | pending |
| `canonical-claim-fields-match-fenced-source` | #458 | yes |
| `canonical-provenance-survives-approved-write-read-path` | #458 | yes |
| `checkout-applicability-is-revalidated-without-relevance-refresh` | #466 | pending |
| `claim-cancellation-preserves-durable-work` | #460 | yes |
| `claim-capacity-failure-is-atomic` | #458 | yes |
| `claim-consumer-replay-includes-published-history` | #460 | yes |
| `claim-disable-preserves-consumer-contract` | #460 | yes |
| `claim-enablement-requires-approved-evidence` | #458 | yes |
| `claim-export-predecode-bounds` | #458 | yes |
| `claim-export-retention-fence` | #460 | yes |
| `claim-format-rollback-preserves-canonical-state` | #458 | yes |
| `claim-local-commit-before-ack` | #460 | yes |
| `claim-rebuild-incremental-parity` | #460 | yes |
| `claim-recovery-converges-within-approved-bound` | #460 | yes |
| `claim-replay-preserves-newest-canonical-state` | #458 | yes |
| `claim-tombstone-masks-all-representations` | #460 | yes |
| `claim-worker-result-cannot-outlive-identity` | #460 | yes |
| `echo-classification-requires-canonical-causality` | #458 | yes |
| `eligibility-cache-cannot-change-canonical-verdict` | #466 | pending |
| `eligible-positive-and-unknown-claims-remain-reachable` | #460 | yes |
| `malformed-required-field-stops-projection-progress` | #458 | yes |
| `occurrence-identity-is-not-payload-or-source-triple` | #458 | yes |
| `optional-edits-require-host-capability-and-survival-proof` | #466 | pending |
| `projection-has-no-second-truth-or-policy-authority` | #458 | yes |
| `retrieved-content-cannot-upgrade-write-authority` | #458 | yes |
| `revision-domains-remain-distinct-and-supported` | #458 | yes |
| `served-sensitivity-and-artifact-policy-govern-egress` | #458 | yes |
| `stale-projection-cannot-authorize-current-use` | #466 | pending |
| `supporting-authority-is-preserved-not-recomputed` | #458 | yes |
| `u5-class-transition-situations-are-witnessed` | #466 | pending |
| `u5-evaluation-keeps-provenance-and-judgment-separate` | #466 | pending |
| `u5-rejection-and-unknown-accounting-is-lossless` | #466 | pending |
| `unknown-echo-state-is-policy-neutral` | #460 | yes |

## Records

### canonical-claim-fields-match-fenced-source

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the kernel reader is compared with the writer's reported
admission and with the serving route at one snapshot; comparison of projected
rows against this reader at a fenced export snapshot belongs to the
materialization ticket.
Guarantee: Every revisioned field `claim_facts_as_of` returns for a claim at
snapshot S (registry, decision, own and lineage admission rows, occurrences,
causality) equals the stored canonical row at S, and a later commit does not
change those fields for S. `served` and `supporting_approval.valid_at_snapshot`
are excluded: they read the cited evidence's class as it is today
(`admission.rs` `served_classes` and `supporting_approval_valid_sql`), and
classification merging rewrites `evidence_meta.sensitivity_class` in place
(`cas/ingest.rs` `MergedClassification::apply`), so a later tightening changes
them for an older S. That is fail-closed serving, not a snapshot fact.
Check: `always` - for each returned claim, own and lineage admission fields,
decision fields, and registry fields equal the rows selected at S by an
independent query; re-reading S after further commits yields an equal value
for the revisioned fields; re-reading S after the cited evidence is tightened
may change `served` and `supporting_approval.valid_at_snapshot`, but no
revisioned field.
Fault/timing angle: a write between two reads of the same S; a snapshot before
the object existed; evidence reclassified after S.
Required faults and enabling state: at least one later commit after S touching
the same lineage; one request naming an id created after S; the same artifact
re-ingested as secret after S.
Confidence: medium - [evidence](evidence/canonical-claim-fields-match-fenced-source.md).
Verified `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` in
`crates/kernel/tests/kernel_claim_facts.rs` writes through the public API and
compares field by field, and `served_facts_follow_the_cited_evidence_class_read_today`
rereads S after tightening.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs` - `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot`, `served_facts_follow_the_cited_evidence_class_read_today`; status unaudited.
Impact: a projection built from drifting facts serves approval or maturity the
kernel never granted; a cache or fenced comparison that treats `served` as
fixed at S misses a later tightening.
Open questions:
- The fenced export comparison at S is the materialization ticket's oracle; this
  record covers the reader only.

### canonical-provenance-survives-approved-write-read-path

Type: safety
Reachability: test-only
Status: active
Exercised: yes - write, reopen, and read of a causality record are constructed
in both test files; every string the detail carries passes the identity check
before the detail is built, so no redaction rewrite can reach a stored record.
Guarantee: A causality record written through `record_claim_causality` reads
back after store reopen with the same class, parents, evidence binding,
operation, and producer, and no string it stores can carry a detected secret.
Check: `always` - `claim_facts_as_of` and `causal_class_as_of` before and after
reopen are equal for the same snapshot; every id in the request passes
`identity` before the detail is built.
Fault/timing angle: process restart between write and read.
Required faults and enabling state: a store closed and reopened with a live
record.
Confidence: medium - [evidence](evidence/canonical-provenance-survives-approved-write-read-path.md).
Verified `facts_survive_reopen` and the reopen half of
`oversized_payloads_read_as_unknown_and_records_survive_reopen`.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs` - `facts_survive_reopen`; `crates/kernel/tests/kernel_claim_causality.rs` - `oversized_payloads_read_as_unknown_and_records_survive_reopen`; status unaudited.
Impact: causal lineage that does not survive its own storage cannot fence echo
classification downstream.
Open questions: None.

### claim-capacity-failure-is-atomic

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the reader refuses a request over `max_claims` before any
row is read; a second record for one subject and an over-bound parent list
poison the commit so nothing of it lands. Projection-side whole-commit refusal
belongs to the materialization ticket.
Guarantee: Exceeding a declared bound produces a refusal with no partial
effect: no rows read for a refused request, no rows written for a refused
commit.
Check: `always(!partial)` - after a refused read the returned value is an error
with no claims; after a refused commit the commit sequence and row counts are
unchanged.
Fault/timing angle: none.
Required faults and enabling state: a request naming `max_claims + 1` ids; a
derivation naming `MAX_DERIVATION_PARENTS + 1` parents.
Confidence: medium - [evidence](evidence/claim-capacity-failure-is-atomic.md).
Verified `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly`
and the `too-many` case of `derived_reinjection_rests_on_exact_live_parents`.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; `crates/kernel/tests/kernel_claim_causality.rs`; status unaudited.
Impact: a partially applied over-limit request leaves the store or its readers
with an inventory that never corresponds to one bound.
Open questions: None.

### claim-enablement-requires-approved-evidence

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no enablement gate exists in the kernel; the reader and
writer are library calls with no production caller.
Guarantee: No production path enables claim projection or delivery without the
approved RP2.9 limits and evidence the specification requires.
Check: `unreachable` - a production route that calls `claim_facts_as_of` or
`record_claim_causality` without an admitted `projection_gates` hook must not
exist.
Fault/timing angle: none.
Required faults and enabling state: a daemon caller wired to these APIs.
Confidence: low - [evidence](evidence/claim-enablement-requires-approved-evidence.md).
Verified by search: no caller outside `crates/kernel` at authoring time.
Existing check: none.
Impact: unapproved capacity or evidence reaches users.
Open questions:
- Which `ProjectionHook` variant will gate claim retrieval? (needs human input)

### claim-export-predecode-bounds

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the causal detail's stored length is compared with the
caller's bound before the payload is read; the RP2.1 export bounds for
descriptor pages already exist in `crates/kernel/src/source_export.rs`.
Guarantee: No causal payload larger than `max_causal_payload_bytes` is read or
decoded; the class is `Unknown(Oversized)` and the record summary carries the
row's identity with `operation == None`.
Check: `always` - a record whose stored payload length exceeds the bound yields
`Unknown(Oversized)` and a summary whose `operation` is `None`; the bound is
compared with `length(observation_payload)` before the payload column is read.
Fault/timing angle: none.
Required faults and enabling state: a bound smaller than a real record.
Confidence: medium - [evidence](evidence/claim-export-predecode-bounds.md).
Verified `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly`
in both test files. An earlier draft of this record said no summary is
returned; the code returns the identity-only summary, and the record follows
the code.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; `crates/kernel/tests/kernel_source_export.rs` for descriptor pages; status unaudited.
Impact: an attacker-sized detail forces unbounded allocation in every reader.
Open questions: None.

### claim-format-rollback-preserves-canonical-state

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the causality detail is versioned and a newer version
reads as `Unknown(UnsupportedVersion)`, but no rollback of a format epoch is
constructed.
Guarantee: Reading a record written under a detail version this build does not
interpret never mutates canonical rows and never grants a class.
Check: `always` - `Unknown(UnsupportedVersion)` for a version other than
`CLAIM_CAUSALITY_DETAIL_VERSION`; the observation row is unchanged after the
read.
Fault/timing angle: a newer binary wrote records, an older binary reads them.
Required faults and enabling state: a stored record with a higher version.
Confidence: medium - [evidence](evidence/claim-format-rollback-preserves-canonical-state.md).
Verified the version branch of
`replay_is_effect_free_and_conflicting_or_unsupported_records_are_unknown` in
`crates/kernel/tests/kernel_claim_causality.rs`.
Existing check: `crates/kernel/tests/kernel_claim_causality.rs`; status unaudited.
Impact: a rollback that rewrites or misreads records changes canonical truth to
fit a projection.
Open questions:
- A format epoch for the canonical schema itself is a separately approved unit
  per KTD2; this record covers the detail version only.

### claim-replay-preserves-newest-canonical-state

Type: safety
Reachability: test-only
Status: active
Exercised: partial - receipt replay of a recording commit writes nothing; a
later record replaces the earlier one and the earlier snapshot keeps its class.
Projection replay belongs to the materialization ticket.
Guarantee: Replaying a causality commit is effect-free, and the class read at
any snapshot is the one live at that snapshot, never an older record revived.
Check: `always` - replay returns `replayed == true` with the original commit
sequence; the newest record is the only live one; older snapshots read their
own record.
Fault/timing angle: duplicate commit; older record after newer.
Required faults and enabling state: the same intent committed twice; two
records for one subject in two commits.
Confidence: medium - [evidence](evidence/claim-replay-preserves-newest-canonical-state.md).
Verified `replay_is_effect_free_and_conflicting_or_unsupported_records_are_unknown` in
`crates/kernel/tests/kernel_claim_causality.rs`.
Existing check: `crates/kernel/tests/kernel_claim_causality.rs`; status unaudited.
Impact: a replayed or reordered record resurrects a withdrawn class.
Open questions: None.

### echo-classification-requires-canonical-causality

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `forged_records_and_copied_strings_grant_nothing`,
`direct_observation_rests_on_live_exact_evidence`, and
`derived_reinjection_rests_on_exact_live_parents` in
`crates/kernel/tests/kernel_claim_causality.rs` construct forgeries, missing,
inexact, and rebound evidence, wrong digests, wrong revisions, missing and
mismatched parents, detail parents that disagree with the `derived_from` rows,
and record-shaped observations under other kinds and source kinds.
Guarantee: `DirectObservation` and `DerivedReinjection` are derived only from a
record the trusted path wrote whose named evidence or parents exist at the
snapshot; text, classification strings, roles, and producer strings in any
observation grant no class.
Check: `always` - every forgery path yields `Unknown`; the generic writers
refuse the kind and its id namespaces with `InvalidInput`.
Fault/timing angle: evidence retired or parent invalidated after the record.
Required faults and enabling state: a record whose evidence is retired; a
record whose parent is retired; a look-alike observation.
Confidence: high - [evidence](evidence/echo-classification-requires-canonical-causality.md).
Existing check: `crates/kernel/tests/kernel_claim_causality.rs`; status unaudited.
Impact: text equality or a copied producer string would let reinjected content
pose as a genuine observation.
Open questions: None.

### malformed-required-field-stops-projection-progress

Type: safety
Reachability: test-only
Status: active
Exercised: partial - a stored admission enum or sensitivity class this build
cannot interpret makes `claim_facts_as_of` fail with `MalformedRequiredField`
for the whole request, and a decision row that disagrees with its registry row
fails with `CorruptCanonicalRow`; checkpoint quarantine belongs to the
materialization ticket.
Guarantee: A required claim field that does not decode is an explicit error,
never a default that implies approval, and never a silently skipped claim.
Check: `always` - `Err(MalformedRequiredField)` when any required enum or
sensitivity column of a returned admission row is unrecognized, or when the
registry class of a returned decision is unrecognized; `Err(CorruptCanonicalRow)`
when the decision row's stored class or creation commit differs from the
registry row's.
Fault/timing angle: none.
Required faults and enabling state: corrupted `maturity`, `disposition`, and
`sensitivity_class` columns; a decision row whose class differs from its
registry row; a registry row whose class no build reads.
Confidence: medium - [evidence](evidence/malformed-required-field-stops-projection-progress.md).
Verified `bounds_apply_before_decoding_and_malformed_required_fields_fail_explicitly`
and `a_registry_class_this_build_cannot_read_is_an_error_not_a_secret_default`.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; status unaudited.
Impact: a decode failure read as a default admits or hides a claim by accident.
Open questions: None.

### occurrence-identity-is-not-payload-or-source-triple

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the inventory keys each representation by the kernel's
encoded whole-buffer occurrence tuple, lists absent representations as
exclusions, keeps equal bytes under `canonical_claims` and `promoted_memory`
as two occurrences, and reports a partial-span publication as `NoDescriptor`;
existing `source_identity` tests cover tuple inequality.
Guarantee: The occurrence inventory of a claim has one entry per published
whole-buffer (class, representation) at the claim's revision, each identified
by the encoded tuple, and a representation without a whole-buffer descriptor
is an explicit exclusion rather than a missing entry. Every id the entry
reports equals the joined registry, observation, and evidence column it was
judged live by. Partial-span descriptors are outside the inventory: the span
is hashed into the lineage id (`source_identity.rs` `finish`), so the reader
cannot derive their object ids from the claim, and the kernel's own claim
producer (`crates/daemon/src/claim_sources.rs`) publishes `span: None` only.
Check: `always` - occurrence ids equal the descriptor outcomes the test wrote;
the exclusion list names every representation without a whole-buffer
descriptor; a stored detail whose `lineage_id`, `evidence_id`,
`artifact_digest`, or `payload_id` differs from the joined columns, or a
registry `source_kind`, `source_revision`, or observation `sensitivity_class`
that disagrees with the row it was admitted with, fails the request with
`CorruptCanonicalRow`.
Fault/timing angle: none.
Required faults and enabling state: a claim with two of three representations
published under two classes with equal bytes; a descriptor detail rewritten
out of band to name other rows; a guarded column rewritten out of band; a
partial-span publication of a claim representation.
Confidence: medium - [evidence](evidence/occurrence-identity-is-not-payload-or-source-triple.md).
Verified `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot`,
`occurrence_facts_refuse_a_detail_that_disagrees_with_its_guarded_rows`, and
`a_partial_span_publication_is_outside_the_whole_buffer_inventory`.
Existing check: `crates/kernel/tests/kernel_source_descriptors.rs`, `crates/kernel/tests/kernel_claim_facts.rs`; status unaudited.
Impact: collapsing occurrences by payload or source triple merges independent
observations into one.
Open questions:
- A claim-scoped inventory of partial-span descriptors needs an identity index
  the kernel does not have; the export's class-wide page is the only reader of
  them today (needs human input if a producer starts publishing spans).

### projection-has-no-second-truth-or-policy-authority

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the reader copies stored rows and derives visibility
through the serving query itself; the `Retired memory-plane identifiers` CI
step in `.github/workflows/ci.yml` rejects the deleted machinery's names in
production content.
Guarantee: The facts reader evaluates no admission policy and restates no
selection rule: the own and lineage rows are chosen by the serving view's own
SQL (`served_own_decision_sql`, `served_lineage_decision_sql`), their fields
are copied, served visibility comes from `admission::served_classes`, and the
occurrence inventory selects descriptors through `Descriptors::LiveAtEnd`, the
export's own liveness predicate.
Check: `always` - own and lineage admission fields equal the writer's
`AdmissionDecision`; served visibility equals `visible_as_of` on every surface
at the same snapshot; the served standing distinguishes a retired object from
a never-admitted one.
Fault/timing angle: none.
Required faults and enabling state: none.
Confidence: medium - [evidence](evidence/projection-has-no-second-truth-or-policy-authority.md).
Verified `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot`,
`a_lineage_admission_binds_every_object_on_the_lineage`, and
`corrected_claims_report_succession_and_serving_standing_at_the_snapshot`.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; `.github/workflows/ci.yml` `Retired memory-plane identifiers` step for A4; status unaudited.
Impact: a second evaluator drifts from the kernel and grants what it denies.
Open questions:
- The A4 identifier gate is a fixed token list in the workflow; whether that
  list covers every deleted claim-machinery name is unaudited.

### retrieved-content-cannot-upgrade-write-authority

Type: safety
Reachability: test-only
Status: active
Exercised: partial - record content grants no class and generic writers cannot
create or correct a record; authority at retrieval and delivery belongs to the
delivery ticket.
Guarantee: No content stored in or retrieved from a claim, observation, or
causality record changes what a writer may do.
Check: `always` - `insert_observation` and `correct_observation` refuse the
causality kind and namespaces; a look-alike record changes no class.
Fault/timing angle: none.
Required faults and enabling state: a forged observation naming the causality
kind, id prefix, or object prefix.
Confidence: medium - [evidence](evidence/retrieved-content-cannot-upgrade-write-authority.md).
Verified `forged_records_and_copied_strings_grant_nothing` in
`crates/kernel/tests/kernel_claim_causality.rs`.
Existing check: `crates/kernel/tests/kernel_claim_causality.rs`; status unaudited.
Impact: poisoned content escalates into write or edit authority.
Open questions: None.

### revision-domains-remain-distinct-and-supported

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the source revision (`object.source_revision`), each
admission row's `policy_revision`, and the snapshot (`known_as_of`) are
separate fields; an unsupported causality detail version reads as `Unknown`;
the serving query's fail-closed policy revision check is existing behavior.
Guarantee: Source revision, policy revision, snapshot commit, and causality
detail version are separate fields, and a version this build does not support
fails closed for its own domain only.
Check: `always` - the fields hold their independent values; a higher detail
version yields `Unknown(UnsupportedVersion)` while the other facts are
returned.
Fault/timing angle: none.
Required faults and enabling state: a record with `causality_version` 2.
Confidence: medium - [evidence](evidence/revision-domains-remain-distinct-and-supported.md).
Verified `claim_facts_copy_stored_values_and_stay_bound_to_their_snapshot` and
the version branch of the replay test.
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; `crates/kernel/src/admission.rs` `decided_row` for policy revision; status unaudited.
Impact: conflating revisions lets a policy change masquerade as a source
change or the reverse.
Open questions: None.

### served-sensitivity-and-artifact-policy-govern-egress

Type: safety
Reachability: test-only
Status: active
Exercised: partial - served sensitivity and per-surface visibility are copied
from the serving query; artifact egress policy is judged by `kernel::eligibility`
and belongs to the delivery ticket.
Guarantee: The served sensitivity and visibility the reader reports equal the
serving route's answer for the same object and snapshot.
Check: `always` - each `ServedFacts` surface equals the row's visibility in
`visible_as_of` for that surface, `Hidden` when the route lists no row, and
`sensitivity` equals the folded served sensitivity.
Fault/timing angle: none.
Required faults and enabling state: none.
Confidence: medium - [evidence](evidence/served-sensitivity-and-artifact-policy-govern-egress.md).
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; status unaudited.
Impact: a reader reporting looser sensitivity than serving leaks to a surface
the kernel denies.
Open questions: None.

### supporting-authority-is-preserved-not-recomputed

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `supporting_approval_is_copied_with_its_validity_at_the_snapshot`
admits under a valid approval, revokes it, and compares both snapshots with
the writer's reported decisions; an out-of-band row naming the revoked
approval reads `valid_at_snapshot == false`.
Guarantee: The supporting approval the reader reports is the one the admission
row stored, with its validity taken from the authority chain at the snapshot;
the reader never chooses between cited and supporting approval itself.
Check: `always` - `supporting_approval.object_id` equals the stored
`approval_object_id`; `valid_at_snapshot` equals
`approval_chain_valid_at_snapshot_sql` for that row.
Fault/timing angle: approval revoked after the admission and before the
snapshot.
Required faults and enabling state: an admission naming an approval that is
later revoked.
Confidence: medium - [evidence](evidence/supporting-authority-is-preserved-not-recomputed.md).
Existing check: `crates/kernel/tests/kernel_claim_facts.rs`; `crates/kernel/tests/kernel_admission.rs` for approval chains; status unaudited.
Impact: recomputing support from a cited approval grants maturity the kernel
withdrew.
Open questions: None.

### claim-tombstone-masks-all-representations

Type: safety
Reachability: test-only
Status: active
Exercised: partial - correction and retirement tombstone every representation
of the predecessor in the shared projection, and a projection that has not
caught up still classifies those rows as Superseded or Retracted from
canonical facts; the vector side reuses RP2.1 obsoletion, whose existing
checks are listed below.
Guarantee: Once a claim decision is corrected or retired, no representation of
it under either class remains a Current candidate: caught-up rows are
tombstoned and lagging rows classify as Superseded or Retracted.
Check: `always(!current)` - after correction or retirement, every occurrence of
the predecessor is either tombstoned or classified as Superseded or Retracted
by `classify`, whichever projection state is read.
Fault/timing angle: the window between a kernel correction and the projection's catch-up.
Required faults and enabling state: a corrected and a retired decision with three and two representations; a projection frozen before catch-up.
Confidence: medium - [evidence](evidence/claim-tombstone-masks-all-representations.md).
Existing check: `crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees` and `correction_retirement_and_replay_cannot_resurrect_stale_rows`; `crates/daemon/tests/embedding_publication.rs` - `the_projection_itself_obsoletes_tombstoned_or_replaced_inputs`; status unaudited.
Impact: a lexical or vector candidate of a retired claim keeps ranking after the kernel withdrew it.
Open questions: None.

### claim-rebuild-incremental-parity

Type: safety
Reachability: test-only
Status: active
Exercised: partial - a projection built from a fresh snapshot after
correction, retirement, quarantine, and a causality record classifies every
live claim occurrence identically to the incrementally caught-up projection,
with the same live row count; tombstones and job rows are compared by the
RP2.1 checks listed below, not by the claim test.
Guarantee: A projection rebuilt from a fresh export snapshot yields the same
classified claim candidates as the incremental projection at the same kernel
tip.
Check: `always` - the `(occurrence_id, state)` map from `classify_live_claims`
is equal for the rebuilt and the incremental projection, and the live row
counts match.
Fault/timing angle: none.
Required faults and enabling state: an incremental projection that has applied a catch-up window and a fresh bootstrap at a later snapshot.
Confidence: medium - [evidence](evidence/claim-rebuild-incremental-parity.md).
Existing check: `crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees`; `crates/retrieval/tests/lexical_projection.rs` (rebuild from one snapshot equals incremental) and `crates/daemon/tests/search_replacement/recovery.rs` for the RP2.1 rebuild path; status unaudited.
Impact: a rebuild that silently classifies a claim differently from the projection it replaces.
Open questions:
- Which tombstone and job identities must a claim-specific parity check compare beyond live rows? (unresolved, needs the normalization rule the specification leaves to the RP2.1 owner)

### unknown-echo-state-is-policy-neutral

Type: safety
Reachability: test-only
Status: active
Exercised: yes - `causal_class_changes_no_state` runs every state-relevant
fact combination under `Unknown`, `DirectObservation`, and
`DerivedReinjection` and asserts equal states; `classify` takes no causal
class by signature; the daemon test classifies the caught-up projection,
records a `DirectObservation` on the successor, classifies again, and asserts
the two maps are equal.
Guarantee: The causal class of a claim never changes its candidate state, and
no field of a candidate carries a score, boost, corroboration count, or
suppression flag derived from it.
Check: `always` - for fixed canonical facts, `classify` returns one state
whatever `causality` holds; `ClaimCandidate` has no ranking field.
Fault/timing angle: none.
Required faults and enabling state: claims with equal facts and different causal classes.
Confidence: high - [evidence](evidence/unknown-echo-state-is-policy-neutral.md).
Existing check: `crates/retrieval/tests/claims.rs` - `causal_class_changes_no_state`; `crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees`; status unaudited.
Impact: Unknown lineage would earn or lose standing it has no evidence for.
Open questions: None.

### eligible-positive-and-unknown-claims-remain-reachable

Type: reachability
Reachability: test-only
Status: active
Exercised: partial - an admitted, served claim with no causality record
classifies Current alongside one with a `DirectObservation` record, so both
reach the candidate set; delivery through a harness belongs to the delivery
ticket.
Guarantee: An otherwise eligible claim whose lineage is Unknown is a Current
candidate exactly as a genuine one is.
Check: `reachable` - `classify_live_claims` returns Current for a served claim
with `Unknown(NoRecord)` causality.
Fault/timing angle: none.
Required faults and enabling state: a served claim with no causality record.
Confidence: medium - [evidence](evidence/eligible-positive-and-unknown-claims-remain-reachable.md).
Existing check: `crates/daemon/tests/claim_sources.rs` - `lagging_projection_classifies_claims_from_canonical_facts_and_rebuild_agrees` (the quiet and anti claims before their transitions); `crates/retrieval/tests/claims.rs`; status unaudited.
Impact: an all-Unknown corpus would deliver nothing, failing the useful-path requirement.
Open questions: None.

### claim-local-commit-before-ack

Type: safety
Reachability: test-only
Status: active
Exercised: partial - claim rows travel through the RP2.1 shared catch-up,
whose existing checks release the local transaction before acknowledgement;
no claim-specific crash cut was added.
Guarantee: Claim rows, tombstones, checkpoint, and pending work are committed
locally and the transaction released before the kernel consumer is
acknowledged.
Check: `always` - the `EpisodeEvent` sequence shows `LocalReleased` before every
acknowledgement, as the RP2.1 harness asserts.
Fault/timing angle: the window between local commit and acknowledgement.
Required faults and enabling state: a process kill between the two.
Confidence: medium - [evidence](evidence/claim-local-commit-before-ack.md).
Existing check: `crates/daemon/tests/search_catchup.rs` - `crash_cuts_recover_to_the_ledger_after_two_reopens_and_never_acknowledge_early`, `released_before_every_acknowledgement`; status unaudited.
Impact: an acknowledgement ahead of durable local progress loses rows on restart.
Open questions: None.

### claim-consumer-replay-includes-published-history

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the claim materializer replays from receipts after lost
and skipped acknowledgements; the shared catch-up's reconciliation of a lost
acknowledgement is an RP2.1 check.
Guarantee: Replaying the claim consumer after a lost or skipped acknowledgement
re-drives already-published commits from receipts and moves the inventory by
nothing.
Check: `always` - after a lost or skipped acknowledgement, the next episode
reaches the target and the kernel inventory equals the pre-fault inventory.
Fault/timing angle: a lost acknowledgement reply; a page published but never acknowledged.
Required faults and enabling state: the `EpisodeFault` variants of the materializer.
Confidence: medium - [evidence](evidence/claim-consumer-replay-includes-published-history.md).
Existing check: `crates/daemon/tests/claim_sources.rs` - `lost_and_skipped_acknowledgements_replay_from_receipts`, `unresolved_acknowledgement_blocks_and_the_next_episode_recovers`; `crates/daemon/tests/search_catchup.rs` - `lost_ack_with_cancelled_reconciliation_keeps_unknown_outcome_and_local_prefix`; status unaudited.
Impact: a replay that skips already-published history leaves a projection missing rows it acknowledged.
Open questions: None.

### claim-export-retention-fence

Type: safety
Reachability: test-only
Status: active
Exercised: partial - the claim tests capture a source hold before every
export and extend it through catch-up; the fence itself is an RP2.1 mechanism
with its own checks.
Guarantee: Every claim export window runs under a source hold registered before
the snapshot, so the evidence behind exported descriptors cannot be pruned
under the reader.
Check: `always` - `export_source_page` refuses a window whose hold is missing,
released, or expired.
Fault/timing angle: pruning between hold capture and export.
Required faults and enabling state: a released or expired hold.
Confidence: medium - [evidence](evidence/claim-export-retention-fence.md).
Existing check: `crates/kernel/tests/kernel_source_holds.rs`, `crates/kernel/tests/kernel_source_export.rs`; status unaudited.
Impact: a rebuild reads descriptors whose bytes are gone and publishes a partial replacement.
Open questions: None.

### claim-worker-result-cannot-outlive-identity

Type: safety
Reachability: test-only
Status: active
Exercised: partial - embedding work for claim occurrences is queued and
obsoleted by the shared batch and guarded publication paths; their checks are
RP2.1's.
Guarantee: An embedding result for a claim occurrence that was tombstoned or
whose input changed is obsoleted, never published against the new identity.
Check: `always` - `complete_embedding_observed` marks the job obsolete when the
occurrence is tombstoned; `guard_current_input` refuses a stale pre-read.
Fault/timing angle: a tombstone landing between dispatch and publication.
Required faults and enabling state: a job dispatched before a correction.
Confidence: medium - [evidence](evidence/claim-worker-result-cannot-outlive-identity.md).
Existing check: `crates/daemon/tests/embedding_publication.rs` - `canonical_mutations_wait_behind_the_guard_and_stale_inputs_become_obsolete`, `the_projection_itself_obsoletes_tombstoned_or_replaced_inputs`; status unaudited.
Impact: a vector for a retired revision serves as if current.
Open questions: None.

### claim-cancellation-preserves-durable-work

Type: safety
Reachability: test-only
Status: active
Exercised: not yet at the claim level - the shared catch-up's cancellation
checks cover the mechanism; request-level cancellation belongs to the delivery
ticket.
Guarantee: Cancelling a claim episode at any write boundary neither
acknowledges unapplied work nor loses work already committed locally.
Check: `always` - after cancellation the local prefix and the kernel checkpoint
are unchanged from the last completed boundary.
Fault/timing angle: cancellation at each `EpisodeEvent` boundary.
Required faults and enabling state: a cancelled episode at every boundary.
Confidence: medium - [evidence](evidence/claim-cancellation-preserves-durable-work.md).
Existing check: `crates/daemon/tests/search_catchup.rs` - `cancellation_at_write_boundaries_never_quarantines_or_acknowledges`, `cancellation_in_the_second_window_preserves_the_first_acknowledged_prefix`; status unaudited.
Impact: a cancelled request drops durable rows or acknowledges work it never applied.
Open questions: None.

### claim-disable-preserves-consumer-contract

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no claim-specific disable path exists; the RP2.1
lifecycle tests cover deregistration and pending-consumer refusal.
Guarantee: Disabling claim projection never acknowledges unapplied work to
release retention; a lagging consumer is refused normal deregistration.
Check: `always` - `deregister_outbox_consumer` returns `ConsumerPending` while
the checkpoint lags the tip.
Fault/timing angle: none.
Required faults and enabling state: a lagging consumer at disable time.
Confidence: low - [evidence](evidence/claim-disable-preserves-consumer-contract.md).
Existing check: `crates/kernel/tests/kernel_outbox.rs`; `crates/daemon/tests/search_replacement/recovery.rs` - `explicit_recovery_bootstraps_deregistered_and_pending_disabled_consumers`; status unaudited.
Impact: disabling releases retention for history a consumer never applied.
Open questions:
- What is the legal pending-consumer transition for the default-deregister plan? (needs human input)

### claim-recovery-converges-within-approved-bound

Type: liveness
Reachability: test-only
Status: active
Exercised: not yet - RP2.9 has approved no recovery window for claims; the
generic recovery harness exists and is listed below.
Guarantee: After a fault-free window of approved length, a claim projection
recovering from a crash, corruption, or incompatibility reaches the kernel
tip with canonical authority intact.
Check: `always` - within the approved bound, the projection checkpoint equals
the kernel tip and the classified candidates equal the oracle.
Fault/timing angle: recovery after each crash cut.
Required faults and enabling state: an approved RP2.9 recovery bound; a crash at each boundary.
Confidence: low - [evidence](evidence/claim-recovery-converges-within-approved-bound.md).
Existing check: `crates/daemon/tests/search_replacement/recovery.rs`, `crates/daemon/tests/search_catchup.rs` process-crash harness; status unaudited.
Impact: recovery that never converges leaves stale candidates authoritative.
Open questions:
- Which numeric recovery bound does RP2.9 approve for claim projections? (needs human input)

## Relationship map

- `echo-classification-requires-canonical-causality` is the load-bearing record;
  `retrieved-content-cannot-upgrade-write-authority` and
  `canonical-provenance-survives-approved-write-read-path` are its two sides:
  nothing but the trusted path writes a record, and a written record reads back
  intact.
- `canonical-claim-fields-match-fenced-source`,
  `supporting-authority-is-preserved-not-recomputed`, and
  `projection-has-no-second-truth-or-policy-authority` share one mechanism: the
  reader copies rows and reuses the serving query.
- `claim-export-predecode-bounds` and `claim-capacity-failure-is-atomic` share
  the bound-before-read discipline.
- `claim-tombstone-masks-all-representations`,
  `claim-rebuild-incremental-parity`, and
  `unknown-echo-state-is-policy-neutral` share one mechanism: the projection
  stores candidates and `retrieval::claims::classify` derives state from the
  kernel's facts each time, so lag, rebuild, and lineage cannot change what a
  claim is allowed to be.
