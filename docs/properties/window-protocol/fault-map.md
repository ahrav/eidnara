# Fault map: window protocol (M1)

Every row of the catalog revision 2 fault map (#824 comment 4, section D),
with its occurrence marker. An occurrence marker is the independent
enabling-state assertion that proves the fault or state was constructed
before the safety check fired; per METHOD's coverage-check rules it asserts
preconditions, never the violation, and fires on a correct implementation.
Marker names are constant and unique (`WP-FM01` to `WP-FM22`). A green safety
assertion without its marker is not coverage.

"Observed" names the witness and the recorded PR run in which the marker
fires. `file:line` is at `f2442b2f` (#833's last code commit) unless noted.
The #833 run is the gate block of the #833 PR description (gates at the final
head, code at `f2442b2f`).

The repository has no campaign runtime. A `sometimes` record (WP-P16 to
WP-P23) and a marker here mean that a named deterministic test constructs the
state once per suite run and asserts it before its verdict. No `WP-FMnn`
token appears in code: the link between a marker and its test exists only in
this file. The daemon has one uniqueness test for coverage marker names,
`crates/daemon/tests/eval_ingestion.rs:857`.

## Rows

| Marker | Fault or enabling state | Occurrence marker (assert before the check) | Observed | Records |
| --- | --- | --- | --- | --- |
| WP-FM01 | Revert past rendered boundary, earlier anchor survives | The revert pass answers `SOFT+` with `reconcile_pending` true while the earlier anchor is present in the window | `reconcile_rematerialize_with_unrecut_store_truncates_and_refolds_prefix` (`crates/daemon/src/transform.rs:19455`), #880. Revision 3 `Revert { keep_through_seq: Some(3) }` in `each_resolution_outcome_has_its_cut_ordinals_and_keeps` (`crates/daemon/src/window_coverage/tests.rs:99`), #875 | WP-E01, E02, E12; WP-P01, P02, P03, P04, P05, P10, P12, P15 |
| WP-FM02 | Revert before first anchor | Resolution `Revert { keep_through_seq: None }` with coverage held and no surviving segment end in the window | Module case "no boundary, coverage held, and no surviving segment end" (`window_coverage/tests.rs:99`), #875. Handler level: `a_revert_before_the_first_anchor_resets_and_serves_the_window_as_a_first_pass` (`crates/daemon/src/transform/revision_3.rs:447`; marker: the resolution is `Revert { keep_through_seq: None }` and `removed_sequence_range` reads "1..=2" before the pass, with no `lineage_switched`), #833; the log line itself is not asserted | WP-E03; WP-P01, P02, P10, P11 |
| WP-FM03 | Interior covered-message removal with anchor intact | A stale declared row resolved with and without the covered interior message `m3`, same anchor | `resolved_ordinals_equal_the_independent_model` (`window_coverage/tests.rs:436`), #875. Plugin side: #833's acceptance event at N = 1M (one covered slot deleted at index 1,000 + 1,000·i on a transform with a known boundary; marker: the deletion precedes the pass and the boundary is known) scanned 300 items with one `transform` call, no `transform.boundary`, and no error, 30/30 in each of 3 runs at `cf89a9c2` and again at `58dbe556` | WP-E01; WP-P01, P03, P05, P12, P15 |
| WP-FM04 | Prune commits before summarizer publication | Both writers observed at the same starting `row_version`, a selected mid absent from the resolved window, prune CAS first | `a_prune_that_commits_first_fences_the_publication_out` (`crates/daemon/src/transform.rs:19708`), #833. Marker: `pinned_firing` (`:19574`) asserts the resolved window `[m4, m5]`, `m6` selected, and `m6`'s identity held; the attempt hook asserts the row is at the firing's version before the transform's CAS | WP-E04, E12; WP-P02, P06, P16 |
| WP-FM05 | Summarizer publication commits before prune | Same preconditions as WP-FM04, publication CAS first | `a_publication_that_commits_first_makes_the_transform_reload_and_match_the_serial_run` (`crates/daemon/src/transform.rs:19741`), #833, with the WP-FM04 marker; the publication commits inside the hook at the shared version. Related but distinct: `a_publish_between_the_core_read_and_the_declared_read_is_invisible` (`window_coverage/tests.rs:287`, #875) commits a publish inside the resolver snapshot | WP-E04, E08; WP-P02, P04, P06, P16 |
| WP-FM06 | Daemon store reset while plugin retains boundary | The declared row is missing while the plugin still holds its anchor; the daemon answers `boundary_unknown` | Module: `a_declared_row_without_a_rendered_boundary_is_unknown` (`window_coverage/tests.rs:237`), #875. Plugin rediscovery: "rediscovers once after boundary_unknown and declines the second in one pass" (`packages/opencode-plugin/src/hooks/context/rust-mode-window.test.ts:607`), #883 and #884; an anchor lost after discovery that rediscovers once, "rediscovers and publishes when an anchor it just discovered draws boundary_unknown" (`:647`), #883's review update (`adcc7baf`). A real reset under a retained plugin boundary is not constructed; #833's D10 reset runs only for a `boundary: null` window, where the plugin holds no anchor | WP-E08, E12; WP-P01, P02, P10, P11 |
| WP-FM07 | Plugin restart with durable coverage | A fresh transform instance (no `state.boundary`) with durable anchors issues a discovery page before its first transform body | "declares a match found past page one" (`rust-mode-window.test.ts:341`, cursors `[undefined, 80]`) #883 and #884, and the first-user cold pass (`:1278`), #884. Cold start at N = 1M: #833's acceptance event (a fresh transform instance per sample; marker: one `transform.boundary` page precedes the one `transform` call, scanned 300); at `58dbe556` (production code identical to `f2442b2f`) 30/30 `ok/SOFT+` in each of 3 runs at 1-minute load 5.9 to 7.4, whole-hook median 77.7 to 81.9 ms, p99 84.7 to 90.2 ms | WP-E05; WP-P01, P02, P10, P11, P14, P15 |
| WP-FM08 | Summarizer stalls while tail grows | W grows past the fixed-W population while a firing is held | Not constructed; the specification excludes the stalled-summarizer W-growth envelope from the steady-state sample. #833 does not measure it (not an acceptance criterion) | WP-E07, E08, E10, E11; WP-P01, P07, P08, P11, P15 |
| WP-FM09 | Hostile slot in covered region during absent-anchor scan | The scan result is the one readable match with every hostile hop above it (in-test comment "Enabling state: every hostile hop sits between the end and the one readable match") | "crosses planted proxies, accessors, and revoked proxies without invoking any hook" (`rust-mode-window.test.ts:123`), #883 and #884; primitive level only | WP-E07; WP-P09, P10, P12, P13, P23 |
| WP-FM10 | Duplicate id inside window | The window holds two messages with the same id while the covered prefix holds an unrelated duplicate | Plugin: "rejects a duplicate id inside the window and ignores one outside it" (`rust-mode-window.test.ts:228`), #883 and #884. Daemon: `boundary_presence_head_sequence_and_duplicates_are_invalid_params` "duplicate mid" (`crates/daemon/src/transform/revision_3.rs:193`), #881 | WP-P01, P02, P03, P05 |
| WP-FM11 | Window exceeds paging or admission caps | The request is paged (`transform_page_total` > 1) or the capture declines on `capture_bytes` | Paging: `paged_revision_and_boundary_are_final_page_scalars_checked_on_the_assembled_request` (`revision_3.rs:287`), #881; plugin final-page scalars (`packages/opencode-plugin/src/hooks/context/module-wire.test.ts:901`), #883 and #884. Capture bytes: the TE23 byte-pressure witnesses (transform-edit-responses catalog), #877 run; fail-open on `capture_bytes` (`rust-mode-transform.test.ts:3653`), #884 | WP-E07, E10; WP-P01, P07, P10, P15 |
| WP-FM12 | Discovery page or time budget exceeded | `cursors.length > 1` before the budget fires (in-test comment "Enabling state: the walk passed page one before the budget fired") | "declines when the time budget fires with no null submission" (`rust-mode-window.test.ts:589`), #883 and #884; a rediscovery after a slow first send spent the budget (`:754`), #883 and #884 | WP-E07; WP-P10, P11, P21 |
| WP-FM13 | Process version skew (revision 2 plugin with revision 3 daemon, and the reverse) | The daemon receives `v` 2 or no `v`; the plugin receives `transform_revision_unsupported` or `unrecognized_request_shape` | Daemon: `a_missing_or_non_3_revision_is_refused_with_expected_and_received_and_no_state_change` (`revision_3.rs:158`) and e2e `test:rust` with the revision 2 plugin (9 pass, 14 fail by design), #881. Plugin: `rust-mode-window.test.ts:938` and the "a revision 2 daemon" case of `:562`, #883 and #884 | WP-P24 |
| WP-FM14 | Legacy meta row near the 512 KiB limit | The fixture row's serialized meta is within a stated margin of 512 KiB and carries an active summarizer state before the first commit | `a_legacy_row_is_read_after_a_restart_and_pruned_on_its_first_commit` (`crates/daemon/src/transform_meta_bound.rs:177`), `a_legacy_prune_that_loses_its_cas_reloads_and_prunes` (`:195`), `a_writer_that_loses_to_the_legacy_prune_reloads_the_pruned_row` (`:230`), #833. Marker: `legacy_session` (`:115`) asserts the `meta` row holds 480 to 512 KiB, and the reopened load holds 2,298 identities with the firing `AwaitingProducer`. Adaptation: since `e15a09a6` the prune target is the `block_identities` rows plus a leftover embedded `meta` key | WP-E10; WP-P25 |
| WP-FM15 | Stale-slice resolution with a nonzero cut | Resolution `StaleSlice { cut: 4 }` with at least one input keep | `a_stale_cut_keep_reconstructs_the_served_array_from_the_unsliced_input` (`window_coverage/tests.rs:513`), #875; handler level `stale_slice_input_keeps_address_the_submitted_native_window` (`revision_3.rs:988`, `StaleSlice { cut: 2 }`), #881 | WP-P05, P19 |
| WP-FM16 | Interior tail omission in a candidate window | `bodies[1].boundary` is `m-6` and `native_messages` has length 4 before an interior window message is removed with the length kept | "declines ${name} during the await ...", case "interior window omission" (`rust-mode-window.test.ts:302`, `:296`), #883 and #884 | WP-P01, P17 |
| WP-FM17 | Nonconfigurable covered slot blocks shrinking length | `publicationRejection(target, S)` is null, then the observed length before restoration is k + 1 | `transform-capture.test.ts:1098`, #877; pass level `rust-mode-transform.test.ts:3252`, #877; window variant `transform-capture.test.ts:1138`, #883 and #884 | WP-E06 baseline; WP-P09, P20 |
| WP-FM18 | Prefix deletion or same-length reorder after capture | `bodies[1].boundary` is `m-6` and `native_messages` has length 4 before the mutation, with the response held | "declines ${name} during the await ..." for prefix deletion, same-length reorder, and root rebinding (`rust-mode-window.test.ts:302`), #883 and #884; anchor moved between discovery and copy (`:845`), #883 and #884 | WP-E05, E06; WP-P05, P12, P22 |
| WP-FM19 | Retained output changes while response is pending | The request carrying `previous_output_revision` is in flight when the retained entry is mutated | "rejects mutated retained output ${mutationTime}" (`rust-mode-transform.test.ts:1432`), #877 and #878. Changed terminal under fail-open: `rust-mode-window.test.ts:1207`, #884 | WP-E05; WP-P11 |
| WP-FM20 | Third same-session request during active-plus-waiting state | One submission while the active pass is held and three of four unit permits free, then the third request answers `session_busy` | `session_lane_holds_one_waiter_refuses_a_third_and_runs_in_arrival_order` (`crates/daemon/src/transform_unit/tests.rs:1016`), #875 | WP-E07, E08; WP-P04, P18 |
| WP-FM21 | CAS conflict after recut commits | The truncate commit is observed (segment count reduced, `revert_epoch` + 1) before the injected CAS conflict | Store no-op repeat: `truncate_history_segments_for_revert_deletes_suffix_and_bumps_epoch` (`crates/memory-store/src/lib.rs:26148`), #879. End to end: `a_cas_conflict_after_the_revert_truncate_folds_in_the_same_request` (`crates/daemon/src/transform/revision_3.rs:1147`; its setup asserts one segment left inside the attempt hook before the conflict) and `a_panic_plus_reopen_after_the_revert_truncate_folds_on_the_next_pass` (`:1163`), #881. The #874 inventory's injected CAS conflict is a read-bound witness, not this row | WP-E02, E12; WP-P02, P06 |
| WP-FM22 | Missing actual first user from a cold window | The cold pass's window starts at `m-3` while the database's earliest user row carries contradictory `tools` | "takes the verdict from the earliest user row in both signal directions" (`rust-mode-window.test.ts:1278`), #884 | WP-P14 |

Observed in a recorded PR run: WP-FM01, WP-FM02 (module and handler level),
WP-FM03 (daemon side; plugin side in #833's acceptance run), WP-FM04, WP-FM05,
WP-FM06 (module level and plugin rediscovery), WP-FM07 (the cold-start event
is in #833's acceptance run), WP-FM09 (primitive level), WP-FM10, WP-FM11,
WP-FM12, WP-FM13, WP-FM14, WP-FM15, WP-FM16, WP-FM17, WP-FM18, WP-FM19,
WP-FM20, WP-FM21 (store level and end to end), WP-FM22. Not constructed:
WP-FM08, and WP-FM06 with a real reset under a retained plugin boundary.

## Fault classes and availability

| Fault class | Injection available at `f2442b2f` | Used by |
| --- | --- | --- |
| Store snapshot barrier | `set_coverage_snapshot_hook` on the memory store (used at `window_coverage/tests.rs:287`, `:317`) | WP-FM05 (resolver variant), WP-P02 |
| Transform attempt hook | `install_transform_attempt_hook` (`crates/daemon/src/transform.rs:2184`); `run_transform_attempt_hook` at `:2196`, called at `:2046` (the D10 reset), `:2823`, `:2951`, `:3358`, `:3455`, `:5002` | WP-FM02, WP-FM04, WP-FM05, WP-FM14, WP-FM21 |
| Unit barrier and lane | `transform_unit/tests.rs` barrier runners | WP-FM20 |
| Statement-work ledger | `sqlite3_trace_v2` hook (`crates/storage/src/lib.rs:299`), test-support only | WP-P07, WP-P15 |
| Deferred fake transport | `fakeDaemon` with a held `pending` promise (`rust-mode-window.test.ts:72`) | WP-FM16, WP-FM18 |
| Planted host objects | Proxies, revocable proxies, accessors, non-configurable slots on real Bun arrays | WP-FM09, WP-FM17 |
| Version skew | Hand-built revision 2 bodies; e2e harness with mismatched plugin and daemon | WP-FM13 |
| OpenCode database fixture | `XDG_DATA_HOME` with a created `message` table | WP-FM22 |
| Synthetic history generator | `crates/daemon/src/test_support/synthetic_history.rs` (#873) | WP-P07, WP-P15, WP-E10 (`transform_meta_bound.rs:93`) |
| Panic after truncate plus reopen | `install_transform_attempt_hook` panicking after the truncate commit, then `reopened` over the same directory (`crates/daemon/src/transform/revision_3.rs:1031`, #881) | WP-FM21 |

## Coverage checks to add

1. WP-FM06 with a real reset: `session.recomp` while the plugin holds a
   boundary, then a pass; assert `boundary_unknown`, one rediscovery, and
   null only after an empty page.
2. WP-FM09 through a pass: a discovery walk whose absent-anchor scan crosses
   hostile covered slots, trap counters at zero. No M1 PR adds one (needs
   human input: accept the primitive witness or add one).
3. WP-FM02's log line: assert the emitted "removed history_segment sequences"
   line, not only `removed_sequence_range`.
4. WP-FM08, if measured: a separate W-growth envelope, not pooled into the
   steady-state sample.

## Ranking by cheapest valid oracle

1. WP-FM02's log line: one captured-stderr assertion in the existing reset
   test.
2. WP-FM09 through a pass: reuses the planted objects of
   `rust-mode-window.test.ts:123` inside a discovery test.
3. WP-FM06 with a real reset: an integration test through the e2e harness,
   the most expensive.
4. WP-FM08: a measurement envelope, not a test.
