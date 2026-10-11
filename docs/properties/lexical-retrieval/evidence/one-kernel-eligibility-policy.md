# one-kernel-eligibility-policy

## Discovery trigger

Ticket #391, acceptance criterion AC3: "Parity tests and architecture checks
prove daemon wire/cache and retrieval batch adapters share one kernel
eligibility policy; checkout applicability stays separate." The RP2.3 property
bundle is unavailable in this repository, so the catalog reconstructs the
record from the ticket and verifies it against the #391 branch. PR #967 maps
`canonical_eligibility_has_one_kernel_policy_entry_outside_the_kernel` to AC3
as the architecture check and names
`retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot` as the
parity test that covers behavior.

## Evidence trail

Kernel policy, all in `crates/kernel/src/eligibility.rs`:

- `:198` `fn judge` is the one verdict function. It has no `pub` and is the
  string the architecture test asserts stays private.
- `:279` `judge_surface_served_in_tx` calls `judge` once per candidate at
  `:310`; `:241` `judge_in_tx` delegates to `judge_surface_in_tx` (`:266`),
  which delegates at `:274`. Every public entry funnels into this one call.
- `:341` `KernelStore::judge_eligibility` and `:351`
  `judge_eligibility_within_budget` both route through `:563`
  `judge_eligibility_with`, which takes one egress read snapshot at one tip.

Callers outside the kernel, the complete set the scan allows:

- `crates/daemon/src/kernel_routes/eligibility.rs:208` inside `fn evaluate`
  (`:195`), reached from `handle_kernel_eligibility_batch` (`:304`); `:458` is
  the module's own test fixture.
- `crates/daemon/src/embedding_dispatch.rs:441`
  `judge_eligibility_within_budget` in the dispatcher.
- `crates/retrieval/src/eligibility.rs:201` in `judge_occurrences` (`:195`) and
  `:217` in `judge_occurrences_within_budget` (`:210`).
- The lexical lane reaches the kernel through `judge_tracked`
  (`crates/retrieval/src/eligibility.rs:296`), called by `judge_batch`
  (`crates/retrieval/src/lexical/retrieve.rs:925`) from `admit_batches`
  (`:958`) and `revalidate` (`:1014`). Other product callers of the retrieval
  adapter: `crates/daemon/src/coverage.rs:189`,
  `crates/daemon/src/packing/mod.rs:397`,
  `crates/daemon/src/search_replacement/selection.rs:1153`,
  `crates/retrieval/src/exact/resolve.rs:427`.
- `crates/retrieval/Cargo.toml:11` opens `[dependencies]`; `:12` names
  `kernel`; no `daemon` line appears before `[features]` at `:19`.

Tests, both in `crates/daemon/tests/claim_eligibility.rs`:

- `:184` `retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot`
  starts a `KernelDaemon`, commits two decisions, materializes six claim
  occurrences, exports a source hold, and applies the batch to a
  `SearchProjection`. The `compare` closure judges the same six candidates
  three ways: `judge_occurrences` (retrieval adapter), the daemon's
  `kernel.eligibility.batch` wire method
  (`crates/daemon/tests/support/kernel_daemon.rs:300`), and
  `store.judge_eligibility` directly. It asserts equal snapshot tips, equal
  `known_as_of`, and per occurrence that the adapter disposition equals the
  kernel verdict and the route's wire verdict. It repeats after a retirement
  (`other` becomes `Retracted`, exclusions tally `canonical_claims` 2 and
  `promoted_memory` 1), after a supersede (every verdict `Retracted`), checks
  `MAX_ELIGIBILITY_CANDIDATES + 1` refuses with `KernelError::InvalidInput`,
  and judges live candidates under a foreign project as `WrongScope`.
- `:456` `canonical_eligibility_has_one_kernel_policy_entry_outside_the_kernel`
  parses the workspace `members` list, drops `crates/kernel`, asserts at least
  one member sits outside `crates/`, walks every `src` tree for `.rs` files,
  and counts non-comment lines containing `.judge_eligibility` or
  `::judge_eligibility`. Callers must be a subset of the three allowed files,
  and both adapters must appear. It then checks retrieval's `[dependencies]`
  block names `kernel` and omits `daemon`, and that
  `crates/kernel/src/eligibility.rs` contains `\nfn judge(` and no
  `pub fn judge(`.

## Failure scenario

A second verdict path, a local eligibility table in retrieval, or a cached
daemon verdict that outlives a retirement lets one path serve an occurrence
another refuses. A retrieval result then contains a memory the daemon's wire
route would deny, or the daemon admits a claim retrieval excluded, and the two
surfaces disagree on the same snapshot.

## Timing windows and dependencies

Retirement and correction after a grant: the test grants, commits a retirement
or a supersede, and judges again. The adapter reads a fresh snapshot each call;
the daemon route's `VerdictCache` keys verdicts by the snapshot's
classification generation (`cache_keys`,
`crates/daemon/src/kernel_routes/eligibility.rs:152`), and `evaluate_with`
(`:233`) re-judges when the miss batch's snapshot differs from the cached one.
The kernel's over-bound check runs before any reader is taken.

## What a test must construct

A live kernel with admitted decisions, a projection populated from a source
hold at one snapshot, and candidates carrying `object_id`, `source_revision`,
and `artifact_digest`. Faults: a retirement after the grant, a supersede that
retires the old lineage, a foreign `ProjectScope`, and an over-bound batch. The
architecture check needs only the working tree: the workspace manifest,
retrieval's manifest, and the kernel source.

## Investigation log

### Q: Does the scan cover every public kernel eligibility entry?

- Sources examined: `crates/kernel/src/eligibility.rs:369`
  `judge_surface_eligibility`, `:381`
  `judge_surface_eligibility_within_budget`, `:439`
  `judge_surface_eligibility_with_claims`, `:467`
  `judge_surface_eligibility_with_selected_claims`; their callers
  `crates/daemon/src/memory_reviewer/broker.rs:1480`,
  `crates/daemon/src/memory_reviewer/selection.rs:155`,
  `crates/retrieval/src/claims.rs:517`.
- Findings: the scan strings `.judge_eligibility` and `::judge_eligibility`
  match the two `judge_eligibility*` methods only. The four
  `judge_surface_eligibility*` methods are public and are called from three
  files outside the allowed list. All of them reach the same private `judge`
  through `judge_surface_in_tx`, so the verdict policy is still one function,
  but the test's caller inventory is scoped to the `judge_eligibility` entry
  points, as the catalog's Guarantee states.
- Missing evidence: a check that the surface entries' callers also form a
  reviewed list.
- Conclusion: resolved with answer - the record's scope is the
  `judge_eligibility` entry points; surface entries share the verdict
  function but are outside this check.

### Q: Is the parity test run against a projection or a fixture engine?

- Sources examined: `crates/daemon/tests/claim_eligibility.rs:184` through
  `:252`.
- Findings: the test opens a real `SearchProjection`, installs an identity,
  and applies a batch built from exported kernel rows, then reads candidates
  with `live_candidates`. The daemon route runs in a started `KernelDaemon`.
- Missing evidence: none for this question.
- Conclusion: resolved with answer - a real projection and a live daemon.
