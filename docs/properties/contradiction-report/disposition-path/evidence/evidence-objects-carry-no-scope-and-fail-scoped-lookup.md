# evidence-objects-carry-no-scope-and-fail-scoped-lookup

Catalog record: [catalog.md](../catalog.md#evidence-objects-carry-no-scope-and-fail-scoped-lookup).
Authored at `3a3b916b7`, 2026-10-25; references re-verified at the PR A
tip of #1160 (code commit `6c4d445e3`), with PR A changes noted inline.

## Discovery trigger

Plan P4 R2: a cited `evidence_id` "must resolve to a live `evidence_meta`
row visible to the caller's project scope". Shared-facts: evidence objects
carry no scope. Lens: security boundaries (cross-project visibility), then
architecture. The route has exactly one scope mechanism, and this record
states what it answers for evidence.

## Evidence trail

- `object_state_sql` selects `COALESCE(dec.scope_id, obs.scope_id)` with
  `LEFT JOIN decisions` and `LEFT JOIN observations`
  (`crates/kernel/src/envelope.rs:1878-1892`); `object_state_from` reads it
  into `ObjectState.scope_id` (`:1894-1901`). The doc comment: "Decisions
  and observations are the object kinds that carry a scope and an evidence
  citation; every other kind reads `None` for both" (`:1903-1904`).
- `evidence_meta` has no `scope_id` column (`crates/kernel/src/schema.rs:
  139`). Tables with `scope_id`: `propositions` (`:118`), `scope_term`
  (`:130`), `asserted_edges` (`:143`), `decisions` (`:167`),
  `observations` (`:175`); `scopes` is the parent (`:126`).
- `ScopeFilter::matches` returns `Ok(false)` when `scope_id` is `None`
  (`crates/daemon/src/kernel_routes/project.rs:199-206`).
- `scoped_object_state` returns `Err(KernelError::NotFound)` when the
  filter does not match, "the same answer a missing object gets, prevents
  cross-project object-id enumeration" (`commit.rs:671-691`).
- Every scoped position in the route goes through it: tokens (`:1053`),
  disposition subject (`:706`), approval and chain members (`:714-716`),
  supersede predecessor and replacement (`:947`, `:957`), retire target
  (`:996`), projected dependencies (`:1029`).
- The preview reads served rows with `served_rows_for(.., Some(scope_
  term))` (`commit.rs:820`), another scope-bound read that selects
  decision rows, not evidence.
- The evidence object's registry row carries `object_kind`, `domain_id`,
  `source_kind`, `source_id` (`schema.rs:94`); `source_kind`/`source_id`
  describe the artifact's origin, not a project.

## Failure scenario

A P4 implementation that reuses `scoped_object_state` to check "visible to
the caller's project scope" refuses every `evidence_id` as `not_found`,
making the `--evidence` path unusable. An implementation that skips the
scope check lets a caller in project A cite evidence ingested under
project B, leaking that B holds an artifact with that id (the FK already
distinguishes existing from missing ids by `already_exists` versus a
successful write; see `admission-evidence-citation-is-existence-checked-
only`).

## Timing windows and dependencies

None in time. Structural: the schema has no project binding for evidence.
Evidence enters through `kernel.artifact.ingest.*` routes, which are
project-bound at the route level (`kernel_route_scope`, `mod.rs:415-441`)
but store no scope on the row.

## What a test must construct

- Ingest an evidence row (store API `ingest_evidence`,
  `kernel_admission.rs:92`, or the ingest routes); read its registry
  `object_id`.
- `kernel.commit` disposition with that `object_id` as subject:
  `invalid/not_found`. As `approval_object_id` on a valid subject:
  `invalid/not_found`. As a token: `invalid/not_found`.
- Assert `object_state(object_id).scope_id == None` through the store API.
- Coverage marker `scoped_lookup_of_evidence_object`.

## Investigation log

### Q: What does "visible to the caller's project scope" mean for a scope-less evidence object?

- Sources examined: P4 R2 and U1 Approach step 2; `envelope.rs:1878-1904`;
  `project.rs:199-206`; `schema.rs:94`, `:139`; `kernel_routes/mod.rs:
  415-441`; `docs/host-wire-protocol.md:26` (bearer key, no per-actor
  principal).
- Findings: no column, join, or route-level record ties an evidence row to
  a project. The ingest route is project-bound when it runs, but the
  binding is not persisted on the evidence row. Candidate definitions:
  (a) reachable through a decision or observation in scope that cites it;
  (b) ingested under a `commit_log` entry whose producer or cause names the
  project (no such column); (c) unconstrained.
- Missing evidence: the owner's intended definition; whether Batch K1
  should add a scope to evidence.
- Conclusion: at `3a3b916b7`, needs human input. Resolved by PR A with
  definition (a), extended to decision events and asserted edges:
  `Envelope::evidence_citations` (`crates/kernel/src/envelope.rs:532-577`)
  returns the scopes of the live citers, and `require_visible_evidence`
  (`crates/daemon/src/kernel_routes/commit.rs:742-759`) accepts an id
  only when the bound project's filter accepts one of them. Evidence
  objects still carry no scope, and this record's guarantee still holds
  for scoped lookups of an evidence object id; the record is re-labelled,
  not invalidated. See `cited-evidence-visible-to-bound-project` in
  [`../../dispute-evidence/catalog.md`](../../dispute-evidence/catalog.md).
