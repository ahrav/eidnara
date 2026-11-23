use std::sync::LazyLock;

use super::*;
use crate::rules::{RuleDeclaration, compile_rule};

/// The fixtures assemble credential-shaped values at run time so each source literal contains only a fragment.
fn chosen() -> String {
    ["Zx9", "-kq7"].concat()
}

fn issued() -> String {
    ["Zq7Lm2Xv9Rt4", "Kp8Wn3Bc6Yd1"].concat()
}

fn bare(text: &str) -> ValueText<'_> {
    ValueText {
        text: text.as_bytes(),
        quote: Quote::Bare,
        closed_by_punctuation: false,
    }
}

fn literal(text: &str) -> ValueText<'_> {
    ValueText {
        quote: Quote::Literal,
        ..bare(text)
    }
}

#[test]
fn the_text_of_a_word_is_the_word_without_what_surrounds_the_value() {
    for (word, expected) in [
        ("Zx9kq7", bare("Zx9kq7")),
        ("\"Zx9 kq7\"", literal("Zx9 kq7")),
        ("'Zx9 kq7'", literal("Zx9 kq7")),
        ("\\\"Zx9 kq7\\\"", literal("Zx9 kq7")),
        (
            "`cat f`",
            ValueText {
                quote: Quote::Command,
                ..bare("cat f")
            },
        ),
        ("'Zx9kq7',", literal("Zx9kq7")),
        ("\"$DB_PASSWORD\")", literal("$DB_PASSWORD")),
        ("\\\"Zx9kq7\\\"}", literal("Zx9kq7")),
        ("Zx9kq7\"}", bare("Zx9kq7")),
        ("Zx9!kq7\",\"example\":true}", bare("Zx9!kq7")),
        ("Zx9!kq7','x':1}", bare("Zx9!kq7")),
        ("Zx9!kq7\\\",\\\"cwd\\\":\\\"${x}\\\"}", bare("Zx9!kq7")),
        ("\"Zx9 kq7\",\"example\":true}", literal("Zx9 kq7")),
        ("'it\\'s 9','x':1}", literal("it\\'s 9")),
        ("Zx9kq7\\\"", bare("Zx9kq7")),
        ("Zx9kq7'", bare("Zx9kq7")),
        (
            "u64,",
            ValueText {
                closed_by_punctuation: true,
                ..bare("u64")
            },
        ),
        (
            "value);",
            ValueText {
                closed_by_punctuation: true,
                ..bare("value")
            },
        ),
        ("Zx9kq7\\nuser:", bare("Zx9kq7")),
        ("Zx9kq7\\r\\nuser:", bare("Zx9kq7")),
        ("\"Zx9kq7\\nmore\"", literal("Zx9kq7\\nmore")),
        ("\"hunter\\n9x!\" next", literal("hunter\\n9x!")),
        ("Ab\\\\\\\",Zx9!", bare("Ab\\\\\\\",Zx9!")),
        ("ab\",cd9", bare("ab\",cd9")),
        ("\"true\"\\nuser=\"bob\"", literal("true")),
        ("Zx9kq7\\\\nuser:", bare("Zx9kq7\\\\nuser:")),
        ("Zx9kq7\\u000Auser:", bare("Zx9kq7")),
        ("\\\"Zx9kq7\\\",\\n\\t};", literal("Zx9kq7")),
        ("\"$DB_PASSWORD", bare("$DB_PASSWORD")),
        ("\\\"Zx9", bare("Zx9")),
        ("'Zx9", bare("Zx9")),
        ("Pa55'!!", bare("Pa55'!!")),
        ("Pa55\\word9", bare("Pa55\\word9")),
        ("Q7r9$\\+v9", bare("Q7r9$\\+v9")),
        ("\"", bare("")),
        ("", bare("")),
    ] {
        assert_eq!(value_text(word.as_bytes()), expected, "{word:?}");
    }
}

#[test]
fn placeholder_shapes_are_closed() {
    for text in [
        "PASSWORD",
        "string",
        "value",
        "my-password",
        "API_KEY]",
        "value.",
        "two words",
        "\u{2581}our",
        "\u{9700}\u{8981}\u{5bc6}\u{7801}\u{ff0c}\u{8bf7}",
        "p\u{e4}ssw\u{f6}rd",
        "****",
        "gho_************",
        "...",
        "$DB_PASSWORD",
        "$(cat",
        "os.environ/OPENAI_API_KEY",
        "env:DB_PASSWORD",
        "GITHUB_TOKEN",
        "%DB_PASSWORD%",
        "%AppData%",
        "<password>",
        "<file>|<text>`:",
        "{password}",
        "{{ vault_password }}",
        "[REDACTED]",
        "%s",
        "~/secrets/token7.txt",
        "./token7",
        "../token7",
        "/path/to/secrets7.txt",
        "https://example.org/settings/tokens7",
        "1433;database=sales",
        "1433;database=sales;encrypt=true;",
        "<a{b}>",
        "id=npm7,src=.npmrc",
        "",
    ] {
        assert!(is_placeholder_shape(text.as_bytes()), "{text:?}");
    }
    for text in [
        "hunter2",
        "Summer!",
        "Strings!",
        "12345678",
        "12.05.1987",
        "8080:80",
        "%40abc",
        "%7qW9%",
        "p@ss",
        "S3cret",
        "a.b9",
        "x*y*z9",
        "ab**c1",
        "a$b1",
        "%Tgb5",
        "Pa55'!!",
        "Pa55\\word9",
        "Q7r9$\\+v9",
        "a<b9",
        "x{9",
        "/aB3dEf/Gh1jKl+mN",
        "QxWvErTy4=",
        "dXNlcg==",
        "os.environ/BUILD_ID7",
        "abcDEF_TOKEN7",
        "x9GITHUB_TOKEN",
        "/run/secrets/db7",
        "ID=npm7",
        "\u{43f}\u{430}\u{440}\u{43e}\u{43b}\u{44c}7",
        "p\u{e4}ss\u{20ac}w\u{f6}rd9",
        "Summer\u{a3}",
        "Geheim\u{20ac}\u{a7}",
        "<a>Zx9!kq7",
        "[Zx9]!kq7",
        "12;ab!9",
        "1433;database=sales;Zx9!kq7",
        "1433;",
        "[Zx9!>",
        "{Zx9!kq7]",
        "<a[b>]",
        "<Zx9!kq7>",
        "{a:b}",
        "id=npm7,Zx9!kq7",
    ] {
        assert!(!is_placeholder_shape(text.as_bytes()), "{text:?}");
    }
    assert!(!is_reference_shape(b"postgres"));
    assert!(is_reference_shape(b"$PGPASS"));
}

#[test]
fn a_pin_is_a_value_under_a_key_that_ends_in_a_memorized_secret_word() {
    for (key, memorized) in [
        ("password", true),
        ("db_password", true),
        ("PGPASSWORD", true),
        ("client-secret", true),
        ("clientSecret", true),
        ("token", false),
        ("api_key", false),
        ("password_length", false),
        ("secret_version", false),
        ("max-tokens", false),
    ] {
        assert_eq!(
            ends_in_memorized_secret_word(key.as_bytes()),
            memorized,
            "{key}"
        );
    }
    for (text, memorized, count) in [
        ("1234", true, false),
        ("12345678", true, false),
        ("123", true, true),
        ("1234", false, true),
        ("1234.5", true, true),
        ("true", true, true),
        ("null", true, true),
        ("900s", true, true),
        ("0x1f", true, true),
        ("12a4", true, false),
    ] {
        assert_eq!(is_count(text.as_bytes(), memorized), count, "{text}");
    }
}

#[test]
fn each_form_admits_a_credential_and_leaves_a_stand_in() {
    use CredentialForm::{
        ClientPassword, Digits, FieldValue, OptionValue, PasswordAssignment, UrlPassword,
    };
    const PW: Option<&str> = Some("password");
    const TK: Option<&str> = Some("token");
    const SC: Option<&str> = Some("secret");
    const AK: Option<&str> = Some("api_key");
    const CAMEL: Option<&str> = Some("apiKey");
    const NO: Option<&str> = Some("no-password");
    let chosen = chosen();
    let quoted_reference = "\"$DB_PASSWORD\"";
    for (form, key, word, admitted) in [
        (OptionValue, PW, chosen.as_str(), true),
        (OptionValue, PW, "Pa55'!!", true),
        (OptionValue, PW, "Q7r9$\\+v9", true),
        (OptionValue, PW, "\"Zx9 kq7\"", true),
        (OptionValue, PW, "123456", true),
        (OptionValue, TK, "123456", false),
        (OptionValue, PW, "PASSWORD", false),
        (OptionValue, PW, quoted_reference, false),
        (OptionValue, PW, "`cat", false),
        (OptionValue, PW, "`pass9`", false),
        (OptionValue, TK, "config.token", false),
        (OptionValue, TK, "\"river.blue7\"", true),
        (OptionValue, NO, chosen.as_str(), false),
        (OptionValue, SC, "id=npm,src=.npmrc", false),
        (FieldValue, PW, chosen.as_str(), true),
        (FieldValue, PW, "Pa55'!!", true),
        (FieldValue, PW, "Pa55\\word9", true),
        (FieldValue, CAMEL, "'Zx9kq7abc',", true),
        (FieldValue, PW, "123456", true),
        (FieldValue, TK, "123456", false),
        (FieldValue, TK, "u64,", false),
        (FieldValue, PW, "hunter9,", false),
        (FieldValue, AK, "Option<String>", false),
        (FieldValue, PW, "&str", false),
        (FieldValue, TK, "*anchor9", false),
        (FieldValue, SC, "String!", false),
        (FieldValue, SC, "Strings9!", true),
        (FieldValue, PW, "getpass9()", false),
        (FieldValue, PW, "\"getpass9()\"", true),
        (FieldValue, PW, "string", false),
        (FieldValue, TK, "https://example.org/tokens", false),
        (PasswordAssignment, None, chosen.as_str(), true),
        (PasswordAssignment, None, "postgres", true),
        (PasswordAssignment, None, "123456", true),
        (PasswordAssignment, None, "123", false),
        (PasswordAssignment, None, "true", false),
        (PasswordAssignment, None, "$PGPASS", false),
        (PasswordAssignment, None, "\"${PGPASS}\"", false),
        (PasswordAssignment, None, "<password>", false),
        (PasswordAssignment, None, "settings.password", false),
        (ClientPassword, None, chosen.as_str(), true),
        (ClientPassword, None, "9234", true),
        (ClientPassword, None, "8080:80", false),
        (ClientPassword, None, "127.0.0.1:5432:5432", false),
        (ClientPassword, None, "root", false),
        (ClientPassword, None, "$MYSQL_PWD", false),
        (UrlPassword, None, chosen.as_str(), true),
        (UrlPassword, None, "12345", true),
        (UrlPassword, None, "password", false),
        (UrlPassword, None, "%DB_PASSWORD%", false),
        (Digits, PW, "12345678", true),
        (Digits, PW, "1234.5", false),
        (Digits, PW, "1234-abcd", false),
    ] {
        let text = form.admitted_text(key.map(str::as_bytes), word.as_bytes());
        assert_eq!(text.is_some(), admitted, "{form:?} {key:?} {word:?}");
        if let Some(text) = text {
            let word = word.as_bytes().as_ptr_range();
            let text = text.as_ptr_range();
            assert!(word.start <= text.start && text.end <= word.end);
        }
    }
}

#[test]
fn basic_credentials_decode_to_a_user_and_a_password() {
    for (value, valid) in [
        (concat!("dXNlcjpaeDlr", "cTc="), true),
        (concat!("dXNlcjpaeDlr", "cTc"), true),
        (concat!("YWxpY2U6cOTf", "d/ZyZA=="), true),
        (concat!("dXNlcjpaeDlr", "cTcK"), true),
        ("YTo_Pz8-", true),
        ("authentication", false),
        ("dXNlcg==", false),
        ("dXNlcjoBAg==", false),
        ("dXNlc", false),
        ("dXN*cjpa", false),
    ] {
        assert_eq!(
            matches!(
                validate_basic_credentials(value.as_bytes()),
                OfflineVerdict::Valid
            ),
            valid,
            "{value}"
        );
    }
}

#[test]
fn credential_form_constants_are_pinned() {
    let mut encoded = Vec::new();
    for word in MEMORIZED_SECRET_WORDS {
        encoded.extend_from_slice(word);
        encoded.push(0);
    }
    encoded.push(0xff);
    for name in NON_NULL_SCALAR_TYPES {
        encoded.extend_from_slice(name.as_bytes());
        encoded.push(0);
    }
    encoded.push(0xff);
    encoded.extend_from_slice(&(MIN_PIN_DIGITS as u64).to_le_bytes());
    assert_eq!(
        crate::rules::digest_hex(&encoded),
        "703faa87e55afafa53c3f048fd64e11742c135e27e59645c9fc213e710468d1f",
        "credential-form constants changed: bump REVISION.semantic_digest_version and re-pin"
    );
}

fn form_rule(form: &str) -> Rule {
    let declaration: RuleDeclaration = serde_json::from_str(&format!(
        r#"{{"name":"t-form","regex":"go (?P<value>[^ ]+)","anchors":["go"],"radius":16,"value_group":"value","credential_form":"{form}"}}"#
    ))
    .unwrap();
    compile_rule(RuleSource::ConservativeOverlay, declaration).unwrap()
}

#[test]
fn a_form_charges_its_word_to_the_work_budget() {
    let rule = form_rule("url_password");
    let rules = RuleSet::from_embedded().unwrap();
    let word = "a".repeat(4_000);
    let input = format!("go {word}");
    let captures = rule.regex.captures(input.as_bytes()).unwrap();
    let evaluate_with = |max_work_bytes| {
        let mut work = 0usize;
        let outcome = evaluate_candidate(
            &rules,
            &rule,
            &captures,
            &input,
            &mut work,
            ScanLimits {
                max_work_bytes,
                ..ScanLimits::default()
            },
        );
        (outcome, work)
    };
    let (outcome, work) = evaluate_with(word.len());
    assert!(matches!(outcome, Ok(None)), "{outcome:?}");
    assert_eq!(work, word.len());
    let (outcome, _) = evaluate_with(word.len() - 1);
    assert!(matches!(outcome, Err(Abort::Work)), "{outcome:?}");
}

#[test]
fn a_rule_without_a_form_keeps_its_matched_value() {
    let rules = RuleSet::from_embedded().unwrap();
    let declaration: RuleDeclaration = serde_json::from_str(
        r#"{"name":"t-plain","regex":"go (?P<value>[^ ]+)","anchors":["go"],"radius":16,"value_group":"value"}"#,
    )
    .unwrap();
    let plain = compile_rule(RuleSource::ConservativeOverlay, declaration).unwrap();
    for (rule, reported) in [(plain, true), (form_rule("url_password"), false)] {
        let input = "go \"PASSWORD\")";
        let captures = rule.regex.captures(input.as_bytes()).unwrap();
        let mut work = 0usize;
        let finding = evaluate_candidate(
            &rules,
            &rule,
            &captures,
            input,
            &mut work,
            ScanLimits::default(),
        )
        .unwrap();
        assert_eq!(finding.is_some(), reported);
        if let Some(finding) = finding {
            assert_eq!(
                (finding.value_span.start(), finding.value_span.end()),
                (3, input.len())
            );
        }
    }
}

static EMBEDDED: LazyLock<RuleSet> = LazyLock::new(|| RuleSet::from_embedded().unwrap());

const WORD_RULES: [&str; 5] = [
    "magic-value-option",
    "magic-value-field",
    "magic-value-password-assignment",
    "magic-value-client-attached",
    "magic-value-client-option",
];

fn conservative_findings(input: &str) -> Vec<Finding> {
    let limits = ScanLimits::default();
    let digest = EMBEDDED
        .semantic_digest(ScanProfile::Conservative, limits)
        .unwrap();
    let report = evaluate(&EMBEDDED, ScanProfile::Conservative, limits, digest, input).unwrap();
    assert_eq!(report.limits_hit, None, "{input:?}");
    report.findings
}

fn word_findings(input: &str) -> Vec<Finding> {
    conservative_findings(input)
        .into_iter()
        .filter(|finding| WORD_RULES.contains(&finding.rule_id.as_str()))
        .collect()
}

const SEP: &str =
    r"\t\n\x0B\f\r \x{a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}";
const BRK: &str = r"\n\x0B\f\r\x{2028}\x{2029}";
const BLK: &str = r"\t \x{a0}\x{1680}\x{2000}-\x{200a}\x{202f}\x{205f}\x{3000}\x{feff}";

/// `WORD_PATTERNS` includes anchored patterns for quoted sections and for quoted sections
/// followed by a run. Each alternative has one reading, so the leftmost-first match is the
/// word.
static WORD_PATTERNS: LazyLock<(regex::Regex, regex::Regex, regex::Regex)> = LazyLock::new(|| {
    let serialized =
        format!(r#"\\"(?:[^"\\{BRK}]|(?:\\{{4}})*\\{{3}}"|\\+[^"\\{BRK}])*(?:\\{{4}})*\\""#);
    let section = |quote: char| format!(r"{quote}(?:[^{quote}\\{BRK}]|\\[^{BRK}])*{quote}");
    let sections = format!(
        "^(?:{serialized}|{}|{}|{})*",
        section('"'),
        section('\''),
        section('`')
    );
    let word = format!(r"{sections}(?:\\[{BLK}\\]|[^{SEP}])*");
    let run = format!(r"^(?:\\[{BLK}\\]|[^{SEP}])*");
    (
        regex::Regex::new(&sections).unwrap(),
        regex::Regex::new(&word).unwrap(),
        regex::Regex::new(&run).unwrap(),
    )
});

fn read(input: &str) -> Word {
    let mut reader = WordReader {
        input,
        at: 0,
        read: 0,
        allowance: usize::MAX,
    };
    let word = read_word(&mut reader, false).unwrap();
    assert!(reader.read >= word.end, "{input:?}");
    word
}

fn assert_reader_agrees_with_the_word_patterns(input: &str) {
    let word = read(input);
    let (sections, whole, _) = &*WORD_PATTERNS;
    assert_eq!(
        (word.plain_start, word.end),
        (
            sections.find(input).unwrap().end(),
            whole.find(input).unwrap().end()
        ),
        "{input:?}"
    );
}

#[test]
fn the_word_reader_reads_sections_then_the_run_to_the_next_separator() {
    for (input, sections, word) in [
        ("Zx9kq7 next", "", "Zx9kq7"),
        ("\"Zx9 kq7\" next", "\"Zx9 kq7\"", "\"Zx9 kq7\""),
        (
            "'Zx9 kq7'\"a b\"tail next",
            "'Zx9 kq7'\"a b\"",
            "'Zx9 kq7'\"a b\"tail",
        ),
        ("'A9\\' B8' next", "'A9\\' B8'", "'A9\\' B8'"),
        ("`A9\\` B8` next", "`A9\\` B8`", "`A9\\` B8`"),
        ("\"A9\\\\\" B8\" next", "\"A9\\\\\"", "\"A9\\\\\""),
        ("\\\"Zx9 kq7\\\" next", "\\\"Zx9 kq7\\\"", "\\\"Zx9 kq7\\\""),
        (
            "\\\"a\\\\\\\"b c\\\" next",
            "\\\"a\\\\\\\"b c\\\"",
            "\\\"a\\\\\\\"b c\\\"",
        ),
        (
            "\\\"a\\\\\\\\\\\" next",
            "\\\"a\\\\\\\\\\\"",
            "\\\"a\\\\\\\\\\\"",
        ),
        ("\\\"a\\\\\" b\\\" next", "", "\\\"a\\\\\""),
        ("\"Zx9 kq7 next", "", "\"Zx9"),
        ("\"Zx9\nkq7\" next", "", "\"Zx9"),
        ("Zx9\"a b\" next", "", "Zx9\"a"),
        ("Zx9\\ kq7 next", "", "Zx9\\ kq7"),
        ("Zx9\\\\ kq7 next", "", "Zx9\\\\"),
        ("Zx9\\\u{a0}kq7\u{a0}next", "", "Zx9\\\u{a0}kq7"),
        ("Zx9\\\nkq7", "", "Zx9\\"),
        ("Zx9\u{2028}next", "", "Zx9"),
        ("", "", ""),
        (" Zx9", "", ""),
    ] {
        let read = read(input);
        assert_eq!(
            (&input[..read.plain_start], &input[..read.end]),
            (sections, word),
            "{input:?}"
        );
        assert_reader_agrees_with_the_word_patterns(input);
    }
}

#[test]
fn digits_are_reported_when_they_are_the_whole_assigned_word() {
    let digits = ["1234", "5678"].concat();
    let rules = |input: &str| -> Vec<String> {
        conservative_findings(input)
            .into_iter()
            .filter(|finding| finding.rule_id.starts_with("magic-value-digits"))
            .map(|finding| {
                format!(
                    "{} {}",
                    finding.rule_id,
                    &input[finding.value_span.start()..finding.value_span.end()]
                )
            })
            .collect()
    };
    for (template, rule) in [
        ("password={}", "magic-value-digits"),
        ("x\\tpassword={}", "magic-value-digits"),
        ("client_secret: {}", "magic-value-digits"),
        ("{\"password\": {}}", "magic-value-digits"),
        (
            "{\"password\": {}, \"user\": \"bob\"}",
            "magic-value-digits",
        ),
        ("login?user=bob&password={}&next=1", "magic-value-digits"),
        ("connect(password={});", "magic-value-digits"),
        ("password=\"{}\"", "magic-value-digits-quoted"),
        ("{\"password\":\"{}\"}", "magic-value-digits-quoted"),
        ("{\\\"password\\\":\\\"{}\\\"}", "magic-value-digits-quoted"),
    ] {
        let input = template.replace("{}", &digits);
        assert_eq!(rules(&input), [format!("{rule} {digits}")], "{input:?}");
    }
    let four = ["12", "34"].concat();
    assert_eq!(
        rules(&format!("password={four}")),
        [format!("magic-value-digits {four}")]
    );
    assert_eq!(
        rules(&format!("password={}", &four[..3])),
        Vec::<String>::new()
    );
    for template in [
        "x\\Tpassword={}",
        "password: {},Zx9kq7",
        "DB_PASSWORD={}$Zx9kq7",
        "password={};x9",
        "password=\"{} abcd\"",
        "password='{}-abcd'",
        "password=\"{}'",
        "password='{}\"abcd",
        "{\\\"password\\\":\\\"{}\"abcd",
        "password={}\\ abcd",
        "password={}\\abcd",
        "password={}.5",
        "password={}-abcd",
        "password:{}",
        "token={}",
        "password_length={}",
    ] {
        let input = template.replace("{}", &digits);
        assert_eq!(rules(&input), Vec::<String>::new(), "{input:?}");
    }
}

#[test]
fn a_password_assignment_on_the_next_serialized_line_has_its_own_match() {
    let chosen = chosen();
    let values = |input: &str| -> Vec<String> {
        word_findings(input)
            .into_iter()
            .map(|finding| input[finding.value_span.start()..finding.value_span.end()].to_owned())
            .collect()
    };
    for line_break in ["\\n", "\\r\\n", "\\u000a", "\\u000D"] {
        let reference = format!("PGPASSWORD=$PGPASS{line_break}PGPASSWORD={chosen} psql");
        assert_eq!(
            values(&reference),
            std::slice::from_ref(&chosen),
            "{reference:?}"
        );
        let first = ["one", "9!"].concat();
        let both = format!("PGPASSWORD={first}{line_break}PGPASSWORD={chosen} psql");
        assert_eq!(values(&both), [first, chosen.clone()], "{both:?}");
    }
    let literal_pair = format!("PGPASSWORD={chosen}\\\\nine psql");
    assert_eq!(values(&literal_pair), [format!("{chosen}\\\\nine")]);
    let escaped_blank = format!("PGPASSWORD=$PGPASS\\ PGPASSWORD={chosen} psql");
    assert_eq!(values(&escaped_blank), Vec::<String>::new());
    let one_word = format!("PGPASSWORD=$PGPASS;PGPASSWORD={chosen} psql");
    assert_eq!(values(&one_word), Vec::<String>::new());
}

#[test]
fn an_open_section_fails_the_scan_closed_past_the_match_bound_and_opens_a_bare_word_below_it() {
    for end in ["", "\n"] {
        for (length, aborts) in [
            (MAX_MATCH_BYTES - 1, false),
            (MAX_MATCH_BYTES, false),
            (MAX_MATCH_BYTES + 1, true),
        ] {
            let open = format!("\"a {}{end}", "b".repeat(length - 3));
            assert_eq!(open.len(), length + end.len());
            let mut reader = WordReader {
                input: &open,
                at: 0,
                read: 0,
                allowance: usize::MAX,
            };
            match read_word(&mut reader, false) {
                Err(Abort::Match) => assert!(aborts, "{length}{end:?}"),
                Ok(word) => {
                    assert!(!aborts, "{length}{end:?}");
                    assert_eq!((word.start, word.end), (0, 2));
                }
                Err(_) => panic!("{length}{end:?}"),
            }
        }
    }
    let multibyte = format!("\"a {}\u{e9}", "b".repeat(MAX_MATCH_BYTES - 3));
    let mut reader = WordReader {
        input: &multibyte,
        at: 0,
        read: 0,
        allowance: usize::MAX,
    };
    assert!(matches!(read_word(&mut reader, false), Err(Abort::Match)));
}

#[test]
fn a_word_past_the_match_bound_fails_the_scan_closed() {
    let long = "a".repeat(MAX_MATCH_BYTES + 2);
    let mut reader = WordReader {
        input: &long,
        at: 0,
        read: 0,
        allowance: usize::MAX,
    };
    assert!(matches!(read_word(&mut reader, false), Err(Abort::Match)));
    assert!(reader.read <= MAX_MATCH_BYTES + 1);
}

#[test]
fn a_section_still_open_past_the_match_bound_fails_the_scan_closed() {
    let open = format!("\"{}", "a ".repeat(MAX_MATCH_BYTES));
    let mut reader = WordReader {
        input: &open,
        at: 0,
        read: 0,
        allowance: usize::MAX,
    };
    assert!(matches!(read_word(&mut reader, false), Err(Abort::Match)));
    let closed = format!("\"{}\"", "a ".repeat(MAX_MATCH_BYTES / 2 - 8));
    assert_eq!(read(&closed).end, closed.len());
    let report = {
        let limits = ScanLimits::default();
        let input = format!("deploy --password {open}");
        let digest = EMBEDDED
            .semantic_digest(ScanProfile::Conservative, limits)
            .unwrap();
        evaluate(&EMBEDDED, ScanProfile::Conservative, limits, digest, &input).unwrap()
    };
    assert_eq!(report.limits_hit, Some(LimitExhausted::Match));
}

#[test]
fn the_word_reader_stops_at_the_work_budget_and_charges_what_it_read() {
    let rule = EMBEDDED
        .active(ScanProfile::Conservative)
        .find(|rule| rule.declaration.name == "magic-value-field")
        .unwrap();
    let word = "a9".repeat(2_000);
    let blanks = " ".repeat(100);
    let input = format!("token: {word}{blanks}# note");
    let captures = rule.regex.captures(input.as_bytes()).unwrap();
    let spans = candidate_spans(rule, &captures, &input).unwrap().unwrap();
    let read_with = |max_work_bytes| {
        let mut work = 0usize;
        let outcome = CredentialForm::FieldValue.word_spans(
            spans,
            &input,
            &mut work,
            ScanLimits {
                max_work_bytes,
                ..ScanLimits::default()
            },
        );
        (outcome, work)
    };
    let read = 2 * word.len() + blanks.len();
    let (outcome, work) = read_with(read);
    let found = outcome.unwrap().unwrap();
    assert_eq!(
        (found.value.start(), found.value.end()),
        (7, 7 + word.len())
    );
    assert_eq!(found.full.start(), 0);
    assert_eq!(work, read);
    let (outcome, work) = read_with(read - 1);
    assert!(matches!(outcome, Err(Abort::Work)), "{outcome:?}");
    assert_eq!(work, read - 1);
}

const LEADS: [&str; 12] = [
    "deploy --password ",
    "deploy --api-key\u{a0}",
    "run -token ",
    "password: ",
    "  api_key:\t",
    "X-Api-Key: ",
    "out\\npassword: ",
    "PGPASSWORD=",
    "export dbpassword=",
    "mysql -uroot -p",
    "sshpass -p ",
    "docker login -u bob -p ",
];
const WORD_PARTS: [&str; 34] = [
    "Zx9", "kq7", "a", "9", "'", "\"", "`", "\\", "\\\"", "$", ";", "&", "|", "<", ">", "(", ")",
    "{", "}", "[", "]", ",", "=", "#", "!", "\\n", "\\ ", "-", " ", "\u{a0}", "\\\\", "\\'", " #",
    "\\u000a",
];
const ENDS: [&str; 12] = [
    "",
    " ",
    "\n",
    "\r\n",
    "\u{2028}next",
    "\u{a0}next",
    " next word",
    "\t# note",
    " --verbose",
    " \"x\"",
    "\x0B",
    " '",
];

const ADMITTED_PARTS: [&str; 10] = ["kq7", "9", "!", "=", "$", "\\ ", "\\\\", "@", "%", "+"];
const LINE_ENDS: [&str; 4] = ["", "\n", "\r\n", "\t# note"];

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(6_000))]

    #[test]
    fn a_word_built_of_admitted_parts_is_reported_from_its_first_byte_to_its_last(
        lead in 0..LEADS.len(),
        parts in proptest::collection::vec(0..ADMITTED_PARTS.len(), 0..10),
        end in 0..LINE_ENDS.len(),
    ) {
        let word: String = std::iter::once("Zx9!")
            .chain(parts.iter().map(|&part| ADMITTED_PARTS[part]))
            .collect();
        let input = [LEADS[lead], &word, LINE_ENDS[end]].concat();
        let spans: Vec<(usize, usize)> = word_findings(&input)
            .into_iter()
            .map(|finding| (finding.value_span.start(), finding.value_span.end()))
            .collect();
        let start = LEADS[lead].len();
        assert_eq!(spans, [(start, start + word.len())], "{input:?}");
    }

    #[test]
    fn the_word_reader_agrees_with_the_word_patterns_on_generated_text(
        parts in proptest::collection::vec(0..WORD_PARTS.len(), 0..16),
        end in 0..ENDS.len(),
    ) {
        let word: String = parts.iter().map(|&part| WORD_PARTS[part]).collect();
        assert_reader_agrees_with_the_word_patterns(&[&word, ENDS[end]].concat());
    }

    #[test]
    fn a_reported_word_is_the_word_the_patterns_read_after_its_lead(
        lines in proptest::collection::vec(
            (0..LEADS.len(), proptest::collection::vec(0..WORD_PARTS.len(), 1..12), 0..ENDS.len()),
            1..4,
        ),
        cut in 0usize..4096,
    ) {
        let input: String = lines
            .iter()
            .map(|(lead, parts, end)| {
                let word: String = parts.iter().map(|&part| WORD_PARTS[part]).collect();
                [LEADS[*lead], &word, ENDS[*end]].concat()
            })
            .collect();
        let findings = word_findings(&input);
        for finding in &findings {
            let (start, end) = (finding.value_span.start(), finding.value_span.end());
            let word = WORD_PATTERNS.1.find(&input[start..]).unwrap().end();
            let expected = if finding.rule_id == "magic-value-password-assignment" {
                let sections = WORD_PATTERNS.0.find(&input[start..]).unwrap().end();
                let plain = &input.as_bytes()[start + sections..start + word];
                sections + serialized_line_break(plain).unwrap_or(plain.len())
            } else {
                word
            };
            assert_eq!(start + expected, end, "{} on {input:?}", finding.rule_id);
        }
        let mut at = cut % (input.len() + 1);
        while !input.is_char_boundary(at) {
            at -= 1;
        }
        let past = |findings: Vec<Finding>, offset: usize| -> Vec<(String, usize, usize, usize)> {
            findings
                .into_iter()
                .filter(|finding| finding.full_span.start() + offset >= at + 8)
                .map(|finding| {
                    (
                        finding.rule_id,
                        finding.full_span.start() + offset,
                        finding.value_span.start() + offset,
                        finding.value_span.end() + offset,
                    )
                })
                .collect()
        };
        let whole = past(findings, 0);
        let from_cut = past(word_findings(&input[at..]), at);
        for finding in &whole {
            assert!(from_cut.contains(finding), "{finding:?} of {input:?} from {at}");
        }
        let escapes = input.as_bytes()[..at]
            .iter()
            .rev()
            .take_while(|byte| **byte == b'\\')
            .count();
        let run = WORD_PATTERNS.2.find(&input[at - escapes..]).unwrap();
        let cut_run_end = at - escapes + run.end();
        for finding in &from_cut {
            assert!(
                whole.contains(finding) || finding.1 < cut_run_end,
                "{finding:?} of {input:?} from {at}"
            );
        }
    }
}

#[test]
fn a_word_with_a_quote_a_backslash_or_a_dollar_sign_is_reported_whole() {
    let issued = issued();
    for (template, word) in [
        ("password: {}", ["Pa55", "'!!"].concat()),
        ("password: {}", ["Pa55'", "\\word9"].concat()),
        ("password: {}", ["Pa55\\", "\"!!"].concat()),
        ("password: {}", ["Xk9Lm2Qp7", "\"#$"].concat()),
        ("password: {}", ["Z9r7", "\\"].concat()),
        (
            "deploy --password {} --verbose",
            ["Q7r9$", "\\+v9"].concat(),
        ),
        ("deploy --password {} --verbose", ["Q7r9", "$%v9"].concat()),
        ("deploy --password {} --verbose", ["Q7r9", "$"].concat()),
        ("mysql -uroot -p{} app", ["Q7r9$", "\\+v9"].concat()),
        ("sshpass -p {} ssh host", ["Q7r9$", "\\+v9"].concat()),
        (
            "docker login -u bob -p {} registry",
            ["Q7r9$", "$v9"].concat(),
        ),
        ("deploy --password {} --verbose", ["'Zx9", " kq7'"].concat()),
        (
            "deploy --password {} --verbose",
            ["\\\"Zx9", " kq7\\\""].concat(),
        ),
        ("deploy --password {} --verbose", ["Zx9\\", " kq7"].concat()),
        ("PGPASSWORD={} psql", ["ab;", "cd9"].concat()),
    ] {
        let input = template.replace("{}", &word);
        let findings = word_findings(&input);
        assert_eq!(findings.len(), 1, "{input:?}: {findings:?}");
        let span = findings[0].value_span;
        assert_eq!(&input[span.start()..span.end()], word, "{input:?}");
    }
    let input = format!("out\\npassword: {issued}\\nuser: bob");
    let findings = word_findings(&input);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let span = findings[0].value_span;
    assert_eq!(
        &input[span.start()..span.end()],
        format!("{issued}\\nuser:")
    );
}
