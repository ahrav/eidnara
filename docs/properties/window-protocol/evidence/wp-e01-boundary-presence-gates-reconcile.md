# wp-e01-boundary-presence-gates-reconcile

## Discovery trigger

Record WP-E01 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. Catalog revision 2 (#824 comment 3) records the reconcile gate
as existing behavior that revision 3 must preserve. Ticket #831 names WP-E01
as preserved while the reconcile path receives the new resolution result.

Exercised status: yes -
`empty_store_bootstrap_then_defers_stably_without_hard_oscillation` and
`reconcile_rematerialize_after_revert_is_not_blocked_by_the_mint_guard`
construct an empty bootstrap and a removed rendered anchor and assert
`reconcile_pending` set on the missing anchor and cleared after the refold;
both ran in the `cargo test -p daemon` gate recorded in #880 (head `748c0c4b`)
and again in the #881 `cargo test -p daemon` gate (head `1c66f16c`), whose
Evidence table lists WP-E01 under "existing checks green".

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #880 (`window-protocol/m1-retire-native-chunks`, head
  `748c0c4b`) records `cargo +1.98 test -p daemon --all-features --locked ok`.
  Both witnesses exist at `748c0c4b` (`transform.rs:20014`, `:20334`) and at
  `f2442b2f` (`:19100`, `:19420`).
- `reconcile_rematerialize_after_revert_is_not_blocked_by_the_mint_guard`
  bootstraps a HARD with `boundary_id` `t2#0`, removes the anchor, and asserts
  the revert pass answers `SOFT+` with `reconcile_pending` true; the re-cut
  pass re-mints `a#0` and clears the flag.
- `empty_store_bootstrap_then_defers_stably_without_hard_oscillation` covers
  the never-minted case: the first pass is the bootstrap HARD and later passes
  defer without a second HARD.
- Revision 3 run: #881 (`window-protocol/m1-daemon-revision-3`, head
  `1c66f16c`) records `cargo +1.98 test -p daemon --all-features --locked ok`
  (one run hit a known timing flake in a 33-test binary; the rerun was green)
  and lists WP-E01 under "existing checks green". `Revert { Some }` enters the
  reconcile path; the witnesses sit at the same lines at `1c66f16c` and
  `f8734c12` and were re-located at `f2442b2f`.
- Earlier M1 PRs (#873 to #879) each record a green daemon suite with these
  tests present; #880 is cited because it is the newest recorded run before
  revision 3.
- Reachability: the gate runs on every pass that finds a stored boundary; no
  configuration enables it.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #873 (#826 PR 1; description gate head `d5bb4efb`, head at read `efb3fe9c`):
  fmt, clippy, `test -p storage` (107), `-p memory-store`, `-p daemon`,
  doctests, `check -p storage --no-default-features`, comment markers, fixture
  build all ok; `test:fixture-contract` 6 pass; `validate-mode-manifest` ok;
  `test:rust` 23 pass, 17 skip, 0 fail.
- #879 (#830 PR 1; head `7a3db43b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail (steady-state byte identity included).
- #880 (#830 PR 2; head `748c0c4b`): fmt, clippy, `-p daemon`, doctests,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  23 pass, 17 skip, 0 fail.
- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.

## Failure scenario

A host revert removes the message that holds the rendered anchor. If the gate
reads an absent real anchor as the never-minted sentinel, the pass folds over
stale coverage and the model loses the reverted turn's replacement; if it
reads a fresh session as a revert, every pass refolds.

## Timing windows and dependencies

Between two passes. No in-pass interleaving is required; the enabling state is
the stored boundary plus a window that lacks it.

## What a test must construct

Commit a HARD that mints an anchor, send a window without it, and assert
`reconcile_pending` and the `SOFT+` response before the next HARD. Separately
start from an empty store and assert no reconcile flag. Assert the frozen
bytes of the defer pass are byte-equal to the prior response.

## Investigation log

### Q: Does the revision 3 resolver bypass the presence gate?

- Sources examined: #881 description (Pass wiring; Evidence row "WP-E01,
  WP-E02, WP-E03, WP-E12"), `crates/daemon/src/transform.rs` at `1c66f16c`.
- Findings: #881 routes `Revert { Some }` into the reconcile path and lists
  WP-E01 as "existing checks green"; the gate stays "the live window contains
  `core.boundary_id`". The witnesses above exist at `1c66f16c` and `f2442b2f`.
- Missing evidence: None.
- Conclusion: resolved with answer: the gate is preserved; #881's description
  and daemon gate confirm it.

### Q: Is the initial reconcile response `SOFT+`?

- Sources examined:
  `reconcile_rematerialize_after_revert_is_not_blocked_by_the_mint_guard` at
  `transform.rs:19420` (`f2442b2f`).
- Findings: The test asserts `assert_eq!(revert.action, "SOFT+", "revert never
  busts on sight")`.
- Missing evidence: None.
- Conclusion: resolved with answer: `SOFT+`, as catalog revision 2 records.
