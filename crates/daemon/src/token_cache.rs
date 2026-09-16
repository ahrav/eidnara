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

impl AccountingRevision {
    pub fn new(text: String) -> Self {
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

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The exact Claude BPE tokenizer this build links against, fingerprinted
    /// by its vocabulary bytes.
    pub fn exact_tokenizer() -> &'static Self {
        static EXACT: OnceLock<AccountingRevision> = OnceLock::new();
        EXACT.get_or_init(|| {
            Self::from_components(&[
                "claude-bpe",
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
/// hashes `NUL ‖ content`, which no kind name can prefix.
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
    let digest = revision.cache_key(content_digest);
    bump_local(|stats| stats.calls += 1);
    // ponytail: one global lock; shard per digest byte if concurrent sessions
    // ever contend here.
    {
        let mut guard = lock_cache();
        let generations = guard.get_or_insert_with(Generations::default);
        if let Some(&count) = generations.current.get(&digest) {
            bump_local(|stats| stats.hits += 1);
            return count as usize;
        }
        if let Some(&count) = generations.previous.get(&digest) {
            bump_local(|stats| stats.hits += 1);
            insert_current(generations, digest, count);
            return count as usize;
        }
    }
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
#[cfg(any(test, feature = "bench-internals"))]
pub fn clear() {
    let mut guard = lock_cache();
    *guard = Some(Generations::default());
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

/// Drop-in replacement for `tokenizer::estimate_tokens` that hashes and
/// caches contents long enough to be worth it.
pub(crate) fn cached_estimate_tokens(content: &str) -> usize {
    cached_count_under(
        AccountingRevision::exact_tokenizer(),
        content,
        tokenizer::estimate_tokens,
    )
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
    // The leading NUL keeps this key domain disjoint from tail hygiene's
    // `kind_name ‖ NUL ‖ content` keys, so content that itself starts with
    // `"text\0"` cannot alias another entry's count.
    let mut hasher = Sha256::new();
    hasher.update([0u8]);
    hasher.update(content.as_bytes());
    count_under(revision, hasher.finalize().into(), content, count)
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
        let first = AccountingRevision::new("profile-a@1".to_owned());
        let second = AccountingRevision::new("profile-a@2".to_owned());
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
        let mut guard = lock_cache();
        let generations = guard.get_or_insert_with(Generations::default);
        generations.previous = std::mem::take(&mut generations.current);
        drop(guard);
        assert_eq!(
            cached_count_under(&second, content, |_| 99),
            11,
            "rotation keeps the entry"
        );
        assert_eq!(cached_count_under(&first, content, |_| 99), 7);
        let exact = AccountingRevision::exact_tokenizer().as_str();
        assert!(exact.starts_with("10:claude-bpe;64:"), "{exact}");
        assert_eq!(exact.len(), "10:claude-bpe;64:".len() + 64);
        assert_ne!(
            AccountingRevision::from_components(&["a@b", "c"]),
            AccountingRevision::from_components(&["a", "b@c"])
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
