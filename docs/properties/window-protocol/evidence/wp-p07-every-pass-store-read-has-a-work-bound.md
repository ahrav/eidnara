# wp-p07-every-pass-store-read-has-a-work-bound

## Discovery trigger

Record WP-P07 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon, store. Revision 3 bounds ingress; this record bounds every
store read behind it. #826 builds the ledger and the inventory.

Exercised status: yes - #874 inventory test covers H at fixed W (rerun in the
#881 and #833 gates, revision 3); #833's D15 run covers N = 1M / H = 50k
(rows, VM steps, bytes decoded). Readings await owner confirmation.

## Evidence trail

Code references verified at `f2442b2f` (#833 code before `f0501b3d`, base `main`
`d68aedf3` with #881, #883, #884) unless noted; first read at `f8734c12`,
moved by line diff; each cited test name found there.

- Ledger (#873): per statement on the guarded connection, trimmed SQL, rows
  materialized, `VM_STEP` via `sqlite3_trace_v2`; a `LIMIT 1` full scan
  reports ~10k+ steps returning one row. Bytes decoded (`3ebfc3b9`): the
  `TRACE_ROW` hook sums each produced value, 8 B per INTEGER or FLOAT, 0 per
  NULL, `sqlite3_column_bytes` for TEXT and BLOB only (numeric columns never
  converted); `statement_work_counts_the_bytes_of_every_returned_value`
  (`crates/storage/src/lib.rs:6401`) pins two six-column rows at 59 B.
- #874 inventory (together axis), H = 100 versus 50,000: every row equal
  except m0 segments (100/2449 versus 2489/59848, bounded by 249 + K + L) and
  scan ledger (random scan ids, within 16 + 1%); e.g. coverage snapshot 27/333,
  tags 39/772, publication set fence 3/51, summarizer assembly 7/158. Two
  temporary mutations failed the test: loading every assembly row
  (`(101, 2229)` versus `(50001, 1100029)`) and dropping the tag range's upper
  bound.
  #875: intersection (1, 676) at H = 4,500 and 9,000; page (4096, 28685) at
  both.
- #881 (head `1c66f16c`): the inventory drives revision 3 requests and
  classifies every boundary read, and the null-boundary window-end match, into
  the coverage-snapshot row; it ran in the #881 daemon gate. H varies at
  fixed W; N does not. Commit `cd596c15`: the inventory failed with an
  unclassified 50,000-row statement at H = 50,000 until it reopened the store
  before measuring (cold range-order check); the #874 table predates it; the
  #881 and #833 daemon gates ran with it.
- Identity load: `load_block_identities`, `sync_block_identities`
  (`crates/memory-store/src/lib.rs:4450`, `:4481`) read every session
  `block_identities` row, in the coverage-snapshot row
  (`crates/daemon/src/transform_read_bound.rs:103`, since #874); #833 prunes to
  window mids in the meta CAS (`prune_block_identities`,
  `crates/daemon/src/transform.rs:5260`): O(W).
- #833 D15 run (`cf89a9c2`, Rust 1.98.1, release test build, W = 300, 30
  samples per point, active summarizer, overlays over the whole session), N =
  1M / H = 50k rows / VM steps: HARD coverage snapshot 1,222 / 6,530, tags 320
  / 19,570, scan ledger 817 / 182,877, publication set fence 3 / 51. Every row
  equals N = 10k rows exactly (VM steps within 8) except m0 (100 at 10k, 2,484
  at 1M; D14 bound 249 + K + L = 2,733). No whole-table statement ran.
- Declared bound per row (`assert_bytes_bound`,
  `crates/daemon/src/transform_read_bound.rs:597`): m0 at (249 + K + L) rows,
  m1 at 260 rows, times `SEGMENT_ROW_BYTES` (`:41`; eleven text columns each
  held to `MAX_DURABLE_TEXT_BYTES` = 512 KiB by `prepare_history_segment`,
  `crates/memory-store/src/lib.rs:429`, `:4649`, plus six integers: 5,767,216
  B); other rows at small-point bytes times the ordinal-width ratio (a
  digits-only value grows at most by it; a read growing with H scales by H);
  coverage snapshot adds the largest m0 read (its `cache_state` row carries
  the frozen fold render).
- In-crate test at `3ebfc3b9` (debug, together, H = 100 against 50,000, width
  ratio 6/3), bytes (bound): HARD coverage snapshot 29,079 to 118,257
  (1,678,397; `frozen_units` 20.5 / 107.7 / 109.5 KB at H = 100 / 5,000 /
  50,000), tags 1,405 to 1,499 (2,810), temporal marks 217 to 247 (434), scan
  ledger 992 to 992, m0 1,620,239 at H = 50,000 (15,790,637,408; 1,615,261 at
  5,000).
- #833 D15 bytes run: `3ebfc3b9`, cargo 1.98.1, release test build, 128 cores,
  1-minute load 4.37 at start, `driver833bytes.rs` (`driver833final.rs` plus
  bytes), raw `bytes-d15-3ebfc3b9.json`, `.log`, `-verdict.txt` in gitignored
  `docs/performance/window-m1/` (measurement worktree). Rows, VM steps equal
  `cf89a9c2`. Bytes N = 10k / H = 100 vs N = 1M / H = 50k (bound, width ratio
  7/5): HARD coverage snapshot 696,516 / 847,206 (1,362,626); m0 14,442 /
  387,504 (15,761,801,328); m1 after publish 111 / 111 (1,499,476,160); tags
  13,440 / 13,440 (18,816); temporal marks, hints, appends 10,588 / 10,791
  (14,823); scan ledger 52,064 / 52,064 (72,889); pass trace and ledgers 912 /
  912 (1,276); publication set fence 24 / 24 (33); notes status version 16 / 16
  (22); summarizer assembly 887 / 952 (1,241); append range validation 8 / 8
  (11); active user memories 0 / 0. All 58 (row, phase) cells hold their bound;
  largest coverage-snapshot ratio 1.232 (steady).

Gates per PR description, all ok (fc = `test:fixture-contract`):

| PR | Heads | Gates |
| --- | --- | --- |
| #873 (#826 PR 1) | gate `d5bb4efb`, read `efb3fe9c` | fmt, clippy, `test -p storage` (107), `-p memory-store`, `-p daemon`, doctests, `check -p storage --no-default-features`, markers, fixture build; fc 6 pass; `validate-mode-manifest`; `test:rust` 23 pass, 17 skip, 0 fail |
| #874 (#826 PR 2) | description `704568ec`, read `c239461c` | fmt, clippy, `-p memory-store`, `-p daemon`, doctests, `check -p storage --no-default-features`, markers, fixture build; fc 6 pass; `validate-mode-manifest`; `test:rust` 0 fail, 17 skip |
| #875 (#827) | local `328ab11c`, read `2d58cc20` | fmt, clippy, `-p daemon` (2,766), `-p memory-store`, doctests, markers, `bun run check:repo`, fixture build; fc 6 pass; `validate-mode-manifest`; `test:rust` 40 pass, 0 fail |
| #881 (#831) | `1c66f16c`, base `main` `be542f0c` | fmt, clippy, `test -p daemon` (one run hit a known timing flake in a 33-test binary; rerun green), `-p memory-store`, `check -p storage --no-default-features`, `-p host-runtime --test protocol_vectors`, doctests, markers, `bun run check:repo`, fixture build; fc 6 pass; `validate-mode-manifest`; `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin is refused), 23 pass, 17 skip, 0 fail with #883 |
| #833 (`window-protocol/m1-exit`) | gates at final head, code `3ebfc3b9` | fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339, `-p storage` 108, doctests 19, `check -p storage --no-default-features`, markers, `check:repo`, fixture build; fc 6 pass; `validate-mode-manifest`; `test:rust` 23 pass, 17 skip, 0 fail. The #833 PR description records the gates and the acceptance run |

## Failure scenario

A pass reads all H segment rows to return the newest six. At H = 50k the HARD
pass holds its connection past the deadline.

## Timing windows and dependencies

Hidden full scans appear on cold paths: first pass after restart, retry, reset.

## What a test must construct

Arm the ledger; drive real SOFT, HARD, CAS-retry, summarizer, absent-boundary
passes at two H and two N values at fixed W; classify every statement by exact
text; fail on unclassified or whole-table shapes; compare rows and VM_STEP per
row; hold bytes decoded to the row's bound.

## Investigation log

### Q: Is N-independence shown?

- Sources examined: #873 and #874 measurement sections; #833 acceptance run.
- Findings: Revision 2 N = 1M was INCONCLUSIVE (first steady pass 109.5 s at
  base). Revision 3: every D15 row at N = 1M / H = 50k equals N = 10k / H =
  100 except m0, bounded by D14; every row holds its bytes bound (`3ebfc3b9`).
- Missing evidence: None.
- Conclusion: resolved with answer for rows, VM steps, bytes decoded; the
  reading awaits owner confirmation.
