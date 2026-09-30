# fa-e08-empty-served-history-means-unserved-and-cold

## Discovery trigger

Existing-behavior record FA-E08 of spec #834 (first comment, at `265df096`;
the independent evaluation added the non-empty-text clause), surface daemon.
At `265df096` an empty `served_output_fingerprint` vector was the only
representation of "no prior service". #858 (PR #906) moves the served
history to the daemon cache and gives absence its own meaning (FA-N10); its
ticket states that FA-E08 is preserved for a genuinely empty vector and no
longer reachable from absence.

Exercised status: yes - the predicate table's "known empty" row and the
divergence table's empty-old row ran green in #906's recorded workspace gate
and in the #859 PR A run at `fd0b52aa5`. Both construct the empty vector
directly.

## Evidence trail

Code references are verified at `0ff62b29a` unless another tree is named.

- Divergence: `first_divergence` returns `None` for an empty old sequence
  (`crates/daemon/src/divergence.rs:42-48`), unchanged since `265df096`.
- Hint deferral: `user_hint_deferred` (`crates/daemon/src/transform.rs:7906-7924`)
  returns false for `Some(&[])` because no served entry matches; it returns
  true for `None` on a non-bust pass with non-empty text.
- At `265df096`, `user_hint_target_was_served(meta, block_id)`
  (`transform.rs:8072-8079` there) scanned `meta.served_output_fingerprint`,
  and admission deferred iff the text was non-empty, the target was served,
  and the pass was not a bust (`:4150-4156` there). At HEAD the admission is
  `crates/daemon/src/transform.rs:4212-4217`.
- Absence versus empty: the retained history is
  `Option<&[ServedBlockFingerprint]>` from `prior_served` (`:7926-7931`);
  `None` is absence (no cache entry, another revert epoch, or no lease) and
  `Some(&[])` is a known-empty history.
- Reachability: hint admission and divergence run only past the native gate
  (`:3207-3208`), which needs a stored Eidnara authority and so a non-empty
  chain (`crates/daemon/src/config.rs:181-185`); at `265df096` they ran by
  default.

Gate results as recorded in the PR descriptions:

- #906 (#858; gate run at `1d2cd55a0`): fmt, clippy `-D warnings`, rustdoc `-D
  warnings`, workspace tests (`--all-features --locked --no-fail-fast`), and
  the marker script pass; the run log records 6,079 passed, 0 failed, 66
  ignored, with the named tests `ok`.
- #859 PR A (`fd0b52aa5`): 6,124 passed, 0 failed, 66 ignored.

## Failure scenario

A pass holds a known-empty served history, treats it as having served the
hint's target, and defers every first hint of a session, or reports a
divergence against nothing and flags a cache bust that did not happen.

## Timing windows and dependencies

None for the helpers. A known-empty history exists only when an accepted
pass promoted an empty served vector.

## What a test must construct

An empty and a matching served vector; a block-zero bare-mid match; bust
and non-bust controls; an absent-history control that defers (FA-N10); an
empty old sequence against a non-empty new one.

## Investigation log

### Q: Is FA-E08 preserved or replaced by #906?

- Sources examined: #858 ticket; `transform.rs:7906-7924`;
  `divergence.rs:42-48`; the table at `transform.rs:13329`.
- Findings: Preserved for the empty vector: the "known empty" row expects no
  deferral and the empty-old divergence row expects `None`. The absent state
  is new and carries FA-N10's opposite answer for hints.
- Missing evidence: None.
- Conclusion: resolved with answer; `Status: active`.

### Q: Is a known-empty served history reachable in production?

- Sources examined: `served_output_fingerprints` callers
  (`transform.rs:3542`, `:3620`, `:5124`); `carried_derived_state`
  (`:7941-7956`).
- Findings: A promoted state carries the served fingerprints of the pass's
  output. No witness shows a folding pass whose served output has no blocks,
  and every Handler test observed serves at least one block.
- Missing evidence: A pass with an empty served output, or proof that none
  exists.
- Conclusion: unresolved, needs that pass or that proof.

### Q: Does the reachability label rest on a constructed known-empty state?

- Sources examined: `crates/daemon/src/transform.rs:3207-3208`, `:13347`;
  `crates/daemon/src/divergence.rs:164-165` at `0ff62b29a`; portfolio
  evaluation I4.
- Findings: No. The label rests on the folding gate; the helper tests supply
  the empty vector directly. Helper-state exercise is verified; production
  reachability is not.
- Missing evidence: A production pass that retains an empty served history,
  or proof that none can.
- Conclusion: unresolved, needs that pass or that proof.
