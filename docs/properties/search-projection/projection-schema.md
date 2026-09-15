# Search projection schema

This document freezes the field inventory of `search.sqlite`, the disposable
search projection that the retrieval crate's
[baseline](../../../crates/retrieval/baseline.sql) creates. Identity or schema
incompatibility requires a rebuild from the canonical store, not a schema
migration. A change to this inventory requires a new schema version and a
rebuild. The schema version is 6.

The [lifecycle contract](spec-traceability.md) requires staging and verifying a
complete, compatible replacement before selecting it. During replacement,
search must use a compatible old projection or return explicit unavailability.
Lifecycle state and recovery authorization belong outside the disposable
database. The persistence APIs described here do not implement this rebuild and
selection protocol.

Every table below is `STRICT` except the virtual table `lexical`, whose
FTS5 module arguments are frozen instead of a column list. Columns are listed
in declaration order; a
column is marked as primary key when it is the rowid alias, the declared
`PRIMARY KEY`, or a member of a composite `PRIMARY KEY(...)` constraint. A test
(`crates/retrieval/tests/schema_inventory.rs`) reads this document, opens a
store from the baseline, and compares every table, column, type, nullability,
and primary-key position here with `PRAGMA table_info`; a baseline that omits
or adds a field fails that comparison. Table-level constraints and indexes are
listed under each table and compared with the stored schema text; an index
entry carries its column list and, for a partial index, its `WHERE` clause.

Wire and byte contracts these fields carry are CC1 through CC5 and CC11 in
[construction-contracts.md](construction-contracts.md).

## `projection_identity`

At most one row records the projection's build identity. The baseline creates
the table without an identity row. The caller must supply the expected
`ProjectionIdentity` to `retrieval::install_identity`, which checks
`SCHEMA_VERSION` even before the first insert, inserts an absent identity, and
returns `IdentityMismatch` if any stored identity component differs.

`SearchProjection::open` checks the storage baseline and pins and verifies
connection pragmas; it does not compare this row with the expected projection
identity. Neither opening the connection nor installing the identity rebuilds
or deletes the projection, proves completeness, or authorizes serving search.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `singleton` | INTEGER | yes | yes | `CHECK(singleton=1)` |
| `schema_version` | INTEGER | yes | no | `CHECK(schema_version>0)` |
| `kernel_incarnation_id` | TEXT | yes | no |  |
| `projection_policy_version` | TEXT | yes | no |  |
| `identity_contract_version` | TEXT | yes | no |  |
| `limit_manifest_protocol_version` | TEXT | yes | no |  |
| `embedding_model` | TEXT | yes | no |  |
| `tokenizer_fingerprint` | TEXT | yes | no |  |
| `analysis_identity` | TEXT | yes | no |  |
| `vector_dimension` | INTEGER | yes | no | `CHECK(vector_dimension>0)` |
| `generation_epoch` | INTEGER | yes | no | `CHECK(generation_epoch>=0)` |
| `installed_at` | INTEGER | yes | no |  |

## `payloads`

Exact selected bytes, stored once per payload identity (CC5). Two occurrences with equal bytes share one row; unequal bytes under one digest are refused.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `payload_id` | TEXT | yes | yes |  |
| `bytes` | BLOB | yes | no |  |
| `byte_length` | INTEGER | yes | no | `CHECK(byte_length=length(bytes))` |
| `created_at` | INTEGER | yes | no |  |

## `occurrences`

One row per occurrence: the CC4 tuple bytes beside their identifier, the lineage, the payload selected, and the canonical row it came from. Sensitivity and provenance stay separate from identity; the domain is a stable identifier, never a name.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `occurrence_id` | TEXT | yes | yes |  |
| `tuple` | BLOB | yes | no |  |
| `lineage_id` | TEXT | yes | no |  |
| `class` | TEXT | yes | no | `CHECK(class IN ('messages','canonical_claims','promoted_memory','git_commits','raw_tool_spans'))` |
| `revision` | INTEGER | yes | no | `CHECK(revision>=0)` |
| `representation` | TEXT | yes | no |  |
| `span_start` | INTEGER | no | no |  |
| `span_end` | INTEGER | no | no |  |
| `payload_id` | TEXT | yes | no | `REFERENCES payloads(payload_id) ON DELETE RESTRICT` |
| `domain_id` | TEXT | yes | no |  |
| `sensitivity` | TEXT | yes | no | `CHECK(sensitivity IN ('normal','sensitive','secret'))` |
| `source_object_id` | TEXT | yes | no |  |
| `source_evidence_id` | TEXT | yes | no |  |
| `source_artifact_digest` | TEXT | yes | no |  |
| `created_commit_seq` | INTEGER | yes | no | `CHECK(created_commit_seq>0)` |
| `persisted_at` | INTEGER | yes | no |  |

Table constraints:

- `CHECK((span_start IS NULL)=(span_end IS NULL))`
- `CHECK(span_start IS NULL OR (span_start>=0 AND span_end>=span_start))`

Indexes:

- `idx_occurrences_lineage` on `(lineage_id,revision)`
- `idx_occurrences_payload` on `(payload_id)`
- `idx_occurrences_source` on `(class,source_object_id,revision)`

## `occurrence_tombstones`

An occurrence that stopped being live. The occurrence row stays so the invalidation fact keeps its identity.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `occurrence_id` | TEXT | yes | yes | `REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT` |
| `invalidated_commit_seq` | INTEGER | yes | no | `CHECK(invalidated_commit_seq>0)` |
| `reason` | TEXT | yes | no | `CHECK(reason IN ('superseded','retired','evidence_invalidated','purged'))` |
| `recorded_at` | INTEGER | yes | no |  |

## `exact_associations`

One row per exact selector key the versioned source mapping derives for an
occurrence. `family` is the selector keyword, `namespace` scopes the key
(`canonical_object` for `id`, `<object_format>:<repository_id>` for `sha`),
`key` is the normalized key bytes, and `target_id` names the logical target
the occurrence is evidence for, so alias rows of one target share it. The
composite primary key serves equality and SHA-prefix range scans in stable
binary order without a sort; the occurrence index serves physical cleanup.
`extraction_version` records the mapping the row was derived under. The exact
extraction version is 1; a new mapping requires a new schema version and a
rebuild, and both the writer and the page reader refuse rows from another
version rather than reinterpret them. Rows are written in the same transaction
as their occurrence and before the projection checkpoint advances, and are
deleted when message cleanup reclaims the occurrence.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `family` | TEXT | yes | yes | `CHECK(family IN ('id','sha','path','symbol','command','config','error'))` |
| `namespace` | TEXT | yes | yes |  |
| `key` | BLOB | yes | yes |  |
| `occurrence_id` | TEXT | yes | yes | `REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT` |
| `target_id` | TEXT | yes | no |  |
| `extraction_version` | INTEGER | yes | no | `CHECK(extraction_version>0)` |
| `created_commit_seq` | INTEGER | yes | no | `CHECK(created_commit_seq>0)` |

Table constraints:

- `PRIMARY KEY(family,namespace,key,occurrence_id)`

Indexes:

- `idx_exact_associations_occurrence` on `(occurrence_id)`

## `lexical`

One FTS5 row per live occurrence, holding the analyzer's original atoms in
`original`, its conservative parts in `parts`, and the occurrence identifier as
an unindexed column. The tokenizer, detail mode, and column layout are the
analysis contract in `docs/lexical-analysis-contract.md`;
`retrieval::lexical::fts5_table_args()` renders the same arguments, and the
projection identity's `analysis_identity` records the contract the rows were
built under. The rowid is one of the four 64-bit words of the occurrence
identifier with the sign bit cleared (`retrieval::lexical::rowids`): the first
word no other occurrence holds. A rebuild and incremental application store
equal rows unless two live occurrences share a word. The row is written in the
same transaction as its occurrence, deleted by `retrieval::tombstone_occurrence`
in the transaction that records the occurrence's tombstone, and deleted again
when message cleanup reclaims the occurrence. The shadow tables the FTS5 module
creates (`lexical_data`, `lexical_idx`, `lexical_content`, `lexical_docsize`,
`lexical_config`) are engine-owned and outside this inventory. `PRAGMA
integrity_check` verifies the inverted index; `retrieval::lexical::verify_rows`
checks that live occurrences and lexical rows correspond one to one and that
every row sits at one of its occurrence's words. It also compares both indexed
columns with the analyzed payload. Reopen, construction verification, and
closed-seed certification run this check under a consistent snapshot. Batch
replay and `batch_status` compare the indexed text with their source records;
a different text is corruption, not a successful replay. Within a batch,
tombstones release their lexical rowids before replacements are placed.

Virtual table: `fts5(original, parts, occurrence_id UNINDEXED, tokenize = 'unicode61 remove_diacritics 2 tokenchars ''_''', detail = full)`

## `projection_checkpoint`

The fixed S the projection was built at and the canonical commit it has applied through. The canonical consumer checkpoint never moves ahead of this.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `singleton` | INTEGER | yes | yes | `CHECK(singleton=1)` |
| `snapshot_commit_seq` | INTEGER | yes | no | `CHECK(snapshot_commit_seq>=0)` |
| `checkpoint_commit_seq` | INTEGER | yes | no | `CHECK(checkpoint_commit_seq>=snapshot_commit_seq)` |
| `hold_id` | TEXT | no | no |  |
| `updated_at` | INTEGER | yes | no |  |

## `vector_generations`

One immutable vector generation: the model, tokenizer, dimension, and epoch its vectors were produced under, and its lifecycle state.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `generation_id` | TEXT | yes | yes |  |
| `embedding_model` | TEXT | yes | no |  |
| `tokenizer_fingerprint` | TEXT | yes | no |  |
| `vector_dimension` | INTEGER | yes | no | `CHECK(vector_dimension>0)` |
| `generation_epoch` | INTEGER | yes | no | `CHECK(generation_epoch>=0)` |
| `state` | TEXT | yes | no | `CHECK(state IN ('building','verified','selected','retired'))` |
| `created_at` | INTEGER | yes | no |  |
| `updated_at` | INTEGER | yes | no |  |

Indexes:

- `idx_vector_generations_selected` on `(state) WHERE state='selected'` (unique: at most one generation is selected)

## `occurrence_vectors`

One vector per occurrence per generation, little-endian f32, four bytes per dimension, with the input accounting the embedding was charged for.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `occurrence_id` | TEXT | yes | yes | `REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT` |
| `generation_id` | TEXT | yes | yes | `REFERENCES vector_generations(generation_id) ON DELETE RESTRICT` |
| `vector` | BLOB | yes | no |  |
| `vector_dimension` | INTEGER | yes | no | `CHECK(vector_dimension>0 AND vector_dimension*4=length(vector))` |
| `input_bytes` | INTEGER | yes | no | `CHECK(input_bytes>=0)` |
| `input_tokens` | INTEGER | yes | no | `CHECK(input_tokens>=0)` |
| `completed_at` | INTEGER | yes | no |  |

Table constraints:

- `PRIMARY KEY(occurrence_id,generation_id)`

Indexes:

- `idx_occurrence_vectors_generation` on `(generation_id,occurrence_id)`

## `embedding_jobs`

Durable embedding work and retry accounting. A pending row is the crash source for the process-local job table. An episode is one finite grant of attempts under one deadline; only an explicit authorization reference opens another, and a stop reason holds the row until one arrives. An admitted row names the host incarnation holding it; work held by any other incarnation returns to pending.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `job_id` | TEXT | yes | yes |  |
| `occurrence_id` | TEXT | yes | no | `REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT` |
| `generation_id` | TEXT | yes | no | `REFERENCES vector_generations(generation_id) ON DELETE RESTRICT` |
| `state` | TEXT | yes | no | `CHECK(state IN ('pending','admitted','embedded','published','obsolete','failed'))` |
| `attempts` | INTEGER | yes | no | `DEFAULT 0 CHECK(attempts>=0)` |
| `last_failure_kind` | TEXT | no | no |  |
| `next_attempt_at` | INTEGER | no | no |  |
| `admitted_epoch` | INTEGER | no | no |  |
| `episode_id` | TEXT | no | no |  |
| `episode_allowance` | INTEGER | yes | no | `DEFAULT 0 CHECK(episode_allowance>=0)` |
| `episode_deadline` | INTEGER | no | no |  |
| `host_job_id` | TEXT | no | no |  |
| `host_incarnation` | TEXT | no | no |  |
| `stop_reason` | TEXT | no | no |  |
| `authorization_ref` | TEXT | no | no |  |
| `created_at` | INTEGER | yes | no |  |
| `updated_at` | INTEGER | yes | no |  |

Table constraints:

- `UNIQUE(occurrence_id,generation_id)`

Indexes:

- `idx_embedding_jobs_dispatch` on `(state,next_attempt_at,job_id)`
- `idx_embedding_jobs_generation` on `(generation_id,job_id)`
- `idx_embedding_jobs_open_order` on `(created_at,job_id) WHERE state IN ('pending','admitted') AND stop_reason IS NULL`

Dispatch reads open jobs in creation order, breaking ties by job identity. The
partial index avoids sorting the due backlog before applying the row limit.
Deferred or WrongScope rows can still form a long prefix, so the daemon keeps a
process-local keyset cursor across successful passes. One pass reads at most two
pages of at most 1,024 candidates and wraps to the beginning at the ordered tail.
The index supplies order; the dispatcher supplies the scan and action budgets.

## `embedding_recovery_authorizations`

Each row records a consumed authorization for one embedding job. Inserting the
receipt and opening its episode share the caller's transaction. Receipts have no
expiry: replaying any consumed reference grants no new attempts, even after
another reference opens an episode. The job's `authorization_ref` column records
the most recent grant; this table determines whether a reference is a replay.
These receipts record local consumption, not external authority to recover or
rebuild the projection.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `job_id` | TEXT | yes | yes | `REFERENCES embedding_jobs(job_id) ON DELETE RESTRICT` |
| `authorization_ref` | TEXT | yes | yes |  |

Table constraints:

- `PRIMARY KEY(job_id,authorization_ref)`

## `retirement_receipts`

Local receipts of a generation's retirement. Consumer retirement records the old
generation without a foreign key because that generation belongs to a separate
database. All six consumer-binding columns are populated together. Receipts for
in-database identity sweeps leave those columns null.

A consumer receipt has reason `consumer_retired`. Its ID is the old family's
immutable seed digest. The selected family digest binds the replacement seed,
generation, and verification certificate. The receipt's fixed target is not a
second checkpoint and never advances with the kernel tip.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `receipt_id` | TEXT | yes | yes |  |
| `generation_id` | TEXT | yes | no |  |
| `reason` | TEXT | yes | no |  |
| `operator_id` | TEXT | no | no |  |
| `retired_at` | INTEGER | yes | no |  |
| `recorded_at` | INTEGER | yes | no |  |
| `old_consumer_id` | TEXT | no | no |  |
| `old_family` | TEXT | no | no |  |
| `selected_family` | TEXT | no | no |  |
| `kernel_incarnation_id` | TEXT | no | no |  |
| `through_commit_seq` | INTEGER | no | no | `CHECK(through_commit_seq>=0)` |
| `obligation_count` | INTEGER | no | no | `CHECK(obligation_count>=0)` |

Table constraints:

- `CHECK((old_consumer_id IS NULL AND old_family IS NULL AND selected_family IS NULL AND kernel_incarnation_id IS NULL AND through_commit_seq IS NULL AND obligation_count IS NULL) OR (old_consumer_id IS NOT NULL AND old_family IS NOT NULL AND selected_family IS NOT NULL AND kernel_incarnation_id IS NOT NULL AND through_commit_seq IS NOT NULL AND obligation_count IS NOT NULL))`

Indexes:

- `idx_retirement_receipts_generation` on `(generation_id,receipt_id)`

## `retirement_dispositions`

Each row records completed removal for one authoritative source revision or
old-consumer barrier. Recovery compares every field against the bounded kernel
inventory, including satisfied barriers and invalidated source revisions.
Missing or extra rows refuse acknowledgement even when the count matches.

| Column | Type | Not null | Primary key | Column constraints |
| --- | --- | --- | --- | --- |
| `receipt_id` | TEXT | yes | yes | `REFERENCES retirement_receipts(receipt_id) ON DELETE RESTRICT` |
| `kind` | TEXT | yes | yes | `CHECK(kind IN ('source','barrier'))` |
| `identity` | TEXT | yes | yes |  |
| `artifact_digest` | TEXT | yes | no |  |
| `commit_seq` | INTEGER | yes | no | `CHECK(commit_seq>=0)` |
| `invalidated_commit_seq` | INTEGER | no | no |  |
| `disposition` | TEXT | yes | no | `CHECK(disposition='removed')` |

Table constraints:

- `PRIMARY KEY(receipt_id,kind,identity)`
