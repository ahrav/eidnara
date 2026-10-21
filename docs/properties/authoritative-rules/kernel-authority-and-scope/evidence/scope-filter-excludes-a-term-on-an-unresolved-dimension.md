# scope-filter-excludes-a-term-on-an-unresolved-dimension

## Discovery trigger

Plan KTD4 and R5: "`ScopeFilter::matches` already treats an unresolved
dimension as non-match, so fail-closed exists; the work is supplying
context." The plan's directory, branch, and environment terms rely on this
behavior to exclude a rule when the pass cannot resolve the dimension.

## Evidence trail

- `crates/daemon/src/kernel_routes/project.rs:181-208` `ScopeFilter`: holds
  the binding's `ProjectScope` and a per-request `verdicts` memo; `matches`
  returns false for `scope_id == None`, otherwise memoizes
  `self.project.names_project(terms(scope_id)?.as_deref())` by `scope_id`.
- `crates/kernel/src/eligibility.rs:70-94` `ProjectScope`: `new` requires a
  64-char lowercase hex digest (`:76-83`); `context` is
  `ScopeMatchContext::new().with_value(Dimension::Project, digest)` (`:81`);
  `names_project` (`:86-94`) is `terms.is_some_and(|t|
  CanonicalScope::from_term_specs(t).is_ok_and(|s| s.term(Project).is_some()
  && scope_matches(&s, ctx, &UnknownGraph) == Matches))`.
- `crates/kernel/src/scope.rs:1450-1471` `scope_matches`: conjunction over
  `scope.terms()`; `DoesNotMatch` returns at once; `Uncertain` is sticky;
  the result is `Matches` only when every term matched.
- `:1392-1437` `term_matches`: `RedactedPlaceholder` is `Uncertain`
  (`:1399-1401`); `GitReachable` consults the oracle; otherwise
  `ctx.value(dimension)` absent is `Uncertain` (`:1418-1420`), a redacted
  context value is `Uncertain` (`:1421-1423`); `Exact(e)` is
  `bool_outcome(e == v)` (`:1425`); `Set(s)` is `bool_outcome(s.contains(v))`
  (`:1426`); `Range` and `VersionRange` follow.
- `:628-659` `CanonicalScope::from_term_specs`: unknown dimension,
  duplicate dimension, or undecodable term is an error, which `names_project`
  turns into false.
- `:473-535` `Dimension`: ten variants including `Entity`, `Branch`,
  `Environment`; stored strings are exactly `as_str`.
- Production scope writer: `project.rs:81-99` `scope_spec` writes one
  `exact` term on `project`. No production code writes any other term (grep
  for `Dimension::Entity`, `Dimension::Branch`, `Dimension::Environment`,
  `"entity"` outside `scope.rs` and tests).
- Read path: `crates/daemon/src/kernel_routes/read.rs:188-195` applies the
  filter to every row the kernel returned.
- Tests: `crates/kernel/tests/kernel_eligibility.rs:148` asserts
  `names_project` is false for `[branch_term(), project_term(A)]`, for a
  foreign project, for `None`, for `[]`, for `[branch_term()]`, and for a
  redacted project value; `crates/kernel/tests/kernel_scope_algebra.rs:325`
  asserts `Uncertain` for a constrained dimension with no context value;
  `:272` placeholders; `:335` definitive mismatch short-circuits.

## Failure scenario

A rule scoped `project + entity:dir:crates/storage` is read by a pass whose
context has no `Entity` value (today: every pass) and is served, so a
directory-scoped rule reaches every turn in the project. Or the filter
treats `Uncertain` as a match for some term kind (a `Set` with a redacted
member), serving a rule whose scope cannot be judged.

## Timing windows and dependencies

- The memo is per request and keyed by `scope_id`; scope rows are immutable
  once inserted, so the memo cannot go stale within a request.
- The SQL prefilter runs first; this filter sees only rows the prefilter
  kept (see the prefilter record).
- The context at HEAD has only the Project value; the plan's `InjectionScope`
  adds values, which turns `Uncertain` into `Matches` or `DoesNotMatch` for
  the dimensions it resolves and leaves the rest excluded.

## What a test must construct

1. Stored scopes through `insert_scope`: `S1 = [project:A]`;
   `S2 = [project:A, entity exact "dir:crates/storage"]`;
   `S3 = [project:A, entity set {"dir:.", "dir:crates"}]`;
   `S4 = [entity exact "dir:."]`; `S5 = [project:<redacted placeholder>]`.
2. `ScopeFilter::new(binding A).matches(Some(Sn), terms)`: `S1` true; `S2`,
   `S3`, `S4`, `S5` false; `None` false.
3. Direct `scope_matches` with a context that adds `Entity =
   "dir:crates/storage"`: `S2` `Matches`, `S3` `DoesNotMatch`; with `Entity
   = "dir:crates"`: `S2` `DoesNotMatch`, `S3` `Matches`.
4. End to end: decisions under `S1` and `S2` in the memory domain,
   `read_visible(binding A, AutoInject, ..)` returns only the `S1` row.
5. Memo: two rows sharing `S2` call `terms` once.

## Investigation log

### Q: Does any production writer emit an `entity` scope term today?

- Sources examined: grep over `crates/**/*.rs` and `packages/**/*.ts` for
  `Dimension::Entity` and `"entity"`; `project.rs:81-99`;
  `crates/daemon/src/lib.rs:13747` (under the `test-support` gate at
  `:13517`).
- Findings: hits only in `crates/kernel/src/scope.rs` (the enum),
  `crates/kernel/tests/kernel_scope_algebra.rs:426`, and
  `crates/kernel/tests/kernel_schema.rs:963,1246` where `"entity"` is an
  object kind string, not a scope dimension.
- Missing evidence: none.
- Conclusion: resolved with answer: no production writer; the `dir:` prefix
  cannot collide at HEAD. The N3 pass owns the forward-looking question.
