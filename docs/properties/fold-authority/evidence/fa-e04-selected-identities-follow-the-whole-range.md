# fa-e04-selected-identities-follow-the-whole-range

## Discovery trigger

Existing-behavior record FA-E04 of specification #834 (first comment),
describing `265df096`: firing assembly copies the identity vector of every
non-synthetic message in the selected range, a missing vector is a no-fire,
and no byte budget bounds the copy. #834's decisions and ticket #859 state
that FA-E04 is replaced by FA-N13; #857 (PR #905) preserved it while moving
identities to rows and listed "an exact multi-message selected-vector
assertion for FA-E04" as a follow-up.

Exercised status: yes - for the surviving clauses, through #859 PR C's
generated-history proptest, the fixed oracle tests, and the missing-identity
corpus test, all green in #859 PR C's run at `0ff62b29a` (6,156 passed).

## Evidence trail

References are verified at `0ff62b29a` unless another tree is named.

- At `265df096` (read there): the selection was a filter over
  `messages` with `!synthetic && start_index <= ordinal <= end_index`,
  returning `MissingBlockIdentity` on the first message without a vector
  (D`history_summarizer_chunk.rs:698-715`). The token budget truncated text,
  not the range.
- At HEAD the selection is built inside the chunk builder.
  `Builder::push_message` records system, empty tool, and empty user
  messages as pending noise (`crates/daemon/src/history_summarizer_chunk.rs:233-267`),
  and the next block takes them into its message list (`:284`, `:302`,
  `:337`). `flush_current_block` charges that list through
  `IdentitySelection::admit` (`:386-395`, `:119-150`), so the selection is
  the identities of every non-synthetic message from the chunk start to the
  last flushed block's last message.
- A message without a stored vector is recorded by `admit` (`:124-126`,
  `:144-146`) and returned as `MissingBlockIdentity` after the budget and
  size checks (`:1026-1029`).
- Replaced clause: `admit` stops the chunk at the 256 KiB serialized budget
  (`:139-143`), so the range itself now shrinks to fit. That is FA-N13.
- The identity map comes from the pass's projection
  (`crates/daemon/src/lib.rs:5625-5629` passes `&projection.identity_by_mid`),
  and publication validates the selection against the stored rows (#905,
  FA-E09).
- Witnesses: `BudgetCase::assert_matches_oracle`
  (`history_summarizer_chunk.rs:1887`) asserts the exact multi-message
  selection equals the oracle's prefix (`:1910-1914`) in
  `an_over_budget_selection_stops_at_the_longest_prefix_of_whole_blocks`
  (`:1960`), `an_indivisible_first_block_over_the_identity_budget_no_fires`
  (`:1981`), and `escaped_and_unicode_mids_are_charged_as_they_serialize`
  (`:1995`). This closes #905's follow-up for plain text ranges.
- `a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget`
  (`:2169`, #859 PR C, 64 cases) builds each case with `generated_case`
  (`:2042-2163`): turns of user, assistant, tool exchange, system, and
  blank-user noise in any order. Its expected blocks start at the first
  non-system message and take each unclassed system or noise message into
  the next classed block, so the expected selection holds every message in
  that span; `assert_matches_oracle` compares it with the firing's selection
  exactly.
  `history_summarizer_boundary_construction_matches_owned_reference`
  (`crates/daemon/src/lib.rs:18613`) asserts `MissingBlockIdentity` for the
  `synthetic-call` message at budget 32,000 (`:18831-18836`).
- `budget_stop_and_tool_only_ranges_are_recorded`
  (`history_summarizer_chunk.rs:2814`), the spec's cited check
  (D`:1744` at `265df096`), still checks tool-only ranges and the budget
  stop, not identities.

## Failure scenario

A selection that skips a message inside the fired range leaves that message
unfenced: publication would accept a summary over bytes it never compared.
Before M2, a selection with no byte budget could push metadata past the
durable-text guard.

## Timing windows and dependencies

None at assembly. Publication re-reads the stored rows inside its
transaction (#905).

## What a test must construct

A range mixing system messages, empty tool results, same-role runs, and
text; assert the selection equals the ordinal filter over
`[start_index, end_index]`; remove one vector and assert
`MissingBlockIdentity`. A coverage marker would count the generated cases
whose selected range holds a system or noise message.

## Investigation log

### Q: Does a test compare the builder-derived selection with the range filter on a range holding system or noise messages?

- Sources examined: the tests above; `lib.rs` tests naming
  `selected_range_identities`; `git show bdf564e3a --
  crates/daemon/src/history_summarizer_chunk.rs`.
- Findings: At `d7b330113` the oracle cases used plain user and assistant
  text, so noise never entered a block's message list. #859 PR C's proptest
  generates 64 cases whose turns include system and blank-user noise roles
  and asserts the exact selection. Whether a case places such a message
  inside the selected range is probabilistic, and no assertion counts those
  cases.
- Missing evidence: None for the selection; a counted marker would make the
  construction certain per run.
- Conclusion: resolved with answer: yes, probabilistically per run.

### Q: Which part of FA-E04 is replaced?

- Sources examined: #834 decisions table and D4a; ticket #859.
- Findings: Only "with no separate byte budget". Range coverage and the
  missing-identity no-fire survive.
- Missing evidence: None.
- Conclusion: resolved with answer: invalidated by #859 PR A, replaced by
  FA-N13 for the budget clause.

### Q: Is `Exercised: yes` supported for the surviving range clause?

- Sources examined: `crates/daemon/src/history_summarizer_chunk.rs:1749-1782`
  at `0ff62b29a`; portfolio evaluation I3 and W2.
- Findings: At `d7b330113`, no: the oracle cases were plain text messages,
  synthetic stress states, and the record requires a mixed system and noise
  range. At `0ff62b29a` the generated-history proptest supplies that range.
- Missing evidence: None.
- Conclusion: resolved with answer: `Exercised: yes` since #859 PR C.
