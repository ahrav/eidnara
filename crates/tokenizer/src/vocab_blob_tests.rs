//! The `build.rs` outputs must equal what the sources say: `vocab.bin` against a fresh base64
//! decode of `assets/claude.tiktoken`, and the vocabulary object against the blob.

use base64::Engine as _;

use super::bpe::NO_RANK;

#[test]
fn vocab_blob_matches_claude_tiktoken() {
    let src = include_str!("../assets/claude.tiktoken");
    let vocab = super::vocab();
    let mut n = 0usize;
    for line in src.lines().filter(|l| !l.is_empty()) {
        let (raw, rank) = line.split_once(' ').unwrap();
        let token = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .unwrap();
        let rank: u32 = rank.parse().unwrap();
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
