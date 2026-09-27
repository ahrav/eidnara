# wp-p07-every-pass-store-read-has-a-work-bound

## Discovery trigger

Record WP-P07 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon, store. Revision 3 bounds ingress; this record bounds every
store read behind it. #826 builds the ledger and the inventory.

Exercised status: yes - the #874 inventory test covers H at fixed W (and
ran again in the #881 and #833 gates under revision 3 requests); #833's
acceptance D15 run covers N = 1M at H = 50k: every row equal to the N = 10k /
H = 100 point except m0, which stays under its D14 cap. Readings await owner
confirmation.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Ledger: per statement run on the guarded connection, trimmed SQL, rows
  materialized, and `VM_STEP` via `sqlite3_trace_v2`; tests show a `LIMIT 1`
  full scan reports ~10k+ steps while returning one row (#873).
- #874 inventory table (together axis), H = 100 versus H = 50,000: every row
  equal except m0 segments (100/2449 versus 2489/59848, bounded by 249 + K +
  L) and the scan ledger (random scan ids, compared within 16 + 1%). Examples:
  coverage snapshot 27/333, tags 39/772, publication set fence 3/51,
  summarizer assembly 7/158.
- #874 records two temporary mutations that failed the test: loading every
  assembly row (`(101, 2229)` versus `(50001, 1100029)`) and dropping the tag
  range's upper bound.
- #875: intersection (1, 676) at H = 4,500 and 9,000; page (4096, 28685) at
  both.
- #881 (head `1c66f16c`): the inventory drives revision 3 requests and
  classifies every boundary read, and the null-boundary window-end match,
  into the coverage-snapshot row; it ran in the #881 daemon gate. H still
  varies at fixed W; N does not.
- Commit `cd596c15` states the inventory failed with an unclassified
  50,000-row statement at H = 50,000 until it reopened the store before
  measuring (cold range-order check); the #874 table predates it, and the
  #881 and #833 daemon gates ran the test with it.
- Identity load: `load_block_identities` and `sync_block_identities`
  (`crates/memory-store/src/lib.rs:4450`, `:4481`) read every
  `block_identities` row of the session; the inventory classifies the
  statement into the coverage-snapshot row
  (`crates/daemon/src/transform_read_bound.rs:103`, since #874). #833 prunes
  the table to window mids inside the meta CAS (`prune_block_identities`,
  `crates/daemon/src/transform.rs:5260`), so the read is O(W).
- #833 acceptance D15 run (commit `cf89a9c2`, Rust 1.98.1, release test
  build, W = 300, 30 samples per point, active summarizer, overlays over the
  whole session): at N = 1M / H = 50k, HARD coverage snapshot 1,222 rows /
  6,530 VM steps, tags 320 / 19,570, scan ledger 817 / 182,877, publication
  set fence 3 / 51; every row equals N = 10k rows exactly (VM steps within 8)
  except m0 (100 rows at 10k, 2,484 at 1M; D14 bound 249 + K + L = 2,733).
  No whole-table statement ran. Bytes decoded: not instrumented, INCONCLUSIVE.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #873 (#826 PR 1; description gate head `d5bb4efb`, head at read `efb3fe9c`):
  fmt, clippy, `test -p storage` (107), `-p memory-store`, `-p daemon`,
  doctests, `check -p storage --no-default-features`, comment markers, fixture
  build all ok; `test:fixture-contract` 6 pass; `validate-mode-manifest` ok;
  `test:rust` 23 pass, 17 skip, 0 fail.
- #874 (#826 PR 2; description head `704568ec`, head at read `c239461c`): fmt,
  clippy, `-p memory-store`, `-p daemon`, doctests, storage no-default check,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  0 fail, 17 skip.
- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `f2442b2f`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 107, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.
  The #833 PR description records the gates and the acceptance run.

## Failure scenario

A pass reads all H segment rows to return the newest six. At H = 50k the HARD
pass holds its connection long enough to exceed the deadline.

## Timing windows and dependencies

Hidden full scans appear on cold paths: first pass after restart, retry, or
reset.

## What a test must construct

Arm the ledger; drive real SOFT, HARD, CAS-retry, summarizer, and
absent-boundary passes at two H values and two N values at fixed W; classify
every statement by exact text; fail on any unclassified or whole-table shape;
compare rows and VM_STEP per row.

## Investigation log

### Q: Is N-independence shown?

- Sources examined: #873 and #874 measurement sections; #833 acceptance run.
- Findings: Under revision 2 N = 1M was INCONCLUSIVE (first steady pass
  109.5 s at base). Under revision 3 every D15 row at N = 1M / H = 50k equals
  the N = 10k / H = 100 point except m0, which is bounded by D14.
- Missing evidence: Bytes decoded (not instrumented).
- Conclusion: resolved with answer for rows and VM steps; the reading
  awaits owner confirmation.
