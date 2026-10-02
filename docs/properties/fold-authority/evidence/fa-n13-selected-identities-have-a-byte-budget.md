# fa-n13-selected-identities-have-a-byte-budget

## Discovery trigger

Proposed record FA-N13 of specification #834 (D4a), owned by ticket #859:
#859 PR A (`fold-authority/m2-exit`, `fd0b52aa5`) adds the budget and #859
PR C (`fold-authority/m2-exit-bounds`, code tree `0ff62b29a`) adds the reserve and
publish witness and a generated-history property. The spec's
existing-behavior record FA-E04 showed that firing assembly copied every
identity in the selected range with no byte budget, so a rendered-token
budget did not bound the durable identity bytes the firing writes into
metadata.

Exercised status: yes - #859 PR C's recorded run at `0ff62b29a` (full
workspace test run, 6,156 passed, 0 failed, 66 ignored; fmt, clippy,
rustdoc, and the comment-marker script green) ran the handler reserve and
publish test, the generated-history proptest, the three assembler oracle
tests, and the handler no-fire test named below. #859 PR A's run at
`fd0b52aa5` (6,124 passed) ran the last four.

## Evidence trail

All references are verified at `0ff62b29a`.

- `SELECTED_IDENTITY_BUDGET_BYTES = 256 * 1024`
  (`crates/daemon/src/history_summarizer_chunk.rs:97`).
- `IdentitySelection` (`:100-106`) starts its running length at `"[]".len()`
  (`:113`). `admit` (`:119-150`) serializes each entry with `serde_json`,
  adds one byte per separator, and compares the total with the budget. Over
  the budget it returns `false`, and on the first block it records
  `refused_len` (`:139-142`). Only an admitted block extends the selection
  and records a missing mid (`:144-149`).
- `Builder::flush_current_block` (`:368-427`) charges tokens first, then
  identities (`:386-395`). A refusal of either kind returns the block to
  `current_block` and truncates the aliases it issued (`:393-395`), so the
  chunk's text, snapshot, and selection are built from the same prefix of
  whole blocks. The chunk is the maximal contiguous whole-block prefix that
  both checks admit; when the token check admits the full candidate, the
  identity check selects its longest fitting prefix.
- The token first-block exception (`self.total_tokens > 0`, `:387`) does not
  guard the identity charge: a first block over the identity budget refuses.
- `assemble_history_summarizer_firing` builds with an identity selection
  (`:973-980`), repeats the build for a placeholder reach (`:981-997`), and
  returns `IdentityBudget { serialized_bytes, budget }` before any prompt or
  fingerprint work (`:999-1006`). The variant is at `:648-651`.
- The public `build_history_summarizer_chunk` builds with no selection
  (`:484-500`). Its production caller is reattachment
  (`crates/daemon/src/lib.rs:5078-5084`), which reuses the stored
  `selected_range_identities` (`:5072-5076`) rather than re-selecting.
- The reservation passes the assembled selection to `fire`
  (`crates/daemon/src/history_summarizer.rs:1866-1874`), inside the same
  store load and commit as the firing state.
- Tests: `BudgetCase` (`history_summarizer_chunk.rs:1743-1949`) gives every
  message two block identities (a 64-hex text identity and a large tool-call
  fingerprint) and an oracle that serializes whole prefixes of blocks
  independently (`:1831-1833`, through `identity_prefix_oracle` at
  `:1369-1376`). `assert_matches_oracle` (`:1887`) asserts
  the fired end ordinal, the exact selection (`:1910-1914`), the serialized
  length under the budget, text and fingerprint equal to an unbudgeted build
  of the same prefix, and that the store holds a default firing state
  (`:1875-1883`).
- `an_over_budget_selection_stops_at_the_longest_prefix_of_whole_blocks`
  (`:1960`), `an_indivisible_first_block_over_the_identity_budget_no_fires`
  (`:1981`), and `escaped_and_unicode_mids_are_charged_as_they_serialize`
  (`:1995`) each run budget - 1, exact, and + 1.
- `a_generated_history_fires_the_longest_whole_block_prefix_within_the_identity_budget`
  (`:2169`, 64 cases) generates 1 to 13 turns of user, assistant, tool
  exchange, system, and blank-user noise roles, mid prefixes that escape
  (`:2040`), and zero to three block identities per message of under 48,000
  bytes, then runs `assert_matches_oracle` (`:2179-2180`). The unbudgeted
  comparison build starts at the firing's own `from_ordinal` (`:1923-1929`).
- `an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix`
  (`crates/daemon/src/lib.rs:36313`) sends 800 alternating user and
  assistant messages whose mids carry 100 control characters through a real
  transform. From the identities the pass stored it computes the oracle
  prefix with `identity_prefix_oracle`, requires it to end before the last
  message, and asserts the reservation (`AwaitingProducer`, `firing_seq` 1,
  the exact selection, the chunk range, and the fingerprint of an unbudgeted
  build), a prompt that ends at the prefix, and after release one published
  segment over the prefix, an Idle state, and `counters.published == 1`.
- `an_indivisible_block_over_the_identity_budget_no_fires_and_reserves_nothing`
  (`crates/daemon/src/lib.rs:36285`) sends 4,000 assistant messages whose
  mids carry 100 control characters (6 escaped bytes each) through a real
  transform; the one same-role block exceeds the budget. It asserts
  `assemble:IdentityBudget`, zero producer starts, and an Idle durable state
  with `firing_seq` 0, no selection, and no chunk range.
- Recorded review (`/tmp/opencode/fa/review859-context.md`): a negative
  control that makes `admit` accept any size fails all three budget tests.

## Failure scenario

A chunk of many tiny messages fits the token budget but its identity vector
is larger than the durable-text headroom. The reservation writes it into
`ModuleMeta.history_summarizer`, the commit is refused by the 512 KiB guard,
and every later pass repeats the refusal. Truncating only the identity
vector instead would let publication accept messages it never fenced.

## Timing windows and dependencies

None at assembly: the builder is single-threaded over one snapshot. The
admission and the selection share the reservation commit.

## What a test must construct

A shrinkable selection, an indivisible first block, and escaped or Unicode
mids at budget - 1, exact, and + 1, compared with an independent prefix
oracle; a no-fire that reserves nothing; and a shrunk chunk carried through
reservation and publication. #859 PR C constructs the last one.

## Investigation log

### Q: Does any test reserve and publish a shrunk chunk with its exact selection?

- Sources examined: the tests above; `lib.rs` handler tests naming
  `IdentityBudget`; `git show bdf564e3a`.
- Findings: At `d7b330113` the oracle tests stopped at the assembled `Fire`
  outcome and the handler test covered only the indivisible no-fire. #859
  PR C adds `an_over_budget_history_reserves_and_publishes_the_longest_fitting_prefix`,
  which reserves and publishes a chunk the identity budget shrank, with its
  exact selection.
- Missing evidence: None.
- Conclusion: resolved with answer: yes, since #859 PR C.

### Q: Does reattachment bypass the budget by building without a selection?

- Sources examined: `lib.rs:5061-5091`.
- Findings: Reattachment rebuilds text for the stored range and reuses the
  stored selection that admission already bounded.
- Missing evidence: None.
- Conclusion: resolved with answer: no bypass.

### Q: Is `Exercised: yes` supported for the atomic-admission clause?

- Sources examined: `crates/daemon/src/history_summarizer_chunk.rs:1776-1782`,
  `:1835-1884`; `crates/daemon/src/lib.rs:36285-36309` at `0ff62b29a`;
  portfolio evaluation I3 and W2.
- Findings: At `d7b330113`, no: the oracle stopped at assembly, the handler
  test covered only the indivisible refusal, and the budget-edge fixtures
  are synthetic stress states with 30,000-character fingerprints. At
  `0ff62b29a` the reserve and publish handler test derives its identities
  from a real transform, so the shrink is production-shaped.
- Missing evidence: None.
- Conclusion: resolved with answer: `Exercised: yes` since #859 PR C.
