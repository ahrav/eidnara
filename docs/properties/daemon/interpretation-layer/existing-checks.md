# Existing checks

Every claim-bearing check in the tree for the interpretation layer, verified at
`41d889ea4` and re-checked at `aff5a00e6`, where `lib.rs` and the replay test
moved and the replay gained three tests listed below. Every check is
`unaudited`; adequacy verdicts belong to `/testing:invariant-test-review`.
Abbreviations follow [`catalog.md`](catalog.md#scope). The inventory carries #1105 Part A's table
forward and adds the checks U1 to U4 landed; a row whose source citation moved
or whose check U4 deleted says so.

## Tool vocabulary (landed by U4)

| Check | Covers | Status |
| --- | --- | --- |
| `tv-tests:17` `each_consumer_keeps_its_name_rule` | 25 names, including `Edit`, `mcp_edit`, `MCP_TodoWrite`, `Bash`, `MCP_READ`, `mcp_mcp_edit`, through `classify`, `tier`, the injection read-back, and `exit_code` for both harnesses | unaudited |
| `tv-tests:66` `each_site_keeps_its_key_order` | fourteen inputs through `path_arg` and `extract_key_arg`: two recognized keys, a non-string first key, `file_path` alone, the display-only keys each alone, a 61-scalar path, `description` alone, a non-object; `diff_keys` order. No input pairs `pattern` with `filePath` | unaudited |
| `tv-tests:127` `pi_and_opencode_path_keys_name_one_path` | Pi `path` and OpenCode `filePath` yield one value | unaudited |
| `tv-tests:133` `an_unknown_tool_takes_every_default` | `mcp_foo` is `Unknown`, tier 3, generic display argument, no exit code; `tv-tests:44` and `:58` hold the doubly prefixed `mcp_mcp_edit`, `Unknown` with selection tier 2 | unaudited |
| `tv-tests:145` `exit_codes_come_only_from_opencode_bash_integers` | the OpenCode 1.19.0 trace fixture cases plus `read`, string, float, and Pi cases | unaudited |
| `tv-tests:185` `truncation_flags_are_read_from_each_harness_field` | both pointers, a non-boolean value, a missing flag, a Pi record read as OpenCode | unaudited |
| `tv-tests:205` `zero_value_note_actions_are_read_and_dismiss` | the `eidnara_note` zero-value actions | unaudited |
| `tv-tests:215` `every_tabled_name_reaches_a_non_unknown_answer` | every tabled name, raw and `MCP_`-prefixed | unaudited |
| `tv-tests:232` `edge_names_classify_as_unknown` | empty, `mcp_foo`, a 10 KiB name, a NUL, a non-ASCII name | unaudited |
| `tv-tests:248` `classify_is_total_and_deterministic` (proptest) | arbitrary strings | unaudited |
| `tv-tests:257` `tabled_names_classify_under_any_ascii_case_and_one_mcp_prefix` (proptest) | case flips and one prefix over every tabled name | unaudited |
| `sel.rs:2828` `identical_calls_produce_no_dedup_decision_for_any_tool_name` | identical `read`, `mcp_read`, `mcp_foo` calls produce no decision; an `edit` positive control reaches supersession | unaudited |
| `scripts/forbid-tool-literals.test.ts` (run by `bun test ./scripts` in `bun run check:repo`) | the gate's exit 0, 1, and 2 contract, its path filter, its test-region skip, `--staged` mode, and a run from a subdirectory | unaudited |
| `scripts/forbid-tool-literals.sh` (CI step `.github/workflows/ci.yml:302-303`) | no distinctive literal outside the module | unaudited |

## Selection and the differential

| Check | Covers | Status |
| --- | --- | --- |
| the dedup selector tests in `selection.rs` naming `mcp_read` and `transform.rs` `duplicate_tool_full_drop_re_renders_byte_identically`, as Part A cited them at `a0e3f2993` | deleted by U4 with the selector (`97afdaff1`) | none at HEAD |
| `crates/daemon/tests/selection_differential.rs` | production selection equals the frozen reference over generated mixes; the reference reads the module through `daemon::tool_vocabulary_evaluation` (`selection_differential.rs:39`) instead of mirrored constants; `TOOLS` (`selection_differential.rs:1389-1415`) holds `mcp_read`, `mcp_grep`, `MCP_READ`, `Edit`, `TodoWrite` | unaudited |
| `selection_differential.rs:2248` `optimized_matches_frozen_reference_on_identical_calls` | identical calls under every tool name | unaudited |
| `sel.rs:1461` `selection_golden_matches_ts_selectors` with `crates/daemon/testdata/selection-golden.json` | edit supersession on `filePath`, `todowrite` keep-one, zero-value meta drops, against the checkout's own fixture; no CI step diffs the fixture against the PR base | unaudited |
| `sel.rs:2229` `edit_marker_region_hint_clamps_utf16_and_trusts_only_hint_sized_sentinels`, `sel.rs:2571` `drop_wins_over_edit_marker` | the edit-marker payload (`sel.rs:478-492`) clamps `diff_keys` values | unaudited |

## Summary line and chunk text

| Check | Covers | Status |
| --- | --- | --- |
| `history_summarizer_chunk.rs:3251` `repeated_call_ids_render_each_call_with_its_own_arguments` | `TC: read(one.rs)` from `filePath` | unaudited |
| `history_summarizer_chunk.rs:3312` `a_call_summary_holds_no_line_separator` | the description arm and a `filePath` key argument on separate inputs, with newlines collapsed | unaudited |
| `crates/daemon/src/lib.rs:19653` `history_summarizer_boundary_construction_matches_owned_reference` | the bare-name arm: `TC: bash` for an input holding only `command` (`lib.rs:19927-19932`, `:26292`) | unaudited |
| `crates/daemon/testdata/history_summarizer-chunk-golden.json` | `TC:` lines from short arguments | unaudited |

## Wire and injection

| Check | Covers | Status |
| --- | --- | --- |
| `crates/daemon/testdata/wire-golden.json` | the flatten step, including `path` inputs | unaudited |
| `wire.rs:1260` (inside a reduction test) | a reduced tool result keeps its output variant; no path | unaudited |
| `injection.rs:553` `constants_match_ts_golden`, `:1047`, `:1052`, and `crates/daemon/testdata/injection-golden.json` | the synthetic `todowrite` round trip under `TODO_TOOL` | unaudited |
| `crates/daemon/src/transform.rs:29651` `warm_cache_selection_bust_does_not_replay_collapsed_synthetic_todo_as_live` | the echoed synthetic pair beside a native `todowrite` call produces no duplicate tool-use id | unaudited |

## Capture adapters and outcome records

| Check | Covers | Status |
| --- | --- | --- |
| `hs-tests:445` `adapters_bind_native_identity_exactly_and_refuse_missing_identity` | 4 units from 7 parts; a Pi user `image` beside text yields one unit; a Pi `toolResult` with an `image` item is `UnsupportedShape("tool result item")`; an OpenCode record whose settled tool output is an object, beside text parts, is `UnsupportedShape("tool output")` (`hs-tests:681-686`) | unaudited |
| `hs-tests:782`, `:795`, `:866`, `:878`, `:1130` | refusal arms: Pi and OpenCode roles, harness namespaces, the first failing Pi field | unaudited |
| `hs-tests:922` `unfinished_assistant_text_is_deferred_but_settled_tools_publish` | the unfinished-assistant-text arm | unaudited |
| `hs-tests:2358`, `:2521` | a 65,537-byte unit is `UnitTooLarge`, siblings publish, no slot taken | unaudited |
| `hs-tests:2734` `capture_batch_refuses_a_secret_unit_without_failing_siblings`; `sc-tests:594` `a_secret_bearing_unit_is_terminal_and_its_siblings_publish` | the secret-bearing pre-screen arm | unaudited |
| `hs-tests:2856` `batch_refusal_names_the_failing_unit_and_only_a_conflict_is_terminal` | `conflicting_unit` names a unit only for `Conflict` | unaudited |
| `hs-tests:3619` `opencode_tool_outcomes_follow_the_pinned_trace` | the OpenCode 1.19.0 trace: string `exit`, absent metadata, `tool = "task"` yield `None` | unaudited |
| `sc-tests:224-227` | a failed `bash` call's record carries no exit code through the capture path | unaudited |
| `sc-tests:820` | a foreign-session record counts once in `adapter_refused` | unaudited |
| `sc-tests:1316`, `:1319` | three rows without `type` count 3 | unaudited |

## Corpus oracle (landed by U1 to U3)

| Check | Covers | Status |
| --- | --- | --- |
| `replay:80` `every_frozen_session_reconciles_with_no_unclassified_loss` | both frozen sessions reconcile with every unit admitted or under one named condition; the Pi image-bearing result is refused whole and classified `non_text_item` | unaudited |
| `replay:213` `ae1_replays_nineteen_units_and_one_classified_skip` | 19 units and an `image` part as `non_text_part` | unaudited |
| `replay:236` `a_mutated_adapter_fails_the_replay_naming_the_part` | the oracle shares no adapter code | unaudited |
| `replay:297` `an_oversized_unit_and_a_33_unit_batch_reconcile_to_named_losses` | `unit_too_large` and `deferred` | unaudited |
| `replay:336` `a_pending_tool_part_is_a_non_unit_and_loses_no_unit` | the `unsettled_tool` non-unit | unaudited |
| `replay:423` `admitted_plus_named_loss_equals_expected_for_any_order_split_and_size` (proptest) | the loss identity over creation order, batch splits, and sizes around 64 KiB | unaudited |
| `replay:443` `the_identity_case_fails_when_the_adapter_refuses_every_record` | an adapter that refuses every record fails the identity | unaudited |
| `replay:470` `a_seeded_dropped_unit_fails_the_identity_naming_the_unit` | a dropped unit is named | unaudited |
| `replay:585` `a_refused_outcome_record_is_unclassified_loss` | a permanent descriptor refusal is an unnamed, unclassified loss, not `identity_conflict` ([`loss-conditions.md`](loss-conditions.md#loss-condition-enumeration)) | unaudited |
| `replay:615` `a_changed_unit_under_a_published_identity_conflicts_alone` | `identity_conflict` names only the changed unit | unaudited |
| `crates/daemon/src/source_corpus_tests.rs` | the Rust readers and the closed enumerations | unaudited |

## Project documents and repository identity

| Check | Covers | Status |
| --- | --- | --- |
| `pd.rs:391`, `:417`, `:457`, `:471` | final-component symlink, a sparse file already past the cap and a FIFO, `Missing` versus `Unreadable`, oversize; no test grows a file during the read | unaudited |
| `pd.rs:543`, `:554` (proptests), `pd.rs:588` golden | canonical form, escape, rendered block, and hash | unaudited |
| `sc-tests:1964`, `:2144`, `:2209`, `:2283`, `:2322` | document lineage, retirement, renames, project isolation, unreadable reads, with rows selected by `relative_path` (`sc-tests:1903-1909`); none asserts the identity values capture writes | unaudited |
| `sc-tests:2743`, `:2784`, `:2818`, `:2867`, `:2942` | U7: the derived identity, stale-row retirement before publication, worktrees, a failed derivation, a shallow checkout; `document_identities` (`sc-tests:2728`) reads the `repository_id` capture writes | unaudited |
| `crates/daemon/src/repository_identity_tests.rs` | U7: the derivation against the plugin's rendering, two roots, a subdirectory, the fallbacks, the failures, the unchanged encoding | unaudited |
| symlinked intermediate directory | none found; unreachable at HEAD because no name holds a separator | none |

## Re-admission

| Check | Covers | Status |
| --- | --- | --- |
| `sc-tests:2521` `a_blocked_digest_is_refused_alone_and_its_sibling_publishes` | AE5 for a tombstoned and a pending-unlink digest, and the pre-screen digest equal to the stored artifact digest | unaudited |
| `sc-tests:2568` `a_purge_between_screen_and_commit_fails_one_pass_then_converges` | one `kernel_error` pass, then AE5 | unaudited |
| `sc-tests:2614` `a_purged_unit_with_a_stored_receipt_replays` | receipt replay precedes the pre-screen | unaudited |
| `sc-tests:2638` `a_reclaiming_digest_stays_pending_and_publishes_after_reclamation` | a reclaiming-only digest is never terminal | unaudited |
| `hs-tests:2555` `a_purged_digest_is_refused_before_the_batch_and_takes_no_admission_slot` | two refused units take no admission slot, and screening resumes after each | unaudited |
| `hs-tests:2607` `a_purged_revision_leaves_its_lineage_to_a_later_revision` | a refused revision leaves its lineage to a later revision in the same batch | unaudited |
| `hs-tests:2664` `an_evidence_receipt_conflict_takes_precedence_over_a_purged_digest` | an evidence-receipt request conflict is an identity conflict before and after the purge | unaudited |
| `crates/kernel/tests/cas_fault_injection.rs:589` `readmission_preview_names_tombstoned_and_pending_unlink_digests_and_writes_nothing` | the preview query's truth table and that it writes nothing | unaudited |

## Suspiciously quiet areas

- No test asserts that keep-one and the read-back act on the native
  `todowrite` call alone when the echoed synthetic pair is present; selection
  never sees the pair (`transform.rs:3469-3473`).
- No test asserts a reduced `ToolCall` input's `path` against the original
  input's `path_arg`.
- No harness-produced fixture holds a Pi `details.truncation.truncated` value.
- No capture-pass test counts an OpenCode record refused for a non-string
  tool output in `adapter_refused`; the adapter refusal itself is
  `hs-tests:681-686`.
- No test gives the summary line one input holding both a description and a
  key argument, and none pairs `pattern` with `filePath` at the display query.
- No CI step diffs `crates/daemon/testdata` against the PR base.
