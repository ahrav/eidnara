# Selective Indexing

## Goal

Eidnara should preserve broad project history without giving every byte the same
search treatment.

Different information needs different representations:

```text
identifiers, paths, hashes, test names
    → exact and lexical search

failures, diagnostics, file changes
    → structured fields and lexical search

rationale, explanations, design discussion
    → lexical and semantic search

large logs, diffs, and tool output
    → raw evidence plus a compact searchable record
```

The target is:

```text
large durable history
+
small active search working set
```

Raw evidence remains authoritative. Search representations exist only to find
that evidence.

This design complements [Prepared Context Reuse](prepared-context-reuse.md).
That document covers reuse and invalidation of derived context. This document
covers which derived search representations should exist in the first place.

## Feasibility

This design fits the current architecture. Eidnara already has several required
primitives:

- content-addressed artifact storage
- separate evidence identities and artifact digests
- source revisions, invalidation, and supersession
- durable outbox consumers and deletion barriers
- structured historian compartments, notes, and claims
- canonical embedding batch identities
- model fingerprints and table epochs
- bounded token and embedding caches

The missing piece is a rebuildable search projection:

```text
source revision
  → evidence occurrence
  → search unit
  → lexical representation
  → optional semantic representation
  → searchable generation
```

This is more than a ranking change. Eidnara does not yet have a durable lexical
index, embedding store, vector index, or hybrid query path.

## Existing seams

### Raw evidence and payload deduplication

`crates/kernel/src/cas/ingest.rs` hashes stored artifact bytes and publishes them
with no-replace semantics. `crates/kernel/src/schema.rs` stores each evidence row
with both an `evidence_id` and an `artifact_digest`.

That already supports this shape:

```text
run on Monday ─┐
               ├─ shared artifact payload
run on Tuesday ┘
```

The two runs remain separate evidence occurrences even when their payload bytes
are identical.

`crates/kernel/src/cas/read.rs` also checks that an evidence reference is still
live before returning artifact bytes. A digest match alone does not authorize a
read.

### Memory search

`crates/memory-store/src/lib.rs` already exposes bounded searches over historian
compartments and notes. The store uses case-folded SQL `LIKE` queries and can
load bounded compartment candidates for later ranking.

This worktree's daemon defines the `ctx_search` prompt surface in
`crates/daemon/src/lib.rs`, but its search handler is not present on this branch.
The available store methods still show the current lexical boundary: they cover
compartments and notes, not source files, commits, diagnostics, kernel evidence,
or general tool output.

### Embeddings

Synapse is an embedding service, not a search engine.

`crates/host-runtime/src/synapse/protocol.rs` builds canonical batch identities
from content hashes, item IDs, model identity, embedding fingerprint, and table
epoch. `crates/host-runtime/src/synapse/jobs.rs` can reuse an identical retained
batch job.

Current reuse is process-local and expires. Eidnara does not persist vectors or
associate them with source revisions and searchable generations.

Interactive queries and batch jobs also share the same CPU semaphore in
`crates/host-runtime/src/synapse/mod.rs`. Serialization prevents concurrent
inference. It does not give interactive work priority over queued background
batches. Any background embedding pipeline must be paced or scheduled around
foreground queries.

### Derived-state lifecycle

The kernel already records source revisions, commit positions, outbox events,
consumer checkpoints, deletion barriers, sensitivity, and purge state.

Those primitives can drive a search projection. The search store should consume
canonical changes, publish its own generation, and acknowledge work only after
its required effects are durable.

## Evidence and search representations

Every searchable item has two roles:

```text
Evidence
    authoritative bytes and event identity

Search representation
    rebuildable data used to locate the evidence
```

Examples:

```text
raw test run
    → test failure record

raw compiler output
    → diagnostic records

raw git diff
    → changed paths, statuses, symbols, and summary

conversation range
    → decision, rationale, and explanation units

source file
    → path, symbols, text, and selected prose units
```

Deleting a search index must not delete project history. Rebuilding a search
index must not change evidence identity.

## Representation policy

Start with deterministic rules. Use source kind, media type, tool name, file
extension, size, exit status, and known schemas.

| Content | Structured | Lexical | Semantic | Raw evidence |
| --- | ---: | ---: | ---: | ---: |
| File paths and symbols | Yes | Yes | Rarely | Yes |
| Source code | Yes | Yes | Selectively | Yes |
| Documentation | Yes | Yes | Yes | Yes |
| Design discussion | Yes | Yes | Yes | Yes |
| Decisions and rationale | Yes | Yes | Yes | Yes |
| Commit metadata | Yes | Yes | Message only | Yes |
| Compiler diagnostics | Yes | Yes | Rarely | Yes |
| Test failures | Yes | Yes | Selectively | Yes |
| Successful test output | Summary only | Rarely | No | Yes |
| Git diffs | Yes | Yes | Summary only | Yes |
| Generic tool output | When known | Selectively | Rarely | Yes |
| Large generated output | Minimal | Rarely | No | Yes |

Embedding is an explicit policy result. It is not a default side effect of
storing text.

Do not start with an LLM classifier. Static rules are cheaper, reproducible, and
easier to evaluate. Add learned classification only after measured failures show
that deterministic rules are insufficient.

## Search units

A search unit is the smallest representation that can be ranked and expanded
back to evidence.

```rust
SearchUnit {
    unit_id,
    evidence_id,
    source_namespace,
    source_id,
    source_revision,
    unit_kind,
    content_digest,
    path,
    symbol,
    title,
    lexical_text,
    semantic_text,
    extractor_revision,
    policy_revision,
    evidence_group_id,
}
```

`semantic_text` is optional. Most structured records should leave it empty.

`evidence_group_id` groups representations that support the same underlying
fact or decision. It is not an event identity. Two independent failures may
share bytes while belonging to separate evidence groups.

Search results return search-unit metadata and evidence references. They do not
copy raw evidence into every result record.

## Deterministic extractors

Prefer structured output from the producing tool.

Examples:

- Cargo `--message-format=json` for compiler messages and build artifacts
- rustc JSON diagnostics for codes, levels, spans, and rendered messages
- Git `--name-status`, `--numstat`, and NUL-delimited output for changed files
- known test harness formats for test names and failure details
- repository parsers for paths and symbols

A tool-specific extractor should retain the original output and emit compact
records. It should not rewrite the original artifact.

Unknown output can remain archive-only until a real retrieval need justifies an
extractor.

## Query routing

Exact and lexical retrieval should remain available for every query.

```text
query
  ├─ exact identifiers and structured filters
  ├─ lexical candidates
  └─ semantic candidates, when useful
          ↓
       candidate union
          ↓
          fusion
          ↓
  evidence-group deduplication
          ↓
  current eligibility check
          ↓
     bounded context packing
```

Useful routing signals include:

- quoted strings and backticks
- path separators
- `::`, `.`, and known symbol forms
- commit-like hashes
- test names and error codes
- words such as `where`, `defined`, and `which file`
- words such as `why`, `rationale`, `alternative`, and `decided`

Routing should change candidate budgets, not remove the exact-search baseline.
A rationale query may still contain an identifier that supplies the best lexical
candidate.

Start with simple rank fusion such as reciprocal rank fusion. Do not mix BM25,
cosine, and structured scores as if they shared one calibrated scale.

## Vector storage and search

Do not add an approximate nearest-neighbor index first.

Selective indexing should make the semantic corpus much smaller than the full
history. Start with exact scoring over the eligible semantic subset. Measure
filtering, vector reads, scoring, and top-k selection at target load.

Add ANN only if exact search misses a stated latency or resource budget. Compare
ANN against exact scoring over the same eligible rows and embedding generation.

An embedding identity should include at least:

```text
content digest
model fingerprint
normalization contract
truncation contract
extractor or chunker revision
```

Equal dimensions do not prove that two vectors belong to the same embedding
space.

## Deduplication and lineage

Deduplicate at three different layers.

### Payload deduplication

Identical stored bytes share one content-addressed artifact. Separate event and
evidence records remain intact.

### Representation deduplication

Identical canonical semantic text may share an embedding when the complete
embedding identity matches. Each search unit still retains its own source,
visibility, sensitivity, and deletion metadata.

### Result deduplication

Collapse repeated summaries or quotations by evidence group when they derive
from the same underlying decision. Do not collapse independent events merely
because their bytes match.

A content hash is not a lineage proof. Later summaries should point to the
original evidence when that relationship is known.

## Query-result reuse

Query-result reuse is separate from selective indexing. Selective indexing does
not depend on it.

There are four distinct reuse problems:

| Reuse | Initial policy |
| --- | --- |
| Document embedding reuse | Build early |
| Exact query-embedding reuse | Build early |
| Exact retrieval-result reuse | Add after measurement |
| Similar-query or follow-up reuse | Defer |

Exact retrieval results can be reused when every input matches:

```rust
QueryResultKey {
    canonical_query,
    retrieval_snapshot,
    retrieval_config_digest,
    visibility_scope,
    result_budget,
}
```

The retrieval snapshot should include the source frontier, lexical generation,
embedding fingerprint, vector generation, extractor revision, and policy
generation.

Start with whole-generation invalidation and a bounded process-local cache. Do
not start with semantic query caching. Similar questions can need different
evidence even when their query embeddings are close.

For follow-up questions, previous evidence can be a warm candidate set. Fresh
retrieval remains the authority.

## Storage boundary

Keep the search store separate from canonical kernel and memory stores.

The search store should be:

- safe to delete and rebuild
- bounded by bytes and entry count
- published by generation
- tied to source and policy frontiers
- unable to authorize disclosure by itself
- covered by deletion and purge propagation

A practical first store can use SQLite tables for structured records and FTS5
for lexical search. FTS5 external-content or contentless tables separate indexed
terms from stored content. Eidnara must still own consistency, rebuild, and
publication rules.

The first prototype should verify that the linked SQLite build enables FTS5. Do
not infer that from a development CLI using a different SQLite library.

## Experiment

Use one real project history and compare representation policies against the
same judged queries.

### Policies

```text
A: embed every text chunk

B: exclude logs and tool output

C: embed prose, docs, decisions, and selected source units

D: use dense retrieval only for natural-language knowledge
```

### Query slices

- identifier and location
- rationale
- chronology
- failure cause
- rejected alternative
- proving test
- exact error
- verified no-answer

### Measurements

- known-relevant recall at a fixed cutoff
- first relevant rank
- required evidence-group coverage
- unique lexical and semantic contributions
- index and vector bytes
- extraction and embedding CPU
- query latency and failures
- Synapse queue pressure
- rebuild duration
- raw bytes preserved versus bytes actively indexed

For each miss, record the first failed stage:

```text
not preserved
not extracted
not represented
not eligible
missed by lexical retrieval
missed by semantic retrieval
lost during fusion
collapsed by deduplication
removed during context packing
```

A final ranking score alone cannot identify which policy failed.

## Staged path

1. Inventory the current corpus by source kind, size, duplication, and retention.
2. Export one project into evidence and occurrence records.
3. Add deterministic extractors for source, Cargo diagnostics, tests, and Git.
4. Build a disposable SQLite structured and lexical index.
5. Establish lexical-only retrieval and judged queries.
6. Add embeddings only for policy-selected units.
7. Compare policies with exact vector scoring.
8. Publish search generations from canonical change frontiers.
9. Add bounded exact query-result caching if measurements justify it.
10. Consider ANN, semantic caching, or automatic promotion only after evidence.

The offline experiment should come before daemon protocol changes.

## Main risks

- A classifier can omit evidence that no later ranker can recover.
- An incomplete embedding key can reuse vectors from the wrong representation.
- Search publication can race source updates, deletion, or policy changes.
- Shared representations can accidentally collapse visibility boundaries.
- Background embedding can delay foreground query inference.
- Summaries can hide the only useful line in a large artifact.
- More indexed material can increase noise and reduce retrieval quality.
- Cheap one-shot extraction can cost more to cache than to repeat.

Raw expansion and an exact-search baseline limit several of these risks. They do
not replace lifecycle and eligibility checks.

## Reference model

Useful prior art:

- [SQLite FTS5 external-content and contentless tables](https://www.sqlite.org/fts5.html#external_content_and_contentless_tables): separate indexed terms from content storage.
- [Git objects](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects): content-addressed blobs with separate trees and commits.
- [GitHub Blackbird](https://github.blog/engineering/architecture-optimization/the-technology-behind-githubs-new-code-search/): separate n-gram indexes for content, symbols, and paths.
- [Zoekt design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md): positional trigram code search and immutable shards.
- [Cargo JSON messages](https://doc.rust-lang.org/cargo/reference/external-tools.html#json-messages): structured compiler and build output.
- [OpenTelemetry log data model](https://opentelemetry.io/docs/specs/otel/logs/data-model/): event metadata, attributes, and body as separate fields.
- [Elastic hybrid retrieval](https://www.elastic.co/docs/solutions/search/hybrid-semantic-text): lexical and semantic fields with rank fusion.
- [Lucene `LRUQueryCache`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/search/LRUQueryCache.html): bounded selective query caching.
- [Elasticsearch shard request cache](https://www.elastic.co/docs/reference/elasticsearch/rest-apis/shard-request-cache): exact request caching with refresh-based invalidation.
- [Salsa red-green algorithm](https://salsa-rs.github.io/salsa/reference/algorithm.html) and [Bazel Skyframe](https://bazel.build/reference/skyframe): dependency tracking and minimal recomputation.

These systems provide mechanisms, not an Eidnara policy. The representation
policy still needs project-specific evaluation.

## Design principle

Eidnara should preserve evidence broadly and index representations selectively.

```text
preserve the event
share identical payload bytes
extract compact searchable records
embed only where meaning adds retrieval value
expand back to evidence when needed
```

The active index should reflect what helps retrieval, not everything that exists.
