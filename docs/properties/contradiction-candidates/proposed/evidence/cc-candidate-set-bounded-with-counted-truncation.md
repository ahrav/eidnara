# cc-candidate-set-bounded-with-counted-truncation

Verified at the #1167 branch tip (base `39a2eb3f9`).

## Discovery trigger

A project's memory decisions grow without bound, and every summary is analyzed
once per descriptor.

## Evidence trail

- `crates/daemon/src/contradiction_candidates/pass.rs:409` `candidate_summaries`
  reads explicit-search ids with the project scope term, judges each scope with
  `ScopeFilter`, keeps the newest `bounds.decisions` with `NewestRows`, adds
  the number of scope-matched decisions it cut to `candidates_truncated`, filters
  by `eligible` through claim facts in chunks of 64, cuts payloads by
  `decision_payload_sizes_as_of` before any load, and loads the rest in 64-id
  chunks.
- `crates/kernel/src/admission.rs:3165`
  `visible_domain_decisions_as_of_in_scope` matches domain and kind in SQL and
  has no row or budget bound; the pass's peak memory before the heap scales with
  the project's visible `memory` decisions.
- `crates/daemon/src/kernel_routes/project.rs:207` `ScopeFilter::matches` drops
  a `None` scope and judges each scope once (G1: the pass's rows all name the
  bound project).

## Failure scenario

Without the heap and the byte cut a project with 100,000 decisions would load
100,000 summaries per pass.

## Timing windows and dependencies

Every read uses the tip captured at the pass start (G2: ids, claim facts, sizes,
and payloads share one snapshot, so a later commit cannot mix two states into
one candidate set). Explicit search hides rejected, contradicted, quarantined,
and secret rows at `crates/kernel/src/admission.rs:4194`.

## What a test must construct

N + 2 visible decisions at a reduced N, so the count of cut decisions differs
from the count of truncating passes, another project's decision, a summary
cut, a superseded decision; all in `crates/daemon/src/candidate_pass_tests.rs`.
The production N is pinned by value, not constructed.

## Investigation log

### Q: Is the unbounded SQL walk acceptable? (OQ5-1)

- Sources examined: the evidence trail above.
- Findings: accepted at ticket approval; a row-capped, budgeted Kernel read is a
  follow-up.
- Missing evidence: none.
- Conclusion: resolved with answer; the record stays `partial`.
