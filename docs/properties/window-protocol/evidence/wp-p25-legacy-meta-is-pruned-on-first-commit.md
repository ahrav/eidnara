# wp-p25-legacy-meta-is-pruned-on-first-commit

## Discovery trigger

Record WP-P25 of catalog revision 2
([#824](https://github.com/ahrav/eidnara/issues/824) comments 3 and 4),
surface store. D12 bounds new sessions; this record covers sessions whose meta
was written before revision 3.

Exercised status: yes - #833's legacy fixture is read after a restart,
pruned on its first commit, and converges under a CAS conflict in both
orders; the three tests ran in #833's `cargo test -p daemon` gate.

## Evidence trail

Code references are verified at `f2442b2f`, #833's code before `f0501b3d`
(`window-protocol/m1-exit`, base `main` `d68aedf3`, which merged #881, #883,
and #884), unless another tree is named. They were first read at `f8734c12`
and moved by a line diff to `f2442b2f`; each cited test name was found there.

- A persisted row is at most 512 KiB by construction
  (`ensure_durable_text_bound`, `crates/memory-store/src/lib.rs:4306`;
  `MAX_DURABLE_TEXT_BYTES`, `:429`), so the one-time legacy read is bounded
  (WP-E10).
- #833 obligations: fixture row near the 512 KiB limit with an active
  summarizer state; restart before the commit reads it without error; CAS
  conflict in either order converges; "the row is below 128 KiB afterwards".
- #833 fixture `legacy_session`
  (`crates/daemon/src/transform_meta_bound.rs:115`): H = 1,000 seeded segments,
  one pass, a firing `AwaitingProducer`, then a commit that writes an identity
  row for every covered mid (1,998 plus the 300-message window) and a direct SQL
  `json_set` that embeds a `block_identity_by_mid` map in `meta` until the row
  holds 480 to 512 KiB (asserted). `assert_pruned` (`:166`): identities equal
  the submitted window, `meta` under 128 KiB, firing still `AwaitingProducer`.
- `:177` restart: drops the store, reopens the directory, asserts the load
  holds 2,298 identities, then a `SOFT+` pass (not HARD) commits and prunes.
  `:195` prune loses: a writer commits inside the attempt hook (its own
  commit already shrinks `meta` below 128 KiB); the retry prunes the rows.
  `:230` writer loses: a stale commit at the legacy version gets
  `CasConflict`; the reloaded writer commits the pruned map.
- Prune placement: `prune_block_identities`
  (`crates/daemon/src/transform.rs:5260`) runs on every ordinary committing
  plan, so a Defer-only session prunes (the falsifier's case).
- #831 review F8: #881 adds
  `a_legacy_meta_row_with_the_retired_divergence_counter_loads`
  (`crates/memory-store/src/lib.rs:25989`), which loads a legacy `ModuleMeta`
  row still carrying `boundary_divergence_pending_count`; it ran in #881's
  `cargo test -p memory-store` gate (head `1c66f16c`). That is decode
  compatibility, not the prune.
- Since `e15a09a6` (on `main` before M1) `ModuleMeta::block_identity_by_mid`
  is `#[serde(skip)]` (`crates/memory-store/src/lib.rs:2110-2111`) and
  persisted in the `block_identities` table, one row per mid
  (`crates/memory-store/baseline.sql`). `load_block_identities`
  (`lib.rs:4450-4468`) reads every row of the session (`SELECT mid, identities
  FROM block_identities WHERE session_id = ?1`) on `load` (`:7470`) and on the
  transform snapshot load (`:7659`); `sync_block_identities` (`:4481`) reads
  the same rows and diff-writes them at commit (`:9877`).
- `ModuleMeta` derives `Deserialize` without `deny_unknown_fields`
  (`lib.rs:1922-1923`), so a blob written before `e15a09a6` that still carries
  a `block_identity_by_mid` key reads without error, the key is ignored, and
  the next commit re-serializes the blob without it; no code moves such a key
  into the table (`git grep` finds no reader of the key at `f2442b2f`).

Gate results of the runs cited here, as recorded in the PR descriptions:

- #881 (#831; head `1c66f16c`, base `main` `be542f0c`): fmt, clippy, `test -p
  daemon` ok (one run hit a known timing flake in a 33-test binary; the rerun
  was green), `-p memory-store`, `check -p storage --no-default-features`, `-p
  host-runtime --test protocol_vectors`, doctests, markers, `bun run
  check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 9 pass, 14 fail on #881 alone (by design: the revision 2 plugin
  is refused) and 23 pass, 17 skip, 0 fail with #883.
- #833 (`window-protocol/m1-exit`; gates at the final head, code at
  `3ebfc3b9`): fmt, clippy, `-p daemon` 2,749 passed, `-p memory-store` 339,
  `-p storage` 108, doctests 19, storage no-default check, markers,
  `check:repo`, fixture build ok; fixture-contract 6 pass; manifest ok;
  `test:rust` 23 pass, 17 skip, 0 fail.

## Failure scenario

A session written before revision 3 carries 480 KiB of identities. It only
ever defers, so a HARD-only prune never runs and the next identity append
crosses 512 KiB.

## Timing windows and dependencies

Restart before the first commit; CAS conflict at the first commit.

## What a test must construct

Seed a legacy meta row near 512 KiB with a summarizer state; restart and read;
run a Defer-only committing pass; assert the map equals the window mids and
the row is under 128 KiB; inject a CAS conflict in each order and assert
convergence.

## Investigation log

### Q: Which resolutions prune?

- Sources examined: #833 ticket; D12; `prune_block_identities` at
  `transform.rs:5260`.
- Findings: Every committing plan except a revert before its HARD;
  pass-through paths return before the call.
- Missing evidence: None.
- Conclusion: resolved with answer; the restart test's `SOFT+` pass shows a
  non-HARD prune.

### Q: Which store object does WP-P25 bound on the landing tree?

- Sources examined: `crates/memory-store/src/lib.rs:1922-2111`, `:4450-4550`;
  `e15a09a6`; #833 implementation report and fixture.
- Findings: Identities live in `block_identities` rows; a pre-`e15a09a6`
  `meta` blob's embedded key is ignored on load and dropped by the next
  commit. #833 applies the obligation to both: the rows are pruned to the
  window, the `meta` row ends under 128 KiB.
- Missing evidence: The owner's confirmation of this reading.
- Conclusion: resolved with answer by #833's adaptation; recorded for owner
  confirmation with the PR.

## Dated note (2026-10-12, #989)

Dated note, 2026-10-12, #989 (spec #988 amendment A1): the host wire has one
unversioned contract, so every clause here that names a protocol version,
numbered revision, version-skew refusal, or retired-field refusal is
invalidated. At the #989 head, routed envelope, context, memory-capture,
note-evaluation, discovery, and transform bodies carry no `v`; transform
ignores a field named `v` and the former carriers, strict bodies refuse them
as unknown fields, `transform_revision_unsupported` and
`transform_tail_delta_retired` no longer exist, `transform/revision_3.rs` is
`transform/window_admission.rs`, `transform/revision_goldens.rs` is
`transform/recorded_goldens.rs`, and the golden fixture `transform-revision-2-goldens.json` is now
`transform-recorded-goldens.json`; `serve_native` remains until #993. Line
anchors into files #989 changed are as of their earlier recorded head.

## Dated note (2026-10-12, #993)

Dated note, 2026-10-12, #993 (spec #988 milestone U3): the transform request
carries no `serve_native`. The `pi` and `opencode-aisdk` profiles send
`native_messages` as their only window, the daemon derives the pass's CK from
it through the profile's codec before any projection and always serves native
recipes, and the `owned-llmrunner` and `claude-code-anthropic` profiles send
`messages`. Every clause here that names `serve_native`, a CK window sent
beside the native window, or the Pi check that aligned rows with `messages` is
invalidated; line anchors into files #993 changed are as of their earlier
recorded head.
