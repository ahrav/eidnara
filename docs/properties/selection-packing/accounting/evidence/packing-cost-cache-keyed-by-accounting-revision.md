# packing-cost-cache-keyed-by-accounting-revision

## Discovery trigger

RP2.8 acceptance row AC7 requires that a cost cached under one accounting
revision is not served under another; the U4a ticket extends the existing
two-generation cache key rather than adding a second cache.

## Evidence trail

- `crates/daemon/src/token_cache.rs` `AccountingRevision::from_components`
  length-prefixes each component so no pair can imitate another; the first
  component is the constructing authority (`exact` for
  `AccountingRevision::exact_tokenizer`, `heuristic` for
  `AccountingRevision::heuristic`), so a heuristic that repeats the exact
  identity and digest derives another key; the revision digests its text once
  and `cache_key` hashes that digest with the content digest, so the same
  content under two revisions has two keys. `count_with_digest` and
  `cached_estimate_tokens` key every existing caller under the exact tokenizer
  revision, which fingerprints the embedded vocabulary blob exposed by
  `tokenizer::vocab_blob`.
- `crates/daemon/src/packing/accounting.rs` `AccountingProfile::charge` counts
  through `cached_count_under` with the profile's revision.
- The cache unit test seeds two revisions, forces a rotation, and reads both
  back; the accounting test does the same through two profiles.

## Failure scenario

After a tokenizer upgrade a session would keep reading counts produced by the
old vocabulary until both generations rotated, under-charging or over-charging
every item meanwhile.

## Timing windows and dependencies

Concurrent misses may count the same content twice; the second insert stores
the same value.

## What a test must construct

- Two revisions with counting functions that disagree on one content.
- A heuristic profile built from the exact identity and vocabulary digest.
- A forced rotation of `current` into `previous`.
