# repo-policy-rules-derive-from-head-tree-or-nothing

## Discovery trigger

Plan requirement R2 ("a recognized repository policy file in the bound
project's HEAD tree ... when the checkout has no readable HEAD, nothing is
written"), KTD2, key decision "Verified repository policy means witnessed
present at a named commit", scope boundary "working-tree bytes: no revision
identity, so no provenance witness", acceptance example AE4, blocking OQ3.
Forbidden outcome F4.

## Evidence trail

Verified at HEAD:

- gix is a daemon dependency: `crates/daemon/src/git_reconcile.rs:434-491`
  walks commits with `repo.rev_walk`, recomputes each object's hash
  (`:462-472`) before trusting it, and resolves refs by full name only
  (`:478-491`).
- `crates/kernel/src/applicability/checkout.rs:699-715` reads `head_id()`
  before and after a scan and rejects a snapshot whose HEAD moved; `:1467-
  1470` maps a missing head to the token `unborn`. These are the repository's
  existing patterns for "no readable HEAD" and for HEAD stability.
- `crates/kernel/src/envelope.rs:213-216` `RepositoryProvenance {
  repository_id, revision }`.
- `crates/kernel/src/cas/ingest.rs:184-187`: provenance is affirmative only
  when both fields are non-empty.
- `crates/daemon/src/harness_sources.rs:129-141`: the existing producer of
  `RepositoryProvenance` names `repository_id` and the commit `oid`; the plan
  substitutes the project digest and the HEAD oid.

Plan-only: the module, the gix tree enumeration of `AGENTS.md` entries, the
"return without writes" branch, and the project digest as `repository_id`.

Brief-versus-plan discrepancy: the brief's priority list names "detached"
among fail-closed inputs; plan U5 step 1 names no repo, unborn HEAD, and gix
error; KTD4 treats detached HEAD as "Branch absent" on the read side. A
detached HEAD has a commit oid, so the plan as written reconciles it.

## Failure scenario

Legal witness: HEAD `X` has tree entry `AGENTS.md` blob `B`. `reconcile`
resolves `X`, reads `B` by its oid from `X`'s tree, ingests `B` with
provenance `{digest, X}`; every rule on the lineage cites an artifact whose
digest is `sha256(B)` and whose revision is `X`; `git cat-file -p X:AGENTS.md`
reproduces `B`.

Smallest violating traces:

1. The module reads `<project_root>/AGENTS.md` from the filesystem and stamps
   provenance `{digest, X}`: an uncommitted edit becomes a Verified rule with
   a witness that does not contain it. The check (re-read blob at
   `revision:path`, compare digest) fails.
2. The module resolves HEAD to `X`, then enumerates the tree by calling
   `repo.head_tree()` again after a concurrent `git checkout` moved HEAD to
   `Y`: bytes from `Y` carry provenance `X`.
3. The project root is not a repository; gix open fails; the module writes a
   `config_present` observation with an empty revision: ingest demotes the
   artifact to Sensitive (no affirmative provenance), the trigger sensitivity
   hides the rules. Fail-closed by accident; the record requires no write at
   all so AE4's "no observation is written" holds.
4. Unborn HEAD (`git init` without a commit): `head_id()` errors; any write is
   a violation.

## Timing windows and dependencies

HEAD can move between oid resolution and tree read; keying the tree read by
the resolved oid removes the window. HEAD can also move between the tree read
and the Kernel commit; that is benign because provenance names the oid the
bytes came from, and the cache record handles reconvergence.

## What a test must construct

- A temp directory that is not a repository; run a pass; assert zero
  `repository_policy` objects and zero `config_present` observations on
  `repository_policy` lineages, and the m0 block unchanged (AE4).
- `git init` with no commit; same assertions.
- A repository where `AGENTS.md` at HEAD differs from the worktree; run a
  pass; assert the rule text equals the HEAD bytes and the artifact digest
  equals `sha256(HEAD blob)`.
- A corrupt loose object for the blob (gix read error); assert no writes for
  that pass.
- Oracle for every live rule: resolve `provenance.revision` as a commit, read
  the blob at the lineage path, compare digest with the artifact.

## Investigation log

### Q: Does a detached HEAD count as "no readable HEAD" for the writer?
- Sources examined: plan U5 step 1, KTD4, brief priorities for N2.
- Findings: plan reconciles (oid exists); brief lists detached as fail-closed.
- Missing evidence: a design statement reconciling the two.
- Conclusion: needs human input.

### Q: Is the project digest an acceptable `repository_id`?
- Sources examined: `ingest.rs:95-125` identity checks on provenance fields;
  `harness_sources.rs:129-141` uses a repository identity string.
- Findings: any identity-valid non-empty string is accepted; the digest is a
  hex string and passes.
- Missing evidence: none.
- Conclusion: resolved with answer: accepted by the Kernel; semantic fit is a
  plan decision.
