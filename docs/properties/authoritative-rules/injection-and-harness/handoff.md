# Handoff per record

Routing recommendations only; the receiving skill chooses the form and the
oracle. Reused records keep their original owners.

| Record | Primary handoff | Secondary | Why |
| --- | --- | --- | --- |
| injectable-rows-require-visible-verdict-and-positive-decision-kind | `/testing:invariant-test-review` | `/testing:test-strategy` | Three existing tests claim the filter; the `adr_accepted` arm is one missing row; the open question on the vocabulary oracle under KTD6 is a design decision |
| memory-trim-admits-in-serving-order-not-category-order | `/testing:test-strategy` | `/testing:invariant-test-review` | Pure function; a two-category boundary case and a subsequence proptest; existing tests compare against a self-reference and need an adequacy verdict |
| memory-trim-cost-model-mirrors-rendered-block-structure | `/testing:test-strategy` | none | No existing check; the byte identity is an exact unit oracle; the tokenizer arm is a measurement first |
| memory-trim-clamps-malformed-budget-and-floors-the-integer | `/testing:invariant-test-review` | `/low-level-systems:defensive-assertions-and-invariant-guards` | Covered by two tests; the clamp is a production guard whose failure behavior (silent one-token budget) deserves an enforcement verdict; U2's single-conversion decision is human |
| snapshot-revision-digests-only-rendered-rows | `/testing:invariant-test-review` | `/low-level-systems:defensive-assertions-and-invariant-guards` | Six tests exist; the `DefaultHasher` persistence question is an enforcement and compatibility decision, not a test |
| project-memory-truncated-flag-excludes-budget-trimming | `/testing:test-strategy` | none | One assertion to add to an existing integration test; the `true` arm needs a cheaper seam than 8193 rows |
| subagent-pass-is-never-hard-and-serves-no-m0 | `/testing:test-strategy` | `/low-level-systems:defensive-assertions-and-invariant-guards` | No test; two unit cases on an existing fixture; the additive-path asymmetry is a production-behavior decision |
| additive-only-pass-still-composes-project-memory | `/testing:test-strategy` | none | No test; U6 depends on it; one unit case on an existing fixture |
| pi-context-handler-is-gated-by-compaction-on | `/testing:test-strategy` | none | No test; needs a recording `ExtensionAPI` stub; U6 changes both gates |
| pass-field-builder-is-shared-and-pi-is-subagent-is-constant-false | `/testing:test-strategy` | none | One Pi unit case plus a cross-harness key-set test; the in-process child question needs an e2e or Pi host knowledge |
| memory-mark-command-is-model-free-and-confirms-before-writing | `/testing:invariant-test-review` | none | Well covered; verdict on whether the 21 tests prove the write discipline the directive command will copy; add a static registration check |
| memory-seed-fixture-writes-repo-source-kind-under-code-observed | `/testing:test-strategy` | `/testing:stateful-workload-design` if AE8 becomes a multi-step e2e | e2e seam against a real fixture; one readback of one seeded row, and a decision on extending the seam before AE8 |

No record routes to `/testing:deterministic-simulation-testing`: every
property in scope is a pure function, a per-request gate, or a two-call
harness runner with injectable fakes; none needs schedule control, time
control, or replay.

## Notes for the N1..N5 pass and for U2 and U6 implementers

- Re-verify the `Reachability` of the three reused `packing-required-*`
  records after U2: they are `test-only` at their base because
  `prepare_required` has no production caller; the memory trim becomes the
  first.
- The transform catalog's `canonical-read-staleness-is-distinguishable-from-emptiness`
  describes `transform.rs:13894` as the "compaction-off engine"; the test's
  context has `eidnara_folds: true`. A refinement to that catalog's wording
  belongs to its owner.
- The wire document lists `project_memory` without a shape
  (`docs/host-wire-protocol.md:1032`); KTD7 adds members to an undocumented
  object. The wire-compatibility enrichment named in the brief should
  document the current shape first.
