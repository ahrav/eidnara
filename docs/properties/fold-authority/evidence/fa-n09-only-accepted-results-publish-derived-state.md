# fa-n09-only-accepted-results-publish-derived-state

## Discovery trigger

Proposed obligation FA-N09 of spec #834 (D4b, last-accepted derived state),
owned by ticket #858 and landed by PR #906 (head `f6dde3fa3`, merged as
`74e347d9e`). Served fingerprints and tail-baseline parts leave the durable
row and live in the daemon's per-session snapshot cache, so a new publication
contract decides which pass may write them.

Exercised status: yes - the promotion family, including a child-process
SIGKILL cut, ran green in #906's recorded workspace gate and again in the
#859 PR A run at `fd0b52aa5`.

## Evidence trail

Code references are verified at `0ff62b29a` (branch `fold-authority/m2-exit`).

- Proposal and acceptance: `ProposedDerivedState` carries the revert epoch,
  the row version the pass read, the served fingerprints, and optional
  baseline parts (`crates/daemon/src/derived_state.rs:62-67`). `accept`
  (`:70-92`) takes the committed row version when the pass committed;
  otherwise it re-reads `load_fold_authority(session).row_version` and
  returns `None` unless it equals the read version. A load error also
  discards.
- Supersession: `DerivedState::supersedes` compares
  `(revert_epoch, accepted_row_version)` with `>=` (`:30-33`), so an equal
  pair replaces and an older pair is ignored.
- Lease: `first_unit` leases the prior state and then calls `begin`, which
  turns Ready into InFlight (`crates/daemon/src/lib.rs:8899-8900`). Since
  `96aad0baf` (#906 on `main`) the first transform's lease lives in
  `PassStart` (`:3393-3397`) and drops when that transform returns
  (`:9379-9381`); a rerun (`PassState::Reload`) leases for its own
  transform only and drops it after (`:9305-9313`). `lease_derived`
  charges the active-lease budget through `LeaseCharge` (`:2141-2150`,
  `:1944-1971`). The lease reaches the transform as `ProducerContext.derived`
  (`:9238`).
- Promotion: `run_transform` calls `transform_with_projection_cached`, then
  `promote_derived_state`, then `finished()` (`:9247-9249`), so a failed
  transform returns before promotion and a post-commit boundary-read error
  is returned after it. `promote_derived_state` (`:9252-9281`) accepts the
  proposal and calls `promote_derived` with the pass's snapshot generation.
  The fixture halt point is at `:9270-9275` under the `direct-host-fixture`
  feature.
- Fence: `promote_derived` (`:2152-2188`) refuses unless
  `generation_present_in_flight_or_ready` holds (`:2241-2250`) and refuses a
  state that does not supersede the retained one. Since `96aad0baf` a
  promotion that carries no baseline parts keeps the parts of the state it
  replaces when both have the same revert epoch (`:2161-2170`). `remove` clears
  derived
  state with the snapshot entry (`:2252-2258`). Retained bytes share
  `ready_bytes` and `retained_lru` with Ready snapshots, and eviction removes
  derived entries (`:2123-2139`).
- Native folds: `apply_additive_only` sets `derived: None`
  (`crates/daemon/src/transform.rs:3162`), and `apply_once` enters it unless
  the stored authority folds (`:3217-3218`).

Gate results as recorded in the PR descriptions:

- #906 (#858; gate run at `1d2cd55a0`, base `871ebfb08`): `cargo +1.98 fmt --all
  -- --check`, clippy `--workspace --all-targets --all-features --locked -D
  warnings`, rustdoc `-D warnings`, `cargo +1.98 test --workspace
  --all-features --locked --no-fail-fast`, and
  `scripts/forbid-comment-markers.sh` all pass. The run log behind the test
  gate records 6,079 passed, 0 failed, 66 ignored, and lists every test
  named in this record as `ok`.
- #859 PR A (`fd0b52aa5`): workspace tests 6,124 passed, 0
  failed, 66 ignored; fmt, clippy, rustdoc, and the marker script green.

## Failure scenario

A pass loses its compare-and-swap, or its session is deleted while it pauses
after commit, and it still writes its fingerprints into the cache. The next
pass then decides hint deferral and divergence against fingerprints that no
accepted pass served, or a recreated session inherits the deleted
incarnation's baseline parts.

## Timing windows and dependencies

- Between the commit and `promote_derived`: session delete, purge, route
  replacement, recomp `begin`, eviction, caller cancellation, process death.
- Between a no-write pass's read and its promotion: another writer advances
  the row version; `accept` discards.
- Under the per-session lane a sibling route's pass waits until the first
  pass has promoted, so it reads the newer state.
- The lease budget (8 active leases, `crates/daemon/src/lib.rs:890`) can
  refuse `lease_derived`; the pass then reads absence.

## What a test must construct

A folding session with a retained state; a pause at promotion; in that pause
each of: a rival commit, a `session.delete`, a purge, caller cancellation,
a SIGKILL of the daemon process; a no-write pass after the retained state was
removed; assertions on the retained `(revert_epoch, accepted_row_version)`,
the lease budget count and bytes, and the reopened store's row version.

## Investigation log

### Q: Does a no-write pass promote at a stale read?

- Sources examined: `derived_state.rs:70-92`; the unit test at `:148`.
- Findings: No. The test commits twice and checks that a proposal read at the
  first version is refused once the second exists, and accepted with an
  explicit committed version.
- Missing evidence: None.
- Conclusion: resolved with answer.

### Q: What does a lease-budget refusal do to retained parts?

- Sources examined: `lib.rs:2141-2150`, `:8899`; `transform.rs:7951-7966`.
- Findings: `lease_derived` returns `None`; `carried_derived_state` then
  carries no parts. Before `96aad0baf` the accepted proposal replaced the
  retained state, so the parts became absent and FA-N11 read the baseline
  invalid until a bust. Since `96aad0baf` (#906 on `main`), `promote_derived`
  keeps the replaced state's parts when the new state carries none and both
  share a revert epoch (`crates/daemon/src/lib.rs:2161-2170`);
  `a_pass_refused_a_lease_keeps_the_retained_baseline_parts` (`:35260`) and
  `a_promotion_without_parts_keeps_the_retained_parts_of_its_epoch_only`
  (`:35293`) witness it, and
  `an_emergency_pass_holds_a_derived_lease_only_inside_each_transform`
  (`:35318`) witnesses the lease scope. `parts_for` still checks the
  baseline generation on read.
- Missing evidence: None.
- Conclusion: resolved with answer: the owner's fix keeps same-epoch parts.

### Q: Is the reference model from the verification strategy present?

- Sources examined: #906 tests; spec Testing Seams.
- Findings: No `(epoch, accepted_version, derived_payload, lease)` model is
  built; each barrier test asserts retained state directly, and the cache
  unit test uses an independent capacity oracle for bytes.
- Missing evidence: The owner's acceptance of the substitution.
- Conclusion: needs human input.

### Q: Does the invariant hold at every `promote_derived` call?

- Sources examined: `crates/daemon/src/lib.rs:2152-2188`, `:9247-9281`;
  `crates/daemon/src/transform.rs:1742-1747` at `0ff62b29a`; portfolio
  evaluation I1.
- Findings: No, only on successful installation. Correct code also calls it
  with stale or revoked proposals and rejects them, leaving the retained
  state unchanged, or absent for an over-budget state. An accepted commit
  whose boundary read fails promotes before `finished()` returns the error.
- Missing evidence: None.
- Conclusion: resolved with answer; the Check is restated on installation.
