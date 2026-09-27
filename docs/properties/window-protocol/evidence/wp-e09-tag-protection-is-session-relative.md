# wp-e09-tag-protection-is-session-relative

## Discovery trigger

Record WP-E09 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface daemon. #826 replaces the session-wide tag baseline cache with three
bounded queries; catalog revision 2 requires decision-level parity for all
three.

Exercised status: yes -
`window_tag_read_keeps_every_session_relative_tag_decision` compares every tag
decision of the bounded read against a full read for K in {0, 1, 2, 3, 5, 8,
40, 100}, and `shared_row_iterator_matches_slice_for_protected_legacy_orphan`
covers the legacy orphan; both ran in the #874 daemon gate (description head
`704568ec`).

## Evidence trail

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- Revision-bound run: #874 `cargo test -p daemon ok`, `memory-store ok`.
- `MemoryStore::load_tags_for_window` (`crates/memory-store/src/lib.rs:8674`,
  `f2442b2f`): (a) rows for window mids as exact id or `<mid>#` prefix on
  `UNIQUE(session_id, block_id)`, EXPLAIN-pinned; (b) legacy rows by window
  tool call ids; (c) the newest `max(K, 1) + window_rows` rows on the primary
  key.
- The tag baseline cache, its LRU, budget constant, and `TagCacheSummary` are
  deleted; latency B3 is marked invalidated (#874 description).
- #874 inventory: tags 39 rows and 772 VM steps at both H = 100 and H =
  50,000. Commit `08afec9d` reports 30 rows after its rewrite; no description
  records that run. At `cf89a9c2` #833's acceptance D15 run reads the tags
  row at 320 rows / 19,570 VM steps on HARD at both N = 10k / H = 100 and N =
  1M / H = 50k (W = 300).
- #874 records a temporary mutation that dropped the tag range's upper bound:
  the inventory failed with `tags: (432, 6605)` against `(175058, 2551121)`.

Gate results of the runs cited here, as recorded in the PR descriptions:

- #874 (#826 PR 2; description head `704568ec`, head at read `c239461c`): fmt,
  clippy, `-p memory-store`, `-p daemon`, doctests, storage no-default check,
  markers, fixture build ok; fixture-contract 6 pass; manifest ok; `test:rust`
  0 fail, 17 skip.
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

## Failure scenario

Under revision 3 the window holds 300 messages; the three newest tag numbers
belong to covered messages. A window-only read would protect the wrong tags
and let hygiene reduce protected content.

## Timing windows and dependencies

None within a pass.

## What a test must construct

Seed tags inside and outside the window, legacy orphan rows keyed by tool call
id, ambiguous mappings, and a protected arc; compare every protection decision
of the bounded read with the full read for several K.

## Investigation log

### Q: Does the bounded read keep every decision?

- Sources examined: #874 description; the named differential.
- Findings: Decisions compared for eight K values against a full read.
- Missing evidence: None. The differential ran after `08afec9d` in the
  #881 and #833 `cargo test -p daemon` gates.
- Conclusion: resolved with answer.

### Q: Is the unread `tag_cache_generations` table a risk?

- Sources examined: #874 description (C12, no schema change).
- Findings: The table and triggers stay and are unread.
- Missing evidence: None.
- Conclusion: resolved with answer: no correctness effect; a later migration
  can drop them.
