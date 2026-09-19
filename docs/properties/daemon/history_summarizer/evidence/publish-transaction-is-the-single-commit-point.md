# publish-transaction-is-the-single-commit-point

## Discovery trigger

The task asked for the commit point precisely: the single operation after which
the substitution is visible and irreversible. Tracing that question backwards
from `publish_validated_chunk` showed that the substitution is four writes, not
one: the history_segment rows, the chunk transcripts, the queued side channels,
and the `cache_state` row `UPDATE` that carries the raised floor, the idle
producer state, and the nonadmission facts. The interesting property is not
where the commit is but what is inside it, because separate writes would land
in separate transactions and this set lands in one.

## Evidence trail

The publish path in `crates/daemon/src/history_summarizer.rs`:

- `:418-434` `publish_validated_chunk` re-checks the pinned fingerprint against
  the observed one and abandons the matching firing before returning a mismatch.
- `:436-485` projects validated history_segments, events, primer candidates, and
  optionally user observations onto store row shapes.
- `:487-500` builds `HistorySummarizerPublishRequest`, always with
  `chunk_transcript: Some(...)`, and carries the caller's
  `memory_reviewer_nonadmission` code.
- `:501-504` calls the publication fence when present, otherwise
  `store.publish_history_summarizer_chunk` directly.

The transaction in `crates/memory-store/src/lib.rs`:

- `:11199` `write.execute(&self.inner, |coordinated| { ... })` opens it through
  the prepared-write wrapper.
- `:11202-11223` reads `(row_version, meta)` and applies the row-version CAS.
- `:11225-11248` deserializes meta, checks the phase, checks the five-field
  predicate.
- `:11254-11266` the block-identity content fence.
- `:11268-11275` the revert-epoch check.
- `:11277-11296` re-reads `(MAX(sequence), COUNT(*))` and checks it against the
  pinned history_segment-set generation.
- `:11299-11303` raises `meta.publication_floor_ordinal` with a `max`.
- `:11304` `meta.history_summarizer = meta.history_summarizer.cleared_of_in_flight_firing()`,
  the idle state that keeps the sequence and the nonadmission facts.
- `:11305-11312` increments the nonadmission count and binds the latest reason
  to the publishing `firing_seq` when the request carries a code.
- `:11315-11325` serializes and secret-scans the metadata. This runs before any
  row is appended so that a serialization failure writes nothing; the serialized
  metadata depends on nothing the appends produce.
- `:11327` `first_appended_sequence = next_history_segment_sequence_tx(...)`.
- `:11328-11341` `append_history_segments_tx`, write 1. It validates the whole
  batch before its first insert, so an overlap return writes nothing.
- `:11342-11350` `insert_chunk_transcripts_tx`, write 2.
- `:11351` `enqueue_history_summarizer_side_channels_tx`, write 3.
- `:11353-11357` `UPDATE cache_state SET row_version = next, meta = ...
  WHERE session_id = ?1 AND row_version = ?4`, write 4, which carries the floor,
  the idle state, and the nonadmission facts in one row.
- `:11359-11362` returns `PublishTxnOutcome::Committed`.
- `:11372-11378` after the transaction, drains the queued side channels best
  effort. Failures stay queued for a later transform, per the comment at
  `:11372-11373`.

Correction: an earlier revision of this trail cited the transaction at
`:9360-9517`, named `idle_history_summarizer_after_success`, and listed the floor
and state updates as writes 4 and 5 after the side-channel enqueue. That helper
no longer exists, the metadata is now computed and serialized before the first
append, and the row `UPDATE` is the only write that carries it. The line
numbers above are verified against HEAD.

The wrapper, outside this repository, at
`../commons/crates/storage/src/lib.rs` (source-catalog path, not present at HEAD):

- `:185-192` takes an `Immediate` transaction.
- `:194-227` ensures and checks the writer-epoch fence table, rejecting when a
  newer writer owns the database (`:211-218`) and claiming it otherwise
  (`:219-227`).
- `:229` runs the closure.
- `:230-232` `tx.commit()`, then returns.

Every early return inside the closure returns a `PublishTxnOutcome` value rather
than an error, so the transaction commits in those cases too. That is deliberate
for `Committed` and harmless for the rejection variants, which write nothing.

Confirmation that the transaction touches no render state: the store's doc at
`:11163-11168` states it "intentionally leaves render state (`CoreState`,
`coverage_ordinal`, watermarks, and m1 revision) untouched", matching the module
side at `history_summarizer.rs:415-417`. I read the closure body and found no write
outside the four listed above.

## Failure scenario

Suppose the history_segment append at `:11328` committed but the row-version bump at
`:11353` did not. The session would hold a model-generated summary row while
`meta.history_summarizer` still said `Publishing` and the publication floor had not
moved. The next `handle_restart_load` would see `Publishing`, abandon, and make
the session refire-eligible (`history_summarizer.rs:592-629`, the `Publishing` arm at
`:620-627`). The refire would assemble
a chunk starting past `MAX(end_message)` (`history_summarizer_chunk.rs:635-648`), which
now includes the orphaned history_segment, so the range would not be re-summarized.
The orphaned history_segment would fold the range while the floor stayed behind it,
and the boundary logic would treat those ordinals as still eligible for
placement (`boundary.rs:448-452`, which raises the protected tail to
`prior_boundary_ordinal` only while `publication_floor_active`). The symptom is a fold whose floor does not
protect it, not lost content.

The reverse partial, floor raised without history_segments appended, is worse: a
range with no summary and no boundary eligibility.

## Timing windows and dependencies

The window is the interior of one `Immediate` SQLite transaction on a local
file, so it is short in wall-clock terms and only a process kill or a SQLite
error can land in it. Dependencies:

- SQLite's own atomicity, and the `Immediate` behaviour chosen at
  `../commons/crates/storage/src/lib.rs:191` (source-catalog path, not present at HEAD).
- The single-writer lease acquired before the file is opened (`:265-277`), plus
  the per-transaction epoch fence (`:211-218`), which together are what stop a
  second process from interleaving.
- Durable pragmas, which `open_sqlite` claims to set (`:266`). I did not read
  the pragma list.

## What a test must construct

A configured model chain, a fired run driven to `Publishing`, and a fault inside
the transaction. The blocker is that no such seam exists. The
`#[cfg(test)] after_store_publish` hook (`lib.rs:3432-3433`, fired at
`:3453-3461`; its reattach twin at `:3470-3471` and `:3491-3499`) runs after `store.publish_history_summarizer_chunk` has returned, so it is
outside the window by construction. Options, cheapest first:

1. Assert the post-condition conjunction after a successful publish and after
   each rejection variant, which does not test atomicity but does pin the
   write set so a future change that drops one is caught.
2. Add a `#[cfg(test)]` hook inside the closure, between the append and the
   row-version bump, that returns a `rusqlite::Error`. That exercises rollback,
   which is the achievable half of the property.
3. A SIGKILL harness driving the real binary, which is the only way to test the
   process-death case. Expensive and belongs with the crash-consistency work
   rather than here.

## Investigation log

### Q: Is there any intended fault-injection seam inside the publish transaction, or is the wrapper's atomicity taken on faith?

- Sources examined: `crates/memory-store/src/lib.rs:9351-9546` (the publish transaction at that revision; now `:11163-11378`); every `#[cfg(test)]`
  and `cfg(feature =` occurrence in `crates/memory-store/src/lib.rs`;
  `crates/daemon/src/lib.rs:3426-3503` for the two publication fences;
  every `cfg(feature =` occurrence in `crates/daemon/src/lib.rs` (all
  `test-support`; the former `drive-fault` feature block was deleted in
  `06653606`).
- Findings: the only test hook near the publish is
  `after_store_publish`, which fires after the store call returns. No
  `drive-fault` feature exists at HEAD, and no `cfg(feature)` item in
  `crates/daemon/src/lib.rs` reaches the store. No
  `#[cfg(test)]` branch exists inside the publish closure.
- Missing evidence: whether the sibling `storage` crate offers a fault
  hook. Its `with_conn_fenced` at `../commons/crates/storage/src/lib.rs:185-232` (source-catalog path, not present at HEAD)
  has none, and that crate is outside this repository so changing it is not in
  scope for Part 4a.
- Conclusion: unresolved, needs a decision on whether a store-level fault seam
  is worth adding. The atomicity is taken on faith today. Recording it as
  `Exercised: partial` with the two existing transcript tests is the honest
  label.
