# packing-selection-never-admits-every-rendered-row-is-kernel-visible

## Discovery trigger

Plan requirement R12 ("Every row the injection renders is Visible on
AutoInject by the Kernel's own verdict; the selection narrows, it never
admits, and a labeled row never appears unlabeled") and R3 (model-inferred
rows stay Hidden on AutoInject). Serves KTD5 and KTD6; tracker issue 657
("a batch verdict never grants AutoInject; labels survive presentation").

## Evidence trail

- `crates/daemon/src/canonical_memory.rs:183-211`: `injectable_snapshot`
  keeps `rows.iter().filter(|row| row.visibility == SurfaceVisibility::
  Visible)`, joins each to its `DecisionRow`, keeps positive categories,
  then trims. The doc comment at `:177-182` states labeled rows never reach
  `auto_inject` and that the guard keeps a label-bearing row out of a block
  that has no label renderer.
- `crates/daemon/src/canonical_memory.rs:166-172`: the read is
  `read_visible(&store, project, Surface::AutoInject, Some(tip),
  RowSelection::DomainDecisions(MEMORY_DOMAIN_ID))`.
- `crates/kernel/src/eligibility.rs:200-238`: `judge` returns `Hidden` when
  the served visibility is Hidden or no admission serves the object;
  `Superseded`, `Retracted`, `Stale`, `WrongScope`, `ProviderSensitive`
  precede it.
- `crates/daemon/src/kernel_routes/read.rs:188-195`: every returned row has
  passed `ScopeFilter::matches`.
- `crates/retrieval/src/packing/required.rs:137-139`: the required
  primitive refuses a `Hidden` disposition as `RequiredContextFailure::
  Hidden`; if the packer adapts rows to this primitive, a Hidden row fails
  the phase rather than being skipped.
- `crates/daemon/src/canonical_memory.rs:282-313`: existing test
  `injectable_rows_are_visible_decisions_in_a_positive_category`.
- `docs/properties/compression-fidelity/delivery/catalog.md:182-220`:
  `cf-memory-credit-requires-admitted-visible-content` is the
  provider-boundary sibling.

## Failure scenario

Legal witness: snapshot rows: D1 (`user_directive`, Visible), R1
(`repository_policy`, Visible), M1 (`model_inference` classes, Hidden on
AutoInject, `source_kind = user`, text "Run cargo fmt before every
commit"). Block renders D1 and R1; M1 is absent; `rules_served == 2`.

Smallest violating traces:
1. Partition before filter: the packer builds the required list from
   `response.rows` by `source_kind` before the `Visible` filter. A test seed
   or a future writer that inserts a `user_directive` row with model classes
   (Hidden) is rendered as a directive.
2. Second source: the required phase reads rules through a different read
   (for example a `RowSelection::All` read or a direct store query by
   `source_kind`) that lacks the AutoInject visibility judgement. A
   `review_only` row renders without its label.
3. Label loss: a row with surface visibility `Labeled` (if such a verdict
   exists for AutoInject in the future) is treated as Visible by an
   `!= Hidden` test instead of `== Visible`.

## Timing windows and dependencies

- Snapshot semantics: a directive revoked after the read is rendered for
  this pass and absent on the next; `known_as_of` discloses the snapshot.
  This is not a violation.
- Depends on N1 and N2 admitting through the unchanged policy; this record
  guarantees only that the read side adds nothing the Kernel did not serve.

## What a test must construct

1. Seed through the Kernel (not by constructing `CanonicalMemory` directly):
   - D1: `adr_accepted`, `user_directive`, `(explicit_user, user_explicit)`,
     `AcceptedAdr`: expected Visible.
   - R1: `PROJECT_RULES`, `repository_policy`, `(trusted_local_code,
     current_config)`, `ConfigObserved` with a live observation: expected
     Visible.
   - M1: `CONSTRAINTS`, `source_kind = user`, model classes: expected
     Hidden.
   - M2: a `user_directive` row admitted with model classes (constructible
     only through a test seed that bypasses the directive route): expected
     Hidden.
2. Run the injection read; assert the rendered object-id set equals
   `{D1, R1}` and is a subset of the Visible rows of the same `ReadResponse`.
3. Assert `rules_served == 2`, `rules_omitted == 0`.
4. Negative: construct a `ReadResponse` with a Visible row whose
   `decision_kind` is `adr_accepted` and `source_kind` is `repository_policy`;
   assert it is absent (see the `USER_DIRECTIVES` record).
5. Oracle is a subset relation per pass; no marker is needed.

## Investigation log

### Q: Does any existing path render memory rows from a non-AutoInject read?
- Sources examined: `grep -rn "render_memory_block" crates/daemon/src`;
  `m0_compose.rs:239`, `transform.rs:2826`.
- Findings: both composers render `ctx.project_memory_rows()`, which comes
  from the single `read_project_memory` read on AutoInject.
- Missing evidence: none at HEAD; the obligation is that U2 keeps this
  single source.
- Conclusion: resolved with answer - one read, one surface, at HEAD.
