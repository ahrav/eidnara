# Cited capture anchors: fault map

Fault classes come from the specification's fault map
([#1130](https://github.com/ahrav/eidnara/issues/1130), Fault and state
coverage). Availability names what can construct the fault at the branch
that adds this part.

## Fault classes

| Id | Fault | Availability |
| --- | --- | --- |
| F1 | Malformed anchor row (unknown kind, bad oid, unowned column, oversized, duplicate captures) | constructible: `AnchorRowSpec` fixtures for the engine; `validate_anchor_row` for raw rows; the typed writer refuses every shape it can represent |
| F2 | HEAD not a descendant of the anchored commit | constructible: git fixtures in `crates/kernel/tests/support/git_fixtures.rs` |
| F3 | Absent object, partial clone, shallow boundary | constructible: anchor-resolution fixtures |
| F4 | Evaluation budget exhaustion or interrupt | constructible: `EvalBudget` in tests |
| F5 | HEAD moves during snapshot, or between a capture submit and its retry | constructible: engine tests that move HEAD between batches; the U2 route test commits between submit and retry |
| F6 | Retired or unknown parent row | constructible: retire a parent, or insert it later in one envelope |
| F7 | Operation-key reuse with different bytes | constructible: Kernel envelope tests; test-only for capture (refinement R7) |
| F8 | Kernel writer held past 5 s | constructible: hold the writer from a test thread |
| F9 | Model output invents or omits a source id or quotes foreign text | constructible: memory-capture unit tests |
| F10 | Session delete interleaved with checkpoint or publish | constructible: gate-holding daemon tests |
| F11 | Blocking hop failure | constructible: daemon route tests |
| F12 | Source-capture slot busy, budget spent, cancelled, Kernel starting | constructible: source-capture test module |
| F13 | Secret-bearing text | constructible: known secret fixtures |
| F14 | Unsupported object format, oversized delta base, replacement refs; unborn and non-repository roots | constructible: git-source tests and the U2 head-observation fixtures (`crates/daemon/src/memory_capture/head.rs:139`) |
| F15 | Harness capture past `retain_until` while a decision cites it | constructible: capture citation events since U3 (`crates/daemon/src/source_capture_tests.rs:3796`) |
| F16 | Fragment id differs from descriptor message id (hashed, chunked, multi-block) | production: messages over 16 KiB or out-of-grammar ids |

## Per-property required faults

| Record | Faults |
| --- | --- |
| `p2-undecodable-or-oversized-anchor-row-is-uncertain` | F1 |
| `p2-reachable-from-holds-on-ancestor-of-head` | F2 |
| `p2-reachable-from-negative-needs-complete-evidence` | F3, F4 |
| `p2-anchor-verdict-cache-key-covers-row-head-and-boundary` | F5, F3 |
| `p2-decision-parents-must-be-live-at-insert` | F6 |
| `p2-citing-decision-carries-at-least-its-evidence-class` | a restrictive evidence class only |
| `p2-cited-evidence-is-never-retired-or-swept` | F15 |
| `p2-capture-commit-replays-by-job-key-and-frozen-digest` | F7, F11 |
| `p2-capture-quote-is-an-exact-substring-of-its-own-batch-source` | F9 |
| `p2-capture-job-and-decision-identity-derive-from-the-source-tuple` | F13, F16 |
| `p2-capture-publish-and-session-delete-are-serialized` | F10 |
| `p2-messages-descriptor-identity-is-per-block-and-evidence-id-is-the-receipt-key` | F16 |
| `p2-deferred-source-units-publish-on-a-later-pass` | F12 |
| `p2-git-open-is-isolated-and-allocation-bounded` | F14 |
| `p2-new-anchor-row-decodes-on-write` | F1, F13 |
| `p2-new-anchor-and-registry-share-one-envelope` | F6, plus a refusal after the anchor insert in one envelope |
| `p2-new-capture-anchor-identity-is-deterministic` | F11 (rolled-back commit then retry) |
| `p2-new-head-observed-once-and-frozen` | F5 (checkout between submit and retry) |
| `p2-new-head-unavailable-keeps-decision-admitted` | F14 |
| `p2-new-citation-is-alias-only` | F9, F16 |
| `p2-new-citation-block-target-rule` | F16 |
| `p2-new-citation-folds-sensitivity-and-pins-evidence` | F15 |
| `p2-new-settlement-wait-is-bounded-and-gate-free` | F12, F8 |
| `p2-new-lookup-runs-only-after-settlement` | F12 |
| `p2-new-miss-reason-matches-outcome` | F12, F14, F16 |
| `p2-new-miss-counters-track-rationale` | F11 |
| `p2-new-anchor-and-citation-add-no-authority` | F12 |
| `p2-new-cited-decision-pair-observed` | F12, F14 |

## Coverage checks to add

Each check asserts the preconditions of a window and stays silent on the
violation.

- `sometimes(decision_insert_named_an_anchor_id)`: a production
  `insert_decision` carries `anchor_id: Some`. The marker gates the anchor
  clause of `p2-decision-parents-must-be-live-at-insert` (refinement R1); it
  fires on every anchored capture publish since U3.
- `sometimes(anchor_insert_then_refusal_in_one_envelope)`: one envelope
  records an `insert_anchor` success followed by a refused write. Constructed
  today by `a_decision_failure_after_the_anchor_rolls_back_both`.
- `sometimes(settlement_running_at_submit)`: a submit observes a running
  settlement entry. Constructed since U4 by
  `a_submit_during_the_publish_waits_outside_the_gate_and_then_cites`, which
  counts the armed wait.
- `sometimes(hashed_fragment_id_at_lookup)`: a citation lookup runs for a
  hashed `message_id`. Constructed since U3 by
  `misses_carry_one_closed_reason_each_and_the_counters_follow_them`.
- `sometimes(one_message_id_in_two_sessions)`: two sessions publish one
  message id before a capture of one of them is cited. Queued; no test
  constructs it yet.

## Ranking by oracle cost

Cheapest valid oracle first:

1. Kernel store commit with an envelope closure (seam 1) for the N1 records,
   the parent clause, and the fold: in-process, with a temporary store and
   no repository or daemon.
2. Raw-row validation through `validate_anchor_row` for F1 shapes the typed
   writer cannot represent.
3. Applicability engine over a git fixture (seam 2) for the S1 records and
   the U3 `Current` checks.
4. Daemon management routes (seam 3) for S3, N2, N3, N5, N6.
5. Settlement interleavings under a paused clock (seam 5) for N4.
