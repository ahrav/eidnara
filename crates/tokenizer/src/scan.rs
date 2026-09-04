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
enum Class {
    Letter,
    Number,
    Space,
    Other,
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

fn class_of_scalar(c: u32) -> Class {
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

/// End of the maximal run of `class` starting at `i`.
#[inline]
fn run_end(bytes: &[u8], mut i: usize, class: Class) -> usize {
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
pub fn piece_end(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let n = bytes.len();
    if bytes[pos] == b'\'' && pos + 1 < n {
        match bytes[pos + 1] {
            b's' | b't' | b'm' | b'd' => return pos + 2,
            b'r' | b'v' if pos + 2 < n && bytes[pos + 2] == b'e' => return pos + 3,
            b'l' if pos + 2 < n && bytes[pos + 2] == b'l' => return pos + 3,
            _ => {}
        }
    }
    let (c0, l0) = class_at(bytes, pos);
    if bytes[pos] == b' ' && pos + 1 < n {
        let (c1, l1) = class_at(bytes, pos + 1);
        if c1 != Class::Space {
            return run_end(bytes, pos + 1 + l1, c1);
        }
    }
    if c0 != Class::Space {
        return run_end(bytes, pos + l0, c0);
    }
    // Whitespace run: give back the last character when a non-whitespace character follows,
    // unless the run is that single character.
    let mut last_start = pos;
    let mut i = pos + l0;
    while i < n {
        let (c, len) = class_at(bytes, i);
        if c != Class::Space {
            return if last_start == pos { i } else { last_start };
        }
        last_start = i;
        i += len;
    }
    n
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
