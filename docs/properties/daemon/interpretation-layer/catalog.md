# Interpretation-layer property catalog

Records for the daemon's interpretation layer: the code that reads harness
records and tool calls and decides what a tool name, an argument key, or a
native field means. The record contract is [`../../METHOD.md`](../../METHOD.md);
Part B records add one `Falsifier or legal witness` line, as the parent
specification's Part B contract does.

## Scope

The layer is the seven caller sites the parent specification names
([#1105](https://github.com/ahrav/eidnara/issues/1105), D3 and R5), held in
nine files (the codecs are two files, and `tool_vocabulary.rs` holds the moved
tables): selection
(`crates/daemon/src/selection.rs`), chunk text (`chunk_text.rs`), wire
extraction (`wire.rs`), tool outcome (`tool_outcome.rs`), injection
(`injection.rs`), the summarizer chunk formatter
(`history_summarizer_chunk.rs`), and the OpenCode and Pi codecs
(`codec/opencode.rs`, `codec/pi.rs`), plus `tool_vocabulary.rs`, which U4
([#1107](https://github.com/ahrav/eidnara/issues/1107)) made the one home of
their tool-name and argument-key tables. The capture adapters
(`harness_sources.rs`), the capture job (`source_capture.rs`), and the project
document reader (`project_docs.rs`) appear where a record's guarantee runs
through them.

Abbreviations: `sel.rs` = `crates/daemon/src/selection.rs`; `tv.rs` =
`crates/daemon/src/tool_vocabulary.rs`; `tv-tests` =
`crates/daemon/src/tool_vocabulary_tests.rs`; `hs.rs` =
`crates/daemon/src/harness_sources.rs`; `hs-tests` =
`crates/daemon/tests/harness_sources.rs`; `sc.rs` =
`crates/daemon/src/source_capture.rs`; `sc-tests` =
`crates/daemon/src/source_capture_tests.rs`; `pd.rs` =
`crates/daemon/src/project_docs.rs`; `to.rs` =
`crates/daemon/src/tool_outcome.rs`; `replay` =
`crates/daemon/tests/source_corpus_replay.rs`; `#1104` = the
`daemon/source-capture` catalog on PR #1104's branch at `6336ebe48`, which is
not on this branch.

Provenance. Part A records (existing behavior) come from #1105 comment 1 and
Part B S2 records (proposed obligations) from comment 2, both authored at
`a0e3f2993`. Every record here was re-verified against the code at
`41d889ea4`, the tip of the #1109 branch this part's branch starts from; this
part changes only `docs/`, so the references hold at its head. Corrections to
the source's coordinates and claims are stated in each record's evidence
file. Records keep their source's `Check` semantics and confidence unless the
evidence file's investigation log says why they changed; a Part B record's
higher confidence rests on the U4 code and tests that implement it, which is
evidence, not proof.

## Reachability classes

- `default-production`: runs under `DaemonConfig::default()`. The transform
  path runs selection, wire flattening, and injection with
  `compaction_enabled: true` (`crates/daemon/src/config.rs:149`; selection is
  called from `crates/daemon/src/transform.rs:4133`, injection from
  `transform.rs:4067` and `:4451`). The m0 composer reads project documents with
  `inject_docs: true` (`config.rs:162`).
- `explicit-config-only`: capture runs only when `source_capture_enabled` is
  set (`config.rs:152` defaults it false; `sc.rs:285-296` gates on it). The
  summarizer `TC:` line runs only on the history-summarizer path, which needs a
  non-empty model chain: `model_chain` defaults empty (`config.rs:147`),
  `eidnara_folds` requires it (`config.rs:187-191`),
  `prepare_history_summarizer_with` returns before the trigger estimate when
  `eidnara_folds` is false (`crates/daemon/src/lib.rs:9814`), and the firing
  assembler returns `NoModels` (`history_summarizer_chunk.rs:860-864`).
- `test-only`: none.

## Index

| Record | Part | Type | Reachability | Check | Exercised | Status |
| --- | --- | --- | --- | --- | --- | --- |
| [il-dedup-selector-never-fires-on-harness-names](#il-dedup-selector-never-fires-on-harness-names) | A | safety | default-production | always(!X) | not yet | invalidated |
| [il-adapter-refusal-is-per-message](#il-adapter-refusal-is-per-message) | A | safety | explicit-config-only | always | partial | active |
| [il-unknown-part-kind-skipped-uncounted](#il-unknown-part-kind-skipped-uncounted) | A | safety | explicit-config-only | always | partial | active |
| [il-exit-code-only-for-opencode-bash](#il-exit-code-only-for-opencode-bash) | A | safety | explicit-config-only | always | yes | active |
| [il-truncated-flag-read-per-harness](#il-truncated-flag-read-per-harness) | A | safety | explicit-config-only | always | partial | active |
| [il-path-arg-key-order-first-match](#il-path-arg-key-order-first-match) | A | safety | default-production | always | yes | active |
| [il-repository-id-equals-project-id-at-head](#il-repository-id-equals-project-id-at-head) | A | safety | explicit-config-only | always | partial | invalidated |
| [il-tool-name-matching-differs-across-consumers](#il-tool-name-matching-differs-across-consumers) | A | safety | default-production | always | yes | active |
| [il-doc-read-nofollow-final-component-flat-list](#il-doc-read-nofollow-final-component-flat-list) | A | safety | default-production | always | partial | active |
| [il-reduced-tool-call-carries-extracted-path](#il-reduced-tool-call-carries-extracted-path) | A (queued) | safety | default-production | always | not yet | active |
| [il-synthetic-todowrite-round-trips-under-one-name](#il-synthetic-todowrite-round-trips-under-one-name) | A (queued) | safety | default-production | always | partial | active |
| [il-tool-summary-description-precedes-key-arg](#il-tool-summary-description-precedes-key-arg) | A (queued) | safety | explicit-config-only | always | partial | active |
| [il-readmission-block-ends-batch-unnamed-at-head](#il-readmission-block-ends-batch-unnamed-at-head) | A (queued) | safety | explicit-config-only | always | not yet | invalidated |
| [il-vocabulary-literals-live-in-one-module](#il-vocabulary-literals-live-in-one-module) | B S2 | safety | default-production | always | partial | active |
| [il-classify-total-with-unknown](#il-classify-total-with-unknown) | B S2 | safety | default-production | always | yes | active |
| [il-unknown-tool-never-dropped-deduped-or-edit-marked](#il-unknown-tool-never-dropped-deduped-or-edit-marked) | B S2 | safety | default-production | always(!X) | yes | active |
| [il-gate-fails-on-distinctive-literal-outside-module](#il-gate-fails-on-distinctive-literal-outside-module) | B S2 | safety | default-production | always | yes | active |
| [il-dedup-removal-leaves-goldens-byte-identical](#il-dedup-removal-leaves-goldens-byte-identical) | B S2 | safety | default-production | always | yes | active |
| [il-exit-code-only-for-named-tool-and-harness](#il-exit-code-only-for-named-tool-and-harness) | B S2 | safety | explicit-config-only | always | yes | active |
| [il-literal-classified-contract-or-vocabulary](#il-literal-classified-contract-or-vocabulary) | B S2 | safety | default-production | always | partial | active |
| [il-path-arg-harness-neutral](#il-path-arg-harness-neutral) | B S2 | safety | default-production | always | yes | active |
| [il-classify-one-case-rule-per-query](#il-classify-one-case-rule-per-query) | B S2 | safety | default-production | always | yes | active |

Semantics distribution: 20 `always`, 2 `always(!X)`. No liveness or
reachability record: the surface is pure functions over one record or one
input plus one persistence step. The coverage markers a campaign should reach
are listed in [`fault-map.md`](fault-map.md#coverage-checks-to-add).

## Records

### il-dedup-selector-never-fires-on-harness-names

Type: safety
Reachability: default-production - at `a0e3f2993` `select_tool_dedup` ran inside `select_reductions_with_outcome`, which `transform.rs` called on the compaction path; `compaction_enabled` defaults true (`config.rs:149`).
Status: invalidated - U4 deleted `DEDUP_SAFE_TOOLS`, `select_tool_dedup`, and `dedup_name` in `97afdaff1`; no selector remains at `41d889ea4` (`sel.rs:630-677` is the only per-name selector). Kept so its successor record has a named predecessor.
Exercised: not yet - the property has no code to exercise at HEAD; the successor [il-dedup-removal-leaves-goldens-byte-identical](#il-dedup-removal-leaves-goldens-byte-identical) carries the check that removal changed no output.
Guarantee: Over a transform window whose tool names are the names the pinned Pi and OpenCode harnesses send, the dedup selector returned the empty set, so no tool arc was dropped as a duplicate.
Check: `always(!X)` - over every frozen corpus session, no `ToolCall.name` is in the dedup list and the selection outcome holds no duplicate-drop reduction. `always(!X)` because the forbidden state had no dedicated detection point.
Fault/timing angle: none in code; the premise was a vocabulary fact outside the repository (a tool literally named `mcp_read`).
Required faults and enabling state: two completed calls of one tool with equal serialized arguments under one owner message, with names from a real harness session.
Confidence: high - [evidence](evidence/il-dedup-selector-never-fires-on-harness-names.md). The deletion is read from `git diff a91dbc2b6 97afdaff1 -- crates/daemon/src/selection.rs`; the frozen corpus census finds no `mcp_` name.
Existing check: `sel.rs:2828` `identical_calls_produce_no_dedup_decision_for_any_tool_name` (identical `read`, `mcp_read`, and `mcp_foo` calls produce no decision); unaudited.
Impact: none at HEAD. Before U4, a tool literally named `mcp_grep` would have lost its duplicate arcs.
Open questions: None.

### il-adapter-refusal-is-per-message

Type: safety
Reachability: explicit-config-only - `opencode_units_within` and `pi_units_within` have one production caller, `CaptureJob::units` (`sc.rs:1124-1154`), behind `consents` (`sc.rs:285-296`).
Status: active
Exercised: partial - `hs-tests:445` (Pi tool result with an `image` item refuses as `UnsupportedShape("tool result item")`), `:782`, `:795`, `:1130`; `sc-tests:820`, `:1316`, `:1319` count refused records; `replay:93` `every_frozen_session_reconciles_with_no_unclassified_loss` replays the frozen Pi session's image-bearing tool result, asserts the adapter refused it, and asserts the reader classifies it `non_text_item`. No test holds an OpenCode message whose text parts sit beside a settled tool part with a non-string output.
Guarantee: An adapter decides a whole native record at once: any refusal arm reached while reading a record returns `Err(AdapterRefusal)` and the record yields zero units, including units already built from earlier parts, and the pass counts that record once in `adapter_refused`.
Check: `always` - for every record the adapter returns `Ok(Some(units))`, `Ok(None)` (budget stop through `Kept::keep`, `hs.rs:321`, `:354`, `:435`, `:443`, `:454`), or `Err(_)` with no unit; the `?` arms at `hs.rs:313`, `:327`, `:340`, `:343`, `:351`, `:424`, `:431`, `:433`, `:452` return before `kept.units` is read, and `sc.rs:1146` counts each `Err` once, added at `:1149-1152`. `always` because each call is a pure function of one record.
Fault/timing angle: shape-driven. A budget stop (`Ok(None)`) discards that record's earlier units and ends the pass; refusals seen earlier in the stopped pass are still counted (`sc.rs:1140-1152`).
Required faults and enabling state: a Pi `toolResult` with a text item and a non-text item; an OpenCode message with a text part and a settled tool part whose `state.output` is not a string; an OpenCode text part whose `text` is not a string beside other text parts.
Confidence: high - [evidence](evidence/il-adapter-refusal-is-per-message.md). Code read in full at `41d889ea4`; both refusal shapes matched to tests.
Existing check: `hs-tests:445`, `:782`, `:795`, `:1130`; `sc-tests:820`, `:1316`; `replay:93`; all unaudited.
Impact: one unsupported item silences every text block of its message. The loss is counted, not attributed; the corpus reconciliation derives the lost-unit count from the independent reader ([`loss-conditions.md`](loss-conditions.md), `adapter_refused`). `sc-adapter-identity-native-or-refused` (#1104) covers the identity arms, not the per-message blast radius.
Open questions:
- Should a non-text item refuse the message or be skipped like a Pi user-message `image` item (`hs.rs:448` skips it)? The asymmetry is implemented behavior. (needs human input)

### il-unknown-part-kind-skipped-uncounted

Type: safety
Reachability: explicit-config-only - same caller and gate as the record above.
Status: active
Exercised: partial - `hs-tests:445` (4 units from 7 parts), `:922` (unfinished assistant text deferred); `replay:225` `ae1_replays_nineteen_units_and_one_classified_skip` (an `image` part as `non_text_part`), `replay:345` `a_pending_tool_part_is_a_non_unit_and_loses_no_unit`, and the frozen OpenCode session (`replay:93`, `non_text_part` present). The independent reader classifies every arm (`crates/daemon/src/source_corpus.rs:573-594`); no daemon counter observes the skips because none exists.
Guarantee: A part the adapter does not read is skipped without a unit, a refusal, or a counter: OpenCode part kinds other than `text` and `tool`, `text` parts marked `ignored`, assistant `text` parts of a turn without `time.completed`, `tool` parts whose `state.status` is neither `completed` nor `error`, interrupted tool parts of an incomplete turn, and Pi user or assistant content items whose `type` is not `text`.
Check: `always` - for every such part, the unit vector and `Count::AdapterRefused` are unchanged: arms at `hs.rs:307` (`ignored`), `:308` (unfinished assistant text), `:331` (`_ => continue` on status), `:334-336` (interrupted in an incomplete turn), `:357` (`_ => {}` on part type), `:448` (Pi item filter). `always` because the arms are unconditional on the matched shape.
Fault/timing angle: the `running`/`pending` and interrupted arms are time-dependent (the part becomes a unit on a later pass once the turn completes); the `ignored`, non-text, and unknown-kind arms are permanent for that part.
Required faults and enabling state: a window holding a part of each arm; for the time-dependent arms, a second pass after the turn completes.
Confidence: high - [evidence](evidence/il-unknown-part-kind-skipped-uncounted.md). Each arm read and cited at `41d889ea4`.
Existing check: `hs-tests:445`, `:922`; `replay:93`, `:225`, `:345`; unaudited. No counter exists to audit.
Impact: `adapter_refused` undercounts observed-but-uncaptured content. The corpus reconciliation reaches `unclassified_loss == 0` by classifying these from the independent reader, as U1 decided.
Open questions: None. The source's question (daemon counter or oracle-side classification) is resolved by U1 and U3: the oracle classifies them through the closed `NonUnitReason` enumeration.

### il-exit-code-only-for-opencode-bash

Type: safety
Reachability: explicit-config-only - `to.rs:21` `opencode` and `to.rs:59` `pi` have production callers only in the capture adapters (`hs.rs:350`, `:432`); the record enters a capture unit's request digest.
Status: active
Exercised: yes - `tv-tests:145` `exit_codes_come_only_from_opencode_bash_integers` runs every case of `crates/daemon/testdata/opencode-tool-outcome-trace.json` (OpenCode 1.19.0) and the string, float, `read`, and Pi cases; `tv-tests:17` `each_consumer_keeps_its_name_rule` holds `Bash` and every other spelling to `None`; `hs-tests:3524` `opencode_tool_outcomes_follow_the_pinned_trace`; `sc-tests:224-227` reads `exit_code` through the capture path.
Guarantee: `ToolOutcome.exit_code` is `Some(n)` only when the record is an OpenCode `tool` part whose `tool` equals `"bash"` exactly and whose `state.metadata.exit` is a JSON integer; every Pi record and every other OpenCode tool leaves it `None`.
Check: `always` - `to.rs:31` calls `tool_vocabulary::exit_code(Harness::OpenCode, tool_name, metadata)`, which matches `EXIT_CODE_TOOL` exactly and reads `exit` with `as_i64` (`tv.rs:200-207`, the name at `tv.rs:35`); `to.rs:72` sets `exit_code: None` for Pi. `always` because the functions are pure over one record.
Fault/timing angle: none.
Required faults and enabling state: a `bash` part with an integer `exit`; with a null, string, or float `exit`; a non-`bash` part carrying `exit`.
Confidence: high - [evidence](evidence/il-exit-code-only-for-opencode-bash.md). Code and tests read; the frozen OpenCode 1.18.22 session's four `bash` parts carry `exit` 0 three times and `exit: null` once.
Existing check: `tv-tests:17`, `:145`; `hs-tests:3524`; `sc-tests:224`; unaudited.
Impact: an exit code attributed to a tool whose `exit` means something else would enter the frozen request digest. The digest grammar's `[; exit=N]` (#1066 D9) is still not emitted (`to.rs:156-176`).
Open questions: None. The source asked whether OpenCode 1.18.22 writes `metadata.exit` for `bash`; the frozen session (`crates/daemon/testdata/corpus/sessions/opencode-1.18.22/mix.json`) shows it does.

### il-truncated-flag-read-per-harness

Type: safety
Reachability: explicit-config-only - same callers as the record above.
Status: active
Exercised: partial - `tv-tests:185` `truncation_flags_are_read_from_each_harness_field` reads both paths, a non-boolean value, and a missing flag on synthetic records; `hs-tests:3524` covers the OpenCode trace. No corpus session holds a Pi `details.truncation.truncated` value: none of the frozen Pi session's thirteen tool results (twelve captured; the image-bearing one is refused) has the flag.
Guarantee: `ToolOutcome.truncated` is read only as the boolean `state.metadata.truncated` on an OpenCode `tool` part and only as the boolean `details.truncation.truncated` on a Pi `toolResult`; any other shape leaves it unset.
Check: `always` - `to.rs:37` and `to.rs:71` call `tool_vocabulary::truncated_flag`, which reads the pointer `/metadata/truncated` under `state` or `/details/truncation/truncated` under the message with `as_bool` (`tv.rs:212-218`). `always` because the functions are pure over one record.
Fault/timing angle: none.
Required faults and enabling state: an OpenCode `bash` part with `truncated: true`; a Pi `bash` `toolResult` with `details.truncation.truncated: true`; a non-boolean value at either path.
Confidence: high - [evidence](evidence/il-truncated-flag-read-per-harness.md). Code read; the Pi side has no harness-produced fixture.
Existing check: `tv-tests:185`; `hs-tests:3524`; unaudited.
Impact: the corpus report stratifies byte lengths by this flag; for Pi every result is `truncated=unknown` until a session truncates.
Open questions:
- Which Pi 0.80.2 tool output makes the harness write `details.truncation`? The frozen 70,000-byte Pi result carries no flag. (unresolved, needs a Pi session whose `bash` or `read` output passes the harness's own limit)

### il-path-arg-key-order-first-match

Type: safety
Reachability: default-production - wire flattening (`wire.rs:540`, every `ToolCall` of every transform request) and edit supersession (`sel.rs:649`) run on the transform path; the third site, the summarizer `TC:` line (`chunk_text.rs:122`, through `history_summarizer_chunk.rs:1290`), is explicit-config-only.
Status: active
Exercised: yes - `tv-tests:66` `each_site_keeps_its_key_order` runs fourteen inputs, including two recognized keys, a non-string first key, `file_path` alone, and a 61-scalar value, through `path_arg` and `extract_key_arg`; `selection-golden.json` and the `TC:` goldens hold `filePath` inputs.
Guarantee: Each site returns the value of the first key in its ordered list whose value is a JSON string, skipping absent or non-string values: `PATH_KEYS` = `["filePath", "file_path", "path"]` for wire extraction and edit supersession (`tv.rs:41`), and `DISPLAY_PATH_KEYS` = `["filePath", "path", "pattern", "query"]` then `DISPLAY_SYMBOL_KEYS` = `["symbol", "module", "action"]` for the summary line (`tv.rs:43`, `:45`).
Check: `always` - `tv.rs:166-170` `first_string` returns the first `as_str` hit; `path_arg` (`tv.rs:174-176`) serves `wire.rs:646-648` and `sel.rs:649`; `display_arg` (`tv.rs:187-191`) serves `chunk_text.rs:122-127`, which cuts a path-like value to 60 scalar values plus `…` (`chunk_text.rs:129-137`). `always` because the functions are pure.
Fault/timing angle: none.
Required faults and enabling state: `{filePath: 1, file_path: "f", path: "p"}`; `{path: "p", filePath: "f"}`; `{file_path: "c"}` alone; `{pattern: "p", filePath: "f"}`.
Confidence: high - [evidence](evidence/il-path-arg-key-order-first-match.md). The lists and the U4 table read at `41d889ea4`.
Existing check: `tv-tests:66`, `:127`; `sel.rs:1461` `selection_golden_matches_ts_selectors`; `history_summarizer_chunk.rs:3251`, `:3312`; unaudited.
Impact: a tool using `file_path` gets edit supersession and a wire `path` but a bare `TC: name` summary. The #1072 key list differs from both lists (lead L8).
Open questions:
- Is the summary line's omission of `file_path` intentional (display) or an oversight? U4 preserved it and `tv-tests:66` pins it. (needs human input)

### il-repository-id-equals-project-id-at-head

Type: safety
Reachability: explicit-config-only - `plan_document` (`sc.rs:1036`) runs inside `capture_documents` (`sc.rs:885`) under consent with `inject_docs` set.
Status: invalidated - U7 ([#1112](https://github.com/ahrav/eidnara/issues/1112)) derives `repository_id` from the repository (`crates/daemon/src/repository_identity.rs:62`, used at `sc.rs:1058`); the project digest remains the value only as the fallback. Kept so that change has a named predecessor.
Exercised: partial - the document tests at `sc-tests:1964`, `:2137`, `:2202`, and `:2276` assert identity fields one by one; no test asserts the equality as such, and none holds two checkouts of one repository.
Guarantee: Before U7, every `RepositoryDocuments` unit the capture path built carried the identity `[("project_id", P), ("repository_id", P), ("relative_path", name)]` with `P` the bound project digest and `name` a `PROJECT_DOC_FILES` entry, and the live-row lookup, the unchanged check, the head read, and retirement all key on that tuple.
Check: `always` - before U7 the identity vector at `sc.rs:1056-1060` held `project_id` twice; it now holds the derived value; `sc.rs:1061` `document_lineage(&identity)`; `sc.rs:1063-1065` filters the live row to `CAPTURE_DOMAIN_ID`; `sc.rs:940` retires by the live row's `object_id` through `retire_document` (`sc.rs:1091`). `always` because the construction is unconditional.
Fault/timing angle: none at HEAD. Once `repository_id` is derived, the value depends on repository history, which is why U7 adds a retire-by-`relative_path` step.
Required faults and enabling state: two worktrees or clones holding the same `ARCHITECTURE.md`; a legacy row keyed `(P, P, path)` present when the first derived-identity pass runs.
Confidence: high - [evidence](evidence/il-repository-id-equals-project-id-at-head.md). Code read at `41d889ea4` and re-read after U7.
Existing check: the U7 successors `sc-tests:2739` (AE2 and restart convergence), `sc-tests:2780` (a crash between retire and publish), `sc-tests:2814` (AE3, two worktrees), `sc-tests:2863` (a failed derivation), `sc-tests:2938` (a shallow checkout), and `crates/daemon/src/repository_identity_tests.rs`; `crates/kernel/tests/eval_identity.rs` pins the encoding, not the value; unaudited.
Impact: worktrees and clones split every document lineage. After U7 the obligations are the `ri-` records of #1105 Part B S3.
Open questions:
- A history walk that exceeds its cap on one pass and not the next would flip the derived identity. U7 memoizes the past-the-cap fallback for the incarnation, so a flip waits for a restart; the cap is `MAX_ROOT_WALK_COMMITS` (2^18, OQ1).

### il-tool-name-matching-differs-across-consumers

Type: safety
Reachability: default-production - selection's `classify` (`sel.rs:645`) and tier lookup (`sel.rs:749-755`) and the injection read-back (`injection.rs:435`) run on the transform path; `exit_code` (`to.rs:31`) is capture-only.
Status: active
Exercised: yes - `tv-tests:17` `each_consumer_keeps_its_name_rule` pins `Edit`, `mcp_edit`, `MCP_TodoWrite`, `Bash`, `MCP_READ`, `mcp_mcp_edit`, and every tabled name at each consumer; `tv-tests:257` holds every tabled name under any ASCII case and one `mcp_` prefix.
Guarantee: Three name rules apply: selection evaluates the name lowercased with one leading `mcp_` removed (`tv.rs:78-84` `normalize`, applied in `classify` at `tv.rs:107` and `tier` at `tv.rs:137`); the injection read-back compares the raw name ignoring ASCII case (`tv.rs:156-158`); `exit_code` compares the raw name exactly (`tv.rs:202`). The summary formatter shows the raw name and matches nothing (`history_summarizer_chunk.rs:1290-1302`).
Check: `always` - for a name `N`, selection's edit, `todowrite`, `eidnara_*`, and zero-value rules evaluate `classify(N)`; injection evaluates `N.eq_ignore_ascii_case("todowrite")`; `exit_code` evaluates `N == "bash"`. Each is a deterministic function of `N`. `always` because the rules are static.
Fault/timing angle: none.
Required faults and enabling state: names `Edit`, `mcp_edit`, `MCP_TodoWrite`, `Bash`: `Edit` and `mcp_edit` are edit tools for supersession; `MCP_TodoWrite` is `todowrite` for selection's keep-one but not for the injection read-back; `Bash` yields no `exit_code`.
Confidence: high - [evidence](evidence/il-tool-name-matching-differs-across-consumers.md). Each comparison read; the U4 table asserts each consequence.
Existing check: `tv-tests:17`, `:215`, `:257`; unaudited.
Impact: one rule for every consumer would change supersession or keep-one for mixed-case or prefixed names. Selection's tier lookup normalizes twice (`tv.rs:150-152`), so `mcp_mcp_edit` classifies `Unknown` but takes tier 2; U4 preserved that and documents it in `docs/compression-fidelity.md`.
Open questions: None. The source asked which rule `classify` applies; U4 keeps the per-caller rules as module data (`tv.rs:57-73` `NameRule`).

### il-doc-read-nofollow-final-component-flat-list

Type: safety
Reachability: default-production - `read_project_docs_canonical` (`pd.rs:174`) runs on every m0 composition when `inject_docs` holds (default true, `config.rs:162`), independent of capture.
Status: active - U8 ([#1113](https://github.com/ahrav/eidnara/issues/1113)) revises it by introducing configured nested names and an `openat2` beneath-root resolver.
Exercised: partial - `pd.rs:244` (final-component symlink), `:267` (growth under the read), `:304` (`Missing` versus `Unreadable`), `:318` (oversize); `sc-tests:2315` `unreadable_documents_are_counted_and_retire_nothing`. No test opens a document through a symlinked parent because no name has a parent below the root.
Guarantee: Each document is opened at `project_root.join(name)` for the fixed names with `O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, validated as a regular file on the opened descriptor, read to at most 256 KiB, and decoded as UTF-8; the names hold no separator, so the only resolvable symlink on the path is the project root itself.
Check: `always` - `pd.rs:9` list; `pd.rs:178` `dir.join(filename)`; `config.rs:703-719` open flags and `is_file` on the descriptor; `pd.rs:113-133` classification. `always` because the read is unconditional per name.
Fault/timing angle: a parent-directory swap before the open is outside `NOFOLLOW`, and at HEAD the only parent is the root. `read_document` classifies a `NotFound` open as `Missing` when `std::fs::metadata(parent)` is a directory (`pd.rs:117-122`), and `metadata` follows symlinks; under nested names a missing target under a symlinked parent would classify `Missing` and `plan_document` would retire the live row (`sc.rs:1066-1072`). U8's resolver must cover the classification path as well as the open.
Required faults and enabling state: a symlinked final component (covered); a symlinked intermediate directory with and without a target file (unreachable at HEAD; U8 scenarios); a kernel without `openat2`.
Confidence: high - [evidence](evidence/il-doc-read-nofollow-final-component-flat-list.md). Code read; flags verified at `41d889ea4`.
Existing check: `pd.rs:244`, `:267`, `:304`, `:318`, `:387`, `:398`, `:429`; `sc-tests:2315`; unaudited.
Impact: without U8's resolver, nested names would let a repository-controlled symlink route the daemon's read and capture outside the root. The hash input stays `file:{name}\n{canonical}` (`pd.rs:208-211`), so the default list hashes identically.
Open questions: None (the classification-path finding is carried by OQ15 and U8).

### il-reduced-tool-call-carries-extracted-path

Type: safety
Reachability: default-production - `flatten_block` (`wire.rs:520`) stores `extract_file_path(input)` for every `ToolCall` (`wire.rs:540`, field `wire.rs:568`); the transform renders a reduced call from that field (`transform.rs:11890-11896`) through `reduced_block` (`wire.rs:445`).
Status: active
Exercised: not yet - no test asserts a reduced `ToolCall` input's `path` against `extract_file_path` of the original; `wire.rs:1260` reduces a result with no path.
Guarantee: The `path` key of every reduced `ToolCall` input equals `path_arg` of the original input, so the path key list is byte-visible compaction output.
Check: `always` - for every reduced call in a transform response, `input.path == path_arg(original.input)` when `path_arg` is `Some`, and the input has no `path` key otherwise (`wire.rs:484-489`). `always` because the rendering is unconditional.
Fault/timing angle: none.
Required faults and enabling state: an original input under each key of `PATH_KEYS` and under none of them.
Confidence: high - [evidence](evidence/il-reduced-tool-call-carries-extracted-path.md). Traced end to end at `41d889ea4`; the source recorded medium because the trace was pending.
Existing check: `wire-golden.json` (the flatten step only); unaudited.
Impact: changing `PATH_KEYS` changes compaction output; the golden gate catches only inputs the goldens hold.
Open questions: None.

### il-synthetic-todowrite-round-trips-under-one-name

Type: safety
Reachability: default-production - injection emits the synthetic call and result (`injection.rs:155-180`) and reads to-do state back (`injection.rs:429-457`) on the transform path (`transform.rs:4067`, `:4451`).
Status: active
Exercised: partial - `tv-tests:17` asserts `classify("todowrite") == TodoWrite` and `is_injected_todo_tool("todowrite")`; `injection.rs:553`, `:1047`, `:1052` assert the synthetic constants and names. No test runs selection's keep-one rule over a window holding the synthetic call beside a native call.
Guarantee: Injection emits a synthetic call and result named `TODO_TOOL` (`injection.rs:158`, `:173`), reads to-do state back from calls `is_injected_todo_tool` accepts (`injection.rs:435`), and selection's keep-one rule counts calls `classify` names `TodoWrite` (`sel.rs:660-663`, `:693`); all three resolve `TODO_TOOL` (`tv.rs:26`).
Check: `always` - `classify(TODO_TOOL) == ToolClass::TodoWrite` and `is_injected_todo_tool(TODO_TOOL)`. `always` because the constant is static and both queries are pure.
Fault/timing angle: none.
Required faults and enabling state: a window holding one synthetic and one native `todowrite` call.
Confidence: high - [evidence](evidence/il-synthetic-todowrite-round-trips-under-one-name.md). The three uses read at `41d889ea4`; U4 replaced three spellings with one constant.
Existing check: `tv-tests:17`; `injection.rs:553`, `:1047`, `:1052`; `injection-golden.json`; unaudited.
Impact: if the names diverged, selection would drop the synthetic call as an older `todowrite` or the read-back would miss it.
Open questions: None.

### il-tool-summary-description-precedes-key-arg

Type: safety
Reachability: explicit-config-only - the summarizer path (see Reachability classes) calls `format_tool_summary` directly at `history_summarizer_chunk.rs:1254` and `crates/daemon/src/boundary.rs:1860`, and through `call_summary` (`to.rs:251`, `:270`) at `history_summarizer_chunk.rs:1259` and `boundary.rs:1867`.
Status: active
Exercised: partial - `history_summarizer_chunk.rs:3312` `a_call_summary_holds_no_line_separator` covers the description arm and the key-argument arm; `tv-tests:66` covers the 60-scalar cut and the marker; the `TC:` goldens hold short arguments. No test asserts the bare-name arm.
Guarantee: A non-empty string `description` renders `TC: {description}`; otherwise `extract_key_arg` supplies the argument, cut to 60 scalar values plus `…` for a path-like key, and renders `TC: {name}({key_arg})`; otherwise `TC: {name}`; the result passes through `normalize_text`.
Check: `always` - `history_summarizer_chunk.rs:1290-1302`. `always` because the function is pure.
Fault/timing angle: none.
Required faults and enabling state: inputs with a description and a path; with a 61-scalar path; with neither.
Confidence: high - [evidence](evidence/il-tool-summary-description-precedes-key-arg.md). The source stated the description arm as `name(description)`; the code renders the description alone, and this record states the code.
Existing check: `history_summarizer_chunk.rs:3251`, `:3312`; `tv-tests:66`; the chunk golden; unaudited.
Impact: `display_arg` must reproduce the precedence and the marker byte for byte, or the chunk golden changes.
Open questions: None.

### il-readmission-block-ends-batch-unnamed-at-head

Type: safety
Reachability: explicit-config-only - batch ingest under consent; the block arises only after an operator purge.
Status: invalidated - U6 ([#1111](https://github.com/ahrav/eidnara/issues/1111)) refuses a blocked unit alone in the screening preview (`hs.rs:1594-1599`). Kept so the `sc-readmit-` obligations have a named predecessor.
Exercised: not yet - the behavior no longer exists for a block present at the screening preview; the path it describes remains for a purge that commits after that preview, which `sc-tests:2561` exercises.
Guarantee: Before U6, a blocked digest found during batch ingest returns `KernelError::Conflict` naming no unit (`crates/kernel/src/cas/ingest.rs:822-827`, the batch contract at `:693`); `BatchRefusal::conflicting_unit` returns `None` (`hs.rs:1483-1485`), the pass counts `kernel_error` (`sc.rs:1475-1480`), and `publish_locked` records every keyed unit of the batch `Pending` (`sc.rs:845-870`), so the batch is retried whole on each later pass while the block holds.
Check: `always` - a batch holding one tombstoned or pending-unlink digest ends `kernel_error` with every keyed unit pending and nothing retained. `always` because the Kernel check runs on every new reference.
Fault/timing angle: a purge committed before the pass; the block outlives the pending unlink because every pending unlink is written with a tombstone.
Required faults and enabling state: an operator purge of a captured digest, then identical bytes under a new identity beside a sibling.
Confidence: high - [evidence](evidence/il-readmission-block-ends-batch-unnamed-at-head.md). Traced at `41d889ea4`, whose pre-screen held no re-admission check, and re-read after U6.
Existing check: the U6 successors `sc-tests:2514` (AE5, tombstone and pending unlink), `sc-tests:2561` (a purge between the screening preview and the commit), `sc-tests:2607` (receipt replay precedence), `sc-tests:2631` (a reclaiming digest stays pending), `hs-tests:2555` (no admission slot), and `crates/kernel/tests/cas_fault_injection.rs:589` (the preview query); unaudited.
Impact: before U6, one purged digest stalled every unit of its batch on every pass. U6 replaces this with a terminal per-unit refusal; the #1104 document rows "Pending unlink (OQ20)" and "Tombstone after purge (OQ21)" become stale then (U10 handoff).
Open questions: None.

### il-vocabulary-literals-live-in-one-module

Type: safety
Reachability: default-production - selection, chunking, wire extraction, and injection run on every transform.
Status: active
Exercised: partial - the gate (`scripts/forbid-tool-literals.sh`, CI step `.github/workflows/ci.yml:302-303`) holds every distinctive vocabulary literal in `tv.rs`; [`literals.md`](literals.md) classifies every literal. Generic vocabulary literals remain in two callers.
Guarantee: Every vocabulary literal (tool names, argument keys, tool-authored metadata keys such as `state.metadata.exit`, the tier tables, the `eidnara_note` zero-value actions, the `mcp_` strip and lowercasing, and the per-caller name rule) is defined in `tv.rs`, and the interpretation-layer callers obtain them through typed queries only.
Check: `always` - a static property: the gate finds no distinctive literal outside the module, and the classification table lists no vocabulary row outside `tv.rs`. Evaluated by the gate on every CI run and by the table at authoring.
Fault/timing angle: none.
Required faults and enabling state: a new literal introduced in a caller file.
Confidence: high - [evidence](evidence/il-vocabulary-literals-live-in-one-module.md). The table finds the residue below; the gate passes at `41d889ea4`.
Existing check: `scripts/forbid-tool-literals.sh` and `scripts/forbid-tool-literals.test.ts`; unaudited.
Impact: a harness renaming a to-do field or the `description` key is fixed outside the module, and the #1072 resolver could copy a list a second time.
Open questions:
- Contract-versus-code: `injection.rs:21-24`, `:407-409`, and `:465` read the `todowrite` input schema (`todos`, `content`, `status`, `priority`, `completed`, `cancelled`, `medium`), and `history_summarizer_chunk.rs:1292` reads `description`. Each is vocabulary by R5 and lives outside the module; the gate misses them because they are generic words. Move them behind module queries, or record them as accepted residue? (needs human input: daemon owner)
Falsifier or legal witness: Falsifier: a distinctive literal such as `"filePath"` in `sel.rs` after U4. Legal witness: `sel.rs:649` reads the path through `tool_vocabulary::path_arg` and the gate is green.

### il-classify-total-with-unknown

Type: safety
Reachability: default-production - `classify` runs for every arc in selection (`sel.rs:610`, `:645`, `:693`).
Status: active
Exercised: yes - `tv-tests:248` `classify_is_total_and_deterministic` (proptest over arbitrary strings); `tv-tests:232` `edge_names_classify_as_unknown` (empty, `mcp_foo`, a 10 KiB name, a NUL, a non-ASCII prefix); `tv-tests:133` `an_unknown_tool_takes_every_default`.
Guarantee: `classify(name)` is total over every string, returns exactly one `ToolClass`, and returns `Unknown` for every name whose normalized form no table lists; it never fails and never panics on empty, non-ASCII, or very long names.
Check: `always` - `tv.rs:106-124` is a chain of table lookups ending in `Unknown`, with no indexing and no fallible call. A pure function, so evaluated per call.
Fault/timing angle: none.
Required faults and enabling state: names `""`, `"mcp_foo"`, `"MCP_READ"`, a 10 KiB name, a name with a NUL.
Confidence: high - [evidence](evidence/il-classify-total-with-unknown.md). Code and tests read at `41d889ea4`.
Existing check: `tv-tests:133`, `:232`, `:248`; unaudited.
Impact: a panicking lookup would break the transform for a harness with a new tool.
Open questions: None. `classify` applies `normalize` (`tv.rs:107`), so `mcp_read` classifies as `read`.
Falsifier or legal witness: Falsifier: `classify("")` panics. Legal witness: `classify("mcp_foo") == Unknown` and `classify("read") == Tabled`.

### il-unknown-tool-never-dropped-deduped-or-edit-marked

Type: safety
Reachability: default-production - `select_supersession` (`sel.rs:630`) and the tier lookup (`sel.rs:749-755`) run for every arc on the transform path (`transform.rs:4133`).
Status: active
Exercised: yes - `sel.rs:2828` `identical_calls_produce_no_dedup_decision_for_any_tool_name` runs two identical `mcp_foo` calls carrying `filePath` and `oldString` and asserts no decision, beside an `edit` positive control; `tv-tests:133` `an_unknown_tool_takes_every_default`.
Guarantee: For an `Unknown` tool, selection never drops it as a zero-value meta tool, never deduplicates it, and never compresses it into an `edit_marker` by a class decision; its tier is 3, its display argument comes from the generic key list, and `exit_code` is `None`.
Check: `always(!X)` - over a selection input holding an `Unknown` arc, the output holds no class-driven decision for it: `sel.rs:645-658` marks only `ToolClass::Edit`, `sel.rs:669` maps `Unknown` to no drop, `tv.rs:143` gives tier 3, and `tv.rs:202-205` gives no exit code. `always(!X)` because the forbidden drop, dedup, and mark have no dedicated code point.
Fault/timing angle: the emergency tiered drop (`sel.rs:789`) may drop tier-3 arcs under budget pressure; the guarantee covers class decisions only.
Required faults and enabling state: a tool named `mcp_foo` with `{filePath, oldString}` input and `metadata.exit: 1`.
Confidence: high - [evidence](evidence/il-unknown-tool-never-dropped-deduped-or-edit-marked.md). Code and tests read at `41d889ea4`.
Existing check: `sel.rs:2828`; `tv-tests:133`; unaudited.
Impact: a harness's custom or MCP tool would be removed from context or summarized as an edit.
Open questions:
- R7 "never dropped" versus the budget-driven tiered drop: the plan does not separate class decisions from budget decisions. (needs human input; this record assumes class decisions only)
Falsifier or legal witness: Falsifier: `mcp_foo` with an `oldString` key is rendered as an `edit_marker`. Legal witness: two identical `mcp_foo` calls survive selection unchanged.

### il-gate-fails-on-distinctive-literal-outside-module

Type: safety
Reachability: default-production - the gate runs in CI on every pull request (`.github/workflows/ci.yml:302-303`).
Status: active
Exercised: yes - `scripts/forbid-tool-literals.test.ts` (run by `bun test ./scripts` in `bun run check:repo`) asserts exit 0 on a clean tree, exit 1 naming file and line for a production literal, an `mcp_` name, and a nested production module, exit 0 for the module, `*_tests.rs`, `tests/`, `testdata/`, and a top-level test module, and exit 2 outside a git checkout.
Guarantee: A distinctive literal from the gated set (`filePath`, `file_path`, `oldString`, `newString`, `old_string`, `new_string`, `bash_status`, `bash_kill`, `todowrite`, `apply_patch`, `aft_outline`, `aft_zoom`, `aft_search`, or an `mcp_`-prefixed string) in `crates/daemon/src/**/*.rs` outside `tool_vocabulary.rs`, `*_tests.rs`, `tests/`, `testdata/`, and top-level `#[cfg(test)]` module regions makes the script exit 1 naming file and line; a tool failure exits 2; a clean tree exits 0.
Check: `always` - three-way exit contract per run: the pattern at `scripts/forbid-tool-literals.sh:16`, the path filter at `:32-38`, the region-skipping scan at `:42-50`, exit 2 through `report_tool_failure` at `:19-22`, and exit 1 at `:96-99`, following `scripts/forbid-comment-markers.sh`.
Fault/timing angle: none.
Required faults and enabling state: a `"filePath"` literal added to `boundary.rs`; the same literal in `boundary_tests.rs`.
Confidence: high - [evidence](evidence/il-gate-fails-on-distinctive-literal-outside-module.md). Script and tests read; the gated set equals the distinctive vocabulary subset in [`literals.md`](literals.md).
Existing check: `scripts/forbid-tool-literals.test.ts`; unaudited.
Impact: without the gate the single-module invariant decays silently.
Open questions: None. The source asked whether `eidnara_reduce` and `eidnara_note` are gated and how inline test modules are treated; U4 classifies the facade names as Eidnara-owned and leaves them ungated, and skips top-level `#[cfg(test)]` module regions.
Falsifier or legal witness: Falsifier: the gate exits 1 on the post-U4 tree because `prompt_surface.rs:67` defines `eidnara_note`. Legal witness: a `"filePath"` literal in `boundary.rs` exits 1 with `crates/daemon/src/boundary.rs:<line>`; in `boundary_tests.rs` it exits 0.

### il-dedup-removal-leaves-goldens-byte-identical

Type: safety
Reachability: default-production - the goldens pin the output of selection, chunking, and compression on the transform path (`transform.rs:4133`); the check itself runs once per PR in CI.
Status: active
Exercised: yes - `git diff a91dbc2b6 97afdaff1 -- crates/daemon/testdata` is empty, so U4 changed no golden; `crates/daemon/tests/selection_differential.rs:2248` `optimized_matches_frozen_reference_on_identical_calls` and the rest of the differential pass against a reference without dedup.
Guarantee: Removing `DEDUP_SAFE_TOOLS`, `select_tool_dedup`, `dedup_name`, and the mirrored differential constants changed no byte of `selection-golden.json`, `differential-golden.json`, `history_summarizer-chunk-golden.json`, or `compression-fidelity.json`, because no tool either harness sends was in the dedup list.
Check: `always` - golden files byte-identical against the PR base; the differential proptest passes. Evaluated once per PR by CI.
Fault/timing angle: none.
Required faults and enabling state: two identical `read` calls in one selection input (no dedup decision before or after).
Confidence: high - [evidence](evidence/il-dedup-removal-leaves-goldens-byte-identical.md). The empty testdata diff and the deleted code read from `97afdaff1`.
Existing check: `crates/daemon/tests/selection_differential.rs`; `sel.rs:2828`; unaudited.
Impact: a behavior change hidden inside a move-verbatim PR.
Open questions: None. At `a0e3f2993` the dedup comparison read the raw name, so `mcp_read` never normalized to `read` there.
Falsifier or legal witness: Falsifier: `selection-golden.json` differs after U4 by a removed dedup record. Legal witness: byte-identical goldens and a green differential.

### il-exit-code-only-for-named-tool-and-harness

Type: safety
Reachability: explicit-config-only - `exit_code` has one production caller, `to.rs:31`, reached only from the capture adapter (`hs.rs:350`). The source labelled this record default-production; the callers at `41d889ea4` say otherwise.
Status: active
Exercised: yes - `tv-tests:145`, `tv-tests:17`, `tv-tests:133`.
Guarantee: `exit_code(harness, name, metadata)` returns `Some` only when the module's table marks the tool as carrying an exit code (`bash`, `tv.rs:35`), the harness is OpenCode, and the integer `state.metadata.exit` is present; every other tool, `Unknown` included, returns `None`; a Pi record always returns `None`.
Check: `always` - `tv.rs:200-207`, a pure function evaluated per record over the trace fixture `crates/daemon/testdata/opencode-tool-outcome-trace.json`.
Fault/timing angle: none.
Required faults and enabling state: a `read` result whose metadata carries `exit: 0`; a `bash` result with `exit: null`.
Confidence: high - [evidence](evidence/il-exit-code-only-for-named-tool-and-harness.md). Code and tests read.
Existing check: `tv-tests:17`, `:133`, `:145`; unaudited.
Impact: an invented exit code in the outcome record would change the request digest (`source-request.v2` binds the outcome record, `hs.rs:600`), turning stored receipts into `IdentityReused` conflicts.
Open questions: None.
Falsifier or legal witness: Falsifier: `exit_code(OpenCode, "mcp_foo", {exit: 2}) == Some(2)`. Legal witness: `exit_code(OpenCode, "bash", {exit: 3}) == Some(3)` and `exit_code(OpenCode, "read", {exit: 0}) == None`.

### il-literal-classified-contract-or-vocabulary

Type: safety
Reachability: default-production - the classification governs every literal of the interpretation layer.
Status: active
Exercised: partial - [`literals.md`](literals.md) lands the table (256 distinct literals, 167 rows covering every field, tool, and argument-key literal, 100 excluded literals with reasons), checked once at authoring; no recurring check re-runs the classification.
Guarantee: Every string literal in the interpretation-layer files that names a field, a tool, or an argument key carries exactly one recorded class per site: contract (a harness with zero tools registered still emits it), vocabulary (a tool author chose it), or Eidnara-owned (Eidnara defines it); vocabulary literals live in the module, contract literals stay with their codec or adapter, and Eidnara-owned identifiers are defined once and referenced.
Check: `always` - the table lists every in-scope occurrence in exactly one row, and the gate's distinctive set equals the vocabulary rows' distinctive subset. `always` because the table is a static artifact checked at authoring.
Fault/timing angle: none.
Required faults and enabling state: none; static.
Confidence: medium - [evidence](evidence/il-literal-classified-contract-or-vocabulary.md). The extraction, the one-row-per-occurrence check, and the gate-set equality are mechanical; assigning a class to a generic word is a judgement the table records.
Existing check: none beyond the table; unaudited.
Impact: an unclassified literal is moved into or out of the module by habit, and the gate's set drifts from the classification.
Open questions:
- These Eidnara-owned spellings are not defined once: `synthetic_todo_` (`injection.rs:20`, `codec/opencode.rs:1074`) and the `eidnara_note` actions `read` and `dismiss` (`tv.rs:37`, `lib.rs:12854`, `:13171`, `:18609`, `crates/daemon/src/conditional_note_evaluation.rs:1905`). Define each once and reference it, or accept the spellings? (needs human input: daemon owner)
Falsifier or legal witness: Falsifier: `state.metadata.exit` recorded as contract. Legal witness: `info` contract, `filePath` vocabulary, `eidnara_note` Eidnara-owned.

### il-path-arg-harness-neutral

Type: safety
Reachability: default-production - `path_arg` serves selection and wire extraction on every transform; `display_arg` serves the summarizer line.
Status: active
Exercised: yes - `tv-tests:127` `pi_and_opencode_path_keys_name_one_path` (Pi `path` and OpenCode `filePath` yield the same value); `tv-tests:66` pins the four source inputs at both queries; the goldens are unchanged by U4.
Guarantee: `path_arg(input)` returns the same value for the Pi and OpenCode spellings of a path argument, following one ordered key list owned by the module, and `display_arg(input)` reproduces the summarizer's precedence byte for byte, so the selection, wire, and chunk goldens stayed identical when the three lists collapsed into the module.
Check: `always` - the per-site table of [il-path-arg-key-order-first-match](#il-path-arg-key-order-first-match) reproduces through `path_arg` and `extract_key_arg`, and the goldens are byte-identical against the U4 base. `always` because the queries are pure.
Fault/timing angle: none.
Required faults and enabling state: `{filePath: 1, file_path: "f", path: "p"}`, `{path: "p", filePath: "f"}`, `{file_path: "c"}`, `{pattern: "p", filePath: "f"}` at each query.
Confidence: high - [evidence](evidence/il-path-arg-harness-neutral.md). Code, tests, and the empty testdata diff read.
Existing check: `tv-tests:66`, `:127`; `selection-golden.json`, `wire-golden.json`, `history_summarizer-chunk-golden.json`; unaudited.
Impact: a harness-specific key spelling would get supersession but no summary, or the reverse.
Open questions: None. U4 kept the display list without `file_path` (today's behavior), and `tv-tests:66` pins it.
Falsifier or legal witness: Falsifier: `path_arg({filePath: "x"})` differs from `path_arg({path: "x"})`. Legal witness: both spellings yield `Some("x")` and the chunk golden is unchanged.

### il-classify-one-case-rule-per-query

Type: safety
Reachability: default-production - the selection queries (`sel.rs:645`, `:750`) and the injection read-back (`injection.rs:435`) run on the transform path; the `exit_code` rule is capture-only.
Status: active
Exercised: yes - `tv-tests:17` (the per-consumer table), `tv-tests:215` `every_tabled_name_reaches_a_non_unknown_answer`, `tv-tests:257` `tabled_names_classify_under_any_ascii_case_and_one_mcp_prefix`.
Guarantee: Each module query applies one declared name rule, and the rules reproduce HEAD at every caller: selection queries (`classify`, `tier`, `selection_tier`) evaluate the name lowercased with a leading `mcp_` removed; the injection read-back evaluates the raw name ignoring ASCII case; `exit_code` evaluates the raw name exactly; the `mcp_` strip is a shipped-default rule whose collision with `edit` or `write` for an MCP tool named `edit` is a known condition, not a guarantee.
Check: `always` - the per-consumer table reproduces through the module, and every tabled name is classified non-`Unknown` under its rule. `always` because the rules are static.
Fault/timing angle: none.
Required faults and enabling state: the names `Edit`, `mcp_edit`, `MCP_TodoWrite`, `Bash` at each consumer; one tabled name per query.
Confidence: high - [evidence](evidence/il-classify-one-case-rule-per-query.md). Code and tests read at `41d889ea4`.
Existing check: `tv-tests:17`, `:215`, `:257`; the differential's `TodoWrite`, `Edit`, and `MCP_READ` probes (`crates/daemon/tests/selection_differential.rs:1404-1406`); unaudited.
Impact: one rule for every consumer would change supersession or keep-one for mixed-case or prefixed names.
Open questions: None. OQ20 is resolved by U4: the per-caller rules are module data (`tv.rs:57-73`).
Falsifier or legal witness: Falsifier: after U4, `MCP_TodoWrite` is counted by selection's keep-one but no longer by the read-back's rule, or the reverse changes. Legal witness: the per-consumer table is unchanged.

## Relationship map

- il-adapter-refusal-is-per-message and il-unknown-part-kind-skipped-uncounted
  share `CaptureJob::units` with `sc-adapter-identity-native-or-refused` and
  `sc-unfinished-assistant-text-deferred` (#1104); the dominance is suspected on
  the "no partial vector" half only. Both feed the `adapter_refused` condition
  and the non-unit reasons in [`loss-conditions.md`](loss-conditions.md).
- il-exit-code-only-for-opencode-bash, il-exit-code-only-for-named-tool-and-harness,
  and il-truncated-flag-read-per-harness feed the request digest that
  `sc-receipt-replays-equal-digest-conflicts-otherwise` (#1104) depends on.
- il-path-arg-key-order-first-match, il-path-arg-harness-neutral,
  il-tool-name-matching-differs-across-consumers,
  il-classify-one-case-rule-per-query, il-reduced-tool-call-carries-extracted-path,
  il-synthetic-todowrite-round-trips-under-one-name, and
  il-tool-summary-description-precedes-key-arg are the behavior U4 preserved
  for byte-identical goldens; il-dedup-removal-leaves-goldens-byte-identical is
  the gate that held it.
- il-dedup-selector-never-fires-on-harness-names is the invalidated predecessor
  of il-dedup-removal-leaves-goldens-byte-identical.
- il-vocabulary-literals-live-in-one-module, il-literal-classified-contract-or-vocabulary,
  and il-gate-fails-on-distinctive-literal-outside-module share the
  classification in [`literals.md`](literals.md): the table decides the class,
  the gate enforces the distinctive subset, and the module holds the rest.
- il-classify-total-with-unknown and il-unknown-tool-never-dropped-deduped-or-edit-marked
  share `ToolClass::Unknown`.
- il-repository-id-equals-project-id-at-head is the predecessor of the `ri-`
  records (#1105 Part B S3, U7) and is bounded by
  `sc-repodoc-one-live-revision-above-historical-head`,
  `sc-repodoc-only-verified-missing-retires`, and
  `sc-occurrence-class-set-frozen-across-mirrors` (#1104).
- il-doc-read-nofollow-final-component-flat-list shares `read_document` with
  `sc-project-docs-canonical-hash-over-fixed-list` and
  `sc-repodoc-only-verified-missing-retires` (#1104) and is the predecessor of
  U8's `pd-` records.
- il-readmission-block-ends-batch-unnamed-at-head is the predecessor of U6's
  `sc-readmit-` records.

## #1104 records this work touches

The ten `sc-` records below live on PR #1104's branch at `6336ebe48`. This part
links them; it does not revise them. U10 hands the revisions to that catalog.

| Record | After #1105 | Linked from |
| --- | --- | --- |
| sc-project-docs-canonical-hash-over-fixed-list | revised under R12, R13, R14: the configured list, default the two fixed names; beneath-root resolution replaces `NOFOLLOW` | il-doc-read-nofollow-final-component-flat-list |
| sc-readmission-blocked-after-purge-tombstone | daemon half revised under R16: per-unit terminal refusal before the transaction | il-readmission-block-ends-batch-unnamed-at-head |
| sc-repodoc-one-live-revision-above-historical-head | true per lineage; R10 adds a cross-lineage rule | il-repository-id-equals-project-id-at-head |
| sc-repodoc-only-verified-missing-retires | revised under R10: a second retirement cause | il-repository-id-equals-project-id-at-head, il-doc-read-nofollow-final-component-flat-list |
| sc-unit-cap-refuses-whole-never-prefix | unchanged (R17) | the `unit_too_large` condition in [`loss-conditions.md`](loss-conditions.md) |
| sc-adapter-identity-native-or-refused | unchanged | il-adapter-refusal-is-per-message, il-unknown-part-kind-skipped-uncounted |
| sc-terminal-disposition-never-retried | unchanged; governs U6's new terminal cause | il-readmission-block-ends-batch-unnamed-at-head |
| sc-pending-unit-publishes-next-ready-pass | unchanged; R16 removes one cause of indefinite pending | il-readmission-block-ends-batch-unnamed-at-head |
| sc-digest-bounded-deterministic-char-boundary | unchanged (R19) | il-exit-code-only-for-opencode-bash |
| sc-occurrence-class-set-frozen-across-mirrors | unchanged (R10, R11) | il-repository-id-equals-project-id-at-head |
