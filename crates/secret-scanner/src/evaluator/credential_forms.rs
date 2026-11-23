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
        ("\"Zx9kq7\\nmore\"", literal("Zx9kq7")),
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
        "\u{9700}\u{8981}\u{5bc6}\u{7801}",
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

const WORD_RULES: [&str; 6] = [
    "magic-option-value",
    "magic-field-value",
    "magic-field-value-serialized",
    "magic-password-suffix-assignment",
    "magic-client-password-attached",
    "magic-client-password-option",
];

fn is_separator(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\x0B' | '\x0C' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn word_findings(input: &str) -> Vec<Finding> {
    let limits = ScanLimits::default();
    let digest = EMBEDDED
        .semantic_digest(ScanProfile::Conservative, limits)
        .unwrap();
    let report = evaluate(&EMBEDDED, ScanProfile::Conservative, limits, digest, input).unwrap();
    assert_eq!(report.limits_hit, None, "{input:?}");
    report
        .findings
        .into_iter()
        .filter(|finding| WORD_RULES.contains(&finding.rule_id.as_str()))
        .collect()
}

fn assert_words_are_reported_whole(input: &str) {
    for finding in word_findings(input) {
        let after = input[finding.value_span.end()..].chars().next();
        assert!(
            after.is_none_or(is_separator),
            "{} reports {:?} of {input:?} and leaves {after:?} after it",
            finding.rule_id,
            &input[finding.value_span.start()..finding.value_span.end()],
        );
    }
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
const WORD_PARTS: [&str; 28] = [
    "Zx9", "kq7", "a", "9", "'", "\"", "`", "\\", "\\\"", "$", ";", "&", "|", "<", ">", "(", ")",
    "{", "}", "[", "]", ",", "=", "#", "!", "\\n", "\\ ", "-",
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

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(6_000))]

    #[test]
    fn a_reported_word_ends_at_a_separator_or_the_end_of_the_text(
        lead in 0..LEADS.len(),
        parts in proptest::collection::vec(0..WORD_PARTS.len(), 1..12),
        end in 0..ENDS.len(),
        cut in 0usize..4096,
    ) {
        let word: String = parts.iter().map(|&part| WORD_PARTS[part]).collect();
        let input = [LEADS[lead], &word, ENDS[end]].concat();
        assert_words_are_reported_whole(&input);
        let mut at = cut % (input.len() + 1);
        while !input.is_char_boundary(at) {
            at -= 1;
        }
        assert_words_are_reported_whole(&input[..at]);
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
