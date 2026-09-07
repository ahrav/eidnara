# Prepared Context Reuse

## Goal

Eidnara should spend work in proportion to what changed.

When a user continues working in an unchanged part of a project, the normal path should be:

```text
validate existing state
reuse prepared context
retrieve the small missing delta
assemble references
```

It should not repeat this sequence on every turn:

```text
read
extract
render
tokenize
embed
rank
copy
```

The target is better and more current evidence per token, with bounded CPU, memory, and I/O.

## Feasibility

This design fits the current architecture. Eidnara already has many required primitives:

- content hashes and content-addressed artifacts
- source revisions and snapshot commits
- selective message and prefix reuse
- cached token counts
- embedding model fingerprints and epochs
- idempotent embedding batches
- bounded caches, semaphores, byte reservations, and retention
- generation-fenced eligibility and applicability checks

The missing piece is a shared model for derived context:

```text
source revision
  → content units
  → rendered units
  → token counts
  → embeddings
  → searchable generation
  → retrieval result
  → prepared context
```

Today these identities exist in separate subsystems. Eidnara does not yet record which derived objects depend on which source, model, policy, or index generation.

## Existing seams

### Token counts

`crates/daemon/src/token_cache.rs` already hashes content and skips tokenization on a hit. The cache is bounded and avoids holding its lock while tokenizing.

This proves the basic reuse model works. It is currently process-local and not tied to a durable tokenizer identity.

### Embeddings

`crates/host-runtime/src/synapse/protocol.rs` builds canonical batch identities from:

- content hashes
- item IDs
- model
- embedding fingerprint
- table epoch

Synapse also verifies each supplied content hash. This is a strong base for persistent embedding reuse. Current completed-job reuse is process-local and expires.

### Context assembly

The daemon already reuses unchanged message projections, serialized output, native attachments, and frozen prefix bytes. Relevant code lives in:

- `crates/daemon/src/lib.rs`
- `crates/daemon/src/transform.rs`
- `crates/daemon/src/wire.rs`
- `crates/cache-stability/src/lib.rs`

Prepared evidence can extend this same selective invalidation model from conversation messages to project sources.

### Source evolution

`crates/kernel/src/schema.rs` records source identity, revisions, commit positions, invalidation, supersession, artifact digests, extraction runs, and purge state.

This gives prepared artifacts a reliable authority for freshness. Prepared storage should remain a derived projection, not become a second source of truth.

## Four kinds of reuse

### 1. Prepared-content reuse

```text
same source bytes
+ same preparation contract
= same prepared artifact
```

This covers extraction, rendering, token counts, and embeddings. It is the safest and highest-value first step.

### 2. Prepared-context reuse

Store context as a manifest of immutable units:

```text
Turn 1 = A + B + C
Turn 2 = A + B + C + D
```

Both turns reference the same `A`, `B`, and `C`. They do not store separate copies.

### 3. Exact query-result reuse

An exact result can be reused when all inputs match:

```text
canonical query
+ searchable snapshot
+ retrieval configuration
+ visibility scope
+ result budget
```

A new searchable generation should invalidate the result conservatively.

### 4. Follow-up query reuse

Related questions do not necessarily have the same relevant evidence.

Previous evidence should be a warm candidate set, not the final result:

```text
previous evidence
+ fresh delta retrieval
- stale or ineligible evidence
→ rerank and repack
```

Semantic query caching should not be a correctness authority without retrieval evaluations and a safe fallback.

## Proposed artifact model

Keep identities separate so one change does not invalidate unrelated work.

```rust
ContentUnit {
    content_digest,
    canonical_bytes,
    source_provenance,
    sensitivity_class,
}

RenderedUnitKey {
    content_digest,
    renderer_version,
    redaction_policy_revision,
    output_format,
}

TokenCountKey {
    rendered_digest,
    tokenizer_fingerprint,
}

EmbeddingKey {
    content_digest,
    model_fingerprint,
    normalization_contract,
}

PreparedContextManifest {
    ordered_rendered_units,
    source_snapshot,
    selection_config_digest,
    total_tokens,
}
```

A source revision maps to an ordered list of content units. After an edit, Eidnara compares old and new unit digests and prepares only changed units.

For code, semantic units such as functions, types, and modules are better than arbitrary byte windows. Large or unstructured inputs can use content-defined chunking inside logical outer boundaries.

## Query-result reuse

Start with whole-generation invalidation.

```rust
QueryResultKey {
    query_digest,
    retrieval_snapshot,
    retrieval_config_digest,
    visibility_scope,
    result_budget,
}
```

A retrieval snapshot should cover at least:

- source or kernel tip
- searchable publication frontier
- lexical index generation
- embedding-space fingerprint
- vector index generation
- chunker generation
- policy generation

Per-segment reuse can come later. Lucene shows how immutable segments allow per-segment query caching, but global top-k ranking, deletes, reranking, and ANN make correct composition harder.

## Storage boundary

Prepared artifacts, embeddings, indexes, and query caches are rebuildable. Keep them in a dedicated derived-artifact store rather than adding authority to the baseline memory schema.

The store should be:

- safe to delete and rebuild
- published by atomic generation
- bounded by bytes and entry count
- subject to source eligibility, sensitivity, and purge rules
- unable to authorize disclosure by itself

A digest match proves content identity. It does not prove current relevance or permission to disclose.

## Staged path

1. **Measure repeated work.** Record bytes read, rendered, tokenized, embedded, copied, and avoided.
2. **Persist prepared units.** Add source-to-unit mappings, rendered-unit storage, and tokenizer-fingerprinted counts.
3. **Persist embeddings.** Reuse Synapse's existing content and model identity contract.
4. **Publish searchable generations.** Build derived indexes and switch generations atomically.
5. **Store context manifests.** Reference immutable rendered units instead of copying full contexts.
6. **Cache exact query results.** Key by complete retrieval snapshot and use whole-generation invalidation.
7. **Reuse session working sets.** Keep prior evidence as candidates for follow-up retrieval.
8. **Consider segment or semantic caching only after measurement.**

## Main risks

- Incomplete cache keys can silently reuse wrong artifacts.
- Unchanged bytes can still be stale because policy, applicability, or newer evidence changed.
- Deduplication can conflict with deletion and sensitivity boundaries.
- Cheap one-shot work may cost more to cache than recompute.
- Local byte reuse does not remove model-side token processing unless the provider supports prompt or KV caching.

## Reference model

Useful prior art:

- [Salsa red-green algorithm](https://salsa-rs.github.io/salsa/reference/algorithm.html): dependency tracking, memoization, and change pruning.
- [Bazel Skyframe](https://bazel.build/reference/skyframe): incremental dependency graphs and reverse invalidation.
- [Nix store resolution](https://nix.dev/manual/nix/2.34/store/resolution.html): complete identity contracts for reusable derivations.
- [Lucene LRUQueryCache](https://lucene.apache.org/core/10_5_0/core/org/apache/lucene/search/LRUQueryCache.html): bounded per-segment query caching.
- [Elasticsearch shard request cache](https://www.elastic.co/docs/reference/elasticsearch/rest-apis/shard-request-cache): exact request reuse with refresh-based invalidation.
- [FastCDC](https://www.usenix.org/conference/atc16/technical-sessions/presentation/xia): content-defined chunking for edit-stable deduplication.

## Design principle

Eidnara should represent expensive derived context as immutable, versioned artifacts with explicit dependencies. Each turn should validate and reuse those artifacts, then recompute only the smallest invalidated units.

Start with stable prepared units, persistent token and embedding reuse, immutable context manifests, and exact query caching. Defer semantic query caching until measurements show it is needed.
