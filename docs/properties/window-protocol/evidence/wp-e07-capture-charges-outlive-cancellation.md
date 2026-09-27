# wp-e07-capture-charges-outlive-cancellation

## Discovery trigger

Record WP-E07 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Catalog revision 2 records capture accounting as existing
behavior. #829 collapses retained output into `RetainedOutputs` and must not
touch lease accounting.

Exercised status: yes - the model-based lease sequence test, the per-session
lease test, the aggregate byte test, and the retained-output
eviction-versus-live-lease family ran in the #877 `bun run check:repo` gate
(head `3844a179`).

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #877 (head `3844a179`) `bun run check:repo ok` (one
  unrelated flaky timer test passed 3 of 3 on rerun).
- #877 description: "Eviction never touches capture leases (tested with a live
  lease through count and byte evictions)".
- #875 records WP-E07 preserved with existing tests green; #875's plugin
  change adds `session_busy` and unrecognized-status declines only.
- `RetainedOutputs` is at `rust-mode-transform.ts:206` (`f2442b2f`), 64
  sessions and 64 MiB.
- #883 and #884: the membership filter charges `4·N` bytes to the lease
  ("holds every present id, refuses an unaffordable filter, and retains no id
  string", `rust-mode-window.test.ts:166`; #883's Evidence: "4·N bytes
  charged; no id string retained"); rediscovery refunds the first attempt's
  charge (`CaptureLease.refund()`, "refunds the first attempt so a
  rediscovered window near the byte limit still publishes", `:802`). Both ran
  in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run
  check:repo` gates.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #877 (#829 PR A; head `3844a179`): `bun run check:repo` ok (one unrelated
  flaky timer test passed 3 of 3 on rerun), fmt, clippy, `-p daemon`, fixture
  build ok; fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17
  skip, 0 fail.
- #883 (#832 PR one; head `d7712d75`, base
  `window-protocol/m1-daemon-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).
- #884 (#832 PR two; head `f8734c12`, base
  `window-protocol/m1-plugin-revision-3`): `bun install --frozen-lockfile`,
  `bun run check:repo`, fmt, clippy, markers, fixture build ok;
  fixture-contract 6 pass; manifest ok; `test:rust` 23 pass, 17 skip, 0 fail
  (no addon_unavailable skips).

## Failure scenario

A superseded pass is cancelled while its blocking work still holds a capture.
Releasing the charge on cancel lets a new pass exceed the budget while both
captures are live.

## Timing windows and dependencies

Between cancellation and owner settlement.

## What a test must construct

Admit a session, suspend its owner, cancel it, and assert the charge and slot
remain until settlement; generate random lease sequences against a reference
ledger; evict retained output while a lease is live and assert `activePasses`
and the charge are unchanged.

## Investigation log

### Q: Does retained-output eviction release lease charges?

- Sources examined: #877 description; `rust-mode-transform.test.ts:1319`.
- Findings: The family evicts by count and by bytes while a lease is live and
  asserts the lease is untouched.
- Missing evidence: None.
- Conclusion: resolved with answer: no.

### Q: Is the new discovery filter charged?

- Sources examined: #883 description; `messageIdFilter` at
  `transform-capture.ts:121`.
- Findings: The filter reserves `4·N` bytes through the lease before it
  allocates.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; the test ran in the #883 and #884
  `bun run check:repo` gates.
