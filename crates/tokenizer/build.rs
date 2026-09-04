//! Decodes `assets/claude.tiktoken` once at build time into `$OUT_DIR/vocab.bin` so the first
//! call pays for hash inserts only, not 65k base64 decodes and allocations.
//! `src/vocab_blob_tests.rs` re-derives the blob from the asset and asserts equality.
//!
//! `vocab.bin` layout, little-endian: `u32 count`, then `count` records of `u16 len, u32 rank`,
//! then the concatenated token bytes in record order.

use std::io::Write as _;
use std::path::PathBuf;

use base64::Engine as _;

fn main() {
    println!("cargo::rerun-if-changed=assets/claude.tiktoken");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    let asset = manifest_dir.join("assets/claude.tiktoken");
    let src =
        std::fs::read_to_string(&asset).unwrap_or_else(|e| panic!("read {}: {e}", asset.display()));
    let mut records = Vec::new();
    let mut bytes = Vec::new();
    for line in src.lines().filter(|l| !l.is_empty()) {
        let (raw, rank) = line.split_once(' ').expect("vocab line");
        let token = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .expect("vocab token base64");
        let rank: u32 = rank.parse().expect("vocab rank");
        let len = u16::try_from(token.len()).expect("vocab token longer than u16::MAX");
        records.push((len, rank));
        bytes.extend_from_slice(&token);
    }
    let count = u32::try_from(records.len()).expect("vocab record count exceeds u32::MAX");
    let mut blob = Vec::with_capacity(4 + records.len() * 6 + bytes.len());
    blob.extend_from_slice(&count.to_le_bytes());
    for (len, rank) in &records {
        blob.extend_from_slice(&len.to_le_bytes());
        blob.extend_from_slice(&rank.to_le_bytes());
    }
    blob.extend_from_slice(&bytes);
    std::fs::File::create(out.join("vocab.bin"))
        .expect("create the vocabulary blob in OUT_DIR")
        .write_all(&blob)
        .expect("write the vocabulary blob");
}
