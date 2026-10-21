# Handoff: Kernel admission and scope eligibility (S1, S2)

One line per record: the suggested next owner and what it should do. Every
record is `active`; none is `invalidated`.

| Record | Handoff | Ask |
| --- | --- | --- |
| `explicit-user-accepted-adr-self-approves-to-approved` | `/testing:test-strategy` then `/testing:invariant-test-review` | choose the store-level form that reads the row back on `AutoInject` through `read_visible`; audit whether `an_accepted_adr_roots_authority_only_from_an_accepted_decision_object` proves the evaluator half |
| `accepted-adr-requires-live-accepted-decision-subject` | `/testing:test-strategy` | add the missing-object and other-decision-kind subjects beside `only_an_accepted_decision_object_self_approves`; one test, three negative subjects, one positive |
| `config-observed-promotes-only-on-exact-lineage-trigger` | `/testing:test-strategy` then `/testing:invariant-test-review` | a decision-object subject (the existing tests use staged candidates) with the five negative controls; review whether the existing candidate tests already discharge the predicate for object subjects |
| `sensitive-decision-is-hidden-on-automatic-surfaces-and-grants-no-authority` | `/testing:test-strategy` | a self-approved `Sensitive` `adr_accepted` decision read on `AutoInject` and cited as an approval; a tightening replay as the second case |
| `authority-loss-cascades-to-latest-dependents` | `/testing:invariant-test-review` first, then `/testing:test-strategy` (generated 4,097-dependent fixture) for the deferred branch | audit the five cascade tests for the per-dependent `min(effective, ceiling)` assertion and history preservation; the 4,097-dependent graph needs a generated workload and the `cascade_deferred_a_dependent` marker |
| `decision-succession-keeps-lineage-advances-revision-and-forbids-identity-reuse` | `/testing:test-strategy` | extend `kernel_slice.rs` with the `Rejected` and `Contradicted` predecessors and the same-object-id case; record the class-continuity probe as documentation of lead L2 for the N1 pass |
| `explicit-reject-moves-to-rejected-review-only-and-bars-succession` | `/testing:test-strategy` | one route test: `explicit_reject`, then `supersede_decision` refused, then `mark_stale` denied; hand lead L4 to the N1 pass |
| `plugin-commit-classes-are-closed-at-model-inference` | `/testing:test-strategy` | add `user_directive` and `repository_policy` to the unknown-kind cases in `a_plugin_write_is_capped_at_inference_classes` and one route request per kind |
| `policy-digest-pins-revision-one` | `/testing:mutation-testing-operations` | a mutation run scoped to the marked sections as situation coverage; surface the "request preparation is outside the digest" question to the owner |
| `scope-filter-excludes-a-term-on-an-unresolved-dimension` | `/testing:test-strategy` | stored-scope cases `S1` to `S5` through `insert_scope` and `read_visible`; a proptest over `context_strategy` for the `Exact`/`Set` algebra |
| `sql-project-prefilter-is-a-superset-of-the-daemon-verdict` | `/testing:test-strategy` | the one-store id-set comparison in the evidence file; this is the only record with no existing check |
| `served-rows-exclude-objects-invalidated-at-the-snapshot` | `/testing:invariant-test-review` first, then `/testing:test-strategy` | audit the integration test for the superseded and retired absence assertions; add the snapshot-boundary case (`as_of` equal to the invalidating commit) as a store-level test |

## Production-guard questions for `/low-level-systems:defensive-assertions-and-invariant-guards`

- The cascade's deferred set keeps stored elevated visibility until a later
  write (`admission.rs:1424-1446`). Whether a bounded repair step belongs in
  production is a guard-placement question.
- Lead L4: `kernel.commit` `retire_decision`, `supersede_decision`, and
  `disposition` do not compare the caller's derived class with the subject's
  stored class. The N1 pass proposes the guard; its placement (route versus
  kernel) and failure behavior are for this skill.

## Leads handed to other passes

- L1, L2, L3, L4 to the N1 (directive channel) pass.
- L5 to the N2 (repository policy) pass, with the trigger record's open
  question on plugin-written observations.
- L6 to the S4 (m0 injection) pass.
- The resolved `Entity` writer question to the N3 (scope context) pass.
