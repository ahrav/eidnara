# Cited capture anchors: portfolio evaluation

## Source of this evaluation

The specification ([#1130](https://github.com/ahrav/eidnara/issues/1130),
Portfolio evaluation) records a fresh-context evaluation over the 14
existing-behavior records through four lenses: harness fit, coverage balance,
implementability, and wildcard. It re-verified about 90 `path:line`
references and every cited test name at `a0e3f2993`, invalidated no record,
and returned 10 refinements, 7 gaps, and 4 biases. The evaluation's working
files are not durable (specification N5), so this file restates its findings
and records their disposition as applied by the U1 ticket
([#1133](https://github.com/ahrav/eidnara/issues/1133)). The evaluation did
not cover the 14 proposed obligations; they enter as designed records with
falsifiers.

## Refinements

The specification lists the refinements inline; the numbering below follows
that order and splits the fixture-rows item into its two tables.

| Id | Refinement | Disposition |
| --- | --- | --- |
| R1 | Relabel the `anchor_id` clause of `p2-decision-parents-must-be-live-at-insert` as `always-or-unreached` with a `sometimes(decision_insert_named_an_anchor_id)` precondition marker | applied in `catalog.md` and `fault-map.md`; U3 reverses it when production passes an anchor id |
| R2 | A raw-SQL anchor fixture needs its `object_registry` row | applied: the evidence for the S1 and N1 records states it; `insert_anchor` now writes the registry row |
| R3 | A raw-SQL anchor fixture needs its `commit_log` row | applied: as R2; the envelope supplies the commit row |
| R4 | Tighten the cache-key check to "differ in exactly one key component" | applied to `p2-anchor-verdict-cache-key-covers-row-head-and-boundary` |
| R5 | Give the serialization record a gate-hold observation method | applied to `p2-capture-publish-and-session-delete-are-serialized` |
| R6 | Narrow the deferred-unit record to "can publish from the memo's `Pending` entry" | applied to `p2-deferred-source-units-publish-on-a-later-pass` |
| R7 | Mark the receipt-conflict coverage marker test-only | applied to `p2-capture-commit-replays-by-job-key-and-frozen-digest` |
| R8 | Fix the offset `memory_capture.rs:126` | applied: the prompt's quote rule is at `crates/daemon/src/memory_capture.rs:133` at the branch tip |
| R9 | Fix the offset `anchor.rs:329` | applied: references to the decoder use `crates/kernel/src/anchor.rs:273` (`AnchorCondition::decode`) and the interval check inside it |
| R10 | Fix the offset `source_capture.rs:598` | applied: `into_job` is `crates/daemon/src/source_capture.rs:632` and `CAPTURE_BUDGET` is `:35` at the branch tip |

Line drift between `a0e3f2993` and the branch tip moved several daemon
references: the `crates/daemon/src/lib.rs` capture tests by 31 lines and
`harness_capture_expiry_keeps_held_and_cited_evidence_and_drains_in_pages`
from line 3331 to 3482. Each record cites the current line; the test names
match the specification's.

The per-field capture secret scan moved Kernel references: in
`crates/kernel/src/anchor_write.rs`, `validate_anchor` from 263 to 296,
`validate_anchor_row` from 254 to 287, `insert_anchor` from 375 to 405, and
`insert_anchor_inner` from 379 to 409; in `crates/kernel/src/anchor.rs`,
`AnchorCondition::decode` from 272 to 273; and the three
`kernel_anchor_write.rs` tests after the refused-shape list by 73 lines. The
catalog's `insert_anchor_inner` reference had named line 320, inside
`insert_row`; it now names 409.

## Gaps queued

| Id | Gap | Owner |
| --- | --- | --- |
| G1 | Liveness of the capture drain within `MAX_CAPTURE_ATTEMPTS = 9` | unassigned |
| G2 | Cross-pipeline text identity: redacted job text versus retained descriptor bytes (OQ2-13) | U3 ([#1135](https://github.com/ahrav/eidnara/issues/1135)) locates blocks by digest, so redacted job text fails to locate and degrades to `first` with the rule recorded |
| G3 | Fragment-id derivation versus descriptor entry id and `block_index` (OQ2-3, OQ2-12) | U3 joins the raw entry id per block index and reports hashed ids as `source_id_unjoinable` |
| G4 | Lineage succession of a re-published message revision and what a narrowed read returns | U3 reads the lineage's one live descriptor, so a re-published revision cites its newest live row; no test publishes two revisions of one message yet |
| G5 | `snapshot_checkout` HEAD semantics: detached accepted, unborn `NoHead` (OQ2-11) | U2 ([#1134](https://github.com/ahrav/eidnara/issues/1134)) reads HEAD through `git_sources::open` instead and matches the owner decision: detached anchors, unborn is `head_unavailable`; the engine's `snapshot_checkout` stays as it was |
| G6 | Memo eviction of `Pending` units | U4 ([#1136](https://github.com/ahrav/eidnara/issues/1136)) |
| G7 | `Deadline` and `InvalidInput` publish failures mapped to `kernel_write_failed` | unassigned |

## Biases for a human

- B1: five test-only S1 and S5 records weight the well-tested Kernel engine,
  while the daemon join seams (G2 to G4) have none. The two N1 records add
  two more test-only records until U3 calls the writer.
- B2: no liveness record covers existing behavior although the code bounds
  attempts and deadlines; the only liveness record is proposed (U4).
- B3: the specification carried evidence inline; this part supplies the
  per-record evidence files and the mechanical records-equal-evidence check.
  Evidence files for proposed records are shorter than METHOD's 60 to 120
  line target because their code does not exist; each states what a test
  must construct.
- B4: `high` confidence on test-only records rates the code reading, not
  production exposure.

## Verification

Mechanical checks run on the branch that adds this part:

- 28 `### ` record headings in `catalog.md`, 28 index rows, and 28 files in
  `evidence/`, with equal slug sets.
- Every `evidence/<slug>.md` link in `catalog.md` resolves.
- Every record carries the twelve METHOD fields in order.
- Every `path:line` reference in the part names an existing line.

Semantics distribution: `always` 25, `always-or-unreached` 1, `sometimes` 2,
`reachable` 0, `unreachable` 0.
