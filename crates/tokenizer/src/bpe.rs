//! Byte-pair merging with tiktoken's exact order: repeatedly merge the adjacent pair with the
//! lowest rank, ties to the leftmost pair, until no adjacent pair is in the vocabulary.
//!
//! Two equivalent engines pick the same pair at every step:
//!
//! - `merge_scan`: tiktoken's O(n·m) rescan (n parts, m merges) in a reusable scratch buffer,
//!   initial pair ranks from a 256x256 table, ids written straight into the output. Wins for the
//!   short pieces real text produces.
//! - `merge_heap`: a doubly linked list of parts plus a min-heap of `(rank, position)` for every
//!   live pair with a finite rank, O((n + m) log n). The heap's minimum is the lowest rank and,
//!   among equal ranks, the leftmost position, which is exactly tiktoken's choice. Entries go
//!   stale when the part at their position is merged away or its pair rank changes; both are
//!   detected on pop and skipped. A stale entry can never masquerade as current because a
//!   recomputed pair spans a strictly longer byte string and vocabulary ranks are unique.
//!
//! `HEAP_THRESHOLD` selects the engine by piece length; `parity_tests` exercises both.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use hashbrown::HashTable;
use rustc_hash::FxHashMap;

use crate::Rank;

/// Sentinel for "no vocabulary entry for this span".
pub const NO_RANK: Rank = Rank::MAX;
/// Marks a part that has been merged into its left neighbour (heap engine only).
pub(crate) const DEAD: Rank = Rank::MAX - 1;
/// Pieces longer than this use the heap engine; measured crossover on this host is ~192 B for
/// ASCII runs and ~32 B for CJK (`bpe::crossover::engine_crossover`, ignored test).
pub const HEAP_THRESHOLD: usize = 192;
/// Heap threshold for pieces whose first byte after an optional leading space is non-ASCII.
pub const HEAP_THRESHOLD_NON_ASCII: usize = 40;

/// Per-thread scratch reused across pieces so the hot path never allocates.
#[derive(Default)]
pub struct Scratch {
    parts: Vec<(u32, Rank)>,
    next: Vec<u32>,
    prev: Vec<u32>,
    rank: Vec<Rank>,
    /// `(rank << 32) | position`, packed so the heap compares one `u64`. Lowest rank pops first;
    /// ties go to the leftmost position.
    heap: BinaryHeap<Reverse<u64>>,
}

/// Vocabulary views the merge loop needs.
pub struct Vocab {
    /// Byte string -> rank for tokens of 16 bytes or more. Keys borrow the embedded blob,
    /// avoiding per-token key allocations.
    pub ranks: FxHashMap<&'static [u8], Rank>,
    /// Tokens of 3..=7 bytes as `(packed key, rank)` with the key inline in the bucket: one
    /// cache line per probe and a single-register compare, where the byte-slice map costs a
    /// bucket line, a pointer chase to the key bytes, and a `memcmp` call.
    pub short: HashTable<(u64, Rank)>,
    /// Tokens of 8..=15 bytes, same idea with a 128-bit inline key; CJK merges look up 9- and
    /// 12-byte spans constantly and paid the byte-slice map's pointer chase and `memcmp`.
    pub mid: HashTable<(u128, Rank)>,
    /// Rank of the 2-byte token `[a, b]` at `a * 256 + b`, or `NO_RANK`.
    pub pair: Box<[Rank; 65536]>,
    /// Rank of the single byte `b` at `b`.
    pub byte: [Rank; 256],
}

/// Packed key for a 3..=7-byte token in `text[start..start + len]`: bytes little-endian in the
/// low 56 bits, length in the top byte. One unaligned 8-byte load when the text extends that
/// far (the common case inside a longer text), so the hot path has no length-dependent loop.
#[inline]
fn short_key(text: &[u8], start: usize, len: usize) -> u64 {
    debug_assert!((3..=7).contains(&len));
    let word = match text.get(start..start + 8) {
        Some(w) => u64::from_le_bytes(w.try_into().unwrap()),
        None => {
            let mut b = [0u8; 8];
            b[..len].copy_from_slice(&text[start..start + len]);
            u64::from_le_bytes(b)
        }
    };
    (word & (u64::MAX >> (64 - 8 * len))) | ((len as u64) << 56)
}

/// Packed key for an 8..=15-byte token: bytes little-endian in the low 120 bits, length in the
/// top byte. One unaligned 16-byte load when the text extends that far.
#[inline]
fn mid_key(text: &[u8], start: usize, len: usize) -> u128 {
    debug_assert!((8..=15).contains(&len));
    let word = match text.get(start..start + 16) {
        Some(w) => u128::from_le_bytes(w.try_into().unwrap()),
        None => {
            let mut b = [0u8; 16];
            b[..len].copy_from_slice(&text[start..start + len]);
            u128::from_le_bytes(b)
        }
    };
    (word & (u128::MAX >> (128 - 8 * len))) | ((len as u128) << 120)
}

#[inline]
fn mid_hash(key: u128) -> u64 {
    short_hash((key as u64) ^ ((key >> 64) as u64).rotate_left(29))
}

/// One multiply then fold the high half down: hashbrown takes the bucket index from the low
/// bits and the tag from the top 7, and the low bits of a bare product depend only on the low
/// bits of the key, so keys differing only in their upper bytes would collide.
#[inline]
fn short_hash(key: u64) -> u64 {
    let h = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^ (h >> 32)
}

impl Vocab {
    /// Parses the `build.rs` blob (`u32 count`, `count` x (`u16 len`, `u32 rank`), bytes) straight
    /// into the lookup tables; each token is inserted once.
    pub fn from_blob(blob: &'static [u8]) -> Self {
        let count = u32::from_le_bytes(blob[..4].try_into().unwrap()) as usize;
        let (header, mut bytes) = blob[4..].split_at(count * 6);
        let records = header.as_chunks::<6>().0;
        let len_of = |rec: &[u8; 6]| u16::from_le_bytes([rec[0], rec[1]]);
        let (mut n_short, mut n_mid, mut n_long) = (0usize, 0usize, 0usize);
        for rec in records {
            match len_of(rec) {
                3..=7 => n_short += 1,
                8..=15 => n_mid += 1,
                16.. => n_long += 1,
                _ => {}
            }
        }
        let mut v = Vocab {
            ranks: FxHashMap::with_capacity_and_hasher(n_long, Default::default()),
            short: HashTable::with_capacity(n_short),
            mid: HashTable::with_capacity(n_mid),
            pair: vec![NO_RANK; 65536].try_into().unwrap(),
            byte: [NO_RANK; 256],
        };
        for rec in records {
            let len = u16::from_le_bytes([rec[0], rec[1]]) as usize;
            let rank = Rank::from_le_bytes([rec[2], rec[3], rec[4], rec[5]]);
            let (token, rest) = bytes.split_at(len);
            bytes = rest;
            v.insert(token, rank);
        }
        assert!(bytes.is_empty(), "vocab blob has trailing bytes");
        assert!(
            v.byte.iter().all(|&r| r != NO_RANK),
            "vocabulary must cover every byte"
        );
        v
    }

    fn insert(&mut self, token: &'static [u8], rank: Rank) {
        match token {
            [a] => self.byte[*a as usize] = rank,
            [a, b] => self.pair[*a as usize * 256 + *b as usize] = rank,
            _ if token.len() <= 7 => {
                let key = short_key(token, 0, token.len());
                self.short
                    .insert_unique(short_hash(key), (key, rank), |e| short_hash(e.0));
            }
            _ if token.len() <= 15 => {
                let key = mid_key(token, 0, token.len());
                self.mid
                    .insert_unique(mid_hash(key), (key, rank), |e| mid_hash(e.0));
            }
            _ => {
                self.ranks.insert(token, rank);
            }
        }
    }

    /// Rank of `text[a..b]` or `NO_RANK`. Takes the enclosing text so a short key can be read
    /// with one 8-byte load.
    #[inline(always)]
    fn rank_of(&self, text: &[u8], a: usize, b: usize) -> Rank {
        match b - a {
            1 => self.byte[text[a] as usize],
            2 => self.pair[text[a] as usize * 256 + text[a + 1] as usize],
            len @ 3..=7 => {
                let key = short_key(text, a, len);
                self.short
                    .find(short_hash(key), |e| e.0 == key)
                    .map_or(NO_RANK, |e| e.1)
            }
            len @ 8..=15 => {
                let key = mid_key(text, a, len);
                self.mid
                    .find(mid_hash(key), |e| e.0 == key)
                    .map_or(NO_RANK, |e| e.1)
            }
            _ => self.ranks.get(&text[a..b]).copied().unwrap_or(NO_RANK),
        }
    }

    /// Appends the ids of the piece `text[start..end]` (1..=MAX_PIECE_BYTES bytes) to `out`.
    /// Takes the whole text so short-token keys can be read with one 8-byte load.
    #[inline]
    pub fn encode_piece(
        &self,
        text: &[u8],
        start: usize,
        end: usize,
        scratch: &mut Scratch,
        out: &mut Vec<Rank>,
    ) {
        debug_assert!(start < end && end <= text.len());
        let whole = self.rank_of(text, start, end);
        if whole != NO_RANK {
            out.push(whole);
            return;
        }
        let piece = &text[start..end];
        // Multi-byte text merges many more times per byte (a CJK char is one 3-byte token and
        // pairs merge well), so the heap pays off from ~32 bytes there; ASCII runs of one
        // class merge little and the rescan wins to ~192 bytes (`crossover::engine_crossover`).
        // ` ?\p{L}+` prefixes a letter run with a space, so classify the piece by its following
        // byte.
        debug_assert!(piece.len() >= 2, "single bytes are vocabulary entries");
        let lead = if piece[0] == b' ' { piece[1] } else { piece[0] };
        let threshold = if lead < 0x80 {
            HEAP_THRESHOLD
        } else {
            HEAP_THRESHOLD_NON_ASCII
        };
        if piece.len() <= threshold {
            self.merge_scan(text, start, end, &mut scratch.parts, out);
        } else {
            self.merge_heap(text, start, end, scratch, out);
        }
    }

    /// `parts[i]` is `(piece-relative start offset, rank of the pair starting there)`. A
    /// piece-relative offset stays below `MAX_PIECE_BYTES`, so `u32` holds it wherever the piece
    /// sits in `text`; lookups add `start` to recover the absolute position.
    fn merge_scan(
        &self,
        text: &[u8],
        start: usize,
        end: usize,
        parts: &mut Vec<(u32, Rank)>,
        out: &mut Vec<Rank>,
    ) {
        let piece = &text[start..end];
        let n = piece.len();
        parts.clear();
        parts.reserve(n + 1);
        let mut min_rank = NO_RANK;
        let mut min_idx = 0usize;
        for i in 0..n - 1 {
            let r = self.pair[piece[i] as usize * 256 + piece[i + 1] as usize];
            if r < min_rank {
                min_rank = r;
                min_idx = i;
            }
            parts.push((i as u32, r));
        }
        parts.push(((n - 1) as u32, NO_RANK));
        parts.push((n as u32, NO_RANK));

        while min_rank != NO_RANK {
            let i = min_idx;
            // Merge parts[i] with parts[i+1]: the span of the new part i runs to parts[i+2].
            // Recompute the pair rank ending at i and the one starting at i before removing.
            if i > 0 {
                parts[i - 1].1 = self.span_rank(text, start, parts, i - 1, i + 2);
            }
            parts[i].1 = self.span_rank(text, start, parts, i, i + 3);
            parts.remove(i + 1);

            min_rank = NO_RANK;
            for (j, &(_, r)) in parts[..parts.len() - 1].iter().enumerate() {
                if r < min_rank {
                    min_rank = r;
                    min_idx = j;
                }
            }
        }
        for w in parts.windows(2) {
            out.push(self.rank_of(text, start + w[0].0 as usize, start + w[1].0 as usize));
        }
    }

    /// Part `i` spans `piece[i..next[i]]`; `rank[i]` is the rank of `piece[i..next[next[i]]]`
    /// (the pair of part `i` and the part after it), `NO_RANK` at the end, `DEAD` once merged.
    fn merge_heap(
        &self,
        text: &[u8],
        start: usize,
        end: usize,
        s: &mut Scratch,
        out: &mut Vec<Rank>,
    ) {
        let piece = &text[start..end];
        let n = piece.len();
        let end = n as u32;
        s.next.clear();
        s.prev.clear();
        s.rank.clear();
        s.heap.clear();
        s.next.extend(1..=end);
        s.prev.push(u32::MAX);
        s.prev.extend(0..end - 1);
        for i in 0..n - 1 {
            let r = self.pair[piece[i] as usize * 256 + piece[i + 1] as usize];
            s.rank.push(r);
            if r != NO_RANK {
                s.heap.push(Reverse(((r as u64) << 32) | i as u64));
            }
        }
        s.rank.push(NO_RANK);

        while let Some(Reverse(key)) = s.heap.pop() {
            let (r, i) = ((key >> 32) as Rank, key as u32 as usize);
            if s.rank[i] != r {
                continue;
            }
            let j = s.next[i] as usize;
            let after = s.next[j];
            s.next[i] = after;
            s.rank[j] = DEAD;
            if after < end {
                s.prev[after as usize] = i as u32;
                let r = self.rank_of(text, start + i, start + s.next[after as usize] as usize);
                s.rank[i] = r;
                if r != NO_RANK {
                    s.heap.push(Reverse(((r as u64) << 32) | i as u64));
                }
            } else {
                s.rank[i] = NO_RANK;
            }
            let p = s.prev[i];
            if p != u32::MAX {
                let p = p as usize;
                let r = self.rank_of(text, start + p, start + after as usize);
                s.rank[p] = r;
                if r != NO_RANK {
                    s.heap.push(Reverse(((r as u64) << 32) | p as u64));
                }
            }
        }
        let mut i = 0usize;
        while i < n {
            let j = s.next[i] as usize;
            out.push(self.rank_of(text, start + i, start + j));
            i = j;
        }
    }

    /// Rank of the span `parts[i].0 .. parts[end].0`, or `NO_RANK` if `end` is past the last
    /// real part (the pair would run off the piece).
    #[inline]
    fn span_rank(
        &self,
        text: &[u8],
        start: usize,
        parts: &[(u32, Rank)],
        i: usize,
        end: usize,
    ) -> Rank {
        if end < parts.len() {
            self.rank_of(
                text,
                start + parts[i].0 as usize,
                start + parts[end].0 as usize,
            )
        } else {
            NO_RANK
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both engines must agree on every input, including ones far below `HEAP_THRESHOLD`
    /// (the threshold is a performance choice, not a correctness boundary). Inputs are random
    /// bytes from small alphabets so equal-rank ties between distant pairs are common; the CJK
    /// alphabet yields mostly invalid UTF-8, which the engines must also agree on.
    #[test]
    fn heap_and_scan_engines_agree() {
        let vocab = crate::vocab();
        let mut scratch = Scratch::default();
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabets: [&[u8]; 4] = [
            b"aab ",
            b"the and of to in",
            "你好世界の".as_bytes(),
            b"0123456789.,-",
        ];
        for case in 0..20_000 {
            let alphabet = alphabets[case % alphabets.len()];
            let len = 2 + (next() % 200) as usize;
            let piece: Vec<u8> = (0..len)
                .map(|_| alphabet[(next() as usize) % alphabet.len()])
                .collect();
            let mut a = Vec::new();
            let mut b = Vec::new();
            vocab.merge_scan(&piece, 0, piece.len(), &mut scratch.parts, &mut a);
            vocab.merge_heap(&piece, 0, piece.len(), &mut scratch, &mut b);
            assert_eq!(
                a,
                b,
                "engines differ on {:?}",
                String::from_utf8_lossy(&piece)
            );
        }
    }
}

#[cfg(test)]
mod crossover {
    use super::*;

    /// Timing probe for `HEAP_THRESHOLD`; prints ns/byte per engine and piece length. A unit
    /// starting with a space contributes that space once, as ` ?\p{L}+` would.
    #[test]
    #[ignore = "timing probe"]
    fn engine_crossover() {
        let vocab = crate::vocab();
        let mut s = Scratch::default();
        for (name, unit) in [
            ("space", " "),
            ("a", "a"),
            ("cjk", "你"),
            (" cjk", " 你"),
            ("mixed", "the fox "),
        ] {
            for len in [32usize, 64, 128, 192, 256, 512, 4096] {
                let (head, body) = match unit.strip_prefix(' ') {
                    Some(rest) if !rest.is_empty() => (" ", rest),
                    _ => ("", unit),
                };
                let piece: Vec<u8> = head
                    .bytes()
                    .chain(body.as_bytes().iter().copied().cycle())
                    .take(len)
                    .collect();
                let iters = 200_000 / len + 1;
                let mut out = Vec::new();
                let t = std::time::Instant::now();
                for _ in 0..iters {
                    out.clear();
                    vocab.merge_scan(&piece, 0, len, &mut s.parts, &mut out);
                }
                let scan = t.elapsed().as_nanos() as f64 / (iters * len) as f64;
                let t = std::time::Instant::now();
                for _ in 0..iters {
                    out.clear();
                    vocab.merge_heap(&piece, 0, len, &mut s, &mut out);
                }
                let heap = t.elapsed().as_nanos() as f64 / (iters * len) as f64;
                eprintln!("{name:6} len={len:5} scan={scan:8.2} heap={heap:8.2} ns/B");
            }
        }
    }
}
