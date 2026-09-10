# canonical-memory-irrelevant-candidates-do-not-consume-caps

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

Reducing the full candidate Vec must not reintroduce cap pressure from rows
the existing canonical reader excludes before bounded selection.

## Evidence trail

- [read.rs:84-92][keeps] admits only decisions in the requested memory domain.
- [read.rs:174-187][filter] applies that predicate and the scope algebra before
  rows enter `NewestRows`; exclusions do not set its dropped flag.
- [admission.rs:3185-3221][sql] has an inner own-admission join, no domain/kind
  restriction, and a conservative scope prefilter. Unadmitted registry rows
  are not its returned candidates.
- [admission.rs:2880-2910][visible] removes Hidden rows from the visible result.
- [canonical_memory.rs:195-210][late] filters Visible/positive categories and
  trims the memory budget after the route reader has applied its caps.

## Failure scenario

An optimization limits the kernel query before caller selection. Unrelated
recent observations consume its slots and push eligible decisions out.
Moving the later category filter earlier is a different regression: it changes
which memory decisions consumed the baseline cap, even if output looks useful.

## Timing windows and dependencies

Paired stores must retain the same relevant admissions, lineage, and scope
facts. A purported noise row that changes shared sensitivity is not irrelevant.
Adding commits can change the tip; compare each known-as-of to its own requested
snapshot rather than demanding unchanged metadata across different tips.

## What a test must construct

Independently seed eligible memory with valid other-domain decisions,
observations, hidden rows, and out-of-project rows. Place noise ahead of memory
in lexical and serving rank. Compare selected IDs, order, bytes, revision, and
truncation. K3 supplies pressure; existing generic cap checks remain
[unaudited](../existing-checks.md#canonical-read). No new exercise is claimed.

## Investigation log

### Q: Which fixture mutations are genuinely irrelevant?

- Sources examined: [The scope/admission query][sql] and [late filters][late].
- Findings: Domain/kind and scope exclusion precede caps, but positive category
  and Labeled exclusion do not. Shared lineage facts can affect another row.
- Missing evidence: A paired fixture isolating noise from shared authority has
  not been constructed or executed.
- Conclusion: The predicate boundary is resolved; fixture independence remains
  unresolved and must be checked before a metamorphic comparison is evidence.

[keeps]: ../../../../crates/daemon/src/kernel_routes/read.rs#L84-L92
[filter]: ../../../../crates/daemon/src/kernel_routes/read.rs#L174-L187
[sql]: ../../../../crates/kernel/src/admission.rs#L3185-L3221
[visible]: ../../../../crates/kernel/src/admission.rs#L2880-L2910
[late]: ../../../../crates/daemon/src/canonical_memory.rs#L195-L210
