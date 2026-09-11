-- Baseline schema of search.sqlite, applied once to a pristine file behind the
-- fence and format_marker tables the storage crate installs first. The bytes
-- of this file are part of the store identity storage::open_sqlite checks on
-- every open. There is no migration path: a projection whose identity or
-- schema no longer matches is rebuilt from the canonical store, never patched.
-- The field inventory is frozen in
-- docs/properties/search-projection/projection-schema.md, and a test compares
-- that document with this file.

-- The single identity the whole projection was built under. Any component
-- that differs at open means the projection is incompatible and is rebuilt.
CREATE TABLE projection_identity(
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    schema_version INTEGER NOT NULL CHECK(schema_version>0),
    kernel_incarnation_id TEXT NOT NULL,
    projection_policy_version TEXT NOT NULL,
    identity_contract_version TEXT NOT NULL,
    limit_manifest_protocol_version TEXT NOT NULL,
    embedding_model TEXT NOT NULL,
    tokenizer_fingerprint TEXT NOT NULL,
    vector_dimension INTEGER NOT NULL CHECK(vector_dimension>0),
    generation_epoch INTEGER NOT NULL CHECK(generation_epoch>=0),
    installed_at INTEGER NOT NULL
) STRICT;

-- Exact selected bytes, stored once per payload identity. Two occurrences
-- with equal bytes share one row; unequal bytes under one digest are refused
-- by the writer before this table is touched.
CREATE TABLE payloads(
    payload_id TEXT PRIMARY KEY,
    bytes BLOB NOT NULL,
    byte_length INTEGER NOT NULL CHECK(byte_length=length(bytes)),
    created_at INTEGER NOT NULL
) STRICT;

-- One row per occurrence: the CC4 tuple bytes beside their identifier, the
-- lineage they belong to, the payload they select, and the canonical row they
-- came from. Sensitivity and provenance stay separate from identity.
CREATE TABLE occurrences(
    occurrence_id TEXT PRIMARY KEY,
    tuple BLOB NOT NULL,
    lineage_id TEXT NOT NULL,
    class TEXT NOT NULL CHECK(class IN ('messages','canonical_claims','promoted_memory','git_commits','raw_tool_spans')),
    revision INTEGER NOT NULL CHECK(revision>=0),
    representation TEXT NOT NULL,
    span_start INTEGER,
    span_end INTEGER,
    payload_id TEXT NOT NULL REFERENCES payloads(payload_id) ON DELETE RESTRICT,
    domain_id TEXT NOT NULL,
    sensitivity TEXT NOT NULL CHECK(sensitivity IN ('normal','sensitive','secret')),
    source_object_id TEXT NOT NULL,
    source_evidence_id TEXT NOT NULL,
    source_artifact_digest TEXT NOT NULL,
    created_commit_seq INTEGER NOT NULL CHECK(created_commit_seq>0),
    persisted_at INTEGER NOT NULL,
    CHECK((span_start IS NULL)=(span_end IS NULL)),
    CHECK(span_start IS NULL OR (span_start>=0 AND span_end>=span_start))
) STRICT;
CREATE INDEX idx_occurrences_lineage ON occurrences(lineage_id,revision);
CREATE INDEX idx_occurrences_payload ON occurrences(payload_id);
CREATE INDEX idx_occurrences_source ON occurrences(class,source_object_id,revision);

-- An occurrence that stopped being live: superseded by a newer revision,
-- retired, or its evidence deleted or purged. The occurrence row stays so the
-- invalidation fact keeps its identity.
CREATE TABLE occurrence_tombstones(
    occurrence_id TEXT PRIMARY KEY REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT,
    invalidated_commit_seq INTEGER NOT NULL CHECK(invalidated_commit_seq>0),
    reason TEXT NOT NULL CHECK(reason IN ('superseded','retired','evidence_invalidated','purged')),
    recorded_at INTEGER NOT NULL
) STRICT;

-- The fixed S the projection was built at and the canonical commit it has
-- applied through. The canonical consumer checkpoint never moves ahead of this.
CREATE TABLE projection_checkpoint(
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    snapshot_commit_seq INTEGER NOT NULL CHECK(snapshot_commit_seq>=0),
    checkpoint_commit_seq INTEGER NOT NULL CHECK(checkpoint_commit_seq>=snapshot_commit_seq),
    hold_id TEXT,
    updated_at INTEGER NOT NULL
) STRICT;

-- One immutable vector generation: the model, tokenizer, dimension, and epoch
-- every vector in it was produced under, and where it stands in its lifecycle.
CREATE TABLE vector_generations(
    generation_id TEXT PRIMARY KEY,
    embedding_model TEXT NOT NULL,
    tokenizer_fingerprint TEXT NOT NULL,
    vector_dimension INTEGER NOT NULL CHECK(vector_dimension>0),
    generation_epoch INTEGER NOT NULL CHECK(generation_epoch>=0),
    state TEXT NOT NULL CHECK(state IN ('building','verified','selected','retired')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE UNIQUE INDEX idx_vector_generations_selected ON vector_generations(state) WHERE state='selected';

-- One vector per occurrence per generation: little-endian f32 values, so the
-- blob is exactly four bytes per dimension, with the input accounting the
-- embedding was charged for.
CREATE TABLE occurrence_vectors(
    occurrence_id TEXT NOT NULL REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT,
    generation_id TEXT NOT NULL REFERENCES vector_generations(generation_id) ON DELETE RESTRICT,
    vector BLOB NOT NULL,
    vector_dimension INTEGER NOT NULL CHECK(vector_dimension>0 AND vector_dimension*4=length(vector)),
    input_bytes INTEGER NOT NULL CHECK(input_bytes>=0),
    input_tokens INTEGER NOT NULL CHECK(input_tokens>=0),
    completed_at INTEGER NOT NULL,
    PRIMARY KEY(occurrence_id,generation_id)
) STRICT;
CREATE INDEX idx_occurrence_vectors_generation ON occurrence_vectors(generation_id,occurrence_id);

-- Durable embedding work. A pending row is the crash source for the process
-- local job table; retry accounting lives here so a restart resumes with the
-- same identity and the same attempt history. An episode is one finite grant
-- of attempts under one deadline; only an explicit authorization reference
-- opens another, and a stop reason holds the row until one arrives. An
-- admitted row names the host incarnation holding it; work held by any other
-- incarnation is unreachable and returns to pending.
CREATE TABLE embedding_jobs(
    job_id TEXT PRIMARY KEY,
    occurrence_id TEXT NOT NULL REFERENCES occurrences(occurrence_id) ON DELETE RESTRICT,
    generation_id TEXT NOT NULL REFERENCES vector_generations(generation_id) ON DELETE RESTRICT,
    state TEXT NOT NULL CHECK(state IN ('pending','admitted','embedded','published','obsolete','failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
    last_failure_kind TEXT,
    next_attempt_at INTEGER,
    admitted_epoch INTEGER,
    episode_id TEXT,
    episode_allowance INTEGER NOT NULL DEFAULT 0 CHECK(episode_allowance>=0),
    episode_deadline INTEGER,
    host_job_id TEXT,
    host_incarnation TEXT,
    stop_reason TEXT,
    authorization_ref TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(occurrence_id,generation_id)
) STRICT;
CREATE INDEX idx_embedding_jobs_dispatch ON embedding_jobs(state,next_attempt_at,job_id);
CREATE INDEX idx_embedding_jobs_generation ON embedding_jobs(generation_id,job_id);

-- Local receipts of a generation's retirement, kept so a retired generation's
-- files can be reclaimed once and the reclamation can be audited.
CREATE TABLE retirement_receipts(
    receipt_id TEXT PRIMARY KEY,
    generation_id TEXT NOT NULL REFERENCES vector_generations(generation_id) ON DELETE RESTRICT,
    reason TEXT NOT NULL,
    operator_id TEXT,
    retired_at INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_retirement_receipts_generation ON retirement_receipts(generation_id,receipt_id);
