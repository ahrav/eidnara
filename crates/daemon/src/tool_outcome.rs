//! The execution-outcome extractor: one reading of what a harness reported about a settled tool call. [`opencode`] and [`pi`] read each record field from a named native field, and an absent or malformed optional field is `None`, meaning unknown. `from_output` reads a result's status and text from its wire block, and `digest` renders the summarizer's view of it.
//!
//! OpenCode (`MessageV2` tool part, verified against the OpenCode 1.19.0 session store in `testdata/opencode-tool-outcome-trace.json`):
//! - `status` is `state.status`, `completed` or `error`.
//! - `truncated` is the boolean `state.metadata.truncated`, which completed tools report.
//! - `exit_code` is the integer `state.metadata.exit` of a `bash` call; a null `exit` is unknown.
//! - `exception_ref` is the `name` of the enclosing assistant message's `info.error` for a call whose `state.metadata.interrupted` is `true`: the abort that interrupted the call is the exception the harness named. A call that settled on its own keeps the record its first observation saw, so a later abort of the turn leaves its record, and its request digest, unchanged. The adapter reads an interrupted call only once its turn has `time.completed` ([`opencode_interrupted`]); the record's stability rests on OpenCode writing the abort's `info.error` no later than that completion timestamp, which the 1.19.0 store shows on every aborted turn it holds, and a turn that never completes leaves its interrupted call unpublished.
//!
//! Pi (`toolResult` message): `status` follows `isError`, and `truncated` is the boolean `details.truncation.truncated` the bash tool reports. A Pi session entry carries status and truncation only, so a Pi record leaves `exit_code` and `exception_ref` unset.

use kernel::{TOOL_OUTCOME_VERSION, ToolOutcome, ToolStatus};
use serde_json::Value;

/// The longest exception name the record keeps; a longer one is unknown.
const MAX_EXCEPTION_NAME_BYTES: usize = 64;

/// The outcome of a settled OpenCode tool `part` whose returned string is `output`, inside an assistant message whose `info.error` is `message_error`. `None` when the part names no tool or has not settled.
pub fn opencode(part: &Value, message_error: Option<&Value>, output: &str) -> Option<ToolOutcome> {
    let tool_name = part.get("tool").and_then(Value::as_str)?;
    let state = part.get("state")?;
    let status = match state.get("status").and_then(Value::as_str)? {
        "completed" => ToolStatus::Completed,
        "error" => ToolStatus::Error,
        _ => return None,
    };
    let metadata = state.get("metadata");
    let interrupted = opencode_interrupted(state);
    let exit_code = (tool_name == "bash")
        .then(|| metadata.and_then(|metadata| metadata.get("exit")))
        .flatten()
        .and_then(Value::as_i64);
    Some(ToolOutcome {
        version: TOOL_OUTCOME_VERSION,
        status,
        tool_name: tool_name.to_owned(),
        byte_length: output.len() as u64,
        truncated: metadata
            .and_then(|metadata| metadata.get("truncated"))
            .and_then(Value::as_bool),
        exit_code,
        exception_ref: interrupted
            .then_some(message_error)
            .flatten()
            .and_then(|error| error.get("name"))
            .and_then(Value::as_str)
            .filter(|name| is_exception_name(name))
            .map(str::to_owned),
    })
}

/// Whether an OpenCode tool `state` records that the turn's abort interrupted the call (`metadata.interrupted` is `true`).
pub fn opencode_interrupted(state: &Value) -> bool {
    state.pointer("/metadata/interrupted") == Some(&Value::Bool(true))
}

/// The outcome of one text block, `output`, of a Pi `toolResult` message. `None` when the message names no tool or carries no boolean `isError`.
pub fn pi(message: &Value, output: &str) -> Option<ToolOutcome> {
    let tool_name = message.get("toolName").and_then(Value::as_str)?;
    let status = if message.get("isError").and_then(Value::as_bool)? {
        ToolStatus::Error
    } else {
        ToolStatus::Completed
    };
    Some(ToolOutcome {
        version: TOOL_OUTCOME_VERSION,
        status,
        tool_name: tool_name.to_owned(),
        byte_length: output.len() as u64,
        truncated: message
            .pointer("/details/truncation/truncated")
            .and_then(Value::as_bool),
        exit_code: None,
        exception_ref: None,
    })
}

/// An exception name is a short ASCII identifier, such as `MessageAbortedError`.
fn is_exception_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_EXCEPTION_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The status and output text of a tool result as its wire block holds it, the shape the summarizer reads. An error output variant or a denied execution is `error`. The text is the output variant's text as is, its JSON serialized, the text of its content blocks joined by newlines, or a denial's reason.
pub(crate) fn from_output(output: &memory_store::ToolOutput) -> (ToolStatus, String) {
    use memory_store::{OutputKind, ResultBlockKind};
    let text_of = |blocks: &[memory_store::ResultBlock]| {
        blocks
            .iter()
            .filter_map(|block| match &block.kind {
                ResultBlockKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    match &output.kind {
        OutputKind::Text { text } => (ToolStatus::Completed, text.clone()),
        OutputKind::Json { value } => (ToolStatus::Completed, value.to_string()),
        OutputKind::Content { blocks } => (ToolStatus::Completed, text_of(blocks)),
        OutputKind::ErrorText { text } => (ToolStatus::Error, text.clone()),
        OutputKind::ErrorJson { value } => (ToolStatus::Error, value.to_string()),
        OutputKind::ErrorContent { blocks } => (ToolStatus::Error, text_of(blocks)),
        OutputKind::ExecutionDenied { reason } => {
            (ToolStatus::Error, reason.clone().unwrap_or_default())
        }
    }
}

/// The largest digest the summarizer sees for one tool result.
pub(crate) const MAX_DIGEST_BYTES: usize = 1024;
/// Marks an excerpt or a call summary clipped to its bound.
const CLIP_MARKER: &str = "…";

/// The outcome half of a tool result's digest: `; <status>[; exit=N]; bytes=N[; head="…"][; tail="…"]`. `bytes` is the byte length of `output`. `head` and `tail` are its first and last nonblank lines with trailing whitespace trimmed; a single nonblank line is `head` alone. A double quote or a citation-marker bracket inside an excerpt becomes a single quote, so each excerpt is one quoted field, and each excerpt is then clipped to the claim-value bound on a character boundary with a clip marker. Output bytes reach the digest only through the two excerpts, and equal inputs give equal bytes.
pub(crate) fn outcome_digest(status: ToolStatus, exit_code: Option<i64>, output: &str) -> String {
    let mut digest = format!("; {}", status.as_str());
    if let Some(exit_code) = exit_code {
        digest.push_str(&format!("; exit={exit_code}"));
    }
    digest.push_str(&format!("; bytes={}", output.len()));
    let mut lines = output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty());
    let head = lines.next();
    let tail = lines.next_back();
    for (label, line) in [("head", head), ("tail", tail)] {
        if let Some(line) = line {
            let excerpt = clip(
                &line.replace(['"', '\u{ab}', '\u{bb}'], "'"),
                crate::history_summarizer_citations::CLAIM_VALUE_MAX_BYTES,
            );
            digest.push_str(&format!("; {label}=\"{excerpt}\""));
        }
    }
    digest
}

/// The summarizer's view of one tool result: `call`, the call's `TC:` summary, followed by its [`outcome_digest`], at most [`MAX_DIGEST_BYTES`]. A call summary that would pass the bound is clipped on a character boundary with a clip marker.
pub(crate) fn digest(call: &str, outcome: &str) -> String {
    let mut digest = clip(call, MAX_DIGEST_BYTES - outcome.len());
    digest.push_str(outcome);
    digest
}

/// `text` followed by each of `digests`, joined by the chunk's part separator. A digest joins after text compaction, so its bytes are the digest's.
pub(crate) fn append_digests(text: &mut String, digests: impl IntoIterator<Item = String>) {
    for digest in digests {
        if !text.is_empty() {
            text.push_str(" / ");
        }
        text.push_str(&digest);
    }
}

/// `text` within `max` bytes: whole when it fits, otherwise its longest prefix that ends on a character boundary and leaves room for the clip marker, followed by the marker.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max.saturating_sub(CLIP_MARKER.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CLIP_MARKER}", &text[..end])
}

#[cfg(test)]
mod digest_tests {
    use super::*;
    use crate::history_summarizer_citations::CLAIM_VALUE_MAX_BYTES;
    use proptest::prelude::*;

    fn completed(call: &str, output: &str) -> String {
        digest(call, &outcome_digest(ToolStatus::Completed, None, output))
    }

    /// `sc-digest-bounded-deterministic-char-boundary`: CRLF lines lose their carriage returns and trailing whitespace; a single nonblank line is `head` alone; quotes and marker brackets become single quotes; only the first and last nonblank lines appear.
    #[test]
    fn a_digest_shows_status_bytes_and_the_outer_nonblank_lines() {
        let output =
            "\r\n  first line \r\nSENTINEL middle\r\n\r\nlast «s9» \"quoted\" line\r\n\r\n";
        assert_eq!(
            completed("TC: bash(run)", output),
            format!(
                "TC: bash(run); completed; bytes={}; head=\"  first line\"; tail=\"last 's9' 'quoted' line\"",
                output.len()
            )
        );
        assert_eq!(
            completed("TC: read(a.rs)", "only line\n"),
            "TC: read(a.rs); completed; bytes=10; head=\"only line\""
        );
        assert_eq!(
            completed("TC: read(a.rs)", ""),
            "TC: read(a.rs); completed; bytes=0"
        );
        assert_eq!(
            digest(
                "TC: bash",
                &outcome_digest(ToolStatus::Error, Some(2), "boom\n")
            ),
            "TC: bash; error; exit=2; bytes=5; head=\"boom\""
        );
    }

    /// An excerpt is the longest prefix that fits the claim-value bound with its clip marker, cut on a character boundary for two- and three-byte characters; a call summary that would push the digest past 1 KiB is clipped the same way.
    #[test]
    fn a_digest_clips_on_character_boundaries_within_its_bounds() {
        let output = format!("{}\n{}", "é".repeat(100), "日本".repeat(60));
        assert_eq!(
            outcome_digest(ToolStatus::Completed, None, &output),
            format!(
                "; completed; bytes={}; head=\"{}…\"; tail=\"{}日…\"",
                output.len(),
                "é".repeat(62),
                "日本".repeat(20)
            )
        );
        let long_call = format!("TC: {}", "word ".repeat(400));
        let digest = digest(
            &long_call,
            &outcome_digest(ToolStatus::Error, None, &output),
        );
        assert_eq!(
            digest.len(),
            MAX_DIGEST_BYTES,
            "the clipped call fills the bound"
        );
        assert!(digest.contains("…; error; bytes="), "{digest}");
    }

    /// An error variant or a denied execution is `error`; content blocks join their text.
    #[test]
    fn a_wire_output_maps_its_variant_to_the_status() {
        use memory_store::{OutputKind, ResultBlock, ResultBlockKind, ToolOutput};
        assert_eq!(
            from_output(&ToolOutput::bare(OutputKind::ErrorText {
                text: "exit 2\n".to_owned()
            })),
            (ToolStatus::Error, "exit 2\n".to_owned())
        );
        assert_eq!(
            from_output(&ToolOutput::bare(OutputKind::ExecutionDenied {
                reason: None
            })),
            (ToolStatus::Error, String::new())
        );
        let text = |text: &str| ResultBlock {
            kind: ResultBlockKind::Text {
                text: text.to_owned(),
            },
            provider_extras: Default::default(),
        };
        assert_eq!(
            from_output(&ToolOutput::bare(OutputKind::Content {
                blocks: vec![text("a"), text("b")],
            })),
            (ToolStatus::Completed, "a\nb".to_owned())
        );
    }

    /// The status spelling the digest uses is the record's stored spelling.
    #[test]
    fn a_status_spells_as_the_record_stores_it() {
        for status in [ToolStatus::Completed, ToolStatus::Error] {
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
    }

    fn line() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![
                Just("é".to_owned()),
                Just("日".to_owned()),
                Just("🙂".to_owned()),
                Just("\"".to_owned()),
                Just("«".to_owned()),
                Just("»".to_owned()),
                Just(" ".to_owned()),
                Just("\t".to_owned()),
                Just("\r".to_owned()),
                "[a-z]{1,8}",
            ],
            0..120,
        )
        .prop_map(|parts| parts.concat())
    }

    proptest! {
        /// Over arbitrary lines with CRLF, multibyte characters, quotes, and marker brackets: the digest is at most 1 KiB, each excerpt is at most the claim-value bound, holds no quote or bracket, and is the clip of the escaped first or last nonblank line, and a sentinel on a middle line never appears.
        #[test]
        fn every_digest_is_bounded_and_shows_only_its_outer_lines(
            first in line(),
            last in line(),
            call in "[ -~é日]{0,1500}",
            crlf in any::<bool>(),
        ) {
            let newline = if crlf { "\r\n" } else { "\n" };
            let output = format!("a{first}{newline}SENTINEL-mid{newline}{last}z{newline}");
            let built = digest(&call, &outcome_digest(ToolStatus::Completed, None, &output));
            prop_assert!(built.len() <= MAX_DIGEST_BYTES);
            prop_assert!(!built.contains("SENTINEL-mid"));
            let lines: Vec<&str> = output
                .lines()
                .map(str::trim_end)
                .filter(|line| !line.trim().is_empty())
                .collect();
            for (label, expected) in [("head", lines.first()), ("tail", lines.last())] {
                let Some(field) = built.split(&format!("; {label}=\"")).nth(1) else {
                    continue;
                };
                let excerpt = field.split('"').next().unwrap();
                prop_assert!(excerpt.len() <= CLAIM_VALUE_MAX_BYTES);
                prop_assert!(!excerpt.contains(['«', '»']));
                let escaped = expected.unwrap().replace(['"', '«', '»'], "'");
                prop_assert_eq!(excerpt, clip(&escaped, CLAIM_VALUE_MAX_BYTES));
            }
        }
    }
}
