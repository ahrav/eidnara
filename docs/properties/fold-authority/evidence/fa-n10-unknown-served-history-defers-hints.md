# fa-n10-unknown-served-history-defers-hints

## Discovery trigger

Proposed obligation FA-N10 of spec #834 (D4c, cold-record semantics), owned
by ticket #858 and landed by PR #906 (head `f6dde3fa3`, merged as `74e347d9e`).
Once served
fingerprints live only in the daemon cache, a restart or an eviction leaves
no record of what the provider saw, so "no record" must not read as "nothing
was served".

Exercised status: yes - the predicate table, the fixture restart lifecycle,
and the divergence-unknown test ran green in #906's recorded workspace gate
and in the #859 PR A run at `fd0b52aa5`.

## Evidence trail

Code references are verified at `0ff62b29a`.

- Predicate: `user_hint_deferred(hint_text, served, block_id, is_bust_pass)`
  (`crates/daemon/src/transform.rs:7916-7934`) returns false for an empty
  hint or a bust, true when `served` is `None`, and otherwise true only when
  a served entry equals the block id or, for block 0, the bare mid.
- Retained history: `prior_served` (`:7936-7941`) reads
  `ctx.derived?.served_for(meta.revert_epoch)`; `served_for`
  (`crates/daemon/src/derived_state.rs:14-16`) returns `None` for another
  revert epoch, so a reset also reads unknown.
- Admission: `apply_once` calls the predicate with the loaded meta and
  `is_bust_pass` (`crates/daemon/src/transform.rs:4222-4227`). A deferred
  hint when `pending_user_hint_block_ids` already holds
  `MAX_PENDING_USER_HINT_BLOCK_IDS` (16, `:148`) is skipped as
  `UserHintSkip::DeferralsFull` (`:4228-4232`); otherwise it is recorded and
  pushed with `deferred` set (`:4233-4252`). #859 PR A added the cap.
- Divergence: every call site uses
  `prior_served(ctx, &loaded.meta).and_then(|prior| first_divergence(...))`
  (`:3553-3554`, `:3631-3632`, `:5135-5136`), so unknown history reports no
  divergence.
- Native folds: the additive path neither decides hints nor carries derived
  state (`:3162`, gate `:3217-3218`).
- At `265df096`, `user_hint_target_was_served` read
  `meta.served_output_fingerprint` (`transform.rs:8082-8089`), and a missing
  field was an empty vector, so a cold session never deferred.

Gate results as recorded in the PR descriptions:

- #906 (#858; gate run at `1d2cd55a0`): fmt, clippy `-D warnings`, rustdoc `-D
  warnings`, `cargo +1.98 test --workspace --all-features --locked
  --no-fail-fast`, and the marker script pass. The run log records 6,079
  passed, 0 failed, 66 ignored, with each named test `ok`.
- #859 PR A (`fd0b52aa5`): 6,124 passed, 0 failed, 66 ignored; the
  deferral-cap fixture test is `ok`.

## Failure scenario

The daemon restarts between two passes of one session. The next non-bust
pass decides a new hint whose target block the provider has already cached,
reads the absent history as empty, and injects the hint into that block. The
served prefix changes on a pass that was not meant to bust the cache.

## Timing windows and dependencies

- Restart, eviction, `remove`, a lease-budget refusal, or a revert-epoch
  change between the pass that served the target and the pass that decides
  the hint.
- The next accepted pass promotes its own fingerprints, so the history is
  known again from then on; the deferred hint waits in
  `pending_user_hint_block_ids` for a bust.

## What a test must construct

A folding session whose retained history is absent, a non-bust pass with an
eligible non-empty hint, and observation that the hint is deferred and not
served; a known-empty control that admits; a bust control that admits; then
a later `HARD` that serves the deferred hint once.

## Investigation log

### Q: Does the deferral cap from #859 weaken FA-N10?

- Sources examined: `transform.rs:4222-4237`;
  `crates/daemon/tests/eval_surface_ledger.rs:533`
  `a_new_hint_is_skipped_while_every_deferral_slot_is_taken`.
- Findings: No. At a full deferral set the hint is skipped and nothing is
  recorded; it is not admitted. The fixture test admits the 16th slot,
  refuses at full with the stored set unchanged, and a bust frees all slots.
- Missing evidence: None.
- Conclusion: resolved with answer; the guarantee's falsifier (unknown
  history authorizes an immediate prefix mutation) still cannot occur.

### Q: Is the end-to-end deferral witnessed across a real restart?

- Sources examined: `crates/daemon/tests/eval_surface_ledger.rs:442`.
- Findings: Yes. With retained history a `SOFT+` pass admits and serves the
  hint; after a fixture restart the same hint is deferred and absent, a
  repeated `SOFT+` keeps it absent, and the next `HARD` serves it once.
- Missing evidence: None.
- Conclusion: resolved with answer.
