# packing-user-directive-renders-under-user-directives-by-source-kind

## Discovery trigger

Plan KTD6 ("`injectable_snapshot` maps `source_kind = user_directive` to
the `USER_DIRECTIVES` heading and keeps `decision_kind` as category for
every other row; repository rules are written with `decision_kind =
PROJECT_RULES`") and U2 test scenario "An `adr_accepted` row with
`source_kind = user_directive` renders under `USER_DIRECTIVES`; an
`adr_accepted` row with another `source_kind` is not rendered". Serves R1,
R12, AE1, AE3.

## Evidence trail

- `crates/daemon/src/canonical_memory.rs:189-199`: `injectable_snapshot`
  filters `is_positive_memory_category(&decision.decision_kind)` and sets
  `category: decision.decision_kind.clone()`. `adr_accepted` is not in the
  positive list, so a directive row would be dropped at HEAD.
- `crates/daemon/src/memory_render.rs:67-80`: `POSITIVE_MEMORY_CATEGORIES`
  includes `USER_DIRECTIVES` at index 5; `:58-64` the five ordered
  categories do not include it, so it renders after `NAMING`, in name order
  among the unranked positives (`:85-109`).
- `crates/kernel/src/envelope.rs:155`: `ObjectRow.source_kind` is available
  on every visible row; `DecisionRow` (`crates/kernel/src/slice/read.rs:
  11-22`) carries `decision_kind` and `payload` but not `source_kind`, so the
  mapping joins `VisibleRow.object.source_kind` with the decision.
- `crates/daemon/src/canonical_memory.rs:282-313`: existing test pins the
  `decision_kind` filter.
- Plan KTD1: the directive route writes `decision_kind: "adr_accepted"`,
  `source_kind: "user_directive"`; plan U5 step 4: repository rules write
  `PROJECT_RULES` (the plan's U5 text says "decision_kind" implicitly through
  AE3 "renders under `PROJECT_RULES`").

## Failure scenario

Legal witness: Visible rows (`adr_accepted`, `user_directive`, "Run cargo
fmt before every commit") and (`PROJECT_RULES`, `repository_policy`,
"[crates/storage] Preserve single-baseline storage."). Block:
`<PROJECT_RULES>` with the repository line, then `<USER_DIRECTIVES>` with
the directive line.

Smallest violating traces:
1. Directive dropped: U2 forgets the mapping; `adr_accepted` fails the
   positive-category filter; the directive is Visible in the Kernel and
   absent from the block. AE1 fails silently; `rules_served` under-counts
   or, worse, counts the row while the renderer drops it (the digest's
   positive filter at `canonical_memory.rs:51-54` would also drop it, making
   `rules_served` and the rendered set disagree).
2. Wrong row promoted to the heading: a test seed or a future writer inserts
   (`adr_accepted`, `repository_policy`); a mapping keyed on `decision_kind
   == adr_accepted` renders it under `USER_DIRECTIVES`, presenting a
   repository unit as a user directive.
3. Mapping too wide: a mapping keyed on `source_kind == user_directive`
   alone also renders a `user_directive` row whose `decision_kind` is a
   non-positive kind (for example a future `adr_rejected`), because the
   positive-category check is bypassed for mapped rows.
4. Category used as authority: a reader of the block treats
   `USER_DIRECTIVES` as the authority signal; the plan's key decision says
   the admission row is the authority and the category carries none. A
   `model_inference` row with `decision_kind = USER_DIRECTIVES` (a category
   the `eidnara_memory` tool can name today) would, if Visible, render under
   the same heading. It is Hidden, so the hazard is contained by the
   visibility record, not by this mapping.

## Timing windows and dependencies

- None.
- Depends on N1 writing exactly `source_kind = "user_directive"` and on
  `packing-selection-never-admits-every-rendered-row-is-kernel-visible`
  for trace 4.

## What a test must construct

1. Unit test on the mapping function `category_for(source_kind,
   decision_kind) -> Option<&str>`:
   - (`user_directive`, `adr_accepted`) -> `Some("USER_DIRECTIVES")`;
   - (`repository_policy`, `PROJECT_RULES`) -> `Some("PROJECT_RULES")`;
   - (`repository_policy`, `adr_accepted`) -> `None`;
   - (`user`, `USER_DIRECTIVES`) -> `Some("USER_DIRECTIVES")` (the existing
     behavior for a positive `decision_kind`, reached only if Visible);
   - (`user_directive`, `CONSTRAINTS`) -> pin: `Some("USER_DIRECTIVES")`
     per the plan's wording, or `None`; see the open question.
2. Snapshot test: a `ReadResponse` with the four rows above; the rendered
   block has the directive under `USER_DIRECTIVES` and no `adr_accepted`
   text from the repository-sourced row.
3. Digest consistency: the digest's positive filter must use the mapped
   category, so a directive row contributes to the revision; assert that
   adding a directive row changes the revision.
4. E2E (AE1): `/eidnara-directive add` then a pass; `<USER_DIRECTIVES>`
   contains the text; `review.list` shows no reviewer involvement.

## Investigation log

### Q: Must the required partition also check `decision_kind` for directives?
- Sources examined: plan KTD5 (partition by `source_kind` only), KTD6
  (mapping by `source_kind`), KTD1 (the route writes `adr_accepted`).
- Findings: with N1 as the only writer of `source_kind = user_directive`,
  every such row is `adr_accepted` and the question is moot in production.
  The Kernel does not forbid other kinds under that source kind; a test seed
  can create one.
- Missing evidence: a design decision on whether the mapping is
  `source_kind` alone or the pair.
- Conclusion: needs human input - the plan's wording maps by `source_kind`;
  pairing it with `adr_accepted` is stricter and costs nothing for the
  planned writer.
