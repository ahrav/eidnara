# canonical-read-staleness-is-distinguishable-from-emptiness

## Discovery trigger

The N1 specification (issue 306) retires the claim-mirror read fence in the
transform, whose every failure branch returned `Ok(None)` and rendered as "no
claims" (`mirror-read-fence-relies-on-generation-advance`, now invalidated). The
replacement reader composes the project-memory block from canonical kernel rows,
and the specification requires that a read the daemon could not serve be a
distinguishable composition outcome, never an empty memory set. This record is
the replacement property named in that specification.

## Evidence trail

Verified at HEAD.

- `crates/daemon/src/canonical_memory.rs`: `CanonicalMemoryRead` has two
  variants, `Available(CanonicalMemorySnapshot)` and `Withheld(KernelOutcome)`
  (`:91-97`). `composition()` (`:101-112`) maps `Available` to
  `ProjectMemoryComposition::Canonical { known_as_of, truncated, revision }` and
  `Withheld` to `ProjectMemoryComposition::Withheld { state }` with
  `state = verdict.state_key()`. `revision()` (`:115-120`) is `None` when
  withheld. `rows()` (`:123-128`) returns the snapshot rows or an empty slice.
  Each is a single `match` with no default arm.
- `crates/daemon/src/canonical_memory.rs:141-175` (`read_project_memory`): the
  store phase (`KernelOpenCoordinator::kernel_store`), the `tip` read, the
  `outbox_lag` read, the serving decision
  `serving::project(serving::decide_for_tip_read(&lag), Surface::AutoInject)`,
  and the `read_visible` call each return `Withheld` carrying a
  `KernelOutcome`; only a served read constructs `Available`, and it does so
  through `injectable_snapshot` (`:184-212`), which filters to visible
  positive-category decisions, trims them to the memory budget, and hands the
  trimmed rows to `CanonicalMemorySnapshot::new` (`:50`), which digests exactly
  those rows once as the lines the renderer emits. The read passes
  `RowSelection::DomainDecisions(MEMORY_DOMAIN_ID)` (`kernel_routes/read.rs`),
  so only memory-domain decisions reach the row and byte caps and another
  domain's rows cannot crowd them out. The tip is read before the lag sample
  and passed to `read_visible` as `as_of`, the same order `handle_kernel_read`
  uses for a gated read, so the rows a served read pins are the rows the
  freshness verdict covered.
- `crates/daemon/src/kernel_routes/serving.rs:58-63` (`decide_for_tip_read`):
  zero registered consumers is served; otherwise `decide` applies the route's
  lag thresholds. So in a deployment with no outbox consumer, the withheld arm
  is reached only through store phase or read errors.
- `crates/daemon/src/kernel_routes/state.rs` (`KernelOutcome::state_key`):
  the recorded reason is the serialized `kind` tag joined with the serialized
  `reason` by `:`, read from the same wire object the TypeScript client derives
  its `stateKey` from, so the two cannot drift.
- `crates/memory-store/src/lib.rs:1322-1336` (`ProjectMemoryComposition`):
  `#[serde(tag = "kind")]` with variants `canonical` and `withheld`, so the two
  records differ on the wire and in the durable blob, not only in Rust.
- `crates/daemon/src/transform.rs:2622`, `:4266`, `:4437`: every HARD arm
  (compaction-off, compaction-on, and the re-cut arm) writes
  `meta.project_memory = ctx.project_memory_composition()` next to the frozen
  m0 bytes, inside the same `TransformCommit`. The accessor (`:561`) maps a
  `None` context read to `None`, so a memory-disabled pass records no
  composition.
- `crates/daemon/src/transform.rs:1748-1755` (`compose_m0_for_context`) and
  the additive path pass `ctx.project_memory_rows()` to the renderer, so a
  withheld read renders no block because it has no rows, not because a
  separate flag suppresses it.
- `crates/daemon/src/lib.rs:7994`: the read is taken once per pass through
  `Handler::project_memory_read` (`lib.rs:4834`), which returns `None` when
  memory is disabled so the kernel store is not consulted, before the
  `run_transform` closure; every attempt of that pass clones the same value
  into its `ProducerContext`, so the m1 revision signal, m0, and additive m0
  compose from one snapshot. A historian firing the pass triggers receives the
  same pinned value through `HistorianPrepareContext` (`lib.rs:5104`). The
  wrapup route takes its own read through the same helper (`lib.rs:5244`).
- `crates/daemon/src/historian_chunk.rs:681-687`: the assembler gates the read
  on `memory_enabled` once, renders the block from that gated read's `rows()`,
  and (`:734`) records the same gated read's `composition()` on
  `AssembledHistorianFiring.project_memory`, so a withheld read renders no
  block and carries `Withheld { state }`, a served read with zero rows renders
  no block and carries `Canonical { .. }`, and a gated-off read carries `None`.
  `prepare_historian_fire` copies that record into the fired
  `HistorianDiagnostics.project_memory` (`lib.rs:5148`), which the transform
  response returns under `historian`; every not-fired diagnostics literal
  leaves it `None`. Summarization continues through a kernel outage while the
  diagnostics keep the reason.

## Failure scenario

The kernel store is still opening when the first transform pass of a session
arrives. The reader returns `Withheld(Unavailable { StoreStarting })`. m0
freezes without a `<project-memory>` block. If the composition record were
`Canonical { known_as_of: 0, truncated: false, .. }` or absent, a later
operator reading `session.status` or the response could not distinguish this
session from one whose project holds no injectable memory, and the m1 revision
signal would treat the two states as equal, so the block would never be
rematerialized when the store became ready.

## Timing windows and dependencies

- The window between the read (`lib.rs:7994`) and the commit is closed to the
  verdict: the read result is a value in the context, and a phase or lag change
  after it cannot alter what the pass records.
- The property depends on the two-variant enum staying exhaustive; a third
  variant fails to compile at both `match` sites.
- The distinction reaches the m1 revision signal through
  `ProjectMemoryComposition::revision()` (`None` when withheld), so a served
  read with rows changes the external revision and forces the HARD that
  rematerializes m0 (`transform.rs:13346` pins this). The digest covers only
  the rows the block renders, so a change to a row the budget excludes, or a
  flip of the read cap's `truncated` flag, leaves the revision alone
  (`canonical_memory.rs` tests `rows_past_the_memory_budget_are_neither_injected_nor_digested`
  and `revision_changes_only_when_the_rendered_inputs_change`).

## What a test must construct

1. A `ProducerContext` whose `project_memory` is `Some(Withheld(verdict))` for
   each of `Stale`, `Abstained`, and `Unavailable`, plus one whose `project_memory`
   is `Available` with zero rows. Run one HARD pass each.
2. Assert the frozen m0 bytes contain no `<project-memory>` element in every
   case, and that `meta.project_memory` (and the response field) is
   `Withheld { state }` with the verdict's state key for the three withheld
   cases and `Canonical { .. }` for the empty case.
3. Assert the withheld record is not equal to the empty record.

`withheld_canonical_read_composes_no_block_and_differs_from_empty_memory`
(`transform.rs:13282`) constructs exactly this at the injected-context seam;
`a_lagging_consumer_withholds_the_block_and_acknowledging_restores_it`
(`tests/transform_canonical_memory.rs`) reaches the withheld arm through the
reader itself, with a registered consumer trailing the published outbox by the
position threshold, and then shows the block return once the consumer
acknowledges.

For the historian, the same three steps run over `HistorianAssemblerConfig`
and the fired `HistorianDiagnostics`:
`a_withheld_memory_read_renders_no_block_and_records_its_verdict`
(`historian_chunk.rs:1369`) assembles withheld, empty, served, and gated-off
reads at the assembler seam;
`a_withheld_historian_memory_read_is_recorded_and_differs_from_an_empty_block`
(`lib.rs:28494`) fires the historian through the handler with the kernel store
still opening (`Withheld { state: "unavailable:store_starting" }`) and again
with an open, empty store (`Canonical { .. }`), and asserts the two records
differ while neither captured prompt carries the block; and
`historian_prompt_composes_project_memory_from_canonical_rows`
(`lib.rs:28370`) commits verified, quarantined, retired, superseded, and
other-project decisions to a real kernel store and checks the captured prompt
carries only the verified row, with the diagnostics pinning `known_as_of` to
the store tip.

## Investigation log

### Q: Does a withheld read on a defer or soft pass leave a stale record?

- Sources examined: the three `meta.project_memory` writers; `apply_once` and
  `apply_additive_only` plan arms.
- Findings: only HARD arms write the record, and the record describes the m0
  bytes frozen by that HARD. A defer pass with a withheld read leaves the
  previous HARD's record in place, which is the record of the bytes still
  served. The m1 revision signal of that pass hashes `None`, so the next
  eligible pass folds to a served m0 without the block and writes `Withheld`.
- Missing evidence: none.
- Conclusion: resolved with answer; the record always describes the served
  bytes.

### Q: Is `Canonical { known_as_of: 0, .. }` reachable in production?

- Sources examined: `read_project_memory` (`canonical_memory.rs`), which reads
  `store.tip()` before the lag sample and passes it to `read_visible` as
  `as_of`; `KernelStore::tip`
  (`crates/kernel/src/envelope.rs:569-575`, `COALESCE(MAX(commit_seq), 0)`).
- Findings: a kernel store with no commits reports `tip() == 0`, so a served
  read of an empty store records `Canonical { known_as_of: 0, truncated: false,
  .. }`. That is a truthful record of an empty canonical snapshot and is still
  distinct from every `Withheld` record by its serde tag.
- Missing evidence: none.
- Conclusion: resolved with answer; reachable, and correctly classified as
  served-empty.
