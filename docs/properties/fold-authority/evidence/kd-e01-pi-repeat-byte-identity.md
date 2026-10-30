# kd-e01-pi-repeat-byte-identity

## Discovery trigger

Existing-behavior record KD-E01 of #1060's Property Catalog section A
(`kd-e-pi-repeat-byte-identity`). Ticket #1061; PR #1068 (head `f04c54e4d`,
`cargo +1.98 nextest run --profile ci -p daemon --lib`, 1,871 passed).
Surface: daemon.

Exercised status: yes - the native-authority Pi witness the specification
assigned to U1.

## Evidence trail

All references are verified at `07ea883b5`, the head of #1071, which holds
#1068 to #1071.

- Every response allocates a fresh output revision
  (`crates/daemon/src/edit_recipe.rs:102`).
- A native pass keeps the retained output as a `previous` source only when
  the caller offers that revision and the revert epoch matches
  (`crates/daemon/src/lib.rs:2583-2590`, read at `:9751-9758`).
- `a_repeated_native_pi_window_reconstructs_identical_rows`
  (`crates/daemon/src/pi_native_serving_tests.rs:778`) binds native with a
  live chain and repeats the closed-role window (`:312`). Its arm table
  names the expected `committed` value: `false` on two unchanged repeats,
  `true` on a new usage reading, and none for a repeat whose retained
  output was cleared from the test client.
- Each repeat asserts serialized rows equal to the first pass's, a fresh
  revision, `previous_output_revision` equal to the prior revision, and the
  recipe `[keep(source: previous, start: 0, count: n + 2)]`
  (`:611` `assert_whole_previous_keep`). The retention-lost arm asserts zero
  `previous` keeps and the first-pass recipe (`:594`).
- `committed` is compared with whether `meta` or `row_version` changed on
  every arm; `assert_native_commit` (`:621`) runs after each pass.

## Failure scenario

A repeat that re-rendered m0 or m1, or re-inserted copies of the submitted
rows, would serve identical bytes while losing the retained rows; only the
whole-output keep distinguishes them. A repeat that reported
`committed: false` after a usage write would hide a metadata commit.

## Timing windows and dependencies

A repeat beside a render change (a memory write, an epoch bump) ends
byte-stable replay on purpose; KD-E02 covers that. A lost retained output
forces the input recipe.

## What a test must construct

Two accepted passes with stable clock, notes, configuration, and memory; a
repeat with new usage; a repeat with no retained output.

## Investigation log

### Q: Is `committed: false` asserted only when metadata is unchanged?

- Sources examined: `pi_native_serving_tests.rs:778-841`.
- Findings: Yes. The arm table asserts `false` only on the unchanged arms,
  `true` on the usage arm, and the equivalence with a metadata change on
  every arm (#1060 KD-E01 correction).
- Missing evidence: None.
- Conclusion: resolved with answer.
