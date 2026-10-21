# packing-required-phase-is-a-prefix-ending-at-first-overbudget

## Discovery trigger

Plan KTD5 ("the first `OverBudget` stops the phase (skip-and-continue in
the required phase would be a hidden priority inversion)") and acceptance
example AE7 ("rules are served in order until the first that does not
fit"). Serves R10 and KTD5; sibling of
`packing-required-phase-precedes-optional-work` and
`packing-required-bytes-are-charged-untruncated` in
`docs/properties/selection-packing/required/catalog.md:57, 178`.

## Evidence trail

- `crates/retrieval/src/packing/required.rs:176-205`: `reserve_required`
  accumulates `charged` and returns `Err(OverBudget { limit, charged })` at
  the first item that crosses `token_limit`; the `Ok` branch returns costs
  for the whole set. There is no partial-success return. A single call over
  all required rows therefore reserves nothing when any row overflows.
- `crates/retrieval/src/packing/required.rs:114-174`: `admit_required`
  operates on `RequiredFact { request: RequiredRequest { occurrence:
  OccurrenceId, revision }, row: &SelectedOccurrence, disposition }` and
  refuses `Hidden`, `Stale`, `Superseded`, other `PolicyExcluded`, and
  oversize items as `Err`. These are retrieval occurrence types, not memory
  decision rows.
- `crates/retrieval/src/packing/scan.rs:18-43`: `skip_and_continue` records
  skips and keeps going; it is the optional-phase primitive and the
  forbidden required-phase primitive.
- `crates/daemon/src/m0_compose.rs:74-111`: today's single-phase trim is
  `skip_and_continue` over positive rows in serving order, so the current
  behavior is the one the plan forbids for rules.
- `crates/retrieval/AGENTS.md:52-60, 66-68`: the required module is
  described as pure and store-free; `scan.rs` is named as the rule "both the
  packer and the daemon's memory trim use". The plan cites `:71-74`; at HEAD
  the relevant sentences sit at 66-68 (line reference drift noted, not a
  content disagreement).

## Failure scenario

Legal witness: nine required rows with costs `[300, 300, 300, 300, 300, 300,
900, 100, 100]`, limit 1600. Served prefix: the first five (1500); the sixth
(300) crosses 1600; phase ends; `rules_omitted = 4`. Rows 8 and 9 (100 each)
are omitted even though they fit.

Smallest violating traces:
1. Skip-and-continue: the packer uses `skip_and_continue` for required rows;
   it skips row 6 and admits rows 8 and 9. Two lower-ranked rules displace
   none directly, but under a different cost vector `[1500, 200, 100]` with
   limit 1600, skipping row 2 (200) to admit row 3 (100) serves a
   lower-ranked rule over a higher-ranked one. The disclosure count says 1
   omitted and does not say which, so the inversion is invisible.
2. All-or-nothing: the packer passes the whole ordered list to
   `reserve_required` once; `Err(OverBudget)` is treated as "no rules fit";
   zero rules served, `rules_omitted = 9`, and the minimum-service rule is
   not applied. AE7 fails.
3. Omitted rows leak into the optional phase: omitted required rows are
   appended to the optional list; row 8 is then served by skip-and-continue
   in the optional phase ahead of ordinary memories, which re-creates the
   inversion through the back door.

## Timing windows and dependencies

- None inside a pass.
- Depends on `packing-required-order-is-total-and-identical-across-passes`
  (the prefix is of that order) and on
  `packing-reserve-is-computed-once-after-wrapper-and-disclosure` (the
  limit).
- The exception is `packing-first-rule-is-served-when-it-fits-budget-minus-
  wrapper`, which applies only at index 0.

## What a test must construct

1. A pure packer `pack_required(ordered, limit, cost) -> (served_count,
   charged)` with the nine-row fixture above; assert `served_count == 5`.
2. The `[1500, 200, 100]` fixture with limit 1600: assert `served_count ==
   1` (not 2).
3. Assert the required loop contains no skip branch: a code-level marker is
   impossible to assert from a test, so pin it structurally by asserting that
   for every fixture, `served` is a prefix of `ordered` (no gaps): for all
   `i < served_count`, `ordered[i]` is served, and for all `i >=
   served_count`, `ordered[i]` is not served and does not appear in the
   optional phase's admitted list.
4. If `reserve_required` is reused, a test that the adapter calls it per
   prefix or interprets `OverBudget.charged - cost(last)` to recover the
   prefix; otherwise a test on the memory-specific loop.
5. Integration: nine seeded rules plus five ordinary memories; the block
   contains the five rules, `<omitted-rules count="4"/>`, and ordinary
   memories in the remainder.

## Investigation log

### Q: Can `admit_required`/`reserve_required` be used as the plan states?
- Sources examined: `required.rs:1-205`; `packing/mod.rs:119-150`
  (`SelectedOccurrence`).
- Findings: the primitives need `SelectedOccurrence` (occurrence id, class,
  representation, span, payload ref, provenance, tombstone, grouping) and a
  `Disposition`. Memory rows are `VisibleRow` plus `DecisionRow`. An adapter
  would synthesize occurrence facts for decisions. The reservation is
  all-or-nothing by contract (`OverBudget` is an `Err`).
- Missing evidence: U2's adaptation design.
- Conclusion: resolvable at implementation - the plan's "reuse" claim is
  partial; either call per prefix (quadratic but bounded by the rule count)
  or implement a memory-specific prefix loop over `TokenCount`; both satisfy
  this record. Recorded in `open-questions.md`.
