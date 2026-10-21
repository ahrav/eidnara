# packing-required-order-is-total-and-identical-across-passes

## Discovery trigger

Plan requirement R10 ("orders them by authority channel (`user_directive`
first), then scope specificity, then recency, then object id") and R13
(identical rule object ids across harnesses). Serves KTD5 and KTD8; U2 test
scenario "Order is stable across passes for the same rows".

## Evidence trail

- `crates/kernel/src/envelope.rs:152-158`: `ObjectRow` carries `object_id`,
  `source_kind`, `source_revision`, `created_commit_seq`; the inputs of the
  order are available on every visible row.
- `crates/daemon/src/kernel_routes/read.rs:102-111`: today's serving order
  is `created_commit_seq` desc then `object_id` asc (`ServingOrderLast`);
  the new order appends two keys in front and keeps these two as the tail.
- `crates/daemon/src/kernel_routes/read.rs:239-243`: `decisions` is a
  `HashMap<String, DecisionRow>`; iteration over it is unordered, so the
  packer must iterate `rows` (ordered) and look up decisions, never the map.
- `crates/daemon/src/canonical_memory.rs:189-199`: the current `injectable`
  list is built by iterating `rows` in serving order, which is the pattern
  to keep.
- `crates/daemon/src/memory_render.rs:85-109`: the renderer sorts by
  category rank then object id; render position is independent of packing
  order. The plan's "render before" scenario is an open question recorded in
  `open-questions.md`.
- `docs/properties/daemon/decisions/catalog.md:1014`:
  `dec-a-selection-decision-order-is-total-under-hashmap-iteration` is the
  precedent for this class of obligation elsewhere in the daemon.

## Failure scenario

Legal witness: rows D1 (`user_directive`, specificity 0, seq 10), D2
(`user_directive`, `dir:crates` matched, specificity (1, depth 1), seq 5),
R1 (`repository_policy`, `dir:crates/storage` matched, (1, depth 2), seq
20), R2 (`repository_policy`, specificity 0, seq 21). Order: D2, D1, R1, R2.
Two passes, either harness, produce the same sequence and, under a limit
admitting three, the same served prefix `[D2, D1, R1]`.

Smallest violating traces:
1. HashMap leak: the required list is built by iterating `decisions`
   (a `HashMap`). Two passes produce `[D1, D2, ...]` and `[D2, D1, ...]`;
   with a limit admitting one, D1 serves on one pass and D2 on the next.
   Parity across harnesses fails nondeterministically.
2. Ties unresolved: the comparator orders by rank and seq only; D1 and a
   second directive from the same commit (equal seq) compare `Equal`, and
   `sort_by` stability depends on input order, which depends on the read.
3. Specificity computed from the stored scope instead of from the matched
   context: a rule with a `dir:` term is counted specific even when the
   term was matched by `.` only; the plan's "count of non-Project terms
   matched" is about the match, not the scope's shape.

## Timing windows and dependencies

- None within a pass. Across passes, a new commit changes seq values and
  may reorder; this is legal because the inputs changed.
- Depends on `scope-rule-serves-only-when-every-carried-term-matches` for
  the specificity inputs.

## What a test must construct

1. A pure comparator `required_order(a, b, ctx) -> Ordering` with a property
   test over generated rows: antisymmetry, transitivity, and `Equal` only
   for equal object ids.
2. A table test: `user_directive` with specificity 0 and newer seq precedes
   `repository_policy` with specificity (1, 3) (channel outranks
   specificity); within one channel, (1, 2) precedes (1, 1) precedes (0, 0);
   equal specificity resolves by seq desc; equal seq by object id asc.
3. Two packs of the same `ReadResponse` and context produce identical
   served sequences; shuffle the `decisions` map insertion order between the
   two runs to expose a HashMap dependence.
4. A `sometimes` marker for the campaign: a pass where a `user_directive`
   row older than a `repository_policy` row is served first.

## Investigation log

### Q: Is specificity a function of the stored scope or of the match?
- Sources examined: plan KTD5 ("count of non-Project terms matched, then
  `dir:` depth").
- Findings: "matched" means the terms that matched the context; since a
  rule is served only when every term matched, the count equals the number
  of non-Project terms present. Depth is the depth of the `dir:` value on
  the rule, not of the ancestor that matched it (they are equal under
  `Exact`; under `Set`, the matched member's depth is the natural reading).
- Missing evidence: the `Set` case decision.
- Conclusion: resolvable at implementation; pin depth as the matched
  member's depth for `Set` terms or the maximum member depth, and test both
  readings against the fixture.
