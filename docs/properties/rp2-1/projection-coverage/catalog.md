# RP2.1 projection and source-coverage properties

## Scope and provenance

System: `/local/home/ahrav/scratch/eidnara`.
HEAD: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
Date: 2026-09-10. Method: `../../METHOD.md` and
`property-discovery-and-catalog`.

The user supplied the settled [RP2.1 plan][plan] plus its linked
[index][index], [parent][parent] and [N1][n1]. No incidents were supplied.
The [source register](_lenses/model.md#source-register) records why each was
consulted and hashes the external working-tree documents. Repository citations
refer to the HEAD above. Documented guarantees remain claims under test.
No runtime result, implementation readiness or approval is inferred from them.

This part owns projection rows, local checkpoint and durable pending work;
occurrence/payload identity; revision and tombstone replay; source inventory;
raw tool fidelity; disable gates; shared eligibility; and coverage reporting.
Export/fenced rebuild and embedding/tokenizer/JobTable are separate owners.
Their interfaces are dependencies here, not additional specifications.

All model and property lenses ran, with distributed coordination marked not
applicable and both wildcards last. Retained notes: [model](_lenses/model.md),
[properties](_lenses/properties.md), [wildcard](_lenses/wildcard.md).
Subagent dispatch failed at the harness depth limit; passes were sequential.
Existing overlap was assessed before additions in
[existing-checks.md](existing-checks.md). The user supplied the central fresh
independent evaluation by analyst `ses_f7623dcccffe3Y09nW2wVoABif`.
[portfolio-evaluation.md](portfolio-evaluation.md) records its findings,
qualifications and dispositions. These edits are not another independent review.

## Reachability and observation contract

Each record explains its own reachability in its evidence file. `test-only`
on a proposed record means a future test must construct it; it does not mean
an executable test exists. The RP2.9 evidence gate is also proposed. Live
absent-route/configuration rejection is adjacent behavior, not an exercise of
that gate.

Oracle notation is descriptive, not a proposed public API:

- `c` is a complete canonical commit sequence in one source incarnation.
  `C` is the durable local checkpoint. The export/recovery owner's
  [acknowledgement record][ack-owner] owns the kernel/local progress relation.
- `E(c,p)` is an independently specified expected occurrence inventory at
  `c` under declared projection policy `p`, including deletion facts.
- An occurrence key contains source class, namespaced source identity,
  canonical revision, representation and span. A payload key names checked
  bytes. Payload sharing never changes occurrence multiplicity.
- `D(o,m)` is a vector-validity decision supplied by the embedding owner for
  occurrence `o` and active model identity `m`. It is not "a vector exists".
- `M` is an approved, independently specified bounded mapping from canonical
  fields to projection/embedding input bytes. Current-input hashes are computed
  from fresh canonical inputs through `M`, not copied from derived rows.
- Durable observations come from one local read snapshot after commit/reopen.
  An unobserved COMMIT response is an unknown outcome, not a rollback witness.

## Index

| Slug | Type | Reachability | Semantics | Status | Confidence |
| --- | --- | --- | --- | --- | --- |
| [projection-commit-checkpoint-pending-atomic](#projection-commit-checkpoint-pending-atomic) | safety | test-only | always | active | medium |
| [projection-replay-does-not-resurrect](#projection-replay-does-not-resurrect) | safety | test-only | always | active | medium |
| [projection-occurrence-payload-separation](#projection-occurrence-payload-separation) | safety | test-only | always | active | medium |
| [projection-source-inventory-complete](#projection-source-inventory-complete) | safety | test-only | always | active | medium |
| [projection-raw-tools-exact-and-lexical](#projection-raw-tools-exact-and-lexical) | safety | test-only | always | active | medium |
| [projection-n13-hooks-stay-gated](#projection-n13-hooks-stay-gated) | safety | test-only | always | active | medium |
| [projection-canonical-eligibility-authority](#projection-canonical-eligibility-authority) | safety | test-only | always | active | medium |
| [projection-lexical-dense-coverage-distinct](#projection-lexical-dense-coverage-distinct) | safety | test-only | always | active | medium |
| [projection-bounded-admission-preserves-progress](#projection-bounded-admission-preserves-progress) | safety | test-only | always | active | medium |
| [projection-remediation-invalidates-derived-bytes](#projection-remediation-invalidates-derived-bytes) | safety | test-only | always | active | medium |
| [projection-acceptance-situations-witnessed](#projection-acceptance-situations-witnessed) | reachability | test-only | sometimes | active | medium |

## Records

### projection-commit-checkpoint-pending-atomic

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the local projection transaction and crash hooks do not exist.
Guarantee: Projection rows, local checkpoint and required pending embedding work
commit atomically as one complete local state.
Check: `always` - each recovered local state equals a complete-commit prefix of
the independent oracle, including every required pending identity not validly
completed or made obsolete; rows, `C` and outstanding work never describe
different prefixes, because local transaction atomicity applies to every
durable observation.
Fault/timing angle: Before/after search COMMIT, lost local COMMIT response and
reopen. Postcommit interruption witnesses are reused from the ack owner.
Required faults and enabling state: An established bootstrap baseline, a
multirow commit with dense-required work, actual termination at a declared
local boundary and reopen with independently known complete source prefixes.
Confidence: medium - [evidence](evidence/projection-commit-checkpoint-pending-atomic.md).
The plan establishes the obligation; kernel acknowledgement exists but no local
transaction implements it.
Existing check: none for local search atomicity. The kill/barrier/reopen pattern
in `crates/kernel/tests/cas_fault_injection.rs:924-990,1046-1094` is reusable
substrate, status unaudited, not an RP2.1 product hook.
Impact: Acknowledgement can release replay history while rows or required
vector work are absent.
Open questions:

- How are local baseline and source incarnation paired with the export owner's complete-prefix input? (needs human input)

### projection-replay-does-not-resurrect

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no local occurrence mutation or pending-job deduplication
path exists.
Guarantee: Replaying a committed prefix preserves current revisions and
tombstones without duplicating durable work.
Check: `always` - after each replay, the logical row/tombstone map and outstanding
job identities equal single application of the complete source prefix, with one
active row per current occurrence tuple and at most one outstanding row per
required job key; replaying an older prefix after a newer revision or deletion
cannot reactivate it or lower `C`, because replay is observable on every retry.
Fault/timing angle: Commit succeeds but its response or acknowledgement is lost;
an older prefix is retried after a newer one.
Required faults and enabling state: Insert, revise and delete one source; share
its payload with another occurrence; lose one response, reopen and replay both
identical and overlapping prefixes.
Confidence: medium - [evidence](evidence/projection-replay-does-not-resurrect.md).
Canonical receipt and checkpoint mechanisms were inspected; local replay is
proposed.
Existing check: `crates/kernel/src/envelope.rs:1043-1074` is a canonical receipt
guard, status unaudited; none for search replay.
Impact: Deleted evidence can reappear, current data can regress, or retries can
amplify embedding work.
Open questions:

- What mutation/job key distinguishes divergent replay from authorized canonical byte changes under the same occurrence tuple? (needs human input)

### projection-occurrence-payload-separation

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - occurrence and payload identity functions and a collision
seam are absent.
Guarantee: Deterministic occurrence identities preserve distinct source
occurrences while payload sharing requires exact byte equality.
Check: `always` - the same complete occurrence tuple yields the same ID across
order and restart; distinct tuples never alias; every shared payload ID resolves
to equal bytes, and a forced digest collision with unequal bytes cannot merge
occurrences or return the wrong payload, because identity applies to every
admitted row.
Fault/timing angle: Replay, namespace overlap, equal payloads at distinct
revisions/spans, and digest collision.
Required faults and enabling state: Equal bytes in different source classes and
host records, two representations and overlapping spans, a same-rendered-text
revision, and injected equal digests for unequal byte buffers.
Confidence: medium - [evidence](evidence/projection-occurrence-payload-separation.md).
RP2.1 KTD4 is explicit; existing codec fingerprints serve a different purpose.
Existing check: none for retrieval identity; codec alignment checks remain in
the linked existing catalog, status unaudited.
Impact: Deduplication can erase provenance, merge unrelated evidence or attach a
vector to the wrong revision.
Open questions:

- Which canonical encoding, namespace and byte-span convention define the tuple? (needs human input)
- Does a payload collision fail admission or allocate a disambiguated key? Either must preserve exact bytes. (needs human input)

### projection-source-inventory-complete

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - durable message/git adapters and the five-class projection
inventory are absent.
Guarantee: Every declared complete projection represents the policy-selected
source occurrences and deletion facts for every required class at its checkpoint.
Check: `always` - whenever coverage at `c` is declared complete, the observed
per-class occurrence/revision/representation/span/tombstone inventory equals
`E(c,p)` with matching multiplicities, including empty classes and explicit
selection exclusions; a missing class cannot be hidden by equal aggregate
totals, because completeness is a claim made at each publication.
Fault/timing angle: Revision or deletion during ingestion, class-specific
adapter omission, and restart between source batches.
Required faults and enabling state: Messages, canonical claims, promoted memory,
git commits and selected tool spans, with at least one admitted item and a later
revision or deletion in each class; both harness inputs where applicable.
Confidence: medium - [evidence](evidence/projection-source-inventory-complete.md).
Source obligations and the reader at
`crates/daemon/src/canonical_memory.rs:141-212` were inspected; the test at
`:282-312` checks its positive-category filtering, not a full projection oracle.
Existing check: codec golden tests and
`crates/daemon/src/canonical_memory.rs:282-312`,
`injectable_rows_are_visible_decisions_in_a_positive_category`, are adjacent
checks, status unaudited; none declares five-class durable coverage complete.
Impact: A retrieval lane can appear healthy while an entire class or current
revision is unsearchable.
Open questions:

- What are the canonical source mappings, promoted-memory boundary and selected-tool-span policy? (needs human input)
- Which approved source envelope and RP2.9 bound feed the export owner's [normal catch-up and authorized recovery contract][progress-owner]? (needs human input)

### projection-raw-tools-exact-and-lexical

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no durable raw-tool projection or source-byte capture
contract exists.
Guarantee: Raw tool output is retained exactly, selected spans reference its
exact bytes, and default tool projection creates lexical rows without embedding
jobs.
Check: `always` - for each accepted raw result, retained bytes equal the
independently captured source buffer, each selected `[lo,hi)` payload equals
that buffer slice, and default policy creates no dense job for any raw-tool
occurrence while the selected lexical inventory is present, because every
accepted raw result must retain fidelity and obey policy.
Fault/timing angle: Decoding, normalization, multiline/multipart selection,
local commit/reopen, and accidental default dense routing.
Required faults and enabling state: CRLF, whitespace, Unicode, escaped
JSON-looking text, error output, repeated spans and multipart results; observe
bytes before projection and jobs after commit.
Confidence: medium - [evidence](evidence/projection-raw-tools-exact-and-lexical.md).
The plan promises exact retention; current codecs expose structured values
rather than a retrieval byte contract.
Existing check: `crates/daemon/src/codec/mod.rs:59-94,184-219` compares JSON
values, status unaudited; none for stored raw buffers or lexical-only job absence.
Impact: Search evidence can differ from the original output or silently expand
dense cost and exposure.
Open questions:

- Which source boundary defines exact bytes for multipart/non-text results, and how does it compose with canonical admission/redaction? (needs human input)

### projection-n13-hooks-stay-gated

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - the RP2.9 evidence gate and its activation transitions are
proposed; live absent-route/configuration rejection is adjacent behavior only.
Guarantee: N1.3-dependent hooks execute only after their class coverage,
freshness, resource and both-harness capability gates are accepted.
Check: `always` - for every N1.3 hook and activation attempt, missing, failed or
unsupported/inapplicable acceptance evidence implies zero executions and no
durable work created by that attempt; unsupported adapters remain disabled,
because every attempted product activation must be gated.
Fault/timing angle: Startup, configuration reload, supervisor tick, and
invalidated or incomplete gate evidence.
Required faults and enabling state: Hostile user/project enable flags and each
N1.3 entry point; later fixtures vary one gate at a time and include an
unsupported adapter.
Confidence: medium - [evidence](evidence/projection-n13-hooks-stay-gated.md).
Production routing/configuration reject absent paths; no product path interprets
the proposed RP2.9 evidence gate, so that obligation has no production reachability.
Existing check: `crates/daemon/src/config.rs:1881-1929` and
`crates/daemon/src/lib.rs:32439-32479`, status unaudited.
These checks assert absent entry points, not evidence-gate decisions.
Impact: Product hooks can run against missing durable sources, stale coverage
or unsupported harness behavior.
Open questions:

- Who owns the frozen hook-to-gate inventory and evidence invalidation rules at startup/reload? (needs human input)

### projection-canonical-eligibility-authority

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - shared kernel eligibility policy and the retrieval adapter
do not exist.
Guarantee: Projection consumers use the same canonical eligibility authority as
daemon validation and cannot grant eligibility from stale projection metadata.
Check: `always` - both adapter call paths reach the same kernel policy owner
and, for identical candidate identities, destination, project scope and canonical
snapshot, return identical verdicts; an ineligible current canonical candidate
is never admitted solely by a projected grant, because derived state cannot
authorize access.
Fault/timing angle: Retirement, supersession, revision, scope or sensitivity
changes after projection and before validation; cached and uncached reads.
Required faults and enabling state: At least one eligible and one ineligible
candidate, a stale projected grant, identical frozen canonical facts for adapter
comparison, and a later canonical change for revalidation.
Confidence: medium - [evidence](evidence/projection-canonical-eligibility-authority.md).
The live daemon policy and cache inputs are verified; the shared-owner move is
proposed.
Existing check: `crates/daemon/tests/stage1_eligibility.rs:87-224` and
`crates/daemon/src/kernel_routes/eligibility.rs:536-629`, status unaudited;
none compares a retrieval adapter.
Impact: Retired, foreign-scope, hidden or provider-sensitive evidence can be
treated as authorized.
Open questions:

- What shared batch contract preserves snapshot identity and wire verdict ordering when policy moves into kernel? (needs human input)

### projection-lexical-dense-coverage-distinct

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no RP2.1 per-occurrence lexical/dense coverage report exists.
Guarantee: Coverage reports distinguish lexical presence from vectors valid for
the current source revision and model, with source-class policy exclusions
explicit.
Check: `always` - per class, reported lexical IDs equal current indexed
occurrences and dense-covered IDs V equal only occurrences satisfying `D(o,m)`.
For the current dense-required set R, missing-dense IDs M equal R minus V.
The pending-coverage set P is exactly members of M backed by current,
non-obsolete durable pending work; M minus P is missing without pending work.
A valid vector awaiting job bookkeeping is not in M or P; raw pending-job
capacity is a separate count. Stale vectors, pending jobs alone and
policy-excluded raw tools never count as valid dense coverage. All sets use
one checkpoint, policy and model observation, so every report has the same
denominator and identity boundary. Where an
approved mapping uses a remediated field, current validity also consumes the
[canonical byte/input-hash comparison](#projection-remediation-invalidates-derived-bytes).
Fault/timing angle: Source revision, model identity change, missing exact-token
preflight, pending work, and tombstone application.
Required faults and enabling state: A class with full lexical coverage but
absent/stale vectors, another with valid vectors, a revised same-text row, a
deleted row and a lexical-only raw tool.
Confidence: medium - [evidence](evidence/projection-lexical-dense-coverage-distinct.md).
The plan explicitly separates the reports; implementation and reporting schema
are absent.
Existing check: none for RP2.1 coverage; canonical withheld-read reporting is
reused by link, not treated as dense coverage evidence.
Impact: False completeness can enable a feature whose dense retrieval misses
current evidence.
Open questions:

- What report identity binds checkpoint, source policy and active model, and how are unknown inventories and zero denominators represented? (needs human input)

### projection-bounded-admission-preserves-progress

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - local transaction and durable-pending capacity admission
are absent.
Guarantee: Projection admission obeys approved local limits without reporting
local progress, skipping or discarding work that does not fit.
Check: `always` - each admitted local batch stays within approved
batch/transaction-byte/pending-count limits; a next complete commit refused
before COMMIT leaves its rows, checkpoint and required work unapplied, with an
explicit blocked result, because capacity exhaustion must not falsify local
progress.
Fault/timing angle: A commit exceeds a batch or byte cap; pending work reaches
capacity; configuration has no approved limits.
Required faults and enabling state: Approved small test limits, a full pending
set, a multirow commit crossing a cap and a single oversized payload;
observation before allocation/admission and after refusal.
Confidence: medium - [evidence](evidence/projection-bounded-admission-preserves-progress.md).
Plan bounds are explicit; existing outbox row limits do not establish local
byte or job bounds.
Existing check: `crates/kernel/tests/kernel_outbox.rs:622-698` checks row limits
and a mid-commit cut, status unaudited; none for local byte/pending limits.
Impact: A backfill can exhaust memory/disk or drop required work while reporting
progress.
Open questions:

- Which units and approval artifact define local transaction bytes and pending capacity, and what explicit response represents an oversized complete commit? (needs human input)

### projection-remediation-invalidates-derived-bytes

Type: safety
Reachability: test-only
Status: active
Exercised: not yet - no approved mapping from remediated canonical fields to
RP2.1 input bytes or production projection consumer exists.
Guarantee: If approved projection or embedding input uses a remediated canonical
field, catch-up and rebuild respect the changed bytes even when the occurrence
tuple is unchanged, and replay cannot make stale derived bytes current again.
Check: `always` - for each affected occurrence selected by independent bounded
mapping `M`, after canonical remediation at `r`, every claim of current coverage
through `r` or later must use payload bytes equal to `M` of current canonical
inputs and an input hash equal to the independently recomputed current hash;
a vector is current only if its input hash also matches and `D(o,m)` holds.
Replaying pre-remediation work must not restore its old payload or vector as
current. This is conditional authority safety, not optional waiver semantics.
Fault/timing angle: In-place canonical field remediation after projection or
dispatch, before catch-up/rebuild/currentness reporting, then old-work replay.
Required faults and enabling state: An approved bounded `M` that actually uses
`domains.name`, before/after canonical field bytes and source-revision witness,
changed mapped input bytes, retained old payload/vector and replay. Observe the
occurrence tuple rather than assuming it or every embedding-key field agrees.
Confidence: medium - [evidence](evidence/projection-remediation-invalidates-derived-bytes.md).
`crates/kernel/src/envelope.rs:395-438` rewrites only `domains.name` with an
`operator_remediation` change and no source-revision update; RP2.1 dependence on
that field is not established, and a current-input hash can detect the change.
Existing check: `crates/kernel/tests/kernel_retention.rs:395-466,743-814`
checks canonical name replacement and repeated remediation, status unaudited;
none checks derived RP2.1 input currentness.
Impact: A projection can report remediated source bytes or vectors as current
if it treats an unchanged occurrence tuple as proof of unchanged input.
Open questions:
- Which approved bounded source mapping includes remediated fields, and how does it independently reconstruct current projection/embedding input? (needs human input)
- What at-rest residue policy applies to old payloads, vectors, WAL and historical state? No new erasure SLA or permission is inferred. (needs human input)

### projection-acceptance-situations-witnessed

Type: reachability
Reachability: test-only
Status: active
Exercised: not yet - no approved three-surface campaign, complete witness matrix
or executable product observation hooks exist.
Guarantee: A completed approved RP2.1 campaign witnesses every required marker
and scenario dimension for its enabled scope across all three surface maps.
Check: `sometimes` - at campaign completion, the approved nonempty requirement
matrix `R` is frozen and every required `(marker, scenario_id)` has an independent
observed witness in that campaign, including all five source classes, a complete
declaration over nonempty independent inventory, certified inference offered
input, every declared actual crash boundary and the required `Current` recovery
states. Unknown, skipped or unfired required cells make the predicate false.
One witness for an arbitrary marker cannot pass this campaign-scoped situation
check; the aggregation marker itself is not a prerequisite cell in `R`.
Fault/timing angle: Campaign finalization after required normal, fault and
bounded recovery episodes; omitted scenarios otherwise permit vacuous success.
Required faults and enabling state: Approved limits and scope; the
[projection](fault-map.md), [export/recovery][export-faults] and
[embedding][embedding-faults] marker maps; a fixed finite scenario matrix and
external observations keyed by scenario identity. Required supported paths
cannot be replaced by disabled unsupported lanes.
Confidence: medium - [evidence](evidence/projection-acceptance-situations-witnessed.md).
This binding witness rule follows the independent evaluation and coordinator's
explicit decision; no completed campaign or new test is claimed.
Existing check: none for RP2.1 acceptance. The CAS descriptor/driver check at
`crates/kernel/tests/cas_fault_injection.rs:391-423,981-989` is adjacent
unaudited evidence that declared boundaries can be checked against driven ones.
Impact: Safety checks can pass without exercising required source classes,
production inference, crash windows or successful bounded recovery.
Open questions:
- What approved campaign manifest freezes required marker/scenario cells and records their independent witnesses without post-run scope reduction? (needs human input)

## Relationships and reuse

Atomic commit protects replay state, but does not establish correct identity or
source mapping. Replay and identity together support inventory comparison;
neither dominates it. Exact tool bytes and lexical-only policy share source
fixtures with inventory but have different failure oracles. Coverage reporting
consumes identity, inventory and embedding validity. Gate checks consume coverage
and independent RP2.9 acceptance, not a single successful report.

The [ack owner][ack-owner] alone specifies cross-store acknowledgement order and
bounds. The [progress owner][progress-owner] specifies finite normal catch-up
and authorized recovery; this part consumes its `Current` witnesses.

Remediation currentness strengthens byte identity when authorized canonical
input changes without a source-revision increment. It does not require a new
revision or generation: current-input hash comparison may suffice. Dependency
on `domains.name` is conditional on the approved mapping, not an assumed source
class. At-rest residue and sensitivity admission policy remain owner decisions;
no new exclusion, storage permission or universal erasure deadline is created.

The acceptance witness record binds all required scenario cells across the three
fault maps. Clearly unrelated optional checks may be excluded only by declared
scope. Unsupported optional external lanes remain disabled, and their negative
gate witnesses cannot replace a required supported path. Unresolved conditional
applicability, missing approvals and skipped required scenarios cannot pass.

Canonical eligibility remains independent of projection completeness. A complete
projection can contain a now-ineligible candidate. Capacity admission depends on
atomic rollback but also requires pre-admission measurement; a correct rollback
alone does not prove bounded allocation. These are suspected dependencies, not
proofs of implication.

Exact existing guarantees are reused in the
[overlap register](existing-checks.md#overlap-register). Codec round trips,
historian raw publication, canonical withheld-read reporting and Dreamer
lease/receipt semantics are not re-authored here. Invalidated mirror records
are not implementation scope or current check evidence.

## Handoff

| Records | Next owner and needed seam |
| --- | --- |
| commit-checkpoint-pending-atomic; replay-does-not-resurrect | `/testing:test-strategy` chooses the local oracle boundary; reuse the CAS kill/barrier/reopen pattern after adequacy review. Ack traces and order remain with [their sole owner][ack-owner]. |
| occurrence-payload-separation; raw-tools-exact-and-lexical | `/testing:test-strategy` receives independent byte buffers, source tuples and deterministic collision injection. |
| source-inventory-complete | `/testing:test-strategy` receives the approved five-class inventory and source adapters; export/rebuild owner supplies the fenced source oracle. |
| n13-hooks-stay-gated | `/testing:test-strategy` receives the N1.3 hook manifest and per-gate activation attempts, including startup and supervisor entry. |
| canonical-eligibility-authority | Kernel/daemon owners settle the shared policy seam; `/testing:test-strategy` compares adapters on pinned canonical facts. |
| lexical-dense-coverage-distinct | Embedding owner supplies current validity facts; `/testing:test-strategy` checks report set algebra independently of inference. |
| bounded-admission-preserves-progress | RP2.9 supplies approved units/limits; `/testing:test-strategy` receives admission and allocation observations. |
| remediation-invalidates-derived-bytes | Canonical/source owners settle bounded `M`; projection and embedding owners compare current canonical input hashes and reject stale-current claims without prescribing a new generation. |
| acceptance-situations-witnessed | Campaign coordinator freezes all three maps and required scenario identities; `/testing:test-strategy` consumes the binding witness matrix. RP2.9 approves numeric limits. |

All row names in this handoff carry the `projection-` prefix in the index.
Existing tests remain routed to `/testing:invariant-test-review`; production
guards remain routed to
`/low-level-systems:defensive-assertions-and-invariant-guards` for adequacy.
Neither adequacy pass ran here.

The export/rebuild owner retains fixed-S paging, retention fencing, mismatch
rebuild, selector recovery and disable/deregister/abandonment mechanics. The
embedding owner retains exact tokenizer limits, model-result identity, vector
publication, JobTable admission/leases, retry, backfill and identity GC. Shared
supervisor reuse links to the existing scheduler record; task-specific sweep
inventory and RP2.9 numeric progress approvals remain open under the
[progress owner][progress-owner]. Canonical-claim
policy construction remains outside this part; RP2.1 preserves canonical
identity and consumes its shared verdict rather than inventing a policy copy.

Semantics distribution: ten `always` safety records and one `sometimes`
reachability record. The witness matrix makes required situation coverage an
acceptance obligation, not a metric. No nonexistent code point is labelled
`unreachable`, and numeric recovery bounds remain RP2.9's decision.

## Verification receipt

Mechanical inspection confirms eleven record headings, eleven index rows and eleven
matching evidence files. Each record has the METHOD fields in order. Evidence
files have all required sections and explicit per-record reachability evidence.
All local links and anchors resolve; cited repository source files match the
named HEAD. External source hashes are recorded separately. New cross-part
catalogs are working-tree artifacts, not files at that HEAD. Marker definitions
have single owners; acceptance references them with bounded scenario identities.
The independent evaluation is recorded with attribution in the portfolio report.
No tests or builds ran, and this disposition pass is not a second independent
review or a successful acceptance campaign.

[plan]: ../../../../../commons/docs/plans/2026-09-10-feat-eidnara-rp2-1-projection-coverage-plan.md
[index]: ../../../../../commons/docs/plans/2026-09-10-eidnara-rp2-plan-index.md
[parent]: ../../../../../commons/docs/plans/2026-09-08-0523-feat-eidnara-native-rust-cutover-plan.md
[n1]: ../../../../../commons/docs/plans/2026-09-08-1614-feat-eidnara-rust-product-state-ownership-plan.md
[ack-owner]: ../export-recovery/catalog.md#rp21-ack-follows-local-release
[progress-owner]: ../export-recovery/catalog.md#rp21-catchup-and-authorized-recovery-converge
[export-faults]: ../export-recovery/fault-map.md
[embedding-faults]: ../embedding/fault-map.md
