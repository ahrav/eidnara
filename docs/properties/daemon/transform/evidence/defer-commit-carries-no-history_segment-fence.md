# defer-commit-carries-no-history_segment-fence

## Discovery trigger

`commit_transform` can fence a cache-state commit against the current maximum
history_segment sequence. The transform supplies that fence only for a non-subagent
provider-prefix mutation pass, while a Defer can still write a
history_segment-derived watermark. This record checks whether that asymmetry creates
a reachable production race.

## Provenance

All citations were verified on 2026-09-01 against Magic source commit
`af5e153c12750354a82f91bc796367031ac5c658` plus the current companion U6 diff.
This record does not depend on the sibling commons worktree.

## Evidence trail

### The store predicate

`MemoryStore::commit_transform` starts at
`crates/memory-store/src/lib.rs:6903-6907`. Inside one fenced transaction it checks:

1. the current cache-state row version at `:6992-6998`;
2. the expected row version at `:7000-7007`; and
3. the optional history_segment maximum sequence at `:7018-7027`.

A third check, the optional claim snapshot vector, sat between items 2 and 3
until the transform moved to canonical kernel rows; `TransformCommit` no longer
carries a vector and the commit performs no mirror comparison.

The history_segment check queries `MAX(sequence)` from `history_segments` and returns
a CAS conflict when it differs from the supplied value (`:7018-7026`).

### Which passes supply the history_segment fence

The module defines a provider-prefix mutation pass as `Hard`, `MigrateHard`, or
`Soft`, then restricts a bust to non-subagents
(`crates/daemon/src/transform.rs:4134-4138`). At commit it passes

```rust
history_segment_max_seq: is_bust_pass.then_some(m1_signal.max_history_segment_seq),
```

at `crates/daemon/src/transform.rs:5164-5174`. A `PassPlan::Defer` therefore
passes `None`. A subagent also passes `None`, including a subagent Soft step.
This asymmetry is verified.

### What Defer can write

The first `m1_signal` read is
`crates/daemon/src/transform.rs:3619-3634`. Boundary-divergence handling may
replace it with a revalidated read at `:3664-3680`. The module computes
`history_segment_seq_changed_since_meta` from the selected signal at `:3714-3716`.

The Defer arm is `crates/daemon/src/transform.rs:4774-4787`. After its
SoftPlus step, it writes
`meta.coverage_history_segment_seq = Some(m1_signal.max_history_segment_seq)` when the
history_segment sequence changed and the combined m1 digest still matches the loaded
revision (`:4782-4786`). That metadata change contributes to `state_changed` and
`commit_required` at `:5154-5159`.

### Production history_summarizer publication also bumps the row version

The production publication chain reaches `publish_history_summarizer_chunk` through
`publish_output_from_awaiting` and `publish_validated_chunk`:

- `publish_output_from_awaiting` persists the `Publishing` state, captures its
  returned row version, and calls `publish_validated_chunk` at
  `crates/daemon/src/history_summarizer.rs:1624-1647`;
- `publish_validated_chunk` builds `HistorySummarizerPublishRequest` and dispatches
  through an optional `HistorySummarizerPublicationFence`, otherwise directly to
  `MemoryStore::publish_history_summarizer_chunk`, at `history_summarizer.rs:431-434` and `:500-517`;
- both production fence implementations call the same store method at
  `crates/daemon/src/lib.rs:3245-3268` and `:3280-3305`.

`MemoryStore::publish_history_summarizer_chunk` runs one fenced transaction
(`crates/memory-store/src/lib.rs:8783-8798`). In that transaction it:

1. checks the expected cache-state row version (`:8798-8819`);
2. checks the history_summarizer and history_segment-set predicates (`:8821-8890`);
3. appends history_segments through `append_history_segments_tx` (`:8892-8906`); and
4. updates `cache_state.row_version` from `current` to `current + 1`
   (`:8919-8939`).

The history_segment append and row-version increment therefore commit atomically.
If publication lands after a transform reads its state but before that transform
commits, `commit_transform` observes the newer row version and rejects the stale
transform CAS. The optional history_segment predicate is not needed for this
history_summarizer interleaving.

### Search for an unfenced production append path

`append_history_segments_tx` is defined at
`crates/memory-store/src/lib.rs:11815-11862`. Its standalone public wrapper at
`:8612-8635` does not update `cache_state`; this is the verified mechanism
behind the original concern. However, every workspace caller of that standalone
wrapper is test-only: store tests begin at `memory-store/src/lib.rs:13301-13302`,
history_summarizer tests at `daemon/src/history_summarizer.rs:1731-1732`, transform tests at
`daemon/src/transform.rs:11924-11925`, and module tests at
`daemon/src/lib.rs:15516-15517`.

The other direct history_segment insertion sites are state import
(`memory-store/src/lib.rs:6799-6843`) and whole-set replacement (`:8345-8365`), not
history_summarizer append callers. No production call path was found that publishes a
history_summarizer history_segment through the standalone append wrapper.

### Later consumers of the watermark

`history_segment_revision_matches` compares the applied sequence with the current
signal at `crates/daemon/src/transform.rs:3682-3687`.
`boundary_divergence_recut` uses that result at `:3705-3710`. The pending-pass
limit is three (`:78-82`). The next pass recomputes
`history_segment_seq_changed_since_meta` at `:3714-3716`.

These are real mechanisms, but production history_summarizer publication cannot create
the stale committed watermark proposed by the original scenario because its row
version bump rejects the stale transform first.

## Failure classification

The proposed defer-versus-history_summarizer race is unreachable in the production call
graph and is therefore vacuous. The earlier failure sequence incorrectly treated
production history_summarizer publication as the standalone `append_history_segments` path.
It has been removed.

This conclusion is limited to the verified production callers. A future
production caller of standalone `append_history_segments` would reopen the question
because that wrapper does not bump the cache-state row version.

## Timing windows and dependencies

The transform still has a source-level interval between its selected signal read
(`transform.rs:3619-3680`) and `commit_transform` (`:5164-5174`). A production
history_summarizer publish may commit during that interval, but its atomic row-version
increment turns the interval into an ordinary stale-CAS retry, not a stale
watermark commit.

## What a test must construct

No test should assert that a production history_summarizer publish lets the stale Defer
commit succeed. That expected result is false.

No failure construction remains for this proposed race. Verification belongs in
the source call-graph and transaction evidence above; using the test-only
standalone `append_history_segments` wrapper would model a different path.

## Investigation log

### Q: Is the history_segment predicate present on Defer?

- Sources examined: `transform.rs:4134-4138`, `:4774-4787`, `:5164-5174`;
  `memory-store/src/lib.rs:6992-7027`.
- Finding: no. Defer supplies `None` for `history_segment_max_seq`.
- Missing evidence: none.

### Q: Does production history_summarizer publication evade the row-version CAS?

- Sources examined: `history_summarizer.rs:431-517`, `:1624-1647`;
  `daemon/src/lib.rs:3245-3305`; `memory-store/src/lib.rs:8783-8940`.
- Finding: no. HistorySegment append and row-version increment share one fenced
  transaction, so a stale transform CAS rejects.
- Missing evidence: none.

### Q: Is there another production append caller that omits the row-version bump?

- Sources examined: every workspace call to `append_history_segments`, all
  `insert_history_segment_tx` sites, and the production history_summarizer publication chain.
- Finding: no. Standalone append callers are tests; import and replacement are
  different operations.
- Missing evidence: external callers outside this workspace are not visible.
