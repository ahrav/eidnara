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

Verified at the commit that introduces the reader.

- `crates/daemon/src/canonical_memory.rs`: `CanonicalMemoryRead` has two
  variants, `Available(CanonicalMemorySnapshot)` and `Withheld(KernelOutcome)`
  (`:55-60`). `composition()` (`:64-75`) maps `Available` to
  `ProjectMemoryComposition::Canonical { known_as_of, truncated, revision }` and
  `Withheld` to `ProjectMemoryComposition::Withheld { state }` with
  `state = verdict.state_key()`. `revision()` (`:78-83`) is `None` when
  withheld. `rows()` (`:86-91`) returns the snapshot rows or an empty slice.
  Each is a single `match` with no default arm.
- `crates/daemon/src/canonical_memory.rs:99-125` (`read_project_memory`): the
  store phase (`KernelOpenCoordinator::kernel_store`), the `outbox_lag` read,
  the serving decision `serving::project(serving::decide_for_tip_read(&lag),
  Surface::AutoInject)`, and the `read_visible` call each return `Withheld`
  carrying a `KernelOutcome`; only a served read constructs `Available`.
- `crates/daemon/src/kernel_routes/serving.rs:58-63` (`decide_for_tip_read`):
  zero registered consumers is served; otherwise `decide` applies the route's
  lag thresholds. So in a deployment with no outbox consumer, the withheld arm
  is reached only through store phase or read errors.
- `crates/daemon/src/kernel_routes/state.rs` (`KernelOutcome::state_key`):
  the recorded reason is the serde `kind` tag joined with the reason by `:`,
  the same vocabulary the TypeScript client indexes guidance by.
- `crates/memory-store/src/lib.rs:1320-1340` (`ProjectMemoryComposition`):
  `#[serde(tag = "kind")]` with variants `canonical` and `withheld`, so the two
  records differ on the wire and in the durable blob, not only in Rust.
- `crates/daemon/src/transform.rs:2597`, `:4243`, `:4415`: every HARD arm
  (compaction-off, compaction-on, and the re-cut arm) writes
  `meta.project_memory = Some(ctx.project_memory.composition())` next to the
  frozen m0 bytes, inside the same `TransformCommit`.
- `crates/daemon/src/transform.rs:1723-1730` (`compose_m0_for_context`) and
  the additive path pass `ctx.project_memory.rows()` to the renderer, so a
  withheld read renders no block because it has no rows, not because a
  separate flag suppresses it.
- `crates/daemon/src/lib.rs:8007`: the read is taken once per pass, before the
  `run_transform` closure, and every attempt of that pass clones the same value
  into its `ProducerContext`, so the m1 revision signal, m0, and additive m0
  compose from one snapshot. The historian takes its own read
  (`lib.rs:4831`) only when memory is enabled; a withheld verdict renders no
  block and is logged with its state key (`historian_chunk.rs`,
  `assemble_historian_firing`), so summarization continues through a kernel
  outage while the log keeps the reason.

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

- The window between the read (`lib.rs:8007`) and the commit is closed to the
  verdict: the read result is a value in the context, and a phase or lag change
  after it cannot alter what the pass records.
- The property depends on the two-variant enum staying exhaustive; a third
  variant fails to compile at both `match` sites.
- The distinction reaches the m1 revision signal through
  `ProjectMemoryComposition::revision()` (`None` when withheld), so a served
  read with rows changes the external revision and forces the HARD that
  rematerializes m0 (`transform.rs:13322` pins this).

## What a test must construct

1. A `ProducerContext` whose `project_memory` is `Withheld(verdict)` for each
   of `Stale`, `Abstained`, and `Unavailable`, plus one whose `project_memory`
   is `Available` with zero rows. Run one HARD pass each.
2. Assert the frozen m0 bytes contain no `<project-memory>` element in every
   case, and that `meta.project_memory` (and the response field) is
   `Withheld { state }` with the verdict's state key for the three withheld
   cases and `Canonical { .. }` for the empty case.
3. Assert the withheld record is not equal to the empty record.

`withheld_canonical_read_composes_no_block_and_differs_from_empty_memory`
(`transform.rs:13258`) constructs exactly this at the injected-context seam;
`a_lagging_consumer_withholds_the_block_and_acknowledging_restores_it`
(`tests/transform_canonical_memory.rs`) reaches the withheld arm through the
reader itself, with a registered consumer trailing the published outbox by the
position threshold, and then shows the block return once the consumer
acknowledges.

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

- Sources examined: `read_visible` (`kernel_routes/read.rs`), which reads at
  `store.tip()` when `as_of` is `None`; `KernelStore::tip`
  (`crates/kernel/src/envelope.rs:569-575`, `COALESCE(MAX(commit_seq), 0)`).
- Findings: a kernel store with no commits reports `tip() == 0`, so a served
  read of an empty store records `Canonical { known_as_of: 0, truncated: false,
  .. }`. That is a truthful record of an empty canonical snapshot and is still
  distinct from every `Withheld` record by its serde tag.
- Missing evidence: none.
- Conclusion: resolved with answer; reachable, and correctly classified as
  served-empty.
