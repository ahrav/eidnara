use secret_scanner::{RuleSource, ScanProfile, Scanner};

#[test]
fn provider_canaries_return_stable_rule_ids_and_value_spans() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let cases = [
        (
            "A3-0A1B2C-3D4E5F6G7H8-9I0J1-2K3L4-5M6N7",
            "1password-secret-key",
        ),
        (
            "AGE-SECRET-KEY-1QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7LQPZRY9X8GF2TVDW0S3JN54KHCE",
            "age-secret-key",
        ),
    ];
    for (input, rule_id) in cases {
        assert_whole_input_is_the_value(&scanner, input, rule_id);
    }
}

/// Both letters the `facebook-page-access-token` character class accepts
/// still match; the embedded corpus spells that class in a different order
/// than the Gossip-rs corpus does.
#[test]
fn facebook_page_access_token_matches_both_prefix_letters() {
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let body = "q8Zt3Kp1Lm9Xw2Vb7Ny4Rc6Hd0Jf5Gs8Aq1Te3Yu7Io2Pw9Dk4Lx6Zn0Bv5Cm8Fr1Gt3Hy7Ju2Kp9Lq4Mw6Nx0Oz5Pa8Rb1Sc3Td7Ue2Vf";
    assert_eq!(body.len(), 106);
    for prefix in ["EAAM", "EAAC"] {
        let input = format!("{prefix}{body}");
        assert_whole_input_is_the_value(&scanner, &input, "facebook-page-access-token");
    }
}

fn assert_whole_input_is_the_value(scanner: &Scanner, input: &str, rule_id: &str) {
    let report = scanner.scan(input).unwrap();
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.rule_id == rule_id)
        .unwrap_or_else(|| panic!("missing canary rule {rule_id}"));
    assert_eq!(finding.rule_source, RuleSource::Upstream);
    assert_eq!(
        input.get(finding.value_span.start()..finding.value_span.end()),
        Some(input)
    );
}

#[test]
fn profile_and_finding_semantics_change_the_digest() {
    let conservative = Scanner::new(ScanProfile::Conservative).unwrap();
    let comprehensive = Scanner::new(ScanProfile::Comprehensive).unwrap();
    assert_ne!(
        conservative.semantic_digest(),
        comprehensive.semantic_digest()
    );
}
