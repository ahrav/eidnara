//! Hand-written pre-tokenizer equivalent to `CLAUDE_PAT_STR` under leftmost-first alternation:
//!
//! ```text
//! 's|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+
//! ```
//!
//! with `\s` meaning the ECMAScript WhiteSpace + LineTerminator set. Every character belongs to
//! exactly one of four classes (letter, number, whitespace, other), so each alternative after the
//! contractions is "optional U+0020, then a maximal run of one class", and the two whitespace
//! alternatives collapse to one rule: take the maximal whitespace run; if a non-whitespace
//! character follows and the run has at least two characters, give the last one back. That is
//! what `\s+(?!\S)` backtracks to, and `\s+` only wins when the run is a single character.
//!
//! `parity_tests` proves equivalence against the `fancy-regex` reference on random and
//! adversarial inputs; `unicode_gen_tests` pins the `\p{L}`/`\p{N}` tables to the regex crate's
//! Unicode version.

use crate::unicode_tables::{LETTER, NUMBER};

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Class {
    Letter = 0,
    Number = 1,
    Space = 2,
    Other = 3,
}

const ASCII_CLASS: [Class; 128] = {
    let mut t = [Class::Other; 128];
    let mut i = 0;
    while i < 128 {
        let b = i as u8;
        t[i] = if b.is_ascii_alphabetic() {
            Class::Letter
        } else if b.is_ascii_digit() {
            Class::Number
        } else if matches!(b, b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ') {
            Class::Space
        } else {
            Class::Other
        };
        i += 1;
    }
    t
};

/// Requires sorted, disjoint, inclusive ranges.
const fn in_ranges(table: &[(u32, u32)], c: u32) -> bool {
    let mut lo = 0;
    let mut hi = table.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let (a, b) = table[mid];
        if c < a {
            hi = mid;
        } else if c > b {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

const fn class_from_tables(c: u32) -> Class {
    if c < 0x80 {
        return ASCII_CLASS[c as usize];
    }
    match c {
        0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF => {
            Class::Space
        }
        _ if in_ranges(LETTER, c) => Class::Letter,
        _ if in_ranges(NUMBER, c) => Class::Number,
        _ => Class::Other,
    }
}

/// Two bits per BMP code point (16 KiB). Evaluated at compile time so the hot path carries no
/// first-use build and no lazy-init read. `Class` discriminants are the stored values.
static BMP_CLASS: [u8; 0x4000] = {
    let mut t = [0u8; 0x4000];
    let mut c = 0u32;
    while c < 0x10000 {
        t[(c >> 2) as usize] |= (class_from_tables(c) as u8) << ((c & 3) * 2);
        c += 1;
    }
    t
};

#[inline]
fn class_of_scalar(c: u32) -> Class {
    if c < 0x80 {
        return ASCII_CLASS[c as usize];
    }
    if c < 0x10000 {
        let v = (BMP_CLASS[(c >> 2) as usize] >> ((c & 3) * 2)) & 3;
        return match v {
            0 => Class::Letter,
            1 => Class::Number,
            2 => Class::Space,
            _ => Class::Other,
        };
    }
    class_from_tables(c)
}

/// Class and encoded length of the character starting at `i`. `bytes` is valid UTF-8 and `i`
/// is a char boundary, so the lead byte fixes the length and no validation is needed.
#[inline]
fn class_at(bytes: &[u8], i: usize) -> (Class, usize) {
    let b0 = bytes[i];
    if b0 < 0x80 {
        return (ASCII_CLASS[b0 as usize], 1);
    }
    let (cp, len) = if b0 < 0xE0 {
        (((b0 as u32 & 0x1F) << 6) | (bytes[i + 1] as u32 & 0x3F), 2)
    } else if b0 < 0xF0 {
        (
            ((b0 as u32 & 0x0F) << 12)
                | ((bytes[i + 1] as u32 & 0x3F) << 6)
                | (bytes[i + 2] as u32 & 0x3F),
            3,
        )
    } else {
        (
            ((b0 as u32 & 0x07) << 18)
                | ((bytes[i + 1] as u32 & 0x3F) << 12)
                | ((bytes[i + 2] as u32 & 0x3F) << 6)
                | (bytes[i + 3] as u32 & 0x3F),
            4,
        )
    };
    (class_of_scalar(cp), len)
}

const HI: u64 = 0x8080_8080_8080_8080;
const LO: u64 = 0x0101_0101_0101_0101;

/// Bit 7 of each byte set where `m < byte < n`, for words whose bytes are all below 0x80
/// (`x + (0x7F - m)` overflows into bit 7 exactly when `byte > m`, `(0x7F + n) - x` exactly
/// when `byte < n`, and neither carries across byte lanes).
#[inline]
fn between(x: u64, m: u8, n: u8) -> u64 {
    (LO * (0x7F + n as u64)).wrapping_sub(x) & x.wrapping_add(LO * (0x7F - m as u64)) & HI
}

/// Bit 7 of each byte set where `byte == v`, for words whose bytes are all below 0x80. A
/// lane of `x ^ v` is zero exactly when it stays below 0x80 after adding 0x7F; the add cannot
/// carry between lanes because every lane is at most 0x7F + 0x7F.
#[inline]
fn eq(x: u64, v: u8) -> u64 {
    !((x ^ (LO * v as u64)).wrapping_add(LO * 0x7F)) & HI
}

/// Bit 7 of each byte set where the byte belongs to `class`; the word must be all ASCII.
#[inline]
fn ascii_class_mask(w: u64, class: Class) -> u64 {
    match class {
        Class::Letter => between(w | (LO * 0x20), 0x60, 0x7B),
        Class::Number => between(w, 0x2F, 0x3A),
        Class::Space => between(w, 0x08, 0x0E) | eq(w, 0x20),
        Class::Other => {
            !(between(w | (LO * 0x20), 0x60, 0x7B)
                | between(w, 0x2F, 0x3A)
                | between(w, 0x08, 0x0E)
                | eq(w, 0x20))
                & HI
        }
    }
}

/// End of the maximal run of `class` starting at `i`. Eight ASCII bytes per step while the
/// input stays ASCII, then one character at a time.
#[inline]
fn run_end(bytes: &[u8], mut i: usize, class: Class) -> usize {
    while let Some(chunk) = bytes.get(i..i + 8) {
        let w = u64::from_le_bytes(chunk.try_into().unwrap());
        if w & HI != 0 {
            break;
        }
        let m = ascii_class_mask(w, class);
        if m == HI {
            i += 8;
            continue;
        }
        return i + ((!m & HI).trailing_zeros() / 8) as usize;
    }
    while i < bytes.len() {
        let (c, len) = class_at(bytes, i);
        if c != class {
            break;
        }
        i += len;
    }
    i
}

/// End offset of the pre-token piece starting at `pos` (`pos < text.len()`, char boundary).
#[inline]
fn piece_end(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let b0 = bytes[pos];
    if b0 < 0x80 {
        // ASCII lead byte: the class comes from the table and the two special leads (`'`
        // for contractions, space for the optional-space prefix) are exact byte tests.
        if b0 == b'\'' && pos + 1 < n {
            match bytes[pos + 1] {
                b's' | b't' | b'm' | b'd' => return pos + 2,
                b'r' | b'v' if pos + 2 < n && bytes[pos + 2] == b'e' => return pos + 3,
                b'l' if pos + 2 < n && bytes[pos + 2] == b'l' => return pos + 3,
                _ => {}
            }
        }
        let c0 = ASCII_CLASS[b0 as usize];
        if b0 == b' ' && pos + 1 < n {
            let (c1, l1) = class_at(bytes, pos + 1);
            if c1 != Class::Space {
                return run_end(bytes, pos + 1 + l1, c1);
            }
        }
        if c0 != Class::Space {
            return run_end(bytes, pos + 1, c0);
        }
        return whitespace_piece_end(bytes, pos, 1);
    }
    let (c0, l0) = class_at(bytes, pos);
    if c0 != Class::Space {
        return run_end(bytes, pos + l0, c0);
    }
    whitespace_piece_end(bytes, pos, l0)
}

/// Whitespace run starting at `pos` (first char `l0` bytes long): give back the last character
/// when a non-whitespace character follows, unless the run is that single character.
#[inline]
fn whitespace_piece_end(bytes: &[u8], pos: usize, l0: usize) -> usize {
    let end = run_end(bytes, pos + l0, Class::Space);
    if end == bytes.len() || end == pos + l0 {
        return end;
    }
    let mut last = end - 1;
    while bytes[last] & 0xC0 == 0x80 {
        last -= 1;
    }
    last
}

/// Byte ranges of the pre-token pieces of `text`, in order, covering it exactly.
pub fn pieces(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut pos = 0;
    std::iter::from_fn(move || {
        if pos >= text.len() {
            return None;
        }
        let end = piece_end(text, pos);
        let span = (pos, end);
        pos = end;
        Some(span)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_table_matches_range_tables() {
        for c in 0..0x10000u32 {
            assert!(class_of_scalar(c) == class_from_tables(c), "{c:#x}");
        }
    }

    /// The SWAR class masks must agree with the byte table for every ASCII byte in every lane,
    /// with every other lane holding an arbitrary ASCII byte (catches cross-lane carries).
    #[test]
    fn swar_masks_match_ascii_table() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let w = state & !HI;
            for class in [Class::Letter, Class::Number, Class::Space, Class::Other] {
                let m = ascii_class_mask(w, class);
                for lane in 0..8 {
                    let b = (w >> (8 * lane)) as u8;
                    let expect = ASCII_CLASS[b as usize] == class;
                    assert_eq!((m >> (8 * lane + 7)) & 1 == 1, expect, "{b:#x} {lane}");
                }
            }
        }
    }

    fn split(text: &str) -> Vec<&str> {
        pieces(text).map(|(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn matches_reference_on_hand_cases() {
        for text in [
            "hello world",
            "I'm sure it's you've done well, they'll see, we're 'ready'.",
            "a    b\t\tc\n\nd",
            "trailing   ",
            "  leading",
            " x",
            " ",
            "\n",
            "\n\n",
            "\nx",
            "x\u{feff}\n",
            "wait \u{85} what\u{85} mojibake a \u{85}b",
            "全角\u{3000}スペース\u{3000}test",
            "e\u{301} a\u{300} cafe\u{301}",
            "1234567890 42 3.14159 1,000,000",
            "'sa 'ta 're 'r 'l 'll 'lla '' ' '",
            "obj.valueOf(); __proto__",
            "👋 🌍 👨‍👩‍👧‍👦 🏳️‍🌈",
            "  \t\r\n  x  \n",
        ] {
            let reference: Vec<&str> = crate::reference_impl::piece_spans(text)
                .into_iter()
                .map(|(s, e)| &text[s..e])
                .collect();
            assert_eq!(split(text), reference, "{text:?}");
        }
    }
}
