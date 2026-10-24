//! The single site of the daemon's tool vocabulary: every tool name and argument key whose
//! meaning a tool author chose, behind typed queries. A literal is contract when a harness with
//! zero tools registered still emits it (message and part shapes, status, identity, and settled
//! timestamp fields); it is vocabulary when its correctness depends on a tool author's choice
//! (tool names, argument keys, tool-authored metadata such as `state.metadata.exit`). Contract
//! literals stay with the readers that parse them; vocabulary literals live here, and
//! `scripts/forbid-tool-literals.sh` fails CI when a distinctive one appears in daemon production
//! source outside this file.
//!
//! The tables are closed and compile-time. A name no table describes classifies as
//! [`ToolClass::Unknown`], a legal value every caller handles: never dropped, deduplicated, or
//! edit-marked by a class decision, default tier (see [`selection_tier`] for doubly prefixed
//! names), display argument from the generic key list, and no exit code.
//!
//! Each name query applies one fixed [`NameRule`], preserved per caller: selection queries match the
//! [`normalize`]d name, the injection read-back matches the raw name ignoring ASCII case, and the
//! outcome queries match the raw name exactly. Eidnara's own tool identifiers are defined once in
//! [`crate::prompt_surface`] and referenced here, never redefined.

use serde_json::Value;

use crate::harness_sources::Harness;
use crate::prompt_surface::{EIDNARA_NOTE_TOOL, EIDNARA_REDUCE_TOOL};

/// The harness's to-do tool; the injection path also synthesizes calls under this name.
pub(crate) const TODO_TOOL: &str = "todowrite";

const EDIT_TOOLS: &[&str] = &["edit", "write"];
const ZERO_VALUE_META_TOOLS: &[&str] = &["bash_status", "bash_kill"];
/// Tier 1, kept longest: navigation and structure the agent re-uses.
const T1_TOOLS: &[&str] = &[TODO_TOOL, "read", "task", "aft_outline", "aft_zoom"];
/// Tier 2: edit-class continuation context.
const T2_TOOLS: &[&str] = &["edit", "write", "apply_patch", "grep", "glob", "aft_search"];
/// The one tool whose `state.metadata.exit` is a process exit code.
const EXIT_CODE_TOOL: &str = "bash";
/// `eidnara_note` actions whose calls carry no value once read.
const ZERO_VALUE_NOTE_ACTIONS: &[&str] = &["read", "dismiss"];
const NOTE_ACTION_KEY: &str = "action";

/// Path keys in precedence order; the first string value wins.
const PATH_KEYS: &[&str] = &["filePath", "file_path", "path"];
/// Display keys whose value is truncated, in precedence order.
const DISPLAY_PATH_KEYS: &[&str] = &["filePath", "path", "pattern", "query"];
/// Display keys whose value is shown whole, consulted after [`DISPLAY_PATH_KEYS`].
const DISPLAY_SYMBOL_KEYS: &[&str] = &["symbol", "module", "action"];
/// Keys whose string values an edit marker clamps to a region hint.
const DIFF_KEYS: &[&str] = &[
    "oldString",
    "newString",
    "content",
    "old_string",
    "new_string",
];

const MCP_PREFIX: &str = "mcp_";

/// How a query compares a harness-supplied name with a tabled name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameRule {
    /// Compare the raw name ignoring ASCII case.
    IgnoreAsciiCase,
    /// Compare the raw name byte for byte.
    Exact,
}

impl NameRule {
    fn matches(self, name: &str, tabled: &str) -> bool {
        match self {
            Self::IgnoreAsciiCase => name.eq_ignore_ascii_case(tabled),
            Self::Exact => name == tabled,
        }
    }
}

/// Lowercases `name` and strips one leading `mcp_`, the shipped-default rule for MCP-surfaced
/// names. A stripped MCP name equal to a tabled name (`mcp_edit` and `edit`) takes that name's
/// class.
pub(crate) fn normalize(name: &str) -> String {
    let mut lower = name.to_lowercase();
    if lower.starts_with(MCP_PREFIX) {
        lower.drain(..MCP_PREFIX.len());
    }
    lower
}

/// The selection class of a tool, from its [`normalize`]d name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    /// Older calls superseded by a newer call on the same path become edit markers.
    Edit,
    /// The newest call is the live plan; older calls may drop.
    TodoWrite,
    /// Every call may drop.
    ZeroValueMeta,
    /// Eidnara's reduce facade; the newest calls stay as housekeeping examples.
    EidnaraReduce,
    /// Eidnara's note facade; a call whose action is zero-value may drop.
    EidnaraNote,
    /// A tabled name with no class rule of its own.
    Tabled,
    /// A name no table describes.
    Unknown,
}

/// Total over every string.
pub fn classify(name: &str) -> ToolClass {
    let name = normalize(name);
    let name = name.as_str();
    if EDIT_TOOLS.contains(&name) {
        ToolClass::Edit
    } else if name == TODO_TOOL {
        ToolClass::TodoWrite
    } else if ZERO_VALUE_META_TOOLS.contains(&name) {
        ToolClass::ZeroValueMeta
    } else if name == EIDNARA_REDUCE_TOOL {
        ToolClass::EidnaraReduce
    } else if name == EIDNARA_NOTE_TOOL {
        ToolClass::EidnaraNote
    } else if T1_TOOLS.contains(&name) || T2_TOOLS.contains(&name) || name == EXIT_CODE_TOOL {
        ToolClass::Tabled
    } else {
        ToolClass::Unknown
    }
}

/// Eviction tier; lower tiers are kept longer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    T1,
    T2,
    /// The default for every name outside the tier tables, [`ToolClass::Unknown`] included.
    T3,
}

/// The tier of `name` after [`normalize`].
pub(crate) fn tier(name: &str) -> Tier {
    let name = normalize(name);
    if T1_TOOLS.contains(&name.as_str()) {
        Tier::T1
    } else if T2_TOOLS.contains(&name.as_str()) {
        Tier::T2
    } else {
        Tier::T3
    }
}

/// The tier selection assigns an arc's raw tool name. Selection normalizes the name once when it
/// groups an arc and again in the tier lookup, so a name carrying two `mcp_` prefixes takes the
/// tier of the name beneath them (`mcp_mcp_edit` is [`Tier::T2`]).
pub fn selection_tier(name: &str) -> Tier {
    tier(&normalize(name))
}

/// Whether the injection read-back treats `name` as the to-do tool ([`NameRule::IgnoreAsciiCase`]
/// on the raw name).
pub(crate) fn is_injected_todo_tool(name: &str) -> bool {
    NameRule::IgnoreAsciiCase.matches(name, TODO_TOOL)
}

/// Whether an `eidnara_note` call's `action` is one whose call carries no value once read.
pub fn is_zero_value_note_call(input: &Value) -> bool {
    first_string(input, &[NOTE_ACTION_KEY])
        .is_some_and(|action| ZERO_VALUE_NOTE_ACTIONS.contains(&action))
}

fn first_string<'v>(input: &'v Value, keys: &[&str]) -> Option<&'v str> {
    let object = input.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}

/// The path a call names, harness-neutral: Pi `path`, OpenCode `filePath`, and `file_path`, the
/// first string value in that precedence.
pub fn path_arg(input: &Value) -> Option<&str> {
    first_string(input, PATH_KEYS)
}

/// The argument a one-line tool summary shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayArg<'v> {
    /// A path, pattern, or query, which a summary truncates.
    PathLike(&'v str),
    /// A symbol, module, or action, which a summary shows whole.
    SymbolLike(&'v str),
}

pub(crate) fn display_arg(input: &Value) -> Option<DisplayArg<'_>> {
    first_string(input, DISPLAY_PATH_KEYS)
        .map(DisplayArg::PathLike)
        .or_else(|| first_string(input, DISPLAY_SYMBOL_KEYS).map(DisplayArg::SymbolLike))
}

pub fn diff_keys() -> &'static [&'static str] {
    DIFF_KEYS
}

/// The exit code a settled call reports: the integer `state.metadata.exit` of an OpenCode call
/// whose tool is exactly `bash` ([`NameRule::Exact`]). Every Pi record, every other tool, and a
/// null, string, or float `exit` yield `None`.
pub(crate) fn exit_code(harness: Harness, name: &str, metadata: Option<&Value>) -> Option<i64> {
    match harness {
        Harness::OpenCode if NameRule::Exact.matches(name, EXIT_CODE_TOOL) => {
            metadata?.get("exit")?.as_i64()
        }
        Harness::OpenCode | Harness::Pi => None,
    }
}

/// The harness's own truncation flag on a settled call: OpenCode `metadata.truncated` read from
/// the tool part's `state`, and Pi `details.truncation.truncated` read from the `toolResult`
/// message. A missing or non-boolean flag is `None`.
pub(crate) fn truncated_flag(harness: Harness, record: &Value) -> Option<bool> {
    let pointer = match harness {
        Harness::OpenCode => "/metadata/truncated",
        Harness::Pi => "/details/truncation/truncated",
    };
    record.pointer(pointer).and_then(Value::as_bool)
}

#[cfg(test)]
pub(crate) const NOTE_ACTIONS: &[&str] = ZERO_VALUE_NOTE_ACTIONS;

#[cfg(test)]
pub(crate) const TABLED_NAMES: &[&[&str]] = &[
    EDIT_TOOLS,
    ZERO_VALUE_META_TOOLS,
    T1_TOOLS,
    T2_TOOLS,
    &[EXIT_CODE_TOOL, TODO_TOOL],
];
