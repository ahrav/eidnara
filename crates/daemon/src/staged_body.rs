//! `StagedBody` writes a response body through one fixed staging buffer. JSON values are encoded
//! straight into the buffer, byte-identical to `serde_json::to_writer`, and the sink receives the
//! body in buffer-sized writes.

use std::io::{self, Write};

use serde_json::Value;

/// Buffer bytes collected before the sink receives them.
pub(crate) const STAGE_BYTES: usize = 8 * 1024;

/// Bytes one encoder step may write past the fill limit: an escaped block of 16 bytes stores
/// eight-byte table words at `u8` offsets.
const STEP_BYTES: usize = 256 + 8;

/// Bytes a number's text may take: twenty digits and a sign for an integer, and at most
/// twenty-four characters for a float.
const NUMBER_BYTES: usize = 32;

/// `ESCAPED` packs the bytes `serde_json` writes for each input byte into the low bytes of one
/// word and the output length into its top byte.
static ESCAPED: [u64; 256] = escape_table();

const fn escape_table() -> [u64; 256] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut table = [0u64; 256];
    let mut byte = 0usize;
    while byte < 256 {
        let short = match byte as u8 {
            b'"' => b'"',
            b'\\' => b'\\',
            0x08 => b'b',
            b'\t' => b't',
            b'\n' => b'n',
            0x0c => b'f',
            b'\r' => b'r',
            _ => 0,
        };
        table[byte] = if short != 0 {
            (2 << 56) | ((short as u64) << 8) | b'\\' as u64
        } else if byte < 0x20 {
            (6 << 56)
                | ((HEX[byte & 0x0f] as u64) << 40)
                | ((HEX[byte >> 4] as u64) << 32)
                | ((b'0' as u64) << 24)
                | ((b'0' as u64) << 16)
                | ((b'u' as u64) << 8)
                | b'\\' as u64
        } else {
            (1 << 56) | byte as u64
        };
        byte += 1;
    }
    table
}

/// `Sink` receives the staged body in order.
pub(crate) trait Sink {
    fn drain(&mut self, bytes: &[u8]) -> io::Result<()>;
}

impl Sink for Vec<u8> {
    fn drain(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.extend_from_slice(bytes);
        Ok(())
    }
}

/// `BoundedSink` passes at most `max_len` bytes to `destination` and refuses the write that would
/// pass it, so a body longer than its measured length never reaches the destination whole.
pub(crate) struct BoundedSink<'a> {
    destination: &'a mut dyn Write,
    max_len: usize,
    written: usize,
}

impl<'a> BoundedSink<'a> {
    pub(crate) fn new(destination: &'a mut dyn Write, max_len: usize) -> Self {
        Self {
            destination,
            max_len,
            written: 0,
        }
    }

    pub(crate) fn written(&self) -> usize {
        self.written
    }
}

impl Sink for BoundedSink<'_> {
    fn drain(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > self.max_len - self.written {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "prepared body exceeded measured length",
            ));
        }
        self.destination.write_all(bytes)?;
        self.written += bytes.len();
        Ok(())
    }
}

/// `StagedBody` encodes into `buffer` and passes the staged bytes to `sink` whenever an encoder
/// step starts past `limit`; every step fits in the `STEP_BYTES` the buffer holds past `limit`.
pub(crate) struct StagedBody<S> {
    buffer: Box<[u8]>,
    len: usize,
    /// `len` past `limit` sends the buffer to the sink before the next encoder step.
    limit: usize,
    sink: S,
}

impl<S: Sink> StagedBody<S> {
    /// `expected` sizes the buffer for a body known to be short.
    pub(crate) fn new(sink: S, expected: usize) -> Self {
        let limit = expected.clamp(64, STAGE_BYTES);
        Self {
            buffer: vec![0; limit + STEP_BYTES].into_boxed_slice(),
            len: 0,
            limit,
            sink,
        }
    }

    /// Sends every staged byte to the sink.
    pub(crate) fn drain(&mut self) -> io::Result<()> {
        if self.len > 0 {
            self.sink.drain(&self.buffer[..self.len])?;
            self.len = 0;
        }
        Ok(())
    }

    /// Drains the buffer and returns the sink.
    pub(crate) fn finish(mut self) -> io::Result<S> {
        self.drain()?;
        Ok(self.sink)
    }

    #[inline]
    fn make_room(&mut self) -> io::Result<()> {
        if self.len > self.limit {
            self.drain()?;
        }
        Ok(())
    }

    /// Stages `bytes`; a run as long as the buffer goes to the sink unstaged.
    #[inline]
    pub(crate) fn bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() <= STEP_BYTES {
            self.make_room()?;
        } else if self.len + bytes.len() > self.buffer.len() {
            self.drain()?;
            if bytes.len() > self.limit {
                return self.sink.drain(bytes);
            }
        }
        self.buffer[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }

    #[inline]
    pub(crate) fn byte(&mut self, byte: u8) -> io::Result<()> {
        self.make_room()?;
        self.buffer[self.len] = byte;
        self.len += 1;
        Ok(())
    }

    /// Writes `value` as compact JSON, byte-identical to `serde_json::to_writer`.
    pub(crate) fn value(&mut self, value: &Value) -> io::Result<()> {
        match value {
            Value::Null => self.bytes(b"null"),
            Value::Bool(true) => self.bytes(b"true"),
            Value::Bool(false) => self.bytes(b"false"),
            Value::Number(number) => self.number(number),
            Value::String(text) => self.string(text),
            Value::Array(items) => {
                self.byte(b'[')?;
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.byte(b',')?;
                    }
                    self.value(item)?;
                }
                self.byte(b']')
            }
            Value::Object(fields) => {
                self.byte(b'{')?;
                for (index, (key, item)) in fields.iter().enumerate() {
                    if index > 0 {
                        self.byte(b',')?;
                    }
                    self.string(key)?;
                    self.byte(b':')?;
                    self.value(item)?;
                }
                self.byte(b'}')
            }
        }
    }

    /// Writes `value` as [`Self::value`] does while it nests at most `levels` container levels.
    /// `Ok(false)` reports a deeper container; the bytes written before it stay staged.
    pub(crate) fn value_within(&mut self, value: &Value, levels: usize) -> io::Result<bool> {
        let (opening, closing) = match value {
            Value::Array(_) => (b'[', b']'),
            Value::Object(_) => (b'{', b'}'),
            scalar => return self.value(scalar).map(|()| true),
        };
        let Some(inner) = levels.checked_sub(1) else {
            return Ok(false);
        };
        self.byte(opening)?;
        match value {
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.byte(b',')?;
                    }
                    if !self.value_within(item, inner)? {
                        return Ok(false);
                    }
                }
            }
            Value::Object(fields) => {
                for (index, (key, item)) in fields.iter().enumerate() {
                    if index > 0 {
                        self.byte(b',')?;
                    }
                    self.string(key)?;
                    self.byte(b':')?;
                    if !self.value_within(item, inner)? {
                        return Ok(false);
                    }
                }
            }
            _ => {}
        }
        self.byte(closing)?;
        Ok(true)
    }

    fn number(&mut self, number: &serde_json::Number) -> io::Result<()> {
        self.make_room()?;
        let mut window = &mut self.buffer[self.len..self.len + NUMBER_BYTES];
        serde_json::to_writer(&mut window, number).map_err(io::Error::other)?;
        let unused = window.len();
        self.len += NUMBER_BYTES - unused;
        Ok(())
    }

    /// Writes `text` as a quoted JSON string with `serde_json`'s escapes. A 16-byte block with no
    /// byte to escape is copied whole, and one with a single escape is copied around it; any
    /// other block, and a final partial block that needs an escape, is written byte by byte from
    /// `ESCAPED`.
    pub(crate) fn string(&mut self, text: &str) -> io::Result<()> {
        self.byte(b'"')?;
        let bytes = text.as_bytes();
        let (blocks, rest) = bytes.as_chunks::<16>();
        for (index, block) in blocks.iter().enumerate() {
            self.make_room()?;
            let (low, high) = block.split_at(8);
            let low = crate::json_string::escape_marks(u64::from_le_bytes(
                low.try_into().unwrap_or_default(),
            ));
            let high = crate::json_string::escape_marks(u64::from_le_bytes(
                high.try_into().unwrap_or_default(),
            ));
            self.buffer[self.len..self.len + 16].copy_from_slice(block);
            if low | high == 0 {
                self.len += 16;
            } else if (low & low.wrapping_sub(1)) | (high & high.wrapping_sub(1)) == 0
                && (low == 0 || high == 0)
            {
                // Every escaped byte is marked and a false mark only follows a true one, so a
                // lone mark is the block's only escape.
                let lane = if low != 0 {
                    low.trailing_zeros() / 8
                } else {
                    8 + high.trailing_zeros() / 8
                } as usize;
                self.lone_escape(block, bytes.get(index * 16 + lane + 1..), lane);
            } else {
                self.escaped(block);
            }
        }
        self.make_room()?;
        if !self.clean_short(rest) {
            self.escaped(rest);
        }
        self.byte(b'"')
    }

    /// Copies `bytes`, shorter than a block, when none of them takes an escape. Two overlapping
    /// reads of eight, four, or two bytes cover every length from 2 to 15; a single byte and an
    /// empty run take the table.
    #[inline]
    fn clean_short(&mut self, bytes: &[u8]) -> bool {
        // Bytes that never take an escape fill the unread high bytes of a short read.
        const FILL4: u64 = 0x6161_6161 << 32;
        const FILL2: u64 = 0x6161_6161_6161 << 16;
        let len = bytes.len();
        let (first, last, width) = if let (Some(first), Some(last)) =
            (bytes.first_chunk::<8>(), bytes.last_chunk::<8>())
        {
            (u64::from_le_bytes(*first), u64::from_le_bytes(*last), 8)
        } else if let (Some(first), Some(last)) =
            (bytes.first_chunk::<4>(), bytes.last_chunk::<4>())
        {
            (
                u64::from(u32::from_le_bytes(*first)) | FILL4,
                u64::from(u32::from_le_bytes(*last)) | FILL4,
                4,
            )
        } else if let (Some(first), Some(last)) =
            (bytes.first_chunk::<2>(), bytes.last_chunk::<2>())
        {
            (
                u64::from(u16::from_le_bytes(*first)) | FILL2,
                u64::from(u16::from_le_bytes(*last)) | FILL2,
                2,
            )
        } else {
            return false;
        };
        if crate::json_string::escape_marks(first) | crate::json_string::escape_marks(last) != 0 {
            return false;
        }
        let at = self.len;
        self.buffer[at..at + 8].copy_from_slice(&first.to_le_bytes());
        self.buffer[at + len - width..at + len - width + 8].copy_from_slice(&last.to_le_bytes());
        self.len = at + len;
        true
    }

    /// Finishes a block already copied at `len` whose only escape is at `lane`: the escape
    /// replaces that byte and the block's remaining bytes follow it, copied from `after`, the
    /// text that follows the escaped byte.
    #[inline]
    fn lone_escape(&mut self, block: &[u8; 16], after: Option<&[u8]>, lane: usize) {
        let entry = ESCAPED[usize::from(block[lane])];
        let at = self.len + lane;
        self.buffer[at..at + 8].copy_from_slice(&entry.to_le_bytes());
        let at = at + (entry >> 56) as usize;
        match after.and_then(|after| after.first_chunk::<16>()) {
            Some(after) => self.buffer[at..at + 16].copy_from_slice(after),
            None => {
                let tail = &block[lane + 1..];
                self.buffer[at..at + tail.len()].copy_from_slice(tail);
            }
        }
        self.len = at + 15 - lane;
    }

    /// Writes up to 16 bytes through `ESCAPED`: each step stores one table word and advances by
    /// its length, so the next step overwrites the word's unused bytes.
    #[inline]
    fn escaped(&mut self, bytes: &[u8]) {
        let window: &mut [u8; STEP_BYTES] = (&mut self.buffer[self.len..self.len + STEP_BYTES])
            .try_into()
            .expect("the window spans STEP_BYTES bytes");
        // At most 16 steps of six bytes each advance `at`, so it stays below 96 and a `u8` holds
        // it; a `u8` offset plus eight also bounds every store inside the window.
        let mut at = 0u8;
        for &byte in bytes.iter().take(16) {
            let entry = ESCAPED[usize::from(byte)];
            let start = usize::from(at);
            window[start..start + 8].copy_from_slice(&entry.to_le_bytes());
            at = at.wrapping_add((entry >> 56) as u8);
        }
        self.len += usize::from(at);
    }
}

impl StagedBody<Vec<u8>> {
    /// Bytes written so far, drained or staged.
    pub(crate) fn written(&self) -> usize {
        self.sink.len() + self.len
    }

    /// Every byte written. A body that never drained returns its own buffer, so it is not copied.
    pub(crate) fn into_bytes(mut self) -> io::Result<Vec<u8>> {
        if self.sink.is_empty() {
            let mut bytes = self.buffer.into_vec();
            bytes.truncate(self.len);
            return Ok(bytes);
        }
        self.drain()?;
        Ok(self.sink)
    }
}

impl<S: Sink> Write for StagedBody<S> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes(bytes)?;
        Ok(bytes.len())
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.bytes(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn staged(value: &Value, expected: usize) -> Vec<u8> {
        let mut body = StagedBody::new(Vec::new(), expected);
        body.value(value).unwrap();
        body.finish().unwrap()
    }

    fn assert_serde_bytes(value: &Value) {
        let expected = serde_json::to_vec(value).unwrap();
        for buffer in [0, 100, expected.len(), STAGE_BYTES] {
            assert_eq!(
                staged(value, buffer),
                expected,
                "{value} staged in a {buffer}-byte buffer"
            );
        }
    }

    #[test]
    fn every_byte_escapes_as_serde_json_does_at_every_offset_of_a_block() {
        let all: String = (0u32..0x800).filter_map(char::from_u32).collect();
        for skip in 0..64 {
            for take in 0..40 {
                let text: String = all.chars().skip(skip).take(take).collect();
                assert_serde_bytes(&json!(text));
            }
        }
        let escaped = [
            '\0', '\u{1}', '\u{1f}', '\u{8}', '\t', '\n', '\u{b}', '\u{c}', '\r', '"', '\\',
            '\u{7f}',
        ];
        for len in 0..48 {
            let clean = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOP"[..len].to_owned();
            assert_serde_bytes(&json!(clean));
            for first in 0..len {
                for &byte in &escaped {
                    let mut text = clean.clone();
                    text.replace_range(first..=first, &byte.to_string());
                    assert_serde_bytes(&json!(text));
                    for second in first + 1..len {
                        let mut pair = text.clone();
                        pair.replace_range(second..=second, "\"");
                        assert_serde_bytes(&json!(pair));
                    }
                }
            }
        }
    }

    #[test]
    fn a_block_of_six_byte_escapes_and_a_long_string_cross_buffer_drains_intact() {
        let controls: String = (0u8..0x20).map(char::from).collect::<String>().repeat(700);
        let quoted = "a\"b\\c\nd".repeat(5_000);
        let multibyte = "\u{e9}\u{65e5}\u{1f642}\u{2028}\"".repeat(3_000);
        for text in [controls, quoted, multibyte, "x".repeat(3 * STAGE_BYTES + 7)] {
            assert_serde_bytes(&json!(text));
            assert_serde_bytes(&json!({ text.clone(): [text, 1, null] }));
        }
    }

    #[test]
    fn scalars_and_containers_encode_as_serde_json_does() {
        for value in [
            json!(null),
            json!(true),
            json!(false),
            json!(0),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(-0.0),
            json!(1e300),
            json!(5e-324),
            json!(-1.234_567_890_123_456_7e-308),
            json!(0.1),
            json!(f64::MAX),
            json!([]),
            json!({}),
            json!([[], {}, [[[]]], {"": {}}]),
            json!({"k\n": "v\u{1}", "a": [1, 2.5, "x\ny", {"b": false}]}),
        ] {
            assert_serde_bytes(&value);
        }
    }

    fn json_value() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<u64>().prop_map(Value::from),
            any::<i64>().prop_map(Value::from),
            any::<f64>().prop_map(|number| json!(number)),
            any::<String>().prop_map(Value::String),
            "(\\PC|[\\x00-\\x1f\"\\\\\u{7f}\u{2028}]){0,80}".prop_map(Value::String),
        ];
        leaf.prop_recursive(5, 96, 8, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
                proptest::collection::btree_map("(\\PC|[\\x00-\\x1f\"\\\\]){0,24}", inner, 0..8)
                    .prop_map(|fields| Value::Object(fields.into_iter().collect())),
            ]
        })
    }

    proptest::proptest! {
        #[test]
        fn every_value_encodes_as_serde_json_does(value in json_value(), buffer in 0usize..300) {
            proptest::prop_assert_eq!(staged(&value, buffer), serde_json::to_vec(&value).unwrap());
        }
    }

    /// Records every write the sink passes on.
    #[derive(Default)]
    struct Writes(Vec<Vec<u8>>);

    impl Write for Writes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_bounded_sink_passes_whole_drains_and_refuses_the_one_past_its_length() {
        let text = "line \"quoted\"\n".repeat(2_000);
        let value = json!([text, text]);
        let expected = serde_json::to_vec(&value).unwrap();

        let mut writes = Writes::default();
        let mut body = StagedBody::new(
            BoundedSink::new(&mut writes, expected.len()),
            expected.len(),
        );
        body.value(&value).unwrap();
        assert_eq!(body.finish().unwrap().written(), expected.len());
        assert_eq!(writes.0.concat(), expected);
        assert!(
            writes.0.len() > 2
                && writes
                    .0
                    .iter()
                    .all(|write| write.len() <= STAGE_BYTES + STEP_BYTES)
        );

        let mut short = Writes::default();
        let mut body = StagedBody::new(
            BoundedSink::new(&mut short, expected.len() - 1),
            expected.len(),
        );
        let refused = body.value(&value).and_then(|()| body.drain());
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::WriteZero);
        let passed = short.0.concat();
        assert!(passed.len() < expected.len() && expected.starts_with(&passed));
    }

    #[test]
    fn a_run_that_overflows_the_buffer_reaches_the_sink_unstaged_after_the_staged_bytes() {
        let run = vec![b'x'; 2 * STAGE_BYTES];
        let mut writes = Writes::default();
        let mut body = StagedBody::new(BoundedSink::new(&mut writes, run.len() + 2), run.len());
        body.bytes(b"[").unwrap();
        body.bytes(&run).unwrap();
        body.bytes(b"]").unwrap();
        body.finish().unwrap();
        assert_eq!(writes.0, vec![b"[".to_vec(), run, b"]".to_vec()]);
    }
}
