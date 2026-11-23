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
    /// A key that follows an escaped line break carries the public-key marker.
    PublicKeyAfterEscape,
}

struct Row {
    input: String,
    /// The values `main`'s build reports for `input` under the conservative profile.
    main: Vec<String>,
    exemption: Option<Exemption>,
}

/// Fills a template: `{c}` is a password a person chose, `{i}` an issued credential, `{p}`
/// a Base64 value that ends in padding, and `{w}` a password's opening word that holds `=`.
/// The values are joined at run time, so each line of this file stays clear of the credential
/// shapes the repository's secret scanners reject.
fn fill(template: &str) -> String {
    template
        .replace("{w}", &["p@", "ss="].concat())
        .replace("{c}", &["hunter", "-two"].concat())
        .replace("{i}", &["Zq7Lm2Xv9Rt4", "Kp8Wn3Bc6Yd1"].concat())
        .replace("{p}", &["QUJDRDEyMzQ1", "Ng=="].concat())
}

fn row(input: &str, main: &[&str], exemption: Option<Exemption>) -> Row {
    Row {
        input: fill(input),
        main: main.iter().map(|value| fill(value)).collect(),
        exemption,
    }
}

fn rows() -> Vec<Row> {
    vec![
        row("password={c}", &["{c}"], None),
        row("api_key = {i}", &["{i}"], None),
        row("auth_token\t=\t{c}", &["{c}"], None),
        row("DB_PASSWORD\n={c} now", &["{c}"], None),
        row("export SECRET_KEY={i} && run", &["{i}"], None),
        row("token={c};tail9", &["{c}"], None),
        row("Server=a;Password={c};Database=d", &["{c}"], None),
        row("password={c}&x9", &["{c}"], None),
        row("password={c}$x9", &["{c}"], None),
        row("connect(password={c})", &["{c}"], None),
        row("password=\"{c} two\"", &["{c} two"], None),
        row("password='{c}'", &["{c}"], None),
        row("password = \"{c}\"", &["{c}"], None),
        row("password =\n  \"{c}\"", &["{c}"], None),
        row("{\"password\":\"{c}\"}", &["{c}"], None),
        row("{\"clientSecret\": \"{i}\"}", &["{i}"], None),
        row("'api_token': '{i}'", &["{i}"], None),
        row("\"auth_key\": '{i}'", &["{i}"], None),
        row("'auth_key': \"{i}\"", &["{i}"], None),
        row("Authorization: Bearer {i}", &["{i}"], None),
        row("DB_PASSWORD= {c}", &["{c}"], None),
        row("DB_PASSWORD=\n{c}\nnext line", &["{c}"], None),
        row("DB_PASSWORD=\r\n  {c}\r\n", &["{c}"], None),
        row("DB_PASSWORD =\n{c}", &["{c}"], None),
        row("DB_PASSWORD\n=\n{c}\n", &["{c}"], None),
        row("msg=\"login password= {c}\" user=bob", &["{c}"], None),
        row("(password= {c})", &["{c}"], None),
        row("password= {c}; echo done", &["{c}"], None),
        row("password= {c};", &["{c}"], None),
        row("password= {c} # rotate", &["{c}"], None),
        row("password=\n{c}&x9", &["{c}"], None),
        row("password=\n{c}|cat", &["{c}"], None),
        row("password= {c},", &["{c},"], None),
        row("token= {c} \\\n  --flag", &["{c}"], None),
        row("api_key=\n{p}\n", &["{p}"], None),
        row("api_key= {p}", &["{p}"], None),
        row("password= {w}{c}", &["{w}{c}"], None),
        row("API_TOKEN=\n{c}>", &["{c}"], None),
        row("password=\t{c}\t\n", &["{c}"], None),
        row(
            "API_TOKEN=\nDB_PASSWORD= {c}",
            &["DB_PASSWORD="],
            Some(Exemption::StructuralWord),
        ),
        row(
            "if (e.key === \"Enter\") {}",
            &["=="],
            Some(Exemption::Comparison),
        ),
        row("token == other1", &["="], Some(Exemption::Comparison)),
        row(
            "google-auth==2.49.1",
            &["=2.49.1"],
            Some(Exemption::Comparison),
        ),
        row("password=={c}", &["={c}"], Some(Exemption::Comparison)),
        row(
            "api_token= {c} --verbose",
            &["{c}"],
            Some(Exemption::AnotherWordFollows),
        ),
        row(
            "api_token=\n{c} and more words",
            &["{c}"],
            Some(Exemption::AnotherWordFollows),
        ),
        row(
            "API_KEY= run command",
            &["run"],
            Some(Exemption::AnotherWordFollows),
        ),
        row(
            "API_TOKEN=\nDATABASE_URL=postgres://localhost/db",
            &["DATABASE_URL=postgres://localhost/db"],
            Some(Exemption::StructuralWord),
        ),
        row(
            "API_TOKEN=\n#comment7",
            &["#comment7"],
            Some(Exemption::StructuralWord),
        ),
        row("API_TOKEN=\n}", &["}"], Some(Exemption::StructuralWord)),
        row(
            "API_TOKEN=\n[section]",
            &["[section]"],
            Some(Exemption::StructuralWord),
        ),
        row("API_TOKEN=\n---", &["---"], Some(Exemption::StructuralWord)),
        row(
            "API_TOKEN=\nURL=",
            &["URL="],
            Some(Exemption::StructuralWord),
        ),
        row(
            "out\\napi_token= {i}\\nnext line",
            &["{i}\\nnext"],
            Some(Exemption::EscapedLineBreak),
        ),
        row(
            "x\\npublic_key={i}",
            &["{i}"],
            Some(Exemption::PublicKeyAfterEscape),
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
    for Row {
        input,
        main,
        exemption,
    } in rows()
    {
        let recorded: Vec<(usize, usize)> = main
            .iter()
            .map(|value| {
                let start = input
                    .find(value.as_str())
                    .unwrap_or_else(|| panic!("{input:?} holds no {value:?}"));
                (start, start + value.len())
            })
            .collect();
        assert!(!recorded.is_empty(), "{input:?} records no span of main");
        let base_spans = base_overlay_spans(&base, &input);
        let embedded_spans = embedded_overlay_spans(&embedded, &input);
        let Some(exemption) = exemption else {
            // The base document under this evaluator reports at least what `main`'s build
            // recorded, and the embedded document covers each of those spans.
            for (start, end) in &recorded {
                assert_eq!(
                    first_uncovered(&base_spans, *start, *end),
                    None,
                    "{input:?}"
                );
            }
            for span in &base_spans {
                assert_eq!(
                    first_uncovered(&embedded_spans, span.start(), span.end()),
                    None,
                    "the embedded overlay leaves part of {:?} in {input:?}",
                    &input[span.start()..span.end()]
                );
            }
            continue;
        };
        exemptions_shown.push(exemption);
        for (start, end) in recorded {
            let uncovered = first_uncovered(&embedded_spans, start, end);
            if exemption == Exemption::EscapedLineBreak {
                // The embedded span covers the value and stops at the escape.
                let escape = start + input[start..end].find('\\').unwrap();
                assert_eq!(uncovered, Some(escape), "{input:?}");
            } else {
                assert_eq!(uncovered, Some(start), "{input:?}");
            }
        }
    }
    for exemption in [
        Exemption::Comparison,
        Exemption::AnotherWordFollows,
        Exemption::StructuralWord,
        Exemption::EscapedLineBreak,
        Exemption::PublicKeyAfterEscape,
    ] {
        assert!(exemptions_shown.contains(&exemption), "{exemption:?}");
    }
}
