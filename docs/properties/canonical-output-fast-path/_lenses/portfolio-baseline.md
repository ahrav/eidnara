# Initial portfolio evaluation trace

## Authority and scope

This file records the initial independent agent evaluation and its synthesis
dispositions. The main agent passed that evaluation to this writer; it is not
a user-originated claim or an evaluation performed by the writer. At initial
synthesis, final evaluation had not run and this writer could not spawn it
because subagent depth was blocked. The separate final evaluation is now
[complete](../portfolio-evaluation.md), without runtime test evidence.

The user supplies the settled [plan][plan] and three discovery lanes as the
evidence scope. No additional incidents were supplied, and no fictional
“none” interview answer is recorded. The main agent fetched GitHub #350 and
#441 on 2026-09-13 and reports both OPEN with no comments. Those lookups are
boundary provenance only, not a new scope or evidence of delivered work.

Source refinements below are checked through `git show HEAD:<path>` at
`2e4433e6b511ae74944df8a9669c428e73915d29`, using colgrep for discovery leads.
The plan's `4980f8af3bb90d58b19b80a38a227fb6363a8b33` baseline is separate.
No runtime run, candidate measurement, or adequacy verdict is imported.

## Findings and dispositions

| Initial finding | Class | Verified or assumed | Disposition |
| --- | --- | --- | --- |
| Allocation slope cannot prove removal of fixed B. | gap | Verified: the [allocation check][alloc] subtracts two event totals; [encode][encode] allocates B once per call. | S5 requires absolute allocation/size attribution, A lifetime identity, and copy accounting. Baseline always-copy is a proposed negative control, not a retained production variant. |
| A process-global allocator can count harness work. | refinement | Verified: the existing allocator counts every alloc/realloc without owner gating. No isolation run was performed. | S5 specifies nonallocating owner-thread recording and explicit filtered libtest/nextest commands. A mutex or serial tests alone is insufficient. |
| Local canonicalizer and full-constructor benchmark cells are missing. | gap | Verified in the scoped HEAD benchmark inventory; [hot_path][bench] provides transform-level cold/warm cells. | Extend existing test-only entry points and benchmark files for U0/U4. No new production API. |
| Existing direct_host support is not a user-visible latency benchmark. | refinement | Verified: [request_json][direct] is a request seam; [ipc_budget][echo] uses echo. | A retained real-transform host driver is required only for a user-visible latency claim, including serve_native and correlated terminal receipt. |
| Fixture counts are not evidence of real population shape. | bias | 100/1,000-message [constants][counts] are verified; representativeness and miss frequencies are unmeasured assumptions. | Preserve controlled points and report populations. Do not call them empirically representative. |
| `Value` canonical oracle depends on features. | refinement | Verified manifest request is raw_value; no resolved feature graph was captured. The [sorter][sort] defines decoded-string order independently. | Keep literals and emitted-key permutation oracles. Reject requiring preserve_order off or using original presence as authorization. |
| Unchanged-copy identity needs its own bridge. | gap | Verified: no direct comparison of unchanged real tables with A is present in scoped tests. | S3 adds the lemma and a private real-encode observation. Do not duplicate setup or require a giant geometry validator. |
| Tests being present does not mean candidates were exercised. | refinement | Verified: this pass executes no runtime tests. | All nine records say not yet; all existing checks remain unaudited. |
| Boundary diagnostics need path-specific comparisons. | refinement | Verified in [write/count/error mapping][dispatch]. | S7 fixes variant, cap, measurement/write partitions. Preserve Write versus Serialize and first-crossing lengths. |
| Broad owners already exist. | refinement | B1/T1–T4 and invalidated W1/W2 were inspected. | Link owners without rewriting or reactivating them. T3/T4 are not delivered or exercised; measurement remains with the plan. |

## Remaining baseline gaps

This list preserves the initial implementation and evidence gaps, not a claim
of outstanding final discovery blockers. The final evaluation resolves the
full-constructor seam route through private in-crate tests; U0 must still
implement and verify the observer before U1.

- S1–S5 need implemented observations and candidate execution; S5 needs a
  noncontaminating allocation ledger, and full-constructor residency remains
  unmeasured through receipts, hashing, and Arc conversion.
- S6 needs a scoped independent constructor/encoder observer, excluding the
  test-only fresh differential, and pointer checks on positive hits.
- C1/C2 need reusable accumulated situation checks, not isolated sample names.
- S7 needs missing exact diagnostic assertions, not a new transport campaign.
- Before/after measurement and the plan's implementation gates remain undone.
  No source, tests, CI, tracker, or historical catalog are changed here.

## Handoff

The [catalog](../catalog.md) turns these findings into seven safety records
and two situation records. Each goes to `/testing:test-strategy`. Existing
tests go to `/testing:invariant-test-review`; production guards go separately
to `/low-level-systems:defensive-assertions-and-invariant-guards`.

This trace preserves the first evaluation rather than attributing later work
to it. Final independent analyst task `ses_f675203e4ffevgEH7CseOxb6jK` accepts
the portfolio with no blockers; its eight findings and dispositions are in
the [final report](../portfolio-evaluation.md).

The final source pass also verifies [locked serde_json 1.0.151][serde-lock]
with dependencies `itoa`, `memchr`, `serde`, `serde_core`, and `zmij`, but no
`indexmap`. Alongside the manifest, this supports the inspected locked map
assumption, not a runtime-captured feature graph. The algorithm remains
feature-independent. Neither evaluation supplies runtime or peak measurements.

[plan]: ../../../plans/2026-09-13-0030-perf-canonical-output-direct-frame-plan.md
[alloc]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/served_json_passthrough_allocations.rs#L10-L86
[encode]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L121-L142
[sort]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/served_json.rs#L144-L164
[bench]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/benches/hot_path.rs#L320-L397
[counts]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/benches/hot_path.rs#L33-L35
[direct]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/tests/support/direct_host.rs#L359-L371
[echo]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/host-runtime/benches/ipc_budget.rs#L24-L28
[dispatch]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/crates/daemon/src/dispatch.rs#L237-L408
[serde-lock]: https://github.com/ahrav/eidnara/blob/2e4433e6b511ae74944df8a9669c428e73915d29/Cargo.lock#L2828-L2839
