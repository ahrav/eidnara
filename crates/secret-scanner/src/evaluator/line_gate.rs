use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use aho_corasick::automaton::Automaton as _;
use aho_corasick::dfa::DFA as LiteralDfa;
use regex_automata::dfa::{Automaton as _, StartKind, dense};
use regex_automata::util::start;
use regex_syntax::ParserBuilder;
use regex_syntax::hir::{Class, Hir, HirKind, Look};

use super::*;
use crate::ScanProfile;

static EMBEDDED: LazyLock<RuleSet> = LazyLock::new(|| RuleSet::from_embedded().unwrap());

/// The embedded rules with the line gate off: `evaluate` takes its `captures_iter` branch for
/// every rule, which makes this set the reference for the gated branch.
static WHOLE_INPUT: LazyLock<RuleSet> = LazyLock::new(|| {
    RuleSet::from_embedded()
        .unwrap()
        .searched_over_the_whole_input()
});

const LINE_GATED_RULES: [&str; 4] = [
    "magic-value-digits",
    "magic-value-digits-quoted",
    "magic-value-field",
    "magic-value-option",
];

fn gated_rules() -> impl Iterator<Item = (&'static Rule, &'static [&'static str])> {
    EMBEDDED
        .active(ScanProfile::Comprehensive)
        .filter_map(|rule| rule.line_literals.map(|literals| (rule, literals)))
}

fn parse(rule: &Rule) -> Hir {
    ParserBuilder::new()
        .unicode(rule.declaration.unicode)
        .utf8(false)
        .build()
        .parse(&rule.declaration.regex)
        .unwrap_or_else(|error| panic!("{}: {error}", rule.declaration.name))
}

fn consumes(hir: &Hir, byte: u8) -> bool {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => false,
        HirKind::Literal(literal) => literal.0.contains(&byte),
        HirKind::Class(Class::Bytes(class)) => class
            .ranges()
            .iter()
            .any(|range| range.start() <= byte && byte <= range.end()),
        HirKind::Class(Class::Unicode(class)) => class
            .ranges()
            .iter()
            .any(|range| range.start() <= char::from(byte) && char::from(byte) <= range.end()),
        HirKind::Repetition(repetition) => consumes(&repetition.sub, byte),
        HirKind::Capture(capture) => consumes(&capture.sub, byte),
        HirKind::Concat(parts) | HirKind::Alternation(parts) => {
            parts.iter().any(|part| consumes(part, byte))
        }
    }
}

fn looks(hir: &Hir, found: &mut Vec<Look>) {
    match hir.kind() {
        HirKind::Look(look) => found.push(*look),
        HirKind::Repetition(repetition) => looks(&repetition.sub, found),
        HirKind::Capture(capture) => looks(&capture.sub, found),
        HirKind::Concat(parts) | HirKind::Alternation(parts) => {
            parts.iter().for_each(|part| looks(part, found));
        }
        HirKind::Empty | HirKind::Literal(_) | HirKind::Class(_) => {}
    }
}

/// Word-boundary assertions treat a trailing line feed and the end of input alike.
#[test]
fn every_match_of_a_line_gated_rule_fits_on_one_line() {
    let mut names = Vec::new();
    for (rule, literals) in gated_rules() {
        let name = rule.declaration.name.as_str();
        names.push(name);
        assert_eq!(rule.source, RuleSource::ConservativeOverlay, "{name}");
        assert!(rule.declaration.credential_form.is_some(), "{name}");
        let hir = parse(rule);
        assert!(
            !consumes(&hir, b'\n'),
            "{name}: a match of the pattern can hold a line feed"
        );
        assert!(
            hir.properties().minimum_len().is_some_and(|len| len > 0),
            "{name}: the pattern matches the empty string"
        );
        let mut found = Vec::new();
        looks(&hir, &mut found);
        for look in found {
            assert!(
                matches!(
                    look,
                    Look::WordAscii
                        | Look::WordAsciiNegate
                        | Look::WordUnicode
                        | Look::WordUnicodeNegate
                ),
                "{name}: {look:?} can read a line feed differently from the end of the input"
            );
        }
        for literal in literals {
            assert!(!literal.is_empty(), "{name}");
            assert_eq!(
                *literal,
                literal.to_ascii_lowercase(),
                "{name}: the literal search folds the input to lower case"
            );
        }
    }
    assert_eq!(names, LINE_GATED_RULES);
    let client = EMBEDDED
        .active(ScanProfile::Comprehensive)
        .find(|rule| rule.declaration.name == "magic-value-client-option")
        .unwrap();
    assert!(
        consumes(&parse(client), b'\n'),
        "the client pattern reads a line break before its program"
    );
    assert_eq!(client.line_literals, None);
}

/// Returns a string that the anchored pattern matches after some look-behind byte, or at the
/// start of the input, and that contains none of `literals` under ASCII case folding.
fn match_without_a_literal(rule: &Rule, literals: &[&str]) -> Option<Vec<u8>> {
    let name = rule.declaration.name.as_str();
    let nfa = regex_automata::nfa::thompson::Compiler::new()
        .configure(regex_automata::nfa::thompson::Config::new().utf8(false))
        .build_from_hir(&parse(rule))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let pattern = dense::Builder::new()
        .configure(
            dense::Config::new()
                .match_kind(regex_automata::MatchKind::All)
                .start_kind(StartKind::Anchored),
        )
        .build_from_nfa(&nfa)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let gate = LiteralDfa::builder()
        .ascii_case_insensitive(true)
        .match_kind(aho_corasick::MatchKind::Standard)
        .start_kind(aho_corasick::StartKind::Unanchored)
        .build(literals)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    let gate_start = gate.start_state(aho_corasick::Anchored::No).unwrap();

    let mut gate_states = vec![gate_start];
    let mut known: HashSet<_> = gate_states.iter().copied().collect();
    let mut index = 0;
    while let Some(&state) = gate_states.get(index) {
        index += 1;
        for byte in 0..=255u8 {
            let next = gate.next_state(aho_corasick::Anchored::No, state, byte);
            if !gate.is_match(next) && known.insert(next) {
                gate_states.push(next);
            }
        }
    }
    let mut classes = HashSet::new();
    let representatives: Vec<u8> = (0..=255u8)
        .filter(|&byte| {
            let mut signature = vec![usize::from(pattern.byte_classes().get(byte))];
            signature.extend(gate_states.iter().map(|&state| {
                gate.next_state(aho_corasick::Anchored::No, state, byte)
                    .as_usize()
            }));
            classes.insert(signature)
        })
        .collect();

    let mut queue = VecDeque::new();
    let mut paths = HashMap::new();
    for look_behind in std::iter::once(None).chain((0..=255u8).map(Some)) {
        let config = start::Config::new()
            .anchored(regex_automata::Anchored::Yes)
            .look_behind(look_behind);
        let state = pattern
            .start_state(&config)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        if paths.insert((state, gate_start), Vec::new()).is_none() {
            queue.push_back((state, gate_start));
        }
    }
    while let Some((state, gate_state)) = queue.pop_front() {
        let path: Vec<u8> = paths[&(state, gate_state)].clone();
        if pattern.is_match_state(pattern.next_eoi_state(state)) {
            return Some(path);
        }
        for &byte in &representatives {
            let next = pattern.next_state(state, byte);
            assert!(!pattern.is_quit_state(next), "{name}: the automaton quits");
            // The automaton enters a match state one byte after the match ends.
            if pattern.is_match_state(next) {
                return Some(path);
            }
            let next_gate = gate.next_state(aho_corasick::Anchored::No, gate_state, byte);
            if pattern.is_dead_state(next) || gate.is_match(next_gate) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = paths.entry((next, next_gate))
            {
                let mut longer = path.clone();
                longer.push(byte);
                slot.insert(longer);
                queue.push_back((next, next_gate));
            }
        }
    }
    None
}

#[test]
fn every_match_of_a_line_gated_rule_contains_one_of_its_literals() {
    for (rule, literals) in gated_rules() {
        assert_eq!(
            match_without_a_literal(rule, literals),
            None,
            "{}",
            rule.declaration.name
        );
        for missing in 0..literals.len() {
            let fewer: Vec<&str> = literals
                .iter()
                .enumerate()
                .filter_map(|(index, literal)| (index != missing).then_some(*literal))
                .collect();
            assert!(
                match_without_a_literal(rule, &fewer).is_some(),
                "{}: {:?} is never the only literal of a match",
                rule.declaration.name,
                literals[missing]
            );
        }
    }
}

fn spans(captures: &Captures<'_>) -> Vec<Option<(usize, usize)>> {
    (0..captures.len())
        .map(|group| captures.get(group).map(|part| (part.start(), part.end())))
        .collect()
}

fn assert_gated_search_replays_captures_iter(input: &str) {
    let bytes = input.as_bytes();
    let mut starts = LiteralStarts::new(bytes);
    for (rule, literals) in gated_rules() {
        let expected: Vec<_> = rule
            .regex
            .captures_iter(bytes)
            .map(|captures| spans(&captures))
            .collect();
        let gated: Vec<_> = LineGatedCaptures::new(&rule.regex, literals, &mut starts)
            .map(|captures| spans(&captures))
            .collect();
        assert_eq!(gated, expected, "{} on {input:?}", rule.declaration.name);
    }
}

/// Compares whole reports: findings, `candidates_evaluated`, `work_bytes`, and `limits_hit`.
fn assert_gated_scan_reports_the_whole_input_scan(input: &str, limits: ScanLimits) {
    for profile in [ScanProfile::Conservative, ScanProfile::Comprehensive] {
        assert_eq!(
            evaluate(&EMBEDDED, profile, limits, [0; 32], input),
            evaluate(&WHOLE_INPUT, profile, limits, [0; 32], input),
            "{profile:?} under {limits:?} on {input:?}"
        );
    }
}

fn generated_text(parts: &[(bool, usize)]) -> String {
    parts
        .iter()
        .map(|&(key, index)| {
            if key {
                KEY_PARTS[index % KEY_PARTS.len()]
            } else {
                TEXT_PARTS[index]
            }
        })
        .collect()
}

const KEY_PARTS: [&str; 12] = [
    "pass", "word", "PASS", "WORD", "sec", "ret", "SEC", "RET", "tok", "en", "api_", "key",
];
const TEXT_PARTS: [&str; 34] = [
    ": ", ":", "=", " = ", "\t", " ", "\u{a0}", "\u{3000}", "\"", "'", "\\\"", "\\'", "--", "-",
    "1234", "98765", "Zx9", "kq7!", "a", "_", ".", "9", "\n", "\r\n", "\r", "\u{2028}", "\\n",
    "\\u000a", "\u{17f}", "\u{212a}", "#", "\u{e9}", "{", "}",
];

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(6_000))]

    #[test]
    fn the_line_gated_search_yields_the_captures_of_the_whole_input(
        parts in proptest::collection::vec((proptest::bool::ANY, 0..TEXT_PARTS.len()), 0..24),
    ) {
        assert_gated_search_replays_captures_iter(&generated_text(&parts));
    }

    #[test]
    fn a_scan_through_the_line_gate_reports_what_the_whole_input_scan_reports(
        parts in proptest::collection::vec((proptest::bool::ANY, 0..TEXT_PARTS.len()), 0..24),
        max_candidates in 0usize..8,
    ) {
        let input = generated_text(&parts);
        assert_gated_scan_reports_the_whole_input_scan(&input, ScanLimits::default());
        assert_gated_scan_reports_the_whole_input_scan(
            &input,
            ScanLimits {
                max_candidates,
                ..ScanLimits::default()
            },
        );
    }
}

#[test]
fn a_scan_through_the_line_gate_reports_what_the_whole_input_scan_reports_at_each_limit() {
    assert!(
        WHOLE_INPUT
            .active(ScanProfile::Comprehensive)
            .all(|rule| rule.line_literals.is_none())
    );
    let scan = |profile, input: &str| {
        evaluate(&EMBEDDED, profile, ScanLimits::default(), [0; 32], input).unwrap()
    };
    let pass = ["pass", "word"].concat();
    let secret = ["sec", "ret"].concat();
    let folded = ["pa", "\u{17f}\u{17f}", "word"].concat();
    let kelvin = ["api_", "\u{212a}", "ey"].concat();
    // The option form declines a plain word, so each lead is a candidate without a finding.
    let declined = format!("deploy --{pass} hunter --{secret} plain");
    let report = scan(ScanProfile::Conservative, &declined);
    assert_eq!(report.findings, []);
    assert!(report.candidates_evaluated >= 2, "{report:?}");
    let unterminated = format!("--{pass} \"{}", "x".repeat(MAX_MATCH_BYTES + 8));
    assert_eq!(
        scan(ScanProfile::Conservative, &unterminated).limits_hit,
        Some(LimitExhausted::Match)
    );
    for input in [
        declined,
        unterminated,
        format!("deploy --{pass} Zx9kq7 --token Qw8rt2Lm"),
        format!("{pass}: Zx9kq7\n{secret}: {}\napi_key: Qw8rt2Lm5\n", 1234),
        format!("\"{pass}\": \"{}\", {secret}={}", 1234, 5678),
        format!("{folded}: {}\n--{kelvin} Zx9kq7", 1234),
    ] {
        let complete = scan(ScanProfile::Comprehensive, &input);
        for max_candidates in 0..=complete.candidates_evaluated + 1 {
            assert_gated_scan_reports_the_whole_input_scan(
                &input,
                ScanLimits {
                    max_candidates,
                    ..ScanLimits::default()
                },
            );
        }
        let step = if complete.work_bytes <= 8192 {
            1
        } else {
            complete.work_bytes / 48
        };
        for max_work_bytes in (0..=complete.work_bytes + 1).step_by(step) {
            assert_gated_scan_reports_the_whole_input_scan(
                &input,
                ScanLimits {
                    max_work_bytes,
                    ..ScanLimits::default()
                },
            );
        }
    }
}

#[test]
fn the_line_gated_search_yields_the_captures_of_the_whole_input_at_line_edges() {
    let pass = ["pass", "word"].concat();
    let secret = ["sec", "ret"].concat();
    let folded = ["pa", "\u{17f}\u{17f}", "word"].concat();
    let kelvin = ["api_", "\u{212a}", "ey"].concat();
    let long = "x".repeat(70_000);
    for input in [
        String::new(),
        "\n\n".to_owned(),
        format!("{pass}={}{secret}={}", 1234, 5678),
        format!("{pass}: {}\n{secret}: {}", 1234, 5678),
        format!("{pass}\n: {}", 1234),
        format!("{pass}:\n {}", 1234),
        format!("a{pass}: {}\nb{secret}: {}\n", 1234, 5678),
        format!("{pass}: {}\r\n--{secret} {}\r\n", 1234, "Zx9kq7"),
        format!("{folded}: {}\n{kelvin}: {}", 1234, "Zx9kq7"),
        format!("{long}{pass}: {}\n{long}", 1234),
        format!("{long}\n{pass}\n{long}\n{pass}={}", 1234),
        format!("--{pass} --{secret} {}", "Zx9kq7"),
        format!("{pass}{pass}{pass}: {}", 1234),
        format!("\\n{pass}: {}\\n{secret}: {}", 1234, 5678),
        format!("\"{pass}\": \"{}\"\n'{secret}': {}", 1234, 5678),
        format!("{pass}: {}\n", 1234).repeat(3_000),
        format!("{pass}\n{}\n", "x".repeat(700)).repeat(400) + &format!("{secret}={}", 5678),
    ] {
        assert_gated_search_replays_captures_iter(&input);
    }
    // The literal after the filler line starts `GATED_LINE_GAP - 1` to `GATED_LINE_GAP + 2`
    // bytes after the first line's end.
    for filler in GATED_LINE_GAP - 3..=GATED_LINE_GAP {
        let input = format!(
            "{pass}: {}\n{}\n{secret}: {}",
            1234,
            "x".repeat(filler),
            5678
        );
        assert_gated_search_replays_captures_iter(&input);
    }
}

#[test]
fn literal_starts_report_every_start_under_ascii_case_folding() {
    let mut starts = LiteralStarts::new("AaAa \u{17f}x KEY key".as_bytes());
    assert_eq!(starts.starts("aa"), [0, 1, 2]);
    assert_eq!(starts.starts("\u{17f}"), [5]);
    assert_eq!(starts.starts("key"), [9, 13]);
    assert_eq!(starts.first(&["key", "aa"], 0), Some(0));
    assert_eq!(starts.first(&["key", "aa"], 3), Some(9));
    assert_eq!(starts.first(&["key", "\u{17f}"], 3), Some(5));
    assert_eq!(starts.first(&["key"], 14), None);
}

fn windows(input: &str, literals: &[&'static str]) -> Vec<(usize, usize)> {
    let mut starts = LiteralStarts::new(input.as_bytes());
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(window) = gated_window(&mut starts, literals, from) {
        found.push(window);
        from = window.1;
    }
    found
}

#[test]
fn a_gated_window_runs_through_the_lines_of_nearby_literals() {
    let pass = ["pass", "word"].concat();
    let dense = format!("{pass}\n").repeat(3_000);
    assert_eq!(windows(&dense, &["password"]), [(0, dense.len())]);

    let line = format!("{pass}\n{}\n", "x".repeat(GATED_LINE_GAP));
    let sparse = line.repeat(3);
    let starts: Vec<usize> = (0..3).map(|index| index * line.len()).collect();
    assert_eq!(
        windows(&sparse, &["password"]),
        starts
            .iter()
            .map(|start| (*start, start + pass.len()))
            .collect::<Vec<_>>()
    );

    // The literals share a window when the `filler + 2` bytes from the first line feed to the
    // second literal fit within `GATED_LINE_GAP`.
    for (filler, merged) in [
        (GATED_LINE_GAP - 3, true),
        (GATED_LINE_GAP - 2, true),
        (GATED_LINE_GAP - 1, false),
    ] {
        let input = format!("{pass}\n{}\n{pass}", "x".repeat(filler));
        let expected = if merged {
            vec![(0, input.len())]
        } else {
            vec![(0, pass.len()), (input.len() - pass.len(), input.len())]
        };
        assert_eq!(windows(&input, &["password"]), expected, "filler {filler}");
    }
}
