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
