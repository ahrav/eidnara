//! Included by `#[path]` from `tests/served_json_shell_allocations.rs` and
//! `examples/canonical_output_evidence.rs`.

use memory_store::{BlockKind, HarnessMeta, ProviderExtras, WireBlock, WireMessage};

pub const KEYS_PER_EXTRA_OBJECT: usize = 8;
/// Keys per decoded ASCII block: `kind`, `provider_extras`, `text`, `type`, the `x` namespace, plus the extras.
pub const KEYS_PER_ASCII_BLOCK: usize = KEYS_PER_EXTRA_OBJECT + 5;
pub const BLOCK_COUNTS: [usize; 2] = [1, 65];

/// The fixture orders keys by decoded strings, not JSON escape sequences.
const ESCAPED_KEYS: &[&str] = &[
    r#""\u0001""#,
    r#""\n""#,
    r#"" ""#,
    r#""!""#,
    r#""a""#,
    r#""a b""#,
    r#""a\"""#,
    r#""é""#,
    r#""😀""#,
];

/// Every message is an owned typed value that serializes `role` before `content`
/// and each block's tag before its payload, so the canonicalizer reorders all of
/// them. The decoded populations carry their discriminating keys inside
/// `provider_extras`, the one block-level map the decode retains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Population {
    DecodedAscii { blocks: usize },
    DecodedEscaped { blocks: usize },
    DecodedLargePayload { blocks: usize, payload_bytes: usize },
    TypedShell { blocks: usize },
    OneEditedBlock { blocks: usize },
}

pub fn populations() -> impl Iterator<Item = Population> {
    BLOCK_COUNTS.into_iter().flat_map(|blocks| {
        [
            Population::DecodedAscii { blocks },
            Population::DecodedEscaped { blocks },
            Population::DecodedLargePayload {
                blocks,
                payload_bytes: 64 * 1024,
            },
            Population::TypedShell { blocks },
            Population::OneEditedBlock { blocks },
        ]
    })
}

impl Population {
    pub fn label(&self) -> String {
        match self {
            Self::DecodedAscii { blocks } => format!("decoded_ascii/{blocks}blocks"),
            Self::DecodedEscaped { blocks } => format!("decoded_escaped/{blocks}blocks"),
            Self::DecodedLargePayload {
                blocks,
                payload_bytes,
            } => format!("decoded_large_payload/{blocks}blocks_{payload_bytes}B"),
            Self::TypedShell { blocks } => format!("typed_shell/{blocks}blocks"),
            Self::OneEditedBlock { blocks } => format!("one_edited_block/{blocks}blocks"),
        }
    }

    pub fn build(&self) -> WireMessage {
        match *self {
            Self::DecodedAscii { blocks } => decoded_ascii_message(blocks),
            Self::DecodedEscaped { blocks } => decoded_escaped_message(blocks),
            Self::DecodedLargePayload {
                blocks,
                payload_bytes,
            } => decoded_large_payload_message(blocks, payload_bytes),
            Self::TypedShell { blocks } => typed_shell_message(blocks),
            Self::OneEditedBlock { blocks } => one_edited_block_message(blocks),
        }
    }
}

/// Sorted `Value` maps with unique keys give canonical bytes independently of
/// the span canonicalizer. The oracle holds only while `serde_json` maps sort
/// their keys, so it checks that assumption on every call.
pub fn reference_bytes(message: &WireMessage) -> Vec<u8> {
    assert_eq!(
        serde_json::to_string(&serde_json::json!({"b": 0, "a": 0})).expect("to_string"),
        r#"{"a":0,"b":0}"#,
        "the reference oracle needs sorted serde_json maps"
    );
    serde_json::to_vec(&serde_json::to_value(message).expect("to_value")).expect("to_vec")
}

/// Declaration-order serialization equals the canonical bytes exactly when no
/// object needs reordering.
pub fn declaration_order_equals_canonical(message: &WireMessage, canonical: &[u8]) -> bool {
    serde_json::to_vec(message).expect("to_vec") == canonical
}

fn decoded(body: &str) -> WireMessage {
    serde_json::from_str(body).expect("fixture body parses")
}

fn decoded_body(block_count: usize, mut extra_object: impl FnMut(&mut String)) -> String {
    let mut body = String::from(r#"{"role":"user","content":["#);
    for block in 0..block_count {
        if block != 0 {
            body.push(',');
        }
        body.push_str(r#"{"kind":{"text":"t","type":"text"},"provider_extras":{"x":{"#);
        extra_object(&mut body);
        body.push_str("}}}");
    }
    body.push_str("]}");
    body
}

/// Every key is unescaped ASCII, so no object needs the escape-aware decode path.
pub fn decoded_ascii_message(block_count: usize) -> WireMessage {
    decoded(&decoded_body(block_count, |body| {
        for key in 0..KEYS_PER_EXTRA_OBJECT {
            if key != 0 {
                body.push(',');
            }
            body.push_str(&format!(r#""k{key:02}":{key}"#));
        }
    }))
}

pub fn decoded_escaped_message(block_count: usize) -> WireMessage {
    decoded(&decoded_body(block_count, |body| {
        for (index, key) in ESCAPED_KEYS.iter().enumerate() {
            if index != 0 {
                body.push(',');
            }
            body.push_str(key);
            body.push(':');
            body.push_str(&index.to_string());
        }
    }))
}

pub fn decoded_large_payload_message(block_count: usize, payload_bytes: usize) -> WireMessage {
    let payload = "x".repeat(payload_bytes);
    let mut body = String::from(r#"{"role":"user","content":["#);
    for block in 0..block_count {
        if block != 0 {
            body.push(',');
        }
        body.push_str(r#"{"kind":{"text":""#);
        body.push_str(&payload);
        body.push_str(r#"","type":"text"}}"#);
    }
    body.push_str("]}");
    decoded(&body)
}

pub fn typed_shell_message(block_count: usize) -> WireMessage {
    let blocks = (0..block_count)
        .map(|index| {
            WireBlock::bare(BlockKind::Text {
                text: format!("typed block {index}"),
            })
        })
        .collect();
    WireMessage::from_parts(
        "user",
        blocks,
        None,
        ProviderExtras::new(),
        HarnessMeta::default(),
    )
}

pub fn one_edited_block_message(block_count: usize) -> WireMessage {
    let mut message = decoded_ascii_message(block_count);
    let sibling_bytes = serde_json::to_vec(&message.content()[1..]).expect("to_vec");
    *message.content_mut()[0].kind_mut() = BlockKind::Text {
        text: "edited".to_string(),
    };
    assert_eq!(
        serde_json::to_vec(&message.content()[1..]).expect("to_vec"),
        sibling_bytes,
        "an edit to one block leaves its siblings' bytes unchanged"
    );
    message
}
