# marker-grammar-is-single-and-named-in-guidance

## Discovery trigger

Specification [#835](https://github.com/ahrav/eidnara/issues/835), Property
Catalog record `marker-grammar-is-single-and-named-in-guidance` (refinement,
default-production), derived at `265df096`; decision D-8 (a single marker
grammar serves m0 spans, m0 footers, and the m1 block) and decision D-9 (one
paragraph naming the shapes and the override sentence, and the shapes on the
never-reproduce line, in all four guidance assets, with the plugin golden
pinning the bytes). The M2 ticket
[#839](https://github.com/ahrav/eidnara/issues/839) carries the guidance and
A1 golden re-export criteria.

## Evidence trail

`crates/daemon/src/decay_render.rs::correction_marker` is the one formatter.
`MarkerForm::Splice` yields `[corrected @N: key = value]` or
`[retracted @N: key]`; `MarkerForm::Entry` yields `key = value @N` or
`key retracted @N`. `crates/daemon/src/decay_render.rs::corrections_line`
joins entries into `[corrections: ...; ...]`. Every production caller routes
through these two: `apply_corrections` in the same file for m0 splices and
footers, and `crates/daemon/src/m1_compose.rs::render_memory_updates` for the
m1 block. A grep over `crates/daemon/src` finds no other production site that
formats `[corrected @`, `[retracted @`, or `[corrections:`.

`crates/daemon/src/decay_render.rs::PRECEDENCE_SENTENCE` is the sentence the
m1 block and the guidance share.
`crates/daemon/tests/eval_stale_render.rs::the_served_precedence_sentence_is_the_measured_one`
asserts it equals `eval_core::PRECEDENCE_SENTENCE`, the sentence M0 measured.

`crates/daemon/src/prompt_surface.rs::tests::every_guidance_names_the_correction_markers_the_renderer_emits`
builds the shapes by calling `correction_marker` with key `name` and ordinal
0, replacing `@0` with `@N`. For each of the four assets it finds the line
holding the spliced value shape and asserts that line also holds the spliced
retraction shape, the footer line over both entry shapes, and
`PRECEDENCE_SENTENCE`. It then finds the `Never reproduce` line and asserts it
lists `` `[corrected @N: …]` ``, `` `[retracted @N: …]` ``, and
`` `[corrections: …]` ``. The guidance shapes derive from the formatter, so a
formatter change without an asset change fails this test.

`crates/daemon/src/prompt_surface.rs::tests::no_guidance_holds_text_the_secret_scanner_flags`
runs the production redactor over each asset and asserts no detection. The
placeholder is `name` because `key = value` is text the scanner flags.

`packages/opencode-plugin/src/plugin/tool-registry.test.ts`, describe block
"A1 prompt-surface golden", reads each asset from `crates/daemon/assets` and
asserts `packages/opencode-plugin/src/shared/prompt-surface-a1-golden.md`
holds the same text and byte and MD5 baselines.

## Failure scenario

A second formatter drifts from the first, or one asset lacks the paragraph.
The agent then meets a marker shape its guidance never explained, may treat it
as history content, or may reproduce it in a reply. If an asset held
scanner-flagged text, evaluator cassettes would refuse every request that
carries it.

## Timing windows and dependencies

None. The assets are compiled into the daemon and read by the plugin golden at
test time.

## What a test must construct

All four assets checked against shapes produced by the production formatter;
the never-reproduce line checked for all three bracket shapes; the redactor run
over each asset; and the plugin golden compared byte for byte with the asset
files.

## Investigation log

### Q: Is there exactly one formatter for every emitted marker?

- Sources examined: grep for `correction_marker`, `corrections_line`,
  `[corrected @`, and `[corrections:` over `crates/daemon/src` at HEAD
  `c38af85a`.
- Findings: production call sites are `decay_render.rs` lines 196, 203, and
  209 inside `apply_corrections`, and `render_memory_updates` in
  `m1_compose.rs`. All other hits are test assertions with literal bytes.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Could a shared formatter make a golden self-consistent and wrong?

- Sources examined: the render and transform goldens in `m0_compose.rs`,
  `transform.rs`, and `decay_render.rs` tests.
- Findings: those goldens hold literal marker bytes such as
  `[corrected @3: db.port = 7000]`, not bytes derived from the formatter. Only
  the guidance test derives shapes from the formatter, and it compares them to
  hand-written asset text, so both sides are independent.
- Missing evidence: none.
- Conclusion: resolved with answer.

### Q: Do the named checks pass at HEAD?

- Sources examined: `cargo +1.98 nextest run -p daemon -p memory-store
  --all-features --locked` filtered to this part's tests; `bun test
  src/plugin/tool-registry.test.ts -t "A1 prompt-surface golden"` in
  `packages/opencode-plugin`; both at HEAD `c38af85a`.
- Findings: the Rust run passed 17 of 17, including both `prompt_surface`
  tests and `the_served_precedence_sentence_is_the_measured_one`. The plugin
  run passed 2 of 2 tests in the A1 block with 57 expectations.
- Missing evidence: none.
- Conclusion: resolved with answer.
