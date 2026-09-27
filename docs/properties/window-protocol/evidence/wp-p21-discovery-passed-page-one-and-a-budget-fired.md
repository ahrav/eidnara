# wp-p21-discovery-passed-page-one-and-a-budget-fired

## Discovery trigger

Record WP-P21 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface plugin. Ticket #832 pairs WP-P10 with this marker.

Exercised status: yes - "declares a match found past page one" (cursors
`[undefined, 80]`) and "declines when the time budget fires with no null
submission" (more than one cursor before the budget) construct both
situations; both ran in the #883 (head `d7712d75`) and #884 (head `f8734c12`)
`bun run check:repo` gates.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Page-two test: the first page lists `reverted-2`, `reverted-1`; the second
  lists `m-3`; asserts cursors `[undefined, 80]`, each timeout at most 1,000
  ms, `boundary` `m-3`, and `native_messages` equal `host.slice(3)`.
- Budget test: each page sleeps 300 ms and lists one absent anchor; asserts
  `cursors.length > 1` (the walk passed page one) and no transform body.
- `DISCOVERY_BUDGET_MS = TRANSFORM_SEND_TIMEOUT_MS - 4_000` at
  `rust-mode-transform.ts:796` (`f2442b2f`).
- The decline family "declines ${name} without sending null"
  (`rust-mode-window.test.ts:562`) covers the non-budget exits: repeated
  cursor, ascending page, malformed page, unsafe sequence, timeout, and a
  revision 2 daemon; each asserts a `discovery_declined` log line, no body,
  and `boundary` undefined.
- "sends null with the whole array only after an empty page" (`:486`) is the
  legal exhaustion control: cursors `[undefined, 5]` and `boundary` null.
- Review P1 and P2 on #832 changed the rediscovery path; #883 records "one
  discovery per pass" and "refund before rerun". The page-two and budget tests
  keep their names at `f2442b2f`.

Gate results of the runs cited here, as recorded in the PR descriptions:

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

Not applicable to a marker; see WP-P10.

## Timing windows and dependencies

Page round trips under the discovery budget.

## What a test must construct

Serve two pages with the match on page two; separately delay pages until the
budget fires.

## Investigation log

### Q: Is the budget run's page delay deterministic?

- Sources examined: `rust-mode-window.test.ts:589-605`.
- Findings: It uses `Bun.sleep(300)` per page against a 1 s budget.
- Missing evidence: A deterministic clock.
- Conclusion: resolved with answer: wall-clock bounded; flake risk recorded in
  `portfolio-evaluation.md`.

### Q: Does the budget test assert the walk passed page one?

- Sources examined: `rust-mode-window.test.ts:602-603`.
- Findings: Yes: the comment "Enabling state: the walk passed page one before
  the budget fired" precedes `expect(cursors.length).toBeGreaterThan(1)`.
- Missing evidence: None.
- Conclusion: resolved with answer: yes; it ran in the #883 and #884 `bun run
  check:repo` gates.
