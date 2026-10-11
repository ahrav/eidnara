# retrieval-reads-no-payload

## Discovery trigger

Ticket #391, acceptance criterion AC7: "Authorization precedes payload
loading; denied requests load no payload", together with AC2's "Probe results
contain IDs, raw rank, and tie identity, never payload bytes." The RP2.3
property bundle is unavailable in this repository, so the catalog
reconstructs the record from the ticket and verifies it against the #391
branch. PR #967 lists AC7 as met by an existing test and mapped in the
catalog.

## Evidence trail

Contract:

- `crates/retrieval/AGENTS.md:15` through `:17`: `lexical::retrieve` reads
  `lexical`, `occurrences`, and `occurrence_tombstones` only; the caller loads
  bytes after its own authorization step. This is the documented obligation,
  a claim under test, and the code below is the evidence for it.

Production code, `crates/retrieval/src/lexical/retrieve.rs`:

- Every statement the lane runs is one of the six constants at `:256`
  `COUNT_SQL`, `:259` `RANKED_SQL`, `:264` `WHOLE_SQL`, `:270` `COMMON_SQL`,
  `:276` `IDS_SQL`, and `:280` `DETAILS_SQL`. Their `FROM` and `JOIN` clauses
  name `lexical`, `occurrences`, `occurrence_tombstones`, and `json_each`.
  The selected columns are `rank`, `rowid`, `occurrence_id`, `class`,
  `source_object_id`, `revision`, `source_artifact_digest`, and the tombstone
  predicate. No statement names `payloads`, `payload_id`, or `bytes`, and none
  selects the FTS5 `original` or `parts` text columns.
- `:59` `Contribution` carries `occurrence_id`, `class`, `rank`, `ordinal`,
  and the kernel `candidate`. `:131` `Retrieval` carries contributions,
  completion, reasons, snapshot, incarnation, and `consumed`. Neither type
  has a byte field.
- `:317` `retrieve` is the lane entry; the daemon's `lexical_read`
  (`crates/daemon/src/query_route.rs:1075`) calls `scan` (`:349`) and
  `admit_lexical` (`query_route.rs:1242`) calls `admit`.

Schema, `crates/retrieval/baseline.sql`:

- `:30` `payloads(payload_id, bytes, byte_length, created_at)` holds the
  bytes; `:49` `occurrences.payload_id` references it. `:94` `lexical` is the
  FTS5 table with `occurrence_id UNINDEXED`.

Payload loading after authorization, `crates/daemon/src/packing/mod.rs`:

- `:269` `fetch_payload` is the one read keyed by a `PayloadRef`. In
  `prepare_required` (`:351`) the kernel judges the selected occurrences at
  `:397` through `judge_occurrences_within_budget`, `admit_required` filters
  on the verdicts, and only then `:421` fetches bytes for admitted items.
  `crates/daemon/src/query_route.rs:1` states the route reads no payload bytes
  and packing materializes them under its own bounds.

Test, `crates/retrieval/tests/lexical_retrieval.rs:1275`
`retrieval_reads_no_payload_bytes`:

- `Fixture::all_admitted` (`:317`) opens a kernel and a projection store,
  installs an identity, and projects the corpus through `apply_batch` with
  `Payload::Whole` rows, so the `payloads` table is populated.
- The test retrieves `parse fetch io` with `bounds()` (`:153`: `scan_rows`
  64, `max_accepted` 64, `batch_rows` 2) and an unbounded budget, then opens a
  raw connection (`raw`, `:496`) and runs
  `ALTER TABLE payloads RENAME TO payloads_hidden`. It asserts
  `SELECT count(*) FROM payloads` now errors, so any payload read would fail
  rather than return zeroed bytes.
- It retrieves again and asserts `after == before` on the whole `Retrieval`
  value, with six contributions.

## Failure scenario

A lexical statement that joins `payloads`, or a contribution that carries
bytes, loads payload content before the kernel judges the occurrence. A
request the kernel denies, or one whose claims fail final revalidation, has
already read the bytes it must not see, and the data leaves the authorization
boundary through the retrieval result.

## Timing windows and dependencies

None. The guarantee is structural: the lane's statements name no payload
table and its result type has no byte field, so no interleaving changes the
outcome. The ordering dependency sits in packing, where judgment at
`packing/mod.rs:397` precedes `fetch_payload` at `:421` in straight-line code.

## What a test must construct

A projection with populated `lexical`, `occurrences`, and `payloads` tables; a
request with several matching probes; a fault that makes any payload read
fail loudly, here the `payloads` table renamed away; a baseline retrieval
before the fault and an equality assertion on the full result afterward.

## Investigation log

### Q: Is the test exercised against a real projection schema?

- Sources examined: `crates/retrieval/tests/lexical_retrieval.rs:243`
  through `:316` (`Fixture`, `Fixture::new`), `:346` (`Fixture::project`),
  `:109` (`open_store`).
- Findings: the fixture opens a `SqliteStore` through `open_sqlite`, installs
  a `ProjectionIdentity`, and writes rows with `apply_batch`, the production
  batch path. The `ALTER TABLE payloads` statement succeeds, which proves the
  table exists in the fixture's schema.
- Missing evidence: none for this question.
- Conclusion: resolved with answer - a real projection, populated through the
  production batch path.

### Q: Does the route read payload bytes anywhere before packing?

- Sources examined: `crates/daemon/src/query_route.rs:1`, `:69` through
  `:71`, `:1708` through `:1722`; `crates/retrieval/src/claims.rs:441`
  `read_selected_claims`; `crates/kernel/src/claim_facts.rs:43` and `:427`.
- Findings: the route's module doc states no payload read. Claim validation
  passes `max_causal_payload_bytes` of 1 MiB into `read_selected_claims`,
  and the kernel's `claim_facts` applies that bound at `:427` when it reads
  the causal decision record from the kernel store, under
  `Phase::ClaimValidation`. That is a kernel read of canonical decision
  payload, bounded in bytes, after `Phase::Revalidation` has judged every
  fused entry. The projection `payloads` table is untouched on the route.
- Missing evidence: a test that denies a claim and asserts the causal
  record read does not run for it.
- Conclusion: resolved with answer for the lexical lane; the claim-facts
  causal read is a kernel-side, post-judgment read outside this record.
