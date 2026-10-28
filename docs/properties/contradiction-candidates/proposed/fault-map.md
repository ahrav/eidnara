# Fault map: P5 proposed obligations

Verified at the #1164 branch tip (base `979fb814e`). This map covers the
three active S-A records. The 12 proposed records need the faults their
catalog entries list under "Required faults and enabling state"; their
availability is assessed when #1165 to #1168 land the code.

## Fault classes and availability

| Fault class | Available | Mechanism |
| --- | --- | --- |
| A1 input permutation of the candidate list | yes | reorder the `CandidateInput` slice (`contradiction_candidates_scorer.rs:276-318`) |
| A2 tie on score and `matched_terms` across position K | yes | identical summaries under distinct ids |
| A3 repeated decision object id | yes | two `CandidateInput` values with one id (`:320-346`); no test combines it with more than K passing summaries |
| A4 descriptor one byte over or exactly at the byte bound | yes | `"a ".repeat(n)` (`:224-241`) |
| A5 summary one byte over or at the byte bound | yes | padded summary (`:243-274`) |
| A6 input over `max_atoms` within the byte bound | no | unconstructible: `max_atoms = max_input_bytes / 2 + 1` (`scorer.rs:88-99`) |
| A7 floor-emptied summary | yes | short, numeric, or stopword-only text |
| A8 case-only, non-ASCII case, length-changing lowercase, case change across a part boundary | yes | literal strings (`:31-80`, `:131-159`, `:163-171` for `configLoader` and `configloader`) |
| A9 diacritic-only and NFC versus NFD spellings | yes | literal strings (`:131-159`) |
| A10 second thread or process | yes, unused | spawn a thread in the test; no test does |
| A11 toolchain Unicode change | no in-tree | build under another toolchain; the digest pin fails by design |

## Per-property required faults

| Record | Required | Present in tests |
| --- | --- | --- |
| `cc-scorer-output-deterministic-under-contract-version` | A1, A2, A3, A7 | A1, A2, A3, A7; A10 absent |
| `cc-oversize-text-refused-whole-never-truncated` | A4, A5, A7 | A4, A5, A7; A6 unconstructible |
| `cc-case-fold-applied-identically-to-both-sides` | A8, A9 | A8, A9 |

## Coverage checks to add

These assert enabling preconditions, never the violation.

- `sometimes(top_k_ties_cross_position_k)`: a campaign scores at least one
  descriptor whose passing summaries tie on score and `matched_terms` at
  position K. Precondition for the determinism record.
- `sometimes(summary_unscorable_beside_scored)`: at least one descriptor has
  an unscorable summary and at least one candidate. Precondition for the
  oversize record's "the others still score" clause.
- `sometimes(fold_changes_term)`: at least one term where
  `to_lowercase(t) != t` on either side. Precondition for the case-fold
  record.

The pass (#1167) is the natural place for these markers; the scorer has no
campaign of its own.

## Leverage ranking (cheapest valid oracle first)

1. Equality of `score_descriptor` output under permutation: a pure-function
   comparison with no fixture beyond strings (present).
2. `Err` versus `Ok` at the exact bound: two inputs (present).
3. Term-set equality under the fold: hand-written sets (present).
4. A second-thread run: low value given no shared mutable state.
5. The pass-arm counters for oversize descriptors: needs #1167.
