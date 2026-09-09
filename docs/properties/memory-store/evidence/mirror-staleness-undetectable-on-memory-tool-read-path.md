# mirror-staleness-undetectable-on-memory-tool-read-path

## Discovery trigger

When this record was discovered, three production functions read committed mirror
claims; two bracketed the read with a snapshot-vector comparison and bailed out on
any mismatch. Those two have since moved to canonical kernel rows. The remaining
one, `list_committed_claims`, reads `claim_mirror_state()` only to extract an
incarnation string and then lists claims with no comparison at all.

## Evidence trail

**The unfenced path.**

```
57 pub fn list_committed_claims(
58     store: &MemoryStore,
59     public_claim_ids: &BTreeSet<String>,
60     category: Option<&str>,
61     limit: usize,
62 ) -> Result<Vec<CommittedClaimMirrorRow>, MemoryToolError> {
63     let Some(state) = store.claim_mirror_state()? else {
64         return Ok(Vec::new());
65     };
66     Ok(store
67         .list_claim_mirror(&state.database_incarnation_id, None)?
```

(`crates/daemon/src/memory_tool.rs:57-67`.) The signature takes no expected
vector, so the function could not compare even if it wanted to. `state` is consumed
solely for `database_incarnation_id`. Everything after `:67` is filtering by claim
ID, category, and limit (`:68-80` onward). There is no second state read and no
freshness test.

**There is no fenced path left, for contrast.**

The transform and historian mirror reads that once bracketed
`list_claim_mirror` with a snapshot-vector comparison were deleted when both
moved to canonical kernel rows (`crates/daemon/src/canonical_memory.rs`), and the
commit-time comparison in `MemoryStore::commit_transform` that re-read the vector
inside the fenced transaction and converted a mismatch into
`CommitOutcome::CasConflict` was deleted with them. `memory_tool.rs:67` is the
only production caller of `list_claim_mirror`.

`claim_mirror::snapshot_vector_from_connection` (`claim_mirror.rs:806-840`) is
still defined, but its one remaining caller is `replace_claim_mirror_snapshot`
(`claim_mirror.rs:980`), which compares the stored vector against an incoming
seed's vector to decide whether the seed is an idempotent replay of what is
already stored. That is a writer-side replay check on the seed path, not a
freshness test on a read, so it does not fence any consumer of claim rows.

**There is no other freshness signal available.** `claim_mirror_state` carries
`updated_at_ms` (`crates/memory-store/baseline.sql:770`), written on seed
(`claim_mirror.rs:1021-1026`) and on every receipt (`:1374-1376`). No `SELECT`
anywhere in the tree retrieves it: the statements that read `claim_mirror_state`
project `vector_version, database_incarnation_id, workspace_epoch`
(`claim_mirror.rs:811-812`),
`mirror_version, vector_version, database_incarnation_id, workspace_epoch`
(`:878-880`), `database_incarnation_id` alone (`:962`, `:1422`), and
`database_incarnation_id, workspace_epoch` (`:1128-1129`). So age is written and
never read, and `ClaimMirrorState` (`:188-199`) has no timestamp field to expose. A
caller cannot ask how old the mirror is even if it wanted to.

**And the mirror can genuinely fall arbitrarily behind.** Every admission check in
`apply_claim_mirror_receipt` refuses the whole receipt on failure: project-set
mismatch (`:942-957`), generation mismatch (`:963-990`), checkpoint mismatch
(`:992-1006`), and the per-claim guards (`:1008-1050`). A refused receipt leaves the
mirror at its previous position, and because the source's next receipt chains from a
position the mirror never reached, every subsequent receipt is refused too. The lane
wedges at a fixed, self-consistent, arbitrarily old state — and
`mirror-reset-cycle-requires-a-rebuild-grant` shows production cannot reseed out of
it.

**Reachability.** `memory_tool.rs:57` is production: the `#[cfg(test)] mod tests` in
that file begins at `:361-362`, well below. `transform.rs` and `historian_chunk.rs`
paths are likewise production. So all four read shapes are default-production, and
the unfenced one is not gated on configuration.

## Failure scenario

1. Receipt 9 is refused for any of the reasons above — say a `CheckpointMismatch`
   after the double-apply in `mirror-receipt-replay-applies-effects-once`.
2. The mirror stops advancing. Its state row, project rows, and claim rows remain
   internally consistent and pass every validation, so nothing looks broken.
3. The transform and historian compose their memory surfaces from canonical
   kernel rows (`crates/daemon/src/canonical_memory.rs`) and never consult the
   mirror, so they keep serving current memory.
4. `list_committed_claims` continues to return the frozen claim set, indefinitely,
   with no error and no signal to its caller.

So the system degrades inconsistently: the prompt surfaces read the authority and
stay current, while the tool surface reads the mirror and presents stale memories
as current. Nothing in the mirror emits a signal when it stops advancing, so a
wedged mirror produces no error anywhere, only staleness on the one surface that
still reads it.

## Timing windows and dependencies

- No bounded window. The staleness is unbounded in both duration and magnitude,
  because nothing prunes, expires, or ages the mirror and `updated_at_ms` is
  unreadable.
- Depends on the wedge being reachable at all, which
  `mirror-reset-cycle-requires-a-rebuild-grant` establishes: without a reseed path,
  a refused receipt is permanent rather than transient.
- Independent of `mirror-read-fence-relies-on-generation-advance`, which was about
  whether the deleted fenced paths' comparison was *sound* and is now invalidated.
  This record is about a path with no comparison to be sound or unsound.

## What a test must construct

1. Enumerate the read surface as a structural assertion rather than a runtime one.
   For every production call site of `list_claim_mirror`, assert the enclosing
   function either accepts a `SnapshotVector` parameter or is annotated as
   staleness-tolerant. On the current tree that enumeration is one site,
   `memory_tool.rs:67`, and it fails. This is the cheapest oracle and needs no
   fault injection.
2. Behavioural version. Seed a mirror and apply receipt 8. Then submit receipt 10,
   skipping 9, and assert it is refused with `CheckpointMismatch`, leaving the mirror
   at receipt 8's state.
3. With the mirror wedged at 8, run a transform pass and assert its
   `<project-memory>` block is composed from canonical rows and does not change
   (`tests/transform_canonical_memory.rs` covers the canonical path).
4. Call `list_committed_claims` and assert it returns the receipt-8 claim set. That
   is the finding: the same wedged mirror leaves the prompt surfaces current and
   the tool surface confidently stale.
5. Assert there is no way for the caller to detect it: confirm `ClaimMirrorState`
   (`claim_mirror.rs:188-199`) exposes no timestamp and no source position beyond
   `acked_effect_id`, and that `acked_effect_id` alone is meaningless without the
   authority's position, which this store never holds.
6. Do not write a coverage marker pairing `always(!stale)` with `sometimes(stale)`.
   The independent preconditions to assert are: a receipt was refused, and a read
   occurred through an unfenced call site.

## Investigation log

### Q: Is `list_committed_claims` a tool-facing read whose caller already accepts lag?

- Sources examined: `memory_tool.rs:57-67` and the filtering that follows,
  `:361-362` (the test module boundary, confirming production reachability), and
  the now-deleted transform and historian mirror reads, which received their
  expected vector from a lane configuration the caller already held.
- Findings: the signature cannot check. `list_committed_claims` takes claim IDs, a
  category, and a limit, and nothing that could serve as a freshness reference. So
  either the caller is expected to tolerate lag, or the parameter is missing. The
  function's behaviour is consistent with the first reading; the deleted readers'
  behaviour was consistent with the second.
- Missing evidence: the tool-surface contract for this call, and whether its result
  is presented to a user as current.
- Conclusion: needs human input.

### Q: Is `updated_at_ms` read anywhere, giving a fallback freshness signal?

- Sources examined: every reference to `claim_mirror_state` in the tree:
  `claim_mirror.rs:811-812`, `:878-880`, `:962`, `:1021-1026`, `:1128-1129`,
  `:1374-1376`, `:1422`, and the schema at `baseline.sql:763-771`. Also
  `ClaimMirrorState` (`claim_mirror.rs:188-199`) for an exposed field.
- Findings: written in two places, projected by none. The struct has no timestamp
  field, so even if a statement selected it there would be nowhere to put it.
- Missing evidence: none.
- Conclusion: resolved with answer — no. `updated_at_ms` is write-only and cannot
  serve as a freshness signal without a schema-adjacent code change.
