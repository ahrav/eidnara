//! The encoder writes `&str` values as JSON strings byte-identical to `serde_json` output.

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGHS: u64 = 0x8080_8080_8080_8080;

/// `escape_marks` sets the high bit of each byte that JSON escapes: bytes below 0x20, `"`, or
/// `\`. A subtraction borrow can also mark bytes above the lowest marked one, so only the lowest
/// set bit locates an escaped byte exactly.
#[inline(always)]
fn escape_marks(word: u64) -> u64 {
    let quote = word ^ (ONES * u64::from(b'"'));
    let backslash = word ^ (ONES * u64::from(b'\\'));
    (word.wrapping_sub(ONES * 0x20)
        | (quote.wrapping_sub(ONES) & !quote)
        | (backslash.wrapping_sub(ONES) & !backslash))
        & !word
        & HIGHS
}

/// `push_escape` emits escapes byte-identical to `serde_json`.
#[inline(always)]
fn push_escape(out: &mut Vec<u8>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let short = |code: u8| ([b'\\', code, 0, 0, 0, 0], 2);
    let (escape, len) = match byte {
        b'"' => short(b'"'),
        b'\\' => short(b'\\'),
        0x08 => short(b'b'),
        b'\t' => short(b't'),
        b'\n' => short(b'n'),
        0x0c => short(b'f'),
        b'\r' => short(b'r'),
        _ => (
            [
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0x0f)],
            ],
            6,
        ),
    };
    let kept = out.len() + len;
    out.extend_from_slice(&escape);
    out.truncate(kept);
}

/// `push_json_string` appends `value` to `out` as a quoted JSON string, byte-identical to
/// `serde_json::to_writer`.
pub(crate) fn push_json_string(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    out.reserve(bytes.len() + 2);
    out.push(b'"');
    let mut run = 0;
    let mut at = 0;
    loop {
        while let Some(block) = bytes.get(at..at + 16) {
            let (low, high) = block.split_at(8);
            let low = u64::from_le_bytes(low.try_into().unwrap_or_default());
            let high = u64::from_le_bytes(high.try_into().unwrap_or_default());
            if escape_marks(low) | escape_marks(high) != 0 {
                break;
            }
            at += 16;
        }
        let Some(chunk) = bytes.get(at..at + 8) else {
            break;
        };
        let word: [u8; 8] = chunk.try_into().unwrap_or_default();
        let marks = escape_marks(u64::from_le_bytes(word));
        if marks == 0 {
            at += 8;
            continue;
        }
        let lane = (marks.trailing_zeros() / 8) as usize;
        out.extend_from_slice(&bytes[run..at]);
        let kept = out.len() + lane;
        out.extend_from_slice(&word);
        out.truncate(kept);
        push_escape(out, word[lane]);
        run = at + lane + 1;
        at = run;
    }
    for (index, &byte) in bytes.iter().enumerate().skip(at) {
        if byte < 0x20 || byte == b'"' || byte == b'\\' {
            out.extend_from_slice(&bytes[run..index]);
            push_escape(out, byte);
            run = index + 1;
        }
    }
    out.extend_from_slice(&bytes[run..]);
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::push_json_string;

    fn encoded(value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        push_json_string(&mut out, value);
        out
    }

    #[test]
    fn every_byte_and_run_length_escapes_as_serde_json_does() {
        let all: String = (0u32..0x800).filter_map(char::from_u32).collect();
        for skip in 0..64 {
            for take in 0..40 {
                let value: String = all.chars().skip(skip).take(take).collect();
                assert_eq!(
                    encoded(&value),
                    serde_json::to_vec(&value).unwrap(),
                    "{value:?}"
                );
            }
        }
    }

    /// Lengths 0 through 47 put one or two escaped bytes on both sides of every eight- and
    /// sixteen-byte boundary.
    #[test]
    fn an_escape_at_every_offset_of_a_clean_run_encodes_as_serde_json_does() {
        let escaped = [
            '\0', '\u{1f}', '\u{8}', '\t', '\n', '\u{c}', '\r', '"', '\\',
        ];
        for len in 0..48 {
            let clean = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOP"[..len].to_owned();
            assert_eq!(encoded(&clean), serde_json::to_vec(&clean).unwrap());
            for first in 0..len {
                for &byte in &escaped {
                    let mut value = clean.clone();
                    value.replace_range(first..=first, &byte.to_string());
                    assert_eq!(
                        encoded(&value),
                        serde_json::to_vec(&value).unwrap(),
                        "{value:?}"
                    );
                    for second in first + 1..len {
                        let mut pair = value.clone();
                        pair.replace_range(second..=second, "\"");
                        assert_eq!(
                            encoded(&pair),
                            serde_json::to_vec(&pair).unwrap(),
                            "{pair:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn multibyte_text_around_escapes_encodes_as_serde_json_does() {
        for value in [
            "\u{7f}\u{80}\u{ff}\u{2028}\u{2029}\u{fffd}\u{10ffff}\u{1f642}",
            "\u{e9}\"\u{65e5}\u{672c}\\\n\u{1f642}\u{1f642}\u{1f642}\u{1f642}\t",
            "\"\"\"\"\"\"\"\"\"\"\"\"\"\"\"\"\"",
            "\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\\",
        ] {
            assert_eq!(
                encoded(value),
                serde_json::to_vec(value).unwrap(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn the_string_appends_after_existing_bytes() {
        let mut out = b"{\"k\":".to_vec();
        push_json_string(&mut out, "a\"b");
        assert_eq!(out, b"{\"k\":\"a\\\"b\"");
    }
}
