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
            format!("deploy --password --token {issued}"),
            issued.clone(),
            "<REDACTED:token>",
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
        (
            format!("{{\"command\":\"deploy --password {chosen}\",\"example\":true}}"),
            format!("{chosen}\",\"example\":true}}"),
            PASSWORD,
        ),
        (
            format!("{{\"cmd\":\"deploy --password {chosen}\",\"cwd\":\"${{workspaceFolder}}\"}}"),
            format!("{chosen}\",\"cwd\":\"${{workspaceFolder}}\"}}"),
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
        "deploy --password --token".to_owned(),
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
            format!("secret: password: {chosen}"),
            chosen.clone(),
            PASSWORD,
        ),
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
            format!("password: \"{chosen}"),
            format!("\"{chosen}"),
            PASSWORD,
        ),
        (
            format!("password: {chosen}\\ two"),
            format!("{chosen}\\ two"),
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
fn each_of_two_adjacent_credentials_is_replaced() {
    let (first, second) = (chosen(), issued());
    for (input, expected) in [
        (
            format!("username: bob\\ntoken: {first}\\npassword: {second}\\nhost: db"),
            "username: bob\\ntoken: <REDACTED:token> <REDACTED:password> db".to_owned(),
        ),
        (
            format!("token: $TOKEN\\npassword: {second}\\nhost: db"),
            "token: $TOKEN\\npassword: <REDACTED:password> db".to_owned(),
        ),
        (
            format!("run\\n--token {first}\\n--password {second}\\n--verbose"),
            "run\\n--token <REDACTED:token> <REDACTED:password>".to_owned(),
        ),
        (
            format!("mysql -uroot -p{first}\\nmysql -uroot -p{second} app"),
            "mysql -uroot -p<REDACTED:password> -uroot -p<REDACTED:password> app".to_owned(),
        ),
        (
            format!("Use `mysql -p{first} app` or `mysqldump -p{second} app > dump.sql`"),
            format!("Use `mysql -p{PASSWORD} app` or `mysqldump -p{PASSWORD} app > dump.sql`"),
        ),
        (
            format!("mysql -p{first} app < a.sql  # then: mysql -p{second} app < b.sql"),
            format!("mysql -p{PASSWORD} app < a.sql  # then: mysql -p{PASSWORD} app < b.sql"),
        ),
        (
            format!("password: {first}\ntoken: {second}\n"),
            "password: <REDACTED:password>\ntoken: <REDACTED:token>\n".to_owned(),
        ),
    ] {
        assert_eq!(redact_durable_text(&input).text, expected, "{input:?}");
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
        format!("password: {chosen}\\ #two and more words"),
        format!("password: \"{chosen}\"\"\\ntwo three\" and more words"),
        format!("the tokens: {chosen}"),
        format!("out\\Npassword: {chosen}"),
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
        (
            "deploy --password {} --verbose",
            ["'A9\\'", " B8'"].concat(),
        ),
        (
            "deploy --password {} --verbose",
            ["\"A9\\\"", " B8\""].concat(),
        ),
        ("deploy --password {} --verbose", ["12;", "ab!9"].concat()),
        (
            "deploy --password {} --verbose",
            ["p\u{e4}ss", "\u{20ac}w\u{f6}rd9"].concat(),
        ),
        (
            "deploy --password {} --verbose",
            ["Zx9\\\\n", "kq7"].concat(),
        ),
        (
            "deploy --password {} --verbose",
            ["Summer", "\u{a3}"].concat(),
        ),
        (
            "deploy --password {} --verbose",
            ["<a>Zx9", "!kq7"].concat(),
        ),
    ] {
        assert_redacts(&template.replace("{}", &word), &word, PASSWORD);
    }
}

#[test]
fn a_gate_reads_the_text_the_word_reader_delimits() {
    let chosen = chosen();
    let (head, tail) = chosen.split_at(3);
    for value in [
        format!("\"{head}\\n{tail}\""),
        format!("{head}\",{tail}"),
        format!("1433;database=sales;{chosen}"),
        format!("[{chosen}>"),
        format!("{{{chosen}]"),
    ] {
        assert_redacts(
            &format!("deploy --password {value} --yes"),
            &value,
            PASSWORD,
        );
    }
    for line_break in ["\\n", "\\r\\n", "\\u000a", "\\u000D"] {
        let lines = format!("PGPASSWORD={head}{line_break}DBPASSWORD={chosen} psql");
        assert_eq!(
            redact_durable_text(&lines).text,
            format!("PGPASSWORD={PASSWORD}{line_break}DBPASSWORD={PASSWORD} psql")
        );
        let field = format!("token: {chosen} {line_break}next: 1");
        assert_redacts(&field, &chosen, "<REDACTED:token>");
    }
    let twice = format!("user: bob\\\\npassword: {chosen}\\\\nhost: db");
    let word = format!("{chosen}\\\\nhost:");
    assert_redacts(&twice, &word, PASSWORD);
    assert_redacts(
        &format!("{{\"stdout\":\"user: bob\\npassword: {chosen} \\nhost: db\"}}"),
        &chosen,
        PASSWORD,
    );
    assert_exact(&format!(
        "{{\"stdout\": \"password: {chosen}\", \"exit_code\": 0}}"
    ));
    assert_exact("{\"content\":\"    token: u64,\"}");
    assert_exact("token: \"\\n\"");
    for (input, value) in [
        (
            format!("mysqldump -uroot -p{chosen}>${{BACKUP_DIR}}/app.sql"),
            format!("{chosen}>${{BACKUP_DIR}}/app.sql"),
        ),
        (
            format!("mysql -uroot -p{chosen}<insert_users.sql"),
            format!("{chosen}<insert_users.sql"),
        ),
        (
            format!("PGPASSWORD={chosen}&&./example.sh"),
            format!("{chosen}&&./example.sh"),
        ),
        (
            format!("deploy --password {chosen}\\texample.org"),
            format!("{chosen}\\texample.org"),
        ),
        (
            format!("deploy --password 12;{chosen} now"),
            format!("12;{chosen}"),
        ),
    ] {
        assert_redacts(&input, &value, PASSWORD);
    }
    for input in [
        "deploy --password ${PASS}>out now",
        "mysql -uroot -p$MYSQL_PWD<in.sql",
        "deploy --password changeme;x9 now",
        "PGPASSWORD=changeme&&./run9.sh",
    ] {
        assert_exact(input);
    }
    for quoted in [format!("\"[{chosen}>\""), format!("\"{{{chosen}}}\"")] {
        assert_redacts(&format!("password: {quoted}"), &quoted, PASSWORD);
    }
    let bracketed = format!("<{chosen}>");
    assert_redacts(
        &format!("deploy --password {bracketed} --yes"),
        &bracketed,
        PASSWORD,
    );
    for input in [
        "deploy --password 1433;database=sales;encrypt=true --yes",
        "deploy --password <a{b}> --yes",
    ] {
        assert_exact(input);
    }
    assert_redacts(&format!("x\\ntoken: {chosen}"), &chosen, "<REDACTED:token>");
}

#[test]
fn a_form_reads_its_stated_bounds_and_spellings() {
    let chosen = chosen();
    let (header, line) = (private_key_header(), private_key_body_line());
    let slot = format!("<{chosen}>example.sql");
    assert_redacts(&format!("mysql -uroot -p{slot}"), &slot, PASSWORD);
    let blanks = |count: usize| format!("sshpass -p{}{chosen} ssh host", "\u{a0}".repeat(count));
    assert_redacts(&blanks(8), &chosen, PASSWORD);
    assert_exact(&blanks(9));
    let ends_in_backslashes = format!("{}\\\\", ["Sum", "mer"].concat());
    assert_redacts(
        &format!("deploy --password {ends_in_backslashes} --yes"),
        &ends_in_backslashes,
        PASSWORD,
    );
    let continued = format!("{}\\\n  --yes", ["sum", "mer"].concat());
    assert_exact(&format!("deploy --password {continued}"));
    let list = "'1433;database=sales;;encrypt=true'";
    assert_redacts(&format!("deploy --password {list} --yes"), list, PASSWORD);
    assert_redacts(
        &format!("user: bob\\\\npassword: {chosen} \\\\nhost: db"),
        &chosen,
        PASSWORD,
    );
    for block in [
        format!("{header}\nComment: C:\\keys\\id_rsa\n\n{line}\n"),
        format!("{header}\\r\\u000a{line}\\u000d\\n"),
        format!("{header}\r\\n{line}\n"),
    ] {
        assert_redacts(&block, &line, "<REDACTED:private_key>");
    }
    let name = &line[..16];
    assert_redacts(
        &format!("{header}\n{name}: [output clipped]\n"),
        name,
        "<REDACTED:private_key>",
    );
    let in_array = format!("[{{\"command\":\"deploy --password {chosen}\"}},{{\"example\":true}}]");
    let word = format!("{chosen}\"}},{{\"example\":true}}]");
    assert_redacts(&in_array, &word, PASSWORD);
    let records = format!("[{{\"text\":\"password: {chosen}\"}},{{\"text\":\"ok\"}}]");
    let word = format!("{chosen}\"}},{{\"text\":\"ok\"}}]");
    assert_redacts(&records, &word, PASSWORD);
    let next_line = ["DBPASSWORD", "example"].join("=");
    let quoted = format!("\"{chosen}\\n{next_line}\"");
    assert_redacts(&format!("PGPASSWORD={quoted} psql"), &quoted, PASSWORD);
    let digits = digits();
    for line_break in ["\\n", "\\r\\n", "\\u000a", "\\u000d"] {
        let serialized = format!("{{{line_break}  \\\"password\\\": {digits}{line_break}}}");
        assert_redacts(&serialized, &digits, PASSWORD);
    }
    let twice = format!("{digits}\\\\nnext");
    assert_redacts(&format!("password: {twice}"), &twice, PASSWORD);
    let closers = |count: usize| format!("password={digits}{} next", ")".repeat(count));
    assert_redacts(&closers(1007), &digits, PASSWORD);
    assert_exact(&closers(1008));
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
        (format!("x\\nPGPASSWORD={chosen}\\npsql"), chosen.clone()),
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
        ("user=bob\\npassword={} host=db", PASSWORD),
    ] {
        assert_redacts(&template.replace("{}", &digits), &digits, placeholder);
    }
    for (template, placeholder) in [
        ("password=\"{}\"", PASSWORD),
        ("password:\"{}\"", PASSWORD),
        ("{\"client_secret\":\"{}\"}", "<REDACTED:client_secret>"),
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
    let at_first = format!("@{chosen}");
    assert_redacts(
        &url("https", "alice", &at_first, "db.internal/"),
        &at_first,
        "<REDACTED:url_password>",
    );
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
    let shortest = ["YT", "p4"].concat();
    assert_redacts(
        &format!("authorization: basic {shortest}"),
        &shortest,
        "<REDACTED:basic>",
    );
    assert_exact(&format!("x\\NAuthorization: Basic {basic}"));
    assert_redacts(
        &format!("Authorization: Basic\nAuthorization: Basic {basic}"),
        &basic,
        "<REDACTED:basic>",
    );
    assert_redacts(
        &format!("Authorization: Basic Authorization: Basic {basic}"),
        &basic,
        "<REDACTED:basic>",
    );
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
        (format!("./mysql -p{chosen} app"), chosen.clone()),
        (format!("mysql.exe -p{chosen} app"), chosen.clone()),
        (
            format!("podman  auth   login -p {chosen} registry"),
            chosen.clone(),
        ),
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
            format!("{{\"cmd\":\"mysql -uroot -p{chosen}\",\"mask\":\"***\"}}"),
            format!("{chosen}\",\"mask\":\"***\"}}"),
        ),
        (
            format!("cd /srv\\nmysql -uroot -p{chosen} app"),
            chosen.clone(),
        ),
        (
            format!("mysql {} -p{chosen} app", "x".repeat(200)),
            chosen.clone(),
        ),
        (
            format!("docker exec db mysql -p{chosen} app -e 'show tables' -prefix"),
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
    let attached = |units: usize| format!("mysql {} -p{chosen}", "x".repeat(units));
    assert_redacts(&attached(255), &chosen, PASSWORD);
    assert_exact(&attached(256));
    let pairs = |units: usize| format!("docker login {} -p {chosen}", "\\x".repeat(units - 1));
    assert_redacts(&pairs(256), &chosen, PASSWORD);
    assert_exact(&pairs(257));
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
    let (sixteen, fifteen) = (&line[..16], &line[..15]);
    assert_redacts(
        &format!("{header}\n{sixteen}\n{fifteen}\n"),
        &format!("{sixteen}\n{fifteen}"),
        "<REDACTED:private_key>",
    );
    assert_exact(&format!("{header}\n{fifteen}\n{fifteen}\n"));
    assert_exact(&format!("{header}\\N{line}\\N{line}"));
    let lines = |count: usize| {
        format!(
            "{header}\n{}{line}\n{line}\n",
            "Comment: armor\n".repeat(count)
        )
    };
    assert_redacts(
        &lines(4),
        &format!("{line}\n{line}"),
        "<REDACTED:private_key>",
    );
    assert_exact(&lines(5));
    let indented = |blanks: usize| format!("{header}\n{}{line}\n", " ".repeat(blanks));
    assert_redacts(&indented(256), &line, "<REDACTED:private_key>");
    assert_exact(&indented(257));
    let last = |blanks: usize| format!("{header}\n{line}\n{short}{}\n", " ".repeat(blanks));
    assert_redacts(
        &last(256),
        &format!("{line}\n{short}"),
        "<REDACTED:private_key>",
    );
    assert_redacts(&last(257), &line, "<REDACTED:private_key>");
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
    for (input, value, placeholder, rules) in [
        (
            format!(
                "MY_SECRET={}",
                url("https", "alice", &chosen, "db.internal/x")
            ),
            url("https", "alice", &chosen, "db.internal/x"),
            "<REDACTED:secret>",
            ["magic-keyed-assignment", "magic-value-url-userinfo"],
        ),
        (
            format!("dbpassword=\"{chosen} two\" run"),
            format!("\"{chosen} two\""),
            "<REDACTED:secret>",
            [
                "magic-keyed-assignment-double-quoted",
                "magic-value-password-assignment",
            ],
        ),
        (
            format!("api_key: sk-ant-api03-{issued}{issued}"),
            format!("sk-ant-api03-{issued}{issued}"),
            "<REDACTED:api_key>",
            ["magic-anthropic-api-key", "magic-value-field"],
        ),
        (
            format!("deploy --token {chosen}\\nDB_PASSWORD={issued} now"),
            format!("{chosen}\\nDB_PASSWORD={issued}"),
            "<REDACTED:token>",
            ["magic-keyed-assignment", "magic-value-option"],
        ),
        (
            format!("token: {chosen}\\nDB_PASSWORD={issued}"),
            format!("{chosen}\\nDB_PASSWORD={issued}"),
            "<REDACTED:token>",
            ["magic-keyed-assignment", "magic-value-field"],
        ),
    ] {
        assert_redacts(&input, &value, placeholder);
        let findings = redactor().unwrap().scanner.scan(&input).unwrap().findings;
        for rule in rules {
            assert!(
                findings.iter().any(|finding| finding.rule_id == rule),
                "{rule} on {input:?}: {findings:?}"
            );
        }
    }
}

#[test]
fn a_vendor_token_in_an_added_form_keeps_its_vendor_label() {
    let issued = issued();
    let vendor = format!("sk-ant-api03-{issued}{issued}");
    for (input, rule) in [
        (
            format!(
                "git clone {}",
                url("https", "alice", &vendor, "example.org/r.git")
            ),
            "magic-value-url-userinfo",
        ),
        (
            format!("PGPASSWORD={vendor} psql"),
            "magic-value-password-assignment",
        ),
        (
            format!("sshpass -p {vendor} ssh host"),
            "magic-value-client-option",
        ),
    ] {
        let redaction = redact_durable_text(&input);
        assert_eq!(
            redaction.text,
            input.replacen(&vendor, "<ANTHROPIC_API_KEY_REDACTED>", 1)
        );
        assert_eq!(redaction.detections.len(), 1, "{input:?}");
        assert_eq!(redaction.detections[0].secret_type, "anthropic_api_key");
        let findings = redactor().unwrap().scanner.scan(&input).unwrap().findings;
        for rule in ["magic-anthropic-api-key", rule] {
            assert!(
                findings.iter().any(|finding| finding.rule_id == rule),
                "{rule} on {input:?}: {findings:?}"
            );
        }
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
        for secret in [
            chosen.as_str(),
            issued.as_str(),
            "two",
            "a9",
            &digits(),
            &basic_credentials(),
            &private_key_body_line(),
        ] {
            assert!(!redacted.contains(secret), "{line:?} keeps {secret:?}");
        }
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

#[test]
fn a_chain_of_quoted_option_words_is_replaced_the_same_in_every_window() {
    let chosen = chosen();
    let head = format!("--password \"{chosen} B8! ");
    let unit = format!("{head}{} ", "q".repeat(1023 - head.len()));
    assert_eq!(unit.len(), 1024);
    let line = unit.repeat(320);
    let alone = redactor().unwrap().redact(&line).unwrap();
    assert!(!alone.text.contains(&chosen));
    assert_eq!(alone.text.matches("B8!").count(), 1);
    for filler_bytes in [201 * 1024, 201 * 1024 + 517, 230 * 1024, 193 * 1024 + 3] {
        let filler = format!("{}\n", "f".repeat(filler_bytes - 1));
        let text = format!("{filler}{line}\n");
        assert!(text.len() > MAX_REDACTABLE_BYTES);
        let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
        assert_eq!(
            windowed.text,
            format!("{filler}{}\n", alone.text),
            "{filler_bytes}"
        );
    }
}

#[test]
fn a_header_without_a_value_leaves_the_next_header_to_its_own_match_in_every_window() {
    let basic = basic_credentials();
    let start = MAX_REDACTABLE_BYTES - WINDOW_OVERLAP_BYTES;
    let empty = format!("Authorization: Basic{}", "\n".repeat(16 * 1024 - 20));
    let text = format!(
        "{}\n{}Authorization: Basic {basic}",
        "x".repeat(start - 16 * 1024 - 1),
        empty.repeat(8)
    );
    assert!(text.len() > MAX_REDACTABLE_BYTES);
    let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
    assert_eq!(windowed.text, text.replacen(&basic, "<REDACTED:basic>", 1));
    assert_eq!(windowed.detections.len(), 1);
}

#[test]
fn headers_without_a_value_on_one_line_leave_the_last_header_to_its_own_match_in_every_window() {
    let basic = basic_credentials();
    let empty = "Authorization: Basic ";
    for headers in [1023usize, 1024] {
        let line = format!("{}{empty}{basic}", empty.repeat(headers));
        let alone = redact_durable_text(&line);
        assert_eq!(alone.text, line.replacen(&basic, "<REDACTED:basic>", 1));
        for into_line in [0usize, 7, 10 * 1024 + 3, 10 * 1024 + 4] {
            let start = MAX_REDACTABLE_BYTES - WINDOW_OVERLAP_BYTES - into_line;
            let (text, at) = text_with_line_at(start, &line, 2 * MAX_REDACTABLE_BYTES);
            assert_eq!(at, start);
            let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
            assert_eq!(windowed.text, text.replacen(&basic, "<REDACTED:basic>", 1));
            assert_eq!(windowed.detections.len(), 1);
        }
    }
}

#[test]
fn a_key_body_before_a_long_blank_run_is_replaced_the_same_in_every_window() {
    let (header, line) = (private_key_header(), private_key_body_line());
    let blanks = " ".repeat(18 * 1024);
    for block in [
        format!("{header}\n{line}{blanks}\nSaved to disk"),
        format!("{header}\n{line}\n{blanks}Saved to disk"),
    ] {
        let alone = redact_durable_text(&block);
        assert_eq!(
            alone.text,
            block.replacen(&line, "<REDACTED:private_key>", 1)
        );
        let cut = block.find("Saved").unwrap() + 2;
        for past_the_edge in [0usize, 1, 13] {
            let start = MAX_REDACTABLE_BYTES - cut - past_the_edge;
            let (text, at) = text_with_line_at(start, &block, 2 * MAX_REDACTABLE_BYTES);
            assert_eq!(at, start);
            let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
            assert_eq!(
                windowed.text,
                text.replacen(&line, "<REDACTED:private_key>", 1)
            );
            assert_eq!(windowed.detections.len(), 1);
            assert_eq!(detect_windowed_durable_text(&text), Ok(true));
        }
    }
}

#[test]
fn a_field_inside_a_quoted_field_word_is_replaced_the_same_in_every_window() {
    let chosen = chosen();
    let line = format!(
        "password: \"wrap password: {chosen} # x\"{}",
        " ".repeat(18 * 1024)
    );
    let alone = redact_durable_text(&format!("{line}tail\n"));
    assert!(!alone.text.contains(&chosen));
    let before = MAX_REDACTABLE_BYTES - line.len();
    let text = format!("{}\n{line}tail\n", "x".repeat(before - 1));
    assert!(text.len() > MAX_REDACTABLE_BYTES);
    let windowed = redact_windowed_durable_text(&text, usize::MAX).unwrap();
    assert_eq!(
        windowed.text,
        format!("{}\n{}", "x".repeat(before - 1), alone.text)
    );
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
            "suffix keys with escaped blanks",
            filled("PGPASSWORD=$X\\ ", total),
        ),
        (
            "suffix keys on serialized lines",
            filled("PGPASSWORD=$X\\n", total),
        ),
        (
            "suffix keys after unicode escapes",
            filled("aPASSWORD=\\u0041", total),
        ),
        (
            "suffix keys after cut unicode escapes",
            filled("aPASSWORD=\\u00\\x", total),
        ),
        (
            "key header lines of blanks",
            filled(
                &format!("{}\nA:{}\n", private_key_header(), " ".repeat(200)),
                total,
            ),
        ),
        (
            "private key header lines",
            format!(
                "{}\n{}",
                private_key_header(),
                filled("Name: value\n", total - 64)
            ),
        ),
        (
            "headers on one line",
            filled("Authorization: Basic Authorization ", total),
        ),
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
