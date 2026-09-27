# wp-p06-pruning-and-publication-share-the-meta-fence

## Discovery trigger

Record WP-P06 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface store. D12 prunes `block_identity_by_mid` to window mids inside the
meta CAS. The summarizer fence reads the same map, so the two writers race.

Exercised status: yes - #833's prune-first and publish-first tests run both
commit orders through real store transactions, with the enabling state
asserted apart from either verdict, in #833's `cargo test -p daemon` gate.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- `ModuleMeta::block_identity_by_mid` is at
  `crates/memory-store/src/lib.rs:2111` (`f2442b2f`).
- Identity maintenance: `apply_ingress_meta` at
  `crates/daemon/src/transform.rs:5279` inserts identities; #833's
  `prune_block_identities` (`:5260`) keeps the cut prefix plus the resolved
  messages, returns early for a revert before its HARD, and runs before the
  meta CAS on the additive path (`:2742`) and after the pressure refold on the
  main path (`:4981`). Pass-through paths return before either call.
- Publication fence: `publish_history_summarizer_chunk` at
  `crates/memory-store/src/lib.rs:12610` (WP-E04).
- Barrier: the transform attempt hook (`install_transform_attempt_hook`,
  `transform.rs:2184`) fires just before `commit_transform` (`:5002`); the
  tests run the competing writer inside it or after the transform commits.
- `pinned_firing` (`:19574`) commits a `Publishing` firing selecting `m5`,
  `m6` and asserts the enabling state: resolved window `[m4, m5]`, `m6`
  selected, `m6`'s identity held.
- Prune first (`:19708`): the hook asserts the row is at the firing's
  version; the transform commits and prunes to `[m4, m5]`; the publisher at
  that version gets `CasConflict`, and at the reloaded version
  `FenceRejected`; the segment count stays 2.
- Publish first (`:19741`): the hook publishes at the shared version; the
  transform reloads; response hash, segments (3), core, and identity map
  equal a serial run (publish, then transform).
- Storage of the pruned map: Since `e15a09a6` (on `main` before M1)
  `ModuleMeta::block_identity_by_mid` is `#[serde(skip)]`
  (`crates/memory-store/src/lib.rs:2110-2111`) and persisted in the
  `block_identities` table, one row per mid
  (`crates/memory-store/baseline.sql`). `load_block_identities`
  (`lib.rs:4450-4468`) reads every row of the session (`SELECT mid, identities
  FROM block_identities WHERE session_id = ?1`) on `load` (`:7470`) and on the
  transform snapshot load (`:7659`); `sync_block_identities` (`:4481`) reads
  the same rows and diff-writes them at commit (`:9877`). A prune therefore
  lands as `DELETE FROM block_identities` rows in the commit transaction
  (`lib.rs:4535`), and the publication fence reads one row per selected mid
  (`lib.rs:12703`).

Gate results of the runs cited here, as recorded in the PR descriptions:

- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `f2442b2f`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 107, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

The summarizer pins mids 10 to 20. The host's window moves past mid 12 and a
pass prunes it. The summarizer loses its CAS, reloads the row version only,
and commits with its pre-prune map, restoring mid 12's identity and publishing
a summary the fence should reject.

## Timing windows and dependencies

Both writers read the same `row_version`, then race to CAS.

## What a test must construct

Pin a firing whose selected range includes a mid outside the next window; hold
both writers at barriers after their reads; release prune first in one run and
publish first in another; assert durable state after each winner and retry.

## Investigation log

### Q: Where does the barrier sit?

- Sources examined: #833 ticket; `transform.rs:2184`, `:5002` at `f2442b2f`.
- Findings: In the existing transform attempt hook, which fires after the
  pass's reads and just before its meta CAS.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: Do pass-through paths prune?

- Sources examined: D12;
  `reconcile_recut_nothing_survives_arms_pending_raw_without_truncate`
  (`transform.rs:19785`).
- Findings: No; the arming commit leaves `block_identity_by_mid` equal to
  the pre-pass map, asserted in #833's daemon gate.
- Missing evidence: None.
- Conclusion: resolved with answer.
