# Window protocol (M1)

This part catalogs the window protocol of the transform, revision 3, as
specified by [#824](https://github.com/ahrav/eidnara/issues/824) and landed by
milestone M1 (tickets #826 to #833). It carries the 37 records of catalog
revision 2: WP-E01 to WP-E12 (existing behavior revision 3 must preserve or
deliberately invalidate) from #824 comment 3, and WP-P01 to WP-P25 (design
obligations, each with a falsifier in its `Check`) from #824 comment 4. The
record text follows catalog revision 2; `Reachability`, `Exercised`,
`Confidence`, and `Existing check` are re-derived here against the M1 code and
the M1 PR descriptions.

## Evidence boundary

`Exercised` statuses come only from runs recorded in M1 PR descriptions. A
status is `yes` when a PR's gate block records a green suite that contains a
named test which constructs the required faults and asserts the check. A
branch, an implementation report, or a local run is not a recorded PR run.
#831 is PR #881 and #832 is PRs #883 and #884; their gate blocks are the
recorded runs for revision 3. #833 ships this catalog, so its recorded run is
the gate block of its own PR description, run at its final head; a `yes`
that cites #833 holds only while that description records the gates listed
here. Test
adequacy is not reviewed here; that verdict belongs to
`/testing:invariant-test-review`.

Recorded runs used, read with `gh pr view <n> -R ahrav/eidnara` (#873 to #880
on 2026-09-26; #881, #883, and #884 on 2026-09-27, bodies re-read after their
merges). Every PR through #884 is merged; the last two columns name the merge
commit on `main` and the head it merged.

| PR | Ticket | Branch | Head at read | Base | Merge on `main` | Merged head |
| --- | --- | --- | --- | --- | --- | --- |
| #873 | #826 (1 of 2) | `window-protocol/m1-bounded-reads` | `efb3fe9c` | `main` | `d28489d0` | `efb3fe9c` |
| #874 | #826 (2 of 2) | `window-protocol/m1-bounded-reads-tags` | `c239461c` | `window-protocol/m1-bounded-reads` | `7b5ac638` | `f54ccb20` |
| #875 | #827 | `window-protocol/m1-coverage-authority` | `2d58cc20` | `window-protocol/m1-bounded-reads-tags` | `0b3465e1` | `2d58cc20` |
| #876 | #828 | `window-protocol/m1-retire-projection-reuse` | `f8940a26` | `window-protocol/m1-coverage-authority` | `0c3b415b` | `71a75933` |
| #877 | #829 (A) | `window-protocol/m1-plugin-shrink-first` | `3844a179` | `window-protocol/m1-retire-projection-reuse` | `bcf8ba36` | `3844a179` |
| #878 | #829 (B) | `window-protocol/m1-retire-delta-channel` | `f6b3d7bd` | `window-protocol/m1-plugin-shrink-first` | `72df0a7b` | `36871eaa` |
| #879 | #830 (1 of 2) | `window-protocol/m1-native-output-store` | `7a3db43b` | `window-protocol/m1-retire-delta-channel` | `cdefb33d` | `e6a78de2` |
| #880 | #830 (2 of 2) | `window-protocol/m1-retire-native-chunks` | `748c0c4b` | `window-protocol/m1-native-output-store` | `be542f0c` | `7dc45500` |
| #881 | #831 | `window-protocol/m1-daemon-revision-3` | `1c66f16c` | `main` | `6b611cd0` | `c6a1d384` |
| #883 | #832 (1 of 2) | `window-protocol/m1-plugin-revision-3` | `d7712d75` | `main` | `18b3fc2a` | `b93059b9` |
| #884 | #832 (2 of 2) | `window-protocol/m1-plugin-fail-open` | `f8734c12` | `main` | `d68aedf3` | `8055c935` |
| #833 PR | #833 | `window-protocol/m1-exit` | final head (code at `3ebfc3b9`; production code at `f2442b2f`) | `main` (`d68aedf3`) | - | - |

The heads of #873 (`efb3fe9c`), #874 (`08afec9d`, `cd596c15`, `c239461c`), and
#875 (`1431467c`, `2d58cc20`) carry commits pushed after their descriptions
were written, and the merged heads of #874, #876, #878, #879, #880, #881,
#883, and #884 carry further owner commits. #883's description adds a review
update with gates at `adcc7baf` (`check:repo` ok, markers ok, `test:rust` 23
pass, 19 skip, 0 fail); the other late commits have no gate block of their
own. All of them are on `main` `d68aedf3`, under #833, so #833's gates ran
with them. Records that depend on them say so.

Code references are verified at `f2442b2f`, the last code commit of #833
(`window-protocol/m1-exit`, base `main` `d68aedf3`); the later commits on
the branch change only documentation. References are cited at another tree
only where that tree is named. They were read at `f8734c12`, moved to
`52eda0fa` and then to `f2442b2f` with a line diff of each cited file; a
reference inside a changed hunk was re-read by hand, and every cited test
name was found at `f2442b2f` with `git grep`. Tests
deleted by a later M1 PR are cited at the head of the PR whose run recorded
them.

Paths are repository-relative. A bare `:line` refers to the file named last in
the same field.

## Scope

| Surface | Class | Included because |
| --- | --- | --- |
| Coverage presence, replay, and reconcile | Mixed | The state machine remains; anchor evidence changes (WP-E01, WP-P02). |
| Surviving-prefix recut | Mixed | The truncate and refold path receives the resolver's `keep_through_seq` (WP-E02, WP-E12). |
| No-survivor `pending_rewrite` | Mixed | D10 resets the null-anchor shape; the pass-through stays for `lineage_switched` (WP-E03). |
| Summarizer publication fence | Mixed | Identity and epoch checks must survive identity pruning (WP-E04, WP-P06, WP-P16). |
| Positional edit recipes | Mixed | Format remains; input coordinates are submitted-window coordinates (WP-E05, WP-P05, WP-P19). |
| Host publication | Mixed | In-place identity remains; unchanged-on-failure is weakened to shrink-first (WP-E06, WP-P09, WP-P20). |
| Capture leases and retained output | Mixed | Accounting remains; retained owners and charged data change (WP-E07, WP-P11). |
| `TransformSnapshotCache` | Mixed | Generation fence remains; the snapshot holds CK input only (WP-E08). |
| Tag protection and legacy tool ids | Mixed | Decisions remain; full-session reads are replaced (WP-E09). |
| Durable meta size | Mixed | The 512 KiB guard remains; history-dependent growth must go (WP-E10, WP-P25). |
| Revision 3 window and discovery protocol | New | Completeness, absence, ordering, and budget meanings (WP-P01, WP-P10, WP-P12, WP-P13, WP-P24, markers). |
| Per-session daemon serialization | New | Generation fencing does not provide capture-order execution (WP-P04, WP-P18). |
| Fold rendering horizon | New | Bounded read with byte-identical output (WP-P08). |
| First-user tool policy | Mixed | The database resolver replaces window-based cache-miss resolution (WP-P14). |
| Steady-state cost | New | End-to-end measurement and query-work evidence (WP-P07, WP-P15). |

Skipped, as in catalog revision 2: the frame protocol and shared-memory
transport (preserved; application revision 3 is a separate version), codec
unification and CK removal (revision 4), oversized unmanaged cold import
(non-goal; refusal belongs to window admission), host loading of all N
messages (outside plugin and daemon ownership), and distributed consensus (no
such mechanism on this surface).

### Authority model

- Host: the ordered message array and its membership. Other same-process code
  can mutate it.
- Plugin: capture ownership, source validation, publication, and promotion of
  its retained state.
- Daemon: coverage interpretation, effective ordinals, rendered coverage, and
  durable transform decisions.
- Store: committed history, `row_version`, `revert_epoch`, and durable
  selected-message identities.
- Derived state: plugin boundary, retained output, projections, summaries, and
  ready snapshots; none is independently sufficient evidence of current host
  membership.

## Reachability classes

Each record carries its own label and evidence. `default-production` means the
path runs without configuration: the plugin builds the Rust-mode transform
unconditionally (`packages/opencode-plugin/src/hooks/context/hook.ts:518` at
`f2442b2f`; `4061315c` removed `transform_mode`). `explicit-config-only` means
a configured summarizer model chain is required (`model_chain` defaults to
empty at `crates/daemon/src/config.rs:123`). There are eight campaign
markers (WP-P16 to WP-P23). Seven are `test-only`: they assert enabling state
through test instrumentation. WP-P16 is `explicit-config-only`, because its
production overlap needs a configured summarizer.

Distribution: 27 `default-production`, 3 `explicit-config-only`, 7 `test-only`.

## Index

| ID | Record | Type | Check | Surface | Status | Exercised | Owner |
| --- | --- | --- | --- | --- | --- | --- | --- |
| WP-E01 | [`wp-e01-boundary-presence-gates-reconcile`](#wp-e01-boundary-presence-gates-reconcile) | safety | `always` | daemon | active | yes | #880, #881 |
| WP-E02 | [`wp-e02-recut-keeps-the-surviving-prefix`](#wp-e02-recut-keeps-the-surviving-prefix) | safety | `always` | daemon | active | yes | #880, #881 |
| WP-E03 | [`wp-e03-no-survivor-arms-pending-without-truncation`](#wp-e03-no-survivor-arms-pending-without-truncation) | safety | `always` | daemon | active | yes | #833 |
| WP-E04 | [`wp-e04-publish-requires-current-identities-and-epoch`](#wp-e04-publish-requires-current-identities-and-epoch) | safety | `always` | store | active | yes | #873, #833 |
| WP-E05 | [`wp-e05-recipes-bind-positions-and-revisions`](#wp-e05-recipes-bind-positions-and-revisions) | safety | `always` | plugin | active | yes | #879, #877, #883 |
| WP-E06 | [`wp-e06-host-publication-is-in-place-and-all-or-none`](#wp-e06-host-publication-is-in-place-and-all-or-none) | safety | `always` | plugin | invalidated | yes | #875; #877 |
| WP-E07 | [`wp-e07-capture-charges-outlive-cancellation`](#wp-e07-capture-charges-outlive-cancellation) | safety | `always` | plugin | active | yes | #877, #883 |
| WP-E08 | [`wp-e08-ready-snapshots-reject-stale-generations`](#wp-e08-ready-snapshots-reject-stale-generations) | safety | `always` | daemon | active | yes | #878 |
| WP-E09 | [`wp-e09-tag-protection-is-session-relative`](#wp-e09-tag-protection-is-session-relative) | safety | `always` | daemon | active | yes | #874 |
| WP-E10 | [`wp-e10-durable-meta-bound-refuses-the-cliff`](#wp-e10-durable-meta-bound-refuses-the-cliff) | safety | `always` | store | active | partial | #873, #881, #833; refusal witness unowned |
| WP-E11 | [`wp-e11-head-fail-open-does-not-publish-a-failed-candidate`](#wp-e11-head-fail-open-does-not-publish-a-failed-candidate) | safety | `always` | plugin | invalidated | yes | #878, #884 |
| WP-E12 | [`wp-e12-revert-epoch-invalidates-retained-output`](#wp-e12-revert-epoch-invalidates-retained-output) | safety | `always` | store | active | yes | #879, #881, #833 |
| WP-P01 | [`wp-p01-window-is-the-complete-declared-suffix`](#wp-p01-window-is-the-complete-declared-suffix) | safety | `always` | plugin | active | yes | #875, #881, #883 |
| WP-P02 | [`wp-p02-anchor-resolution-uses-authoritative-snapshot`](#wp-p02-anchor-resolution-uses-authoritative-snapshot) | safety | `always` | daemon | active | yes | #875, #881 |
| WP-P03 | [`wp-p03-ordinals-derive-from-the-effective-anchor`](#wp-p03-ordinals-derive-from-the-effective-anchor) | safety | `always` | daemon | active | yes | #875, #881 |
| WP-P04 | [`wp-p04-session-serialization-preserves-capture-order`](#wp-p04-session-serialization-preserves-capture-order) | safety | `always` | daemon | active | yes | #875 |
| WP-P05 | [`wp-p05-recipe-keeps-use-submitted-window-coordinates`](#wp-p05-recipe-keeps-use-submitted-window-coordinates) | safety | `always` | daemon | active | yes | #875, #881 |
| WP-P06 | [`wp-p06-pruning-and-publication-share-the-meta-fence`](#wp-p06-pruning-and-publication-share-the-meta-fence) | safety | `always` | store | active | yes | #833 |
| WP-P07 | [`wp-p07-every-pass-store-read-has-a-work-bound`](#wp-p07-every-pass-store-read-has-a-work-bound) | safety | `always` | daemon, store | active | yes | #873, #874, #875, #833 |
| WP-P08 | [`wp-p08-fold-rendering-reads-a-bounded-set-and-renders-identical-bytes`](#wp-p08-fold-rendering-reads-a-bounded-set-and-renders-identical-bytes) | safety | `always` | daemon | active | yes | #873 |
| WP-P09 | [`wp-p09-shrink-first-has-an-explicit-failure-state`](#wp-p09-shrink-first-has-an-explicit-failure-state) | safety | `always` | plugin | active | yes | #877, #883 |
| WP-P10 | [`wp-p10-discovery-distinguishes-exhaustion-from-budget`](#wp-p10-discovery-distinguishes-exhaustion-from-budget) | safety | `always` | plugin, protocol | active | yes | #875, #883 |
| WP-P11 | [`wp-p11-fail-open-retains-its-acknowledgment-basis`](#wp-p11-fail-open-retains-its-acknowledgment-basis) | safety | `always` | plugin | active | yes | #884 |
| WP-P12 | [`wp-p12-boundary-index-is-fixed-through-publication`](#wp-p12-boundary-index-is-fixed-through-publication) | safety | `always` | plugin | active | yes | #883 |
| WP-P13 | [`wp-p13-id-scan-invokes-no-host-hooks`](#wp-p13-id-scan-invokes-no-host-hooks) | safety | `always` | plugin | active | yes | #883 |
| WP-P14 | [`wp-p14-first-user-policy-comes-from-session-authority`](#wp-p14-first-user-policy-comes-from-session-authority) | safety | `always` | plugin | active | partial | #884; frozen-status assertion unowned |
| WP-P15 | [`wp-p15-steady-state-work-is-window-bounded`](#wp-p15-steady-state-work-is-window-bounded) | safety | `always` | plugin, daemon | active | yes | #874, #883, #833 |
| WP-P16 | [`wp-p16-prune-publish-race-is-actually-constructed`](#wp-p16-prune-publish-race-is-actually-constructed) | reachability | `sometimes` | store | active | yes | #833 |
| WP-P17 | [`wp-p17-interior-omission-is-constructed`](#wp-p17-interior-omission-is-constructed) | reachability | `sometimes` | plugin | active | yes | #883 |
| WP-P18 | [`wp-p18-a-waiting-pass-actually-waited`](#wp-p18-a-waiting-pass-actually-waited) | reachability | `sometimes` | daemon | active | yes | #875 |
| WP-P19 | [`wp-p19-a-nonzero-cut-is-constructed`](#wp-p19-a-nonzero-cut-is-constructed) | reachability | `sometimes` | daemon | active | yes | #875, #881 |
| WP-P20 | [`wp-p20-the-shrink-actually-threw`](#wp-p20-the-shrink-actually-threw) | reachability | `sometimes` | plugin | active | yes | #877, #883 |
| WP-P21 | [`wp-p21-discovery-passed-page-one-and-a-budget-fired`](#wp-p21-discovery-passed-page-one-and-a-budget-fired) | reachability | `sometimes` | plugin | active | yes | #883 |
| WP-P22 | [`wp-p22-the-host-mutated-during-the-await`](#wp-p22-the-host-mutated-during-the-await) | reachability | `sometimes` | plugin | active | yes | #883 |
| WP-P23 | [`wp-p23-the-scan-traversed-a-hostile-slot`](#wp-p23-the-scan-traversed-a-hostile-slot) | reachability | `sometimes` | plugin | active | yes | #883 |
| WP-P24 | [`wp-p24-wire-admissibility-is-explicit`](#wp-p24-wire-admissibility-is-explicit) | safety | `always` | protocol | active | yes | #875, #881, #883 |
| WP-P25 | [`wp-p25-legacy-meta-is-pruned-on-first-commit`](#wp-p25-legacy-meta-is-pruned-on-first-commit) | safety | `always` | store | active | yes | #833 |

Owner lists the PR whose recorded run supplies the status, then, after a
semicolon, the PR or ticket that still owes evidence. Semantics: 29 `always`,
8 `sometimes`, 0 `always-or-unreached`, 0 `reachable`, 0 `unreachable`.

Exercise distribution: 35 `yes`, 2 `partial` (WP-E10, WP-P14), 0
`not yet`.
Status: 35 `active`, 2 `invalidated` (WP-E06, WP-E11).

## Records

### wp-e01-boundary-presence-gates-reconcile

Type: safety
Reachability: default-production - every compaction pass resolves the stored
boundary against the live window; revision 3 keeps the presence gate as "the
live window contains `core.boundary_id`" (#881, which lists WP-E01 under
"existing checks green").
Status: active
Exercised: yes -
`empty_store_bootstrap_then_defers_stably_without_hard_oscillation` and
`reconcile_rematerialize_after_revert_is_not_blocked_by_the_mint_guard`
construct an empty bootstrap and a removed rendered anchor and assert
`reconcile_pending` set on the missing anchor and cleared after the refold;
both ran in the `cargo test -p daemon` gate recorded in #880 (head `748c0c4b`)
and again in the #881 `cargo test -p daemon` gate (head `1c66f16c`), whose
Evidence table lists WP-E01 under "existing checks green".
Guarantee: A deferred pass preserves frozen bytes while distinguishing a
missing minted boundary from a boundary that never exists.
Check: `always` - With `run_started=false`, a defer preserves frozen bytes and
sets `reconcile_pending == (!boundary_match && boundary_id != "")`; a SOFT
cannot advance an anchor while reconciliation is pending. These predicates
hold at each completed core step, so `always` is the only semantics that fits.
Fault/timing angle: The host removes the rendered anchor between passes.
Required faults and enabling state: Matching, missing nonempty, and empty
boundaries with nonempty frozen output.
Confidence: high -
[evidence](evidence/wp-e01-boundary-presence-gates-reconcile.md). Both
witnesses were read at `f2442b2f` and assert `reconcile_pending` on the revert
pass and its clearance on the refold; the gate run is the #880 description's
`cargo test -p daemon ok`. The core predicate lives in
`crates/cache-stability`; its line numbers were not re-verified for this part.
Existing check: `crates/daemon/src/transform.rs:19100`
`empty_store_bootstrap_then_defers_stably_without_hard_oscillation`; `:19420`
`reconcile_rematerialize_after_revert_is_not_blocked_by_the_mint_guard`
(marker: `assert!(revert.reconcile_pending)` with action `SOFT+` before the
refold). Canonical records: shared-primitives
`never-minted-boundary-is-not-reconcile-pending`,
`anchor-holds-while-reconcile-pending`,
`defer-pass-replays-frozen-bytes-verbatim`.
Impact: High. Incorrect absence handling either hides a revert or repeatedly
refolds a fresh session.
Open questions: None.

### wp-e02-recut-keeps-the-surviving-prefix

Type: safety
Reachability: default-production - the HARD reconcile arm truncates and
refolds; under revision 3 the truncate keeps history through the resolved
anchor's sequence (`Revert { keep_through_seq: Some }`, #881).
Status: active
Exercised: yes -
`reconcile_rematerialize_with_unrecut_store_truncates_and_refolds_prefix`
removes a later anchor with an earlier one surviving and asserts the HARD
refold, `coverage_ordinal` 1, and a cleared `reconcile_pending`; it ran in the
#880 daemon gate (head `748c0c4b`) and in the #881 `cargo test -p daemon` gate
(head `1c66f16c`). #881 adds
`a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request` (the
interrupted request answers the HARD against the surviving anchor,
`revert_epoch` + 1, one segment) and
`a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass`
(discovery lists the surviving anchor after reopen and the next pass folds),
both of which also re-enter the truncate and assert a no-op; both ran in the
same gate.
Guarantee: A successful reconcile refold retains only the contiguous surviving
history prefix and remints coverage from that prefix.
Check: `always` - After a successful refold, history contains exactly the
store-ordered prefix whose terminal anchor survives, coverage names that
anchor, and removed coverage is not served as current history. This is a
per-transition predicate, not cross-transaction atomicity.
Fault/timing angle: Revert removes the latest anchor while an earlier anchor
survives.
Required faults and enabling state: At least two segments, a missing later
anchor, `reconcile_pending`, and a subsequent successful HARD pass; for
interruption, a CAS conflict or a panic after the truncate commit.
Confidence: high -
[evidence](evidence/wp-e02-recut-keeps-the-surviving-prefix.md). The witness
was read at `f2442b2f` (`transform.rs:19455`). The revision 3 keep-through
rule and the two interrupted-revert tests were read at `1c66f16c` (same lines
at `f2442b2f`); #881's Evidence table names them for the interrupted revert.
Existing check: `crates/daemon/src/transform.rs:19455`
`reconcile_rematerialize_with_unrecut_store_truncates_and_refolds_prefix`
(marker: `assert!(revert.reconcile_pending)` before the HARD);
`crates/memory-store/src/lib.rs:26148`
`truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`; #881
`crates/daemon/src/transform/revision_3.rs:1147`
`a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request` and
`:1163` `a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass`.
Impact: High. A wrong cut deletes surviving summaries or retains reverted
ones.
Open questions: None. Interrupted recut convergence is an acceptance criterion
of #831; #881's daemon gate records it.

### wp-e03-no-survivor-arms-pending-without-truncation

Type: safety
Reachability: default-production - reached only with `lineage_switched`: the
arm requires `req.lineage_switched` and a `NO_SURVIVOR` resolution
(`pending_rewrite_absent_shape`, `crates/daemon/src/transform.rs:3318-3319`;
`meta.pending_rewrite = Some` at `:3413`, `f2442b2f`). Since #833 (D10) the
null-anchor no-survivor shape without `lineage_switched` resets instead
(`reset_no_survivor`, called from `:1959`, defined at `:2038`).
Status: active
Exercised: yes - #833 moves every pre-disposition `pending_rewrite` test onto
a lineage switch (`switched()`), so each reaches the arm only with
`lineage_switched`, and adds the reset contrast
`a_revert_before_the_first_anchor_resets_and_serves_the_window_as_a_first_pass`;
all ran in #833's `cargo test -p daemon` gate (2,749 passed).
Guarantee: With durable lineage switched and no surviving anchor, the pass
arms `pending_rewrite` and passes through without adopting foreign ingress
identities or truncating history.
Check: `always` - On that path, history, core boundary, and `revert_epoch`
remain unchanged; `pending_rewrite` becomes present; ordinary ingress identity
and usage adoption do not occur; an identical repeat makes no further write.
The path is reachable in production, so `always-or-unreached` would hide a
regression that routes ordinary reverts here.
Fault/timing angle: An unrelated host shape appears under the same session
with `lineage_switched`.
Required faults and enabling state: A committed covered session,
`lineage_switched` present, and input with no surviving anchor; for the reset
contrast, a `boundary: null` window with coverage and no survivor.
Confidence: high -
[evidence](evidence/wp-e03-no-survivor-arms-pending-without-truncation.md).
The arm, the reset, and every witness were read at `f2442b2f`. The reset's
log line is not asserted; its range helper is.
Existing check: `crates/daemon/src/transform.rs:19785`
`reconcile_recut_nothing_survives_arms_pending_raw_without_truncate` (marker:
`armed.action == "PASSTHROUGH"`, `armed.committed`, on a `switched()`
request; #833 adds that the arming commit leaves `block_identity_by_mid`
unchanged); `:19950`
`pending_rewrite_passes_isolate_ingress_meta_usage_and_reconcile`; `:20017`
`pending_rewrite_persists_across_store_restart`. Reset contrast (#833):
`crates/daemon/src/transform/revision_3.rs:447`
`a_revert_before_the_first_anchor_resets_and_serves_the_window_as_a_first_pass`
(marker: resolution `Revert { keep_through_seq: None }` and
`removed_sequence_range` "1..=2" asserted before the pass; then HARD,
`boundary` null, `revert_epoch + 1`, no `pending_rewrite`, no segments, and
the next fold resolves `FirstPass` with ordinals `[1, 2, 3]`); `:498`
`a_cas_conflict_on_the_no_survivor_reset_resolves_again_and_resets_once`;
`:610` `a_no_survivor_reset_that_always_loses_its_cas_surfaces_the_conflict`;
`:635` `a_committed_reset_leaves_the_whole_retry_budget_to_the_pass`; `:657`
`a_second_no_survivor_resolution_in_one_pass_fails_with_a_cas_conflict`.
Impact: High. Mistaking uncertain lineage for a new session destroys history.
Open questions: None. D10 decides the reset; the record stays for
`lineage_switched`.

### wp-e04-publish-requires-current-identities-and-epoch

Type: safety
Reachability: explicit-config-only - publication requires a nonempty
summarizer model chain; the default is empty (`model_chain: Vec::new()` at
`crates/daemon/src/config.rs:123`, `f2442b2f`).
Status: active
Exercised: yes -
`selected_range_identity_drift_during_await_rejects_without_cooldown`,
`tail_identity_extension_during_await_still_publishes`, and
`publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict`
construct drift, a legal extension, and an epoch change; #873 adds
`publish_rejects_a_firing_whose_set_was_truncated_and_regrown_to_the_same_maximum`
for the new `MAX(sequence)` set fence. All ran in the #873 gates (`memory-store
ok`, `daemon ok`).
Guarantee: A summarizer publication cannot commit with missing or changed
selected-message identities or a stale `revert_epoch`.
Check: `always` - At publication, selected identities are nonempty and every
selected vector equals the current durable vector; the expected epoch equals
the durable epoch; the firing identity, phase, history generation, and row
version match. Checked inside the publication transaction on every commit.
Fault/timing angle: Edit, removal, recut, or competing publication during the
producer await.
Required faults and enabling state: A pinned firing plus selected drift or an
epoch change; an unrelated tail extension as the legal control; a truncated
and regrown set at the same maximum.
Confidence: high -
[evidence](evidence/wp-e04-publish-requires-current-identities-and-epoch.md).
All four witnesses were located at `f2442b2f`. The set fence changed in #873
from a session-wide `COUNT(*)` to `MAX(sequence)`; the regrown-set test is the
discriminating witness for that change.
Existing check: `crates/daemon/src/history_summarizer.rs:3799`
`selected_range_identity_drift_during_await_rejects_without_cooldown`; `:3850`
`tail_identity_extension_during_await_still_publishes`;
`crates/memory-store/src/lib.rs:26308`
`publish_history_summarizer_chunk_rejects_recut_epoch_mismatch_as_conflict`;
`:22983`
`publish_rejects_a_firing_whose_set_was_truncated_and_regrown_to_the_same_maximum`
(#873). A selected mid removed by D12's prune reads as drift:
`crates/daemon/src/transform.rs:19708`
`a_prune_that_commits_first_fences_the_publication_out` (#833, WP-P06).
Impact: High. Stale summaries can replace changed or removed context.
Open questions: None. The pruning race is WP-P06 and WP-P16, exercised by
#833.

### wp-e05-recipes-bind-positions-and-revisions

Type: safety
Reachability: default-production - the plugin applies every successful recipe;
the daemon transform is the only transform since `4061315c` ("Remove
transform_mode"), and `createRustModeTransform` is built unconditionally at
`packages/opencode-plugin/src/hooks/context/hook.ts:518` (`f2442b2f`).
Status: active
Exercised: yes - the shared edit-recipe fixtures, the retained-output mutation
family, and `native_previous_keeps_bind_the_applied_revision` (stale revision
gets literals; a same-length edit under a matching revision serves as a cold
encode) ran in the #879 daemon gate and the #877 and #878 `bun run check:repo`
gates.
Guarantee: Recipe keeps select positions from the named input or applied
previous output and cannot borrow a different revision's values.
Check: `always` - Reconstruction concatenates the exact named source slices
and inserted values; each source's ranges are forward and nonoverlapping; base
revisions match; sources remain unchanged; a previous-source keep requires its
retained objects to remain live-valid. It must hold for every recipe the
plugin applies.
Fault/timing angle: Wrong revision, malformed final operation, or retained
object mutation before request or during the response wait.
Required faults and enabling state: Distinct input and previous arrays,
nontrivial positions, and a previous output actually published by the plugin.
Confidence: high -
[evidence](evidence/wp-e05-recipes-bind-positions-and-revisions.md). Witnesses
located at `f2442b2f`. Under revision 3 the input source is the submitted
window; #883 lists WP-E05 as preserved and the recipe format is unchanged.
Existing check:
`packages/opencode-plugin/src/hooks/context/edit-recipe.test.ts:69` "agree
with the shared cases on acceptance and exact output"; `:117` "keeps a
previous entry by reference and never edits either source";
`rust-mode-transform.test.ts:1432` "rejects mutated retained output
${mutationTime}"; `:1488` "keeps from the applied previous output and the
submitted input, then acks its note deliveries";
`crates/daemon/src/lib.rs:25487`
`native_previous_keeps_bind_the_applied_revision` (#879). Canonical owner:
TE21 and `recipe-wire-switch.md`.
Impact: High. A positional or revision error silently substitutes context.
Open questions: None.

### wp-e06-host-publication-is-in-place-and-all-or-none

Type: safety
Reachability: default-production - accepted plugin passes publish through
`publishInPlace`
(`packages/opencode-plugin/src/hooks/context/transform-capture.ts:840`,
`f2442b2f`).
Status: invalidated
Exercised: yes - the baseline preflight witnesses ("reports a destination slot
that stopped accepting writes and reads no candidate getter", "replaces every
slot and the length of an accepted destination in place", "rejects containers
whose element or length assignment could throw") ran in the #875 `bun run
check:repo` gate at `328ab11c`; #877 deleted the all-slot preflight and
replaced the failure contract with WP-P09.
Guarantee: Before #877, publication preserved array identity and wrote the
candidate only after synchronous checks made replacement nonthrowing under the
accepted container contract.
Check: `always` - A prepublication refusal leaves target slots and length
unchanged; success leaves the same array object containing exactly the
candidate, followed by state promotion. This is TE20's baseline contract.
Fault/timing angle: Nonwritable or nonconfigurable slots, source mutation, or
candidate-charge refusal.
Required faults and enabling state: A captured target plus each rejecting
container shape and a successful shrinking replacement.
Confidence: high -
[evidence](evidence/wp-e06-host-publication-is-in-place-and-all-or-none.md).
The three baseline witnesses were located at `328ab11c`; only the
container-rejection test survives at `f2442b2f`, rewritten for
`publicationRejection`. D21 accepts the weaker shrink-first failure state.
Existing check: At `328ab11c`: `transform-capture.test.ts:1000`, `:1015`,
`:1056`. At `f2442b2f`: `:1193` "rejects containers whose element or length
assignment could throw" (now `publicationRejection` reasons). Successor:
WP-P09.
Impact: High. Partial publication corrupts the prompt; WP-P09 bounds the
failure state instead.
Open questions: None. D21 accepts the shrink-first failure state.

### wp-e07-capture-charges-outlive-cancellation

Type: safety
Reachability: default-production - `run` acquires capture admission before
execution on every pass (`TransformCaptureAdmission`, `transform-capture.ts`,
`f2442b2f`).
Status: active
Exercised: yes - the model-based lease sequence test, the per-session lease
test, the aggregate byte test, and the retained-output
eviction-versus-live-lease family ran in the #877 `bun run check:repo` gate
(head `3844a179`).
Guarantee: A live capture keeps its session slot and byte charge until its
owner settles and releases exactly once, even after cancellation.
Check: `always` - The ledger equals the sum of unreleased lease charges, never
exceeds configured limits, and cancellation alone changes neither charge nor
slot ownership; release subtracts once. Observed at every lease event and
owner settlement.
Fault/timing angle: Supersession, cancellation, pressure, or stale double
release while work remains live.
Required faults and enabling state: A charged suspended owner, a same-session
second admission, and independent byte-only pressure.
Confidence: high -
[evidence](evidence/wp-e07-capture-charges-outlive-cancellation.md). Witnesses
located at `f2442b2f`. #883 adds the window copy and a `4·N` filter charge to
the lease; the accounting contract is unchanged.
Existing check:
`packages/opencode-plugin/src/hooks/context/transform-capture.test.ts:1213`
"keeps counts and charges equal to a reference model across generated lease
sequences"; `:1344` "holds one lease per session, declines a newer call, and
cancels the holder"; `:1370` "charges bytes against one aggregate budget and
releases them exactly once"; `rust-mode-transform.test.ts:1319` "never
releases an active capture lease when the ${limit} evicts its session's
output" (#877). Canonical owners: TE23, TE24.
Impact: High. Early release undercounts live memory; missed release strands
capacity.
Open questions: None.

### wp-e08-ready-snapshots-reject-stale-generations

Type: safety
Reachability: default-production - successful transform completion calls
`finish_ready` (`TransformSnapshotCache`, `crates/daemon/src/lib.rs:1860`,
`:1930`, `f2442b2f`).
Status: active
Exercised: yes -
`transform_snapshot_cache_is_generation_safe_and_lru_bounded`,
`snapshot_lease_budget_survives_cache_churn_and_releases_exact_charge`, and
#878's
`ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation` ran
in the #878 daemon gate (head `f6b3d7bd`).
Guarantee: Only the current in-flight generation can become ready, and an
existing snapshot lease remains valid and charged across replacement.
Check: `always` - A stale `finish_ready` cannot replace, resurrect, or expose
a snapshot; wrapup accepts only its current ready generation and epoch; leased
contents remain unchanged until lease drop, which releases their charge once.
Fault/timing angle: Older completion follows a newer begin; eviction or
replacement while wrapup holds an `Arc`.
Required faults and enabling state: Two generations of one session and a held
lease during cache churn.
Confidence: high -
[evidence](evidence/wp-e08-ready-snapshots-reject-stale-generations.md).
Witnesses located at `f2442b2f`. This is a generation fence, not a
serialization test; WP-P04 owns execution order.
Existing check: `crates/daemon/src/lib.rs:21135`
`transform_snapshot_cache_is_generation_safe_and_lru_bounded`; `:21175`
`snapshot_lease_budget_survives_cache_churn_and_releases_exact_charge`;
`:21230`
`ready_snapshot_holds_no_native_payload_and_a_lease_pins_its_generation`
(#878).
Impact: High. Wrapup can act on superseded input or retention becomes
uncounted.
Open questions: None.

### wp-e09-tag-protection-is-session-relative

Type: safety
Reachability: default-production - tail hygiene consumes protection on normal
transform paths; nonempty tags and protected exemplars are enabling state
(`protected_tag_numbers`, `crates/daemon/src/tail_hygiene.rs:748`,
`f2442b2f`).
Status: active
Exercised: yes - `window_tag_read_keeps_every_session_relative_tag_decision`
compares every tag decision of the bounded read against a full read for K in
{0, 1, 2, 3, 5, 8, 40, 100}, and
`shared_row_iterator_matches_slice_for_protected_legacy_orphan` covers the
legacy orphan; both ran in the #874 daemon gate (description head `704568ec`).
Guarantee: Protection uses the newest distinct tag numbers session-wide, plus
explicit block and tool-arc protection, rather than only tags visible in the
submitted array.
Check: `always` - Protected numbers equal the greatest K distinct stored tag
numbers; block and arc protection match the existing mapping, including
unambiguous legacy tool-call ids. Protected eligible parts contribute to T but
not U. Decisions are compared, not only totals.
Fault/timing angle: Covered tags disappear from ingress, or a legacy tool row
does not use a flat block id.
Required faults and enabling state: Protected tags outside the window, legacy
orphan rows, ambiguous mappings, and a protected tool arc.
Confidence: high -
[evidence](evidence/wp-e09-tag-protection-is-session-relative.md). Witnesses
located at `f2442b2f`. A later #874 commit (`08afec9d`, merged with #874 and
on `main`) rewrote the newest-rows tag query to exclude window rows; the
description's differential predates it, and the #881 and #833 daemon gates
ran the witness with it. #833's D15 run reads the tags row at 320 rows at
both N = 10k and N = 1M.
Existing check: `crates/daemon/src/transform.rs:22558`
`window_tag_read_keeps_every_session_relative_tag_decision` (#874);
`crates/daemon/src/tail_hygiene.rs:2564`
`shared_row_iterator_matches_slice_for_protected_legacy_orphan`. Canonical:
the latency catalog's hygiene record.
Impact: High. Query narrowing can make protected content reducible.
Open questions: None.

### wp-e10-durable-meta-bound-refuses-the-cliff

Type: safety
Reachability: default-production - every transform commit serializes
`ModuleMeta` through the durable-text guard (`ensure_durable_text_bound`,
`crates/memory-store/src/lib.rs:4306`; `MAX_DURABLE_TEXT_BYTES` 512 KiB at
`:429`, `f2442b2f`).
Status: active
Exercised: partial - the guard is unchanged. The growth half is exercised:
#833 replaces the cliff pin with
`a_hundred_thousand_message_session_commits_a_three_hundred_message_window`
(`SyntheticHistory::mixed(50_000)`, 100,000 covered messages, a 300-message
window: HARD, committed, meta under 128 KiB), which ran in #833's `cargo test -p
daemon` gate (2,749 passed); #833's acceptance run measures `cache_state.meta`
at 117,066, 117,040, and 117,064 B at N = 10k, 100k, 1M (at `cf89a9c2`;
the later commits change only the plugin, the admission scans, and tests).
The two #873 tests it replaces ran in the #873 and #881 gates. No witness constructs a refused commit.
Guarantee: Oversized meta is refused rather than partially committed, and meta
size does not grow with covered history.
Check: `always` - Inputs above 512 KiB fail the durable-text bound; a refused
transform leaves no cache row and no changed meta; a committing pass at fixed
window writes meta below 128 KiB independent of N. Every commit passes through
the guard.
Fault/timing angle: Normal message growth crosses a durable-field limit.
Required faults and enabling state: A long session over a fresh store; an
exact-boundary byte case; for #833, the synthetic generator's 100k-message
session with a 300-message window.
Confidence: medium -
[evidence](evidence/wp-e10-durable-meta-bound-refuses-the-cliff.md). The guard
and the #833 test were read at `f2442b2f`. Correction to catalog revision 2: the
1,000-success and 1,400-refusal characterization it cites (at `265df096`) was
replaced on `main` by `e15a09a6` ("Keep session meta bounded as covered
history grows") before M1; the refusal half has no witness now.
Existing check: `crates/daemon/src/transform_meta_bound.rs:93`
`a_hundred_thousand_message_session_commits_a_three_hundred_message_window`
(#833; it asserts the identity rows equal the window, which flips the old
helper's "every message keeps a durable identity"). Deleted by #833 (`7a8fb84b`):
`first_hard_pass_meta_respects_the_store_durable_text_bound` and
`meta_bytes_stay_flat_as_covered_history_grows` (at `f8734c12`:
`transform_meta_bound.rs:130`, `:141`).
Impact: High. Without the bound a long session repeats a failing commit on
every pass.
Open questions:

- Does a current witness cover refusal without effect? No: the case lost its
  witness with `e15a09a6` and no M1 PR restores it; a refused transform
  commit test is needed and is queued in `portfolio-evaluation.md`. #833's
  window regression replaces the growth characterization.

### wp-e11-head-fail-open-does-not-publish-a-failed-candidate

Type: safety
Reachability: default-production - real errors enter the plugin's failure arm
(`markFailure`,
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:923`,
`f2442b2f`).
Status: invalidated
Exercised: yes - "does not resend after an outcome-unknown transport failure
and recovers on the next attempt" asserts one uncertain send, unchanged input
identity, and a later success; it ran in the #878 `bun run check:repo` gate
(head `f6b3d7bd`) and still runs in the #883 (head `d7712d75`, `:3298`) and
#884 (head `f8734c12`) `bun run check:repo` gates. #884 replaces the raw-only
fallback with WP-P11.
Guarantee: Before #832's fail-open change, a failed transform published no
replacement candidate and left the host's current input in place.
Check: `always` - On a real prepublication error, candidate publication and
success-state promotion do not occur; transport uncertainty does not trigger
an unbounded resend.
Fault/timing angle: Timeout or reset after a possible daemon commit.
Required faults and enabling state: A dispatched transform whose response
fails, with and without a previously applied output.
Confidence: high -
[evidence](evidence/wp-e11-head-fail-open-does-not-publish-a-failed-candidate.md).
The witness was located at `f2442b2f` (`rust-mode-transform.test.ts:3302`). The
no-failed-candidate clause is carried into WP-P11; the raw-only clause is
invalidated by #884 (#832 PR two).
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.test.ts:3302`
"does not resend after an outcome-unknown transport failure and recovers on
the next attempt" (at `f6b3d7bd`: `:3708`). Successor: WP-P11.
Impact: High. Raw fallback can exceed the provider context; WP-P11 serves the
retained fold instead.
Open questions: None.

### wp-e12-revert-epoch-invalidates-retained-output

Type: safety
Reachability: default-production - retained native output is taken only at the
current `revert_epoch` (`RetainedNativeOutput::into_previous`,
`crates/daemon/src/lib.rs:2317`, `f2442b2f`) and the retained CK previous
output only at the epoch it was served under
(`SerializedOutputCache::take_previous_output`,
`crates/daemon/src/transform.rs:435`).
Status: active
Exercised: yes - `native_output_store_enforces_entry_cap_lru_and_revert_epoch`
and `handler_native_cache_adopts_the_bumped_durable_revert_epoch` (#879), with
`truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch` (real
cut bumps, repeat is a no-op on epoch and row version), ran in the #879
daemon gate (head `7a3db43b`). #881's interrupted-revert tests add one bump
end to end across a CAS conflict and a panic plus reopen (#881 gate). #833
deletes the per-message serialized-output memo and
`serialized_output_cache_revert_epoch_bump_evicts_session` with it, and adds
`serialized_output_cache_take_under_a_new_epoch_returns_nothing` and
`a_same_pass_reset_reuses_no_previous_output` (CK and native, across the
D10 reset); they ran in #833's `cargo test -p daemon` gate.
Guarantee: A real recut invalidates earlier-epoch output reuse, while
repeating an already-completed cut does not advance the epoch again.
Check: `always` - A retained output with a different epoch is unavailable; a
cut that removes rows advances the epoch; a no-op repetition preserves both
epoch and row version. Checked at cut completion and at every lookup.
Fault/timing angle: Recut races publication, or a terminal CAS conflict
retries after a cut already commits.
Required faults and enabling state: Retained output, a removable history
suffix, and a repeated cut using the refreshed row version.
Confidence: high -
[evidence](evidence/wp-e12-revert-epoch-invalidates-retained-output.md). All
witnesses were located at `f2442b2f`. After #833 two retained-output owners
remain, both epoch-bound: `SerializedOutputCache` (`transform.rs:397`), now
holding only the D13 previous CK output with LRU eviction, and
`NativeOutputStore` (`lib.rs:2368`). The per-message memo and its lookup are
gone; every pass renders its window afresh. The D10 reset leaves handler
caches in place and relies on the epoch bump.
Existing check: `crates/daemon/src/lib.rs:25333`
`native_output_store_enforces_entry_cap_lru_and_revert_epoch`; `:25284`
`handler_native_cache_adopts_the_bumped_durable_revert_epoch`;
`crates/memory-store/src/lib.rs:26148`
`truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch`;
`crates/daemon/src/transform.rs:27963`
`serialized_output_cache_take_under_a_new_epoch_returns_nothing` (#833);
`crates/daemon/src/transform/revision_3.rs:701`
`a_same_pass_reset_reuses_no_previous_output` (#833; marker: the repeat
before the reset has nonempty previous keeps); #881
`crates/daemon/src/transform/revision_3.rs:1147` and `:1163` (interrupted
revert, one bump).
Impact: High. Stale output can resurrect reverted context; extra bumps cause
repeated invalidation.
Open questions: None.

### wp-p01-window-is-the-complete-declared-suffix

Type: safety
Reachability: default-production - every revision 3 request carries the
window; the plugin captures `host[boundaryIndex..length)` through `copyWindow`
(`packages/opencode-plugin/src/hooks/context/transform-capture.ts:148`,
`f2442b2f`; #883).
Status: active
Exercised: yes - #875's `impossible_declarations_are_invalid_params` refuses a
window head that is not the declared mid at the module seam; #881's
`boundary_presence_head_sequence_and_duplicates_are_invalid_params` refuses a
head that is not the mid, a duplicate mid, and sequences outside +/-(2^53 - 1)
at the handler with no state change, and
`paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request`
checks them on an assembled paged body (both in the #881 `cargo test -p daemon`
gate (head `1c66f16c`)); #883's "sends the declared window in both
representations and publishes the recipe at boundaryIndex + i", "rejects a
duplicate id inside the window and ignores one outside it", the
interior-omission case (WP-P17), and "declines when the host moves the
discovered anchor before the window is copied" construct the plugin side and ran
in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo`
gates.
Guarantee: A non-null boundary names the first message of the complete host
suffix submitted in both representations, while null submits the whole host
array.
Check: `always` - At capture and send, native membership is exactly
`host[boundaryIndex..capturedLength)`, CK represents that same ordered suffix,
and the first mid equals the declared mid; ids inside the window are unique;
sequence is a JavaScript safe integer. Input admissibility must hold on every
request. Falsifier: capture the first and last tail messages but omit a middle
one.
Fault/timing angle: Interior omission, duplicate id, or oversized input is
mistaken for a smaller valid window.
Required faults and enabling state: Covered prefix, multiple tail messages,
duplicate ids, and a suffix above paging and admission caps.
Confidence: medium -
[evidence](evidence/wp-p01-window-is-the-complete-declared-suffix.md). All
witnesses were read at `f8734c12` and moved by a line diff to `f2442b2f`,
where every cited name was found again. Coverage above
the admission cap is indirect: #884's "serves the last applied output plus the
appended messages on a capture_bytes decline"
(`rust-mode-transform.test.ts:3653`) declines an oversized capture rather than
sending a smaller window. The trusted-producer assumption is contract C7.
Existing check: `crates/daemon/src/window_coverage/tests.rs:217`
`impossible_declarations_are_invalid_params` (#875);
`crates/daemon/src/transform/revision_3.rs:193`
`boundary_presence_head_sequence_and_duplicates_are_invalid_params` and `:287`
`paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request`
(#881);
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:182` "sends
the declared window in both representations and publishes the recipe at
boundaryIndex + i", `:228` "rejects a duplicate id inside the window and ignores
one outside it", and `:845` "declines when the host moves the discovered anchor
before the window is copied" (#883).
Impact: High. An omitted tail message becomes silent context loss.
Open questions: None. C7 states the trusted-producer assumption; the daemon
does not prove host completeness.

### wp-p02-anchor-resolution-uses-authoritative-snapshot

Type: safety
Reachability: default-production - every revision 3 pass resolves its declared
anchor (`window_coverage::resolve`,
`crates/daemon/src/window_coverage.rs:113`, called from `resolve_window`,
`crates/daemon/src/transform.rs:2072`, `f2442b2f`).
Status: active
Exercised: yes - `each_resolution_outcome_has_its_cut_ordinals_and_keeps`
covers Normal, StaleSlice, Revert with and without `keep_through_seq`,
Unknown, and FirstPass;
`a_publish_between_the_core_read_and_the_declared_read_is_invisible` and
`a_removal_between_the_core_read_and_the_intersection_is_invisible` hold a
barrier in the snapshot; all ran in the #875 daemon gate. At the handler,
#881's `each_resolution_outcome_runs_through_the_handler_with_its_effects`
runs every outcome with its effects (`boundary_unknown` leaves `row_version`,
meta, and segments unchanged), and
`a_publish_during_a_null_boundary_pass_keeps_the_cut_at_the_rendered_boundary`
and
`a_reset_that_moves_the_cut_leaves_the_served_window_in_the_ready_snapshot`
change the snapshot between resolution and a CAS attempt; they ran in the #881
`cargo test -p daemon` gate (head `1c66f16c`).
Guarantee: Anchor resolution uses one authoritative snapshot and distinguishes
normal, stale-slice, revert, unknown, and first-pass inputs without treating
incomplete evidence as a revert.
Check: `always` - Under one snapshot: declared equals rendered gives `Normal`;
older with rendered mid present gives `StaleSlice`; older with rendered mid
absent gives `Revert(Some(declared.sequence))`; missing non-null row gives
`Unknown`; null with coverage and survivors gives `StaleSlice` at the newest;
null with coverage and no survivors gives `Revert(None)`, never `Unknown`;
null without coverage gives `FirstPass`. Falsifier: return `Revert` after
reading core before a publish and the declared row after it.
Fault/timing angle: Publication, recut, or reset changes the snapshot before
commit.
Required faults and enabling state: One witness per outcome, stale CAS,
multi-block anchors, reset, and before-first-anchor revert.
Confidence: high -
[evidence](evidence/wp-p02-anchor-resolution-uses-authoritative-snapshot.md).
The table test and both barrier tests were read at `f2442b2f`; the barrier is
`set_coverage_snapshot_hook`. #881 re-resolves on every CAS attempt, and outer
consumers read the request the pass served (`served_request`, review fix F4).
Existing check: `crates/daemon/src/window_coverage/tests.rs:99`
`each_resolution_outcome_has_its_cut_ordinals_and_keeps`; `:237`
`a_declared_row_without_a_rendered_boundary_is_unknown`; `:287`
`a_publish_between_the_core_read_and_the_declared_read_is_invisible`; `:317`
`a_removal_between_the_core_read_and_the_intersection_is_invisible`; `:217`
`impossible_declarations_are_invalid_params`; #881
`crates/daemon/src/transform/revision_3.rs:323`
`each_resolution_outcome_runs_through_the_handler_with_its_effects`; `:876`
`a_publish_during_a_null_boundary_pass_keeps_the_cut_at_the_rendered_boundary`;
`:917`
`a_reset_that_moves_the_cut_leaves_the_served_window_in_the_ready_snapshot`;
`crates/daemon/src/window_coverage/tests.rs:254`
`a_pending_reconcile_renders_the_newest_row_the_truncate_left`; `:580`
`a_null_anchor_hit_above_the_rendered_row_is_not_cut_at`.
Impact: High. A stale or unknown anchor can delete history or loop discovery.
Open questions: None. D7 and D10 decide a declared row newer than rendered:
`invalid_params`.

### wp-p03-ordinals-derive-from-the-effective-anchor

Type: safety
Reachability: default-production - the daemon stamps ordinals on every
resolved window (`resolve`, `crates/daemon/src/window_coverage.rs:113`; #881
overwrites ingress ordinals).
Status: active
Exercised: yes - `resolved_ordinals_equal_the_independent_model` (multi-block
anchor, interior covered removal, lineage continuation base 40, synthetic
head, middle, and tail) and
`synthetic_borrowing_matches_the_plugin_rule_case_for_case` (every pattern up
to eight messages) ran in the #875 daemon gate.
Guarantee: Ordinals derive from the resolved anchor rather than host prefix
length, so covered-prefix removal does not renumber the window.
Check: `always` - The anchored head receives the anchor's stored end ordinal;
later nonsynthetic messages increment once; synthetic messages use the D11
borrowing rule; without an anchor numbering begins at one or the lineage
continuation base plus one. Checked before projection. Falsifier: subtract one
from all tail ordinals after deleting a covered message before the anchor.
Fault/timing angle: Covered interior removal, stale slicing, or synthetic
messages alter host positions.
Required faults and enabling state: Multi-block anchor; synthetic head,
middle, and tail; lineage continuation; interior covered removal.
Confidence: high -
[evidence](evidence/wp-p03-ordinals-derive-from-the-effective-anchor.md). Both
witnesses were read at `f2442b2f`; the model is `plugin_ordinal_model` in the
test module. Review P10 on #832 noted stale comments citing the deleted
`annotateOrdinals`; #883 rewords them (comment-only daemon change, visible in
`window_coverage.rs` and `window_coverage/tests.rs` between `1c66f16c` and
`f2442b2f`).
Existing check: `crates/daemon/src/window_coverage/tests.rs:436`
`resolved_ordinals_equal_the_independent_model`; `:412`
`synthetic_borrowing_matches_the_plugin_rule_case_for_case`; #881 asserts
served `messages` and `operations` contain no `"ordinal"`
(`crates/daemon/src/transform/revision_3.rs:323`).
Impact: High. Wrong ordinals misclassify coverage and tool policy.
Open questions: None. D11 fixes the synthetic rule.

### wp-p04-session-serialization-preserves-capture-order

Type: safety
Reachability: default-production - every transform pass enters the per-session
lane after global admission (`SessionLanes`,
`crates/daemon/src/transform_unit.rs:211`; `PassHold`, `:144`, `f2442b2f`).
Status: active
Exercised: yes -
`session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order`,
`session_lane_activates_in_join_order_under_contention` (400 tasks, 4
threads), the three hand-off tests, and
`cancelled_pass_keeps_its_lane_place_until_its_blocked_unit_finishes` ran in
the #875 daemon gate.
Guarantee: Same-session transforms execute in capture order, with one active
pass and at most one waiting pass, so a later window's absence evidence cannot
be applied to an older capture.
Check: `always` - Active count at most one, waiting count at most one,
processing order equals admitted capture order, and a third pass receives
`session_busy` without coverage mutation. Checked at admission and execution
boundaries. Falsifier: serially execute newer B, then older A, and interpret
B's missing anchor in A as host removal.
Fault/timing angle: A slow older capture arrives after a newer capture
produces a summarizer-visible window.
Required faults and enabling state: Two overlapping captures, a third request,
a barrier-held active pass, and cancellation during a blocking unit.
Confidence: high -
[evidence](evidence/wp-p04-session-serialization-preserves-capture-order.md).
Witnesses were read at `f2442b2f`. The contention test failed one of three
runs against an earlier semaphore design (#875 description). Transport
reordering is outside the contract (C6).
Existing check: `crates/daemon/src/transform_unit/tests.rs:1016`
`session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order`;
`:1141` `session_lane_activates_in_join_order_under_contention`; `:1202`
`session_lane_waiter_dropped_after_hand_off_releases_the_lane`; `:1214`
`session_lane_handed_off_waiter_dropped_unpolled_wakes_the_next`; `:1226`
`session_lane_waiter_leaving_before_hand_off_frees_the_slot`; `:1083`
`cancelled_pass_keeps_its_lane_place_until_its_blocked_unit_finishes`; `:1241`
`waiting_unpaged_pass_is_refused_when_a_page_stream_started_meanwhile`; plugin
`rust-mode-transform.test.ts:441` "declines a daemon ${status} pass without
publishing or promoting".
Impact: High. False revert detection can truncate valid history.
Open questions: None. C6 and D3 fix the carrier.

### wp-p05-recipe-keeps-use-submitted-window-coordinates

Type: safety
Reachability: default-production - every recipe after a stale-slice
resolution; the native recipe is built against the full submitted
`native_messages` (#881).
Status: active
Exercised: yes -
`a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input`
builds keeps at a stale cut of four and reconstructs the served array from the
unsliced input; it ran in the #875 daemon gate. At the handler, #881's
`stale_slice_input_keeps_address_the_submitted_native_window` resolves
`StaleSlice { cut: 2 }` over a native window with no previous output and
asserts every input keep starts at or after the cut and the served native
messages are `m5`, `m6`; it ran in the #881 `cargo test -p daemon` gate (head
`1c66f16c`).
Guarantee: Internal slicing never changes the coordinate system of input keeps
returned to the plugin.
Check: `always` - Reconstruct against the original submitted window and the
named previous output; the result equals the daemon's served array. An
internal input position i after cut c becomes submitted position c + i;
previous-output positions are unchanged. Falsifier: return `keep(input, 0, 1)`
for resolved position zero when the submitted cut is two.
Fault/timing angle: A fold advances coverage after the plugin's prior
response.
Required faults and enabling state: Nonzero stale cut, mixed insert, input,
and previous operations, and a multi-block boundary message.
Confidence: high -
[evidence](evidence/wp-p05-recipe-keeps-use-submitted-window-coordinates.md).
The #875 witness was read at `f2442b2f`. #881 deleted #827's
`translate_input_keeps` (0 hits in `crates` at `1c66f16c`) because the recipe
is built against the submitted window.
Existing check: `crates/daemon/src/window_coverage/tests.rs:513`
`a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input`
(#875, cut 4); `crates/daemon/src/transform/revision_3.rs:988`
`stale_slice_input_keeps_address_the_submitted_native_window` (#881); plugin
publication at `boundaryIndex + i`
(`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:182`,
#883).
Impact: High. The plugin keeps the wrong message without a type error.
Open questions:

- Does the CK recipe path use the same coordinates? Unresolved: a CK keep
  witness at a nonzero cut is needed; #881's handler test builds a native
  body. Queued in `portfolio-evaluation.md`.

### wp-p06-pruning-and-publication-share-the-meta-fence

Type: safety
Reachability: explicit-config-only - pruning runs on ordinary passes, but the
competing publication requires a configured summarizer (`model_chain` defaults
empty, `crates/daemon/src/config.rs:123`).
Status: active
Exercised: yes - #833's `a_prune_that_commits_first_fences_the_publication_out`
and
`a_publication_that_commits_first_makes_the_transform_reload_and_match_the_serial_run`
run both commit orders through real store transactions, with the enabling state
asserted apart from either verdict; both ran in #833's `cargo test -p daemon`
gate (2,749 passed).
Guarantee: Identity pruning commits with transform meta, while a summarizer
retains its independent current-state publication fence.
Check: `always` - After an accepted pruning commit, retained identity keys are
confined to the submitted window (cut prefix plus resolved messages);
publication succeeds only if every selected
identity remains equal and the epoch matches; a stale writer reloads and
re-resolves rather than restoring its old identity map. Falsifier: publish
loses CAS, reloads only the row version, then commits using its pre-prune
identity map.
Fault/timing angle: Prune and publication both read the same row version.
Required faults and enabling state: A selected mid leaves the resolved window;
prune-first and publish-first schedules through real store transactions with
one-shot barriers.
Confidence: high -
[evidence](evidence/wp-p06-pruning-and-publication-share-the-meta-fence.md).
The prune and both tests were read at `f2442b2f`. `prune_block_identities`
(`crates/daemon/src/transform.rs:5260`) keeps the cut prefix plus the
resolved messages and skips a revert's SOFT; it runs before the meta CAS on
the additive path (`:2742`) and on the main path after the pressure refold
(`:4981`). Pass-through paths return before it. The barrier is the existing
transform attempt hook (`install_transform_attempt_hook`, `:2184`), which
fires just before `commit_transform` (`:5002`).
Existing check: `crates/daemon/src/transform.rs:19708`
`a_prune_that_commits_first_fences_the_publication_out` (prune first: the
publisher gets `CasConflict`, then `FenceRejected` after reloading only the row
version, and the segment count stays 2; the falsifier's shape); `:19741`
`a_publication_that_commits_first_makes_the_transform_reload_and_match_the_serial_run`
(publish first: the transform reloads, its response hash, segments, core, and
identity map equal the serial run's, and the publication's third segment stays).
Pass-through: `:19785`
`reconcile_recut_nothing_survives_arms_pending_raw_without_truncate` asserts the
arming commit leaves the map unchanged. WP-E04's witnesses cover drift and epoch
mismatch.
Impact: High. Stale meta can resurrect pruned identities or authorize stale
summaries.
Open questions: None. D12: pruning runs only on resolutions that commit
ordinary meta.

### wp-p07-every-pass-store-read-has-a-work-bound

Type: safety
Reachability: default-production - every SOFT, HARD, retry, and post-transform
store read on the pass path; the statement-work ledger is test-support only
(`sqlite3_trace_v2` at `crates/storage/src/lib.rs:299`, `f2442b2f`).
Status: active
Exercised: yes - `every_pass_read_is_bounded_independent_of_history_size`
classifies every statement of real SOFT, HARD, CAS-retry, summarizer, and
absent-boundary passes into a D15 row and asserts equal rows and VM_STEP at H
= 100 and H = 50,000 (#874 run; #881 and #833 daemon gates under revision 3
requests); #875 adds the intersection and page rows. Since `3ebfc3b9` the
ledger also records bytes decoded per statement and the test asserts each
row's declared bytes bound at H = 50,000. The N axis at fixed W is #833's
acceptance D15 run (N = 10k / H = 100 against N = 1M / H = 50k, W = 300):
every row's rows equal except m0 (2,484 rows, D14 cap 2,733), no
whole-table statement, and every row's bytes within its declared bound
(`3ebfc3b9`). The verdict readings await owner confirmation.
Guarantee: Every per-pass store read has an explicit W-, budget-, or constant
bound on materialized data and inspected work.
Check: `always` - Every store call on the pass path records rows materialized,
`VM_STEP`, and bytes decoded through the connection wrapper; per inventory row
the rows and steps are independent of N and H at fixed W, including CAS
retries, and the bytes stay within the row's declared bound: m0 and m1 at
their row caps times the largest segment row (eleven text columns at
`MAX_DURABLE_TEXT_BYTES`, 512 KiB, plus six integers), every other row at its
small-point bytes times the ordinal-width ratio, and the coverage snapshot
also carries the frozen fold render, so its bound adds the m0 bytes. A SQL
`LIMIT` alone does not prove bounded rows visited. Falsifier: return six references after
loading all H segment rows.
Fault/timing angle: Large history, overlay population, cache miss, retry, or
reset exposes a hidden full scan.
Required faults and enabling state: Fixed W with independently increasing N,
H, overlay count, and active user-memory count.
Confidence: medium -
[evidence](evidence/wp-p07-every-pass-store-read-has-a-work-bound.md). The
inventory test was located at `f2442b2f`. The #874 run varies H only; #833's
acceptance run adds N and states artifact identity (commit `cf89a9c2`, Rust
1.98.1, release test build). Late #874 commits (`08afec9d`, `cd596c15`) add a
one-time range-order scan and reopen the store before measurement; they are
on `main`, so #881's and #833's daemon gates ran the inventory with them; the
#874 measurement table predates them. The identity load (`block_identities`)
is classified into the coverage-snapshot row
(`crates/daemon/src/transform_read_bound.rs:108` at `3ebfc3b9`) and, after
D12, holds window mids only. Bytes decoded come from `3ebfc3b9`: the
ledger's `TRACE_ROW` reads each produced value's type and, for TEXT and BLOB
only, its length.
Existing check: `crates/daemon/src/transform_read_bound.rs:627` (at `3ebfc3b9`)
`every_pass_read_is_bounded_independent_of_history_size` (#874; bytes since
`3ebfc3b9`); `crates/storage/src/lib.rs:6401`
`statement_work_counts_the_bytes_of_every_returned_value` (`3ebfc3b9`);
`crates/memory-store/src/lib.rs:22466`
`per_pass_history_reads_do_constant_work_as_history_grows` (#873);
`crates/daemon/src/window_coverage/tests.rs:591`
`intersection_and_page_work_is_independent_of_history_length` (#875);
`crates/daemon/src/m0_compose.rs:710`
`bounded_fold_work_is_independent_of_history_length` (#873).
Impact: High. One residual scan defeats the window design.
Open questions: None. Revert truncation is the declared exception (C3).

### wp-p08-fold-rendering-reads-a-bounded-set-and-renders-identical-bytes

Type: safety
Reachability: default-production - every SOFT and HARD composition over stored
history (`compose_m0`, `crates/daemon/src/m0_compose.rs:176`; `fold_horizon`,
`crates/daemon/src/decay_render.rs:265`; `m1_row_cap`,
`crates/daemon/src/m1_compose.rs:20`, `f2442b2f`).
Status: active
Exercised: yes -
`bounded_fold_matches_the_full_read_over_the_store_shape_fixture` (388 rows,
four budgets),
`bounded_fold_matches_the_full_read_over_sixty_thousand_segments` (13 budgets
from 20 to 10,000,000), `bounded_fold_work_is_independent_of_history_length`,
and `bounded_m1_matches_the_full_read_and_withholds_an_overflowing_body` ran
in the #873 gates; both named falsifiers differ from the reference within the
sweep.
Guarantee: m0 and m1 rendering read a candidate set bounded by the decay
curve, not by H, and produce bytes identical to rendering over every row.
Check: `always` - For any budget and row set: the daemon reads the newest 249
non-legacy importances, computes pressure exactly as
`compute_budget_pressure`, derives K(p) <= 2,484, reads max(249, K) rows plus
legacy rows by the persisted list, and renders byte-identical to the full
read; rows visited are at most 249 + K + L. m1 reads rows above the folded
sequence, capped at ceil(usable_hard / 322). Falsifiers: pressure from only
the newest K rows; skipping legacy rows.
Fault/timing angle: Importance extremes among the newest 249 rows, legacy rows
older than K, tight budgets, and the retry multiplier.
Required faults and enabling state: H > 2,484 with mixed importances; legacy
rows present; budgets 20 through 10,000,000.
Confidence: high -
[evidence](evidence/wp-p08-fold-rendering-reads-a-bounded-set-and-renders-identical-bytes.md).
All four witnesses were located at `f2442b2f`. Late commit `efb3fe9c` on #873
extends the legacy capture scan with a range-order check (480,010 VM steps at H
= 60,000); it merged with #873, and no description records those numbers. Its
witnesses (`fold_capture_refuses_stored_ranges_out_of_order`,
`crates/memory-store/src/lib.rs:22796`;
`a_fold_over_stored_ranges_out_of_order_fails_closed`,
`crates/daemon/src/transform.rs:20060`) ran in the #881 and #833 gates.
Existing check: `crates/daemon/src/m0_compose.rs:565`, `:636`, `:710`, `:742`
(#873); `crates/daemon/src/transform.rs:14955`
`additive_soft_retries_when_rows_past_the_m1_cap_land_mid_pass`;
`crates/memory-store/src/lib.rs:22858`
`writers_that_add_a_legacy_row_clear_the_persisted_legacy_list`; goldens
`render-golden.json`, `decay-store-shape.json`,
`decay-store-differential.json`, `render-tight-golden.json`. Hot-path H1
preserved.
Impact: High. A wrong horizon silently changes remembered content.
Open questions: None. D14: exact bounded read, no semantic change.

### wp-p09-shrink-first-has-an-explicit-failure-state

Type: safety
Reachability: default-production - every accepted pass publishes through
`publishInPlace`
(`packages/opencode-plugin/src/hooks/context/transform-capture.ts:840`,
`f2442b2f`).
Status: active
Exercised: yes - "shrinks first and leaves exactly the candidate in the
original array object" (define order `length`, `0`, `1`), "restores the
captured references after a shrink stopped by a planted non-configurable
slot", "reports a throw while restoring the captured references", "leaves
exactly a shorter candidate in the original array object", and "restores the
captured references and promotes nothing when a planted slot stops the shrink"
ran in the #877 `bun run check:repo` gate. #883 adds the window variant
"appends the captured window after a shrink stopped at a covered|window slot"
("a covered" and "a window" slot, `boundaryIndex` = `covered.length`); it ran
in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo`
gates.
Guarantee: Publication shrinks before writing candidate slots, and a failed
shrink writes no candidate value or promoted session state.
Check: `always` - On success the original array contains exactly the
candidate. On shrink failure at nondeletable index k, the final array is the
original prefix `[0..k]` followed by the captured window references;
candidate-write count and promotion count are zero. Checked after the
synchronous publication section. Falsifier: write candidate slot zero before
attempting the shrinking length change.
Fault/timing angle: A covered nonconfigurable slot blocks length reduction
after higher slots are deleted.
Required faults and enabling state: Output length below k, k below or inside
the captured window, and deletable captured-window slots.
Confidence: high -
[evidence](evidence/wp-p09-shrink-first-has-an-explicit-failure-state.md).
Witnesses were read at `f2442b2f`; the restoration math is in `publishInPlace`
(`:826-858`). Review P11 on #832 asked whether a test covers k inside the
window; the `it.each` at `transform-capture.test.ts:1138` covers "a covered"
and "a window" slot. #883's description does not mention P11 by name.
Existing check:
`packages/opencode-plugin/src/hooks/context/transform-capture.test.ts:1067`,
`:1098`, `:1153`, `:1138` (window variant, #883);
`rust-mode-transform.test.ts:3236` "leaves exactly a shorter candidate in the
original array object"; `:3252` "restores the captured references and promotes
nothing when a planted slot stops the shrink".
Impact: High. Covered prompt entries can be lost on this failure path; the
state is bounded and logged.
Open questions: None. D21 accepts partial restoration.

### wp-p10-discovery-distinguishes-exhaustion-from-budget

Type: safety
Reachability: default-production - cold start, unknown anchor, and absent
anchor run `transform.boundary` discovery (daemon `boundary_page`,
`crates/daemon/src/window_coverage.rs:284`; plugin `discover`,
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1176`,
`f2442b2f`).
Status: active
Exercised: yes - the daemon side (pages newest first, at most 4,096, never
above the rendered row, empty page terminal, a long unlisted run does not end
the walk, malformed and unsafe bodies `invalid_params`) ran in the #875 daemon
gate. The plugin side ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates: "declares a match found past page
one", "sends null with the whole array only after an empty page", "declines
${name} without sending null" (repeated cursor, ascending, malformed,
unsafe-sequence, timeout, and revision 2 daemon pages), "declines when the
time budget fires with no null submission", and "rediscovers once after
boundary_unknown and declines the second in one pass".
Guarantee: Only a complete, valid anchor walk can authorize null; timeout,
malformed pages, or budget exhaustion cannot.
Check: `always` - Pages are newest first and strictly below the prior cursor;
every accepted continuation decreases the cursor; null is sent only after an
empty terminal page and no usable anchor match; other failures decline
locally; a second unknown outcome in one pass declines. Falsifier: catch a
discovery timeout and assign `boundary=null`.
Fault/timing angle: The matching anchor lies beyond page one or just beyond
the page or time budget.
Required faults and enabling state: Multiple pages, empty completion, timeout,
malformed ordering, repeated cursor, and a hidden later match.
Confidence: medium -
[evidence](evidence/wp-p10-discovery-distinguishes-exhaustion-from-budget.md).
Daemon and plugin witnesses were read at `f8734c12` and re-located at
`f2442b2f`. #883 lists review fixes P1, P2, and P6 as accepted: one
discovery per pass, refund before rerun, and the budget measured from the pass
start. #883's review update (`adcc7baf`, merged) replaces "one discovery per
pass" with one rediscovery after the first `boundary_unknown`. Late commit
`2d58cc20` on #875 bounds pages below -(2^53 - 1); its witness
`an_unsafe_negative_sequence_is_not_listed`
(`crates/daemon/src/window_coverage/tests.rs:655`) ran in the #881 and #833
daemon gates, not in #875's description run.
Existing check: `crates/daemon/src/window_coverage/dispatch_tests.rs:45`
`boundary_walk_is_exhaustive_newest_first_and_bounded_by_the_rendered_row`
(4096/4096/808/0); `:72` `boundary_page_is_empty_without_a_rendered_boundary`;
`:81` `malformed_boundary_bodies_are_invalid_params`;
`window_coverage/tests.rs:622`
`a_long_run_of_unlisted_rows_does_not_end_the_walk`; `:687`
`the_sql_anchor_grammar_matches_split_block_id`; plugin
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:341`,
`:486`, `:502`, `:562`, `:589`, `:607`, `:754` (#883).
Impact: High. False exhaustion enters the no-survivor path, which resets the
session after #833.
Open questions: None. D16 and D19 fix page size, deadline reserve, and cursor
rule.

### wp-p11-fail-open-retains-its-acknowledgment-basis

Type: safety
Reachability: default-production - real errors and `capture_bytes` declines
after an applied pass (`failOpenSource`,
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1085`;
`isAppendOnlyExtension`, `:366`, `f2442b2f`; #884).
Status: active
Exercised: yes - "appends exactly the unacknowledged window suffix and
promotes no basis" folds, grows the host, fails twice, and asserts the exact
suffix by identity with the boundary unchanged; "serves raw against a
mismatched basis anchor or a changed terminal message" constructs both
refusals; "serves raw when a rerun fails after rediscovering the basis the
daemon disowned" covers the rerun guard. All ran in the #884 `bun run
check:repo` gate (head `f8734c12`).
Guarantee: Retained output is reused only against the same declared basis
anchor and an unchanged acknowledged window prefix.
Check: `always` - On a real failure, reuse requires equal basis anchors,
matching tapes for the entire acknowledged prefix including its terminal
message, live-valid applied values, current owner and target, and sufficient
charge; output is exactly `applied ++ window[tapes.length..]`; no new
acknowledgment basis is promoted. Otherwise serve raw. Falsifier: reuse tapes
from anchor A after the pass declares anchor B.
Fault/timing angle: A fold advances coverage, then the next pass fails; an old
terminal message changes in place.
Required faults and enabling state: Retained fold, two consecutive failures,
append-only growth, changed terminal, and a mismatched anchor.
Confidence: medium -
[evidence](evidence/wp-p11-fail-open-retains-its-acknowledgment-basis.md). The
witnesses were read at `f2442b2f`. #884 lists review Q1 and Q2 as accepted:
the fail-open recheck travels with its source (`source.recheck("fail-open")`,
`rust-mode-transform.ts:1106`) and checks a verified capture already implies
are dropped. #884 dropped #819's "applied output grew" check because the raw
array now includes the covered prefix.
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1031`
"appends exactly the unacknowledged window suffix and promotes no basis";
`:1207` "serves raw against a mismatched basis anchor or a changed terminal
message"; `:1092` "serves raw when a rerun ${...} after rediscovering the basis
the daemon disowned"; `:1061` "appends the unacknowledged window suffix on a
${status} decline" (#884).
Impact: High. A wrong basis duplicates or omits context.
Open questions: None.

### wp-p12-boundary-index-is-fixed-through-publication

Type: safety
Reachability: default-production - every revision 3 pass rechecks the fixed
window at wire build, series restart, publish, and fail-open
(`recheckCapture`,
`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1360`,
`f2442b2f`; #883, with the fail-open recheck from #884).
Status: active
Exercised: yes - "declines ${name} during the await with no candidate write
and no promotion" constructs prefix deletion, same-length reorder, root
rebinding, and interior window omission during the await; "declines when the
host moves the discovered anchor before the window is copied" and "declines
cleanly when the host replaces its root array before a rerun" cover the
discovery and rerun gaps. All ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates.
Guarantee: A pass never relocates its captured window to compensate for host
mutation after capture.
Check: `always` - Every recheck uses the original target, captured length,
fixed boundary index, slot references, and content tapes; root rebinding,
length change, or changed captured slots declines before candidate publication
and state promotion. Falsifier: rescan for the boundary after prefix deletion
and accept its new index for the old recipe.
Fault/timing angle: Prefix deletion, same-length reorder, append, or root
rebinding during an await.
Required faults and enabling state: Nonzero boundary index and controlled
pauses before send, after response, and before publication.
Confidence: medium -
[evidence](evidence/wp-p12-boundary-index-is-fixed-through-publication.md).
The witness family was read at `f2442b2f`. #883's Deviations state root
properties outside the window are unchecked (walking them is O(N)), so a host
`metadata` edit now publishes; #883 adds the window head recheck after
discovery (review P4) and reruns on the first attempt's validated root.
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:302`
"declines ${name} during the await with no candidate write and no promotion"
(4 cases: `:285`, `:288`, `:293`, `:296`); `:845` "declines when the host
moves the discovered anchor before the window is copied"; `:734` "declines
cleanly when the host replaces its root array before a rerun" (#883). TE17 is
the whole-array predecessor.
Impact: High. Retargeting can validate one capture and publish another.
Open questions:

- Root properties outside the window are unchecked (#883 deviation), so a host
  edit there during the await publishes; is that accepted? (needs human input)

### wp-p13-id-scan-invokes-no-host-hooks

Type: safety
Reachability: default-production - every revision 3 pass locates its window
with `scanMessageIds`, and discovery builds `messageIdFilter` over the host
array (`packages/opencode-plugin/src/hooks/context/transform-capture.ts:101`,
`:121`, reading through `readOwnDataProperty` at `:86`, `f2442b2f`; #883).
Status: active
Exercised: yes - "crosses planted proxies, accessors, and revoked proxies
without invoking any hook" runs `scanMessageIds` and `messageIdFilter` across
hostile hops with trap counters at zero, and "declines a matched boundary that
fails the hostile walk instead of picking another" runs a pass whose matched
boundary fails the walk; both ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates. The hostile covered-slot traversal is
constructed at the primitive level, not inside a discovery pass (see the
investigation log).
Guarantee: Boundary lookup invokes no proxy trap, accessor, prototype getter,
iterator, or coercion hook at any lookup hop.
Check: `always` - Instrument hooks at array slot, message, `info`, and `id`;
all invocation counters remain zero during scan and decline; reads use own
data descriptors and proxy rejection at each hop; a matched but
nonreferenceable boundary declines rather than selecting a different anchor.
The forbidden effect has no single code point, so this is `always(!hook
invoked)`, not `unreachable`. Falsifier: read `messages[i].info.id` before
checking whether the slot or message is a proxy.
Fault/timing angle: Hostile slots lie in covered history or at the matched
boundary.
Required faults and enabling state: A missing boundary forces traversal
through hostile covered slots; revoked proxies and inherited ids.
Confidence: medium -
[evidence](evidence/wp-p13-id-scan-invokes-no-host-hooks.md). Both witnesses
were read at `f2442b2f`. The hostile scan is exercised through
`scanMessageIds` and `messageIdFilter` directly, not through a full discovery
pass.
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:123`
"crosses planted proxies, accessors, and revoked proxies without invoking any
hook"; `:256` "declines a matched boundary that fails the hostile walk instead
of picking another" (#883). TE19's guard witnesses remain.
Impact: High. Scan-time hooks can mutate capture state or throw inside
publication preparation.
Open questions:

- Is the hostile traversal constructed through a discovery pass?
  Unresolved: a pass-level witness is needed, or an explicit owner
  acceptance of the primitive-level witness (needs human input).

### wp-p14-first-user-policy-comes-from-session-authority

Type: safety
Reachability: default-production - every pass computes tool availability
through `resolveEidnaraReduceAvailability` and `resolveTodowriteAvailability`
(`packages/opencode-plugin/src/hooks/context/rust-mode-transform.ts:1429-1430`,
`f2442b2f`; #884).
Status: active
Exercised: partial - verdict direction constructed; frozen status unasserted.
"takes the verdict from the earliest user row in both signal
directions" and "freezes fail-open without a database and stays fail-closed
for an unpersisted session" run cold passes whose window starts past the first
user against an installed database; `eidnara-reduce-availability.test.ts`
"keeps separate eidnara_reduce and todowrite verdicts for one session" pins
the per-tool cache key. All ran in the #884 `bun run check:repo` gate (head
`f8734c12`).
Guarantee: A cache miss resolves first-user policy from the session database,
never from the first user message remaining in the window.
Check: `always` - With a readable persisted session, the verdict matches its
earliest user row under the resolver's ordering, regardless of contradictory
tools on the window's first user; the resolver's no-database, no-row, and
read-error freezing behavior is preserved. Falsifier: freeze allow from the
first visible tail user after restart.
Fault/timing angle: Plugin restart removes the cached verdict while coverage
hides the actual first user.
Required faults and enabling state: Earlier user denies while the window user
allows; the reverse; unavailable database; unpersisted session.
Confidence: medium -
[evidence](evidence/wp-p14-first-user-policy-comes-from-session-authority.md).
Both witnesses were read at `f2442b2f`. #884's Evidence states three of these
cases fail against PR one's code, which is the discriminating evidence. #884
deletes the `*AvailabilityFromMessages` resolvers (review Q3); `git grep`
finds no hit in `packages` at `f2442b2f`.
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:1278`
"takes the verdict from the earliest user row in both signal directions";
`:1293` "freezes fail-open without a database and stays fail-closed for an
unpersisted session";
`packages/opencode-plugin/src/hooks/context/eidnara-reduce-availability.test.ts:93`
"keeps separate eidnara_reduce and todowrite verdicts for one session" (#884).
Impact: High. Windowing can change whether a tool is callable.
Open questions:

- Is the frozen status compared, not only the Boolean? Unresolved: the tests
  compare tool flags; a frozen-status assertion is needed. Queued in
  `portfolio-evaluation.md`. The no-database case returns callable and
  unfrozen by existing policy; a cached deny is never lifted.

### wp-p15-steady-state-work-is-window-bounded

Type: safety
Reachability: default-production - every admitted steady-state transform; the
benchmark driver is test-only and uncommitted under the gitignored
`docs/performance/`.
Status: active
Exercised: yes - #833's acceptance run (commit `cf89a9c2`, Rust 1.98.1,
Bun 1.3.14, release builds, W = 300 of the fixed 5 KiB shape, 30 samples per
point) records items scanned (300 at every N), rows visited and VM steps
(every D15 row equal at N = 1M / H = 50k except m0 under its D14 cap), and
retained state (`cache_state.meta` equal in entries, 26 B of decimal digits
apart; RSS per idle session within 1.7%). Bytes decoded per D15 row (commit
`3ebfc3b9`, same method): equal at N = 1M / H = 50k except m0 (14,442 to
387,504 B, the fold's rows), the coverage snapshot (up to 23%, the frozen
fold render in the session row), temporal marks (+203 B), and summarizer
assembly (+65 B), the last two decimal digits; each is within its WP-P07
bound. In CI, the #874 inventory and #883's plugin counter test ("stays
equal at a fixed window for 10k and 100k host messages", W = 5) ran in
#833's gates. The readings await owner confirmation.
Guarantee: At fixed window bytes and rendering budget, the work counters of an
admitted steady-state pass (items scanned, bytes serialized, rows visited,
bytes decoded, retained bytes) are equal across N and H.
Check: `always` - At fixed W and budget, every work counter recorded for a
pass at N = 1M and H = 50k equals the counter at N = 10k and H = 100. Wall
clock is not a catalog predicate; the acceptance criteria carry the latency
targets. Falsifier: fixed W with plugin latency increasing linearly with N
because destination preflight still reads every slot.
Fault/timing angle: Hidden full-history work appears only at large N or H.
Required faults and enabling state: Fixed-size messages and window, populated
overlays, active summarizer publication, and independent N and H sweeps.
Confidence: medium -
[evidence](evidence/wp-p15-steady-state-work-is-window-bounded.md). #873 and
#874 record the revision 2 baseline (N = 1M INCONCLUSIVE); #833 records the
revision 3 counters and the wall-clock contract with artifact identity. The
plugin counter test was read at `f2442b2f`. Wall-clock readings (#833, not a
catalog predicate). The daemon, RSS, meta, and D15 readings are at
`cf89a9c2`; the later commits change only the plugin, the admission scans
(speed only), and tests. Daemon `total` p99 14.57 / 15.55 / 19.67 ms at N =
10k, 100k, 1M, 1M/100k median 0.972; HARD at H = 50k against H = 2,500 (≥ K
= 2,484): time 1.017, rows 1.000, hold 1.031 (against H = 100, reported:
1.163, 1.253, 1.098). The plugin readings are at `58dbe556`, whose production
code is identical to `f2442b2f`, in three runs at 1-minute load 5.8 to 7.4:
steady whole-hook p99 70.7 to 73.3, 67.2 to 80.9, and 66.6 to 76.2 ms at N =
10k, 100k, 1M, 1M/100k median 0.968 to 1.082; cold start at N = 1M p99 84.7
to 90.2 ms; revert past the boundary two-pass p99 1,739.1 to 1,848.2 ms. The
H comparison point, the RSS slope reading, and meta equality up to decimal
digits are readings pending owner confirmation, as is bytes-decoded
equality up to the fold render and decimal digits.
Existing check: `crates/daemon/src/transform_read_bound.rs:627` (at `3ebfc3b9`)
`every_pass_read_is_bounded_independent_of_history_size` (#874);
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:998`
"stays equal at a fixed window for 10k and 100k host messages" (#883); `:384`
"finds a cold start's anchor with one backward scan of the window's distance"
(#833, D17). The acceptance drivers are uncommitted, under the gitignored
`docs/performance/`; the #833 PR description carries the numbers.
Impact: High. Long sessions still hit deadlines despite windowed ingress.
Open questions:

- The HARD comparison point: H = 50k against H = 2,500 (≥ K) with m0 checked
  against its D14 bound, rather than against H = 100 (needs human input).
- RSS per idle session read as the slope over k = 8 sessions, not the
  one-session delta (needs human input).
- `cache_state.meta` equal across N up to decimal-digit width (needs human
  input).

### wp-p16-prune-publish-race-is-actually-constructed

Type: reachability
Reachability: explicit-config-only - the production overlap requires a
configured summarizer; the barriers are test instrumentation.
Status: active
Exercised: yes - #833's two WP-P06 tests are the two orders; each asserts
the marker before its verdict, and both ran in #833's `cargo test -p daemon`
gate.
Guarantee: The verification campaign constructs both publication orders while
the selected range and pruning decision conflict.
Check: `sometimes` - Across the campaign, observe both writers read the same
starting version, a selected mid absent from the resolved window, and each
writer winning first in separate runs. The marker asserts enabling state and
order, never stale publication, so it fires on a correct implementation.
Fault/timing angle: Both writers pause between observation and CAS.
Required faults and enabling state: A real pinned selected range, a nonempty
prune set intersecting it, and explicit commit barriers in both orders.
Confidence: high -
[evidence](evidence/wp-p16-prune-publish-race-is-actually-constructed.md).
The marker assertions were read at `f2442b2f`. The shared fixture
`pinned_firing` (`crates/daemon/src/transform.rs:19574`) asserts the resolved
window is `[m4, m5]`, the firing selects `m6`, and the store holds `m6`'s
identity. In each run the attempt hook asserts the row is still at the
publisher's expected version when the transform reaches its commit, and sets
a flag the test asserts afterwards.
Existing check: `crates/daemon/src/transform.rs:19708` (prune first) and
`:19741` (publish first), #833.
Impact: High. WP-P06 passes vacuously if publication always finishes before
pruning starts.
Open questions: None.

### wp-p17-interior-omission-is-constructed

Type: reachability
Reachability: test-only - fixture construction.
Status: active
Exercised: yes - the "interior window omission" case of "declines ${name}
during the await with no candidate write and no promotion" asserts a window at
boundary index 6, removes an interior window message during the await, and
asserts the decline; it ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates.
Guarantee: The campaign constructs a window with boundaryIndex > 0 and a
candidate submission that omits an interior tail message.
Check: `sometimes` - Observe boundaryIndex > 0 and an omitted interior message
in at least one run; WP-P01 must reject it. This is situation coverage, so
`sometimes`.
Fault/timing angle: none
Required faults and enabling state: A covered prefix and at least three tail
messages.
Confidence: medium -
[evidence](evidence/wp-p17-interior-omission-is-constructed.md). The case was
read at `f2442b2f`. It removes host slot 8 and appends a new message during
the await, so the omission is a host mutation after capture rather than an
encoder omission.
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:302`
"declines ${name} during the await with no candidate write and no promotion",
case "interior window omission" (`:296`) (#883).
Impact: WP-P01 passes vacuously without it.
Open questions: None.

### wp-p18-a-waiting-pass-actually-waited

Type: reachability
Reachability: test-only - a campaign marker over test instrumentation; no
production code path asserts it.
Status: active
Exercised: yes -
`session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order`
holds the active pass at a barrier, observes one submission and three free
unit permits while the second waits, refuses the third with `session_busy`,
then observes both commits in arrival order; it ran in the #875 daemon gate.
Guarantee: The campaign observes one active pass, one waiting pass that later
executes, and a third request refused `session_busy` while both are live.
Check: `sometimes` - Observe the waiting pass's admission before the active
pass's completion and its execution after; observe the refusal while two are
live. Situation coverage, so `sometimes`.
Fault/timing angle: The active pass is held at a barrier.
Required faults and enabling state: Three same-session requests and one
barrier.
Confidence: high -
[evidence](evidence/wp-p18-a-waiting-pass-actually-waited.md). Read at
`f2442b2f` (`transform_unit/tests.rs:1016`); #875 names this test as the
WP-P18 witness.
Existing check: `crates/daemon/src/transform_unit/tests.rs:1016`; `:1141`
(contention, 400 tasks).
Impact: WP-P04 passes on a plain mutex without it.
Open questions: None.

### wp-p19-a-nonzero-cut-is-constructed

Type: reachability
Reachability: test-only - a campaign marker over test instrumentation; no
production code path asserts it.
Status: active
Exercised: yes -
`a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input`
resolves `StaleSlice` at cut 4 with an input keep; it ran in the #875 daemon
gate. At the handler, #881's
`stale_slice_input_keeps_address_the_submitted_native_window` asserts
`StaleSlice { cut: 2 }` and a nonempty set of input keeps in the same pass; it
ran in the #881 `cargo test -p daemon` gate (head `1c66f16c`).
Guarantee: The campaign constructs a stale-slice resolution with cut > 0 and a
recipe that keeps from the submitted input.
Check: `sometimes` - Observe cut > 0 and at least one input keep in the same
pass; WP-P05 reconstructs against the unsliced input.
Fault/timing angle: A fold advanced coverage between passes.
Required faults and enabling state: One published segment after the plugin's
last response.
Confidence: high -
[evidence](evidence/wp-p19-a-nonzero-cut-is-constructed.md). Read at
`f2442b2f` (`window_coverage/tests.rs:513`); the table test also asserts
`StaleSlice { cut: 4 }` and `{ cut: 7 }`.
Existing check: `crates/daemon/src/window_coverage/tests.rs:513`;
`crates/daemon/src/transform/revision_3.rs:988` (#881).
Impact: WP-P05 passes vacuously at cut = 0.
Open questions: None.

### wp-p20-the-shrink-actually-threw

Type: reachability
Reachability: test-only - a campaign marker over test instrumentation; no
production code path asserts it.
Status: active
Exercised: yes - "restores the captured references after a shrink stopped by a
planted non-configurable slot" plants a non-configurable slot at k with S <= k
on a real Bun array and asserts the `TypeError`, length k + 1 before
restoration, and no candidate entry; it ran in the #877 `bun run check:repo`
gate.
Guarantee: The campaign plants a non-configurable slot at k with S <= k and
observes the length shrink throw.
Check: `sometimes` - Observe the TypeError, length k + 1 before restoration,
and zero candidate writes.
Fault/timing angle: none
Required faults and enabling state: A real Bun array with the planted slot.
Confidence: high - [evidence](evidence/wp-p20-the-shrink-actually-threw.md).
Read at `f2442b2f` (`transform-capture.test.ts:1098`); the observed-length
probe records `[k + 1]` at the first slot define.
Existing check:
`packages/opencode-plugin/src/hooks/context/transform-capture.test.ts:1098`;
`rust-mode-transform.test.ts:3252`; window variant
`transform-capture.test.ts:1138` (#883; ran in the #883 (head `d7712d75`) and
#884 (head `f8734c12`) `bun run check:repo` gates).
Impact: WP-P09 passes vacuously if the shrink never fails.
Open questions: None.

### wp-p21-discovery-passed-page-one-and-a-budget-fired

Type: reachability
Reachability: test-only - a campaign marker over test instrumentation; no
production code path asserts it.
Status: active
Exercised: yes - "declares a match found past page one" (cursors `[undefined,
80]`) and "declines when the time budget fires with no null submission" (more
than one cursor before the budget) construct both situations; both ran in the
#883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run check:repo` gates.
Guarantee: The campaign observes a discovery walk that continues past its
first page to a match, and a separate walk whose time budget fires.
Check: `sometimes` - Observe page count >= 2 with a match on a later page in
one run and a budget exit with no `null` submission in another.
Fault/timing angle: The daemon delays a page in the budget run.
Required faults and enabling state: More anchors than one page; an injected
page delay.
Confidence: medium -
[evidence](evidence/wp-p21-discovery-passed-page-one-and-a-budget-fired.md).
Read at `f2442b2f` (`:320` and `:436` at `d7712d75`). #883 measures the budget
from the pass start (review P6).
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:341`;
`:589` (#883).
Impact: WP-P10 passes vacuously on single-page walks.
Open questions: None.

### wp-p22-the-host-mutated-during-the-await

Type: reachability
Reachability: test-only - a campaign marker over test instrumentation; no
production code path asserts it.
Status: active
Exercised: yes - the fixed-window family "declines ${name} during the await
with no candidate write and no promotion" mutates the host by prefix deletion,
same-length reorder, root rebinding, and interior window omission between send
and response, with the enabling state asserted first; it ran in the #883 (head
`d7712d75`) and #884 (head `f8734c12`) `bun run check:repo` gates.
Guarantee: The campaign mutates the host array (prefix deletion, same-length
reorder, root rebinding) while a response is pending.
Check: `sometimes` - Observe each mutation shape between send and response in
separate runs; WP-P12 must decline publication in each.
Fault/timing angle: Deferred fake-transport response.
Required faults and enabling state: The deferred-response fake client.
Confidence: medium -
[evidence](evidence/wp-p22-the-host-mutated-during-the-await.md). Read at
`f2442b2f`; `reached.promise` resolves when the second transform body arrives,
before the mutation.
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:302` (4
cases); `:845` (anchor moved before the copy) (#883).
Impact: WP-P12 passes vacuously without a mutation.
Open questions: None.

### wp-p23-the-scan-traversed-a-hostile-slot

Type: reachability
Reachability: test-only - a campaign marker over test instrumentation; no
production code path asserts it.
Status: active
Exercised: yes - "crosses planted proxies, accessors, and revoked proxies
without invoking any hook" places every hostile hop between the end and the
one readable match and asserts the scan returns that match with zero traps; it
ran in the #883 (head `d7712d75`) and #884 (head `f8734c12`) `bun run
check:repo` gates. The array is a bare fixture, not a pass (see WP-P13).
Guarantee: The campaign places a proxy, an accessor, and a revoked proxy at
each id-read hop inside the region the backward scan crosses.
Check: `sometimes` - Observe the scan crossing each hostile slot with zero
trap invocations counted.
Fault/timing angle: none
Required faults and enabling state: Trap counters on each planted object.
Confidence: medium -
[evidence](evidence/wp-p23-the-scan-traversed-a-hostile-slot.md). Read at
`f2442b2f`; the in-test comment states the enabling state ("every hostile hop
sits between the end and the one readable match"). The traversal is at the
primitive level (see WP-P13).
Existing check:
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:123`
(#883).
Impact: WP-P13 passes vacuously if the scan stops before the hostile slot.
Open questions:

- Are the hostile slots in a covered region of a real pass? Unresolved: a
  pass-level witness is needed, or owner acceptance of the primitive-level
  witness (needs human input).

### wp-p24-wire-admissibility-is-explicit

Type: safety
Reachability: default-production - every revision 3 request and every
discovery call (`transform_revision_unsupported` at
`crates/daemon/src/lib.rs:8226`; `window_refusal` at `:13457`, `f2442b2f`).
Status: active
Exercised: yes - #875's discovery shape (unsafe `before_sequence` including
`i64::MIN` and `i64::MAX`, malformed bodies `invalid_params`, unknown methods
still `unrecognized_request_shape`) ran in the #875 daemon gate. #881's
`a_missing_or_non_3_revision_is_refused_with_expected_and_received_and_no_state_change`,
`boundary_presence_head_sequence_and_duplicates_are_invalid_params`,
`a_boundary_that_does_not_decode_is_bad_request`, and
`paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request`
ran in the #881 `cargo test -p daemon` gate (head `1c66f16c`), and #881's
`test:rust` against the revision 2 plugin is red by design (9 pass, 14 fail).
The plugin side ("serves raw and logs an upgrade hint when the daemon refuses
the revision", the "a revision 2 daemon" decline, and the final-page test in
`module-wire.test.ts`) ran in the #883 (head `d7712d75`) and #884 (head
`f8734c12`) `bun run check:repo` gates; the paired `test:rust` is 23 pass, 17
skip, 0 fail in #881 (with #883), #883, and #884.
Guarantee: The daemon admits exactly the revision 3 request shape and refuses
every other with a stable, distinguishable answer.
Check: `always` - Missing or non-3 `v` answers
`transform_revision_unsupported` with expected and received revisions; a
missing `boundary` (distinct from `null`) answers `invalid_params`; a window
head that is not the declared mid answers `invalid_params`; a `sequence`
outside the safe-integer range is refused on both sides; paged `v` and
`boundary` are final-page scalars validated on the assembled request; a
revision 2 daemon answers discovery with `unrecognized_request_shape`, which
the plugin treats as a decline. Falsifier: a decoder that defaults a missing
`v` to 2.
Fault/timing angle: Process version skew in either direction.
Required faults and enabling state: A revision 2 plugin against a revision 3
daemon and the reverse.
Confidence: medium -
[evidence](evidence/wp-p24-wire-admissibility-is-explicit.md). The #875, #881,
and #883 witnesses were read at `f8734c12` and re-located at `f2442b2f`.
#881's gate block records `test:rust` 9 pass, 14 fail on
#881 alone, by design: the revision 2 plugin is refused with
`transform_revision_unsupported` ("expected transform revision 3, received 2")
and serves raw. That demonstrates the skew refusal end to end.
Existing check: `crates/daemon/src/window_coverage/dispatch_tests.rs:81`
`malformed_boundary_bodies_are_invalid_params` (#875);
`crates/daemon/src/transform/revision_3.rs:158`
`a_missing_or_non_3_revision_is_refused_with_expected_and_received_and_no_state_change`,
`:193`, `:231` `a_boundary_that_does_not_decode_is_bad_request`, `:287`
`paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request`
(#881);
`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:938`
"serves raw and logs an upgrade hint when the daemon refuses the revision", the
"a revision 2 daemon" case of `:562`, and `module-wire.test.ts:901` "carries v
and boundary ${JSON.stringify(boundary)} on the final page only" (#883).
Impact: High. Silent acceptance of a revision 2 body would apply an
ordinal-bearing array to the window protocol.
Open questions: None.

### wp-p25-legacy-meta-is-pruned-on-first-commit

Type: safety
Reachability: default-production - any session written before D12: its
`block_identities` rows hold every covered mid, and a row written before
`e15a09a6` may still embed a `block_identity_by_mid` key in `meta`
(`ModuleMeta::block_identity_by_mid` is `#[serde(skip)]`,
`crates/memory-store/src/lib.rs:2110-2111`, `f2442b2f`).
Status: active
Exercised: yes - #833's legacy fixture (1,998 + 300 identity rows, a `meta`
row built to 480 to 512 KiB with an embedded identity map, and a summarizer
firing `AwaitingProducer`) is read after a restart and pruned on its first
commit, and converges under a CAS conflict in both orders; the three tests
ran in #833's `cargo test -p daemon` gate (2,749 passed).
Guarantee: A persisted `block_identity_by_mid` larger than the window is read
as-is, pruned to window mids on the first ordinary commit, and never grows
again.
Check: `always` - After the first ordinary commit over a legacy row, the map
contains exactly the submitted window's mids (cut prefix plus resolved
messages); a revert's SOFT commit keeps the map, because
`prune_block_identities` returns early on a Revert unless the plan is HARD or
MigrateHard (`crates/daemon/src/transform.rs:5266-5268`). The row is below
128 KiB; a restart before
that commit reads the legacy row without error; a CAS conflict in either order
retries and converges. Falsifier: pruning only on HARD passes, leaving
Defer-only sessions unbounded.
Fault/timing angle: Restart or CAS conflict around the first commit.
Required faults and enabling state: A fixture row near the 512 KiB limit with
an active summarizer state.
Confidence: high -
[evidence](evidence/wp-p25-legacy-meta-is-pruned-on-first-commit.md). The
prune, the fixture, and the three tests were read at `f2442b2f`. Adaptation
recorded by #833: since `e15a09a6` the identity map is the `block_identities`
table, not the `meta` blob, so the obligation applies to both parts of a
legacy session. "The map contains exactly the window's mids" is checked on
the identity rows; "the row is below 128 KiB" on the `meta` row, which any
struct-serialized commit shrinks because serde ignores the embedded key.
Existing check: `crates/daemon/src/transform_meta_bound.rs:177`
`a_legacy_row_is_read_after_a_restart_and_pruned_on_its_first_commit`
(marker: the fixture asserts 480 to 512 KiB of `meta` and the reopened load
holds 2,298 identities; then `SOFT+`, not HARD, committed; `assert_pruned`
at `:166`: identities equal the window, `meta` under 128 KiB, firing still
`AwaitingProducer`); `:195`
`a_legacy_prune_that_loses_its_cas_reloads_and_prunes` (a writer commits
inside the attempt hook; the retry prunes); `:230`
`a_writer_that_loses_to_the_legacy_prune_reloads_the_pruned_row` (a stale
writer at the legacy version gets `CasConflict`, reloads, and commits the
pruned map).
Impact: High. Without the prune every legacy session stays near the cliff.
Open questions:

- The adaptation to the `block_identities` table is #833's reading of the
  obligation and awaits owner confirmation with the PR (needs human input).

## Relationship map

- Safety records and their campaign markers: WP-P01 with WP-P17; WP-P04 with
  WP-P18; WP-P05 with WP-P19; WP-P06 with WP-P16; WP-P09 with WP-P20; WP-P10
  with WP-P21; WP-P12 with WP-P22; WP-P13 with WP-P23. A green safety check
  without its marker is not coverage.
- Invalidated baselines and successors: WP-E06 by WP-P09 (D21); WP-E11 by
  WP-P11 (the no-failed-candidate clause survives).
- Shared mechanism, coverage resolution: WP-P02 feeds WP-E01, WP-E02, WP-E03,
  WP-P03, and WP-P05; D10 routes the no-survivor null-anchor shape away from
  WP-E03 into the reset (#833).
- Reset preconditions: WP-P10 (a null boundary only after a complete walk)
  and WP-P01 (the window is the complete declared suffix) gate the D10
  no-survivor reset, which deletes segments and bumps the epoch; a dedicated
  reset record is queued in `portfolio-evaluation.md`.
- Shared mechanism, meta identities: WP-E04, WP-E10, WP-P06, and WP-P25 read
  or write `ModuleMeta::block_identity_by_mid`.
- Shared mechanism, revert epoch: WP-E02 and WP-E12 share
  `truncate_history_segments_for_revert`.
- Shared mechanism, window capture: WP-P01, WP-P12, WP-P13, and WP-E07 share
  `scanMessageIds`, `copyWindow`, and the capture lease.
- Cost: WP-P07 (per-read bounds) and WP-P08 (fold horizon) supply the counters
  WP-P15 compares across N and H.
- Canonical records elsewhere: TE17 to TE30
  (`opencode-plugin/transform-edit-responses/`), the shared-primitives
  reconcile records, the daemon transform recut records, the
  history_summarizer publication fence, and hot-path E2, H1, H2, B1, B3, B5.
  Their dispositions are listed in `portfolio-evaluation.md`.
