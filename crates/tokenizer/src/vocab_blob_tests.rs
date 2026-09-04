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
        match token.as_slice() {
            [a] => assert_eq!(vocab.byte[*a as usize], rank),
            [a, b] => assert_eq!(vocab.pair[*a as usize * 256 + *b as usize], rank),
            t if t.len() <= 15 => {
                let mut ids = Vec::new();
                vocab.encode_piece(t, 0, t.len(), &mut Default::default(), &mut ids);
                assert_eq!(ids, [rank], "{line}");
            }
            t => assert_eq!(vocab.ranks.get(t), Some(&rank), "{line}"),
        }
        n += 1;
    }
    assert_eq!(
        vocab.ranks.len() + vocab.short.len() + vocab.mid.len() + 256 + two_byte(vocab),
        n
    );
}

fn two_byte(vocab: &super::bpe::Vocab) -> usize {
    vocab.pair.iter().filter(|&&r| r != NO_RANK).count()
}
