# scope-context-is-resolved-per-pass-from-named-sources

## Discovery trigger

Plan requirement R6 ("the per-pass scope context is built daemon-side from
the bound project, the checkout HEAD branch, a configured environment, the
session `cwd`, and the paths named in tool-call blocks of the live window")
and KTD4 (Project from the binding, Branch from gix HEAD per pass,
Environment from `/memory/injection_environment`, directory as `Entity`
values with a `dir:` prefix). Serves KTD4 and the brief's read-side item 4.

## Evidence trail

- `crates/kernel/src/scope.rs:866-873`: `ScopeMatchContext` doc states
  "Values are resolved before predicate evaluation (never ambient state)";
  the struct holds one `Option<String>` per dimension plus `head_commit`.
- `crates/kernel/src/scope.rs:881-887`: `with_value` sets one value per
  dimension; a second call replaces the first.
- `crates/daemon/src/lib.rs:9338-9345`: one canonical read per pass; the
  comment says every memory surface of the pass composes from the same
  pinned snapshot. The scope context therefore has pass lifetime.
- `crates/daemon/src/canonical_memory.rs:141-146`: `read_project_memory`
  takes `(kernel, project, now_ms, memory_budget_tokens)`; no context input
  exists at HEAD.
- `crates/daemon/src/kernel_routes/project.rs:22-34`: `ProjectBinding`
  holds the canonical root, its digest, and the `ProjectScope`.
- `crates/daemon/src/config.rs:1198-1201`: the pattern for a numeric memory
  key; no `/memory/injection_environment` key exists at HEAD.
- `crates/daemon/src/git_reconcile.rs:434-491`: the gix usage the plan cites
  resolves ref tips (`ref_tips`) and traverses objects; no call resolves the
  symbolic HEAD referent. Branch derivation is new code.
- `docs/host-wire-protocol.md:953-976`: transform request table; no `cwd`
  row. `:951`: unknown fields are ignored, so an older plugin omitting `cwd`
  decodes.

## Failure scenario

Legal witness: pass 1 on branch `main` in `crates/storage` with the
environment key set to `prod`; `context_resolved` is `{cwd:
"crates/storage", branch: "main", environment: "prod", paths: [".",
"crates", "crates/storage"]}`. The user runs `git checkout --detach`; pass 2
reports `branch: null` and a rule carrying `branch = main` is absent.

Smallest violating trace: the Branch cell is cached in a daemon-wide or
session-wide cell instead of a per-pass cell. Pass 1 resolves `main`. The
checkout detaches. Pass 2 reuses `main`, serves the branch-scoped rule on a
detached HEAD, and `context_resolved.branch` reports `main`. The forbidden
outcome "a branch-scoped rule served on detached HEAD" occurs with a
diagnostic that agrees with the stale value, so the diagnostic cannot detect
it; only an external check of HEAD can.

Second violating trace: the Entity set accumulates across passes (a session
field appended each pass). A directory touched once early in the session
keeps its directory rule in every later block.

## Timing windows and dependencies

- HEAD can move between passes and during a pass; the design resolves once
  per pass and accepts that a checkout during a pass yields whichever value
  the read observed.
- Config reload between passes: the environment value must come from the
  pass's `DaemonConfig`, which the binding already carries
  (`lib.rs:9345` passes `binding.config`).
- The history-summarizer call site (`lib.rs:6006`) takes its own read with
  the same helper; whether it builds a `cwd`-only context is plan OQ7.

## What a test must construct

1. A temporary git repository bound as the project with a commit on `main`.
2. Kernel rows: Project-only; Project plus Branch `main`; Project plus
   Environment `prod`; Project plus Entity `dir:crates/storage`.
3. Pass A: `cwd = <root>/crates/storage`, environment key `prod`. Expect all
   four rows served and `context_resolved` as above.
4. `git checkout --detach`; pass B with the same request. Expect the Branch
   row absent and `context_resolved.branch == null`.
5. Unset the environment key; pass C. Expect the Environment row absent and
   `context_resolved.environment == null`.
6. Pass D with `cwd = <root>` and a window with no tool calls. Expect
   `paths == ["."]` and the `dir:crates/storage` row absent.
7. Oracle: `context_resolved` of each pass equals the externally known
   state; served set equals the predicted set. Assert that pass D's
   `paths` contains nothing from pass A.

## Investigation log

### Q: Does any daemon code resolve the symbolic branch name today?
- Sources examined: `grep -rn "referent_name\|head_ref\|head()" crates/daemon/src crates/kernel/src`.
- Findings: only `head_id()` and applicability snapshot `head()` (an oid
  string) in `crates/kernel/src/applicability/*.rs`; nothing resolves a
  branch name.
- Missing evidence: none; the code is absent.
- Conclusion: resolved with answer - Branch derivation is new; the plan's
  cited pattern covers gix repository opening and oid handling only.

### Q: Full ref name or short name for the Branch value?
- Sources examined: plan KTD4 and U1 ("gix `head().referent_name()`").
- Findings: `referent_name()` yields a `FullName` (`refs/heads/main`); the
  plan does not say whether it is shortened. Rule writers for Branch are
  deferred, so no writer fixes the form yet.
- Missing evidence: a decision on the value form.
- Conclusion: resolvable at implementation; pin the form in the context
  builder and in the deferred `--branch` writer together.
