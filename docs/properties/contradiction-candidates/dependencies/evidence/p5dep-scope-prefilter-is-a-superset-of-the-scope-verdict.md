# p5dep-scope-prefilter-is-a-superset-of-the-scope-verdict

HEAD `3a3b916b7`. Surface S2.

## Discovery trigger

Plan R2 relies on the `ScopeTermFilter` prefilter (`admission.rs:3060-3066`)
to restrict the candidate set to the project. The doc comment claims the
restriction "is a superset of the scope algebra's verdict: a scope with no
term on the dimension, a term whose operator the query cannot evaluate, and a
redacted term are all kept for the caller to judge." Lens: security
boundaries (cross-project leakage) and data integrity (silent omission); C5;
D5.

## Evidence trail

- `admission.rs:3371-3374` `ScopeTermFilter<'a> { dimension: Dimension,
  value: &'a str }`.
- `admission.rs:3463-3472`: redaction placeholders. `exact_redacted` on
  `t.exact_value`, `set_redacted` on each `json_each(t.set_values)` value,
  `filter_redacted` on `:scope_value`. Comments at `:3465-3471`: a redacted
  stored value "decodes to `MatchOutcome::Uncertain`" and a redacted filter
  value "decodes to `Uncertain` against every exact or set term", so both
  are kept.
- `admission.rs:3548-3562` the `WHERE` tail:
  `(:scope_dimension IS NULL OR scope_expr IN (SELECT t.scope_id FROM
  scope_term t WHERE t.dimension=:scope_dimension AND (t.operator NOT IN
  ('exact','set') OR filter_redacted OR t.exact_value=:scope_value OR
  exact_redacted OR EXISTS(... value=:scope_value OR set_redacted))) OR
  scope_expr IS NULL OR scope_expr NOT IN (SELECT t.scope_id FROM scope_term
  t WHERE t.dimension=:scope_dimension))`.
  Reading: a row is kept when any term on the dimension is non-exact/set,
  the filter value is redacted, an exact value equals, a stored value is
  redacted, or a set contains the value; when the scope is NULL; or when the
  scope has no term on the dimension. A row is dropped only when every term
  on the dimension is a decodable `exact`/`set` that excludes the value.
- `admission.rs:3480-3491`: for `DomainDecisions`, `scope_expr` is
  `dec.scope_id`.
- Daemon side: `crates/daemon/src/kernel_routes/project.rs:106-111`
  `scope_term()` = `{ Project, &self.digest }`; `:186-213` `ScopeFilter`
  judges each row with `ProjectScope::names_project` and memoizes per scope
  id; `:204-206` returns `Ok(false)` for a `None` scope id.
- `read.rs:170-188` comment: "The kernel keeps rows whose scope names another
  project out of the snapshot"; `:189`: the row bound "cannot be a SQL
  `LIMIT`" because the filter runs after SQL.
- Test: `crates/kernel/tests/kernel_scope.rs:235-388`: four scopes (branch
  `main`, branch `release`, branch redacted, project only); filter `branch =
  main` keeps `scope-1`, `scope-3`, `scope-4` and drops `scope-2`; a redacted
  filter value keeps all four.

## Failure scenario

Too narrow: a same-project decision whose scope carries a `set` term or an
operator other than `exact`/`set` on `project` would be dropped by SQL and
never reach the daemon filter, so a P5 pass would miss live candidates
silently. Too wide without the daemon filter: rows of other projects whose
scope terms are redacted, non-exact, or absent would be scored against a
descriptor of this project (cross-project leakage of summary text into a
candidate row). The daemon `ScopeFilter` closes the second gap; a P5 pass
that called the Kernel API directly (plan R2 names the Kernel API) must apply
the same filter or an equivalent.

## Timing windows and dependencies

Single snapshot. `stored_terms` (`project.rs:172-181`) reads scope terms from
the store's own read, so a scope inserted between the row read and the term
read is a benign race (the terms are append-only per scope revision).

## What a test must construct

- One decision per scope shape on `Dimension::Project`: exact match, exact
  other, `set` containing, `set` excluding, non-exact operator, redacted
  value, no project term, NULL scope.
- Assert the SQL-kept set is a superset of the algebra's `Match` set and
  equals `Match ∪ Uncertain ∪ NoTerm ∪ NullScope`.
- Then apply `ScopeFilter` and assert the final set equals the algebra's
  `names_project` set.

## Investigation log

### Q: D5. Can a `memory` decision have a NULL scope at HEAD?

- Sources examined: `project.rs:85-104` `scope_spec` (route-written rows
  always get a project scope), `kernel_scope.rs` fixtures (fixtures use
  `scope_id: Some(..)`), `read.rs:204-206`.
- Findings: route-written memory decisions carry a scope; the daemon filter
  drops a NULL-scope row. Other writers (history summarizer promotion,
  reviewer acceptance) were not audited in this pass.
- Missing evidence: an audit of every `insert_decision` caller for
  `scope_id: None` in the `memory` domain.
- Conclusion: partial; the superset claim holds regardless, and the daemon
  drops NULL scopes, so a P5 pass using the daemon pattern is safe.
