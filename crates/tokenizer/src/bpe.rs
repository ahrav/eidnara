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

use rustc_hash::FxHashMap;

use crate::Rank;

/// Sentinel for "no vocabulary entry for this span".
pub const NO_RANK: Rank = Rank::MAX;
/// Marks a part that has been merged into its left neighbour (heap engine only).
const DEAD: Rank = Rank::MAX - 1;
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
    heap: BinaryHeap<Reverse<(Rank, u32)>>,
}

/// Vocabulary views the merge loop needs.
pub struct Vocab {
    /// Byte string -> rank for every token.
    pub ranks: FxHashMap<Vec<u8>, Rank>,
    /// Rank of the 2-byte token `[a, b]` at `a * 256 + b`, or `NO_RANK`.
    pub pair: Box<[Rank; 65536]>,
    /// Rank of the single byte `b` at `b`.
    pub byte: [Rank; 256],
}

impl Vocab {
    pub fn new(ranks: FxHashMap<Vec<u8>, Rank>) -> Self {
        let mut pair: Box<[Rank; 65536]> = vec![NO_RANK; 65536].try_into().unwrap();
        let mut byte = [NO_RANK; 256];
        for (k, &r) in &ranks {
            match k.as_slice() {
                [a] => byte[*a as usize] = r,
                [a, b] => pair[*a as usize * 256 + *b as usize] = r,
                _ => {}
            }
        }
        assert!(
            byte.iter().all(|&r| r != NO_RANK),
            "vocabulary must cover every byte"
        );
        Vocab { ranks, pair, byte }
    }

    #[inline]
    fn rank_of(&self, span: &[u8]) -> Rank {
        match span {
            [a] => self.byte[*a as usize],
            [a, b] => self.pair[*a as usize * 256 + *b as usize],
            _ => self.ranks.get(span).copied().unwrap_or(NO_RANK),
        }
    }

    /// Appends the ids of `piece` (1..=MAX_PIECE_BYTES bytes) to `out`.
    pub fn encode_piece(&self, piece: &[u8], scratch: &mut Scratch, out: &mut Vec<Rank>) {
        debug_assert!(!piece.is_empty());
        let whole = self.rank_of(piece);
        if whole != NO_RANK {
            out.push(whole);
            return;
        }
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
            self.merge_scan(piece, &mut scratch.parts, out);
        } else {
            self.merge_heap(piece, scratch, out);
        }
    }

    /// `parts[i]` is `(start, rank of the pair starting at start)`.
    fn merge_scan(&self, piece: &[u8], parts: &mut Vec<(u32, Rank)>, out: &mut Vec<Rank>) {
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
                parts[i - 1].1 = self.span_rank(piece, parts, i - 1, i + 2);
            }
            parts[i].1 = self.span_rank(piece, parts, i, i + 3);
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
            out.push(self.rank_of(&piece[w[0].0 as usize..w[1].0 as usize]));
        }
    }

    /// Part `i` spans `piece[i..next[i]]`; `rank[i]` is the rank of `piece[i..next[next[i]]]`
    /// (the pair of part `i` and the part after it), `NO_RANK` at the end, `DEAD` once merged.
    fn merge_heap(&self, piece: &[u8], s: &mut Scratch, out: &mut Vec<Rank>) {
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
                s.heap.push(Reverse((r, i as u32)));
            }
        }
        s.rank.push(NO_RANK);

        while let Some(Reverse((r, i))) = s.heap.pop() {
            let i = i as usize;
            if s.rank[i] != r {
                continue;
            }
            let j = s.next[i] as usize;
            let after = s.next[j];
            s.next[i] = after;
            s.rank[j] = DEAD;
            if after < end {
                s.prev[after as usize] = i as u32;
                let r = self.rank_of(&piece[i..s.next[after as usize] as usize]);
                s.rank[i] = r;
                if r != NO_RANK {
                    s.heap.push(Reverse((r, i as u32)));
                }
            } else {
                s.rank[i] = NO_RANK;
            }
            let p = s.prev[i];
            if p != u32::MAX {
                let p = p as usize;
                let r = self.rank_of(&piece[p..after as usize]);
                s.rank[p] = r;
                if r != NO_RANK {
                    s.heap.push(Reverse((r, p as u32)));
                }
            }
        }
        let mut i = 0usize;
        while i < n {
            let j = s.next[i] as usize;
            out.push(self.rank_of(&piece[i..j]));
            i = j;
        }
    }

    /// Rank of the span `parts[i].0 .. parts[end].0`, or `NO_RANK` if `end` is past the last
    /// real part (the pair would run off the piece).
    #[inline]
    fn span_rank(&self, piece: &[u8], parts: &[(u32, Rank)], i: usize, end: usize) -> Rank {
        if end < parts.len() {
            self.rank_of(&piece[parts[i].0 as usize..parts[end].0 as usize])
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
            vocab.merge_scan(&piece, &mut scratch.parts, &mut a);
            vocab.merge_heap(&piece, &mut scratch, &mut b);
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
                    vocab.merge_scan(&piece, &mut s.parts, &mut out);
                }
                let scan = t.elapsed().as_nanos() as f64 / (iters * len) as f64;
                let t = std::time::Instant::now();
                for _ in 0..iters {
                    out.clear();
                    vocab.merge_heap(&piece, &mut s, &mut out);
                }
                let heap = t.elapsed().as_nanos() as f64 / (iters * len) as f64;
                eprintln!("{name:6} len={len:5} scan={scan:8.2} heap={heap:8.2} ns/B");
            }
        }
    }
}
