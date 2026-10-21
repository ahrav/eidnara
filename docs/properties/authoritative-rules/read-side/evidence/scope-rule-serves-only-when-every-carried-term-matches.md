# scope-rule-serves-only-when-every-carried-term-matches

## Discovery trigger

Plan requirement R5 ("the read side matches a rule only when every term it
carries matches the pass context and excludes a rule whose term names a
dimension the context cannot resolve") and R12. Serves KTD4; the brief's
forbidden outcomes: a dir-scoped rule served without a matching ancestor, a
branch-scoped rule served on detached HEAD, a rule served because a
dimension was absent.

## Evidence trail

- `crates/kernel/src/scope.rs:1450-1468`: `scope_matches` iterates present
  terms; `DoesNotMatch` returns at once; `Uncertain` is remembered and
  returned only if no later term is `DoesNotMatch`; absent dimensions match
  (doc comment at `:1448-1449`).
- `crates/kernel/src/scope.rs:1417-1419`: an absent context value for a
  present term is `Uncertain`; `:1399-1401` a `RedactedPlaceholder` term is
  `Uncertain`; `:1420-1422` a redacted context value is `Uncertain`.
- `crates/kernel/src/eligibility.rs:85-94`: `names_project` is `true` only
  when the terms decode, a Project term is present, and the outcome is
  `Matches`. `Uncertain` therefore fails. The doc comment at `:85` states a
  scope with no Project term "would match every project" and so fails.
- `crates/daemon/src/kernel_routes/project.rs:194-208`: `ScopeFilter::
  matches` returns `false` for `None` scope id and otherwise caches
  `names_project` per scope id for one request.
- `crates/kernel/src/scope.rs:613-660`: `CanonicalScope` holds at most one
  term per dimension; duplicates are `DuplicateDimension`.
- `crates/kernel/src/scope.rs:869-873`: the context holds one value per
  dimension, so an ancestor set cannot be one context value.
- `crates/kernel/src/admission.rs:3548-3562`: the SQL prefilter keeps rows
  whose scope has a term on `:scope_dimension` matching `:scope_value`, rows
  with no scope, or rows with no term on that dimension; the read passes
  only the Project term (`read.rs:176-186`), so Branch, Environment, and
  Entity terms are never evaluated in SQL.
- `crates/kernel/tests/kernel_scope_algebra.rs:325-332`: pins `Uncertain`
  for a constrained dimension with an empty context.

## Failure scenario

Legal witness: rows A (Project), B (Project + `dir:crates/storage`), C
(Project + Branch `main`). Context `{project, branch: main, paths: [".",
"crates", "crates/storage", "crates/storage/src"]}`. Served: A, B, C. Context
`{project, branch: null, paths: ["."]}`. Served: A only.

Smallest violating traces:
1. Absent-dimension admits: the daemon maps `Uncertain` to "serve" (for
   example by treating `scope_matches(...) != DoesNotMatch` as a match). Row
   C serves on detached HEAD. The kernel's `names_project` already rejects
   `Uncertain` for the Project term; the new code must do the same for
   every other term.
2. Ancestor iteration disagreement: the daemon evaluates the Entity term by
   building one `ScopeMatchContext` per ancestor and takes "any
   `Matches`" but reports `Matches` when every iteration was `Uncertain`
   (empty ancestor set yields a vacuous result). A `dir:` rule serves with
   no ancestor at all.
3. Project term dropped: the new matcher replaces `names_project` with a
   plain `scope_matches` over all terms. A row whose scope has only a
   Branch term (no Project term) matches every project on that branch.

## Timing windows and dependencies

- The judgement is per row at the pass's pinned read; the context is fixed
  for the pass. No interleaving inside the judgement.
- Dependence on `scope-context-is-resolved-per-pass-from-named-sources`: a
  stale context produces a correct judgement over wrong inputs.
- Dependence on N1 and N2 writing `dir:` values in the same canonical form
  the context derives (`Exact` equality is byte equality,
  `scope.rs:1425`).

## What a test must construct

1. Seed rows: Project-only; Project + Entity `Exact dir:crates/storage`;
   Project + Entity `Set {dir:crates, dir:packages}`; Project + Branch
   `Exact main`; Project + Environment `Exact prod`; a row with Branch
   only and no Project term; a row with a `RedactedPlaceholder` Entity term.
2. Contexts: full (all dimensions resolved, ancestors of
   `crates/storage/src/lib.rs`); detached (branch null); no environment;
   empty ancestor set; ancestors of `packages/cli/src/index.ts`.
3. Oracle per (row, context): served iff `names_project` and every present
   non-Project term is `Matches` under the stated ancestor rule (`Exact`:
   value in ancestor set; `Set`: intersection non-empty; empty ancestor set
   with an Entity term: excluded).
4. Assert the three forbidden outcomes as `always(!X)` over the served set:
   no `dir:` row without a matching ancestor; no Branch row when branch is
   null; no row whose term dimension is unresolved.
5. Assert the Branch-only row is never served under any context.

## Investigation log

### Q: Can the kernel's `scope_matches` be the single oracle for Entity?
- Sources examined: `scope.rs:869-873, 881-887, 1392-1468`.
- Findings: one value per dimension; `Set` terms test `values.contains(
  value)` against that single value. A rule `Exact dir:crates/storage` needs
  the context value to equal exactly that string; with ancestors
  `[., crates, crates/storage]` the daemon must try each. `scope_matches`
  cannot express "any of these values" on the context side.
- Missing evidence: the implementation's chosen iteration and its
  `Uncertain` rule ("Uncertain when no iteration matches and any is
  uncertain", plan U1 step 3).
- Conclusion: resolvable at implementation - one daemon function must own
  the multi-valued evaluation; `ScopeFilter`'s "cannot disagree" docstring
  (`project.rs:177-180`) must be narrowed to the Project term.

### Q: Does the SQL prefilter ever exclude a rule on a non-Project term?
- Sources examined: `admission.rs:3548-3562`, `read.rs:176-186`.
- Findings: only `:scope_dimension = project` is bound; other terms pass.
- Conclusion: resolved with answer - exclusion on other dimensions happens
  only in the daemon filter, so the daemon filter is the whole enforcement
  for Branch, Environment, and Entity.
