# Fault map

Fault classes for the interpretation layer, the records each one constructs,
and the coverage checks to add. Carried from #1105 Part A section A4 and
updated for the checks U1 to U4 landed; code references verified at
`41d889ea4` and re-checked at `aff5a00e6`. Abbreviations follow
[`catalog.md`](catalog.md#scope).

## Fault classes

| Fault or state | Available how | Records | Constructed at HEAD |
| --- | --- | --- | --- |
| F-a1 tool name outside every table (`mcp_foo`, `<server>_<tool>`) | scripted session with one MCP tool; hand-built inputs | il-classify-total-with-unknown, il-unknown-tool-never-dropped-deduped-or-edit-marked, il-tool-name-matching-differs-across-consumers | yes: `tv-tests:133`, `:232`, `sel.rs:2828`; the frozen OpenCode session's `corpus_probe` MCP call |
| F-a2 name differing by case or `mcp_` prefix | hand-built names | il-tool-name-matching-differs-across-consumers, il-classify-one-case-rule-per-query, il-synthetic-todowrite-round-trips-under-one-name | yes: `tv-tests:17`, `:257` |
| F-a3 input with two recognized keys, a non-string first key, `file_path` only, a 61-scalar value | hand-built JSON | il-path-arg-key-order-first-match, il-path-arg-harness-neutral, il-reduced-tool-call-carries-extracted-path, il-tool-summary-description-precedes-key-arg | yes at the query level: `tv-tests:66`, except an input pairing `pattern` with `filePath`; not through a reduced call |
| F-a4 `bash` part with null, string, or float `exit`; non-`bash` part with `exit`; Pi truncated result | trace fixture mutation; Pi corpus session | il-exit-code-only-for-opencode-bash, il-exit-code-only-for-named-tool-and-harness, il-truncated-flag-read-per-harness | yes, except a harness-produced Pi truncated result: `tv-tests:145`, `:185` |
| F-b1 Pi `toolResult` with a text item and an image item | frozen Pi session (`crates/daemon/testdata/corpus/sessions/pi-0.80.2/mix.json`); `hs-tests:445` fixture | il-adapter-refusal-is-per-message | at the adapter: `hs-tests:757-762`, and through a capture pass in `replay:82`, where the reader classifies the record as a `non_text_item` non-unit, so the refusal reconciles as no loss, not as `adapter_refused` |
| F-b2 OpenCode message with text parts beside a settled tool whose output is not a string | hand-built JSON | il-adapter-refusal-is-per-message | yes at the adapter: `hs-tests:681-686`; not through a capture pass |
| F-b3 `step-start`, `reasoning`, `file`, `ignored` text, running or pending tool, interrupted tool | frozen OpenCode session; synthetic sessions in `replay` | il-unknown-part-kind-skipped-uncounted | partly: `replay:82`, `:215`, `:338` |
| F-b4 second pass after the turn completes (unfinished text, interrupted tool) or after a pending or running tool settles | two-pass capture test | il-unknown-part-kind-skipped-uncounted | yes for text: `hs-tests:922`; no for a settled tool or an interrupted tool |
| F-b5 purged digest re-offered beside a sibling | Kernel purge, then a capture pass | il-readmission-block-ends-batch-unnamed-at-head | yes, by U6: `sc-tests:2521`, `:2568`, `:2614`, `:2638`; `hs-tests:2555`, `:2607`, `:2664` |
| F-c1 legacy `(P, P, path)` live row, then a derived-identity pass | seeded Kernel row plus a `git init` root | il-repository-id-equals-project-id-at-head | yes, by U7: `sc-tests:2743`, `:2784`, `:2942` |
| F-c2 two worktrees of one repository | the U1 materializer with a worktree | il-repository-id-equals-project-id-at-head | yes, by U7: `sc-tests:2818` |
| F-c3 symlinked intermediate directory with and without a target; no `openat2` | U8 fixtures; a mocked syscall | il-doc-read-nofollow-final-component-flat-list | yes, by U8: `pd.rs:687`, `:734`; `sc-tests:3124` |
| F-d1 a distinctive literal added to a production file | a scratch tree | il-gate-fails-on-distinctive-literal-outside-module, il-vocabulary-literals-live-in-one-module | yes: `scripts/forbid-tool-literals.test.ts` |
| F-d2 a golden diff in a move-only change | the PR base | il-dedup-removal-leaves-goldens-byte-identical | once, for U4: `git diff a91dbc2b6 97afdaff1 -- crates/daemon/testdata` is empty; no recurring check |
| F-d3 a new literal with no table row | a code change | il-literal-classified-contract-or-vocabulary | by review of [`literals.md`](literals.md) only |

## Coverage checks to add

Each marker asserts preconditions, never the violation.

- `sometimes` a window held a tool name outside every table (F-a1).
- `sometimes` a Pi tool result held a non-text item beside a text item (F-b1).
- `sometimes` an OpenCode window held a running tool that settled on a later
  pass (F-b3 with F-b4).
- `sometimes` two checkouts of one repository captured the same path in one
  campaign (F-c2).
- `sometimes` a reduced call's original input named a path under each key of
  `PATH_KEYS` (F-a3 through a reduced call).

## Ranking by cheapest valid oracle

1. A per-site table row `{"pattern": "p", "filePath": "f"}` in `tv-tests:66`:
   closes the display-precedence gap of il-path-arg-key-order-first-match and
   il-path-arg-harness-neutral with a pure-function oracle.
2. A unit test over `reduced_block` and `flatten_block` with one input per key
   of `PATH_KEYS` and one with none: closes il-reduced-tool-call-carries-extracted-path
   with a pure-function oracle.
3. A `format_tool_summary` case whose input holds both a description and a
   `filePath`: pins the precedence of il-tool-summary-description-precedes-key-arg.
4. A capture pass over the `hs-tests:681-686` record, asserting
   `adapter_refused == 1`: closes F-b2 for il-adapter-refusal-is-per-message
   through the counter.
5. A fold-path transform holding the echoed synthetic `todowrite` pair beside
   a native call, asserting that keep-one and the read-back act on the native
   call alone: closes the round-trip half of
   il-synthetic-todowrite-round-trips-under-one-name. Selection never sees the
   synthetic pair, so a selection-only input cannot construct it.
6. A Pi session whose tool output passes the harness's own limit: gives
   il-truncated-flag-read-per-harness a harness-produced witness.
7. A mount crossing inside the root (F-c3): needs privileges a test host lacks.
