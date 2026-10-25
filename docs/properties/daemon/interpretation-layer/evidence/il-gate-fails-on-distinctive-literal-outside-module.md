# il-gate-fails-on-distinctive-literal-outside-module

## Discovery trigger

#1105 Part B S2 (R6, KTD4, AE6 first half): a CI gate in the comment-marker
gate's style.

## Evidence trail

- `scripts/forbid-tool-literals.sh:16` `PATTERN`: thirteen whole literals and
  the `mcp_` prefix, each spelled with a bracketed character so the script
  does not match itself.
- `:32-38` `scanned`: skips the module, `*_tests.rs`, `tests/`, `testdata/`,
  and every path outside `crates/daemon/src/`.
- `:42-50` `scan_file`: skips top-level `#[cfg(test)]` module regions.
- `:19-22` exits 2 on a tool failure; `:96-99` exits 1 after naming each hit
  as `file:line: text`.
- CI: `.github/workflows/ci.yml:302-303`.
- Tests: `scripts/forbid-tool-literals.test.ts` (exit 0 clean, exit 1 on a
  production literal, an `mcp_` name, and a nested module, exit 0 for each
  skipped path, test-region boundaries, `--staged`, a subdirectory run, exit 2
  outside git).
- The gated set equals the distinctive vocabulary subset of
  [`literals.md`](../literals.md).

## Failure scenario

A contributor adds `"filePath"` to `boundary.rs`; without the gate, the
module stops being the one site and nothing fails.

## Timing windows and dependencies

None.

## What a test must construct

Covered.

## Investigation log

### Q: Are `eidnara_reduce` and `eidnara_note` gated?
- Sources examined: `PATTERN`; `crates/daemon/src/prompt_surface.rs:65`, `:67`;
  `tool_vocabulary.rs:23`.
- Findings: no; they are Eidnara-owned identifiers defined once in
  `prompt_surface.rs`, and the gate leaves them out.
- Missing evidence: none.
- Conclusion: resolved with answer (OQ12).

### Q: How are inline test modules treated?
- Sources examined: `scripts/forbid-tool-literals.sh:42-50`; the test-region tests.
- Findings: a top-level `#[cfg(test)]` module region is skipped from its
  attribute to the next column-0 `}`.
- Missing evidence: none.
- Conclusion: resolved with answer (OQ13).

### Q: Why does confidence move from the source's `medium` to `high`?
- Sources examined: the source record in #1105; the code and tests cited above.
- Findings: U4 implemented the gate; `scripts/forbid-tool-literals.test.ts` asserts each exit status on scratch trees.
- Missing evidence: none for the confidence level; an `Exercised` gap, where
  the record names one, stays open.
- Conclusion: resolved with answer.
