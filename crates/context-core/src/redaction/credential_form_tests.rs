use super::*;

/// The fixtures assemble credential-shaped values at run time so each source literal contains only a fragment.
fn issued() -> String {
    ["Zq7Lm2Xv", "9Rt4Kp8W", "n3Bc6Yd1"].concat()
}

fn chosen() -> String {
    ["Zx9", "!kq7"].concat()
}

fn pin() -> String {
    ["12", "3456"].concat()
}

fn digits() -> String {
    ["1234", "5678"].concat()
}

fn url(scheme: &str, user: &str, password: &str, rest: &str) -> String {
    format!("{scheme}://{user}:{password}@{rest}")
}

fn basic_credentials() -> String {
    ["dXNlcjpaeDlr", "cTc="].concat()
}

fn private_key_header() -> String {
    format!("-----BEGIN {} KEY-----", ["RSA ", "PRIVATE"].concat())
}

fn private_key_body_line() -> String {
    let line = [
        "QUJDREVGR0hJSktM",
        "TU5PUFFSU1RVVldY",
        "WVphYmNkZWZnaGlq",
        "a2xtbm9wcXJzdHV2",
    ]
    .concat();
    assert_eq!(line.len(), 64);
    line
}

#[track_caller]
fn assert_redacts(input: &str, value: &str, placeholder: &str) {
    assert_eq!(
        input.matches(value).count(),
        1,
        "{input:?} holds {value:?} once"
    );
    let redaction = redact_durable_text(input);
    assert_eq!(
        redaction.text,
        input.replacen(value, placeholder, 1),
        "{input:?}"
    );
    assert_eq!(redaction.detections.len(), 1, "{input:?}");
    let detection = &redaction.detections[0];
    assert_eq!(detection.offset, input.find(value).unwrap(), "{input:?}");
    assert_eq!(detection.length, value.len(), "{input:?}");
    assert_eq!(
        format!("<REDACTED:{}>", detection.secret_type),
        placeholder,
        "{input:?}"
    );
    assert_eq!(
        reject_secret_text(input).unwrap_err().kind(),
        RedactionErrorKind::SecretDetected,
        "{input:?}"
    );
}

#[track_caller]
fn assert_exact(input: &str) {
    let redaction = redact_durable_text(input);
    assert_eq!(redaction.text, input);
    assert_eq!(redaction.detections, [], "{input:?}");
    assert_eq!(reject_secret_text(input), Ok(()), "{input:?}");
}

const PASSWORD: &str = "<REDACTED:password>";

#[test]
fn an_option_followed_by_its_value_is_redacted() {
    let (issued, chosen) = (issued(), chosen());
    for (input, value, placeholder) in [
        (
            format!("deploy --password {chosen} --verbose"),
            chosen.clone(),
            PASSWORD,
        ),
        (
            format!("deploy --db-password\t{chosen}"),
            chosen.clone(),
            "<REDACTED:db_password>",
        ),
        (
            format!("deploy --api-key {issued} --host h"),
            issued.clone(),
            "<REDACTED:api_key>",
        ),
        (
            format!("run -token {issued}"),
            issued.clone(),
            "<REDACTED:token>",
        ),
        (
            format!("deploy --client-secret\u{a0}{issued}\u{a0}next"),
            issued.clone(),
            "<REDACTED:client_secret>",
        ),
        (
            format!("deploy --password \"{chosen} two\" --verbose"),
            format!("\"{chosen} two\""),
            PASSWORD,
        ),
        (
            format!("deploy --password '{chosen} two' x"),
            format!("'{chosen} two'"),
            PASSWORD,
        ),
        (
            format!("deploy --password {chosen}\u{2028}next line"),
            chosen.clone(),
            PASSWORD,
        ),
        (
            format!("deploy --password {} --verbose", pin()),
            pin(),
            PASSWORD,
        ),
        (
            format!("run\\n--password {chosen}\\n--verbose"),
            format!("{chosen}\\n--verbose"),
            PASSWORD,
        ),
        (
            format!("{{\"command\":\"deploy --password {chosen}\"}}"),
            format!("{chosen}\"}}"),
            PASSWORD,
        ),
    ] {
        assert_redacts(&input, &value, placeholder);
    }
}

#[test]
fn an_option_followed_by_a_stand_in_stays_exact() {
    let issued = issued();
    for input in [
        "usage: deploy [--password PASSWORD] [--token TOKEN]".to_owned(),
        "deploy --password \"$DB_PASSWORD\" --verbose".to_owned(),
        "deploy --password $(cat /run/secrets/db) --verbose".to_owned(),
        "deploy --token `cat token.txt` --verbose".to_owned(),
        "deploy --password <password>".to_owned(),
        "deploy --api-key {api_key}".to_owned(),
        "the --password option reads the prompt".to_owned(),
        "docker build --secret id=npm,src=.npmrc .".to_owned(),
        "deploy --token-file /run/secrets/token".to_owned(),
        "generate --max-tokens 4096".to_owned(),
        "generate --token 4096".to_owned(),
        "deploy --password --verbose".to_owned(),
        "deploy --token config.token".to_owned(),
        format!("deploy --no-password {issued}"),
        format!("deploy --password\n{issued}"),
    ] {
        assert_exact(&input);
    }
}

#[test]
fn a_field_line_is_redacted() {
    let (issued, chosen) = (issued(), chosen());
    for (input, value, placeholder) in [
        (format!("password: {chosen}"), chosen.clone(), PASSWORD),
        (
            format!("  db_password:\t{chosen}\n  user: bob"),
            chosen.clone(),
            "<REDACTED:db_password>",
        ),
        (
            format!("X-Api-Key: {issued}"),
            issued.clone(),
            "<REDACTED:x_api_key>",
        ),
        (
            format!("password: {chosen}   # rotate"),
            chosen.clone(),
            PASSWORD,
        ),
        (
            format!("password : {chosen}\r\nuser: bob"),
            chosen.clone(),
            PASSWORD,
        ),
        (
            format!("password:\u{a0}{chosen}\u{2028}next"),
            chosen.clone(),
            PASSWORD,
        ),
        (
            format!("password: \"{chosen} two\""),
            format!("\"{chosen} two\""),
            PASSWORD,
        ),
        (
            format!("apiKey: '{issued}',"),
            format!("'{issued}',"),
            "<REDACTED:api_key>",
        ),
        (
            format!("out\\npassword: {chosen}\\nuser: bob"),
            format!("{chosen}\\nuser:"),
            PASSWORD,
        ),
        (
            format!("out\\u000apassword: {chosen}\\u000auser: bob"),
            format!("{chosen}\\u000auser:"),
            PASSWORD,
        ),
        (
            format!("password: \"{chosen} #two\"  # rotate"),
            format!("\"{chosen} #two\""),
            PASSWORD,
        ),
        (
            format!("{{\"stdout\":\"user: bob\\npassword: {chosen}\"}}"),
            format!("{chosen}\"}}"),
            PASSWORD,
        ),
    ] {
        assert_redacts(&input, &value, placeholder);
    }
}

#[test]
fn a_field_of_types_counts_and_references_stays_exact() {
    let chosen = chosen();
    for input in [
        "password: string".to_owned(),
        "token: u64,".to_owned(),
        "api_key: Option<String>,".to_owned(),
        "password: &str".to_owned(),
        "secret: String!".to_owned(),
        "password: getpass()".to_owned(),
        "max_tokens: 4096".to_owned(),
        "token: 4096".to_owned(),
        "token: https://example.org/settings/tokens".to_owned(),
        "password: $DB_PASSWORD".to_owned(),
        "password: ${DB_PASSWORD}".to_owned(),
        "password: {{ vault_password }}".to_owned(),
        ["password: ", "settings.password"].concat(),
        "password: ********".to_owned(),
        "token_file: /run/secrets/token".to_owned(),
        format!("password: {chosen} and more words"),
        format!("password: \"{chosen} #two\" and more words"),
        format!("password: \\\"{chosen} #two\\\" and more words"),
        format!("password: \"{chosen}"),
        format!("the tokens: {chosen}"),
        format!("password:{chosen}"),
    ] {
        assert_exact(&input);
    }
}

#[test]
fn a_word_with_a_quote_a_backslash_or_a_dollar_sign_is_replaced_whole() {
    for (template, word) in [
        ("password: {}", ["Pa55", "'!!"].concat()),
        ("password: {}", ["Pa55'", "\\word9"].concat()),
        ("password: {}", ["Pa55\\", "\"!!"].concat()),
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
        ("PGPASSWORD={} psql", ["ab;", "cd9"].concat()),
    ] {
        assert_redacts(&template.replace("{}", &word), &word, PASSWORD);
    }
}

#[test]
fn a_name_that_ends_in_password_joined_to_its_prefix_is_a_password_key() {
    let chosen = chosen();
    for (input, value) in [
        (format!("PGPASSWORD={chosen} psql -h host"), chosen.clone()),
        (
            format!("docker run -e PGPASSWORD={chosen} postgres"),
            chosen.clone(),
        ),
        (format!("mypassword={chosen}"), chosen.clone()),
        (
            format!("x\\nPGPASSWORD={chosen}\\npsql"),
            format!("{chosen}\\npsql"),
        ),
        (
            format!("PGPASSWORD=\"{chosen} two\" psql"),
            format!("\"{chosen} two\""),
        ),
        (
            format!("PGPASSWORD={} psql", ["post", "gres"].concat()),
            ["post", "gres"].concat(),
        ),
        (format!("PGPASSWORD={} psql", pin()), pin()),
    ] {
        assert_redacts(&input, &value, PASSWORD);
    }
    for input in [
        ["PGPASSWORD=", " psql -h host"].concat(),
        "PGPASSWORD=$PGPASS psql".to_owned(),
        "PGPASSWORD=\"${PGPASS}\" psql".to_owned(),
        "PGPASSWORD=true".to_owned(),
        ["PGPASSWORD=", "settings.password"].concat(),
    ] {
        assert_exact(&input);
    }
}

#[test]
fn digits_under_a_password_or_secret_key_are_redacted() {
    let digits = digits();
    for (template, placeholder) in [
        ("password={}", PASSWORD),
        ("password = {}", PASSWORD),
        ("password: {}", PASSWORD),
        ("{\"password\":\"{}\"}", PASSWORD),
        ("{\"password\": {}}", PASSWORD),
        ("'client_secret': '{}',", "<REDACTED:client_secret>"),
        ("{\\\"password\\\":\\\"{}\\\"}", PASSWORD),
        ("POSTGRES_PASSWORD: {}  # change me", PASSWORD),
        ("login?user=bob&password={}&next=1", PASSWORD),
    ] {
        assert_redacts(&template.replace("{}", &digits), &digits, placeholder);
    }
    for input in [
        "password=123",
        "password=1234.5",
        "password_length=1234",
        "secret_version=1234",
        "max_tokens=4096",
        "token=12345678",
        "api_key: 12345678",
        "Password:14896 /bin/zsh",
    ] {
        assert_exact(input);
    }
}

#[test]
fn a_password_in_url_userinfo_is_redacted() {
    let chosen = chosen();
    for input in [
        format!(
            "git clone {}",
            url("https", "alice", &chosen, "example.org/r.git")
        ),
        url("postgres", "admin", &chosen, "db.internal:5432/app"),
        format!("proxy={}", url("socks5h", "gw", &chosen, "10.0.0.1:1080")),
    ] {
        assert_redacts(&input, &chosen, "<REDACTED:url_password>");
    }
    for input in [
        url("https", "user", "password", "example.org/"),
        url("https", "user", "$PASSWORD", "example.org/"),
        url("https", "user", "%DB_PASSWORD%", "example.org/"),
        "http://localhost:8080/path@v2".to_owned(),
        "https://alice@example.org/r.git".to_owned(),
    ] {
        assert_exact(&input);
    }
}

#[test]
fn basic_authorization_credentials_are_redacted() {
    let basic = basic_credentials();
    for input in [
        format!("Authorization: Basic {basic}"),
        format!("curl -H \"Authorization: Basic {basic}\" https://example.org"),
        format!("GET / HTTP/1.1\\nAuthorization: Basic {basic}\\n"),
    ] {
        assert_redacts(&input, &basic, "<REDACTED:basic>");
    }
    assert_exact("Authorization: Basic authentication is sent in clear text");
}

#[test]
fn a_password_option_of_a_named_client_is_redacted() {
    let chosen = chosen();
    for (input, value) in [
        (format!("mysql -uroot -p{chosen} app"), chosen.clone()),
        (
            format!("mysqldump -h db -p{chosen} app > dump.sql"),
            chosen.clone(),
        ),
        (format!("/usr/bin/mariadb -p{chosen}"), chosen.clone()),
        (
            format!("sshpass -p {chosen} ssh deploy@host"),
            chosen.clone(),
        ),
        (
            format!("sshpass -p'{chosen} two' ssh deploy@host"),
            format!("'{chosen} two'"),
        ),
        (
            format!("docker login -u bob -p {chosen} registry.example.org"),
            chosen.clone(),
        ),
        (
            format!("helm registry login -u bob -p {chosen} registry"),
            chosen.clone(),
        ),
        (
            format!("echo start; docker login -p{chosen}"),
            chosen.clone(),
        ),
        (
            "docker login -u bob -p 9234 registry".to_owned(),
            "9234".to_owned(),
        ),
    ] {
        assert_redacts(&input, &value, PASSWORD);
    }
}

#[test]
fn a_short_option_of_another_program_or_another_command_stays_exact() {
    let chosen = chosen();
    for input in [
        "ssh -p 2222 deploy@host".to_owned(),
        "docker run -p 8080:80 image".to_owned(),
        "mkdir -p build/out7".to_owned(),
        "mysql -u root -p -h db".to_owned(),
        "mysql -uroot -proot app".to_owned(),
        "docker login -u bob -p $REGISTRY_PASSWORD registry".to_owned(),
        "docker login -p 8080:80".to_owned(),
        format!("mysql app\nssh -p{chosen} host"),
        format!("mysql app\u{2028}ssh -p{chosen} host"),
        format!("mysql app\x0Bssh -p{chosen} host"),
        format!("mysql app\u{85}ssh -p{chosen} host"),
        format!("mysql app | ssh -p{chosen} host"),
        format!("mysql app\\nssh -p{chosen} host"),
        format!("mysql app\\u000assh -p{chosen} host"),
        format!("docker login registry\u{2028}ssh -p {chosen} host"),
        format!("docker login registry; ssh -p {chosen} host"),
        format!("xmysql -p{chosen}"),
    ] {
        assert_exact(&input);
    }
}

#[test]
fn a_client_option_is_read_within_the_bounds_of_its_rule() {
    let chosen = chosen();
    let gap = |length: usize| format!("docker login {} -p {chosen}", "x".repeat(length - 1));
    assert_redacts(&gap(256), &chosen, PASSWORD);
    assert_exact(&gap(257));
    let blanks = |before: usize, after: usize| {
        format!(
            "sshpass{}-p{}{chosen} ssh host",
            " ".repeat(before),
            " ".repeat(after)
        )
    };
    assert_redacts(&blanks(8, 8), &chosen, PASSWORD);
    assert_exact(&blanks(9, 1));
    assert_exact(&blanks(1, 9));
    assert_exact(&format!("docker login{}-p {chosen}", " ".repeat(300)));
}

#[test]
fn private_key_body_lines_are_redacted_under_a_header_without_a_footer() {
    let (header, line) = (private_key_header(), private_key_body_line());
    let short = &line[..12];
    for (input, body) in [
        (
            format!("{header}\n{line}\n{line}\n"),
            format!("{line}\n{line}"),
        ),
        (
            format!("{header}\n{line}\n{short}\nnext line"),
            format!("{line}\n{short}"),
        ),
        (
            format!("{header}  \r\n{line}  \r\n{line}\r\n"),
            format!("{line}  \r\n{line}"),
        ),
        (
            format!("{header}\\n{line}\\n{line}\\n"),
            format!("{line}\\n{line}"),
        ),
        (
            format!("{header}\\u000a{line}\\u000a{line}\\u000a"),
            format!("{line}\\u000a{line}"),
        ),
        (
            format!("{{\"output\":\"{header}\\n{line}\\n{short}\"}}"),
            format!("{line}\\n{short}"),
        ),
        (
            format!(
                "{header}\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-256-CBC,00\n\n  {line}\n  {line}\n"
            ),
            format!("{line}\n  {line}"),
        ),
    ] {
        assert_redacts(&input, &body, "<REDACTED:private_key>");
    }
    assert_exact(&format!("{header}\n...\n"));
    assert_exact(&format!("{header}\nthe key body goes here\n"));
    let followed = format!("{header}\n{line}\nplain text follows\n");
    assert_eq!(
        redact_durable_text(&followed).text,
        format!("{header}\n<REDACTED:private_key>\nplain text follows\n")
    );
}

#[test]
fn a_finding_of_an_added_rule_merges_with_the_finding_it_overlaps() {
    let (issued, chosen) = (issued(), chosen());
    for (input, value, placeholder) in [
        (
            format!(
                "MY_SECRET={}",
                url("https", "alice", &chosen, "db.internal/x")
            ),
            url("https", "alice", &chosen, "db.internal/x"),
            "<REDACTED:secret>",
        ),
        (
            format!("dbpassword=\"{chosen} two\" run"),
            format!("\"{chosen} two\""),
            "<REDACTED:secret>",
        ),
        (
            format!("api_key: sk-ant-api03-{issued}{issued}"),
            format!("sk-ant-api03-{issued}{issued}"),
            "<REDACTED:api_key>",
        ),
    ] {
        assert_redacts(&input, &value, placeholder);
    }
}

fn text_with_line_at(target_offset: usize, line: &str, total: usize) -> (String, usize) {
    const FILLER: &str = "plain filler line with nothing to report 0123\n";
    let mut text = String::with_capacity(total + line.len() + 2 * FILLER.len());
    while text.len() + 2 * FILLER.len() <= target_offset {
        text.push_str(FILLER);
    }
    text.push_str(&"x".repeat(target_offset - text.len() - 1));
    text.push('\n');
    let offset = text.len();
    assert_eq!(offset, target_offset);
    text.push_str(line);
    text.push('\n');
    while text.len() < total {
        text.push_str(FILLER);
    }
    (text, offset)
}

#[test]
fn windowed_redaction_agrees_with_the_redaction_of_a_line_alone_at_every_window_edge() {
    const FILLER: &str = "plain filler line with nothing to report 0123\n";
    let (issued, chosen) = (issued(), chosen());
    let lines = [
        format!("deploy --password {chosen} --verbose"),
        format!(
            "deploy --password \"{chosen} {}\" --verbose",
            "two ".repeat(200)
        ),
        format!("deploy --password {chosen}\\ {}", "a9\\ ".repeat(300)),
        format!("  password: {chosen}  # note"),
        format!("password: {chosen}{}", " ".repeat(3000)),
        format!("password: '{chosen} {}'", "two ".repeat(200)),
        format!("out\\npassword: {chosen}\\nuser: bob"),
        format!("X-Api-Key: {issued}"),
        format!("PGPASSWORD={chosen} psql -h host"),
        format!("password={}", digits()),
        url("https", "alice", &chosen, "example.org/r.git"),
        format!("mysql -uroot -p{chosen} app"),
        format!("sshpass -p {chosen} ssh deploy@host"),
        format!("docker login {} -p {chosen}", "x".repeat(200)),
        format!("Authorization: Basic {}", basic_credentials()),
        format!(
            "{}\n{}",
            private_key_header(),
            [private_key_body_line(), private_key_body_line()].join("\n")
        ),
        format!("password={issued}"),
        format!("DB_PASSWORD=\n{issued}"),
    ];
    let window = MAX_REDACTABLE_BYTES;
    let (mut left_to_second, mut kept_by_first) = (0, 0);
    for line in &lines {
        let alone = redact_durable_text(&format!("{FILLER}{line}\n{FILLER}"));
        assert!(!alone.detections.is_empty(), "{line:?}");
        let redacted = &alone.text[FILLER.len()..alone.text.len() - FILLER.len() - 1];
        let size = line.len();
        for edge in [
            window - EDGE_MARGIN_BYTES,
            window,
            window - WINDOW_OVERLAP_BYTES,
            window - WINDOW_OVERLAP_BYTES + EDGE_MARGIN_BYTES,
        ] {
            for offset in [
                edge - size - FILLER.len(),
                edge - size - 2,
                edge - size,
                edge - size / 2,
                edge - 7,
                edge - 1,
                edge,
                edge + 1,
                edge + FILLER.len(),
            ] {
                let (text, start) = text_with_line_at(offset, line, window + window / 2);
                let (second_start, _) = scan_windows(&text).nth(1).unwrap();
                if start + size > window - EDGE_MARGIN_BYTES {
                    left_to_second += 1;
                } else if start < second_start + EDGE_MARGIN_BYTES {
                    kept_by_first += 1;
                }
                let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
                assert_eq!(
                    windowed.text,
                    text.replacen(line.as_str(), redacted, 1),
                    "{line:?} at {offset}"
                );
            }
        }
    }
    assert!(left_to_second >= lines.len() && kept_by_first >= lines.len());
}

#[test]
fn a_quoted_word_cut_by_a_window_end_is_replaced_whole_or_refused() {
    let chosen = chosen();
    let window = MAX_REDACTABLE_BYTES;
    let quoted = |bytes: usize| format!("\"{chosen} {}\"", "two ".repeat(bytes / 4));
    let fits = quoted(20 * 1024);
    let line = format!("deploy --password {fits} --verbose");
    for cut_into_word in [1usize, 8, 1024, 19 * 1024] {
        let (text, start) = text_with_line_at(window - 18 - cut_into_word, &line, 2 * window);
        assert!(start + 18 < window && start + line.len() > window);
        let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
        assert_eq!(windowed.text, text.replacen(fits.as_str(), PASSWORD, 1));
        assert_eq!(windowed.detections.len(), 1);
    }
    let long = format!("deploy --password {} --verbose", quoted(40 * 1024));
    assert_eq!(
        redactor().unwrap().redact(&long).unwrap_err().kind(),
        RedactionErrorKind::MatchLimit
    );
    for cut_into_word in [1usize, 1024, 30 * 1024] {
        let (text, _) = text_with_line_at(window - 18 - cut_into_word, &long, 2 * window);
        assert_eq!(
            redact_windowed_durable_text(&text, usize::MAX)
                .unwrap_err()
                .kind(),
            RedactionErrorKind::MatchLimit
        );
        assert_ne!(detect_windowed_durable_text(&text), Ok(false));
    }
}

fn filled(unit: &str, total: usize) -> String {
    unit.repeat(total / unit.len())
}

fn adversarial_inputs(total: usize) -> Vec<(&'static str, String)> {
    let body_line = format!("{}\n", private_key_body_line());
    vec![
        ("option without a value", filled("--password \t", total)),
        ("option after option", filled("--token --token a ", total)),
        (
            "option before an open quote",
            format!("--password \"{}", filled("a1 ", total - 16)),
        ),
        (
            "options before escaped quotes",
            filled("--password \"a\\\" ", total),
        ),
        (
            "options before serialized quotes",
            filled("--password \\\"a\\\\\\\" ", total),
        ),
        (
            "options before open quotes of each kind",
            filled("--token 'a --token `b --token \"c ", total),
        ),
        (
            "escaped blanks",
            format!("--password {}", filled("a\\ ", total - 16)),
        ),
        ("program without an option", filled("mysql ", total)),
        (
            "program before a long gap",
            filled(&format!("mysql {} ", "x".repeat(300)), total),
        ),
        ("login without an option", filled("docker login y ", total)),
        ("authority without userinfo", filled("x://u:p", total)),
        (
            "authority of colons",
            format!("x://u:{}", filled("p:", total - 8)),
        ),
        (
            "field word that runs to the end",
            format!("token: {}", filled("a1", total - 16)),
        ),
        ("fields on one line", filled("token: a1 ", total)),
        ("serialized fields", filled("token: a1\\n", total)),
        (
            "dotted key prefix",
            format!("{}token: x1 y", filled("a.", total - 16)),
        ),
        ("suffix keys", filled("apassword=", total)),
        (
            "digits before a long word",
            filled(&["password=", "1234a "].concat(), total),
        ),
        (
            "private key body",
            format!(
                "{}\n{}",
                private_key_header(),
                filled(&body_line, total - 64)
            ),
        ),
        (
            "private key headers",
            filled(&format!("{}\n", private_key_header()), total),
        ),
        (
            "scheme without credentials",
            filled("Authorization: Basic ", total),
        ),
    ]
}

#[test]
fn adversarial_input_costs_linear_work() {
    let redactor = Redactor::new().unwrap();
    let full = adversarial_inputs(MAX_REDACTABLE_BYTES);
    let half = adversarial_inputs(MAX_REDACTABLE_BYTES / 2);
    for ((name, full), (_, half)) in full.iter().zip(&half) {
        let full_report = redactor.scanner.scan(full).unwrap();
        let half_report = redactor.scanner.scan(half).unwrap();
        assert!(
            matches!(full_report.limits_hit, None | Some(LimitExhausted::Match)),
            "{name}: {:?}",
            full_report.limits_hit
        );
        assert!(
            full_report.work_bytes <= 2 * half_report.work_bytes + half_report.work_bytes / 8,
            "{name}: {} bytes of work for {} input bytes, {} for half",
            full_report.work_bytes,
            full.len(),
            half_report.work_bytes
        );
        match redactor.redact(full) {
            Ok(redaction) => assert!(redaction.text.len() <= 20 * full.len(), "{name}"),
            Err(error) => assert_eq!(error.kind(), RedactionErrorKind::MatchLimit, "{name}"),
        }
    }
}
