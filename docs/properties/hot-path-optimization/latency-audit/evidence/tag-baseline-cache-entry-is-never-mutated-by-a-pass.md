# tag-baseline-cache-entry-is-never-mutated-by-a-pass

Baseline: `913234433ae36a80a6e22c6aac14c7f9aab74386`, 2026-09-10.
The [scope and provenance](../catalog.md#scope-and-provenance) apply here.

## Discovery trigger

The audit notes that `Arc::make_mut(tag_rows)` copies the whole baseline
vector on every pass whose rows came from a cache hit, and proposes appending
in place or storing the pass's vector back into the cache. The copy exists
because the cache holds a second reference to the same `Arc`. Removing it
without another isolation mechanism exposes pass-local mint rows, which the
store has not numbered, to the next pass of the same session through the
cache. The transform catalog's [two-authorities record][tc-tagnum] covers the
numbering split; this record covers the cache entry's immutability and the
`source_bytes` identity.

## Evidence trail

- [`TagBaselineCacheEntry`][tag-entry] records `store_namespace`,
  `generation`, `count`, `max_tag_number`, and `tags: Arc<Vec<TagRow>>`;
  `matches` requires all four keys equal and `can_append` requires the
  summary to have grown by exactly the appended count.
- [`snapshot`][tag-snapshot] returns `self.sessions.get(session_id)?.clone()`,
  a clone of the entry and therefore a second `Arc` reference to `tags`.
- [`load_cached_tags`][load-tags] returns `entry.tags` on a match; on
  `can_append` it reads `load_tags_after`, re-reads the summary, and appends
  only when `observed == summary`, the tail length equals the appended count,
  and the last tail row carries `max_tag_number`; otherwise it reloads
  [`load_tags_for_session`][load-order], which orders by `tag_number ASC`.
- The pass loads `tag_rows` at [`:3002`][load-call] and later runs
  [`append_tag_mint_rows(Arc::make_mut(tag_rows), ..)`][make-mut] whether or
  not the mint batch is empty. [`append_tag_mint_rows`][append-mint] numbers
  each mint row `max + offset + 1` in the order of `tag_mints`, which is
  projection block order, and returns the start index.
- Mint inputs are built at [`:7156-7161`][mint-input] with
  `source_bytes: source.as_bytes().to_vec()` from [`taggable_source`][taggable],
  which returns the text of a user or assistant `Text` block or the first text
  of a tool result. The active-tag match at [`:7350`][active-match] compares
  `row.source_bytes == source.as_bytes()` for the same predicate.
- The commit takes mint inputs from `tag_rows[tag_mint_start..]`
  ([`:4924-4934`][commit-inputs]). The store numbers each new row from
  [`COALESCE(MAX(tag_number), 0) + 1`][store-number] inside the write
  transaction and skips existing block ids ([`:7140-7145`][store-existed]);
  `source_bytes` pass through
  [`write.bytes("source_bytes", ..)`][mint-prepared],
  where a detection refuses the insert unless the row existed.
- No code path in `transform.rs` stores the pass's `tag_rows` back into the
  cache; only `load_cached_tags` calls `replace`, with rows read from the store.

## Failure scenario

A design appends to the shared vector in place. The cache entry now holds mint
rows numbered by the pass, not the store. On the next pass `snapshot` returns
them; the active-tag match at [`:7350`][active-match] treats a speculative row
as active, and if the mint commit failed the row never existed. A design that
stores the pass's `Arc` back before commit has the same window. A design that
changes the `source_bytes` capture so it no longer equals the projected text
breaks the match for every row.

## Timing windows and dependencies

The exposure window is between `append_tag_mint_rows` and the commit's CAS
result; a failed CAS leaves the speculative rows uncommitted. The store side
has its own window: `load_cached_tags` appends `load_tags_after` only when the
summary is unchanged across two reads. The `source_bytes` identity depends on
the prepared-field path returning the bytes unchanged, which [R1][r1] owns.

## What a test must construct

Tagging active (a profile with `tool_present`), a warm baseline entry, a pass
that mints, then a second pass on the same session. For the aliasing arm, a
mint batch whose commit CAS fails so the speculative rows never land. After
each pass compare the entry's `tags` with `load_tags_for_session` for the
recorded `(store_namespace, generation, count, max_tag_number)`, check the
pass-local vector is baseline plus mint rows with `max + offset + 1`, and
compare every committed `source_bytes` with the block's `taggable_source`
text. The [shared-input checks](../existing-checks.md#shared-input-equivalence)
cover cold-versus-cached parity across five passes ([`t-tagcold`][t-tagcold]),
refill after a direct SQL update ([`t-poison`][t-poison]), and session
isolation ([`t-interleave`][t-interleave]); none fails a mint commit.

## Investigation log

### Q: Is the assumption that landed `source_bytes` are unchanged stated?

- Sources examined: [`write.bytes("source_bytes", ..)`][mint-prepared], the
  [mint input capture][mint-input], the parent's [R1][r1].
- Findings: The store either refuses the insert on a detection or, for an
  existing row, records the refusal and keeps the input bytes. No document
  states that a stored `source_bytes` equals its projected text; the active
  match at [`:7350`][active-match] depends on it.
- Missing evidence: A written statement in R1 or here.
- Conclusion: needs human input.

[tc-tagnum]: ../../../daemon/transform/catalog.md#speculative-tag-numbering-has-two-authorities
[r1]: ../../catalog.md#prepared-field-output-and-audit-policy-agree
[load-call]: ../../../../../crates/daemon/src/transform.rs#L3002
[commit-inputs]: ../../../../../crates/daemon/src/transform.rs#L4924-L4934
[tag-entry]: ../../../../../crates/daemon/src/transform.rs#L6782-L6807
[tag-snapshot]: ../../../../../crates/daemon/src/transform.rs#L6827-L6832
[load-tags]: ../../../../../crates/daemon/src/transform.rs#L6899-L6957
[mint-input]: ../../../../../crates/daemon/src/transform.rs#L7156-L7161
[append-mint]: ../../../../../crates/daemon/src/transform.rs#L7261-L7282
[taggable]: ../../../../../crates/daemon/src/transform.rs#L7286-L7310
[active-match]: ../../../../../crates/daemon/src/transform.rs#L7350
[make-mut]: ../../../../../crates/daemon/src/transform.rs#L7883-L7884
[t-tagcold]: ../../../../../crates/daemon/src/transform.rs#L22418
[t-poison]: ../../../../../crates/daemon/src/transform.rs#L22487
[t-interleave]: ../../../../../crates/daemon/src/transform.rs#L22520
[store-existed]: ../../../../../crates/memory-store/src/lib.rs#L7140-L7145
[mint-prepared]: ../../../../../crates/memory-store/src/lib.rs#L7156-L7164
[store-number]: ../../../../../crates/memory-store/src/lib.rs#L7205-L7209
[load-order]: ../../../../../crates/memory-store/src/lib.rs#L7245-L7273
