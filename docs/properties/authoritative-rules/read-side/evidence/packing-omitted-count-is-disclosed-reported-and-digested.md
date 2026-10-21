# packing-omitted-count-is-disclosed-reported-and-digested

## Discovery trigger

Plan requirement R11 ("Rules not served are counted; the count appears as one
fixed-grammar line inside `<project-memory>` and as
`project_memory.rules_omitted` in the response diagnostics, and it
participates in the snapshot revision digest") and KTD5, KTD7. Serves AE7;
the key decision "Omission is disclosed, never silent".

## Evidence trail

- `crates/daemon/src/memory_render.rs:157-208`: `render_memory_block` emits
  `<wrapper>`, category elements, `</wrapper>`; no other element. The
  disclosure line is a new element the renderer or both callers must emit.
- `crates/daemon/src/m0_compose.rs:239-243` and
  `crates/daemon/src/transform.rs:2826-2830`: both composers call
  `render_memory_block(selected_memories, "project-memory")`; the omitted
  count must reach both or be rendered inside the shared function.
- `crates/daemon/src/canonical_memory.rs:42-67`: the digest covers the
  version string `eidnara-project-memory-revision-v1`, then
  `(category, render_memory_line(row))` of positive rows in object-id
  order; `truncated` is explicitly excluded because it does not change the
  rendered bytes. The omitted count does change the rendered bytes, so by
  the same reasoning it must be digested.
- `crates/memory-store/src/lib.rs:2165-2181`: `ProjectMemoryComposition::
  Canonical { known_as_of, truncated, revision }` is the persisted
  composition record; the plan adds `rules_served`, `rules_omitted`,
  `context_resolved` to the response diagnostics (U2 names
  `ProjectMemoryComposition::Canonical` fields among the files).
- `crates/daemon/src/transform.rs:7035, 7085, 7100`: the response's
  `project_memory` member is populated from the composition.
- `docs/host-wire-protocol.md:1032`: `project_memory` is listed among
  optional diagnostics of the `ok` response.
- `crates/daemon/src/canonical_memory.rs:315-371`: existing test
  `revision_changes_only_when_the_rendered_inputs_change`.

## Failure scenario

Legal witness: nine rules, five served; block ends
`\n<omitted-rules count="4"/>\n</project-memory>`; `rules_served = 5`,
`rules_omitted = 4`; the revision differs from the revision of the same five
served rows with `rules_omitted = 0`.

Smallest violating traces:
1. Silent omission: four rules omitted, no line rendered; the model and the
   user cannot tell the block is partial.
2. Count disagreement: the line says 4 and the diagnostic says 3 because the
   diagnostic counts only rows omitted by the reserve and not the row
   omitted by minimum service's upper bound (or the reverse).
3. Digest blind to N: the context changes so one rule moves from served to
   omitted while the served set stays the same five rows (impossible with
   a fixed order, but possible when a sixth row of equal cost replaces the
   fifth: served set changes). Simpler: a reserve config change moves N from
   0 to 4 with the served set unchanged (the first five fit either way and
   four new rules were just added, all omitted). The rows digested are the
   same five, so the revision is unchanged; the m1 revision signal does not
   move; a cached block without the line is served as current.
4. Line placed inside a category: `<PROJECT_RULES>...<omitted-rules
   count="4"/></PROJECT_RULES>`; a consumer that reads categories attributes
   the omission to one category.
5. Grammar drift: `<omitted-rules count=4/>` or `<omitted_rules>`; the
   parity test's byte comparison still passes (both harnesses get the same
   bytes), but the fixed grammar the plan promises for consumers is broken.

## Timing windows and dependencies

- The digest is consumed by the m1 revision signal and by composition
  metadata that decides whether m0 is re-rendered on later passes
  (`transform.rs:3215` `external_revision_changed`); a digest that ignores
  N delays re-rendering until some row changes.
- Depends on the reserve record for the line's reserved cost and on the
  prefix record for the value of N.

## What a test must construct

1. Renderer test: `render_memory_block_with_omitted(rows, "project-memory",
   N)` for `N = 0` produces exactly today's output; for `N = 4` produces
   today's output with `\n<omitted-rules count="4"/>` inserted before
   `\n</project-memory>`; a regex over the whole block pins the grammar.
2. Digest test: `CanonicalMemorySnapshot::new(.., rows, N)` differs between
   `N = 0` and `N = 4` for equal rows; is equal for equal rows and equal N
   across two constructions; a change in `truncated` alone still does not
   move it.
3. Diagnostics test: a transform response under the nine-rule seed has
   `project_memory.rules_served == 5`, `rules_omitted == 4`, and
   `context_resolved` present; `rules_served + rules_omitted == 9`.
4. Cross-check: the integer in the rendered line equals `rules_omitted`
   (parse the line).
5. Zero case: no `<omitted-rules` substring anywhere in the block and
   `rules_omitted == 0`.

## Investigation log

### Q: Does the existing digest version string change?
- Sources examined: `canonical_memory.rs:57` (`"eidnara-project-memory-
  revision-v1"`); plan U2 step 5 ("include N in the snapshot revision
  digest input").
- Findings: appending N unconditionally changes every existing revision on
  upgrade (one re-render per session); appending only when `N > 0` keeps
  existing revisions stable but makes `N = 0` and "no N" indistinguishable,
  which is harmless since both render the same bytes.
- Missing evidence: the implementation's choice.
- Conclusion: resolvable at implementation; either is sound; the test in
  step 2 must pin the chosen one.

### Q: Is the disclosure line visible to the provider or stripped?
- Sources examined: the protected-tags and surface-strip paths were not
  examined in depth for this pass (`render-a-hygiene-metric-ignores-surface-
  strips`, `docs/properties/daemon/rendering/catalog.md:1627`, names the
  class).
- Findings: unknown whether any strip touches `<project-memory>` children.
- Conclusion: unresolved, needs a trace of the m0 bytes through the surface
  strips; parity comparisons should be made on daemon m0 bytes (`extractM0`).
