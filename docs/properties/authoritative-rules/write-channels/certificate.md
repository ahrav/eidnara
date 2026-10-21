# Output certificate: proposed write channels N1 and N2

Pass: invariant-driven domain modeling, new-behavior scope.
Design under model: the settled plan
`docs/plans/2026-10-21-1902-feat-authoritative-rules-automatic-use-plan.md`
at repository HEAD `08de9051a735f136ff63a9dca49eaa11493b3006`.
Surfaces: N1 directive channel and explicit-user subject guard; N2 repository
policy ingestion and reconciliation.
Every writer inventory below is proposed. Facts marked "verified" were read
from the code at HEAD; everything else is a claim the plan makes.

## SCOPE AND AUTHORITY

Owned state (one deployable: the daemon process with its embedded Kernel
store, one SQLite writer connection):

- Kernel decision rows (`object_registry`, `decisions`), their admission
  history (`admission_decisions`), observations, evidence (artifact
  references), scopes, and the operation receipt ledger
  (`operation_receipts`, `commit_log`). The Kernel admission policy
  (`evaluate_admission`, `POLICY_REVISION` 1) is the winning authority for
  maturity, disposition, visibility, and sensitivity of every row. Verified:
  `crates/kernel/src/admission.rs:20-23, 264-385`.
- Proposed N2 derived state: the cached HEAD oid per project binding digest,
  held in daemon memory. It has no authority over Kernel rows.

Actors and crossings:

- User, through a host command surface (Pi `registerCommand`, OpenCode
  `command.execute.before`), calling `directive.preview|record|revise|revoke`.
  Caller-visible crossing; the harness plugin is a caller, the daemon decides.
- Model, through the `eidnara_memory` tool, calling `kernel.commit` with a
  plugin `source_kind` that `derive_classes` maps to `ModelInference`
  (verified: `crates/daemon/src/kernel_routes/commit.rs:236-248`). The model
  tool client has no `directive.*` method (proposed; the method literals do
  not exist at HEAD).
- MemoryReviewer, through `review.accept`; stays `model_inference`
  (settled; out of scope here, reference pattern only
  `crates/daemon/src/memory_reviewer/settlement.rs:1087-1160`).
- Repository policy module (proposed `crates/daemon/src/repository_policy.rs`),
  a daemon-internal actor triggered by the m0 compose path; its input is the
  git HEAD tree read through gix.
- The git repository on disk is an external authority for N2: HEAD oid and
  tree bytes. Working-tree bytes are outside the model.

Routed decisions (not decided here): wire schema of `directive.*` and the
`cwd` field (api-design); SQLite transaction mechanics beyond what the Kernel
already fixes (persistence-boundary); harness command UX (N5); read-side
scope matching and packing (N3, N4).

## BEHAVIOR

Commands (N1): `directive.record`, `directive.revise`, `directive.revoke`,
`directive.record` with `confirmed_from`. Query (N1): `directive.preview`.
Guard (N1): the explicit-user subject guard inside `kernel.commit`.
Command (N2): `reconcile(binding)`, decomposed into per-file commits.
Query (N2): the cached HEAD oid comparison.

Legal witnesses:

- W1: user records "Run cargo fmt before every commit"; one Kernel commit
  inserts an `adr_accepted` decision with `source_kind user_directive`,
  lineage `("user_directive","directive:<uuid>",1)`, sensitivity Normal, then
  records `AcceptedAdr` at `(explicit_user,user_explicit)`; the row's
  effective maturity is Approved, visibility `automatic`, Visible on
  AutoInject.
- W2: HEAD `X` contains `AGENTS.md` with three list items; reconcile ingests
  the blob as an artifact with `RepositoryProvenance{project digest, X}`,
  inserts one `config_present` observation on lineage
  `("repository_policy","<digest>:AGENTS.md",1)`, inserts three decisions on
  that lineage, admits each under `ConfigObserved` with the observation as
  trigger at `(trusted_local_code,current_config)`; each is Verified and
  Visible on AutoInject.

Forbidden outcomes:

- F1: an admission row with `source_class = explicit_user` whose commit was
  produced by a model-mediated route.
- F2: a directive invalidated, superseded, or dispositioned by a
  `kernel.commit` whose derived class is `model_inference`.
- F3: a revoked or superseded directive served Visible on AutoInject.
- F4: a `repository_policy` decision whose text came from bytes that are not
  in the tree of the commit its artifact provenance names.
- F5: a `repository_policy` decision that is Verified without a live
  `config_present` observation on its exact lineage triple backed by
  provenance-bearing evidence.
- F6: after reconciliation completes for HEAD `Y`, a live `repository_policy`
  decision whose provenance revision is an older oid `X`.
- F7: a model-authored row promoted in place to `explicit_user`.

## RULES

Each rule: temporal kind; observation dependencies; validity point.

- R-N1-1 Class pair fixed by route. Initial admissibility; intrinsic (the
  handler constant). Valid at admission of the `directive.*` request body: a
  body carrying `asserted_source_class` or `asserted_taint_class` is refused.
- R-N1-2 Decision before AcceptedAdr, same commit. Transition safety;
  current authoritative state (the inserted decision must be live when
  `subject_is_accepted_decision` is evaluated). Valid at commit. Verified
  mechanism: `admission.rs:2436-2458` reads the live `adr_accepted` decision
  through `APPROVAL_OBJECT_PREDICATE` (`admission.rs:710-714`).
- R-N1-3 Sensitivity Normal on a directive. State predicate; intrinsic plus
  policy. Verified: approval rows require `sensitivity_class='normal'`
  (`admission.rs:843-856`) and `visibility_row` yields `Automatic` only for
  Active rows at or above Verified (`admission.rs:4243-4255`).
- R-N1-4 Lineage and revision on revise. Transition safety; current
  authoritative state. Successor shares domain, source_kind, source_id and has
  a strictly greater `source_revision` (verified `validate_successor`,
  `crates/kernel/src/slice/write.rs:1271-1284`); the plan fixes `+1`.
  Class continuity: `evaluate_admission` refuses a prior whose classes differ
  (verified `admission.rs:268-274`), so a successor admitted by a model class
  on a lineage whose object prior is explicit_user is refused only when the
  same object is re-admitted; a new successor object has no prior.
- R-N1-5 Idempotency by operation key. Transition safety (once-only);
  current authoritative state (receipt ledger). Verified:
  `(producer, operation_key)` with equal `request_digest` replays the stored
  receipt with `replayed: true`; a different digest is `Conflict`
  (`crates/kernel/src/envelope.rs:1279-1309`), which the commit route reports
  as `OperationKeyReused` (`commit.rs:928-935`).
- R-N1-6 Revoke cascades. Transition safety; current authoritative state.
  Verified: `revoke_approval` invalidates the approval object, records an
  `approval_revoked` audit, and demotes dependents transitively
  (`admission.rs:1351-1445, 1447-1486`).
- R-N1-7 Subject guard. Initial admissibility of a `kernel.commit`
  operation; current authoritative state (the subject's latest admission row)
  plus intrinsic (the caller's derived class). Valid before any write in the
  envelope. Verified gap at HEAD: `RetireDecision` checks scope only
  (`commit.rs:849-853`); `SupersedeDecision` admits only the new object
  (`commit.rs:787-848`); `Disposition` requests use the subject's own stored
  classes (`commit.rs:566-600`), so a model-mediated `explicit_reject` on an
  explicit_user subject passes the kernel's class-continuity check.
- R-N1-8 Promotion only by fresh record. Transition safety; current
  authoritative state. The candidate's classes never change; it receives
  `ExplicitReject`; the new directive is a distinct object on a
  `user_directive` lineage.
- R-N2-1 HEAD tree is the only byte source. Initial admissibility;
  external authority (git) with version = HEAD oid. Valid at the moment the
  tree is read by oid.
- R-N2-2 No HEAD, no writes. Initial admissibility; external authority.
- R-N2-3 Bounds fail closed per file. Initial admissibility; intrinsic.
- R-N2-4 Segmentation is a pure function of bytes. State predicate;
  intrinsic. Identical bytes give identical units, labels, and ids.
- R-N2-5 Per-unit digest reconciliation. Transition safety; current
  authoritative state (live rows on the file lineage) plus external authority
  (new bytes). Valid at the per-file commit.
- R-N2-6 Observation on the exact lineage triple before ConfigObserved.
  Transition safety; current authoritative state. Verified:
  `validate_trigger` matches `o.source_kind, o.source_id, o.source_revision`
  of the subject and requires a live observation of kind `config_present`
  with live backing evidence (`admission.rs:2726-2795`); a missing or
  mismatched trigger degrades the event to `Other`, which denies
  (`admission.rs:1603-1644, 4159`).
- R-N2-7 Provenance keeps the artifact Normal. State predicate; intrinsic.
  Verified: an inspected UTF-8 payload asserted Normal with non-empty
  `repository_id` and `revision` stays Normal; without affirmative provenance
  it becomes Sensitive; a detected secret makes it Secret; non-UTF-8 is at
  least Sensitive (`crates/kernel/src/cas/ingest.rs:184-207`). The trigger's
  sensitivity composes into the decision (`admission.rs:1604-1605, 1631`).
- R-N2-8 Per-file atomicity. Transition safety; current authoritative
  state. One envelope per file; the envelope poisons on any failed mutation
  and the SQLite transaction commits or rolls back whole (verified
  `envelope.rs:348-369, 1310-1330`). Artifact ingest is its own commit
  (verified `ingest.rs:583-600`), so a file is two commits, not one.
- R-N2-9 Cached oid is advisory. Derived-state rule; derived observation.
  The cache may only suppress work; it never authorizes a write or a read.
- R-N2-10 Convergence. Liveness: after any crash or interleaving, one
  subsequent complete pass at a stable HEAD leaves every file lineage at that
  HEAD. Bound: one pass.

## IDENTITY AND VALUES

- Directive identity: EARNED as entity identity through succession. The
  lineage `("user_directive","directive:<uuid>")` is the continuity; object
  ids change on revise (verified: supersession cannot keep the predecessor's
  object id, `write.rs:388-393` refuses a survivor equal to the predecessor).
- Repository rule identity: EARNED as lineage continuity per file
  (`"<digest>:<rel path>"`) with value semantics per unit (content digest).
  Object id `rule_<sha256(source_id:rev:unit_digest)>` is a value-derived
  name; two identical units in one file collide (open question OQ-N2-5).
- Unit text: value semantics after whitespace normalization; equality is
  digest equality.
- HEAD oid: value; equality is string equality of the hex oid.
- Operation key: value; identity of one attempted write.
- Aggregate: NOT EARNED beyond what the Kernel envelope already provides; the
  per-file commit is the consistency boundary.
- Repository role: NOT EARNED; Kernel store methods are the access path.
- Domain service: EARNED for N2 `reconcile` (a process with no natural
  owner), realised as a daemon module.

## TRANSITIONS

Directive object (own latest admission row):

| Current | Command / actor | Guard | Next |
| --- | --- | --- | --- |
| absent | record / user | key unused; text non-empty, bounded; no asserted class | live adr_accepted, Approved, Active, automatic |
| live Approved | revise / user | successor rev = rev+1, same lineage | predecessor Superseded + invalidated, successor Approved |
| live Approved | revoke / user | reason non-empty | invalidated, `approval_revoked`, dependents demoted |
| live Approved | kernel.commit supersede/retire/Disposition / model | refused | unchanged |
| any | record (same key, same digest) | replay | unchanged, receipt replayed |

Illegal histories: AcceptedAdr recorded before the decision exists (self
approval cannot root; evaluates as a denied raise); a successor at rev <= old
(Conflict); a model `kernel.commit` that invalidates a directive.

Repository rule lineage (per file):

| Current | Command | Guard | Next |
| --- | --- | --- | --- |
| no rows | reconcile at oid X | file within bounds, UTF-8 | observation rev 1; N decisions rev 1 Verified |
| rev N at oid X | reconcile at oid Y, blob changed | HEAD readable | observation rev N+1; unchanged units superseded by identical text, changed by new text, removed retired, new inserted |
| rev N at oid X | reconcile at oid X | cache equal or receipt replays | unchanged |
| rev N at oid X | reconcile at oid Y, blob identical | HEAD readable | plan says "an unchanged blob writes nothing" (OQ-N2-2 on how this is keyed) |
| rev N | reconcile, file over bound or non-UTF-8 at Y | | no units, no observation; fate of rev N rows unspecified (OQ-N2-3) |

## PRESERVATION

Enforcement sets (proposed; closure unverified until implemented):

- F1/F7 (no model path to explicit_user): authority is the route method.
  Participants: `directive.*` handler (constant class pair), `kernel.commit`
  `resolve_classes` (verified refuses a class above the derived one,
  `commit.rs:271-312`, pinned by `crates/daemon/tests/kernel_routes.rs:1145`),
  `review.accept` (model_inference, settled). Exceptional writers: test
  seeding through the store API (`crates/daemon/tests/kernel_routes.rs:4355-4386`,
  test-only); direct store access by other daemon modules (must be
  inventoried at implementation). Bypass: none intended. Falsifier: a new
  `derive_classes` arm or a `kernel.commit` op literal reaching
  `(explicit_user, user_explicit)`.
- F2 (guard): authority is the subject's latest admission row read inside the
  commit envelope (`Envelope::subject_admission`, verified
  `admission.rs:1001-1008`, same-transaction cache). Ordered set: resolve
  subject in scope, read latest admission, compare `source_class` to
  `ExplicitUser` and caller class to `ModelInference`, refuse before any
  write. Writers to close: the three op arms in `apply`. Exceptional: the
  `directive.*` route itself (authorized), test seeds.
- F3 (revoked never served): authority is the serving view's latest own
  decision and object liveness; `revoke_approval` invalidates the object
  (verified). The read side (N4) filters Visible rows; this pass asserts the
  Kernel verdict only.
- F4/F5 (witness): authority is `validate_trigger` plus artifact ingest
  classification. Ordered set per file: ingest artifact (own commit) ->
  envelope { insert observation citing the evidence -> per unit
  supersede/retire/insert -> admit each live decision with trigger } ->
  commit. Bypass: a decision admitted with `Other` lands Candidate, not
  Verified (fail closed). A `kernel.ingest` route artifact with client
  provenance is a different producer and never cited by this module.
- F6 (no stale older-HEAD rules): no single Kernel mechanism; the enforcement
  set is the reconcile plan computed from live rows inside the envelope plus
  the cache contract (R-N2-9) plus convergence (R-N2-10). Closure test 3
  (stale observation) is where this set is weakest; see falsifiers.

Commit/restoration points: the SQLite commit of each envelope. Unknown result:
a crash after `COMMIT` returns but before the daemon observes it; the receipt
ledger answers the replay.

## COMMAND OUTCOMES

| Command | Certainty | Cause | Caller action |
| --- | --- | --- | --- |
| record, success | definitely applied | | stop |
| record, same key and digest | definitely applied (earlier) | replay | stop; treat receipt as original |
| record, same key other digest | definitely not applied | `OperationKeyReused` conflict | choose a new key |
| record, asserted class field | definitely not applied | `invalid_params` | stop |
| record, empty or oversized text | definitely not applied | invalid input | fix input |
| revise, rev not advanced | definitely not applied | `RevisionNotAdvanced` conflict | refresh and retry |
| revise, predecessor barred (Rejected, Quarantined, Contradicted) | definitely not applied | `AdmissionPolicy` | stop |
| revoke, object not a live approval | definitely not applied | `NotFound` | refresh |
| any, daemon crash before response | unknown | unavailable | safe replay with the same key and digest |
| kernel.commit on explicit_user subject by model | definitely not applied | policy rejection (new invalid-input outcome) | stop |
| reconcile, no repo / unborn / gix error | definitely not applied | unavailable (fail closed) | none; next pass retries |
| reconcile, file over bound | definitely not applied for that file | domain rejection | none |
| reconcile, per-file commit failure | definitely not applied for that file; earlier files applied | conflict / defect | next pass replays |
| reconcile, crash mid-pass | per file: applied or not; cache must not claim the new oid | unavailable | next pass converges |

## QUERY CONTRACTS

- `directive.preview`: source is the request body and the bound project;
  returns rendered line and scope; no Kernel read is required; writes nothing;
  may not authorize anything.
- cached HEAD oid: source is daemon memory; provenance is the oid of the last
  pass whose per-file commits all succeeded; freshness is "equal to git HEAD
  at the start of that pass"; consumers: `reconcile` early return only;
  prohibited decisions: any write, any read-side verdict; absence means
  "reconcile"; mismatch action: reconcile. See DERIVED STATE.
- subject latest admission (inside the guard): authority is
  `admission_decisions` ordered by `commit_seq, admission_decision_id`
  through the same-transaction cache (verified); freshness is
  read-your-writes within the envelope; absence (no admission) means the
  guard does not fire and the existing `AdmissionPolicy` refusal applies
  (verified `commit.rs:577-579`).

## PERSISTENCE AND CONCURRENCY

- Topology: single Kernel writer; each envelope is one `IMMEDIATE` SQLite
  transaction with a receipt lookup first (verified
  `envelope.rs:1310-1330`). Two concurrent `reconcile` passes for one binding
  serialize at the writer; the second either replays (same key and digest) or
  must compute its plan from the rows the first left live.
- Required atomic outcomes: (decision, AcceptedAdr) in one commit;
  (observation, all unit changes, all admissions) for one file in one commit.
- Forbidden partial results: a live `adr_accepted` directive with no
  admission row; a rev N+1 observation with rev N decisions still live for
  the same file after the commit; a decision admitted `ConfigObserved` whose
  trigger is in a later commit.
- Retry: safe replay by key and digest; a pass must derive its key from
  `(path, oid)` so a retry at the same HEAD replays and a retry at a new HEAD
  is a new write.
- Progress: a reconcile runs inside the transform pass; bounded by 32 files
  times 64 units of Kernel writes plus gix reads; the writer is held per file,
  not per pass. Overload risk is recorded as OQ-N2-7.

## DERIVED STATE

Cached HEAD oid per binding digest (proposed):

- Base authority: git HEAD (for "is there work") and the Kernel rows (for
  "what is live"). Disagreement rule: the Kernel rows win for what is
  served; git wins for what should be written; the cache wins nothing.
- Derivation: set to oid `X` after a pass at `X` completes every file commit.
- Refresh: compared on every m0 compose; invalidated by inequality.
- Visible freshness: best effort; stale when HEAD moves between passes
  (benign, next pass reconciles) or when two passes race (see falsifier).
- Allowed consumer: `reconcile` early return. Prohibited decisions: any
  Kernel write, any serving verdict, any claim that rows are at `X`.
- Mismatch detection: none at HEAD; a cheap oracle is "the provenance
  revision of the live observation on the root lineage equals the cache".
- Rebuild: daemon restart clears it; the first pass reconciles (replays when
  nothing changed).
- Failure meaning: absent means reconcile; a cache that names an oid the
  Kernel does not hold is the F6 window.

## STRUCTURE INPUTS

Hidden decisions: the class pair (route handler); the segmentation grammar
(pure function, fixture-tested); the operation-key derivation
`project.operation_key("repository_policy", "<path>:<oid>")` (verified helper
`crates/daemon/src/kernel_routes/project.rs:73-78`).
Labelled edges: `directive.*` handler -> Kernel store (mutation authority);
`repository_policy` -> gix (observation, external authority) -> Kernel store
(mutation); m0 compose -> `reconcile` (invocation, before the canonical read);
`kernel.commit` guard -> `Envelope::subject_admission` (correctness `uses`).

## VARIATION / ASSEMBLY

NOT EARNED. One class pair, one file name (`AGENTS.md`), one segmentation
grammar. `CLAUDE.md` and friends are deferred by the plan.

## FALSIFIERS

- R-N1-1: a `directive.record` body with `asserted_source_class` accepted.
- R-N1-2: `AcceptedAdr` recorded in a commit before the decision insert; the
  row evaluates at Candidate/Verified ceiling without approval and the
  directive is not Visible.
- R-N1-5: two `record` calls with one key and two texts both landing.
- R-N1-6: after `revoke`, a model row that cited the directive still
  effective Approved.
- R-N1-7 (smallest, reproducible at HEAD): model tool `retire` on a
  root-scoped directive object id succeeds (`commit.rs:849-853` checks scope
  only).
- R-N1-8: a candidate's latest admission row changes `source_class`.
- R-N2-1: a rule whose text is in the working tree but not in the tree of
  the oid its artifact names.
- R-N2-3: a 65 KiB `AGENTS.md` yields an observation or any decision.
- R-N2-4: the same bytes, two runs, two id sets.
- R-N2-5: after a blob change, a rev N decision still live beside rev N+1.
- R-N2-6: a Verified repository rule whose trigger observation is on another
  revision (verified the Kernel refuses: `kernel_admission.rs:1323`).
- R-N2-8: a crash between the observation insert and the last admission
  leaves a committed observation with unadmitted decisions (the envelope
  makes this impossible when both are in one closure; the falsifier is an
  implementation that splits them).
- R-N2-9/F6 (interleaving): pass A reads HEAD `X`; pass B reads HEAD `Y`
  (newer); B commits and sets cache `Y`; A commits later and sets cache `X`;
  next pass sees HEAD `Y` != `X` and heals. Variant: A commits last but B
  sets the cache last: cache `Y`, Kernel rows at `X`, HEAD `Y`; no later pass
  heals until HEAD moves. Smallest violating trace for F6.
- R-N2-10: a crash between file 1 and file 2 with the cache already set to
  the new oid; file 2 never reconciles.

Analysis bounds: single daemon process, one Kernel store, one project
binding; no model checking performed; writer inventories are proposed.

## HANDOFF

- Rust design review: `directive.rs` handler shape, guard placement in
  `apply`, `repository_policy` module API, the envelope-internal plan
  computation.
- Persistence boundary: the two-commit shape per file (ingest, then
  envelope) and the cache write ordering.
- API design: `directive.*` bodies, `invalid_params` versus kernel-state
  outcomes, the §7.8 method list.
- Testing (property catalog): the 14 records in `catalog.md`; falsifiers
  above become failing tests first.
- Open questions: `open-questions.md` (OQ-N1-1 to OQ-N1-4, OQ-N2-1 to
  OQ-N2-8).
