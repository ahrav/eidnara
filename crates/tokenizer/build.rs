//! Decodes `assets/claude.tiktoken` once at build time into `$OUT_DIR/vocab.bin` so the first
//! call pays for hash inserts only, not 65k base64 decodes and allocations, and precomputes the
//! two-bit BMP class table the scanner uses. `src/vocab_blob_tests.rs` re-derives both from
//! the sources and asserts equality.
//!
//! `vocab.bin` layout, little-endian: `u32 count`, then `count` records of `u16 len, u32 rank`,
//! then the concatenated token bytes in record order.

use std::io::Write as _;
use std::path::PathBuf;

use base64::Engine as _;

#[allow(dead_code)]
#[path = "src/unicode_tables.rs"]
mod unicode_tables;

fn in_ranges(table: &[(u32, u32)], c: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if c < lo {
                std::cmp::Ordering::Greater
            } else if c > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Same classification as `scan::class_from_tables`; 0 letter, 1 number, 2 space, 3 other.
fn class(c: u32) -> u8 {
    if c < 0x80 {
        let b = c as u8;
        return if b.is_ascii_alphabetic() {
            0
        } else if b.is_ascii_digit() {
            1
        } else if matches!(b, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ') {
            2
        } else {
            3
        };
    }
    match c {
        0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF => 2,
        _ if in_ranges(unicode_tables::LETTER, c) => 0,
        _ if in_ranges(unicode_tables::NUMBER, c) => 1,
        _ => 3,
    }
}

fn main() {
    println!("cargo::rerun-if-changed=assets/claude.tiktoken");
    println!("cargo::rerun-if-changed=src/unicode_tables.rs");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let src = std::fs::read_to_string("assets/claude.tiktoken").unwrap();
    let mut records = Vec::new();
    let mut bytes = Vec::new();
    for line in src.lines().filter(|l| !l.is_empty()) {
        let (raw, rank) = line.split_once(' ').expect("vocab line");
        let token = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .expect("vocab token base64");
        let rank: u32 = rank.parse().expect("vocab rank");
        assert!(token.len() <= u16::MAX as usize);
        records.push((token.len() as u16, rank));
        bytes.extend_from_slice(&token);
    }
    let mut blob = Vec::with_capacity(4 + records.len() * 6 + bytes.len());
    blob.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for (len, rank) in &records {
        blob.extend_from_slice(&len.to_le_bytes());
        blob.extend_from_slice(&rank.to_le_bytes());
    }
    blob.extend_from_slice(&bytes);
    std::fs::File::create(out.join("vocab.bin"))
        .unwrap()
        .write_all(&blob)
        .unwrap();

    let mut bmp = vec![0u8; 0x4000];
    for c in 0..0x10000u32 {
        bmp[(c >> 2) as usize] |= class(c) << ((c & 3) * 2);
    }
    std::fs::write(out.join("bmp_class.bin"), bmp).unwrap();
}
