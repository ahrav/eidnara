# fa-n11-baseline-validity-requires-a-matching-join

## Discovery trigger

Proposed obligation FA-N11 of spec #834 (D4c, cold-record semantics), owned
by ticket #858 and landed by PR #906 (head `f6dde3fa3`, merged as `74e347d9e`).
The tail-hygiene
baseline keeps its scalars in the durable row while its measured parts,
excluded-prefix length, and prefix digest move to the daemon cache, so every
consumer must pair the two before trusting the baseline.

Exercised status: yes - the join table, the status join, the key test, and
the restart and crash-cut assertions ran green in #906's recorded workspace
gate and in the #859 PR A run at `fd0b52aa5`. The diagnostics-only commit
in the status witness is a direct `store.commit`.

## Evidence trail

Code references are verified at `0ff62b29a`.

- Key: `DerivedState::parts_for(revert_epoch, baseline)`
  (`crates/daemon/src/derived_state.rs:18-28`) returns parts only when a
  durable baseline exists, the retained revert epoch matches, and
  `parts.baseline_generation == baseline.baseline_generation`. The cache
  holds one entry per session id (`crates/daemon/src/lib.rs:2017`), which is
  the session-identity key.
- Evaluation: `prior_baseline_parts` (`crates/daemon/src/transform.rs:7933-7939`)
  feeds `refresh_tail_hygiene_baseline` on both branches (`:4953-4976`). The
  helper (`crates/daemon/src/tail_hygiene.rs:1080-1165`) keeps an
  already-invalidated baseline invalid outside a bust (`:1097-1104`), builds
  generation `previous + 1` on a bust or with no previous baseline
  (`:1105-1133`), and otherwise returns an invalidated baseline with no parts
  when `previous_parts` is absent or of another generation (`:1136-1140`)
  before `same_measured_prefix` runs (`:1141`).
- Status: `session.status` builds `tail_hygiene` only when the durable row
  has a baseline and sets `generation_invalidated` when the retained parts do
  not join, and `evaluable` to the durable flag and not invalidated
  (`crates/daemon/src/lib.rs:6881-6899`).
- Carry: a pass carries prior parts forward only when its revert epoch
  equals the loaded one (`crates/daemon/src/transform.rs:7941-7956`), and a
  bust pass replaces them with its measured parts (`:5131-5134`).
- Durable effect: only a bust writes the refreshed baseline to meta
  (`:4963`); the non-bust branch evaluates the join for this pass's nudge and
  directive decisions (`:4966-4976`, used at `:4994` and `:5146`). The
  durable scalars therefore can read `evaluable: true` while the join says
  invalid; every consumer rejoins.

Gate results as recorded in the PR descriptions:

- #906 (#858; gate run at `1d2cd55a0`): fmt, clippy `-D warnings`, rustdoc `-D
  warnings`, workspace tests (`--all-features --locked --no-fail-fast`), and
  the marker script pass; the run log records 6,079 passed, 0 failed, 66
  ignored, with each named test `ok`.
- #859 PR A (`fd0b52aa5`): 6,124 passed, 0 failed, 66 ignored.

## Failure scenario

After a restart the durable scalars say evaluable, the parts are gone, and a
consumer treats the missing parts as an empty prefix. The empty prefix
matches any current sequence, so the pass computes turn deltas and issues a
hygiene nudge from a baseline that no longer describes the served prefix.

## Timing windows and dependencies

- Restart, eviction, or `remove` between the bust that measured the parts
  and a later non-bust pass or status read. A lease-budget refusal no longer
  drops the parts: since `96aad0baf` a promotion that carries none keeps the
  retained parts of the same revert epoch.
- A diagnostics-only commit advances the row version without changing the
  baseline generation; the join must still match.
- A reset advances the revert epoch; retained parts of the old epoch must not
  join.

## What a test must construct

Durable scalars at generation G with, separately: no retained parts, parts
at G + 1, empty parts at G, and matching parts at G; a no-baseline control;
a diagnostics-only commit between promotion and the status read; a restart
followed by a non-bust pass and then a bust.

## Investigation log

### Q: Does "invalidated until an accepted bust" hold without a durable flag?

- Sources examined: `transform.rs:4953-4976`, `:7941-7956`;
  `tail_hygiene.rs:1105-1133`; `lib.rs:2152-2188`.
- Findings: Yes. Parts are produced only by a bust refresh or a refresh with
  no durable baseline; a non-bust pass carries only parts it leased, and
  promotion never installs an older state. Once parts are absent under a
  durable baseline, no non-bust pass can bring matching parts back.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: Is the diagnostics-only commit witnessed through a transform pass?

- Sources examined: `lib.rs:35724`
  `status_reads_hygiene_validity_joined_with_the_retained_parts`.
- Findings: The test bumps `last_committed_pass_at_ms` and commits through
  `store.commit`; validity stays `(true, false)`. No transform-driven
  diagnostics-only commit is asserted. The key is generation-only by
  construction (`derived_state.rs:18-28`).
- Missing evidence: A transform pass whose commit changes only diagnostics.
- Conclusion: needs human input on whether the direct commit suffices.

### Q: Is the revert-epoch half of the key exercised?

- Sources examined: `derived_state.rs:122`.
- Findings: Yes, at unit level: `parts_for(1, ..)` on an epoch-2 state is
  `None`.
- Missing evidence: A Handler-level reset between promotion and read.
- Conclusion: resolved with answer at unit level.

### Q: Do matching parts always evaluate, and missing parts always invalidate?

- Sources examined: `crates/daemon/src/tail_hygiene.rs:1080-1165`,
  `:2210`, `:2327-2349`; `crates/daemon/src/lib.rs:6881-6899` at
  `0ff62b29a`; portfolio evaluation I2.
- Findings: No. A bust or first refresh rebuilds whatever the prior parts; a
  non-bust over an invalidated baseline stays invalid despite matching
  parts; a disagreeing prefix invalidates after a matching join.
- Missing evidence: None.
- Conclusion: resolved with answer; the Check lists the four cases in
  order.

### Q: Does `main`'s `96aad0baf` change the join?

- Sources examined: `git show 96aad0baf`; `crates/daemon/src/lib.rs:2152-2188`,
  `:35255`, `:35288`; `crates/memory-store/src/lib.rs:2075-2079`, `:29551`.
- Findings: The join rule is unchanged; `parts_for` still checks the
  baseline generation on read. A promotion without parts now keeps the
  replaced state's parts of the same revert epoch, so a lease-refused pass
  leaves the retained parts in place and `session.status` still reports the
  baseline evaluable (`a_pass_refused_a_lease_keeps_the_retained_baseline_parts`);
  a promotion in a new epoch drops them
  (`a_promotion_without_parts_keeps_the_retained_parts_of_its_epoch_only`).
  The durable baseline also serializes an empty `baseline_parts` array so a
  build that still requires the key loads rows this build writes
  (`a_row_this_build_writes_loads_under_the_definition_that_requires_baseline_parts`).
- Missing evidence: None.
- Conclusion: resolved with answer: evidence updated; the record stays
  `active`.
