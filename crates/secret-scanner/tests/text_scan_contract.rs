//! Pins scanner limits, rule gating, deterministic ordering, UTF-8 byte spans,
//! checksum validation, placeholder suppression, and overflow reporting.
//! Reported spans index original UTF-8 input in bytes and must end on character
//! boundaries.

use proptest::prelude::*;
use secret_scanner::{LimitExhausted, RuleSource, ScanError, ScanLimits, ScanProfile, Scanner};
use std::sync::OnceLock;

fn comprehensive_scanner() -> &'static Scanner {
    static SCANNER: OnceLock<Scanner> = OnceLock::new();
    SCANNER.get_or_init(|| Scanner::new(ScanProfile::Comprehensive).unwrap())
}

#[test]
fn conservative_profile_returns_exact_utf8_spans() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    let input = "π prefix password=hunter2 suffix";
    let report = scanner.scan(input).unwrap();
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.rule_id == "magic-keyed-assignment")
        .unwrap();

    assert_eq!(finding.rule_source, RuleSource::ConservativeOverlay);
    assert_eq!(
        input.get(finding.full_span.start()..finding.full_span.end()),
        Some("password=hunter2")
    );
    assert_eq!(
        input.get(finding.value_span.start()..finding.value_span.end()),
        Some("hunter2")
    );
    let key = finding.key_span.unwrap();
    assert_eq!(input.get(key.start()..key.end()), Some("password"));
}

#[test]
fn short_quoted_values_are_kept_and_scalars_are_rejected() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    for input in [r#"{"password":"x"}"#, "'api_token': 'xy'"] {
        assert!(!scanner.scan(input).unwrap().findings.is_empty(), "{input}");
    }
    for input in [
        "tokens.input=45000",
        "password=true",
        "password=false",
        "password=null",
        "password=undefined",
        "password=-1.5e10",
        "password=.5",
        "password=-.25",
        "password=5.",
        "password=True",
        "password=False",
        "password=None",
        "password=NULL",
        "password=nil",
        "password=NaN",
        "password=~",
        "password=0x10",
        "password=0X1F",
        "password=0o755",
        "password=0b1010",
        "password=-0x10",
        "password=0xdeadbeef",
        "tokens.input=45_000",
        "tokens.input=1_000_000",
        "tokens.input=-1_500.5",
        "tokens.input=0xFF_FF",
        "tokens.input=0b1010_1010",
        "password=.inf",
        "password=-.inf",
        "password=.NaN",
        "password=Infinity",
        "password=-Infinity",
        "password=inf",
    ] {
        assert!(scanner.scan(input).unwrap().findings.is_empty(), "{input}");
    }
}

#[test]
fn empty_keyed_values_and_secret_substrings_are_not_findings() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    for input in [
        r#"{"password":""}"#,
        r#"{"password":"   "}"#,
        "'api_token': ''",
        "'api_token': '  '",
        "password=",
        "author=hunter-two",
        "keywords=hunter-two",
        "tokenizer=hunter-two",
        "secretary=hunter-two",
        "monkey=hunter-two",
        "turkey=hunter-two",
        "donkey=hunter-two",
        "hockey=hunter-two",
        "whiskey=hunter-two",
        "jockey=hunter-two",
        "keyboard=hunter-two",
        "keywords=hunter-two",
        "publickey=hunter-two",
        "PUBLICKEY=hunter-two",
        "public_key=hunter-two",
        "PUBLIC_KEY=hunter-two",
        "publicKey=hunter-two",
        "ssh_public_key=hunter-two",
        "pubkey=hunter-two",
        "publishable_key=hunter-two",
        "passwordless=hunter-two",
        "keyed=hunter-two",
        "tokenized=hunter-two",
        "hockey=hunter-two",
        "jockey=hunter-two",
        "mickey=hunter-two",
        r#"{"password":"\n"}"#,
        r#"{"password":"\t"}"#,
        r#"{"password":"\r\n"}"#,
        r#"{"password":" \n "}"#,
        "'api_token': '\\n'",
    ] {
        assert!(scanner.scan(input).unwrap().findings.is_empty(), "{input}");
    }
    for input in [
        "auth_token=hunter-two",
        "api-key=hunter-two",
        "private_key=hunter-two",
        "republic_key=hunter-two",
        // A hex-encoded credential is `0x`-prefixed too, so the radix branch must
        // not reach past a mode or mask into an Ethereum private key.
        "private_key=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        "private_key=0xdeadbeefcafe1234",
        "password=0x",
        "password=0xz",
        "password=0o8",
        "tokens.input=_45",
        "tokens.input=45_",
        "tokens.input=4__5",
        "tokens.input=0x_FF",
        r#"{"clientSecret":"hunter-two"}"#,
        r#"{"password":"\nhunter-two"}"#,
        r#"{"password":"\\"}"#,
    ] {
        assert!(!scanner.scan(input).unwrap().findings.is_empty(), "{input}");
    }
}

#[test]
fn undelimited_secret_key_names_are_gated_like_their_delimited_spelling() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    let value = "Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q";
    for key in [
        "api_key",
        "apiKey",
        "APIKEY",
        "APIKEYS",
        "AUTHTOKEN",
        "clientsecret",
        "CLIENTSECRET",
        "secretkey",
        "privatekey",
        "accesstoken",
        "refreshtoken",
        "DATABASEPASSWORD",
        "WEBHOOKSECRET",
        "dbpassword",
        "servicekey",
        "usertoken",
        "mastertoken",
        "sharedsecret",
        "AWSSECRETACCESSKEY",
        "awssecretaccesskey",
        "dbapikey",
        "PASSWORDHASH",
        "PASSWORD_HASH",
        "APIKEYVALUE",
        "tokenvalue",
        "secretname",
        "keyid",
    ] {
        let report = scanner.scan(&format!("{key}={value}")).unwrap();
        assert!(
            !report.findings.is_empty(),
            "{key} produced no finding while api_key did"
        );
    }
}

#[test]
fn repeated_scans_are_deterministic_and_independent() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let first = scanner.scan("password=first-secret").unwrap();
    let other = scanner.scan("nothing to detect").unwrap();
    let repeated = scanner.scan("password=first-secret").unwrap();
    assert_eq!(first, repeated);
    assert!(other.findings.is_empty());
}

#[test]
fn oversized_input_is_rejected_and_bounds_report_truncation() {
    let input_limited = Scanner::with_limits(
        ScanProfile::Conservative,
        ScanLimits {
            max_input_bytes: 4,
            ..ScanLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        input_limited.scan("12345").unwrap_err(),
        ScanError::InputLimitExceeded
    );

    let candidate_limited = Scanner::with_limits(
        ScanProfile::Conservative,
        ScanLimits {
            max_candidates: 1,
            ..ScanLimits::default()
        },
    )
    .unwrap();
    let report = candidate_limited.scan("password=one password=two").unwrap();
    assert_eq!(report.limits_hit, Some(LimitExhausted::Candidates));
    assert!(!report.is_complete());
    assert_eq!(report.candidates_evaluated, 1);

    let work_limited = Scanner::with_limits(
        ScanProfile::Conservative,
        ScanLimits {
            max_work_bytes: 1,
            ..ScanLimits::default()
        },
    )
    .unwrap();
    let report = work_limited.scan("password=secret").unwrap();
    assert_eq!(report.limits_hit, Some(LimitExhausted::Work));
    assert!(!report.is_complete());
}

/// Upstream rules run before overlay rules, so the upstream secret exhausts the candidate budget before the overlay secret is scanned.
/// The `Comprehensive` docs promise the `Conservative` findings only for complete reports.
#[test]
fn comprehensive_includes_conservative_findings_only_in_complete_reports() {
    let upstream_only = "CLOJARS_ujzde8gxd6ncf10epf91dhodzdoc9is0j8ht9lgmxg9edn581u33xtplpft7";
    let overlay = "password=Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q";
    let input = format!("{upstream_only}\n{overlay}\n");
    let overlay_findings = |profile, limits| {
        Scanner::with_limits(profile, limits)
            .unwrap()
            .scan(&input)
            .unwrap()
            .findings
            .into_iter()
            .filter(|finding| finding.rule_source == RuleSource::ConservativeOverlay)
            .count()
    };

    let complete = Scanner::new(ScanProfile::Comprehensive)
        .unwrap()
        .scan(&input)
        .unwrap();
    assert!(complete.is_complete());
    assert!(
        complete
            .findings
            .iter()
            .any(|finding| finding.rule_source == RuleSource::Upstream)
    );
    assert_eq!(
        overlay_findings(ScanProfile::Comprehensive, ScanLimits::default()),
        overlay_findings(ScanProfile::Conservative, ScanLimits::default()),
        "a complete comprehensive report must carry every conservative finding"
    );

    let one_candidate = ScanLimits {
        max_candidates: 1,
        ..ScanLimits::default()
    };
    assert_eq!(
        overlay_findings(ScanProfile::Conservative, one_candidate),
        1
    );
    assert_eq!(
        overlay_findings(ScanProfile::Comprehensive, one_candidate),
        0,
        "the counter-example stopped reproducing; re-derive it before strengthening the `Comprehensive` docs"
    );
}

#[test]
fn exhausting_a_limit_keeps_the_findings_already_collected() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    let secret = "auth_token=Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q\n";
    let mut padded = String::from(secret);
    while padded.len() < secret_scanner::MAX_INPUT_BYTES - 8 {
        padded.push_str("key=aB ");
    }
    let report = scanner.scan(&padded).unwrap();
    assert!(
        !report.findings.is_empty(),
        "padding must not suppress the secret ahead of it"
    );
}

/// `MAX_MATCH_BYTES` bounds one candidate, not the scan. A candidate whose
/// full match exceeds it is skipped and reported through `limits_hit`, while
/// every other candidate and rule keeps running; otherwise ~33 KiB of bait
/// ahead of a secret would hide it from every rule.
#[test]
fn an_oversized_candidate_is_skipped_without_stopping_other_rules() {
    let bait = format!("key={}\n", "a".repeat(secret_scanner::MAX_MATCH_BYTES + 16));
    let same_rule_secret = "auth_token=Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q\n";
    let other_rule_secret =
        "AGE-SECRET-KEY-1QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7LQPZRY9X8GF2TVDW0S3JN54KHCE\n";
    let input = format!("{bait}{same_rule_secret}{other_rule_secret}");

    let report = comprehensive_scanner().scan(&input).unwrap();
    assert_eq!(report.limits_hit, Some(LimitExhausted::Match));
    assert!(!report.is_complete());
    let rule_ids: Vec<&str> = report
        .findings
        .iter()
        .map(|finding| finding.rule_id.as_str())
        .collect();
    assert!(
        rule_ids.contains(&"magic-keyed-assignment"),
        "a later candidate of the rule that saw the oversized match was dropped: {rule_ids:?}"
    );
    assert!(
        rule_ids.contains(&"age-secret-key"),
        "a rule after the one that saw the oversized match never ran: {rule_ids:?}"
    );
    for finding in &report.findings {
        assert!(
            finding.full_span.start() >= bait.len(),
            "the oversized candidate itself must not be reported: {finding:?}"
        );
    }

    let baseline = comprehensive_scanner()
        .scan(&format!("{same_rule_secret}{other_rule_secret}"))
        .unwrap();
    assert!(baseline.is_complete());
    assert_eq!(
        report.findings.len(),
        baseline.findings.len(),
        "the bait changed the finding set beyond its own span"
    );
}

#[test]
fn a_complete_scan_of_dense_key_value_text_reports_no_truncation() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    let mut input = "key=aB ".repeat(secret_scanner::MAX_INPUT_BYTES / 7 + 1);
    input.truncate(secret_scanner::MAX_INPUT_BYTES);
    while !input.is_char_boundary(input.len()) {
        input.pop();
    }
    let report = scanner.scan(&input).unwrap();
    assert_eq!(report.limits_hit, None);
}

#[test]
fn diagnostics_do_not_include_scanned_text() {
    let sentinel = "privacy-sentinel-password=secret";
    let scanner = Scanner::with_limits(
        ScanProfile::Conservative,
        ScanLimits {
            max_input_bytes: 8,
            ..ScanLimits::default()
        },
    )
    .unwrap();
    let error = scanner.scan(sentinel).unwrap_err().to_string();
    assert!(!error.contains(sentinel));
    assert!(!error.contains("secret"));

    let truncated = Scanner::with_limits(
        ScanProfile::Conservative,
        ScanLimits {
            max_work_bytes: 1,
            ..ScanLimits::default()
        },
    )
    .unwrap();
    let reason = truncated
        .scan(sentinel)
        .unwrap()
        .limits_hit
        .unwrap()
        .to_string();
    assert!(!reason.contains(sentinel));
    assert!(!reason.contains("secret"));
}

#[test]
fn non_ascii_text_does_not_discard_findings_elsewhere() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let age = "AGE-SECRET-KEY-1QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7LQPZRY9X8GF2TVDW0S3JN54KHCE";
    let onepassword = "A3-0A1B2C-3D4E5F6G7H8-9I0J1-2K3L4-5M6N7";
    // A byte-class match can end inside a multi-byte UTF-8 character; later
    // findings must still be reported.
    let polluted =
        format!("netlify_token: Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Qé\n{age}\n{onepassword}\n");
    let report = scanner.scan(&polluted).unwrap();
    let ids: Vec<&str> = report
        .findings
        .iter()
        .map(|finding| finding.rule_id.as_str())
        .collect();
    assert!(ids.contains(&"age-secret-key"), "got {ids:?}");
    assert!(ids.contains(&"1password-secret-key"), "got {ids:?}");
}

#[test]
fn non_ascii_neighbours_do_not_change_findings() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let cases = [
        "password=hunter2",
        "AGE-SECRET-KEY-1QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7LQPZRY9X8GF2TVDW0S3JN54KHCE",
        "A3-0A1B2C-3D4E5F6G7H8-9I0J1-2K3L4-5M6N7",
        "netlify_token: Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q",
    ];
    for ascii in cases {
        let baseline = scanner.scan(ascii).unwrap().findings;
        assert!(!baseline.is_empty(), "{ascii} produced no baseline finding");
        let expected = rule_ids_and_values(ascii, &baseline);
        for (prefix, suffix) in [
            ("é\n", ""),
            ("😀\n", ""),
            ("", " é"),
            ("日本語\n", "\n日本語"),
        ] {
            let decorated = format!("{prefix}{ascii}{suffix}");
            let findings = scanner.scan(&decorated).unwrap().findings;
            assert_eq!(
                rule_ids_and_values(&decorated, &findings),
                expected,
                "non-ASCII around {ascii} changed the findings"
            );
        }
    }
}

fn rule_ids_and_values(input: &str, findings: &[secret_scanner::Finding]) -> Vec<(String, String)> {
    findings
        .iter()
        .map(|finding| {
            (
                finding.rule_id.clone(),
                input[finding.value_span.start()..finding.value_span.end()].to_owned(),
            )
        })
        .collect()
}

/// Corpus patterns require a terminator after the value, so a trailing
/// non-ASCII character must be treated like any other non-terminator byte.
#[test]
fn a_trailing_non_ascii_character_behaves_like_a_trailing_ascii_one() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let keyed = "netlify_token: Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q";
    let ascii_non_terminator = scanner.scan(&format!("{keyed}#")).unwrap();
    let non_ascii = scanner.scan(&format!("{keyed}é")).unwrap();
    assert_eq!(
        non_ascii.findings.len(),
        ascii_non_terminator.findings.len()
    );
    assert!(non_ascii.is_complete());
}

#[test]
fn mixed_case_keys_are_gated_like_their_lowercase_spelling() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let value = "Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q";
    let lowercase = scanner
        .scan(&format!("netlify_token: {value}"))
        .unwrap()
        .findings
        .len();
    assert!(lowercase > 0);
    for key in ["Netlify_Api_Key", "NETLIFY_TOKEN", "netlifyApiKey"] {
        let report = scanner.scan(&format!("{key}: {value}")).unwrap();
        assert!(
            !report.findings.is_empty(),
            "{key} produced no finding while netlify_token did"
        );
    }
}

#[test]
fn value_span_covers_the_secret_for_multi_group_rules() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let token = "ZXlKclpYbGZiM0J6SWpwY0lqRXlNelExTmpjNE9UQXhNak0wTlRZM09EazBNVEl6TkRVMg";
    let input = format!("zxlk {token}");
    let findings = scanner.scan(&input).unwrap().findings;
    assert!(!findings.is_empty(), "no rule matched the JWT-shaped token");
    for finding in findings {
        let value = &input[finding.value_span.start()..finding.value_span.end()];
        let full = &input[finding.full_span.start()..finding.full_span.end()];
        assert_eq!(
            value, full,
            "{} reported a fragment of its match as the secret",
            finding.rule_id
        );
    }
}

/// A caller redacting `value_span` must delete the credential and nothing else.
#[test]
fn value_span_excludes_surrounding_command_text_for_alternation_rules() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let credential = "sk9Xq2Lm7Pv4Rt8Zw1Yc6Nb3Hd5Kf0Jg";
    let input = format!(
        "curl -X POST https://api.corp-internal.net/v1/things -H \"Authorization: Bearer {credential}\" "
    );
    let finding = scanner
        .scan(&input)
        .unwrap()
        .findings
        .into_iter()
        .find(|finding| finding.rule_id == "curl-auth-header")
        .expect("curl-auth-header did not match");
    assert_eq!(
        &input[finding.value_span.start()..finding.value_span.end()],
        credential
    );
}

/// `Indeterminate` does not reject, so an undecodable checksum must be `Invalid`.
#[test]
fn checksum_backed_rules_reject_undecodable_checksums() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let payload = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5";
    let github_pat = |report: &secret_scanner::ScanReport| {
        report
            .findings
            .iter()
            .any(|finding| finding.rule_id == "github-pat")
    };
    // CRC-32 of `payload`, base-62 encoded, so the rule is known to be live before the rejections are checked.
    assert!(github_pat(
        &scanner.scan(&format!("ghp_{payload}0Zb5Hm ")).unwrap()
    ));
    for checksum in ["zzzzzz", "q7r8s9"] {
        let report = scanner.scan(&format!("ghp_{payload}{checksum} ")).unwrap();
        assert!(
            !github_pat(&report),
            "github-pat accepted checksum {checksum}, which exceeds u32"
        );
    }
}

/// `(?i)` makes Grafana prefixes case-insensitive; mixed-case prefixes must still reject invalid checksums.
#[test]
fn offline_validators_check_prefixes_case_insensitively() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let body = "AbCdEfGhIjKlMnOpQrStUvWxYz012345";
    let grafana = |report: &secret_scanner::ScanReport| {
        report
            .findings
            .iter()
            .any(|finding| finding.rule_id == "grafana-service-account-token")
    };
    // CRC-32 of `glsa_{body}` in hex, so the rule is known to be live before the rejections are checked.
    assert!(grafana(
        &scanner.scan(&format!("glsa_{body}_1b447f14 ")).unwrap()
    ));
    for prefix in ["glsa_", "GLSA_", "Glsa_"] {
        let report = scanner.scan(&format!("{prefix}{body}_00000000 ")).unwrap();
        assert!(!grafana(&report), "{prefix} bypassed checksum rejection");
    }
}

/// `entropy.min_len` is a sampling threshold, not a minimum credential length,
/// so a bypassed entropy check must not veto a candidate on its own.
#[test]
fn credentials_below_the_entropy_sampling_length_are_still_reported() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    for (input, rule_id, value) in [
        (
            "curl -u abc:def https://api.corp-internal.net/v1/x",
            "curl-auth-user",
            "abc:def",
        ),
        (
            "curl -u admin:s3cr3t99 https://api.corp-internal.net/v1/x",
            "curl-auth-user",
            "admin:s3cr3t99",
        ),
    ] {
        let finding = scanner
            .scan(input)
            .unwrap()
            .findings
            .into_iter()
            .find(|finding| finding.rule_id == rule_id)
            .unwrap_or_else(|| panic!("{rule_id} produced no finding for {input:?}"));
        assert_eq!(
            &input[finding.value_span.start()..finding.value_span.end()],
            value
        );
    }
}

/// `secret` is both a key name and a qualifier, so every offset matches the name
/// and decomposes on its left, while the `zz` tail never decomposes on the right.
/// A per-offset walk explores all of them before failing.
#[test]
fn a_long_undecomposable_key_name_stays_a_non_finding() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let key = format!("{}zz", "secret".repeat(128));
    let input = format!("\"{key}\": \"Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24\"");
    assert!(
        !scanner
            .scan(&input)
            .unwrap()
            .findings
            .iter()
            .any(|finding| finding.rule_id.starts_with("magic-keyed"))
    );
}

#[test]
fn anchor_preselection_honours_the_work_budget() {
    let scanner = Scanner::with_limits(
        ScanProfile::Comprehensive,
        ScanLimits {
            max_work_bytes: 1,
            ..ScanLimits::default()
        },
    )
    .unwrap();
    let input = "curl password token secret key auth bearer credential ".repeat(4000);
    let report = scanner.scan(&input).unwrap();
    assert_eq!(report.limits_hit, Some(LimitExhausted::Work));
    // `add_work` charges before it checks, so the preselect charge lands in full
    // and stops there; anything larger means a later phase also ran.
    assert_eq!(report.work_bytes, input.len());
    assert_eq!(report.candidates_evaluated, 0);
    assert!(report.findings.is_empty());
}

#[test]
fn a_truncated_report_is_not_a_prefix_of_a_complete_one() {
    let input = "password = sk9Xq2Lm7Pv4Rt8Zw1Yc6Nb3Hd5Kf0Jg\nlater AIzaSyA1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q end";
    let complete = comprehensive_scanner().scan(input).unwrap();
    let truncated = Scanner::with_limits(
        ScanProfile::Comprehensive,
        ScanLimits {
            max_candidates: 1,
            ..ScanLimits::default()
        },
    )
    .unwrap()
    .scan(input)
    .unwrap();
    assert_eq!(truncated.limits_hit, Some(LimitExhausted::Candidates));
    assert_eq!(truncated.findings.len(), 1);
    assert!(complete.findings.len() > 1);
    assert_ne!(
        truncated.findings[0].full_span.start(),
        complete.findings[0].full_span.start(),
        "the counter-example stopped reproducing; re-derive it before relaxing the contract"
    );
}

/// A credential matched by both rule sets must produce the same verdict.
#[test]
fn overlay_vendor_rules_honour_the_upstream_safelists() {
    for profile in [ScanProfile::Conservative, ScanProfile::Comprehensive] {
        let scanner = Scanner::new(profile).unwrap();
        // A key outside the safelist is reported, so the empty reports below come from the safelist and not from a rule that stopped matching.
        assert!(
            !scanner
                .scan("aws_access_key_id = AKIAQ7R3XM2ZT5WN6PBC")
                .unwrap()
                .findings
                .is_empty(),
            "{profile:?} did not report an AWS access key outside the safelist"
        );
        for input in [
            "AKIAIOSFODNN7EXAMPLE",
            "aws_access_key_id = AKIAIOSFODNN7EXAMPLE",
        ] {
            assert!(
                scanner.scan(input).unwrap().findings.is_empty(),
                "{profile:?} reported {input}"
            );
        }
    }
}

/// The regex alternation splits `monkey` around `key`, so the local-context gate
/// is what has to reject it.
#[test]
fn local_context_rejects_key_names_embedded_in_other_identifiers() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let value = "Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24";
    for key in [
        "monkey",
        "turkey",
        "author",
        "keyboard",
        "authority",
        "max_input_tokens",
    ] {
        let report = scanner.scan(&format!("{key}: {value}")).unwrap();
        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.rule_id == "generic-api-key"),
            "{key} was reported as an API key"
        );
    }
    for key in [
        "api_key",
        "ApiKey",
        "apiKey",
        "API_KEY",
        "apikey",
        "api_tokens",
        "access_keys",
    ] {
        let report = scanner.scan(&format!("{key}: {value}")).unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.rule_id == "generic-api-key"),
            "{key} produced no generic-api-key finding"
        );
    }
}

/// A recognized prefix over a malformed body is invalid, not unverifiable. The
/// accepted cases are one per upstream Slack shape.
#[test]
fn slack_validation_rejects_malformed_bodies() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    for input in [
        "xoxb-not-a-real-token",
        "xoxb-1234567890-not-a-real-token",
        "xoxb-GGGGGGGGGG",
        "xoxb-1234567890-a",
        "xapp-notadigit-A012345-1234567890-abcdef",
    ] {
        assert!(
            scanner.scan(input).unwrap().findings.is_empty(),
            "{input} was reported as a Slack token"
        );
    }
    // The corpus rule matches the leading `xoxa-abcdefgh` and reports it, so the
    // overlay must report the same span rather than swallowing the trailing text.
    let trailing = "xoxa-abcdefgh-not-a-real-token";
    let findings = scanner.scan(trailing).unwrap().findings;
    assert!(
        findings
            .iter()
            .any(|finding| finding.rule_id == "magic-slack-token")
    );
    for finding in findings {
        assert_eq!(
            &trailing[finding.value_span.start()..finding.value_span.end()],
            "xoxa-abcdefgh",
            "{} reported past the token shape",
            finding.rule_id
        );
    }
    // Bodies are joined to their prefix at run time so no line of this file holds
    // a well-formed token, which GitHub push protection rejects.
    let prefix = "xox";
    for body in [
        "b-1234567890123-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx",
        "b-1234567890-1234567890Ab-Cd",
        "b-12345678-abcdefghijklmnopqr",
        "p-1234567890-1234567890-1234567890-abcdefghijklmnopqrstuvwxyz1234",
        "o-1-22-333-abcdef123456",
        "a-2-abc12345",
        "r-abc12345678",
    ] {
        let input = format!("{prefix}{body}");
        assert!(
            !scanner.scan(&input).unwrap().findings.is_empty(),
            "{input} stopped reporting"
        );
    }
    let app = format!("{}-1-A012345-1234567890-abcdef", "xapp");
    assert!(
        !scanner.scan(&app).unwrap().findings.is_empty(),
        "{app} stopped reporting"
    );
    // slack-config-refresh-token fixes this body at 146 characters.
    let body: String = "ABCDEF0123456789".repeat(10).chars().take(146).collect();
    assert_eq!(body.len(), 146);
    let refresh = format!("{prefix}e-1-{body}");
    assert!(
        !scanner.scan(&refresh).unwrap().findings.is_empty(),
        "the config-refresh shape stopped reporting"
    );
}

/// The keyed rules have no entropy floor and stay outside the engine safelists,
/// so their suppressor list is the only thing standing between a placeholder and
/// a finding.
#[test]
fn value_suppressors_match_mixed_case_placeholders() {
    let scanner = Scanner::new(ScanProfile::Conservative).unwrap();
    for value in [
        "placeholder",
        "PLACEHOLDER",
        "Placeholder",
        "changeme",
        "CHANGEME",
        "ChangeMe",
        "Redacted",
        "Dummy",
        "ToDo",
        "Example_Value",
        "Your_Key_Here",
        "NotASecret",
        "XxXx",
    ] {
        let report = scanner.scan(&format!("password={value}")).unwrap();
        assert!(
            report.findings.is_empty(),
            "password={value} was reported as a secret"
        );
    }
    assert!(
        !scanner
            .scan("password=hunter-two")
            .unwrap()
            .findings
            .is_empty(),
        "a value matching no suppressor stopped reporting"
    );
}

/// Both an anchor-free input and a candidate-dense input at `MAX_INPUT_BYTES`
/// complete under the default limits.
#[test]
fn maximum_supported_inputs_succeed_with_default_limits() {
    let scanner = comprehensive_scanner();
    let sparse = "x".repeat(secret_scanner::MAX_INPUT_BYTES);
    assert!(scanner.scan(&sparse).unwrap().findings.is_empty());

    let mut dense = "password=x ".repeat(secret_scanner::MAX_INPUT_BYTES / 11 + 1);
    dense.truncate(secret_scanner::MAX_INPUT_BYTES);
    while !dense.is_char_boundary(dense.len()) {
        dense.pop();
    }
    let report = scanner.scan(&dense).unwrap();
    assert!(!report.findings.is_empty());
    assert!(report.work_bytes <= ScanLimits::default().max_work_bytes);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn arbitrary_utf8_with_real_secret_returns_valid_spans(
        prefix in ".{0,128}",
        suffix in ".{0,128}",
    ) {
        let input = format!("{prefix}\npassword=property-secret\n{suffix}");
        let scanner = comprehensive_scanner();
        let report = scanner.scan(&input).unwrap();
        prop_assert!(!report.findings.is_empty());
        prop_assert!(report.candidates_evaluated <= ScanLimits::default().max_candidates);
        if report.is_complete() {
            prop_assert!(report.work_bytes <= ScanLimits::default().max_work_bytes);
        }
        for pair in report.findings.windows(2) {
            prop_assert!(pair[0].full_span.start() <= pair[1].full_span.start());
        }
        for finding in report.findings {
            prop_assert!(finding.full_span.end() <= input.len());
            prop_assert!(finding.full_span.start() <= finding.value_span.start());
            prop_assert!(finding.value_span.end() <= finding.full_span.end());
            prop_assert!(input.is_char_boundary(finding.full_span.start()));
            prop_assert!(input.is_char_boundary(finding.full_span.end()));
            prop_assert!(input.is_char_boundary(finding.value_span.start()));
            prop_assert!(input.is_char_boundary(finding.value_span.end()));
            prop_assert!(!finding.value_span.is_empty());
            if let Some(key) = finding.key_span {
                prop_assert!(finding.full_span.start() <= key.start());
                prop_assert!(key.end() <= finding.full_span.end());
                prop_assert!(input.is_char_boundary(key.start()));
                prop_assert!(input.is_char_boundary(key.end()));
            }
        }
    }
}
