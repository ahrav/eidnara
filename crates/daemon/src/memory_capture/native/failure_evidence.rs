//! A failure report pairs a framing sentence, such as "PR 812's CI went red
//! after the rebase:", with the failure itself, usually a fenced log or a bare
//! log line. The extractor quotes one span, and a framing quote on its own
//! loses the error text a later search names. Each failure line of the source
//! therefore joins the memory whose quotation frames it, so one memory carries
//! both the reference and the error identity. Every appended line is an exact
//! line of the same source message, so the memory holds only source text.

use crate::memory_capture::{CapturedMemory, MAX_CAPTURE_MEMORY_BYTES};

/// A failure's identity is its diagnostic header, its message, its location,
/// and the failing test. Four lines hold those for compiler, panic, exception,
/// and test-runner output and keep the memory about one failure.
const MAX_EVIDENCE_LINES: usize = 4;

/// Marks a line cut to fit the memory budget.
const TRUNCATION_MARK: &str = "…";

/// One failure line, with the line that completes it, as a byte range of the
/// source and its trimmed text.
struct Evidence<'a> {
    start: usize,
    end: usize,
    lines: Vec<&'a str>,
}

/// Appends each failure line of `source` to the memory whose quotation frames
/// it: the nearest quotation ending before the line, or else the nearest one
/// starting after the failure line. A memory gains at most
/// `MAX_EVIDENCE_LINES` lines, each appended only within
/// `MAX_CAPTURE_MEMORY_BYTES`; a source without failure lines leaves every
/// memory unchanged.
pub(super) fn attach(source: &str, memories: &mut [CapturedMemory]) {
    let quotes: Vec<Option<(usize, usize)>> = memories
        .iter()
        .map(|memory| {
            source
                .find(&memory.quote)
                .map(|start| (start, start + memory.quote.len()))
        })
        .collect();
    let mut appended = vec![0; memories.len()];
    for evidence in failure_evidence(source) {
        let Some(target) = framing_memory(&quotes, &evidence) else {
            continue;
        };
        let content = &mut memories[target].content;
        for line in evidence.lines {
            if appended[target] == MAX_EVIDENCE_LINES {
                break;
            }
            if content.contains(line) {
                continue;
            }
            let room = MAX_CAPTURE_MEMORY_BYTES.saturating_sub(content.len() + 1);
            let text = if line.len() <= room {
                line
            } else {
                let keep = line.floor_char_boundary(room.saturating_sub(TRUNCATION_MARK.len()));
                if keep == 0 {
                    break;
                }
                &line[..keep]
            };
            content.push('\n');
            content.push_str(text);
            if text.len() < line.len() {
                content.push_str(TRUNCATION_MARK);
            }
            appended[target] += 1;
        }
    }
}

fn framing_memory(quotes: &[Option<(usize, usize)>], evidence: &Evidence<'_>) -> Option<usize> {
    let spans = || {
        quotes
            .iter()
            .enumerate()
            .filter_map(|(index, span)| span.map(|span| (index, span)))
    };
    let before = spans()
        .filter(|(_, (_, end))| *end <= evidence.start)
        .min_by_key(|(index, (_, end))| (evidence.start - end, *index));
    let after = || {
        spans()
            .filter(|(_, (start, _))| *start >= evidence.end)
            .min_by_key(|(index, (start, _))| (start - evidence.end, *index))
    };
    before.or_else(after).map(|(index, _)| index)
}

/// Failure lines in source order. Inside a fenced block a diagnostic may start
/// at any word, since log lines carry timestamps and prefixes; outside a fence
/// it must start the line, so prose that mentions an error is not evidence.
/// Evidence lines hold printable text and tabs only, which limits the frozen
/// plan's JSON escaping to twice the appended bytes.
/// The next line completes a failure line that ends with `:`, as a panic header
/// does, or that is followed by a location (`-->`) or a top stack frame (`at`).
fn failure_evidence(source: &str) -> Vec<Evidence<'_>> {
    let mut items = Vec::new();
    let mut fenced = false;
    let mut offset = 0;
    let mut lines = source
        .split('\n')
        .map(|line| {
            let start = offset;
            offset += line.len() + 1;
            (start, line)
        })
        .peekable();
    while let Some((start, line)) = lines.next() {
        let text = line.trim();
        if is_fence(text) {
            fenced = !fenced;
            continue;
        }
        if !is_failure_line(text, fenced) {
            continue;
        }
        let mut item = Evidence {
            start,
            end: start + line.len(),
            lines: vec![text],
        };
        if let Some(&(next_start, next)) = lines.peek() {
            let next_text = next.trim();
            if !next_text.is_empty()
                && printable(next_text)
                && !is_fence(next_text)
                && (text.ends_with(':')
                    || next_text.starts_with("-->")
                    || next_text.starts_with("at "))
            {
                item.lines.push(next_text);
                item.end = next_start + next.len();
                lines.next();
            }
        }
        items.push(item);
    }
    items
}

fn is_fence(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

fn printable(line: &str) -> bool {
    !line.chars().any(|c| c.is_control() && c != '\t')
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

fn is_failure_line(line: &str, anywhere: bool) -> bool {
    if !printable(line) {
        return false;
    }
    if !anywhere {
        return diagnostic_at(line);
    }
    line.char_indices().any(|(index, c)| {
        c != '.'
            && is_word(c)
            && !line[..index].chars().next_back().is_some_and(is_word)
            && diagnostic_at(&line[index..])
    })
}

/// Whether `text` starts with a diagnostic header that compilers, runtimes,
/// test runners, and loggers print: `panicked at`; a level word (`ERROR`,
/// `FAIL`, `FAILED`); or an error word (`error`, `fatal`, `panic`, or a type
/// name ending in `Error` or `Exception`) followed by `:` or a `[` code.
fn diagnostic_at(text: &str) -> bool {
    if text.starts_with("panicked at") {
        return true;
    }
    let end = text.find(|c| !is_word(c)).unwrap_or(text.len());
    let word = text[..end].trim_end_matches('.');
    if matches!(word, "ERROR" | "FAIL" | "FAILED") {
        return true;
    }
    let rest = &text[end..];
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
    fn a_turn_without_a_failure_is_unchanged() {
        let source = "Use port 4321 for staging. The error handling lives in src/errors.rs.\n```rust\nfn port() -> u16 {\n    4321\n}\n```\nerror budgets are tracked weekly.";
        let mut memories = [
            memory("User stated", "Use port 4321 for staging."),
            memory("User stated", "The error handling lives in src/errors.rs."),
        ];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Use port 4321 for staging.",
                "User stated: The error handling lives in src/errors.rs."
            ]
        );
    }

    #[test]
    fn a_bare_log_line_joins_the_sentence_before_it() {
        let source = "Deploying build 4410 to staging stopped at the migration step.\nERROR migrate: relation \"sessions\" already exists";
        let mut memories = [memory(
            "User stated",
            "Deploying build 4410 to staging stopped at the migration step.",
        )];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Deploying build 4410 to staging stopped at the migration step.\nERROR migrate: relation \"sessions\" already exists"
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
    fn a_framing_memory_gains_the_error_line_another_memory_quotes() {
        let source = "Issue 5120 regressed:\n```\nValueError: invalid literal for int() with base 10: 'abc'\n```";
        let mut memories = [
            memory("User stated", "Issue 5120 regressed:"),
            memory(
                "User stated",
                "ValueError: invalid literal for int() with base 10: 'abc'",
            ),
        ];
        assert_eq!(
            contents(source, &mut memories),
            [
                "User stated: Issue 5120 regressed:\nValueError: invalid literal for int() with base 10: 'abc'",
                "User stated: ValueError: invalid literal for int() with base 10: 'abc'"
            ]
        );
    }

    #[test]
    fn a_failure_before_any_quotation_joins_the_next_quotation() {
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

        let full = "x".repeat(MAX_CAPTURE_MEMORY_BYTES);
        let source = format!("{full}\n```\nError: late\n```");
        let mut memories = [memory("User stated", &full)];
        let stored = contents(&source, &mut memories).remove(0);
        assert_eq!(stored, format!("User stated: {full}"));
    }
}
