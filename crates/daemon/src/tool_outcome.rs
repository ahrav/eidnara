//! The execution-outcome extractor: one reading of what a harness reported about a settled tool call. Each field comes from a named native field, and an absent or malformed optional field is `None`, meaning unknown.
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
    state
        .get("metadata")
        .and_then(|metadata| metadata.get("interrupted"))
        .and_then(Value::as_bool)
        == Some(true)
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
            .get("details")
            .and_then(|details| details.get("truncation"))
            .and_then(|truncation| truncation.get("truncated"))
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

/// The outcome and output text of a tool result as its wire block holds it, the shape the summarizer reads. An error output variant or a denied execution is `error`; the wire block carries no harness metadata, so the record leaves `truncated`, `exit_code`, and `exception_ref` unset.
pub fn from_output(tool_name: &str, output: &memory_store::ToolOutput) -> (ToolOutcome, String) {
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
    let (status, text) = match &output.kind {
        OutputKind::Text { text } => (ToolStatus::Completed, text.clone()),
        OutputKind::Json { value } => (ToolStatus::Completed, value.to_string()),
        OutputKind::Content { blocks } => (ToolStatus::Completed, text_of(blocks)),
        OutputKind::ErrorText { text } => (ToolStatus::Error, text.clone()),
        OutputKind::ErrorJson { value } => (ToolStatus::Error, value.to_string()),
        OutputKind::ErrorContent { blocks } => (ToolStatus::Error, text_of(blocks)),
        OutputKind::ExecutionDenied { reason } => {
            (ToolStatus::Error, reason.clone().unwrap_or_default())
        }
    };
    let outcome = ToolOutcome {
        version: TOOL_OUTCOME_VERSION,
        status,
        tool_name: tool_name.to_owned(),
        byte_length: text.len() as u64,
        truncated: None,
        exit_code: None,
        exception_ref: None,
    };
    (outcome, text)
}

/// The largest digest the summarizer sees for one tool result.
pub const MAX_DIGEST_BYTES: usize = 1024;
/// Marks an excerpt or a call summary clipped to its bound.
const CLIP_MARKER: &str = "…";

/// The summarizer's view of one tool result: `<call>; <status>[; exit=N]; bytes=N[; head="…"][; tail="…"]`, at most [`MAX_DIGEST_BYTES`]. `call` is the call's `TC:` summary. `head` and `tail` are the output's first and last nonblank lines, each at most the claim-value bound and cut on a character boundary with a clip marker; a single nonblank line is `head` alone. A double quote or a citation-marker bracket inside an excerpt becomes a single quote, so each excerpt stays one quoted field and holds no marker. Equal inputs give equal bytes, and no output byte outside the two excerpts appears.
pub fn digest(call: &str, outcome: &ToolOutcome, output: &str) -> String {
    let mut rest = format!("; {}", outcome.status.as_str());
    if let Some(exit_code) = outcome.exit_code {
        rest.push_str(&format!("; exit={exit_code}"));
    }
    rest.push_str(&format!("; bytes={}", output.len()));
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
            rest.push_str(&format!("; {label}=\"{excerpt}\""));
        }
    }
    let mut digest = clip(call, MAX_DIGEST_BYTES - rest.len());
    digest.push_str(&rest);
    digest
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

    fn completed(output: &str) -> (ToolOutcome, String) {
        from_output(
            "bash",
            &memory_store::ToolOutput::bare(memory_store::OutputKind::Text {
                text: output.to_owned(),
            }),
        )
    }

    /// `sc-digest-bounded-deterministic-char-boundary`: CRLF lines lose their carriage returns; a single nonblank line is `head` alone; equal inputs give equal bytes; only the first and last nonblank lines appear.
    #[test]
    fn a_digest_shows_status_bytes_and_the_outer_nonblank_lines() {
        let output = "\r\n  first line\r\nSENTINEL middle\r\n\r\nlast \"quoted\" line\r\n\r\n";
        let (outcome, text) = completed(output);
        let digest = digest("TC: bash(run)", &outcome, &text);
        assert_eq!(
            digest,
            format!(
                "TC: bash(run); completed; bytes={}; head=\"  first line\"; tail=\"last 'quoted' line\"",
                output.len()
            )
        );
        assert_eq!(digest, super::digest("TC: bash(run)", &outcome, &text));
        assert!(!digest.contains("SENTINEL") && !digest.contains('\r'));

        let (outcome, text) = completed("only line\n");
        assert_eq!(
            super::digest("TC: read(a.rs)", &outcome, &text),
            "TC: read(a.rs); completed; bytes=10; head=\"only line\""
        );
        let (outcome, text) = completed("");
        assert_eq!(
            super::digest("TC: read(a.rs)", &outcome, &text),
            "TC: read(a.rs); completed; bytes=0"
        );
    }

    /// Excerpts are at most the claim-value bound and cut on a character boundary with the clip marker; a call summary that would push the digest past 1 KiB is clipped the same way; an exit code shows when the record has one.
    #[test]
    fn a_digest_clips_on_character_boundaries_within_its_bounds() {
        let line = "é".repeat(100);
        let (outcome, text) = completed(&format!("{line}\n{}", "日本".repeat(60)));
        let digest = digest("TC: bash", &outcome, &text);
        let head = digest
            .split("head=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let tail = digest
            .split("tail=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        for excerpt in [head, tail] {
            assert!(excerpt.len() <= crate::history_summarizer_citations::CLAIM_VALUE_MAX_BYTES);
            assert!(excerpt.ends_with(CLIP_MARKER), "{excerpt}");
        }
        assert!(head.starts_with("éé") && tail.starts_with("日本"));

        let mut failed = outcome.clone();
        failed.status = ToolStatus::Error;
        failed.exit_code = Some(2);
        let long_call = format!("TC: {}", "word ".repeat(400));
        let digest = super::digest(&long_call, &failed, &text);
        assert!(digest.len() <= MAX_DIGEST_BYTES, "{}", digest.len());
        assert!(digest.contains("…; error; exit=2; bytes="), "{digest}");
    }

    /// An error variant or a denied execution is `error`; content blocks join their text.
    #[test]
    fn a_wire_output_maps_its_variant_to_the_status() {
        use memory_store::{OutputKind, ResultBlock, ResultBlockKind, ToolOutput};
        let error = from_output(
            "bash",
            &ToolOutput::bare(OutputKind::ErrorText {
                text: "exit 2: cargo not found\n".to_owned(),
            }),
        );
        assert_eq!(error.0.status, ToolStatus::Error);
        assert_eq!(
            super::digest("TC: bash", &error.0, &error.1),
            "TC: bash; error; bytes=24; head=\"exit 2: cargo not found\""
        );
        let denied = from_output(
            "bash",
            &ToolOutput::bare(OutputKind::ExecutionDenied { reason: None }),
        );
        assert_eq!(
            (denied.0.status, denied.1.as_str()),
            (ToolStatus::Error, "")
        );
        let content = from_output(
            "read",
            &ToolOutput::bare(OutputKind::Content {
                blocks: vec![
                    ResultBlock {
                        kind: ResultBlockKind::Text {
                            text: "a".to_owned(),
                        },
                        provider_extras: Default::default(),
                    },
                    ResultBlock {
                        kind: ResultBlockKind::Text {
                            text: "b".to_owned(),
                        },
                        provider_extras: Default::default(),
                    },
                ],
            }),
        );
        assert_eq!(
            (content.0.status, content.1.as_str()),
            (ToolStatus::Completed, "a\nb")
        );
    }
}
