//! Bounded decoding of one Anthropic Messages response body into one assistant text.
//!
//! The body is already collected under the raw-byte allowance; this module refuses anything the closed response shape does not admit before it allocates for it. Containers nest at most [`MAX_JSON_DEPTH`] deep and hold at most [`MAX_CONTAINER_ITEMS`] members; a string's decoded length is computed from its escaped bytes before the string is allocated, and every string other than a text block's `text` is bounded by [`MAX_OTHER_STRING_BYTES`]; text blocks number at most [`MAX_TEXT_BLOCKS`] and their decoded bytes together at most [`MAX_ASSISTANT_TEXT_BYTES`]. Tool use and unknown content kinds are refused; the result is the concatenated text, the stop reason, and the bytes the parser allocated for its own scratch.

/// Nesting depth of containers a response may use.
pub const MAX_JSON_DEPTH: usize = 16;
/// Members one object or array may hold.
pub const MAX_CONTAINER_ITEMS: usize = 64;
/// Text blocks one message may carry.
pub const MAX_TEXT_BLOCKS: usize = 16;
/// Decoded bytes of every text block together.
pub const MAX_ASSISTANT_TEXT_BYTES: usize = 64 * 1024;
/// Decoded bytes of any string that is not a text block's text, keys included.
pub const MAX_OTHER_STRING_BYTES: usize = 4 * 1024;

/// Why a body was refused; host-authored, never provider text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("syntax")]
    Syntax,
    #[error("depth")]
    Depth,
    #[error("items")]
    Items,
    #[error("string")]
    String,
    #[error("text")]
    Text,
    #[error("text_blocks")]
    TextBlocks,
    #[error("tool_use")]
    ToolUse,
    #[error("unexpected_content")]
    UnexpectedContent,
    /// The message is not the closed shape: wrong `type` or `role`, a missing or non-array `content`, a repeated key, or an unknown value where a fixed one is required.
    #[error("shape")]
    Shape,
    #[error("no_text")]
    NoText,
}

/// Why the model stopped, as a closed set; anything the provider adds later is `Other`, never its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    Refusal,
    Other,
}

impl StopReason {
    fn from_wire(text: &str) -> Self {
        match text {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "stop_sequence" => Self::StopSequence,
            "refusal" => Self::Refusal,
            _ => Self::Other,
        }
    }
}

/// One fully validated assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMessage {
    pub text: String,
    pub stop_reason: Option<StopReason>,
    /// Bytes the parser allocated for decoded strings while building the tree, apart from the transport buffer.
    pub scratch_bytes: usize,
}

#[derive(Debug)]
enum Value {
    Object(Vec<(String, Value)>),
    Array(Vec<Value>),
    String(String),
    Null,
    /// A boolean or number; the value is not kept.
    Other,
}

impl Value {
    fn field(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }
}

/// Where the parser is inside the response shape, so a string can be classed before it is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    Root,
    Content,
    ContentItem,
    ContentText,
    Other,
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
    text_bytes: usize,
    scratch_bytes: usize,
}

/// Decodes `body` as one Anthropic message and validates its shape: `type` is `message`, `role` is `assistant`, `content` is an array of at most [`MAX_TEXT_BLOCKS`] `text` blocks. A repeated key anywhere is refused, so no block can carry two `type`s. `tool_use` blocks are refused as tool use; any other block kind is unexpected content; a message with no text is refused.
pub fn decode_message(body: &[u8]) -> Result<DecodedMessage, DecodeError> {
    let mut parser = Parser {
        bytes: body,
        at: 0,
        depth: 0,
        text_bytes: 0,
        scratch_bytes: 0,
    };
    parser.skip_whitespace();
    let root = parser.value(Position::Root)?;
    parser.skip_whitespace();
    if parser.at != body.len() {
        return Err(DecodeError::Syntax);
    }
    if root.field("type").and_then(Value::text) != Some("message")
        || root.field("role").and_then(Value::text) != Some("assistant")
    {
        return Err(DecodeError::Shape);
    }
    let Some(Value::Array(blocks)) = root.field("content") else {
        return Err(DecodeError::Shape);
    };
    let mut text = String::with_capacity(parser.text_bytes);
    for block in blocks {
        match block.field("type").and_then(Value::text) {
            Some("text") => {
                let piece = block
                    .field("text")
                    .and_then(Value::text)
                    .ok_or(DecodeError::Shape)?;
                text.push_str(piece);
            }
            Some("tool_use") | Some("server_tool_use") => return Err(DecodeError::ToolUse),
            _ => return Err(DecodeError::UnexpectedContent),
        }
    }
    if text.is_empty() {
        return Err(DecodeError::NoText);
    }
    let stop_reason = match root.field("stop_reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(reason)) => Some(StopReason::from_wire(reason)),
        Some(_) => return Err(DecodeError::Shape),
    };
    Ok(DecodedMessage {
        text,
        stop_reason,
        scratch_bytes: parser.scratch_bytes,
    })
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while self.at < self.bytes.len()
            && matches!(self.bytes[self.at], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, literal: &[u8]) -> Result<(), DecodeError> {
        if self.bytes[self.at..].starts_with(literal) {
            self.at += literal.len();
            Ok(())
        } else {
            Err(DecodeError::Syntax)
        }
    }

    fn value(&mut self, position: Position) -> Result<Value, DecodeError> {
        match self.peek().ok_or(DecodeError::Syntax)? {
            b'{' => self.object(position),
            b'[' => self.array(position),
            b'"' => self
                .string(position == Position::ContentText)
                .map(Value::String),
            b't' => self.expect(b"true").map(|()| Value::Other),
            b'f' => self.expect(b"false").map(|()| Value::Other),
            b'n' => self.expect(b"null").map(|()| Value::Null),
            b'-' | b'0'..=b'9' => self.number().map(|()| Value::Other),
            _ => Err(DecodeError::Syntax),
        }
    }

    fn enter(&mut self) -> Result<(), DecodeError> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(DecodeError::Depth);
        }
        Ok(())
    }

    fn object(&mut self, position: Position) -> Result<Value, DecodeError> {
        self.enter()?;
        self.at += 1;
        let mut fields = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            self.depth -= 1;
            return Ok(Value::Object(fields));
        }
        loop {
            if fields.len() >= MAX_CONTAINER_ITEMS {
                return Err(DecodeError::Items);
            }
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(DecodeError::Syntax);
            }
            let key = self.string(false)?;
            self.skip_whitespace();
            self.expect(b":")?;
            self.skip_whitespace();
            if fields.iter().any(|(name, _)| *name == key) {
                return Err(DecodeError::Shape);
            }
            let child = match (position, key.as_str()) {
                (Position::Root, "content") => Position::Content,
                (Position::ContentItem, "text") => Position::ContentText,
                _ => Position::Other,
            };
            let value = self.value(child)?;
            fields.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(DecodeError::Syntax),
            }
        }
    }

    fn array(&mut self, position: Position) -> Result<Value, DecodeError> {
        self.enter()?;
        self.at += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        let child = if position == Position::Content {
            Position::ContentItem
        } else {
            Position::Other
        };
        loop {
            if items.len() >= MAX_CONTAINER_ITEMS {
                return Err(DecodeError::Items);
            }
            if child == Position::ContentItem && items.len() >= MAX_TEXT_BLOCKS {
                return Err(DecodeError::TextBlocks);
            }
            self.skip_whitespace();
            items.push(self.value(child)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(DecodeError::Syntax),
            }
        }
    }

    /// Accepts exactly the JSON number grammar; the value itself is not kept.
    fn number(&mut self) -> Result<(), DecodeError> {
        let digits = |parser: &mut Self| {
            let start = parser.at;
            while parser.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                parser.at += 1;
            }
            (parser.at > start).then_some(()).ok_or(DecodeError::Syntax)
        };
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        if self.peek() == Some(b'0') {
            self.at += 1;
        } else {
            digits(self)?;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            digits(self)?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            digits(self)?;
        }
        Ok(())
    }

    /// Parses a string whose opening quote is at the cursor. The decoded length is measured first, against the text allowance when `is_text` and against [`MAX_OTHER_STRING_BYTES`] otherwise; only a string that fits is allocated.
    fn string(&mut self, is_text: bool) -> Result<String, DecodeError> {
        let start = self.at + 1;
        let (end, decoded_len) =
            scan_string(&self.bytes[start..], None).ok_or(DecodeError::Syntax)?;
        if is_text {
            if self.text_bytes.saturating_add(decoded_len) > MAX_ASSISTANT_TEXT_BYTES {
                return Err(DecodeError::Text);
            }
            self.text_bytes += decoded_len;
        } else if decoded_len > MAX_OTHER_STRING_BYTES {
            return Err(DecodeError::String);
        }
        self.scratch_bytes = self.scratch_bytes.saturating_add(decoded_len);
        let mut decoded = Vec::with_capacity(decoded_len);
        scan_string(&self.bytes[start..], Some(&mut decoded)).ok_or(DecodeError::Syntax)?;
        debug_assert_eq!(decoded.len(), decoded_len);
        self.at = start + end + 1;
        String::from_utf8(decoded).map_err(|_| DecodeError::Syntax)
    }
}

/// Walks a string body that starts after its opening quote and returns the offset of the closing quote with the decoded UTF-8 byte length. With a `sink`, the same walk also writes the decoded bytes, so measuring and decoding share one grammar. `None` for an unterminated string, a bad escape, a lone or reversed surrogate, or a control character.
fn scan_string(bytes: &[u8], mut sink: Option<&mut Vec<u8>>) -> Option<(usize, usize)> {
    let mut at = 0;
    let mut decoded = 0usize;
    let mut emit = |piece: &[u8], decoded: &mut usize| {
        *decoded += piece.len();
        if let Some(sink) = sink.as_deref_mut() {
            sink.extend_from_slice(piece);
        }
    };
    while at < bytes.len() {
        match bytes[at] {
            b'"' => return Some((at, decoded)),
            b'\\' => {
                let escape = *bytes.get(at + 1)?;
                let simple = match escape {
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'/' => b'/',
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'u' => {
                        let code = hex4(bytes.get(at + 2..at + 6)?)?;
                        let (character, consumed) = if (0xD800..0xDC00).contains(&code) {
                            // A high surrogate must be followed by an escaped low surrogate.
                            if bytes.get(at + 6..at + 8)? != b"\\u" {
                                return None;
                            }
                            let low = hex4(bytes.get(at + 8..at + 12)?)?;
                            if !(0xDC00..0xE000).contains(&low) {
                                return None;
                            }
                            let scalar = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            (char::from_u32(scalar)?, 12)
                        } else if (0xDC00..0xE000).contains(&code) {
                            return None;
                        } else {
                            (char::from_u32(code)?, 6)
                        };
                        let mut buffer = [0u8; 4];
                        emit(character.encode_utf8(&mut buffer).as_bytes(), &mut decoded);
                        at += consumed;
                        continue;
                    }
                    _ => return None,
                };
                emit(&[simple], &mut decoded);
                at += 2;
            }
            byte if byte < 0x20 => return None,
            byte => {
                emit(&[byte], &mut decoded);
                at += 1;
            }
        }
    }
    None
}

/// Exactly four ASCII hex digits.
fn hex4(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 4 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(bytes).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(content: &str) -> String {
        format!(
            r#"{{"id":"msg_1","type":"message","role":"assistant","model":"m","content":{content},"stop_reason":"end_turn","usage":{{"input_tokens":1,"output_tokens":2}}}}"#
        )
    }

    #[test]
    fn a_text_message_decodes_with_concatenated_blocks_and_accounted_scratch() {
        let body = message(
            r#"[{"type":"text","text":"hel\u00e9"},{"type":"text","text":"lo \ud83d\ude00"}]"#,
        );
        let decoded = decode_message(body.as_bytes()).unwrap();
        assert_eq!(decoded.text, "helélo 😀");
        assert_eq!(decoded.stop_reason, Some(StopReason::EndTurn));
        assert!(decoded.scratch_bytes >= decoded.text.len());
    }

    #[test]
    fn bounds_are_checked_before_allocation() {
        let deep = format!(
            "{}1{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        assert_eq!(decode_message(deep.as_bytes()), Err(DecodeError::Depth));
        let wide = format!("[{}]", vec!["1"; MAX_CONTAINER_ITEMS + 1].join(","));
        assert_eq!(decode_message(wide.as_bytes()), Err(DecodeError::Items));
        let long_other = message(&format!(
            r#"[{{"type":"text","text":"a","cache":"{}"}}]"#,
            "x".repeat(MAX_OTHER_STRING_BYTES + 1)
        ));
        assert_eq!(
            decode_message(long_other.as_bytes()),
            Err(DecodeError::String)
        );
        let long_text = message(&format!(
            r#"[{{"type":"text","text":"{}"}}]"#,
            "y".repeat(MAX_ASSISTANT_TEXT_BYTES + 1)
        ));
        assert_eq!(decode_message(long_text.as_bytes()), Err(DecodeError::Text));
        let split = message(&format!(
            r#"[{{"type":"text","text":"{}"}},{{"type":"text","text":"{}"}}]"#,
            "y".repeat(MAX_ASSISTANT_TEXT_BYTES / 2 + 1),
            "z".repeat(MAX_ASSISTANT_TEXT_BYTES / 2)
        ));
        assert_eq!(decode_message(split.as_bytes()), Err(DecodeError::Text));
        let escaped_long = message(&format!(
            r#"[{{"type":"text","text":"a","id":"{}"}}]"#,
            "\\u00e9".repeat(MAX_OTHER_STRING_BYTES / 2 + 1)
        ));
        assert_eq!(
            decode_message(escaped_long.as_bytes()),
            Err(DecodeError::String),
            "escaped length is measured decoded, not raw"
        );
        let blocks = message(&format!(
            "[{}]",
            vec![r#"{"type":"text","text":"t"}"#; MAX_TEXT_BLOCKS + 1].join(",")
        ));
        assert_eq!(
            decode_message(blocks.as_bytes()),
            Err(DecodeError::TextBlocks)
        );
    }

    #[test]
    fn scratch_accounting_counts_decoded_strings_only() {
        // Numbers and whitespace cost transport bytes but no parser scratch; every decoded string, keys included, is counted exactly once.
        let body = r#"{"type":"message","role":"assistant","content":[{"type":"text","text":"ab"}],"usage":{"input_tokens":1000000,"output_tokens":2000000},"padding":[1,2,3,4,5,6,7,8,9,10]}"#;
        let decoded = decode_message(body.as_bytes()).unwrap();
        let strings = [
            "type",
            "message",
            "role",
            "assistant",
            "content",
            "type",
            "text",
            "text",
            "ab",
            "usage",
            "input_tokens",
            "output_tokens",
            "padding",
        ];
        assert_eq!(
            decoded.scratch_bytes,
            strings.iter().map(|s| s.len()).sum::<usize>()
        );
        assert!(decoded.scratch_bytes < body.len());
    }

    proptest::proptest! {
        #[test]
        fn the_string_scanner_agrees_with_serde_json(text in "\\PC*") {
            let escaped = serde_json::to_string(&text).unwrap();
            let body = &escaped.as_bytes()[1..];
            let (end, decoded_len) = scan_string(body, None).unwrap();
            proptest::prop_assert_eq!(end, body.len() - 1);
            proptest::prop_assert_eq!(decoded_len, text.len());
            let mut decoded = Vec::new();
            scan_string(body, Some(&mut decoded)).unwrap();
            proptest::prop_assert_eq!(String::from_utf8(decoded).unwrap(), text);
        }

        #[test]
        fn an_accepted_escaped_string_is_valid_json(raw in "[ -~]{0,64}") {
            // Whatever the scanner accepts as a string body, serde_json accepts and decodes to the same bytes.
            let body = format!("{raw}\"");
            if let Some((end, _)) = scan_string(body.as_bytes(), None) {
                let quoted = format!("\"{}\"", &body[..end]);
                let reference: String = serde_json::from_str(&quoted).unwrap();
                let mut decoded = Vec::new();
                scan_string(body.as_bytes(), Some(&mut decoded)).unwrap();
                proptest::prop_assert_eq!(String::from_utf8(decoded).unwrap(), reference);
            }
        }
    }

    #[test]
    fn unexpected_shapes_are_refused_with_host_codes() {
        assert_eq!(
            decode_message(
                message(r#"[{"type":"tool_use","id":"t","name":"x","input":{}}]"#).as_bytes()
            ),
            Err(DecodeError::ToolUse)
        );
        assert_eq!(
            decode_message(message(r#"[{"type":"image","source":{}}]"#).as_bytes()),
            Err(DecodeError::UnexpectedContent)
        );
        assert_eq!(
            decode_message(message("[]").as_bytes()),
            Err(DecodeError::NoText)
        );
        assert_eq!(
            decode_message(
                br#"{"type":"error","error":{"type":"overloaded_error","message":"x"}}"#
            ),
            Err(DecodeError::Shape)
        );
        assert_eq!(
            decode_message(message(r#"[{"type":"text","text":"a"}] trailing"#).as_bytes()),
            Err(DecodeError::Syntax)
        );
        assert_eq!(decode_message(br#"{"type":"message","role":"assistant","content":[{"type":"text","text":"bad \x"}]}"#), Err(DecodeError::Syntax));
        assert_eq!(decode_message(br#"{"type":"message","role":"assistant","content":[{"type":"text","text":"\ud83d"}]}"#), Err(DecodeError::Syntax));
        assert_eq!(decode_message(b"not json"), Err(DecodeError::Syntax));
        assert_eq!(decode_message(b""), Err(DecodeError::Syntax));
        // A repeated key cannot launder a tool-use block into text.
        assert_eq!(
            decode_message(message(r#"[{"type":"text","type":"tool_use","text":"t"}]"#).as_bytes()),
            Err(DecodeError::Shape)
        );
        // Only the JSON number grammar and four-hex-digit escapes are accepted.
        for bad in [
            r#"{"type":"message","role":"assistant","content":[],"n":1.}"#,
            r#"{"type":"message","role":"assistant","content":[],"n":-}"#,
            r#"{"type":"message","role":"assistant","content":[],"n":1e}"#,
            r#"{"type":"message","role":"assistant","content":[],"n":01}"#,
            r#"{"type":"message","role":"assistant","content":[{"type":"text","text":"\u+041"}]}"#,
        ] {
            assert_eq!(
                decode_message(bad.as_bytes()),
                Err(DecodeError::Syntax),
                "{bad}"
            );
        }
        let numbers = r#"{"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"n":[0,-1,1.5,2e10,3E-2,0.0]}"#;
        assert_eq!(decode_message(numbers.as_bytes()).unwrap().text, "ok");
        assert_eq!(
            decode_message(
                message(r#"[{"type":"text","text":"t"}]"#)
                    .replace("end_turn", "something_new")
                    .as_bytes()
            )
            .unwrap()
            .stop_reason,
            Some(StopReason::Other)
        );
        // A null stop reason is an absent one; a boolean, number, array, or object is outside the closed shape.
        let with_stop_reason = |reason: &str| {
            message(r#"[{"type":"text","text":"t"}]"#).replace(r#""end_turn""#, reason)
        };
        assert_eq!(
            decode_message(with_stop_reason("null").as_bytes())
                .unwrap()
                .stop_reason,
            None
        );
        for reason in ["true", "false", "123", "[]", "{}"] {
            assert_eq!(
                decode_message(with_stop_reason(reason).as_bytes()),
                Err(DecodeError::Shape),
                "{reason}"
            );
        }
    }
}
