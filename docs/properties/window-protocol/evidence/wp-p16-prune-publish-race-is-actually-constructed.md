# wp-p16-prune-publish-race-is-actually-constructed

## Discovery trigger

Record WP-P16 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface store. WP-P06's safety check can pass without the race ever occurring.
This marker proves the race was constructed.

Exercised status: yes - #833's two WP-P06 tests construct both orders and
assert the marker before each verdict, in #833's `cargo test -p daemon` gate.

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- #833 ticket obligations: prune wins yields a CAS conflict then fence
  rejection with no segment rows written; publish wins yields a re-resolution
  equal to the serial result with the publication kept; both writers observed
  reading the same starting version with a selected mid absent from the
  resolved window.
- Seams at `f2442b2f`: `install_transform_attempt_hook`
  (`crates/daemon/src/transform.rs:2184`), `run_transform_attempt_hook`
  (`:2196`), and the store's `set_coverage_snapshot_hook` used by #875's
  barrier tests.
- Falsifier named by #833: publish loses CAS, reloads only the row version,
  commits with its pre-prune identity map. The prune-first test runs exactly
  that publisher and asserts `FenceRejected`.
- Marker, shared by both runs: `pinned_firing`
  (`crates/daemon/src/transform.rs:19574`) asserts the resolved window's mids
  are `[m4, m5]`, the firing's selected identities include `m6`, and the
  store holds `m6`'s identity. The attempt hook asserts the row is at the
  firing's version when the transform reaches its commit, so both writers
  start from one version; it then sets a flag the test asserts.
- Order: `a_prune_that_commits_first_fences_the_publication_out` (`:19708`) lets
  the transform commit, then runs the publisher;
  `a_publication_that_commits_first_makes_the_transform_reload_and_match_the_serial_run`
  (`:19741`) publishes inside the hook, before the transform's CAS.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #875 (#827; local head `328ab11c`, head at read `2d58cc20`): fmt, clippy,
  `-p daemon` ok (2,766), `-p memory-store`, doctests, markers, `bun run
  check:repo` ok; fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 40 pass, 0 fail.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `f2442b2f`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 107, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

Not applicable to a marker; see WP-P06.

## Timing windows and dependencies

Both writers hold at barriers after their reads.

## What a test must construct

Record the starting `row_version` seen by each writer, the selected mid, the
resolved window's mids, and which writer committed first; assert the
preconditions per run and both orders across runs.

## Investigation log

### Q: Is one barrier enough for both orders?

- Sources examined: #833 ticket ("one deterministic test-only barrier around
  the meta CAS"); the two tests at `f2442b2f`.
- Findings: Yes. One hook placement serves both: the publisher runs inside
  it for publish first and after the transform's commit for prune first.
- Missing evidence: None.
- Conclusion: resolved with answer.
