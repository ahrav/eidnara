# wp-e08-ready-snapshots-reject-stale-generations

## Discovery trigger

Record WP-E08 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Catalog revision 2 records the generation fence as existing
behavior. #828 changes the snapshot budget and #829 strips native payload from
the snapshot.

Exercised status: yes -
`transform_snapshot_cache_is_generation_safe_and_lru_bounded`,
`snapshot_lease_budget_survives_cache_churn_and_releases_exact_charge`, and
#878's
`ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` ran
in the #878 daemon gate (head `f6b3d7bd`).

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #878 (head `f6b3d7bd`) `cargo test -p daemon ok` and
  `memory-store ok`.
- #878: the ready snapshot keeps the CK input and scalar fields with
  `native_messages: None` and `serve_native: false`, under the unchanged 64
  MiB ready budget, 64 MiB lease budget, eight leases, and generation fence;
  its charge is a bounded field sum (1,952 bytes on the test fixture).
- Budget history, read from source: `TRANSFORM_SNAPSHOT_BUDGET_BYTES` is 256
  MiB at #876's head `f8940a26` (`lib.rs:782`) and 64 MiB at #878's head
  `f6b3d7bd` (`lib.rs:780`) and at `f2442b2f` (`lib.rs:782`);
  `ACTIVE_SNAPSHOT_LEASE_BUDGET_BYTES` equals it (`:784` at `f2442b2f`).
  #876's rise served tail deltas from the ready snapshot; #878 removed the
  delta channel and the native payload, so the 64 MiB budget returns.
- #875 records WP-E08 preserved with existing tests green.
- `ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation`
  asserts budgets, lease count, charge, generation pinned across replacement,
  and lease budget released.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #876 (#828; head `f8940a26`): fmt, clippy, `-p daemon`, doctests, markers,
  fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust` 23 pass,
  19 skip, 0 fail.
- #878 (#829 PR B; head `f6b3d7bd`): fmt, clippy, `-p daemon`, `-p
  memory-store`, doctests, markers, `bun run check:repo`, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

Pass A begins, pass B begins and finishes, then A finishes. If A's
`finish_ready` replaced B's snapshot, wrapup would read A's older input.

## Timing windows and dependencies

Between a newer begin and an older finish; while a wrapup lease is held during
eviction.

## What a test must construct

Begin two generations for one session, finish the older last, and assert the
newer stays ready; hold a lease, churn the cache past its budget, and assert
the leased contents and charge survive until drop.

## Investigation log

### Q: What is the ready-snapshot budget at `f2442b2f`?

- Sources examined: #876 and #878 descriptions;
  `crates/daemon/src/lib.rs:782-784` at `f8940a26` and `f2442b2f`, `:780-782`
  at `f6b3d7bd`.
- Findings: #876 raised the budget to 256 MiB; #878 returned it to 64 MiB,
  which its description calls unchanged relative to the pre-M1 value.
  `f2442b2f` has 64 MiB.
- Missing evidence: None.
- Conclusion: resolved with answer: 64 MiB at `f2442b2f`; the fence semantics
  do not depend on the value.

### Q: Does per-session serialization replace the fence?

- Sources examined: #875 description.
- Findings: No. The lane orders execution; the fence still guards completion
  order for detached work.
- Missing evidence: None.
- Conclusion: resolved with answer: both remain.
