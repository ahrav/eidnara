# wk-ledger-pool-disjoint-from-own-task-writes

## Discovery trigger
Parent specification #1119 C11 and C12, user story 3; Part B record
`wk-ledger-pool-disjoint-from-own-task-writes`; implemented by #1122.

## Evidence trail
- `crates/eval-core/src/access.rs:76` `AccessRecord {writes, reads,
  read_counts}`; `MemoryWrite` and `MemoryRead` carry the fields C11 names.
- `crates/eval-core/src/access.rs:158` validates the ledger first; the write
  pass refuses blank ids, repeats, unknown writers, keys that are not the
  writer's, and runs outside the current run and the writer's lineage; the
  read pass refuses unknown readers and writes, cross-pool reads, and own-task
  reads (`:227`); the attestation pass refuses unknown attested samples and
  count mismatches.
- `crates/eval-core/tests/access.rs` exercises every `AccessError` variant at
  least once.

## Failure scenario
A runner whose store dropped own-task writes for the aged arm but not for the
fresh arm would leak task t1's fresh writes into t1's aged pool; the record
shows the read and the check names it.

## Timing windows and dependencies
Order-free: sequences are carried for audit and never compared, so a read
before its write in one pool is refused the same way.

## What a test must construct
Sample `s1` (task t1, Fresh) reads `w7` written under `(t1, Aged)` in the same
pool; the same write made under a run in `s1.lineage`; a write under a run in
another sample's lineage but not its writer's; a dangling reader, writer,
write, and run id; a read count off by one.

Legal witness: reads(s1) = {w3 keyed (t2, Aged)} with writes {w3, w7 keyed
(t1, Aged)} passes. Smallest falsifier: reads(s1) = {w7} and the check passes.

## Investigation log
### Q: Why refuse a key that is not the writer's?
- Sources examined: the attribution rule in `check_disjoint`
  (`crates/eval-core/src/access.rs:229`) and the key refusal (`:188`)
- Findings: attribution reads `write.key`; the write pass refuses a key that
  is not its writer's (`KeyDisagreesWithWriter`, `:188`) before any read is
  checked, so a t1 writer's write stored under a t2 key is refused; absent
  that refusal it would pass a t1 reader's check
- Missing evidence: none.
- Conclusion: resolved with answer - `KeyDisagreesWithWriter` keeps key-based
  attribution sound, added in #1122 beyond the spec's list
### Q: Catalog open question: Recording completeness is the runner's attestation in `read_counts`
- Sources examined: `crates/eval-core/src/access.rs:153` (`check_disjoint`),
  `:227` (own-run attribution)
- Findings: Recording completeness is the runner's attestation in
  `read_counts`; the evaluator cannot observe a read the runner did not record
- Missing evidence: an owner decision.
- Conclusion: needs human input
