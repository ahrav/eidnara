//! A failure report pairs a framing sentence, such as "PR 812's CI went red
//! after the rebase:", with the failure itself, usually a fenced log or a bare
//! log line. The extractor quotes one span, and a framing quote on its own
//! loses the error text a later search names. The failure lines of a block
//! therefore join the memory whose quotation introduces that block, so one
//! memory carries both the reference and the error identity. Appended lines
//! are trimmed lines of the same source message.

use std::ops::Range;

use crate::memory_capture::{CapturedMemory, MAX_CAPTURE_MEMORY_BYTES};

/// A failure's identity is its diagnostic header, its message, its location,
/// and the failing test. Four lines hold those for compiler, panic, exception,
/// and test-runner output and keep the memory about one failure.
const MAX_EVIDENCE_LINES: usize = 4;

/// Marks a line cut to fit the memory budget.
const TRUNCATION_MARK: &str = "…";

/// Fence info strings that label terminal output or a shell session. A fence
/// labeled with a programming or configuration language holds source code,
/// whose `error:` fields and `except ValueError:` clauses are code.
const OUTPUT_FENCES: [&str; 14] = [
    "",
    "text",
    "txt",
    "plaintext",
    "console",
    "terminal",
    "log",
    "logs",
    "output",
    "shell-session",
    "sh",
    "bash",
    "shell",
    "zsh",
];

/// One source line: its byte range, the offset of its first non-whitespace
/// byte, and its trimmed text.
struct Line<'a> {
    start: usize,
    text_start: usize,
    end: usize,
    text: &'a str,
}

/// One failure line with the line that completes it. `span` covers those
/// lines; `block` covers the fenced block holding them, or equals `span` for
/// a line outside any fence.
struct Evidence<'a> {
    block: Range<usize>,
    span: Range<usize>,
    lines: Vec<&'a str>,
}

/// Appends the failure lines of `source` to the memory whose quotation
/// introduces their block. A memory gains at most `MAX_EVIDENCE_LINES` lines,
/// and its content grows only within `MAX_CAPTURE_MEMORY_BYTES`; a source
/// without failure lines, or whose failure lines no quotation introduces,
/// leaves every memory unchanged.
pub(super) fn attach(source: &str, memories: &mut [CapturedMemory]) {
    let lines = lines(source);
    let quotes: Vec<Option<Range<usize>>> = memories
        .iter()
        .map(|memory| {
            source
                .find(&memory.quote)
                .map(|start| start..start + memory.quote.len())
        })
        .collect();
    let mut appended = vec![0; memories.len()];
    for evidence in failure_evidence(&lines) {
        let Some(target) = framing_memory(source, &lines, &quotes, &evidence) else {
            continue;
        };
        let content = &mut memories[target].content;
        for line in evidence.lines {
            if content.contains(line) {
                continue;
            }
            if appended[target] == MAX_EVIDENCE_LINES || !push_clipped(content, line) {
                break;
            }
            appended[target] += 1;
        }
    }
}

/// Appends `line` on its own line, cut at a character boundary and marked
/// when the whole line would pass `MAX_CAPTURE_MEMORY_BYTES`. Returns false
/// and leaves `content` unchanged when no character of `line` fits.
fn push_clipped(content: &mut String, line: &str) -> bool {
    let room = MAX_CAPTURE_MEMORY_BYTES.saturating_sub(content.len() + 1);
    let (text, mark) = if line.len() <= room {
        (line, "")
    } else {
        let keep = line.floor_char_boundary(room.saturating_sub(TRUNCATION_MARK.len()));
        (&line[..keep], TRUNCATION_MARK)
    };
    if text.is_empty() {
        return false;
    }
    content.push('\n');
    content.push_str(text);
    content.push_str(mark);
    true
}

/// The memory whose quotation introduces the evidence, in this order: the
/// latest quotation that ends on the last nonblank line before the block when
/// that line ends with `:` and no sentence ends between the quotation and the
/// colon; a quotation overlapping the failure lines; a quotation that begins
/// the first nonblank line after the block.
fn framing_memory(
    source: &str,
    lines: &[Line<'_>],
    quotes: &[Option<Range<usize>>],
    evidence: &Evidence<'_>,
) -> Option<usize> {
    let spans = || {
        quotes
            .iter()
            .enumerate()
            .filter_map(|(index, span)| span.as_ref().map(|span| (index, span)))
    };
    let intro = lines
        .iter()
        .rev()
        .find(|line| line.end <= evidence.block.start && !line.text.is_empty())
        .filter(|line| line.text.ends_with(':'));
    let introducing = intro.and_then(|intro| {
        spans()
            .filter(|(_, quote)| {
                quote.end > intro.text_start
                    && quote.end <= intro.end
                    && same_sentence(source, quote.end, intro.end)
            })
            .max_by_key(|(index, quote)| (quote.end, std::cmp::Reverse(*index)))
    });
    let overlapping = || {
        spans()
            .find(|(_, quote)| quote.start < evidence.span.end && evidence.span.start < quote.end)
    };
    let following = || {
        let next = lines
            .iter()
            .find(|line| line.start >= evidence.block.end && !line.text.is_empty())?;
        spans().find(|(_, quote)| quote.start == next.text_start)
    };
    introducing
        .or_else(overlapping)
        .or_else(following)
        .map(|(index, _)| index)
}

/// Whether the text from `quote_end` to `end` continues the quotation's last
/// sentence: the quotation does not end with a sentence terminator before more
/// text, and no terminator followed by whitespace occurs in between.
fn same_sentence(source: &str, quote_end: usize, end: usize) -> bool {
    let gap = &source[quote_end..end];
    let terminator = |c: char| matches!(c, '.' | '!' | '?');
    let quote_closes = source[..quote_end]
        .chars()
        .next_back()
        .is_some_and(terminator);
    !(quote_closes && !gap.trim().is_empty())
        && !gap.char_indices().any(|(index, c)| {
            terminator(c) && gap[index + c.len_utf8()..].starts_with(char::is_whitespace)
        })
}

fn lines(source: &str) -> Vec<Line<'_>> {
    let mut offset = 0;
    source
        .split('\n')
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            let text_start = start + (line.len() - line.trim_start().len());
            Line {
                start,
                text_start,
                end: start + line.len(),
                text: line.trim(),
            }
        })
        .collect()
}

/// Failure lines in source order. Fence lines pair in order, opener with the
/// next fence line; a source holding an odd number of them, such as a
/// fragment cut inside a block, is read as prose throughout.
fn failure_evidence<'a>(lines: &[Line<'a>]) -> Vec<Evidence<'a>> {
    let mut items = Vec::new();
    let fences: Vec<usize> = (0..lines.len())
        .filter(|&index| is_fence(lines[index].text))
        .collect();
    if fences.len() % 2 == 1 {
        scan(lines, None, &mut items);
        return items;
    }
    let mut prose = 0;
    for &[open, close] in fences.as_chunks::<2>().0 {
        scan(&lines[prose..open], None, &mut items);
        if is_output_fence(lines[open].text) {
            let block = lines[open].start..lines[close].end;
            scan(&lines[open + 1..close], Some(block), &mut items);
        }
        prose = close + 1;
    }
    scan(&lines[prose..], None, &mut items);
    items
}

/// Collects the failure lines of one region: the inside of the fenced block
/// `block`, or prose when `block` is `None`. The next line completes a failure
/// line that ends with `:`, as a panic header does, or that is followed by a
/// location (`-->`) or a top stack frame (`at`).
fn scan<'a>(region: &[Line<'a>], block: Option<Range<usize>>, items: &mut Vec<Evidence<'a>>) {
    let mut index = 0;
    while index < region.len() {
        let line = &region[index];
        index += 1;
        if !is_failure_line(line.text, block.is_some()) {
            continue;
        }
        let mut evidence = Evidence {
            block: 0..0,
            span: line.start..line.end,
            lines: vec![line.text],
        };
        if let Some(next) = region.get(index) {
            let next_text = next.text;
            if !next_text.is_empty()
                && printable(next_text)
                && !is_fence(next_text)
                && (line.text.ends_with(':')
                    || next_text.starts_with("-->")
                    || next_text.starts_with("at "))
            {
                evidence.lines.push(next_text);
                evidence.span.end = next.end;
                index += 1;
            }
        }
        evidence.block = block.clone().unwrap_or_else(|| evidence.span.clone());
        items.push(evidence);
    }
}

fn is_fence(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

fn is_output_fence(opener: &str) -> bool {
    let info = opener.trim_start_matches(['`', '~']).trim();
    let language = info.split_whitespace().next().unwrap_or("");
    OUTPUT_FENCES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(language))
}

/// Evidence lines hold printable text and tabs only, which limits the frozen
/// plan's JSON escaping to twice the appended bytes.
fn printable(line: &str) -> bool {
    !line.chars().any(|c| c.is_control() && c != '\t')
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Whether `line` reports a failure. Inside an output block, `panicked at` and
/// a level word count at any word, since runners print them mid-line, and an
/// error header counts at the line start or after leading log tokens
/// (timestamps, `file:line:col` locations, bracketed tags). In prose the line
/// must begin with one of these.
fn is_failure_line(line: &str, fenced: bool) -> bool {
    if !printable(line) {
        return false;
    }
    if !fenced {
        return failure_marker(line) || error_header(line);
    }
    line.char_indices().any(|(index, c)| {
        let rest = &line[index..];
        is_word(c)
            && !line[..index].chars().next_back().is_some_and(is_word)
            && (failure_marker(rest)
                || (error_header(rest) && line[..index].split_whitespace().all(is_log_token)))
    })
}

fn is_log_token(token: &str) -> bool {
    token.starts_with('[') || token.bytes().any(|byte| byte.is_ascii_digit())
}

fn leading_word(text: &str) -> (&str, &str) {
    let end = text.find(|c| !is_word(c)).unwrap_or(text.len());
    (text[..end].trim_end_matches('.'), &text[end..])
}

/// `panicked at`, or the level word `ERROR`, `FAIL`, or `FAILED`, at the start
/// of `text`.
fn failure_marker(text: &str) -> bool {
    text.starts_with("panicked at") || matches!(leading_word(text).0, "ERROR" | "FAIL" | "FAILED")
}

/// An error word at the start of `text` (`error`, `fatal`, `FATAL`, `panic`,
/// or a type name ending in `Error` or `Exception`) followed by `:` or a `[`
/// code, as compilers, runtimes, and loggers print.
fn error_header(text: &str) -> bool {
    let (word, rest) = leading_word(text);
    let delimited = (rest.starts_with(':') && !rest.starts_with("::")) || rest.starts_with('[');
    delimited
        && (matches!(word, "error" | "fatal" | "FATAL" | "panic")
            || word.ends_with("Error")
            || word.ends_with("Exception"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(attribution: &str, quote: &str) -> CapturedMemory {
        CapturedMemory {
            category: "CONSTRAINTS".into(),
            content: format!("{attribution}: {quote}"),
            quote: quote.into(),
            replaces: None,
            duplicate_of: None,
        }
    }

    fn contents(source: &str, memories: &mut [CapturedMemory]) -> Vec<String> {
        attach(source, memories);
        memories
            .iter()
            .map(|memory| memory.content.clone())
            .collect()
    }

    /// Every memory of `source` keeps its quotation-only content.
    fn assert_unchanged(source: &str, quotes: &[&str]) {
        let mut memories: Vec<_> = quotes
            .iter()
            .map(|quote| memory("User stated", quote))
            .collect();
        let expected: Vec<String> = quotes
            .iter()
            .map(|quote| format!("User stated: {quote}"))
            .collect();
        assert_eq!(contents(source, &mut memories), expected, "{source:?}");
    }

    #[test]
    fn a_framing_sentence_keeps_the_fenced_error_line_and_its_location() {
        let source = "PR 812's CI went red after the rebase:\n\n```\n   Compiling server v0.4.0\nerror[E0425]: cannot find value `config` in this scope\n  --> src/server.rs:88:17\n   |\n88 |     let port = config.port;\n```\nCan you look?";
        let mut memories = [memory(
            "User stated",
            "PR 812's CI went red after the rebase:",
        )];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: PR 812's CI went red after the rebase:\nerror[E0425]: cannot find value `config` in this scope\n--> src/server.rs:88:17"
            ]
        );
        assert_eq!(memories[0].quote, "PR 812's CI went red after the rebase:");
    }

    #[test]
    fn tool_output_quoted_in_an_assistant_reply_keeps_the_failing_test_and_exception() {
        let source = "Running `bun test` for OPS-2291 fails on the session suite:\n```text\n$ bun test src/api\nFAIL src/api/session.test.ts\nTypeError: Cannot read properties of undefined (reading 'token')\n    at refresh (src/api/session.ts:42:17)\n    at processTicksAndRejections (node:internal/process/task_queues:95:5)\n```";
        let mut memories = [memory(
            "Assistant reported",
            "Running `bun test` for OPS-2291 fails on the session suite:",
        )];
        assert_eq!(
            contents(source, &mut memories),
            [
                "Assistant reported: Running `bun test` for OPS-2291 fails on the session suite:\nFAIL src/api/session.test.ts\nTypeError: Cannot read properties of undefined (reading 'token')\nat refresh (src/api/session.ts:42:17)"
            ]
        );
    }

    #[test]
    fn a_panic_header_keeps_the_message_on_its_next_line() {
        let source = "The nightly job for INC-77 broke again:\n```\nthread 'decode::tests::rejects_short_frame' panicked at src/decode.rs:41:9:\ncalled `Result::unwrap()` on an `Err` value: InvalidLength { declared: 4096, available: 12 }\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\ntest result: FAILED. 41 passed; 1 failed\n```";
        let mut memories = [memory(
            "User stated",
            "The nightly job for INC-77 broke again:",
        )];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: The nightly job for INC-77 broke again:\nthread 'decode::tests::rejects_short_frame' panicked at src/decode.rs:41:9:\ncalled `Result::unwrap()` on an `Err` value: InvalidLength { declared: 4096, available: 12 }\ntest result: FAILED. 41 passed; 1 failed"
            ]
        );
    }

    #[test]
    fn a_log_prefix_before_the_error_header_still_marks_a_failure_line() {
        let source = "The 03:00 backup for OPS-12 aborted:\n```log\n2025-03-02T03:00:41Z [backup] error: snapshot volume is read-only\nsrc/io.c:88:5: fatal: short write\n```";
        let mut memories = [memory(
            "User stated",
            "The 03:00 backup for OPS-12 aborted:",
        )];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: The 03:00 backup for OPS-12 aborted:\n2025-03-02T03:00:41Z [backup] error: snapshot volume is read-only\nsrc/io.c:88:5: fatal: short write"
            ]
        );
    }

    #[test]
    fn a_turn_without_a_failure_is_unchanged() {
        assert_unchanged(
            "Use port 4321 for staging. The error handling lives in src/errors.rs.\n```rust\nfn port() -> u16 {\n    4321\n}\n```\nerror budgets are tracked weekly.",
            &[
                "Use port 4321 for staging.",
                "The error handling lives in src/errors.rs.",
            ],
        );
    }

    #[test]
    fn source_code_with_error_words_is_not_a_failure() {
        let code = "pub error: Option<String>,\nParseError::Empty => return None,\nreturn Err(ParseError::Empty);\n} catch (error: unknown) {\nexcept ValueError:\n    port = 4321";
        for fence in ["```", "```rust", "```ts", "```python"] {
            assert_unchanged(
                &format!("The handler keeps its own error type:\n{fence}\n{code}\n```"),
                &["The handler keeps its own error type:"],
            );
        }
        assert_unchanged(
            "The service config is:\n```yaml\nerror: false\nretries: 3\n```",
            &["The service config is:"],
        );
    }

    #[test]
    fn a_failure_next_to_an_unrelated_fact_stays_out_of_it() {
        assert_unchanged(
            "Our convention is snake_case for DB columns.\n\nUnrelated, my local build is broken right now:\n```\nerror: linker `cc` not found\n```",
            &["Our convention is snake_case for DB columns."],
        );
        assert_unchanged(
            "PR 812 fails after the rebase. Also, staging uses port 4321.\n```\nerror[E0425]: cannot find value `config`\n```",
            &[
                "PR 812 fails after the rebase.",
                "Also, staging uses port 4321.",
            ],
        );
        assert_unchanged(
            "Staging uses port 4321. CI broke on an unrelated branch:\n```\nerror: linker `cc` not found\n```",
            &["Staging uses port 4321."],
        );
        assert_unchanged(
            "Handlers use snake_case\nCI broke on main:\n```\nerror: linker `cc` not found\n```",
            &["Handlers use snake_case"],
        );
        assert_unchanged(
            "Use snake_case for handler names.\n\nThe database host restarted overnight.\nERROR db: connection refused",
            &["Use snake_case for handler names."],
        );
    }

    #[test]
    fn a_bare_log_line_joins_the_sentence_that_introduces_it() {
        let source = "Deploying build 4410 to staging stopped at the migration step:\nERROR migrate: relation \"sessions\" already exists";
        let mut memories = [memory(
            "User stated",
            "Deploying build 4410 to staging stopped at the migration step:",
        )];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Deploying build 4410 to staging stopped at the migration step:\nERROR migrate: relation \"sessions\" already exists"
            ]
        );
    }

    #[test]
    fn each_failure_joins_its_own_framing_sentence() {
        let source = "Ticket WEB-31 fails on Linux:\n```\nerror: linker `cc` not found\n```\nTicket WEB-40 fails on macOS:\n```\nfatal: not a git repository\n```";
        let mut memories = [
            memory("User stated", "Ticket WEB-31 fails on Linux:"),
            memory("User stated", "Ticket WEB-40 fails on macOS:"),
        ];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Ticket WEB-31 fails on Linux:\nerror: linker `cc` not found",
                "User stated: Ticket WEB-40 fails on macOS:\nfatal: not a git repository"
            ]
        );
    }

    #[test]
    fn the_quotation_nearest_the_colon_introduces_the_block() {
        let source =
            "Staging is down and PR 812 fails on CI:\n```\nerror: linker `cc` not found\n```";
        let mut memories = [
            memory("User stated", "Staging is down"),
            memory("User stated", "PR 812 fails on CI:"),
        ];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Staging is down",
                "User stated: PR 812 fails on CI:\nerror: linker `cc` not found"
            ]
        );
    }

    #[test]
    fn a_framing_quotation_may_end_before_the_colon_of_its_sentence() {
        let source = "Issue 5120 regressed on my branch, differently this time:\n```\nValueError: invalid literal for int() with base 10: 'abc'\n```";
        let mut memories = [
            memory("User stated", "Issue 5120 regressed on my branch"),
            memory(
                "User stated",
                "ValueError: invalid literal for int() with base 10: 'abc'",
            ),
        ];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Issue 5120 regressed on my branch\nValueError: invalid literal for int() with base 10: 'abc'",
                "User stated: ValueError: invalid literal for int() with base 10: 'abc'"
            ]
        );
    }

    #[test]
    fn a_quotation_of_the_error_header_gains_its_location() {
        let source =
            "```\nerror[E0425]: cannot find value `config`\n  --> src/server.rs:88:17\n```";
        let mut memories = [memory(
            "User stated",
            "error[E0425]: cannot find value `config`",
        )];
        assert_eq!(
            contents(source, &mut memories),
            ["User stated: error[E0425]: cannot find value `config`\n--> src/server.rs:88:17"]
        );
    }

    #[test]
    fn a_failure_before_any_prose_joins_the_quotation_that_follows_it() {
        let source = "```\npanic: runtime error: index out of range [3] with length 3\n```\nThat is the crash from SUP-118.";
        let mut memories = [memory("User stated", "That is the crash from SUP-118.")];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: That is the crash from SUP-118.\npanic: runtime error: index out of range [3] with length 3"
            ]
        );
    }

    #[test]
    fn an_unpaired_fence_reads_the_whole_source_as_prose() {
        assert_unchanged(
            "  at foo (x.js:1:1)\n```\nWe chose port 4321:\nThe parser raises ValueError: when input is empty.",
            &["We chose port 4321:"],
        );
        assert_unchanged(
            "We chose port 4321:\n```\nThe parser logs ERROR on empty input.\n```\nThe log follows.\n```\n2025-03-02 ERROR",
            &["We chose port 4321:"],
        );
        let source = "Deploy 5 stopped:\nERROR migrate:\n```";
        let mut memories = [memory("User stated", "Deploy 5 stopped:")];
        assert_eq!(
            contents(source, &mut memories),
            ["User stated: Deploy 5 stopped:\nERROR migrate:"]
        );
    }

    #[test]
    fn a_repeated_line_is_appended_once() {
        let source = "Build 9 fails:\n```\nerror: x\nerror: x\nerror: x\n```";
        let mut memories = [memory("User stated", "Build 9 fails:")];
        assert_eq!(
            contents(source, &mut memories),
            ["User stated: Build 9 fails:\nerror: x"]
        );
    }

    #[test]
    fn a_header_line_stops_at_a_fence_or_a_control_character() {
        let source = "Job 3 fails:\n```\nthread 'main' panicked at src/a.rs:1:1:\n```\nnext";
        let mut memories = [memory("User stated", "Job 3 fails:")];
        assert_eq!(
            contents(source, &mut memories),
            ["User stated: Job 3 fails:\nthread 'main' panicked at src/a.rs:1:1:"]
        );
        let source =
            "Job 4 fails:\n```\nthread 'main' panicked at src/a.rs:1:1:\n\u{1b}[31mboom\n```";
        let mut memories = [memory("User stated", "Job 4 fails:")];
        assert_eq!(
            contents(source, &mut memories),
            ["User stated: Job 4 fails:\nthread 'main' panicked at src/a.rs:1:1:"]
        );
    }

    #[test]
    fn a_line_with_terminal_control_codes_is_left_out() {
        let source = "Build 90 fails:\n```\nerror: linking failed \u{1b}[0m\nerror: could not compile `server`\n```";
        let mut memories = [memory("User stated", "Build 90 fails:")];
        assert_eq!(
            contents(source, &mut memories),
            ["User stated: Build 90 fails:\nerror: could not compile `server`"]
        );
    }

    #[test]
    fn evidence_stops_at_the_line_cap() {
        let errors: String = (0..10)
            .map(|index| format!("error: unresolved import `crate::m{index}`\n"))
            .collect();
        let source = format!("Build 77 fails:\n```\n{errors}```");
        let mut memories = [memory("User stated", "Build 77 fails:")];
        let stored = contents(&source, &mut memories).remove(0);
        assert_eq!(stored.lines().count(), 1 + MAX_EVIDENCE_LINES);
        assert!(stored.ends_with("error: unresolved import `crate::m3`"));
    }

    #[test]
    fn evidence_stays_within_the_memory_budget_at_a_character_boundary() {
        let long = format!("Error: {}", "é".repeat(MAX_CAPTURE_MEMORY_BYTES));
        let source = format!("Job 12 fails:\n```\n{long}\nError: second\n```");
        let mut memories = [memory("User stated", "Job 12 fails:")];
        let stored = contents(&source, &mut memories).remove(0);
        assert!(stored.len() <= MAX_CAPTURE_MEMORY_BYTES);
        assert!(stored.len() > MAX_CAPTURE_MEMORY_BYTES - 4);
        assert!(stored.starts_with("User stated: Job 12 fails:\nError: éé"));
        assert!(stored.ends_with(TRUNCATION_MARK));
        assert!(!stored.contains("second"));

        let full = format!("{}:", "x".repeat(MAX_CAPTURE_MEMORY_BYTES));
        let source = format!("{full}\n```\nError: late\n```");
        let mut memories = [memory("User stated", &full)];
        let stored = contents(&source, &mut memories).remove(0);
        assert_eq!(stored, format!("User stated: {full}"));
    }
}
