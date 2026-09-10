# Embedding portfolio evaluation and dispositions

## Provenance and review boundary

- Repository: `/local/home/ahrav/scratch/eidnara`.
- Revision: `913234433ae36a80a6e22c6aac14c7f9aab74386`.
- Date: 2026-09-10.
- Independent evaluator: central analyst session
  `ses_f7623dcccffe3Y09nW2wVoABif`, reported by the user as having reviewed
  embedding, projection/coverage, and export/recovery together.
- Evidence received here: the user's central finding summary and instructions,
  including the qualification to G1. The four-lens summary below organizes
  those findings; it does not invent additional analyst conclusions.
- This writer checks the cited code and applies embedding-only refinements.
  These edits and mechanical verification are not independent re-review.
- External scope remains the supplied local plans, linked parents, repository,
  and central findings. No incident evidence is supplied. Source reasons and
  plan locators remain in the [source register](catalog.md#source-register).
- No product tests, builds, benchmarks, or tracker operations run. Existing
  checks remain unaudited and all nine proposed records remain unexercised.

## Four-lens evaluation

### Harness fit

G3 identifies reusable process-crash machinery that the initial embedding
inventory omitted. `crates/kernel/tests/cas_fault_injection.rs:1046-1094`
contains a flushed child barrier, parent reexecution/wait, and kill/join guard.
The check at `:924-989` reopens and compares recovered state with uninterrupted
execution. The suite states its process-crash/injected-error boundary at
`:1-6`; it does not establish power-loss ordering or cold-device persistence.

Disposition: inventory that pattern as unaudited and reuse it after product
vector storage, commit hooks, and a per-K oracle exist. Those product seams
remain missing. The handoff does not ask for a new broad crash harness or
credit the CAS oracle with testing embedding completion.

### Coverage balance

B1/B2 require a central choice of enabled acceptance scope and enforceable
situation coverage. Nine `always` records alone can pass without reaching
inference, recovery, or the distinct identity-change dimensions. Optional
inference and one marker firing for only one dimension are insufficient.

Disposition: consume
[projection-acceptance-situations-witnessed](../projection-coverage/catalog.md#projection-acceptance-situations-witnessed).
It owns the obligation to witness every declared enabled marker/scenario
dimension across the three maps. The central owner must declare applicability
before the campaign; missing required dimensions cannot be excused afterward.
Embedding adds no duplicate reachability record. The 16 marker names remain
constant; applicable same-revision remediation extends an existing dimension.

G2 belongs to export/recovery's
[catchup-and-authorized-recovery-converge](../export-recovery/catalog.md#catchup-and-authorized-recovery-converge).
Embedding restart recovery consumes current durable pending state produced by
that path. It does not duplicate steady catch-up or authorized recovery.

### Implementability

G1 identifies a real same-revision mutation at
`crates/kernel/src/envelope.rs:395-438`: operator remediation updates
`domains.name` and carries the loaded object into the change record without
incrementing its revision. It does not establish that every embedding input
uses domain names, or that every component of K stays unchanged. K already
includes the input hash.

Disposition: if the approved input mapping uses the remediated field,
`current(K)` compares K against authoritative current mapped bytes/hash at the
completion boundary. Revision equality or a stale projected hash cannot prove
freshness. A held result for the old bytes becomes obsolete even if the revision
is unchanged. The approved mapping's dependency is still an owner decision.
The [projection remediation record](../projection-coverage/catalog.md#projection-remediation-invalidates-derived-bytes)
owns source invalidation. This refinement requires no new key/version and
defines no broad erasure policy.

R6 makes the async query lane and EvalBudget adapter explicit prerequisites
shared with RP2.7. RP2.1.U3 integrates exact preflight and query priority into
the existing embedding runtime; RP2.7.U3 owns authorization, pre-wait budget
derivation, awaiting embedding before bounded scans, degradation, and final
validation. P7 lines 142-147 supply that ownership. Full query-path delivery is
not scheduled solely inside RP2.1.

R4 makes index semantics comparable across catalogs. The index now has
`Slug | Type | Reachability | Semantics | Status | Confidence`. Both liveness
rows say `always` per admitted episode and RP2.9-blocked. Episode admission
means independently witnessed preconditions, not successful implementation
admission. This avoids selecting only successful queries for evaluation.

### Wildcard

B4 identifies stale baseline claims that must survive outside `_lenses/`.
They are retained in
[existing-checks.md](existing-checks.md#durable-corrections-to-reused-evidence)
and here with current evidence:

- Claude tokenizer production use exists: `crates/daemon/Cargo.toml:32` and
  `crates/daemon/src/token_cache.rs:127-141`. Its vocabulary remains distinct
  from the embedding model tokenizer.
- Production Synapse composition exists at
  `crates/daemon/src/bin/eidnara_host/serve.rs:1097-1127`. Certified artifacts
  or disabled/unsupported fallback are selected at `:1027-1065`. Composition
  is default-production; certified inference/live jobs are explicit-config-only.
- Eviction ranks eligible completed jobs by last poll time falling back to
  completion, then by completion: `crates/host-runtime/src/synapse/jobs.rs:156-158`,
  `:773-797`, `:840-851`. Oldest-completion behavior for unpolled jobs is not
  the full eviction contract.
- The certified-runtime check is explicitly ignored at
  `crates/host-runtime/tests/synapse_bundle.rs:579-581`. Its source presence
  supplies no execution or acceptance witness.

These corrections do not rewrite older catalogs, import their exercise claims,
or upgrade any check from unaudited. The wire's truncation and recovery-ledger
wording remain separate specification prerequisites already recorded by discovery.

## Finding disposition ledger

| Finding | Class | Embedding disposition | Remaining owner or evidence |
| --- | --- | --- | --- |
| G1 | gap, qualified during disposition | Extend current(K), completion evidence, and the held-result marker's input-hash dimension with applicable same-revision remediation. Reject the overclaim that every K stays unchanged. | Spec owner declares which approved mappings depend on remediated fields and preserves the atomic current-bytes predicate. |
| G2 | gap in another lane | Link export/recovery convergence; retain embedding-specific restart recovery. | Export/recovery owns its record and implementation prerequisites. |
| G3 | gap | Inventory and reuse the kernel CAS crash pattern as unaudited. Preserve missing product store/hooks/oracle. | Storage and test owners supply narrow product integration after the protocol is fixed. |
| R4 | refinement | Unify index columns and qualify liveness by admitted episode and RP2.9 blocking. | RP2.9 supplies approved bounds; campaign construction supplies independent premises. |
| R6 | refinement | State shared async query-lane/EvalBudget prerequisite and RP2.1.U3 priority integration in catalog, fault map, evidence, and handoff. | RP2.7 retains full query-route ownership; spec owner preserves the dependency. |
| B1/B2 | biases requiring central choice | Link the shared all-declared-dimensions acceptance record; optional/unreached inference cannot pass. | Central owner declares enabled scenarios, applicability, and required dimensions before execution. |
| B4 | bias | Promote exact baseline corrections into durable inventory and this evaluation. | Existing checks remain unaudited; no older catalog is edited. |

## Open prerequisites retained for the specification

The central evaluation is complete, but its receipt is not product acceptance.
The spec owner must preserve independently certified token semantics, approved
input mappings, authoritative current-byte validation, pending scan/key rules,
vector/completion transaction durability, retry and operator outcomes, query
service guarantees, GC references/retention, shared budget/supervisor integration,
and the physical-drain contract for an uncooperative native call.

RP2.9 owns numeric limits and service envelopes. No value is invented here.
Until these prerequisites and the declared scenario witnesses exist, proposed
records remain active, test-only, and unexercised. A later independent review
must identify its own evidence; these dispositions are not that review.

## Mechanical verification receipt

The embedding directory contains 17 Markdown files: the catalog, inventory,
fault map, this evaluation, nine evidence files, and four retained lens files.
Nine records match nine index rows and nine evidence files. There are seven
safety and two bounded-liveness records, all using `always`, plus 16 unique
constant precondition marker names. The shared acceptance record supplies the
cross-catalog reachability obligation without changing these local counts.

Read-only verification confirms METHOD field order and the six-column index.
All 108 local links and anchors resolve, including the three shared records.
The 174 fully qualified citation occurrences are within their source ranges;
all 21 cited repository source files are byte-identical to the pinned HEAD.
The inventory's 45 named check/helper declarations and 10 scheduler test
declarations resolve. Evidence files contain 76-102 lines. No stale
evaluation-pending statement remains in this directory.

These are artifact checks, not test, build, benchmark, or independent re-review
results. Existing checks retain their unaudited status.
