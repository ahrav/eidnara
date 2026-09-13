# Typed wire decode resource evidence

System: daemon request admission, wire decoding, projection, and retained-size
accounting. Date: 2026-09-13. Source revision:
`2e4433e6b511ae74944df8a9669c428e73915d29` (HEAD).

## Scope answer

The user supplies the settled typed-wire-decode plan, the host wire contract,
latency-audit A1-A3/B1/W1, and HP1 issue references as leads. No additional
incidents or external repositories are supplied. This records the supplied
scope, not an interview that did not happen. Missing evidence stays open.

This is resource-domain enrichment for `/to-spec`, using
`property-discovery-and-catalog` and `docs/properties/METHOD.md`. Publication
and tracker operations are outside this task. No tests, builds, benchmarks,
or source changes establish new implementation evidence here.

## Source identities

| Source | Identity and reason consulted |
| --- | --- |
| P | `docs/plans/2026-09-13-0104-perf-typed-wire-decode-plan.md`; supplied worktree document, absent from HEAD; SHA-256 `badf0d718366bd627d453498576935ba2fd3292cfe5701b7020fa09a07c52f32`. Resource requirements, coefficient proposal, allocation targets, and measurement scope. |
| H | HEAD above. All repository `path:line` citations in this part refer to this revision, including dirty files read with `git show HEAD:<path>`. |
| B | `e451a2b470ae8663b4613ca04f019a30b6d7df53`. Plan's reported measurement baseline; a source identity, not a reproduced measurement. |
| Host contract | `docs/host-wire-protocol.md:304-314`. Separates 64 MiB transport framing from terminal application-limit errors. |
| A1 | `docs/properties/hot-path-optimization/latency-audit/catalog.md:137-231`. Admission ordering, footprint, error codes, and absence of dispatch effects. |
| A2 | `docs/properties/hot-path-optimization/latency-audit/catalog.md:232-317`. Decode-lane compatibility, used to construct resource cases. |
| A3 | `docs/properties/hot-path-optimization/latency-audit/catalog.md:319-350`. Independent pool-shortfall situation; existing evidence is partial. |
| B1 | `docs/properties/hot-path-optimization/latency-audit/catalog.md:356-431`. Shared ownership and projection/served-byte obligations. |
| W1 | `docs/properties/hot-path-optimization/latency-audit/catalog.md:125-130,1768-1823`. Explicit invalidation, with historical text retained. |
| W1 evidence | `docs/properties/hot-path-optimization/latency-audit/evidence/optimized-stage-is-measured-at-production-shape.md`. Historical source/measurement gaps; it does not override the catalog's invalidation. |
| HP1 and related issues | P:L17, P:L77, P:L162-L167 name 350, 435, 436, 438, 441, and 524. Only the plan's descriptions are available in this scope. Full issue bodies, discussion, resolutions, and attached raw evidence are unexamined. |
| Independent portfolio report | User-supplied findings from analyst `ses_f6756093fffeVjNp36S3E8pKrM`; all four lenses reported complete. Consulted to correct scope, categories, predicates, and evidence gaps. Local validation and dispositions are in [portfolio-evaluation.md](portfolio-evaluation.md). |

`P:Lx` is a line in the hashed supplied plan, never a HEAD citation. Plan
measurements remain reported observations on B. The derive-only mirror is not
the final production implementation. No exercise status transfers from it.

## Baseline and citation corrections

`git diff e451a2b4 HEAD` shows no changes in memory-store wire types, meter,
retained-size accounting, wire projection, the parse-peak test, the served
allocation test, or `benches/hot_path.rs`. The daemon library differs by one
module declaration. Its dirty worktree is not used for implementation claims.

Local `main` is `4980f8af3bb90d58b19b80a38a227fb6363a8b33`; its tree has no
`crates/daemon/src/metered_decode.rs`. It is not the before artifact described
by P:L19 or P:L213. No checkout or branch switch is needed for this discovery.

Verified corrections to leads:

- Tree fallback is `crates/daemon/src/lib.rs:12931-12946`; the typed failure
  restart is line 12935, not the start of the direct-lane match.
- Snapshot `tail_delta` accounting is `crates/daemon/src/lib.rs:1766`.
- Actual retained-original checks span `crates/daemon/src/wire.rs:1708-1792`.
- A1-A3 tests often compute capacities from the candidate's `footprint_of`.
  Their existence does not provide a frozen absolute boundary artifact.
- W1's old relationship and handoff prose remains in its source catalog.
  Its explicit invalidated status controls interpretation.

## Method and execution limits

Each system-model and property lens is preserved under `_lenses/`. Separate
attention passes run in this context because the harness rejects nested
subagents at its depth limit. These are not independent reviewer opinions.
System wildcard follows the other system passes; property wildcard follows
the other property passes. Those notes remain original discovery snapshots,
not revised findings. The independent portfolio report supplied afterward
completes the separate review; its dispositions supersede conflicting lens text.

Code location starts with `colgrep`. Its index reports no units for the large
daemon library, so HEAD text extraction supplies exact anchors there.
Existing checks are inventoried, not audited for adequacy. The catalog states
claims under test, not measured guarantees.

## Mechanical verification

The document-only validator checks exact record field order, index/record/
evidence correspondence, local links and heading fragments, source-range
bounds against HEAD, named test-function anchors, evidence lengths, and
trailing whitespace. It does not run repository tests or audit check strength.
The original set contains 36 files with seven active records. Its initial
mechanical check passes with evidence lengths of 87 to 98 lines. The revision
retains seven records/evidence files, invalidates R6, and adds evidence-gates.md:
six active records, one invalidated record, and one evidence gate in 37 files.
All records retain `Exercised: not yet`; gate execution remains pending.

The refined set passes the same document-only checks: exact schema and index
status, seven evidence files of 93 to 114 lines, local links/fragments, 266
HEAD source ranges, and 57 named test anchors. Twelve independent resource
marker rows and the separate evidence receipt preserve all thirteen original
names. No runtime or benchmark result is inferred from document validation.

## Independent-review audit trail

The supplied report is attributed to `ses_f6756093fffeVjNp36S3E8pKrM`, not to
the discoverer's local validation. Its nine findings and four-lens dispositions
are preserved in portfolio-evaluation.md. The prior 36-file bundle SHA-256 is
`4337593f3bf8c3e244ce1ec78187d3c62c0d02f1111c2318d3d82c7e3afb721e`, computed
over sorted relative path, NUL, file contents, NUL for each Markdown file.
Prior catalog and inventory hashes are recorded with the findings.

The revision narrows R3 to original A1-A3 cases and fixed gate rules while
preserving KTD4's wider ceiling; moves payoff to EG1 without reactivating W1;
separates coefficient selection from R1; links the hard-coded-three assertion;
exposes verified A1/probe ordering disagreement; and separates individual
resource markers from the evidence receipt. Existing logical ownership and
resident accounting remain the contract. No exact RSS guarantee is introduced.
