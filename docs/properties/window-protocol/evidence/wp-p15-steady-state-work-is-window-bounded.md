# wp-p15-steady-state-work-is-window-bounded

## Discovery trigger

Record WP-P15 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin, daemon. The specification's acceptance criteria require N- and
H-independent steady-state cost. #833 carries the measurements.

Exercised status: yes - #833's acceptance run records items scanned, rows
visited, VM steps, bytes decoded (`3ebfc3b9`), and retained state across N
and H at W = 300. The readings await owner confirmation.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Baseline at `43e88bdc` (#873 and #874 descriptions; 30 samples, median /
  p99): daemon `total` at N = 10k, W = 300 is 128.38 / 175.06 ms; N = 100k
  2001.98 / 2206.94 ms; N = 1M INCONCLUSIVE (first steady pass 109.5 s); HARD
  H = 100 1118.46 / 1410.85 ms, H = 50,000 3994.73 / 4124.60 ms;
  `cache_state.meta` 145,751 / 146,990 / 142,699 B; idle RSS delta 236,904 /
  1,895,996 / 74,188 KiB.
- #874 head `704568ec`, 30 samples: HARD H-ratio time 1.041, rows 1.0065, hold
  1.067 (PASS); 60 samples 1.038 / 1.0065 / 1.047. #873 head `d5bb4efb` alone
  failed time (3.383) and rows (1.271).
- Artifact identity (#873, #874): cargo 1.98.1 (797e8a9bc 2026-08-05), bun
  1.3.14, release, 128 cores, 1-minute load 2.6 to 6.4.
- #883 Evidence (implementer measurement): at W = 5, N = 10k and N = 100k give
  identical counters: scanned 5, lease charge 9,876 bytes, retained 5,085
  bytes, window wire bytes 1,400.
- #879 and #880 defer full-encode latency to #833.
- #833 acceptance run, artifact identity: commit `cf89a9c2` (the later
  commits change only the plugin, the admission scans, and tests), cargo and
  rustc 1.98.1, Bun 1.3.14, release daemon test build and release
  `direct_host_fixture`, 30 samples per point after 10 warmups, W = 300 of
  5,120 bytes, shared 128-core host (1-minute load 7.9 to 35.5). Drivers and raw reports stay under the gitignored
  `docs/performance/`.
- Counters: plugin id scan 300 hops and one unpaged transform (about 3.26
  MB) at every N; D15 rows at N = 1M / H = 50k equal N = 10k / H = 100 except
  m0 (2,484 rows, cap 249 + K + L = 2,733); `cache_state.meta` 117,066 /
  117,040 / 117,064 B at N = 10k / 100k / 1M, equal entry counts, the 26 B
  spread only decimal digits; RSS per idle session, slope over k = 8
  sessions, mean of 3: 5,340 / 5,426 / 5,338 KiB (1.7%).
- Bytes decoded per D15 row, N = 10k / H = 100 against N = 1M / H = 50k
  (commit `3ebfc3b9`, cargo 1.98.1, release, load 4.37; raw
  `bytes-d15-3ebfc3b9.json` under the gitignored `docs/performance/`):
  equal in every row except m0 (14,442 / 387,504 B, the fold's 100 against
  2,484 rows), the coverage snapshot (HARD 696,516 / 847,206, steady 650,032
  / 800,718: the session row's frozen fold render), temporal marks, hints,
  appends (10,588 / 10,791) and summarizer assembly (887 / 952), the last
  two decimal digits of wider ordinals. Every row holds its WP-P07 bound;
  the table and bounds are in the WP-P07 evidence.
- Daemon `total` median / p99: 13.72 / 14.57, 14.42 / 15.55, 14.02 / 19.67
  ms; 1M/100k median 0.972 (five replicates 0.965 to 1.020).
- HARD at W = 300, N = 50,300: H = 50k against H = 2,500 (≥ K_max 2,484)
  time 1.017, rows 1.000, hold 1.031; against H = 100 (reported) 1.163,
  1.253, 1.098, below the m0 plateau by construction.
- Plugin readings, artifact identity: commit `58dbe556`, whose production
  code is identical to `f2442b2f` (it differs only in a plugin test file),
  release `direct_host_fixture` and seed binary built there, Bun 1.3.14,
  the same driver method, 10 warmups and 30 samples per point, three runs,
  each started at 1-minute load below 8 (5.80, 5.90, 6.56 for steady; 7.37,
  6.41, 5.90 for events). The earlier plugin run at `cf89a9c2` (load 16 to
  22) failed the 100 ms limit at 100k steady and cold start; `c80d48f58`,
  `6e88eacf3`, and `dd6d38993` cut about 17 ms from the steady median.
- Plugin steady whole-hook median / p99 ms at N = 10k, 100k, 1M: r1 58.5 /
  70.7, 58.7 / 67.8, 63.5 / 76.2; r2 55.5 / 71.9, 59.1 / 67.2, 60.5 / 71.0;
  r3 58.1 / 73.3, 59.6 / 80.9, 57.7 / 66.6. 1M/100k median 1.082, 1.024,
  0.968. Every sample `SOFT+` applied, one unpaged `transform` call, scanned
  300, no error.
- Plugin events at N = 1M: cold start median / p99 80.2 / 90.2, 81.9 /
  90.2, 77.7 / 84.7 ms, 30/30 `ok/SOFT+` with one `transform.boundary` page
  and one `transform` call; removal inside covered history p99 76.5, 79.3,
  89.0 ms, 30/30 scanned 300, one `transform` call, no `transform.boundary`;
  the pass after a fold 15/15 `SOFT+`, no retry, no rediscovery, no error;
  revert past the boundary pass 1 p99 739.6, 738.8, 752.3 ms, pass 2 p99
  1,067.6, 1,037.3, 1,122.6 ms, two-pass p99 1,796.8, 1,739.1, 1,848.2 ms.
- Daemon regression check at `58dbe556` (load 5.48): `total` p99 14.59 /
  14.59 / 15.52 ms at N = 10k, 100k, 1M; meta 117,066 / 117,040 / 117,064
  B, unchanged. The in-process driver does not run the admission scans.

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
- #883 (#832 PR one; head `d7712d75`, base
  `window-protocol/m1-daemon-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `3ebfc3b9`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 108, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

A residual O(N) step, such as an all-slot preflight or a full id scan per
pass, makes a 1M-message session miss the 100 ms plugin budget while W is
fixed.

## Timing windows and dependencies

Steady state only; failure and recovery events are not pooled.

## What a test must construct

Drive the whole transform hook on a fresh N-slot array per sample, rebuilt
outside the timer, against the release daemon; record the named counters and
p99 and median per (N, H) point; compare counters for equality and medians
within 10%.

## Investigation log

### Q: Is any N-sweep recorded under revision 3?

- Sources examined: M1 PR descriptions; #833 acceptance run.
- Findings: #833's run covers N = 10k, 100k, 1M at W = 300; every counter it
  records is equal across N up to decimal-digit width. Bytes decoded
  (`3ebfc3b9`) are equal per D15 row up to decimal digits, except m0 and the
  coverage snapshot's frozen fold render, which follow the fold's rows under
  the D14 cap.
- Missing evidence: None.
- Conclusion: resolved with answer; the bytes-decoded reading (equal up to
  the fold render and decimal digits) awaits owner confirmation.

### Q: Does connection hold time stay H-independent?

- Sources examined: #874 description; #833 acceptance run.
- Findings: Revision 2 H-ratio 1.067 and 1.047. Revision 3: 23.00 ms at H =
  50k against 22.30 at H = 2,500 (1.031) and 20.96 at H = 100 (1.098).
- Missing evidence: The owner's choice of comparison point.
- Conclusion: needs human input: confirm H = 2,500 (≥ K) as the comparison
  point, with H = 100 reported.
