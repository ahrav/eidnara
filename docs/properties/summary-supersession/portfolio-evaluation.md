# Portfolio evaluation

## Scope and method

This evaluation covers the 17 records in `docs/properties/summary-supersession/`
at `cab655f2`. I read `catalog.md`, `existing-checks.md`, `fault-map.md`, and
the evidence page for `superseding-claim-is-rendered-whenever-stale-claim-is`.
I did not see the discovery reasoning.

I read the cited test bodies for eight records:

- `claims-block-failure-never-fails-publish`
- `anchor-is-substring-of-stored-p1`
- `claims-column-is-set-once-at-insert`
- `revert-restores-earlier-value`
- `superseding-claim-is-rendered-whenever-stale-claim-is`
- `m0-bytes-change-only-at-hard`
- `correction-visible-before-next-hard`
- `render-cost-bounded-by-rendered-set`

I also read the production code these tests reach: `prepare_claims`, the
lineage copy `INSERT ... SELECT`, `write_seed_history_segment_tx`,
`compose_m1`, and the `PassPlan::Soft` and `PassPlan::Defer` arms of
`transform.rs`.

I ran eight of the cited tests with
`cargo +1.98 nextest run -p daemon -p memory-store --all-features --locked`.
All eight passed.

The Existing check fields are accurate for all eight spot-checked records. The
Exercised fields are accurate for six. The lineage test's `[1, 0, 0]` claim
counts, the source scan's cut at `mod tests {` (after every production
insert), the truncate and recomp assertions, and the `partial`
label on the render-cost record all match the source.

## Findings

| id | lens | class | record(s) | finding | evidence |
| --- | --- | --- | --- | --- | --- |
| F1 | harness fit | refinement, gap | `correction-visible-before-next-hard` | The test calls `arm_soft_refresh` before each SOFT pass. That function has no production caller. The flag it sets overrides a scheduler `Defer`. The record's bound says the m1 revision digest forces the recompose, and no test exercises that mechanism. In production, `PassPlan::Defer` can postpone a pending m1 delta and only logs it. | `transform.rs::a_correction_rides_m1_until_the_next_hard_splices_it_into_m0`; `grep arm_soft_refresh` finds only test callers; the `PassPlan::Defer` and `PassPlan::Soft` arms in `transform.rs` |
| F2 | harness fit | refinement | `m0-bytes-change-only-at-hard`, `correction-visible-before-next-hard` | Exercised says the test "publishes" correcting folds. It writes rows with `replace_history_segments`, so the History Summarizer publish path never runs. | same test body |
| F3 | wildcard | gap | `m0-bytes-change-only-at-hard`, `correction-visible-before-next-hard` | A SOFT plan recomposes m0 and steps `Action::Hard` in two cases: m1 overflow (`body: None` past the row cap of 259) and `soft_pressure_refold`. This is an unscheduled HARD that splices corrections into m0. No test drives either case with claims. | `m1_compose.rs::compose_m1`; `MaterializeReason::PressureRefold` in `transform.rs` |
| F4 | wildcard | gap | `superseding-claim-is-rendered-whenever-stale-claim-is` | The suffix argument requires that legacy rows carry no claims. Only the validator guarantees this. The store accepts a legacy row with claims: the truncate test seeds legacy sequence 1 with a claim. The durable record `hv-publish-accepts-unvalidated-validated-chunk` documents a validation bypass. The evidence page admits that no test stores claims on a legacy row. | `lib.rs::truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`; evidence Q1 |
| F5 | wildcard | gap | `accepted-claim-satisfies-contract-grammar`, `live-claim-is-max-seq-per-key-in-rendered-set` | `prepare_claims` redacts the key on its own. It then builds the pair from the redacted key, so a redacted key passes the pair check and is stored. No check shows that stored keys keep the grammar. No check shows that two distinct keys cannot collapse to one redacted key and supersede each other. I have not verified whether any scanner rule matches the key grammar. | `crates/memory-store/src/lib.rs::prepare_claims` |
| F6 | harness fit | refinement | `claims-block-failure-never-fails-publish` | Required faults lists "a cite outside every accepted segment". The test for that case asserts that the claim is dropped. It does not assert that the chunk equals the no-block chunk. | `history_summarizer_citations_golden.rs::claims_attach_to_the_accepted_segment_their_cite_names` (`short` case) |
| F7 | harness fit | refinement | `apply-corrections-is-a-pure-function-of-body-and-corrections` | The catalog says Exercised `yes`. `fault-map.md` marks the same fault `partial`, because no test runs two hasher seeds. | `fault-map.md` per-property table; `existing-checks.md` quiet areas |
| F8 | harness fit | refinement | `claims-column-is-set-once-at-insert` | The Check calls a forbidden state an `unreachable` source condition. METHOD requires `always(!X)` for a forbidden state. The scan matches literal strings, so a multi-line `UPDATE\n history_segments` would pass it. | `lib.rs::no_production_statement_updates_a_history_segment_row` |
| F9 | wildcard | gap | `revert-restores-earlier-value` | Reachability names the transform's revert paths. The checks run only at the store and compose seams. No test shows the served m0 and the m1 block after a transform-level revert that deletes a correcting row. | `m0_compose.rs::revert_restores_the_earlier_value_and_recomp_renders_no_correction` |
| F10 | coverage balance | bias | all | Sixteen records use `always`. None uses `sometimes` or `reachable`. There are no reachability records. The only `sometimes` candidate, the campaign marker, appears under "Coverage checks to add" and has no record. Surface 4 carries four records. Surfaces 6 and 7 carry one record each. Lineage descent is folded into the anchor record. | index table; `fault-map.md` |
| F11 | coverage balance | bias | all | The records are the specification's 17 obligations. The same milestones wrote the checks. Every record except one is `yes` and `high`. | catalog preamble |

## Disposition proposal

- F1 refinement: change the Check of `correction-visible-before-next-hard` to
  "within the first non-`Defer` pass after the publish, bounded by one m1
  compose". Add the open question: "What bounds the number of consecutive
  `Defer` passes while an m1 delta is pending? (needs human input)". Change
  Exercised to `partial - soft refresh armed by hand; the digest-driven
  recompose and scheduler Defer are not constructed`.
- F1 gap: queue a transform test that publishes a correcting segment without
  calling `arm_soft_refresh`. It runs passes under the default scheduler and
  asserts the block appears within the stated bound. The existing harness
  supports this.
- F2 refinement: in both records, replace "publishes" with "writes through
  `replace_history_segments`" in Exercised.
- F3 gap: queue one transform test with claims under a geometry whose m1 row
  cap overflows, and one under soft pressure. Each asserts that the refold
  splices the live value and that the next SOFT pass keeps m0 frozen. Add
  "SOFT refold and m1 overflow compose m0 as a HARD" to the Fault/timing angle
  of `m0-bytes-change-only-at-hard`.
- F4 gap: queue a store-level `always(!X)` check that no row with `legacy = 1`
  stores a non-empty `claims` cell. The owner may prefer an insert guard. Add
  this precondition to the record's Required faults.
- F5 gap: queue a property test over `prepare_claims`. It asserts that every
  stored key matches the key grammar, or that the claim is dropped. Add an open
  question on whether a redacted key should drop the claim (needs human input).
- F6 refinement: add the outside-cite case to the equality loop, or remove it
  from Required faults.
- F7 refinement: change Exercised to `partial - literal-byte golden per
  process; no two-seed run`.
- F8 refinement: change the Check to `always(!X)` - X is a production
  statement that updates or upserts `history_segments`, and the check scans the
  source. Note the literal-match limitation.
- F9 gap: queue a transform-level revert test with claims on both sides of the
  kept sequence.

## Biases for a human

- The portfolio is framed by the specification. Records restate obligations
  the implementation was built to meet. They do not probe the daemon's other
  mechanisms. F3, F4, and F9 show that the scheduler, refold, legacy rows, and
  the transform revert are thinly examined.
- The checks were written by the same effort that wrote the records. Uniform
  `yes` and `high` labels therefore reflect self-certification, not
  independent adequacy. Every check is `unaudited`, and
  `/testing:invariant-test-review` has not run.
- Every record uses `always`, and the only liveness record rests on a test
  that forces its own trigger (F1). The portfolio has no situation coverage
  showing that corrections occur in campaigns.

## Disposition

The evaluation above ran at `cab655f2`. The findings were dispositioned as
follows; the edits are in the records, `fault-map.md`, and the code at
`0231f2f4` and `1c30058c`.

| id | Class | Disposition |
| --- | --- | --- |
| F1 | refinement, gap | Applied: `correction-visible-before-next-hard` is `partial`, its Check is bounded by the first non-`Defer` pass, and it carries the `Defer` open question. The scheduler-driven test is queued in `fault-map.md`. One premise is corrected: `arm_soft_refresh` has one production caller, the operator `session.flush` request (`handle_session_flush_value` in `crates/daemon/src/lib.rs`), but no publish or correction path calls it, so the finding stands. |
| F2 | refinement | Applied: both records say the test writes through `replace_history_segments`. |
| F3 | gap | Queued in `fault-map.md`; the overflow and pressure-refold HARDs are named in the Fault/timing angle of `m0-bytes-change-only-at-hard`, which is now `partial`. |
| F4 | gap | Closed in code: `prepare_history_segment` keeps no claims on a legacy row (`a_legacy_row_stores_no_claims`), and the suffix test asserts the premise and gained a negative control. |
| F5 | gap | Closed in code: `prepare_claims` drops a claim whose key the scanner rewrites, exercised in `history_segment_content_redacts_and_new_message_identities_reject`. The grammar property over `prepare_claims` is queued. |
| F6 | refinement | Applied: `claims_attach_to_the_accepted_segment_their_cite_names` compares the outside-cite chunk with the chunk without it. |
| F7 | refinement | Applied: `apply-corrections-is-a-pure-function-of-body-and-corrections` is `partial`. |
| F8 | refinement | Applied: the Check is `always(!X)` and names the literal-match limit; the scan now collapses whitespace. |
| F9 | gap | Queued in `fault-map.md`; `revert-restores-earlier-value` is `partial`. |
| F10 | bias | Surfaced under Biases for a human and in the pull request. |
| F11 | bias | Surfaced under Biases for a human and in the pull request. |

Surfaced to a human: the records restate obligations the same effort
implemented and checked, every check is `unaudited`, and no campaign shows
corrections occurring. `/testing:invariant-test-review` is the next audit.
