# Fault map: P5 proposed obligations

Verified at the #1164 branch tip (base `979fb814e`) for the S-A rows; the
read and confirm rows (R1 to R5) are verified at the #1168 tip (base
`cda655423`). The other records list their faults under "Required faults and
enabling state" and name the tests that construct them.

## Fault classes and availability

| Fault class | Available | Mechanism |
| --- | --- | --- |
| A1 input permutation of the candidate list | yes | reorder the `CandidateInput` slice (`contradiction_candidates_scorer.rs:276-318`) |
| A2 tie on score and `matched_terms` across position K | yes | identical summaries under distinct ids |
| A3 repeated decision object id | yes | two `CandidateInput` values with one id (`:320-346`); no test combines it with more than K passing summaries |
| A4 descriptor one byte over or exactly at the byte bound | yes | `"a ".repeat(n)` (`:224-241`) |
| A5 summary one byte over or at the byte bound | yes | padded summary (`:243-274`) |
| A6 input over `max_atoms` within the byte bound | no | unconstructible: `max_atoms = max_input_bytes / 2 + 1` (`scorer.rs:85-96`) |
| A7 floor-emptied summary | yes | short, numeric, or stopword-only text |
| A8 case-only, non-ASCII case, length-changing lowercase, case change across a part boundary | yes | literal strings (`:31-80`, `:131-159`, `:163-171` for `configLoader` and `configloader`) |
| A9 diacritic-only and NFC versus NFD spellings | yes | literal strings (`:131-159`) |
| A10 second thread or process | yes, unused | spawn a thread in the test; no test does |
| A11 toolchain Unicode change | no in-tree | build under another toolchain; the digest pin fails by design |
| R1 evidence retired between list and confirm | yes | `expire_harness_captures` past the retention (`candidate_pass_tests.rs:1945`) |
| R2 Kernel starting or failed at read | yes | no open, or an open at a file path (`candidate_pass_tests.rs:1877`) |
| R3 Memory Store absent | yes | a handler built without a store (`candidate_pass_tests.rs:1877`) |
| R4 candidate table hidden | yes | `ALTER TABLE ... RENAME` on `store.db` (`candidate_pass_tests.rs:1485`, `:2069`) |
| R5 authority lookup fails | no | no seam (dependency fault F18) |

## Per-property required faults

| Record | Required | Present in tests |
| --- | --- | --- |
| `cc-scorer-output-deterministic-under-contract-version` | A1, A2, A3, A7 | A1, A2, A3, A7; A10 absent |
| `cc-oversize-text-refused-whole-never-truncated` | A4, A5, A7 | A4, A5, A7; A6 unconstructible |
| `cc-case-fold-applied-identically-to-both-sides` | A8, A9 | A8, A9 |
| `cc-read-serves-only-live-decision-rows-ids-and-scores-only` | R1 | R1: a row whose evidence is retired at the read is seeded directly (`candidate_pass_tests.rs:1485`) and retired by the sweep after a listing (`:1945`) |
| `cc-read-distinguishes-disabled-unavailable-and-empty` | R2, R3, R4, R5 | R2, R3 (`candidate_pass_tests.rs:1877`), R4 (`:1485`); R5 has no seam |
| `cc-flag-off-no-pass-no-write-no-table-read` | R4 | R4 (`candidate_pass_tests.rs:1485`, `:2069`) |
| `cc-candidate-carries-dispute-ids-evidence-liveness-checked-at-confirm` | R1 | R1 (`candidate_pass_tests.rs:1945`) |

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
