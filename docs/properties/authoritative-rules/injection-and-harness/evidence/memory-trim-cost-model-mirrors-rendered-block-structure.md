# memory-trim-cost-model-mirrors-rendered-block-structure

## Discovery trigger

Lens: unproven assumptions. The trim charges a wrapper, a per-category
wrapper, and a line per row with its own string shapes while the renderer
concatenates differently. KTD5 reserves wrapper and disclosure tokens before
packing, so the additivity of this cost model becomes load bearing.

## Evidence trail

Trim (`crates/daemon/src/m0_compose.rs:80`, `:93-98`):

- outer: `estimate("<project-memory>\n</project-memory>")`
- per row: `estimate(render_memory_line(memory) + "\n")`
- per first row of a category: `estimate(format!("<{c}>\n</{c}>\n"))`

Renderer (`crates/daemon/src/memory_render.rs:184-213`):

- `"<project-memory>"` then per category `"\n<" c ">"`, per row `"\n"` +
  line, `"\n</" c ">"`, then `"\n</project-memory>"`.

Byte arithmetic with `L = len(line)`, `C = len(category)`, `W = 16`:

- outer: trim `16 + 1 + 17 = 34`; render `16 + 18 = 34`.
- per category: trim `C + 2 + 1 + C + 3 + 1 = 2C + 7`; render
  `1 + C + 2 + 1 + C + 3 = 2C + 7`.
- per row: trim `L + 1`; render `1 + L`.

So under a byte estimator the charged total equals
`render_memory_block(kept).len()` exactly, provided the category needs no
attribute escaping. Every positive category name is `[A-Z_]+`
(`memory_render.rs:67-80`), and `push_escaped_xml_attr` (`:40-55`) changes
nothing for them. The line is `render_memory_line` in both, so content
escaping and the 64 KiB cut are identical.

Under the production estimator `token_cache::cached_estimate_tokens`
(`canonical_memory.rs:209`), the tokenizer sees `line + "\n"` and
`"<C>\n</C>\n"` as separate strings while the block is one string; token
boundaries at the joins can merge or split, so the block's token count may
differ from the charged sum by a bounded number of tokens per join.

Tests: `m0_compose.rs:300` and `:337` compare the trim with its own retained
loop (same cost model), never with the renderer; `memory_render.rs:544-616`
tests exact counting for the history block, not the memory block.

## Failure scenario

If the cost model undercounts (a renderer change adds bytes the trim does
not charge), the rendered block exceeds `memory_budget_tokens` while every
trim test passes. If it overcounts, rows that fit are skipped. After KTD5, an
undercount makes the reserved `<omitted-rules count="N"/>` line the byte that
pushes the block over budget, defeating "reserved before packing so it is
never a victim".

## Timing windows and dependencies

None. Pure functions. Depends on the vocabulary staying attribute-safe and
on `render_memory_line` being the shared line renderer.

## What a test must construct

1. Rows across two or more categories with `&`, `<`, newlines in content and
   one row at 64 KiB + 7 bytes; byte estimator; a generous budget. Assert
   `34 + sum_cat(2C+7) + sum_rows(L+1) == render_memory_block(kept,
   "project-memory").len()`.
2. Same rows with `tokenizer::estimate_tokens`; record `charged -
   estimate(block)` and assert `|diff| <= joins` where `joins = 1 + 2 *
   categories + rows`.
3. A budget exactly equal to the charged total; assert all rows kept and
   `render_memory_block(kept).len() == budget` under bytes.
4. Coverage: `m0_trim_two_categories_kept` fires.

## Investigation log

### Q: How large is the tokenizer boundary slack for realistic rows?

- Sources examined: `m0_compose.rs:93-98`; `memory_render.rs:184-213`;
  `crates/daemon/src/token_cache.rs:293` (`cached_estimate_tokens`); no measurement run
  (the brief forbids running tests).
- Findings: the byte identity is exact; the token difference is bounded per
  join by the tokenizer's merge behavior and unmeasured.
- Missing evidence: a run of test 2 above.
- Conclusion: partial: the byte identity is exact; the token difference is
  unmeasured.
