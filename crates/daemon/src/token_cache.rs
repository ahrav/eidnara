//! Bounded token-count cache keyed by accounting revision and SHA-256 content
//! digest.
//!
//! `tokenizer::estimate_tokens` is a pure function of its input under one
//! vocabulary, so a count keyed by the accounting revision and the content
//! digest can be reused across passes and sessions without affecting any
//! rendered byte. Steady transform passes re-measure the same projected blocks
//! every pass; tail hygiene already computes a per-part SHA-256, so the lookup
//! key is nearly free on that path.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};

/// Names the estimator whose counts a cache entry holds; a count cached under
/// one revision is never served under another.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountingRevision {
    text: String,
    key: [u8; 32],
}

pub const EXACT_TOKENIZER_IDENTITY: &str = "claude-bpe";

impl AccountingRevision {
    fn new(text: String) -> Self {
        let key = Sha256::digest(text.as_bytes()).into();
        Self { text, key }
    }

    /// Length-prefixed components, so no component can imitate a boundary.
    pub fn from_components(components: &[&str]) -> Self {
        let text = components
            .iter()
            .map(|component| format!("{}:{component}", component.len()))
            .collect::<Vec<_>>()
            .join(";");
        Self::new(text)
    }

    pub fn heuristic(identity: &str, degradation: &str) -> Self {
        Self::from_components(&["heuristic", identity, degradation])
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The exact Claude BPE tokenizer this build links against, fingerprinted
    /// by its vocabulary bytes.
    pub fn exact_tokenizer() -> &'static Self {
        static EXACT: OnceLock<AccountingRevision> = OnceLock::new();
        EXACT.get_or_init(|| {
            Self::from_components(&[
                "exact",
                EXACT_TOKENIZER_IDENTITY,
                &format!("{:x}", Sha256::digest(tokenizer::vocab_blob())),
            ])
        })
    }

    fn cache_key(&self, content_digest: [u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.key);
        hasher.update(content_digest);
        hasher.finalize().into()
    }

    /// The key of raw `content` in one pass: `revision ‖ NUL ‖ content`. Raw contents are
    /// cached from [`MIN_CACHED_LEN`] bytes, so this preimage is longer than the 64-byte
    /// `revision ‖ digest` preimage of [`Self::cache_key`], and content that itself starts
    /// with tail hygiene's `"text\0"` cannot alias a digest-keyed entry.
    fn raw_key(&self, content: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.key);
        hasher.update([0u8]);
        hasher.update(content.as_bytes());
        hasher.finalize().into()
    }
}

/// Maximum entries in each current or previous generation.
const GENERATION_CAP: usize = 65_536;

/// Upper bound on heap the two generations can retain together, for the
/// component's resident-memory declaration.
///
/// hashbrown admits seven entries per eight buckets and sizes to a power of
/// two, so a generation holding `GENERATION_CAP` entries owns twice that many
/// buckets; each bucket stores the inline key, value, and one control byte.
pub(crate) const RETAINED_BYTES_BOUND: usize = {
    let buckets = GENERATION_CAP * 2;
    let bucket_bytes = std::mem::size_of::<[u8; 32]>() + std::mem::size_of::<u32>() + 1;
    2 * buckets * bucket_bytes
};

/// Contents shorter than this tokenize directly: hashing plus the lock
/// round-trip costs more than the BPE for tiny strings.
const MIN_CACHED_LEN: usize = 64;

#[derive(Default)]
struct Generations {
    current: HashMap<[u8; 32], u32>,
    previous: HashMap<[u8; 32], u32>,
}

static CACHE: Mutex<Option<Generations>> = Mutex::new(None);

/// Calling-thread cache counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TokenCacheStats {
    /// Lookups served from either generation.
    pub hits: u64,
    /// Lookups that required tokenization before insertion.
    pub misses: u64,
    /// Short inputs tokenized without hashing or locking.
    pub bypassed: u64,
    /// Total lookup calls.
    pub calls: u64,
    /// UTF-8 bytes passed to the tokenizer on misses or bypasses.
    pub tokenized_bytes: u64,
}

thread_local! {
    /// Thread-local counters exclude updates from other threads.
    static LOCAL: Cell<TokenCacheStats> = const {
        Cell::new(TokenCacheStats {
            hits: 0,
            misses: 0,
            bypassed: 0,
            calls: 0,
            tokenized_bytes: 0,
        })
    };
}

/// Counters for the calling thread, monotonic for the life of the thread.
///
/// `calls` equals `hits + misses + bypassed` in any single reading.
/// Only differences are meaningful.
pub(crate) fn local_stats() -> TokenCacheStats {
    LOCAL.with(Cell::get)
}

fn bump_local(update: impl FnOnce(&mut TokenCacheStats)) {
    LOCAL.with(|local| {
        let mut stats = local.get();
        update(&mut stats);
        local.set(stats);
    });
}

fn lock_cache() -> std::sync::MutexGuard<'static, Option<Generations>> {
    CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `current` is rotated before it exceeds `GENERATION_CAP`, so `previous`
/// remains within the per-generation bound; every insertion path must use
/// this helper.
fn insert_current(generations: &mut Generations, digest: [u8; 32], count: u32) {
    if generations.current.len() >= GENERATION_CAP {
        generations.previous = std::mem::take(&mut generations.current);
    }
    generations.current.insert(digest, count);
}

/// Exact tokenizer count for `content` whose digest the caller already
/// computed, keyed under the exact tokenizer revision.
///
/// Callers must hash a domain-separated, injective encoding of `content`.
/// Tail hygiene hashes `kind_name ‖ NUL ‖ content`; this module's raw path
/// keys by `revision ‖ NUL ‖ content` instead, a longer preimage than the
/// `revision ‖ digest` key this digest gets.
pub(crate) fn count_with_digest(digest: [u8; 32], content: &str) -> usize {
    count_under(
        AccountingRevision::exact_tokenizer(),
        digest,
        content,
        tokenizer::estimate_tokens,
    )
}

/// Count for `content` under `revision`, computed by `count` on a miss.
///
/// Concurrent misses may count the same content more than once. Insertion
/// remains bounded and later lookups return the stored count.
pub(crate) fn count_under(
    revision: &AccountingRevision,
    content_digest: [u8; 32],
    content: &str,
    count: impl FnOnce(&str) -> usize,
) -> usize {
    count_keyed(revision.cache_key(content_digest), content, count)
}

fn lookup(generations: &mut Generations, key: &[u8; 32]) -> Option<u32> {
    if let Some(&count) = generations.current.get(key) {
        return Some(count);
    }
    let count = *generations.previous.get(key)?;
    insert_current(generations, *key, count);
    Some(count)
}

fn lookup_locked(key: &[u8; 32]) -> Option<u32> {
    // ponytail: one global lock; shard per digest byte if concurrent sessions
    // ever contend here.
    let mut guard = lock_cache();
    lookup(guard.get_or_insert_with(Generations::default), key)
}

/// Count for `content` cached under the final cache key `digest`.
fn count_keyed(digest: [u8; 32], content: &str, count: impl FnOnce(&str) -> usize) -> usize {
    bump_local(|stats| stats.calls += 1);
    if let Some(count) = lookup_locked(&digest) {
        bump_local(|stats| stats.hits += 1);
        return count as usize;
    }
    count_missed(digest, content, count)
}

/// Records a miss and caches `count(content)` under `digest`. The caller has
/// already recorded the call.
fn count_missed(digest: [u8; 32], content: &str, count: impl FnOnce(&str) -> usize) -> usize {
    // Tokenize outside the lock: a 2 KiB payload costs ~80 us and would
    // serialize every concurrent session behind one merge loop.
    bump_local(|stats| {
        stats.misses += 1;
        stats.tokenized_bytes += content.len() as u64;
    });
    let count = count(content);
    // Return counts that exceed u32 uncached to avoid truncated cache hits.
    let Ok(cached) = u32::try_from(count) else {
        return count;
    };
    let mut guard = lock_cache();
    let generations = guard.get_or_insert_with(Generations::default);
    insert_current(generations, digest, cached);
    count
}

/// Clears both shared generations.
#[cfg(any(test, feature = "test-support"))]
pub fn clear() {
    let mut guard = lock_cache();
    *guard = Some(Generations::default());
}

/// Rotates `current` into `previous` without filling it to capacity.
#[cfg(any(test, feature = "test-support"))]
pub fn rotate() {
    let mut guard = lock_cache();
    let generations = guard.get_or_insert_with(Generations::default);
    generations.previous = std::mem::take(&mut generations.current);
}

/// Serializes tests whose assertions depend on shared cache contents.
///
/// Parallel test threads can clear a seeded cache hit before its assertion;
/// counter deltas remain thread-local.
#[cfg(test)]
pub(crate) fn test_cache_guard() -> std::sync::MutexGuard<'static, ()> {
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());
    CACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The exact tokenizer through this cache, as the production m0 composer counts.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ExactTokens;

impl crate::decay_render::TokenCount for ExactTokens {
    fn count(&self, text: &str) -> usize {
        cached_estimate_tokens(text)
    }

    fn counts_joins_exactly(&self) -> bool {
        true
    }
}

/// Drop-in replacement for `tokenizer::estimate_tokens` that hashes and
/// caches contents long enough to be worth it.
///
/// Content of at least `PARAGRAPH_CACHED_LEN` bytes with 2 through
/// `PARAGRAPH_MAX_SPANS` paragraph spans reuses cached span counts, so a body
/// that joins rows counted earlier reuses the counts of its matching spans.
pub(crate) fn cached_estimate_tokens(content: &str) -> usize {
    if content.len() >= PARAGRAPH_CACHED_LEN {
        return cached_paragraph_count(content);
    }
    cached_count_under(
        AccountingRevision::exact_tokenizer(),
        content,
        tokenizer::estimate_tokens,
    )
}

/// Contents shorter than this keep one entry, as rows and blocks counted on
/// their own do.
const PARAGRAPH_CACHED_LEN: usize = 4096;

/// Limits one call's span lookups and span entries to an eighth of a
/// generation. Span discovery collects one extra span to detect overflow, and
/// admission adds at most one whole-content entry beyond the span entries.
const PARAGRAPH_MAX_SPANS: usize = GENERATION_CAP / 8;

fn cached_paragraph_count(content: &str) -> usize {
    let revision = AccountingRevision::exact_tokenizer();
    let whole = revision.raw_key(content);
    bump_local(|stats| stats.calls += 1);
    if let Some(count) = lookup_locked(&whole) {
        bump_local(|stats| stats.hits += 1);
        return count as usize;
    }
    let spans: Vec<&str> = tokenizer::paragraph_spans(content)
        .take(PARAGRAPH_MAX_SPANS + 1)
        .collect();
    if !(2..=PARAGRAPH_MAX_SPANS).contains(&spans.len()) {
        return count_missed(whole, content, tokenizer::estimate_tokens);
    }
    let keys: Vec<Option<[u8; 32]>> = spans
        .iter()
        .map(|span| (span.len() >= MIN_CACHED_LEN).then(|| revision.raw_key(span)))
        .collect();
    let counts: Vec<Option<u32>> = {
        let mut guard = lock_cache();
        let generations = guard.get_or_insert_with(Generations::default);
        keys.iter()
            .map(|key| key.as_ref().and_then(|key| lookup(generations, key)))
            .collect()
    };
    let mut total = (spans.len() - 1) * tokenizer::estimate_tokens("\n");
    let mut tokenized_bytes = 0u64;
    let mut missed = Vec::new();
    for ((span, key), count) in spans.iter().zip(&keys).zip(&counts) {
        if let Some(count) = count {
            total += *count as usize;
            continue;
        }
        let count = tokenizer::estimate_tokens(span);
        tokenized_bytes += span.len() as u64;
        total += count;
        if let (Some(key), Ok(count)) = (key, u32::try_from(count)) {
            missed.push((*key, count));
        }
    }
    bump_local(|stats| {
        if tokenized_bytes == 0 {
            stats.hits += 1;
        } else {
            stats.misses += 1;
        }
        stats.tokenized_bytes += tokenized_bytes;
    });
    if let Ok(count) = u32::try_from(total) {
        missed.push((whole, count));
    }
    let mut guard = lock_cache();
    let generations = guard.get_or_insert_with(Generations::default);
    for (key, count) in missed {
        insert_current(generations, key, count);
    }
    total
}

/// `count` under `revision` for contents long enough to be worth caching.
pub(crate) fn cached_count_under(
    revision: &AccountingRevision,
    content: &str,
    count: impl FnOnce(&str) -> usize,
) -> usize {
    if content.len() < MIN_CACHED_LEN {
        bump_local(|stats| {
            stats.calls += 1;
            stats.bypassed += 1;
            stats.tokenized_bytes += content.len() as u64;
        });
        return count(content);
    }
    count_keyed(revision.raw_key(content), content, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_counts_match_the_tokenizer() {
        for content in [
            "",
            "short",
            "a longer sentence that clears the minimum cached length threshold easily",
            "fn main() { println!(\"hello, tokenizer cache\"); }\n// with a second line so BPE has structure",
        ] {
            assert_eq!(
                cached_estimate_tokens(content),
                tokenizer::estimate_tokens(content),
                "cached count diverged for {content:?}"
            );
            // Second call exercises the hit path; the count must not change.
            assert_eq!(
                cached_estimate_tokens(content),
                tokenizer::estimate_tokens(content)
            );
        }
    }

    #[test]
    fn paragraph_joins_count_exactly_from_their_rows() {
        let _guard = test_cache_guard();
        let rows: Vec<String> = (0..120)
            .map(|i| {
                let tail = ["", " ", "\u{3000}", "\t"][i % 4];
                format!(
                    "## {i}-{i} · paragraph join fixture {i}{tail}\n{}",
                    "fold cache tier ".repeat(i % 7 + 3)
                )
            })
            .collect();
        let body = rows.join("\n\n");
        let wrapped = format!("<session-history>\n{body}\n</session-history>");
        assert!(body.len() >= PARAGRAPH_CACHED_LEN);
        assert_eq!(
            tokenizer::paragraph_spans(&body).count(),
            rows.len(),
            "every join cuts"
        );
        for row in &rows[..rows.len() - 1] {
            cached_estimate_tokens(&format!("{row}\n"));
        }
        cached_estimate_tokens(&rows[rows.len() - 1]);
        let before = local_stats();
        assert_eq!(
            cached_estimate_tokens(&body),
            tokenizer::estimate_tokens(&body)
        );
        let after = local_stats();
        assert_eq!(
            (after.calls - before.calls, after.hits - before.hits),
            (1, 1)
        );
        assert_eq!(after.tokenized_bytes, before.tokenized_bytes);
        assert_eq!(
            cached_estimate_tokens(&wrapped),
            tokenizer::estimate_tokens(&wrapped)
        );
        let blank_runs = format!("{body}\n\n\n\n{body}\n\n \n\n{body}");
        for _ in 0..2 {
            assert_eq!(
                cached_estimate_tokens(&blank_runs),
                tokenizer::estimate_tokens(&blank_runs)
            );
        }
    }

    #[test]
    fn a_repeated_short_paragraph_content_hits_without_tokenizing() {
        let _guard = test_cache_guard();
        clear();
        let short: Vec<String> = (0..700).map(|i| format!("note {i}")).collect();
        let mixed: Vec<String> = (0..300)
            .map(|i| {
                if i % 2 == 0 {
                    format!("## {i} · a paragraph long enough to be cached under its own key")
                } else {
                    format!("aside {i}")
                }
            })
            .collect();
        for parts in [short, mixed] {
            let content = parts.join("\n\n");
            assert!(content.len() >= PARAGRAPH_CACHED_LEN);
            assert!(tokenizer::paragraph_spans(&content).count() > 1);
            assert_eq!(
                cached_estimate_tokens(&content),
                tokenizer::estimate_tokens(&content)
            );
            let before = local_stats();
            assert_eq!(
                cached_estimate_tokens(&content),
                tokenizer::estimate_tokens(&content)
            );
            let after = local_stats();
            assert_eq!(
                (after.calls - before.calls, after.hits - before.hits),
                (1, 1)
            );
            assert_eq!(after.tokenized_bytes, before.tokenized_bytes);
        }
    }

    #[test]
    fn a_paragraph_count_that_tokenizes_records_a_miss() {
        let _guard = test_cache_guard();
        clear();
        let content = "x\n\n".repeat(1_400);
        assert!(content.len() >= PARAGRAPH_CACHED_LEN);
        let before = local_stats();
        assert_eq!(
            cached_estimate_tokens(&content),
            tokenizer::estimate_tokens(&content)
        );
        let after = local_stats();
        assert_eq!(after.calls - before.calls, 1);
        assert_eq!(after.hits - before.hits, 0);
        assert_eq!(after.misses - before.misses, 1);
        assert!(after.tokenized_bytes > before.tokenized_bytes);
    }

    #[test]
    fn a_paragraph_count_admits_a_bounded_number_of_span_entries() {
        let _guard = test_cache_guard();
        clear();
        let spans: Vec<String> = (0..PARAGRAPH_MAX_SPANS + 1_000)
            .map(|i| format!("## {i:06} · a distinct paragraph long enough for its own entry"))
            .collect();
        let content = spans.join("\n\n");
        assert_eq!(tokenizer::paragraph_spans(&content).count(), spans.len());
        assert_eq!(
            cached_estimate_tokens(&content),
            tokenizer::estimate_tokens(&content)
        );
        let revision = AccountingRevision::exact_tokenizer();
        let admitted = {
            let guard = lock_cache();
            let generations = guard.as_ref().expect("the count initialized the cache");
            tokenizer::paragraph_spans(&content)
                .map(|span| revision.raw_key(span))
                .filter(|key| {
                    generations.current.contains_key(key) || generations.previous.contains_key(key)
                })
                .count()
        };
        assert!(admitted <= PARAGRAPH_MAX_SPANS, "{admitted} span entries");
        let tiny = "x\n\n".repeat(PARAGRAPH_MAX_SPANS * 4);
        assert_eq!(
            cached_estimate_tokens(&tiny),
            tokenizer::estimate_tokens(&tiny)
        );
    }

    #[test]
    fn insert_current_rotates_at_capacity() {
        let mut generations = Generations::default();
        for i in 0..GENERATION_CAP {
            let mut key = [0u8; 32];
            key[..8].copy_from_slice(&(i as u64).to_le_bytes());
            generations.current.insert(key, 1);
        }
        // The promote-on-hit path also inserts through this helper, so a full
        // `current` must rotate rather than grow past the cap.
        insert_current(&mut generations, [0xAA; 32], 7);
        assert!(generations.current.len() <= GENERATION_CAP);
        assert_eq!(generations.current.get(&[0xAA; 32]), Some(&7));
        assert_eq!(generations.previous.len(), GENERATION_CAP);
    }

    #[test]
    fn stats_partition_calls_into_hits_misses_and_bypassed() {
        let _guard = test_cache_guard();
        let long =
            "stats partition fixture: unique sentence long enough to clear the cache threshold";
        assert!(long.len() >= MIN_CACHED_LEN);
        let before = local_stats();
        cached_estimate_tokens("tiny");
        cached_estimate_tokens(long);
        cached_estimate_tokens(long);
        let after = local_stats();
        assert_eq!(after.calls - before.calls, 3);
        assert_eq!(after.bypassed - before.bypassed, 1);
        assert_eq!(after.misses - before.misses, 1);
        assert_eq!(after.hits - before.hits, 1);
    }

    #[test]
    fn a_count_cached_under_one_revision_is_not_served_under_another() {
        let _guard = test_cache_guard();
        clear();
        let content = "revision isolation fixture: long enough to be cached under both revisions";
        assert!(content.len() >= MIN_CACHED_LEN);
        let first = AccountingRevision::heuristic("profile-a", "1");
        let second = AccountingRevision::heuristic("profile-a", "2");
        assert_eq!(cached_count_under(&first, content, |_| 7), 7);
        assert_eq!(
            cached_count_under(&first, content, |_| 99),
            7,
            "same revision hits"
        );
        assert_eq!(
            cached_count_under(&second, content, |_| 11),
            11,
            "another revision misses and counts afresh"
        );
        assert_eq!(cached_count_under(&first, content, |_| 99), 7);
        rotate();
        assert_eq!(
            cached_count_under(&second, content, |_| 99),
            11,
            "rotation keeps the entry"
        );
        assert_eq!(cached_count_under(&first, content, |_| 99), 7);
        let exact = AccountingRevision::exact_tokenizer().as_str();
        let prefix = format!(
            "5:exact;{}:{EXACT_TOKENIZER_IDENTITY};64:",
            EXACT_TOKENIZER_IDENTITY.len()
        );
        assert!(exact.starts_with(&prefix), "{exact}");
        assert_eq!(exact.len(), prefix.len() + 64);
        assert_ne!(
            AccountingRevision::from_components(&["a@b", "c"]),
            AccountingRevision::from_components(&["a", "b@c"])
        );
        assert_ne!(
            AccountingRevision::heuristic("exact", "x"),
            AccountingRevision::from_components(&["exact", "x"]),
            "the authority tag is part of the encoding"
        );
    }

    #[test]
    fn kind_prefixed_and_raw_content_keys_do_not_alias() {
        let inner =
            "the retry loop needs a jittered backoff so clients spread out their reconnects ";
        // Raw content that byte-for-byte equals tail hygiene's preimage for
        // `inner` under the `text` kind.
        let adversarial = format!("text\0{inner}");
        assert!(adversarial.len() >= MIN_CACHED_LEN);
        let expected_inner = tokenizer::estimate_tokens(inner);
        let expected_adversarial = tokenizer::estimate_tokens(&adversarial);
        assert_ne!(
            expected_inner, expected_adversarial,
            "fixture must discriminate the two counts"
        );
        // Seed the cache the way tail hygiene does for (kind=text, inner).
        let hygiene_digest: [u8; 32] = Sha256::digest(adversarial.as_bytes()).into();
        assert_eq!(count_with_digest(hygiene_digest, inner), expected_inner);
        // The raw path must not read that entry back for `adversarial`.
        assert_eq!(cached_estimate_tokens(&adversarial), expected_adversarial);
    }
}
