//! This module performs pure rendering for project-memory and session-history prompt surfaces.

use crate::canonical_memory::CanonicalMemory;
use crate::decay_render::{DecayRenderHistorySegment, TokenCount, render_decayed_counted};
use std::cmp::Ordering;

/// `<session-history>` is never omitted so the provider prompt-cache retains a stable breakpoint.
/// Omitting `<session-history>` would shift subsequent prompt bytes and invalidate the cache.
pub const M0_EMPTY_BODY: &str = "<session-history></session-history>";
/// `M1_PLACEHOLDER` keeps the m1 delta block non-empty when it has no new content.
/// The m1 block remains non-empty to preserve the provider prompt-cache breakpoint.
pub const M1_PLACEHOLDER: &str = "(no new content since last materialization)";
/// Default pre-pressure token budget for rendered session history.
pub const DEFAULT_HISTORY_BUDGET_TOKENS: f64 = 60_000.0;

fn escape_xml_content(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_escaped_xml_content(&mut out, s);
    out
}

/// Appends `s` with `&`, `<`, and `>` replaced by their XML entities.
fn push_escaped_xml_content(out: &mut String, s: &str) {
    let bytes = s.as_bytes();
    let mut copied = 0;
    for i in memchr::memchr3_iter(b'&', b'<', b'>', bytes) {
        // Every escaped byte is ASCII, so `i` and `i + 1` are char boundaries.
        out.push_str(&s[copied..i]);
        out.push_str(match bytes[i] {
            b'&' => "&amp;",
            b'<' => "&lt;",
            _ => "&gt;",
        });
        copied = i + 1;
    }
    out.push_str(&s[copied..]);
}

/// Appends `s` with `&`, `<`, `>`, and `"` replaced by their XML entities.
fn push_escaped_xml_attr(out: &mut String, s: &str) {
    let mut copied = 0;
    for (i, byte) in s.bytes().enumerate() {
        let entity = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            _ => continue,
        };
        // Every escaped byte is ASCII, so `i` and `i + 1` are char boundaries.
        out.push_str(&s[copied..i]);
        out.push_str(entity);
        copied = i + 1;
    }
    out.push_str(&s[copied..]);
}

/// `MEMORY_CATEGORY_ORDER` defines the canonical project-memory categories in render order.
pub(crate) const MEMORY_CATEGORY_ORDER: [&str; 5] = [
    "PROJECT_RULES",
    "ARCHITECTURE",
    "CONSTRAINTS",
    "CONFIG_VALUES",
    "NAMING",
];

pub(crate) const POSITIVE_MEMORY_CATEGORIES: [&str; 12] = [
    "PROJECT_RULES",
    "ARCHITECTURE",
    "CONSTRAINTS",
    "CONFIG_VALUES",
    "NAMING",
    "USER_DIRECTIVES",
    "USER_PREFERENCES",
    "CONFIG_DEFAULTS",
    "ARCHITECTURE_DECISIONS",
    "ENVIRONMENT",
    "WORKFLOW_RULES",
    "KNOWN_ISSUES",
];

pub(crate) fn is_positive_memory_category(category: &str) -> bool {
    POSITIVE_MEMORY_CATEGORIES.contains(&category)
}

/// Sort rank of a positive category: its index in [`MEMORY_CATEGORY_ORDER`], or one rank after
/// them for the other positive categories. `None` marks a category the native surfaces drop.
///
/// [`MEMORY_CATEGORY_ORDER`] is a prefix of [`POSITIVE_MEMORY_CATEGORIES`], so one index serves
/// both lists.
fn memory_category_rank(category: &str) -> Option<usize> {
    let position = POSITIVE_MEMORY_CATEGORIES
        .iter()
        .position(|positive| *positive == category)?;
    Some(position.min(MEMORY_CATEGORY_ORDER.len()))
}

/// Ranked categories sort by rank, the rest by name after them, and ties by object id.
fn memory_render_order(
    (left_rank, left): &(usize, &CanonicalMemory),
    (right_rank, right): &(usize, &CanonicalMemory),
) -> Ordering {
    left_rank
        .cmp(right_rank)
        .then_with(|| {
            if *left_rank == MEMORY_CATEGORY_ORDER.len() {
                left.category.cmp(&right.category)
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| left.object_id.cmp(&right.object_id))
}

/// Content cut to at most 64 KiB at a UTF-8 boundary.
fn memory_line_content(memory: &CanonicalMemory) -> &str {
    let mut end = memory.content.len().min(64 * 1024);
    while !memory.content.is_char_boundary(end) {
        end -= 1;
    }
    &memory.content[..end]
}

fn push_memory_line(out: &mut String, memory: &CanonicalMemory) {
    // The object id is caller-supplied text; escaping it and folding line breaks keeps it from closing the block or forging a sibling element.
    for (i, piece) in memory.object_id.split(['\n', '\r']).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_escaped_xml_content(out, piece);
    }
    out.push_str(": ");
    // `<project-memory>` continuation lines must remain indented because m0 byte accounting and the prompt cache depend on its line structure.
    for (i, line) in memory_line_content(memory).split('\n').enumerate() {
        if i > 0 {
            out.push_str("\n  ");
        }
        push_escaped_xml_content(out, line);
    }
}

/// Renders one memory line, escaping XML content and truncating content to at
/// most 64 KiB without splitting a UTF-8 code point.
pub fn render_memory_line(memory: &CanonicalMemory) -> String {
    let mut line =
        String::with_capacity(memory.object_id.len() + memory_line_content(memory).len() + 8);
    push_memory_line(&mut line, memory);
    line
}

/// Renders positive memories grouped in deterministic category and object-id order.
///
/// Native surfaces filter non-positive categories because callers can construct
/// [`CanonicalMemory`] directly and native surfaces lack a warning renderer.
/// Category names are escaped for XML attributes, while `wrapper` is interpolated as written, so callers must pass a valid element name.
/// Memory content is escaped by [`render_memory_line`].
pub fn render_memory_block(memories: &[CanonicalMemory], wrapper: &str) -> String {
    let mut ordered = memories
        .iter()
        .filter_map(|memory| Some((memory_category_rank(&memory.category)?, memory)))
        .collect::<Vec<_>>();
    if ordered.is_empty() {
        return String::new();
    }
    ordered.sort_by(memory_render_order);
    // Unescaped bytes, with every row opening and closing its category; escapes can grow past it.
    let len_hint = ordered
        .iter()
        .map(|(_, memory)| {
            memory.object_id.len()
                + memory_line_content(memory).len()
                + 2 * memory.category.len()
                + 9
        })
        .sum::<usize>()
        + 2 * wrapper.len()
        + 5;
    let mut block = String::with_capacity(len_hint);
    block.push('<');
    block.push_str(wrapper);
    block.push('>');
    let mut open_category: Option<&str> = None;
    for (_, memory) in ordered {
        if open_category != Some(memory.category.as_str()) {
            if let Some(category) = open_category {
                block.push_str("\n</");
                push_escaped_xml_attr(&mut block, category);
                block.push('>');
            }
            open_category = Some(&memory.category);
            block.push_str("\n<");
            push_escaped_xml_attr(&mut block, &memory.category);
            block.push('>');
        }
        block.push('\n');
        push_memory_line(&mut block, memory);
    }
    if let Some(category) = open_category {
        block.push_str("\n</");
        push_escaped_xml_attr(&mut block, category);
        block.push('>');
    }
    block.push_str("\n</");
    block.push_str(wrapper);
    block.push('>');
    block
}

/// The caller token-trims user memories; an empty set renders as an empty string.
pub fn render_user_profile_block(profile_lines: &[String], wrapper: &str) -> String {
    if profile_lines.is_empty() {
        return String::new();
    }
    let mut lines = Vec::with_capacity(profile_lines.len() + 2);
    lines.push(format!("<{wrapper}>"));
    for content in profile_lines {
        lines.push(format!("- {}", escape_xml_content(content)));
    }
    lines.push(format!("</{wrapper}>"));
    lines.join("\n")
}

/// The caller supplies system-role content deduplicated in first-ordinal order.
/// The renderer preserves system-role content byte-for-byte.
/// The renderer does not escape system-role content, preserving original prompt bytes in each entry.
pub fn render_covered_system_messages_block(messages: &[String]) -> String {
    if messages.is_empty() {
        return String::new();
    }
    let mut block = String::from("<covered-system-messages>");
    for content in messages {
        block.push_str("\n<covered-system-message>");
        block.push_str(content);
        block.push_str("</covered-system-message>");
    }
    block.push_str("\n</covered-system-messages>");
    block
}

/// `render_m0` receives blocks the caller has already chosen and token-budget-trimmed.
/// `render_m0` does not choose which rows or history history_segments fit the budget.
pub struct M0Inputs<'a> {
    /// Callers pass an empty string when no `<project-docs>` block exists.
    pub project_docs: &'a str,
    /// The caller token-trims user-profile memory.
    pub user_profile: &'a [String],
    /// The caller deduplicates system-role fragments by ordinal before passing them to `render_m0`.
    /// The caller orders system-role fragments by first appearance before passing them to `render_m0`.
    pub covered_system_messages: &'a [String],
    /// The caller supplies chronologically ordered, token-trimmed history; the renderer applies decay.
    pub history_segments: &'a [DecayRenderHistorySegment],
    /// `history_budget_tokens` is measured before applying the pressure multiplier.
    pub history_budget_tokens: f64,
    /// Values below 1 use an effective multiplier of 1; larger values tighten the effective budget and increase decay.
    /// `decay_pressure_multiplier` produces `effective_budget = history_budget_tokens / max(1, decay_pressure_multiplier)`.
    pub decay_pressure_multiplier: f64,
}

/// `render_m0` expects the caller to pre-trim each sub-block.
pub fn render_m0(inputs: &M0Inputs, estimate_tokens: impl Fn(&str) -> usize) -> String {
    render_m0_counted(inputs, &estimate_tokens).0
}

const SESSION_HISTORY_OPEN: &str = "<session-history>\n";
const SESSION_HISTORY_CLOSE: &str = "\n</session-history>";

/// [`render_m0`] with the count of its first `<session-history>` block under a counter that
/// [counts joins exactly](TokenCount::counts_joins_exactly), when that block is the rendered
/// history: no earlier section opens the tag and the body never closes it. The history body
/// opens with a `## ` heading, so the block counts as its opening line, the body, and its
/// closing line apart.
pub(crate) fn render_m0_counted(
    inputs: &M0Inputs,
    tokens: &impl TokenCount,
) -> (String, Option<usize>) {
    let mut sections: Vec<String> = Vec::new();
    if !inputs.project_docs.is_empty() {
        sections.push(inputs.project_docs.to_string());
    }
    let user_profile = render_user_profile_block(inputs.user_profile, "user-profile");
    if !user_profile.is_empty() {
        sections.push(user_profile);
    }
    let covered_systems = render_covered_system_messages_block(inputs.covered_system_messages);
    if !covered_systems.is_empty() {
        sections.push(covered_systems);
    }

    let effective_budget = inputs.history_budget_tokens / inputs.decay_pressure_multiplier.max(1.0);
    let (session_history, body_tokens) =
        render_decayed_counted(inputs.history_segments, effective_budget, tokens);
    let history = if session_history.is_empty() {
        M0_EMPTY_BODY.to_string()
    } else {
        format!("{SESSION_HISTORY_OPEN}{session_history}{SESSION_HISTORY_CLOSE}")
    };
    let history_len = history.len();
    sections.push(history);

    let m0 = sections.join("\n\n").trim().to_string();
    let first_block = memchr::memmem::find(m0.as_bytes(), b"<session-history>");
    let block_tokens = body_tokens
        .filter(|_| tokens.counts_joins_exactly() && !session_history.is_empty())
        .filter(|_| first_block == Some(m0.len() - history_len))
        .filter(|_| {
            memchr::memmem::find(session_history.as_bytes(), b"</session-history>").is_none()
        })
        .map(|body| {
            tokens.count(SESSION_HISTORY_OPEN) + body + tokens.count(SESSION_HISTORY_CLOSE)
        });
    (m0, block_tokens)
}

/// Wraps non-empty delta blocks in `<session-history-since>` in argument order.
///
/// Returns `placeholder` unchanged when every delta block is empty.
pub fn assemble_m1(
    memory_updates: &str,
    new_history_segments: &str,
    new_memories: &str,
    new_user_profile: &str,
    placeholder: &str,
) -> String {
    let mut blocks: Vec<&str> = Vec::with_capacity(4);
    for piece in [
        memory_updates,
        new_history_segments,
        new_memories,
        new_user_profile,
    ] {
        if !piece.is_empty() {
            blocks.push(piece);
        }
    }
    if blocks.is_empty() {
        return placeholder.to_string();
    }
    format!(
        "<session-history-since>\n{}\n</session-history-since>",
        blocks.join("\n")
    )
}

/// Renders non-empty history_segment input at tier 1 inside `<new-history_segments>`.
pub fn render_new_history_segments(
    history_segments: &[&crate::decay_render::DecayRenderHistorySegment],
) -> String {
    let bodies: Vec<String> = history_segments
        .iter()
        .map(|c| crate::decay_render::render_history_segment_at_tier(c, 1))
        .filter(|body| !body.is_empty())
        .collect();
    if bodies.is_empty() {
        return String::new();
    }
    format!(
        "<new-history_segments>\n{}\n</new-history_segments>",
        bodies.join("\n\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `render_memory_block` and `render_memory_line` as they read before the single-buffer
    /// rewrite: replace chains, a `Vec<String>` of lines, and a stable sort that scans the
    /// category order on every comparison.
    mod replace_chain {
        use super::super::{
            CanonicalMemory, MEMORY_CATEGORY_ORDER, Ordering, is_positive_memory_category,
        };

        fn escape_xml_attr(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        }

        fn escape_xml_content(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        }

        fn order(left: &CanonicalMemory, right: &CanonicalMemory) -> Ordering {
            let rank = |memory: &CanonicalMemory| {
                MEMORY_CATEGORY_ORDER
                    .iter()
                    .position(|category| *category == memory.category)
            };
            match (rank(left), rank(right)) {
                (Some(left_rank), Some(right_rank)) => left_rank
                    .cmp(&right_rank)
                    .then_with(|| left.object_id.cmp(&right.object_id)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => left
                    .category
                    .cmp(&right.category)
                    .then_with(|| left.object_id.cmp(&right.object_id)),
            }
        }

        pub(super) fn line(memory: &CanonicalMemory) -> String {
            let mut end = memory.content.len().min(64 * 1024);
            while !memory.content.is_char_boundary(end) {
                end -= 1;
            }
            let content = escape_xml_content(&memory.content[..end]).replace('\n', "\n  ");
            let object_id = escape_xml_content(&memory.object_id).replace(['\n', '\r'], " ");
            format!("{object_id}: {content}")
        }

        pub(super) fn block(memories: &[CanonicalMemory], wrapper: &str) -> String {
            let mut ordered = memories
                .iter()
                .filter(|memory| is_positive_memory_category(&memory.category))
                .collect::<Vec<_>>();
            if ordered.is_empty() {
                return String::new();
            }
            ordered.sort_by(|left, right| order(left, right));
            let mut lines = vec![format!("<{wrapper}>")];
            let mut open_category: Option<&str> = None;
            for memory in ordered {
                if open_category != Some(memory.category.as_str()) {
                    if let Some(category) = open_category {
                        lines.push(format!("</{}>", escape_xml_attr(category)));
                    }
                    open_category = Some(&memory.category);
                    lines.push(format!("<{}>", escape_xml_attr(&memory.category)));
                }
                lines.push(line(memory));
            }
            if let Some(category) = open_category {
                lines.push(format!("</{}>", escape_xml_attr(category)));
            }
            lines.push(format!("</{wrapper}>"));
            lines.join("\n")
        }
    }

    #[test]
    fn the_memory_block_matches_the_replace_chain_renderer() {
        let categories = [
            "NAMING",
            "PROJECT_RULES",
            "KNOWN_ISSUES",
            "CONFIG_VALUES",
            "REJECTED_APPROACH",
            "USER_DIRECTIVES",
            "ARCHITECTURE",
            "ENVIRONMENT",
            "CONSTRAINTS",
            "FUTURE_NEGATIVE_CATEGORY",
        ];
        let ids = [
            "mem_00000000000000000000000000000007",
            "mem_00000000000000000000000000000003",
            "mem_0000000000000000000000000000000a",
            "mem_",
            "",
            "id & <x>\nnext\r\nline",
            "\u{e9}t\u{e9}",
            "mem_00000000000000000000000000000003",
        ];
        let contents = [
            String::new(),
            "plain fact".to_owned(),
            "a & b < c > d\nsecond line\n\nfourth & <tag/>".to_owned(),
            "\n\n&&&<<<>>>\r\n".to_owned(),
            format!("{}\u{e9}after the cap", "x".repeat(64 * 1024 - 1)),
            "\u{65e5}\u{672c}\u{8a9e} & \u{1f642}\nline".repeat(9),
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as usize
        };
        for round in 0..200 {
            let count = next() % 24;
            let memories: Vec<CanonicalMemory> = (0..count)
                .map(|_| CanonicalMemory {
                    object_id: ids[next() % ids.len()].to_owned(),
                    category: categories[next() % categories.len()].to_owned(),
                    content: contents[next() % contents.len()].clone(),
                })
                .collect();
            assert_eq!(
                render_memory_block(&memories, "project-memory"),
                replace_chain::block(&memories, "project-memory"),
                "round {round}"
            );
            for memory in &memories {
                assert_eq!(render_memory_line(memory), replace_chain::line(memory));
            }
        }
    }

    /// The exact tokenizer as a counter that follows the join identity.
    struct Exact;

    impl TokenCount for Exact {
        fn count(&self, text: &str) -> usize {
            tokenizer::estimate_tokens(text)
        }

        fn counts_joins_exactly(&self) -> bool {
            true
        }
    }

    fn history_rows(rows: usize, date: &str) -> Vec<DecayRenderHistorySegment> {
        crate::test_support::synthetic_history::SyntheticHistory::mixed(rows)
            .rows()
            .iter()
            .enumerate()
            .map(|(n, stored)| {
                let mut row = DecayRenderHistorySegment::from(stored);
                row.title = match n % 5 {
                    0 => format!("{} ", row.title),
                    1 => format!("{}\u{3000}", row.title),
                    2 => format!("{}\n", row.title),
                    _ => row.title,
                };
                if n % 7 == 3 {
                    row.p1 = None;
                }
                row.start_date = Some(date.to_string());
                row.end_date = Some(date.to_string());
                row
            })
            .collect()
    }

    /// Under an exact counter the history block counts as its parts, and the bytes match the
    /// render that counts the extracted block. A block another section opens first, or a body
    /// that closes the tag, falls back to counting the extracted block.
    #[test]
    fn an_exact_counter_counts_the_history_block_from_its_body() {
        for (date, docs, counted) in [
            ("2026-06-08", "", true),
            (
                "2026-06-08",
                "<project-docs>\nno history here\n</project-docs>",
                true,
            ),
            (
                "2026-06-08",
                "<project-docs>\n<session-history>\n</project-docs>",
                false,
            ),
            ("</session-history>", "", false),
        ] {
            let rows = history_rows(600, date);
            for budget in [400.0, 4_000.0, 20_000.0, 60_000.0] {
                let inputs = M0Inputs {
                    project_docs: docs,
                    user_profile: &[],
                    covered_system_messages: &[],
                    history_segments: &rows,
                    history_budget_tokens: budget,
                    decay_pressure_multiplier: 1.0,
                };
                let (m0, block_tokens) = render_m0_counted(&inputs, &Exact);
                assert_eq!(m0, render_m0(&inputs, tokenizer::estimate_tokens));
                let block = crate::decay_render::m0_block(&m0, "session-history").unwrap();
                if counted {
                    assert_eq!(
                        block_tokens,
                        Some(tokenizer::estimate_tokens(block)),
                        "budget {budget}"
                    );
                } else {
                    assert_eq!(block_tokens, None, "{date} {docs} budget {budget}");
                }
            }
        }
    }

    #[test]
    fn an_exact_counter_counts_a_history_block_whose_body_ends_in_whitespace() {
        for tail in [" ", "\u{3000}", "\t", "  "] {
            let mut rows = history_rows(5, "2026-06-08");
            let newest = rows.last_mut().unwrap();
            newest.title = format!("Open question{tail}");
            newest.p1 = None;
            newest.content = String::new();
            let inputs = M0Inputs {
                project_docs: "",
                user_profile: &[],
                covered_system_messages: &[],
                history_segments: &rows,
                history_budget_tokens: 60_000.0,
                decay_pressure_multiplier: 1.0,
            };
            let (m0, block_tokens) = render_m0_counted(&inputs, &Exact);
            let block = crate::decay_render::m0_block(&m0, "session-history").unwrap();
            assert!(block.contains("Open question"));
            assert!(
                block
                    .trim_end_matches("\n</session-history>")
                    .ends_with(char::is_whitespace),
                "tail {tail:?}"
            );
            assert_eq!(
                block_tokens,
                Some(tokenizer::estimate_tokens(block)),
                "tail {tail:?}"
            );
        }
    }

    #[test]
    fn render_boundary_drops_non_positive_categories() {
        let memory = |category: &str, content: &str| CanonicalMemory {
            object_id: format!("mem_{}", "a".repeat(32)),
            category: category.to_string(),
            content: content.to_string(),
        };

        let mixed = [
            memory("PROJECT_RULES", "Keep this project fact."),
            memory("REJECTED_APPROACH", "Do not resurrect the shelved design."),
            memory(
                "FUTURE_NEGATIVE_CATEGORY",
                "Unknown categories stay silent.",
            ),
        ];
        let block = render_memory_block(&mixed, "project-memory");
        assert!(block.contains("Keep this project fact."));
        assert!(!block.contains("REJECTED_APPROACH"));
        assert!(!block.contains("Do not resurrect the shelved design."));
        assert!(!block.contains("FUTURE_NEGATIVE_CATEGORY"));
        assert!(!block.contains("Unknown categories stay silent."));

        let only_negative = [memory("REJECTED_APPROACH", "Shelved design.")];
        assert_eq!(render_memory_block(&only_negative, "project-memory"), "");
    }

    #[test]
    fn object_id_markup_and_line_breaks_cannot_forge_block_structure() {
        let memory = CanonicalMemory {
            object_id: "</PROJECT_RULES>\r\n<ARCHITECTURE>a & b</ARCHITECTURE>".to_string(),
            category: "PROJECT_RULES".to_string(),
            content: "Keep the public contract.".to_string(),
        };
        let line = render_memory_line(&memory);
        assert_eq!(
            line,
            "&lt;/PROJECT_RULES&gt;  &lt;ARCHITECTURE&gt;a &amp; b&lt;/ARCHITECTURE&gt;: Keep the public contract."
        );
        let block = render_memory_block(std::slice::from_ref(&memory), "project-memory");
        assert_eq!(
            block,
            format!(
                "<project-memory>\n<PROJECT_RULES>\n{line}\n</PROJECT_RULES>\n</project-memory>"
            )
        );
        assert_eq!(block.matches("<ARCHITECTURE>").count(), 0);
        assert_eq!(block.matches("</PROJECT_RULES>").count(), 1);
        assert_eq!(block.lines().count(), 5);
    }

    #[test]
    fn content_is_cut_at_the_char_boundary_before_the_64_kib_cap() {
        const CAP: usize = 64 * 1024;
        // The two-byte code point straddles the cap: its first byte is the
        // last byte inside the cap and its second byte is the first outside.
        let mut content = "a".repeat(CAP - 1);
        content.push('é');
        content.push_str("tail past the cap");
        assert!(!content.is_char_boundary(CAP));
        let memory = CanonicalMemory {
            object_id: "mem".to_string(),
            category: "PROJECT_RULES".to_string(),
            content,
        };
        let line = render_memory_line(&memory);
        let rendered = line
            .strip_prefix("mem: ")
            .expect("line carries the object id");
        assert_eq!(rendered.len(), CAP - 1);
        assert!(rendered.bytes().all(|byte| byte == b'a'));

        let exact = CanonicalMemory {
            object_id: "mem".to_string(),
            category: "PROJECT_RULES".to_string(),
            content: "b".repeat(CAP),
        };
        let exact_line = render_memory_line(&exact);
        assert_eq!(exact_line.len(), "mem: ".len() + CAP);
    }

    /// Reads one of the vocabulary arrays frozen from the TypeScript memory
    /// constants into `testdata/memory-category-vocabulary.json`.
    fn vocabulary_array(name: &str) -> Vec<String> {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/memory-category-vocabulary.json"))
                .expect("vocabulary fixture parses");
        fixture[name]
            .as_array()
            .unwrap_or_else(|| panic!("vocabulary fixture lacks {name}"))
            .iter()
            .map(|value| value.as_str().expect("category is a string").to_string())
            .collect()
    }

    #[test]
    fn positive_category_vocabulary_matches_the_frozen_typescript_arrays() {
        assert_eq!(
            vocabulary_array("CATEGORY_PRIORITY"),
            POSITIVE_MEMORY_CATEGORIES
        );

        // CATEGORY_PRIORITY only orders rows; writable taxonomies determine category validity.
        // The test gates decision kinds against writable taxonomies so newly writable positive categories fail instead of being dropped.
        for name in ["V2_MEMORY_CATEGORIES", "PROMOTABLE_CATEGORIES"] {
            let categories = vocabulary_array(name);
            assert!(!categories.is_empty(), "frozen {name} is empty");
            for category in categories {
                assert!(
                    is_positive_memory_category(&category),
                    "frozen {name} entry {category} is missing from POSITIVE_MEMORY_CATEGORIES"
                );
            }
        }
        assert!(!is_positive_memory_category("REJECTED_APPROACH"));
        assert!(!is_positive_memory_category("NOT_A_CATEGORY"));
    }

    #[test]
    fn render_order_is_a_prefix_of_the_positive_vocabulary() {
        // `memory_render_order` must keep `MEMORY_CATEGORY_ORDER` equal to the vocabulary prefix of `POSITIVE_MEMORY_CATEGORIES` so sorting and inclusion remain aligned.
        assert_eq!(
            MEMORY_CATEGORY_ORDER[..],
            POSITIVE_MEMORY_CATEGORIES[..MEMORY_CATEGORY_ORDER.len()]
        );
    }
}
