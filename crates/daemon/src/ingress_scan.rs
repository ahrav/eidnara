//! One pass over a request body that answers the reads admission and lane selection make of it.
//!
//! [`scan`] is one pass that yields everything admission and lane selection read from a body's
//! bytes: the top-level route discriminators, whether a page field is present, the value count
//! behind the footprint floor, and the longest escaped string. It answers only for a body whose
//! every byte it can account for under the grammar serde_json applies to the entry probe: a
//! JSON object with no control byte, top-level keys spelled without escapes, and route values
//! that are plain strings or literals. For such a body each answer equals the one the reference
//! reads produce, `probe_request`, [`crate::metered_decode::footprint_floor`], and the
//! escaped-string scan; for every other body it returns `None` and the caller runs those reads.

use crate::{RouteName, TRANSFORM_PAGE_FIELDS};

/// A top-level route discriminator as the probe reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScannedRoute<'a> {
    /// Missing, `null`, or a boolean.
    Absent,
    /// A string spelled without escapes, at most [`RouteName::MAX_ROUTE_NAME_BYTES`] long.
    Named(&'a str),
    /// A longer string.
    Overlong,
}

/// What the pass read from a body it fully accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyScan<'a> {
    pub(crate) method: ScannedRoute<'a>,
    pub(crate) kind: ScannedRoute<'a>,
    pub(crate) page_fields: bool,
    /// Every value in the body, object keys included.
    pub(crate) values: usize,
    /// The raw length of the longest string holding an escape; zero when none does.
    pub(crate) longest_escaped: usize,
}

/// The deepest nesting the pass tracks; a deeper body goes to the reference reads.
const MAX_DEPTH: usize = 256;

/// Scans `body`; `None` when the body falls outside the grammar the pass accounts for.
pub(crate) fn scan(body: &[u8]) -> Option<BodyScan<'_>> {
    let mut scanner = Scanner {
        body,
        at: 0,
        values: 0,
        longest_escaped: 0,
    };
    let mut method = ScannedRoute::Absent;
    let mut kind = ScannedRoute::Absent;
    let mut page_fields = false;
    scanner.skip_spaces();
    // A body that does not open an object declines before the control-byte pass reads it all.
    scanner.expect(b'{')?;
    if has_control_byte(body) {
        return None;
    }
    scanner.values += 1;
    scanner.skip_spaces();
    if scanner.peek() == Some(b'}') {
        scanner.at += 1;
    } else {
        loop {
            scanner.expect(b'"')?;
            let key = scanner.key()?;
            scanner.skip_spaces();
            scanner.expect(b':')?;
            scanner.skip_spaces();
            match key {
                "method" => method = scanner.route()?,
                "kind" => kind = scanner.route()?,
                key => {
                    page_fields |= TRANSFORM_PAGE_FIELDS.contains(&key);
                    scanner.value()?;
                }
            }
            scanner.skip_spaces();
            match scanner.next()? {
                b',' => scanner.skip_spaces(),
                b'}' => break,
                _ => return None,
            }
        }
    }
    scanner.skip_spaces();
    (scanner.at == body.len()).then_some(BodyScan {
        method,
        kind,
        page_fields,
        values: scanner.values,
        longest_escaped: scanner.longest_escaped,
    })
}

/// Whether any byte of `body` is below 0x20. Folding fixed chunks to their minimum keeps the
/// check branch-free inside a chunk, which lets the compiler vectorize it.
fn has_control_byte(body: &[u8]) -> bool {
    let (chunks, rest) = body.as_chunks::<64>();
    chunks
        .iter()
        .any(|chunk| chunk.iter().fold(u8::MAX, |least, &byte| least.min(byte)) < 0x20)
        || rest.iter().any(|&byte| byte < 0x20)
}

const ONES: u64 = u64::MAX / 0xff;
const HIGHS: u64 = ONES << 7;

/// The words a string search checks in place before `memchr2` scans the rest of the run.
const WORDS: usize = 4;

/// The high bit of each byte of an eight-byte `word` equal to `byte`. A flag above a true one
/// can be false, so only the lowest flag of the result, or of an OR of such results, is exact.
#[inline(always)]
fn equal_bytes(word: u64, byte: u8) -> u64 {
    let differs = word ^ (ONES * u64::from(byte));
    differs.wrapping_sub(ONES) & !differs & HIGHS
}

/// The offset of the first `"` or `\\` at or after `at`.
#[inline(always)]
fn quote_or_backslash(body: &[u8], mut at: usize) -> Option<usize> {
    for _ in 0..WORDS {
        let Some(chunk) = body.get(at..at + 8) else {
            break;
        };
        let word = u64::from_le_bytes(chunk.try_into().ok()?);
        let hits = equal_bytes(word, b'"') | equal_bytes(word, b'\\');
        if hits != 0 {
            return Some(at + hits.trailing_zeros() as usize / 8);
        }
        at += 8;
    }
    memchr::memchr2(b'"', b'\\', body.get(at..)?).map(|found| at + found)
}

struct Scanner<'a> {
    body: &'a [u8],
    at: usize,
    values: usize,
    longest_escaped: usize,
}

impl<'a> Scanner<'a> {
    fn peek(&self) -> Option<u8> {
        self.body.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.next()? == byte).then_some(())
    }

    /// With no control byte in the body, a space is the only whitespace byte left.
    fn skip_spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.at += 1;
        }
    }

    /// Reads the string whose opening quote was just consumed and returns its raw bytes and
    /// whether it holds an escape. Each escape must be one serde_json accepts when it skips a
    /// string: a short escape or `\u` and four hex digits.
    fn string(&mut self) -> Option<(&'a [u8], bool)> {
        let body = self.body;
        let start = self.at;
        let mut at = start;
        let mut escaped = false;
        loop {
            at = quote_or_backslash(body, at)?;
            if body[at] == b'"' {
                break;
            }
            escaped = true;
            match *body.get(at + 1)? {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => at += 2,
                b'u' => {
                    let digits = body.get(at + 2..at + 6)?;
                    if !digits.iter().all(u8::is_ascii_hexdigit) {
                        return None;
                    }
                    at += 6;
                }
                _ => return None,
            }
        }
        let raw = &body[start..at];
        self.at = at + 1;
        self.values += 1;
        if escaped {
            self.longest_escaped = self.longest_escaped.max(raw.len());
        }
        Some((raw, escaped))
    }

    /// A top-level key, which the probe reads as text: one spelled with an escape is left to
    /// the reference probe, and its bytes must be UTF-8.
    fn key(&mut self) -> Option<&'a str> {
        match self.string()? {
            (raw, false) => std::str::from_utf8(raw).ok(),
            (_, true) => None,
        }
    }

    /// A route discriminator's value. A plain string is kept by name, or as overlong past
    /// [`RouteName::MAX_ROUTE_NAME_BYTES`]; a literal names no route. Every other value is left
    /// to the reference probe.
    fn route(&mut self) -> Option<ScannedRoute<'a>> {
        match self.peek()? {
            b'"' => {
                self.at += 1;
                let (raw, false) = self.string()? else {
                    return None;
                };
                let name = std::str::from_utf8(raw).ok()?;
                Some(if name.len() <= RouteName::MAX_ROUTE_NAME_BYTES {
                    ScannedRoute::Named(name)
                } else {
                    ScannedRoute::Overlong
                })
            }
            b't' | b'f' | b'n' => {
                self.literal()?;
                Some(ScannedRoute::Absent)
            }
            _ => None,
        }
    }

    /// Reads `true`, `false`, or `null`; callers have peeked a `t`, an `f`, or an `n`.
    fn literal(&mut self) -> Option<()> {
        let rest = self.body.get(self.at..)?;
        let word: &[u8] = match rest.first()? {
            b't' => b"true",
            b'f' => b"false",
            _ => b"null",
        };
        if !rest.starts_with(word) {
            return None;
        }
        self.at += word.len();
        self.values += 1;
        Some(())
    }

    /// The number grammar serde_json checks when it skips a number.
    fn number(&mut self) -> Option<()> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.next()? {
            b'0' => {
                if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    return None;
                }
            }
            b'1'..=b'9' => self.digits(),
            _ => return None,
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            self.peek().filter(u8::is_ascii_digit)?;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            self.peek().filter(u8::is_ascii_digit)?;
            self.digits();
        }
        self.values += 1;
        Some(())
    }

    fn digits(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
    }

    /// Skips one complete value, nested containers included, with the checks serde_json
    /// applies when it skips a value.
    fn value(&mut self) -> Option<()> {
        // `objects[depth]` is set when the container open at that depth is an object.
        let mut objects = [false; MAX_DEPTH];
        let mut depth = 0usize;
        loop {
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    self.string()?;
                }
                open @ (b'{' | b'[') => {
                    self.at += 1;
                    self.values += 1;
                    self.skip_spaces();
                    let close = if open == b'{' { b'}' } else { b']' };
                    if self.peek() == Some(close) {
                        self.at += 1;
                    } else {
                        if depth == MAX_DEPTH {
                            return None;
                        }
                        objects[depth] = open == b'{';
                        if objects[depth] {
                            self.member_key()?;
                        }
                        depth += 1;
                        self.skip_spaces();
                        continue;
                    }
                }
                b't' | b'f' | b'n' => self.literal()?,
                b'-' | b'0'..=b'9' => self.number()?,
                _ => return None,
            }
            loop {
                if depth == 0 {
                    return Some(());
                }
                let object = objects[depth - 1];
                self.skip_spaces();
                match (self.next()?, object) {
                    (b',', true) => {
                        self.skip_spaces();
                        self.member_key()?;
                        self.skip_spaces();
                        break;
                    }
                    (b',', false) => {
                        self.skip_spaces();
                        break;
                    }
                    (b'}', true) | (b']', false) => depth -= 1,
                    _ => return None,
                }
            }
        }
    }

    /// A nested object's key and its colon.
    fn member_key(&mut self) -> Option<()> {
        self.skip_spaces();
        self.expect(b'"')?;
        self.string()?;
        self.skip_spaces();
        self.expect(b':')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word-at-a-time prefix and the `memchr2` remainder find the first quote or backslash
    /// at every offset, from every start, past bytes whose values sit one off a target or hold
    /// a high bit.
    #[test]
    fn quote_or_backslash_finds_the_first_match_across_the_word_prefix() {
        for filler in [0x20, 0x21, 0x23, 0x5b, 0x5d, 0x80, 0xa2, 0xdc, 0xff] {
            for len in 0..48 {
                for target in [None, Some(b'"'), Some(b'\\')] {
                    for at in 0..len.max(1) {
                        let mut body = vec![filler; len];
                        if let Some(target) = target.filter(|_| len > 0) {
                            body[at] = target;
                            if at + 1 < len {
                                body[at + 1] = if target == b'"' { b'\\' } else { b'"' };
                            }
                        }
                        for start in 0..=len {
                            let naive = body[start.min(len)..]
                                .iter()
                                .position(|byte| matches!(byte, b'"' | b'\\'))
                                .map(|found| start + found);
                            assert_eq!(
                                quote_or_backslash(&body, start),
                                naive,
                                "filler {filler:#x} len {len} target {target:?} at {at} start {start}"
                            );
                        }
                    }
                }
            }
        }
    }
}
