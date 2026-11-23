//! Differential test of the embedded overlay against the overlay of `main` at the merge base.
//!
//! The durable-text redactor replaces or refuses what the scanner reports, so a span the
//! base overlay reports and the embedded overlay leaves out is text that reaches storage.
//! Each row holds one input, the values `main`'s build reported for it, and the exemption
//! the row demonstrates when the embedded overlay leaves that span as written.

use super::{Abort, evaluate, evaluate_candidate};
use crate::rules::{RuleSet, UPSTREAM_BYTES};
use crate::{ScanLimits, ScanProfile, TextSpan, UPSTREAM_CORPUS_SHA256};

/// `conservative_overlay.yaml` at the merge base with `main`, byte for byte.
const BASE_OVERLAY: &[u8] = include_bytes!("../../testdata/conservative_overlay_base.yaml");
/// The digest `main` pins for that document in `CONSERVATIVE_OVERLAY_SHA256`.
const BASE_OVERLAY_SHA256: &str =
    "973181a0af049fb4c0ae06160cd022b1beae3660b87ac9fa4d498864912b3487";

/// A class of span that `main` reports and the embedded overlay leaves as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Exemption {
    /// The value begins with `=`, so the operator is `==` or `===`: a comparison, a version
    /// pin, or a rule of `=` signs. `main` reported the operator and left a quoted literal
    /// after it in place.
    Comparison,
    /// Blanks and another word follow a value detached from its `=`, so the value position
    /// holds the first word of other text (`KEY= command argument`).
    AnotherWordFollows,
    /// The word detached from `=` is an assignment of its own, a comment, a bracket, a
    /// section header, or a rule of punctuation.
    StructuralWord,
    /// The span of a detached value ends where serialized text writes the line break; `main`
    /// ran on through the escape into the first word of the next line.
    EscapedLineBreak,
    /// The text before that escaped line break is judged as at a real line break, where a
    /// number or a member path is left as written.
    HeadBeforeEscapedLineBreak,
    /// A key that follows an escaped line break carries the public-key marker.
    PublicKeyAfterEscape,
}

struct Row {
    input: String,
    /// The byte ranges of the values `main`'s build reports for `input` under the
    /// conservative profile, each with the exemption it demonstrates.
    main: Vec<(usize, usize, Option<Exemption>)>,
}

/// Fills a template: `{c}` is a password a person chose, `{i}` an issued credential, `{p}`
/// a Base64 value that ends in padding, `{m}` a mixed-case value with one `=` of padding,
/// and `{w}` a password's opening word that holds `=`. The values are joined at run time,
/// so each line of this file stays clear of the credential shapes the repository's secret
/// scanners reject.
fn fill(template: &str) -> String {
    template
        .replace("{w}", &["p@", "ss="].concat())
        .replace("{m}", &["Xy7Kp2", "Lm9Qa="].concat())
        .replace("{c}", &["hunter", "-two"].concat())
        .replace("{i}", &["Zq7Lm2Xv9Rt4", "Kp8Wn3Bc6Yd1"].concat())
        .replace("{p}", &["QUJDRDEyMzQ1", "Ng=="].concat())
}

fn row(input: &str, main: &[(usize, usize, Option<Exemption>)]) -> Row {
    Row {
        input: fill(input),
        main: main.to_vec(),
    }
}

/// The ranges are those of the filled input, recorded from `main`'s build at the merge base.
fn rows() -> Vec<Row> {
    vec![
        row("password={c}", &[(9, 19, None)]),
        row("api_key = {i}", &[(10, 34, None)]),
        row("auth_token\t=\t{c}", &[(13, 23, None)]),
        row("DB_PASSWORD\n={c} now", &[(13, 23, None)]),
        row("export SECRET_KEY={i} && run", &[(18, 42, None)]),
        row("token={c};tail9", &[(6, 16, None)]),
        row("Server=a;Password={c};Database=d", &[(18, 28, None)]),
        row("password={c}&x9", &[(9, 19, None)]),
        row("password={c}$x9", &[(9, 19, None)]),
        row("connect(password={c})", &[(17, 27, None)]),
        row("password=\"{c} two\"", &[(10, 24, None)]),
        row("password='{c}'", &[(10, 20, None)]),
        row("password = \"{c}\"", &[(12, 22, None)]),
        row("password =\n  \"{c}\"", &[(14, 24, None)]),
        row("{\"password\":\"{c}\"}", &[(13, 23, None)]),
        row("{\"clientSecret\": \"{i}\"}", &[(18, 42, None)]),
        row("'api_token': '{i}'", &[(14, 38, None)]),
        row("\"auth_key\": '{i}'", &[(13, 37, None)]),
        row("'auth_key': \"{i}\"", &[(13, 37, None)]),
        row("Authorization: Bearer {i}", &[(22, 46, None)]),
        row("DB_PASSWORD= {c}", &[(13, 23, None)]),
        row("DB_PASSWORD=\n{c}\nnext line", &[(13, 23, None)]),
        row("DB_PASSWORD=\r\n  {c}\r\n", &[(16, 26, None)]),
        row("DB_PASSWORD =\n{c}", &[(14, 24, None)]),
        row("DB_PASSWORD\n=\n{c}\n", &[(14, 24, None)]),
        row("msg=\"login password= {c}\" user=bob", &[(21, 31, None)]),
        row("(password= {c})", &[(11, 21, None)]),
        row("password= {c}; echo done", &[(10, 20, None)]),
        row("password= {c};", &[(10, 20, None)]),
        row("password= {c} # rotate", &[(10, 20, None)]),
        row("password=\n{c}&x9", &[(10, 20, None)]),
        row("password=\n{c}|cat", &[(10, 20, None)]),
        row("password= {c},", &[(10, 21, None)]),
        row("token= {c} \\\n  --flag", &[(7, 17, None)]),
        row("api_key=\n{p}\n", &[(9, 25, None)]),
        row("api_key= {p}", &[(9, 25, None)]),
        row("password= {w}{c}", &[(10, 25, None)]),
        row("API_TOKEN=\n{c}>", &[(11, 21, None)]),
        row("password=\t{c}\t\n", &[(10, 20, None)]),
        row(
            "API_TOKEN=\nDB_PASSWORD= {c}",
            &[(11, 23, Some(Exemption::StructuralWord))],
        ),
        row(
            "db.password =\nkeycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[(72, 82, None)],
        ),
        row(
            "API_TOKEN=\nself.token = api_token = {c}",
            &[(36, 46, None)],
        ),
        row("(password= {c} )", &[(11, 21, None)]),
        row("msg=\"password= {c} \"", &[(15, 25, None)]),
        row("password= {c} ;", &[(10, 20, None)]),
        row("password= {c} | cat", &[(10, 20, None)]),
        row(
            "API_TOKEN=\nDB_PASSWORD=\ndb.password=\n{c}",
            &[(11, 23, Some(Exemption::StructuralWord)), (37, 47, None)],
        ),
        row("x\\tpublic_key={i}", &[(14, 38, None)]),
        row("C:\\\\npublic_key={i}", &[(16, 40, None)]),
        row("API_TOKEN=\n{m};tail9", &[(11, 23, None)]),
        row("password={c};tail9;", &[(9, 19, None)]),
        row(
            "if (e.key === \"Enter\") {}",
            &[(11, 13, Some(Exemption::Comparison))],
        ),
        row("token == other1", &[(7, 8, Some(Exemption::Comparison))]),
        row(
            "google-auth==2.49.1",
            &[(12, 19, Some(Exemption::Comparison))],
        ),
        row("password=={c}", &[(9, 20, Some(Exemption::Comparison))]),
        row(
            "api_token= {c} --verbose",
            &[(11, 21, Some(Exemption::AnotherWordFollows))],
        ),
        row(
            "api_token=\n{c} and more words",
            &[(11, 21, Some(Exemption::AnotherWordFollows))],
        ),
        row(
            "API_KEY= run command",
            &[(9, 12, Some(Exemption::AnotherWordFollows))],
        ),
        row(
            "API_TOKEN=\nDATABASE_URL=postgres://localhost/db",
            &[(11, 47, Some(Exemption::StructuralWord))],
        ),
        row(
            "API_TOKEN=\n#comment7",
            &[(11, 20, Some(Exemption::StructuralWord))],
        ),
        row(
            "API_TOKEN=\n}",
            &[(11, 12, Some(Exemption::StructuralWord))],
        ),
        row(
            "API_TOKEN=\n[section]",
            &[(11, 20, Some(Exemption::StructuralWord))],
        ),
        row(
            "API_TOKEN=\n---",
            &[(11, 14, Some(Exemption::StructuralWord))],
        ),
        row(
            "API_TOKEN=\nURL=",
            &[(11, 15, Some(Exemption::StructuralWord))],
        ),
        row(
            "out\\napi_token= {i}\\nnext line",
            &[(16, 46, Some(Exemption::EscapedLineBreak))],
        ),
        row(
            "api_token= 4096\\nnext",
            &[(11, 21, Some(Exemption::HeadBeforeEscapedLineBreak))],
        ),
        row(
            "password= cfg.value\\nnext",
            &[(10, 25, Some(Exemption::HeadBeforeEscapedLineBreak))],
        ),
        row(
            "x\\npublic_key={i}",
            &[(14, 38, Some(Exemption::PublicKeyAfterEscape))],
        ),
    ]
}

/// Value spans the base overlay reports for `input`, each rule run through its pattern.
fn base_overlay_spans(base: &RuleSet, input: &str) -> Vec<TextSpan> {
    let mut spans = Vec::new();
    for rule in base.active(ScanProfile::Conservative) {
        for captures in rule.regex.captures_iter(input.as_bytes()) {
            let mut work = 0;
            match evaluate_candidate(
                base,
                rule,
                &captures,
                input,
                &mut work,
                ScanLimits::default(),
            ) {
                Ok(Some(finding)) => spans.push(finding.value_span),
                Ok(None) => {}
                Err(Abort::Work | Abort::Match | Abort::Invalid(_)) => {
                    panic!("the base overlay stopped at a limit on {input:?}")
                }
            }
        }
    }
    spans
}

/// Value spans the embedded overlay reports for `input` through the scanner's entry point.
fn embedded_overlay_spans(embedded: &RuleSet, input: &str) -> Vec<TextSpan> {
    let limits = ScanLimits::default();
    let digest = embedded
        .semantic_digest(ScanProfile::Conservative, limits)
        .unwrap();
    let report = evaluate(embedded, ScanProfile::Conservative, limits, digest, input).unwrap();
    assert!(report.is_complete(), "{input:?}");
    report
        .findings
        .iter()
        .map(|finding| finding.value_span)
        .collect()
}

/// Where the union of `spans` first leaves a byte of `start..end` out.
fn first_uncovered(spans: &[TextSpan], start: usize, end: usize) -> Option<usize> {
    let mut at = start;
    let mut sorted = spans.to_vec();
    sorted.sort_by_key(|span| span.start());
    for span in sorted {
        if span.start() <= at && at < span.end() {
            at = span.end();
        }
    }
    (at < end).then_some(at)
}

/// Whether a span of `spans` shares a byte with `start..end`.
fn overlaps(spans: &[TextSpan], start: usize, end: usize) -> bool {
    spans
        .iter()
        .any(|span| span.start() < end && start < span.end())
}

#[test]
fn the_embedded_overlay_reports_every_span_the_base_overlay_reports_apart_from_named_exemptions() {
    let base = RuleSet::from_sources(
        UPSTREAM_BYTES,
        UPSTREAM_CORPUS_SHA256,
        BASE_OVERLAY,
        BASE_OVERLAY_SHA256,
    )
    .unwrap();
    let embedded = RuleSet::from_embedded().unwrap();
    let mut exemptions_shown = Vec::new();
    for Row { input, main } in rows() {
        assert!(!main.is_empty(), "{input:?} records no span of main");
        let base_spans = base_overlay_spans(&base, &input);
        let embedded_spans = embedded_overlay_spans(&embedded, &input);
        for &(start, end, exemption) in &main {
            // The base document reports the recorded range under this evaluator too. The
            // key gate both documents share reads the public marker after an escape, so
            // for that class the recorded range alone stands for `main`.
            if exemption != Some(Exemption::PublicKeyAfterEscape) {
                assert_eq!(
                    first_uncovered(&base_spans, start, end),
                    None,
                    "the base overlay leaves part of {:?} in {input:?}",
                    &input[start..end]
                );
            }
            let Some(exemption) = exemption else {
                assert_eq!(
                    first_uncovered(&embedded_spans, start, end),
                    None,
                    "the embedded overlay leaves part of {:?} in {input:?}",
                    &input[start..end]
                );
                continue;
            };
            exemptions_shown.push(exemption);
            let exempt_from = if exemption == Exemption::EscapedLineBreak {
                // The embedded span covers the value and stops at the escape.
                let escape = start + input[start..end].find('\\').unwrap();
                assert_eq!(
                    first_uncovered(&embedded_spans, start, escape),
                    None,
                    "{input:?}"
                );
                escape
            } else {
                start
            };
            assert!(
                !overlaps(&embedded_spans, exempt_from, end),
                "{exemption:?} leaves {:?} as written in {input:?}",
                &input[exempt_from..end]
            );
        }
        // Every span the base document reports outside the recorded ranges is covered too.
        for span in &base_spans {
            let recorded = main
                .iter()
                .any(|(start, end, _)| *start <= span.start() && span.end() <= *end);
            assert!(
                recorded || first_uncovered(&embedded_spans, span.start(), span.end()).is_none(),
                "the embedded overlay leaves part of {:?} in {input:?}",
                &input[span.start()..span.end()]
            );
        }
    }
    for exemption in [
        Exemption::Comparison,
        Exemption::AnotherWordFollows,
        Exemption::StructuralWord,
        Exemption::EscapedLineBreak,
        Exemption::HeadBeforeEscapedLineBreak,
        Exemption::PublicKeyAfterEscape,
    ] {
        assert!(exemptions_shown.contains(&exemption), "{exemption:?}");
    }
}
