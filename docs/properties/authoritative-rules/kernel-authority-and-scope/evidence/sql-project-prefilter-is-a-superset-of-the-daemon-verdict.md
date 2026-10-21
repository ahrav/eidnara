# sql-project-prefilter-is-a-superset-of-the-daemon-verdict

## Discovery trigger

Plan KTD4: "The SQL prefilter stays Project-only (`admission.rs:3548-3562`);
other dimensions are judged in the daemon filter." For that division of
labor to be sound, the SQL must never drop a row the daemon filter would
keep. No existing test states the relation, so it is cataloged as a claim.

## Evidence trail

- `crates/kernel/src/admission.rs:3371-3375` `ScopeTermFilter { dimension,
  value }`: one dimension, one exact value.
- `:3449-3565` `served_classes_sql`; the scope clause (`:3548-3562`) is:
  `:scope_dimension IS NULL` OR `scope_expr IN (scope ids with a term on the
  dimension whose operator is not exact/set, OR the context value is
  redacted, OR exact_value = :scope_value, OR exact_value is a redaction
  placeholder, OR a set member equals :scope_value or is a placeholder)` OR
  `scope_expr IS NULL` OR `scope_expr NOT IN (scope ids with any term on the
  dimension)`.
- `:3465-3471` comments: a redacted exact or set value decodes to
  `Uncertain` and is kept for the caller; a scope with no term on the
  dimension is kept because `scope_matches` treats absence as matching.
- `scope_expr` is `COALESCE(dec.scope_id, obs.scope_id)` for named and all
  selections and `dec.scope_id` for domain decisions (`:3480-3492`).
- `:3068-3100` `visible_as_of_in_scope` and
  `visible_domain_decisions_as_of_in_scope` thread the filter to the SQL.
- Daemon: `crates/daemon/src/kernel_routes/read.rs:178-184` binds
  `project.scope_term()` (`Dimension::Project`, the binding digest) and
  `:188-195` applies `ScopeFilter::matches` afterwards; the comment at
  `:190` says the filter judges operators the kernel query keeps.
- `ScopeFilter::matches` and `names_project` refuse every class the SQL
  keeps for the caller: `scope_id == None` (false at `project.rs:198-200`),
  no project term (`eligibility.rs:89`), redacted term (`Uncertain`,
  `scope.rs:1399-1401`), non-`exact`/`set` operator on `project` (`Range`
  or `VersionRange` compare against the digest and are `DoesNotMatch` or
  `Uncertain`; `GitReachable` on `project` is `Uncertain` with no head
  commit).
- The only rows SQL drops are those whose scope has a `project` term with an
  unredacted `exact_value` different from the digest, or an unredacted set
  with no member equal to it. `names_project` returns false for both
  (`Exact(e) == v` false, `Set(s).contains(v)` false).
- Test `crates/daemon/tests/kernel_routes.rs:1803`
  `rows_serve_only_to_the_project_their_scope_names` observes final rows
  only; `:3311` `a_row_under_any_scope_naming_the_project_is_readable_and_mutable`
  shows a non-default scope id with the project term serves.

## Failure scenario

A future edit tightens the SQL (for example drops rows with no `project`
term to save the filter work) and a row the kernel judges in scope vanishes
from every read with no refusal recorded; or the SQL compares the digest
against a `range` term's `exact_value` column (NULL) and drops it, while
`names_project` would have judged the range. Either way the daemon filter is
no longer the deciding judgement.

## Timing windows and dependencies

- Both the SQL and the filter read scope rows at the same snapshot
  (`:governing_as_of` bounds registry rows; `scope_term` rows are
  immutable).
- Redaction placeholders are compared textually in SQL
  (`sql_contains_redaction_placeholder`, `crates/kernel/src/redaction.rs:18`)
  and by `contains_redaction_placeholder` in Rust; the two must agree on the
  placeholder shape.

## What a test must construct

1. One store, binding digest `A`, scopes: `P_A = [project exact A]`,
   `P_B = [project exact B]`, `P_SET = [project set {A, C}]`, `P_SETB =
   [project set {B, C}]`, `P_NONE = []` (no project term), `P_RANGE =
   [project range (A0, A9)]`, `P_RED = [project exact <placeholder>]`, and a
   decision with `scope_id = NULL`; one decision admitted `Verified` under
   each.
2. `S = store.visible_as_of_in_scope(AutoInject, tip, None, Some(filter A))`
   ids; `F = read_visible(binding A, AutoInject, ..)` ids.
3. Assert `F ⊆ S`; assert `S` contains the `P_NONE`, `P_RANGE`, `P_RED`, and
   NULL-scope rows (kept for the filter) and omits `P_B` and `P_SETB`.
4. Assert `F == {P_A row, P_SET row}`.
5. Coverage markers: `prefilter_kept_a_row_the_filter_then_dropped` fires on
   the `P_NONE` row; `prefilter_dropped_a_row` fires on `P_B`.

## Investigation log

### Q: Does any row the SQL drops pass `names_project`?

- Sources examined: the clause at `:3548-3562`; `term_matches`
  (`scope.rs:1392-1437`); `names_project` (`eligibility.rs:86-94`).
- Findings: SQL drops a row only when the scope has a `project` term that is
  `exact` or `set`, the context value is not a placeholder, the stored value
  is not a placeholder, and no equality holds. `term_matches` on that term
  returns `DoesNotMatch`, so `scope_matches` is `DoesNotMatch` and
  `names_project` is false.
- Missing evidence: an executed comparison; the superset claim is read from
  the SQL and the Rust, not run.
- Conclusion: unresolved, needs the test in "What a test must construct";
  confidence stays medium until it runs.
