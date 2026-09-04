use base64::Engine as _;
use rustc_hash::FxHashSet;

use super::bpe::{DEAD, NO_RANK};

#[test]
fn vocab_blob_matches_claude_tiktoken() {
    let src = include_str!("../assets/claude.tiktoken");
    let vocab = super::vocab();
    let mut n = 0usize;
    let mut seen_ranks = FxHashSet::default();
    for line in src.lines().filter(|l| !l.is_empty()) {
        let (raw, rank) = line.split_once(' ').unwrap();
        let token = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .unwrap();
        let rank: u32 = rank.parse().unwrap();
        // The heap engine's stale-entry detection relies on every rank being unique and below
        // both sentinels.
        assert!(rank < DEAD, "{line}: rank collides with a sentinel");
        assert!(seen_ranks.insert(rank), "{line}: duplicate rank");
        assert_eq!(vocab.ranks.get(token.as_slice()), Some(&rank), "{line}");
        match token.as_slice() {
            [a] => assert_eq!(vocab.byte[*a as usize], rank),
            [a, b] => assert_eq!(vocab.pair[*a as usize * 256 + *b as usize], rank),
            _ => {}
        }
        n += 1;
    }
    assert_eq!(vocab.ranks.len(), n);
    let two_byte_entries = vocab.pair.iter().filter(|&&r| r != NO_RANK).count();
    assert_eq!(
        two_byte_entries,
        vocab.ranks.keys().filter(|k| k.len() == 2).count()
    );
}
