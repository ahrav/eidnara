# cc-scorer-output-deterministic-under-contract-version

Verified at the #1164 branch tip (base `979fb814e`, scorer commits
`05acd0107` and `446c7363f`). Surface S-A.

## Discovery trigger

The domain-modeling pass at `3a3b916b7` proposed this record for plan unit
U1: the candidate table is a derived cache that must rebuild to the same
rows, so the scorer's top K cannot depend on SQL row order or hash
iteration. #1163 D1 and C4 fix the total order; #1164 implements it.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/scorer.rs:102-110`
  `term_set` collects folded, floored terms into a `BTreeSet<String>`, so a
  text's terms have one order and duplicates collapse.
- `scorer.rs:139-148` `ScoredCandidate::rank`: score descending by
  cross-multiplying `matched_terms` and `decision_terms` (no floating point),
  then `matched_terms` descending, `decision_terms` ascending, decision
  object id ascending, decision id ascending. `String` comparison is byte
  order, so `rank` is a total order over distinct (object id, decision id)
  pairs.
- `scorer.rs:217-233` `TopCandidates::record` scores one summary and offers
  a passing one; `:235-250` `offer` replaces a kept entry with the same
  decision object id only when the new one ranks better, inserts at
  `partition_point`, and truncates to `CANDIDATES_PER_DESCRIPTOR` (`:62`,
  K = 4).
- `scorer.rs:271-288` `score_descriptor` feeds every candidate through
  `record`, collects unscorable ids, and sorts them (`sort_unstable` on
  `String`, a total order, so equal ids are indistinguishable).
- `scorer.rs:292-327` `matcher_preimage` names `ORDER_RULE_ID` (`:79-80`);
  `:338-345` the digest is the SHA-256 of the preimage, held in a
  `LazyLock`.
- Tests: `crates/daemon/tests/contradiction_candidates_scorer.rs:276-318`
  `the_top_k_is_the_same_under_every_input_order`;
  `:320-346`
  `a_decision_object_id_appears_once_and_the_better_entry_wins_in_any_order`;
  `:348-400` `the_matcher_digest_pins_every_rule` asserts the whole
  preimage, including the order rule id, and the digest literal.

## Failure scenario

If `offer` kept the first of two equal-rank entries, or truncated before
inserting, the K-th candidate would follow input order. If terms were a
`HashSet` and a later step iterated it, output could vary between
processes. If the unscorable list kept input order, two rebuilds from
differently ordered reads would report different lists. Any of these makes
the derived table depend on the Kernel read order.

## Timing windows and dependencies

None. The function is pure: it reads only its arguments and the constant
`LazyLock` digest, which `score_descriptor` does not consult.

## What a test must construct

- More than K passing summaries with ties on score and `matched_terms`
  across the K-th position (present: five 4-of-4 ties and a 6-of-6 for
  K = 4).
- A summary with a lower score but more matched terms than the ties
  (present: `lower-score`, 5 of 6), so the order is score first.
- An unscorable summary inside the permuted list (present).
- Several input orders (present: nine rotations and each reversal).
- Two entries for one decision object id in both orders (present).
- A repeated decision object id inside a list with more than K passing
  summaries, over many orders (absent; the 18-order test has no repeated
  id). A property test over random permutations of such a list is the
  stronger oracle.

## Investigation log

### Q: Does the order depend on anything outside the inputs?

- Sources examined: `scorer.rs:102-110`, `:139-148`, `:217-250`,
  `:271-288`.
- Findings: no hash container, clock, global, or I/O reaches the result.
  `rank` ignores the summary and ends at the decision id, so two entries
  tie only when their ids and counts are equal, and such entries give
  equal outputs; `offer` keeps one entry per object id.
- Missing evidence: a second-thread or second-process run; the code has no
  state that would make one differ.
- Conclusion: resolved with answer: the order is a function of the inputs
  and the matcher constants.

### Q: Which tie-break order applies? (originally needs human input)

- Sources examined: #1163 D1 and C4, #1164 Scope, `scorer.rs:139-148`.
- Findings: the specification fixes score desc, `matched_terms` desc,
  `decision_terms` asc, object id asc; the code adds decision id asc and
  one entry per object id, both named in the order rule id.
- Missing evidence: none.
- Conclusion: resolved with answer: the specification's order, extended to a
  total order by decision id.
