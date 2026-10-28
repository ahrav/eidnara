# p5dep-domain-decision-read-matches-domain-and-kind-in-sql

HEAD `3a3b916b7`; the `admission.rs` references are re-verified at the
#1166 branch tip. Surface S2.

## Discovery trigger

Plan R2 names `visible_domain_decisions_as_of_in_scope(Surface::ExplicitSearch,
tip, "memory", Some(ScopeTermFilter {..}))` as the candidate-set read. The
doc comment at `crates/kernel/src/admission.rs:3165-3167` claims the domain
and kind are matched in SQL "before any row is read, so rows of other domains
are neither decoded nor counted against a caller's bounds, and a corrupt row
outside the domain never fails this read." Lens: protocol contracts; C4.

## Evidence trail

- `admission.rs:3168-3181`: the public method passes
  `ServedSelection::DomainDecisions(domain_id)` to `visible_selected_as_of`.
- `admission.rs:3481-3488` `ServedSelection` variants: `All`, `Named(&str)`
  (JSON array bound as `:ids`), `DomainDecisions(&str)` (bound as
  `:domain_id`). "Each variant is its own prepared statement, so its
  predicate is one the planner can serve from an index."
- `admission.rs:3554-3560` `ids_predicate`: for `DomainDecisions`,
  `"o.domain_id=:domain_id AND o.object_kind='decision' AND"`, prepended to
  the `WHERE` clause at `:3625`.
- `admission.rs:3561-3573`: for `DomainDecisions` the joins omit
  `observations` and use `dec.scope_id` / `dec.evidence_id` directly, so the
  statement reads only decision rows.
- `admission.rs:3677-3681`: three `LazyLock<String>` statement texts, one per
  shape, so `prepare_cached` reuses them.
- `admission.rs:3183-3211` `visible_selected_as_of`: reads in one
  `read_snapshot`, filters `Hidden`, sorts by `object_id`.
- Daemon re-check: `crates/daemon/src/kernel_routes/read.rs:89-97`
  `RowSelection::keeps` re-checks `object_kind == "decision" && domain_id ==
  domain_id` on each returned row ("Applied in the kernel query as well;
  `keeps` re-checks each returned row", `:84`).
- Tests: `crates/kernel/tests/kernel_canonical_read.rs:227`
  `the_domain_selection_equals_the_filtered_full_read`; `:448`
  `the_domain_selection_reads_no_row_of_another_domains_payload_pressure`;
  `:420` `the_domain_selection_reaches_a_target_past_the_row_cap_of_the_unfiltered_order`;
  in-module `admission.rs:6267-6275` reads `"memory"` after a reopen.

## Failure scenario

If the domain predicate were applied in Rust after decoding, a corrupt row in
the `harness-capture` domain (where P5's own descriptors live) would fail the
`memory` read with `CorruptCanonicalRow`, and the P5 pass would do work
proportional to the whole store rather than the domain. If the kind predicate
were dropped, scope and observation rows of the `memory` domain would enter
the candidate set and the P5 summary lookup would fail on them.

## Timing windows and dependencies

Single read snapshot; the tip is captured with the rows. No lock beyond the
reader.

## What a test must construct

- Decisions in `memory` and in another domain at one snapshot; assert set
  equality between the domain read and the filtered full read (existing).
- A row in the other domain with an undecodable payload or a missing registry
  row; assert the `memory` domain read is `Ok` (existing test does this
  through payload pressure; a corruption variant would strengthen it).
- A `memory`-domain observation and scope object; assert they are absent from
  the domain read.

## Investigation log

### Q: Is there any production caller besides `kernel.read` consumers?

- Sources examined: grep for `visible_domain_decisions_as_of_in_scope`
  across `crates/`.
- Findings: `crates/daemon/src/kernel_routes/read.rs:185` is the only
  non-test caller; it is reached from `canonical_memory.rs:188`,
  `memory_capture.rs:1054`, `:1611`, and `lib.rs` tests.
- Missing evidence: none.
- Conclusion: resolved; P5 would be the second direct caller.
