# p5dep-baseline-digest-change-closes-the-activation-gate

HEAD `3a3b916b7`. Surface S5.

## Discovery trigger

Plan KTD3 and U3: the `contradiction_candidates` table is a memory-store
baseline change; index D4 batches it into Batch M1. `crates/storage/AGENTS.md`:
"Preserve single-baseline storage. Reject schema mismatches; do not add
migrations or a version ledger." `docs/memory-reviewer-operations.md:86-88`,
`:234-240` describe the operational consequence. Lenses: version
compatibility; lifecycle transitions; C11.

## Evidence trail

- `crates/memory-store/src/lib.rs:333-335`: `BASELINE =
  include_str!("../baseline.sql")`, "The complete schema, applied once to a
  pristine file. The bytes of the file are the store identity
  `storage::open_sqlite` checks on every open."
- `lib.rs:339-342` `baseline_digest()` = `storage::baseline_digest(BASELINE)`:
  "the digest of the storage crate's baseline followed by this crate's, not of
  this crate's alone."
- `crates/storage/src/lib.rs:2630-2631`: "The lowercase SHA-256 of
  `STORE_BASELINE`, a newline, and `consumer`. `open_sqlite` verifies each
  file against this digest; fresh files store it in `format_marker`."
- `storage/src/lib.rs:2846`, `:2884-2890`: `format_marker` read and write.
- `lib.rs:8157-8158` `MemoryStore::open` -> `open_sqlite(descriptor,
  BASELINE)`.
- `crates/memory-store/tests/baseline.rs:250-262`
  `the_published_baseline_digest_is_the_one_every_open_checks`; `:458`
  `a_file_with_a_different_baseline_is_refused_as_a_baseline_mismatch`
  (message contains "does not match the baseline digest"); `:484-514`
  frozen `PRIOR_BASELINE` fixture with digest `45baed0c...5f36`, refused as
  `StoreError::Baseline`, file bytes unchanged.
- Activation gate: `crates/daemon/src/memory_reviewer/activation.rs:37`,
  `:66` `memstore_baseline_digest: String` on record and live identity;
  `:174-200` `identity_terms` in comparison order (prompt template, step
  schema, scanner ruleset, egress policy, kernel baseline, memory store
  baseline, kernel incarnation, memory store incarnation, provider);
  `:218-240` `evaluate`: `Malformed` checks, then the first differing term
  -> `Closed::IdentityMismatch(field)`.
- `crates/daemon/src/memory_reviewer/worker.rs:167`: live
  `memstore_baseline_digest: memory_store::baseline_digest()`.
- `activation.rs:491-495` test table: `"memstore_baseline_digest"` changed ->
  `IdentityMismatch("memory store baseline")`.
- `docs/memory-reviewer-operations.md:44`: "Every term is compared against
  the live deployment on every pass and again before every job. Any mismatch
  closes the gate"; `:86-88`: a prior usage-column change "changed the Memory
  Store baseline digest. A database created under the previous baseline
  refuses to open under this binary, and the previous activation record fails
  closed on `memstore_baseline_digest` until the deployment owner writes a
  renewed record"; `:234-240` the three-step reset and its consequences
  (state discarded, new incarnation, old holds unadoptable).
- Existing catalog: `docs/properties/shared-primitives/catalog.md:676`
  `store-schema-identity-matches-the-baseline` (active, `always`,
  default-production) covers `open_sqlite`'s refusal for any consumer
  baseline, with bytes unchanged. `docs/properties/memory-store/catalog.md:717`,
  `:761`, `:819` are invalidated migration-era records whose invalidation
  notes state the single-baseline fact.

## Failure scenario

If `baseline_digest()` excluded part of the baseline text (for example a
trailing table), a store at the old baseline would open under a binary whose
code expects `contradiction_candidates` and fail at first query. If the gate
compared a build identity instead of the digest, a binary that changes the
baseline without a version bump would keep the gate open against a store it
cannot open; the gate binds the digest precisely to avoid this
(`operations.md:44`).

## Timing windows and dependencies

Deployment time. `evaluate` runs on every reviewer pass and before every job.

## What a test must construct

- The existing prior-baseline fixture test; after P5 U3 lands, the fixture
  must be refreshed to the pre-P5 baseline so the refusal is exercised
  against the new digest.
- An activation record naming the pre-P5 digest against a live identity with
  the post-P5 digest; assert `IdentityMismatch("memory store baseline")`.

## Investigation log

### Q: Does any code path migrate or adopt an old-baseline store?

- Sources examined: `storage/AGENTS.md`, `storage/src/lib.rs:2032-...`
  (`open_sqlite`), `memory-store/catalog.md` invalidation notes.
- Findings: none; `open_sqlite` classifies and refuses. The invalidated
  catalog records document the removal of the migration ladder.
- Missing evidence: none.
- Conclusion: resolved; no migration path exists.
