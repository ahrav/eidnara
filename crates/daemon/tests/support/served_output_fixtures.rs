//! Included by `#[path]` from `tests/served_json_passthrough_allocations.rs` and
//! `examples/canonical_output_evidence.rs`.

use memory_store::{BlockKind, HarnessMeta, ProviderExtras, WireBlock, WireMessage};

pub const KEYS_PER_EXTRA_OBJECT: usize = 8;
/// Keys per passthrough block: `extra`, `kind`, `text`, `type`, plus the extras.
pub const KEYS_PER_ASCII_BLOCK: usize = KEYS_PER_EXTRA_OBJECT + 4;
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

/// Retained-original shells replay parsed `Value` maps, whose keys are already in
/// decoded order. Typed shells serialize fields in declaration order and need reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Population {
    RetainedAscii { blocks: usize },
    RetainedEscaped { blocks: usize },
    RetainedLargePayload { blocks: usize, payload_bytes: usize },
    TypedShell { blocks: usize },
    OneEditedBlock { blocks: usize },
}

pub fn populations() -> impl Iterator<Item = Population> {
    BLOCK_COUNTS.into_iter().flat_map(|blocks| {
        [
            Population::RetainedAscii { blocks },
            Population::RetainedEscaped { blocks },
            Population::RetainedLargePayload {
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
            Self::RetainedAscii { blocks } => format!("retained_ascii/{blocks}blocks"),
            Self::RetainedEscaped { blocks } => format!("retained_escaped/{blocks}blocks"),
            Self::RetainedLargePayload {
                blocks,
                payload_bytes,
            } => format!("retained_large_payload/{blocks}blocks_{payload_bytes}B"),
            Self::TypedShell { blocks } => format!("typed_shell/{blocks}blocks"),
            Self::OneEditedBlock { blocks } => format!("one_edited_block/{blocks}blocks"),
        }
    }

    pub fn expects_canonical_miss(&self) -> bool {
        matches!(
            self,
            Self::RetainedAscii { .. }
                | Self::RetainedEscaped { .. }
                | Self::RetainedLargePayload { .. }
        )
    }

    /// Whether the canonicalizer returns its serialization buffer for this
    /// population instead of a fresh exact-size reorder buffer. Retained
    /// originals serialize in canonical order and keep the buffer; typed and
    /// edited shells still need the reorder copy.
    pub fn expects_serialization_buffer_return(&self) -> bool {
        self.expects_canonical_miss()
    }

    pub fn build(&self) -> WireMessage {
        match *self {
            Self::RetainedAscii { blocks } => retained_ascii_message(blocks),
            Self::RetainedEscaped { blocks } => retained_escaped_message(blocks),
            Self::RetainedLargePayload {
                blocks,
                payload_bytes,
            } => retained_large_payload_message(blocks, payload_bytes),
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

pub fn serialization_buffer_root_bytes() -> usize {
    let mut probe: Vec<u8> = Vec::new();
    probe.extend_from_slice(b"{");
    probe.capacity()
}

fn retained(body: &str) -> WireMessage {
    let message: WireMessage = serde_json::from_str(body).expect("fixture body parses");
    assert!(
        message.original().is_some(),
        "retained fixtures must carry their original JSON"
    );
    message
}

fn passthrough_body(block_count: usize, mut extra_object: impl FnMut(&mut String)) -> String {
    let mut body = String::from(r#"{"role":"user","content":["#);
    for block in 0..block_count {
        if block != 0 {
            body.push(',');
        }
        body.push_str(r#"{"extra":{"#);
        extra_object(&mut body);
        body.push_str(r#"},"kind":{"text":"t","type":"text"}}"#);
    }
    body.push_str("]}");
    body
}

/// Every key is unescaped ASCII, so no object needs the escape-aware decode path.
pub fn retained_ascii_message(block_count: usize) -> WireMessage {
    retained(&passthrough_body(block_count, |body| {
        for key in 0..KEYS_PER_EXTRA_OBJECT {
            if key != 0 {
                body.push(',');
            }
            body.push_str(&format!(r#""k{key:02}":{key}"#));
        }
    }))
}

pub fn retained_escaped_message(block_count: usize) -> WireMessage {
    retained(&passthrough_body(block_count, |body| {
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

pub fn retained_large_payload_message(block_count: usize, payload_bytes: usize) -> WireMessage {
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
    retained(&body)
}

pub fn typed_shell_message(block_count: usize) -> WireMessage {
    let blocks = (0..block_count)
        .map(|index| {
            WireBlock::bare(BlockKind::Text {
                text: format!("typed block {index}"),
            })
        })
        .collect();
    let message = WireMessage::from_parts(
        "user",
        blocks,
        None,
        ProviderExtras::new(),
        HarnessMeta::default(),
    );
    assert!(message.original().is_none());
    message
}

pub fn one_edited_block_message(block_count: usize) -> WireMessage {
    let mut message = retained_ascii_message(block_count);
    *message.content_mut()[0].kind_mut() = BlockKind::Text {
        text: "edited".to_string(),
    };
    assert!(message.original().is_none());
    assert!(message.content()[0].original().is_none());
    assert!(
        block_count == 1 || message.content()[1].original().is_some(),
        "untouched sibling blocks keep their originals"
    );
    message
}
