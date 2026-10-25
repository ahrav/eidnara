# Portfolio evaluation

The fresh-context evaluation of Part A ran during #1105's authoring, at
`a0e3f2993`, through the four METHOD lenses (harness fit, coverage balance,
implementability, wildcard). This file records its dispositions as carried
into this part and what re-verification at `41d889ea4` changed. Part B S2
records are proposed obligations and have no portfolio result of their own.

## Finding disposition

| Finding | Class | Disposition |
| --- | --- | --- |
| The dedup record asserted emptiness at a seam the code lacks | refinement | applied in the source: the check moved to the corpus seam. At HEAD the selector is deleted, and the record is `invalidated` |
| The adapter refusal record named two return shapes | refinement | applied in the source: three shapes, including the budget stop |
| The exit-code record hid the Pi truncation source | refinement | applied in the source: split into il-exit-code-only-for-opencode-bash and il-truncated-flag-read-per-harness |
| No record covered the reduced call's `path` | gap | queued as il-reduced-tool-call-carries-extracted-path; traced end to end here |
| No record covered the synthetic `todowrite` name | gap | queued as il-synthetic-todowrite-round-trips-under-one-name; verified here |
| No record covered the summary line's precedence | gap | queued as il-tool-summary-description-precedes-key-arg; verified here, with the description arm corrected to `TC: {description}` |
| No record covered the re-admission block at HEAD | gap | queued as il-readmission-block-ends-batch-unnamed-at-head; verified here |
| `read_document` classifies `Missing` through a followed parent | wildcard | carried in il-doc-read-nofollow-final-component-flat-list and [`loss-conditions.md`](loss-conditions.md); owned by U8 and OQ15 |

## Biases surfaced for the owner

- B1. HEAD records carry post-plan obligations in their `Impact` lines.
  Disposition: kept as links to the record or unit that owns each obligation.
- B2. The reachability of the summarizer `TC:` path rested on a
  medium-confidence label in #1104. Disposition: resolved here; the path needs
  a non-empty model chain, and the evidence is in
  [`catalog.md`](catalog.md#reachability-classes). Records on that path are
  `explicit-config-only`.

## Re-verification at `41d889ea4`

- Status changes: il-dedup-selector-never-fires-on-harness-names is
  `invalidated` (U4 deleted the selector).
- Reachability changes: il-exit-code-only-for-named-tool-and-harness is
  `explicit-config-only`; its only production caller is the capture adapter.
  Eight more records are `explicit-config-only` because selection, injection,
  and reduced-call rendering run only on the fold path, which needs
  `eidnara_folds()` and so a non-empty model chain
  ([`catalog.md`](catalog.md#reachability-classes)):
  il-dedup-selector-never-fires-on-harness-names,
  il-tool-name-matching-differs-across-consumers,
  il-reduced-tool-call-carries-extracted-path,
  il-synthetic-todowrite-round-trips-under-one-name,
  il-classify-total-with-unknown,
  il-unknown-tool-never-dropped-deduped-or-edit-marked,
  il-dedup-removal-leaves-goldens-byte-identical, and
  il-classify-one-case-rule-per-query. Class-driven supersession also needs
  `smart_drops`. il-path-arg-key-order-first-match and
  il-path-arg-harness-neutral stay `default-production` through wire
  extraction, which runs on every transform.
- `Exercised` changes: seven records moved to `yes` on U4's checks
  (`crates/daemon/src/tool_vocabulary_tests.rs`,
  `scripts/forbid-tool-literals.test.ts`, `selection.rs:2828`):
  il-tool-name-matching-differs-across-consumers,
  il-classify-total-with-unknown, il-unknown-tool-never-dropped-deduped-or-edit-marked,
  il-gate-fails-on-distinctive-literal-outside-module,
  il-dedup-removal-leaves-goldens-byte-identical,
  il-exit-code-only-for-named-tool-and-harness, and
  il-classify-one-case-rule-per-query. il-path-arg-key-order-first-match and
  il-path-arg-harness-neutral are `partial`: U4's per-site table holds no input
  pairing `pattern` with `filePath`. il-exit-code-only-for-opencode-bash was
  already `yes`. il-literal-classified-contract-or-vocabulary is `partial` on
  this part's table. il-adapter-refusal-is-per-message and
  il-unknown-part-kind-skipped-uncounted gained U3 replay witnesses.
- Confidence changes: thirteen records moved from the source's `medium` to
  `high`; each evidence file's investigation log names the code or test the
  change rests on.
- New findings: the generic vocabulary literals in `injection.rs` and
  `history_summarizer_chunk.rs`, and the Eidnara-owned spellings defined more
  than once (records il-vocabulary-literals-live-in-one-module and
  il-literal-classified-contract-or-vocabulary).
