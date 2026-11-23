//! Differential test of the embedded overlay against the overlay of `main`.
//!
//! The durable-text redactor replaces or refuses what the scanner reports, so a span that
//! `main`'s rules report and the embedded rules leave out is text that reaches storage.
//! The embedded overlay is `main`'s document followed by added rules, and the evaluator
//! evaluates each rule on its own, so the embedded rule set reports every finding `main`'s
//! rule set reports. This module checks each link of that argument:
//!
//! - the embedded document begins with `main`'s document byte for byte, and each rule of
//!   `main`'s document compiles to the same rule in both sets;
//! - on inputs recorded from a build of `main`, `main`'s document under this evaluator
//!   reports exactly what that build reported;
//! - on those inputs and on generated ones, every finding of `main`'s rule set is a finding
//!   of the embedded rule set, with the same rule, spans, and key.

use std::sync::LazyLock;

use super::{
    CONSERVATIVE_OVERLAY_SHA256, OVERLAY_BYTES, Rule, RuleSet, UPSTREAM_BYTES,
    UPSTREAM_CORPUS_SHA256, digest_hex,
};
use crate::evaluator::evaluate;
use crate::{Finding, RuleSource, ScanLimits, ScanProfile, ScanReport};

/// `conservative_overlay.yaml` of `main` at the merge base, byte for byte.
const BASE_OVERLAY: &[u8] = include_bytes!("../../testdata/conservative_overlay_base.yaml");
/// The digest `main` pins for that document in `CONSERVATIVE_OVERLAY_SHA256`.
const BASE_OVERLAY_SHA256: &str =
    "973181a0af049fb4c0ae06160cd022b1beae3660b87ac9fa4d498864912b3487";

/// The rules the embedded overlay adds to `main`'s document.
const ADDED_RULES: [&str; 10] = [
    "magic-basic-authorization",
    "magic-client-password-attached",
    "magic-client-password-option",
    "magic-field-value",
    "magic-field-value-serialized",
    "magic-option-value",
    "magic-password-suffix-assignment",
    "magic-private-key-body",
    "magic-secret-digits",
    "magic-url-userinfo",
];

static BASE: LazyLock<RuleSet> = LazyLock::new(|| {
    RuleSet::from_sources(
        UPSTREAM_BYTES,
        UPSTREAM_CORPUS_SHA256,
        BASE_OVERLAY,
        BASE_OVERLAY_SHA256,
    )
    .expect("main's rule documents are valid")
});

static EMBEDDED: LazyLock<RuleSet> =
    LazyLock::new(|| RuleSet::from_embedded().expect("embedded rules are valid"));

const PROFILES: [ScanProfile; 2] = [ScanProfile::Conservative, ScanProfile::Comprehensive];

fn scan(rules: &RuleSet, profile: ScanProfile, input: &str) -> ScanReport {
    let limits = ScanLimits::default();
    let digest = rules.semantic_digest(profile, limits).unwrap();
    evaluate(rules, profile, limits, digest, input).unwrap()
}

/// Every finding of `main`'s rule set on `input` is a finding of the embedded rule set.
///
/// A scan that stops at a limit is refused by the redactor whatever it found, so a limit
/// the embedded rules reach where `main`'s rules complete also keeps the text out of
/// storage; the generated inputs are far below every limit and assert that none is hit.
fn assert_embedded_reports_every_base_finding(input: &str) {
    for profile in PROFILES {
        let base = scan(&BASE, profile, input);
        let embedded = scan(&EMBEDDED, profile, input);
        assert_eq!(base.limits_hit, None, "{profile:?} {input:?}");
        assert_eq!(embedded.limits_hit, None, "{profile:?} {input:?}");
        for finding in &base.findings {
            assert!(
                embedded.findings.contains(finding),
                "{profile:?}: the embedded rules leave out {finding:?} on {input:?}"
            );
        }
        // An added rule is the only source of a finding outside `main`'s report.
        for finding in &embedded.findings {
            assert!(
                base.findings.contains(finding) || ADDED_RULES.contains(&finding.rule_id.as_str()),
                "{profile:?}: {finding:?} on {input:?} comes from a rule of main's document"
            );
        }
    }
}

#[test]
fn the_embedded_overlay_begins_with_the_document_of_main() {
    assert_eq!(digest_hex(BASE_OVERLAY), BASE_OVERLAY_SHA256);
    assert!(
        OVERLAY_BYTES.starts_with(BASE_OVERLAY),
        "the embedded overlay changes a byte of main's document"
    );
    assert_ne!(CONSERVATIVE_OVERLAY_SHA256, BASE_OVERLAY_SHA256);
}

/// The fields `compile_rule` derives from a declaration, which the evaluator reads.
fn compiled(rule: &Rule) -> impl PartialEq + std::fmt::Debug + '_ {
    (
        rule.source,
        rule.encoded_declaration.as_slice(),
        rule.regex.as_str(),
        rule.unnamed_captures.as_slice(),
        rule.required_byte,
        rule.quoted_capture,
        rule.confidence_bonus,
        rule.code_reference_gate,
        rule.declaration.credential_form,
    )
}

#[test]
fn every_rule_of_main_is_embedded_as_main_compiles_it() {
    let embedded: Vec<&Rule> = EMBEDDED.active(ScanProfile::Comprehensive).collect();
    let mut matched = 0;
    for base in BASE.active(ScanProfile::Comprehensive) {
        let twin = embedded
            .iter()
            .find(|rule| rule.declaration.name == base.declaration.name)
            .unwrap_or_else(|| panic!("{} is not embedded", base.declaration.name));
        assert_eq!(compiled(twin), compiled(base), "{}", base.declaration.name);
        assert_eq!(base.declaration.credential_form, None);
        matched += 1;
    }
    let mut added: Vec<&str> = embedded
        .iter()
        .filter(|rule| {
            !BASE
                .active(ScanProfile::Comprehensive)
                .any(|base| base.declaration.name == rule.declaration.name)
        })
        .map(|rule| {
            assert_eq!(rule.source, RuleSource::ConservativeOverlay);
            rule.declaration.name.as_str()
        })
        .collect();
    added.sort_unstable();
    assert_eq!(added, ADDED_RULES);
    assert_eq!(matched + added.len(), embedded.len());
}

struct Row {
    input: String,
    /// The rule and the value byte range of each finding `main`'s build reports for
    /// `input` under the conservative profile, in report order.
    main: Vec<(&'static str, usize, usize)>,
}

/// The fixtures assemble credential-shaped values at run time so each source literal contains only a fragment.
/// `fill` expands `{_N}` to N spaces.
fn fill(template: &str) -> String {
    let mut filled = template
        .replace("{w}", &["p@", "ss="].concat())
        .replace("{m}", &["Xy7Kp2", "Lm9Qa="].concat())
        .replace("{c}", &["hunter", "-two"].concat())
        .replace("{i}", &["Zq7Lm2Xv9Rt4", "Kp8Wn3Bc6Yd1"].concat())
        .replace("{p}", &["QUJDRDEyMzQ1", "Ng=="].concat())
        .replace("{s}", &["Zx9", "!kq7"].concat())
        .replace("{d}", &["1234", "5678"].concat())
        .replace("{b}", &["dXNlcjpaeDlr", "cTc="].concat())
        .replace("{h}", &["-----BEGIN RSA PRIVATE", " KEY-----"].concat())
        .replace("{k}", &["MIIEvQIBADANBgkq", "hkiG9w0BAQEFAASC"].concat())
        .replace("{q}", &["Pa55", "'!!"].concat())
        .replace("{x}", &["Q7r9$", "\\+v9"].concat())
        .replace("{y}", &["ab;", "cd9"].concat())
        .replace("{n}", &["12", "34"].concat())
        .replace("{t}", &["90", "0s"].concat());
    while let Some(start) = filled.find("{_") {
        let end = start + filled[start..].find('}').expect("a run closes with `}`");
        let count: usize = filled[start + 2..end]
            .parse()
            .expect("a run names its length");
        filled.replace_range(start..=end, &" ".repeat(count));
    }
    filled
}

fn row(input: &str, main: &[(&'static str, usize, usize)]) -> Row {
    Row {
        input: fill(input),
        main: main.to_vec(),
    }
}

/// The findings are those of the filled input, recorded from a build of `main` at the merge
/// base. The rows hold every form `main` reports, the words `main` reads as the value of an
/// empty assignment, the comparisons it reports, serialized text, and each form an added
/// rule reads.
fn rows() -> Vec<Row> {
    vec![
        row("password={c}", &[("magic-keyed-assignment", 9, 19)]),
        row("api_key = {i}", &[("magic-keyed-assignment", 10, 34)]),
        row("auth_token\t=\t{c}", &[("magic-keyed-assignment", 13, 23)]),
        row(
            "DB_PASSWORD\n={c} now",
            &[("magic-keyed-assignment", 13, 23)],
        ),
        row(
            "export SECRET_KEY={i} && run",
            &[("magic-keyed-assignment", 18, 42)],
        ),
        row("token={c};tail9", &[("magic-keyed-assignment", 6, 16)]),
        row(
            "Server=a;Password={c};Database=d",
            &[("magic-keyed-assignment", 18, 28)],
        ),
        row("password={c}&x9", &[("magic-keyed-assignment", 9, 19)]),
        row("password={c}$x9", &[("magic-keyed-assignment", 9, 19)]),
        row(
            "connect(password={c})",
            &[("magic-keyed-assignment", 17, 27)],
        ),
        row(
            "password=\"{c} two\"",
            &[("magic-keyed-assignment-double-quoted", 10, 24)],
        ),
        row(
            "password='{c}'",
            &[("magic-keyed-assignment-single-quoted", 10, 20)],
        ),
        row(
            "password = \"{c}\"",
            &[("magic-keyed-assignment-double-quoted", 12, 22)],
        ),
        row(
            "password =\n  \"{c}\"",
            &[("magic-keyed-assignment-double-quoted", 14, 24)],
        ),
        row(
            "{\"password\":\"{c}\"}",
            &[("magic-keyed-double-quoted", 13, 23)],
        ),
        row(
            "{\"clientSecret\": \"{i}\"}",
            &[("magic-keyed-double-quoted", 18, 42)],
        ),
        row(
            "'api_token': '{i}'",
            &[("magic-keyed-single-quoted", 14, 38)],
        ),
        row(
            "\"auth_key\": '{i}'",
            &[("magic-keyed-double-single", 13, 37)],
        ),
        row(
            "'auth_key': \"{i}\"",
            &[("magic-keyed-single-double", 13, 37)],
        ),
        row(
            "Authorization: Bearer {i}",
            &[("magic-bearer-token", 22, 46)],
        ),
        row("DB_PASSWORD= {c}", &[("magic-keyed-assignment", 13, 23)]),
        row(
            "DB_PASSWORD=\n{c}\nnext line",
            &[("magic-keyed-assignment", 13, 23)],
        ),
        row(
            "DB_PASSWORD=\r\n  {c}\r\n",
            &[("magic-keyed-assignment", 16, 26)],
        ),
        row("DB_PASSWORD =\n{c}", &[("magic-keyed-assignment", 14, 24)]),
        row(
            "DB_PASSWORD\n=\n{c}\n",
            &[("magic-keyed-assignment", 14, 24)],
        ),
        row(
            "msg=\"login password= {c}\" user=bob",
            &[("magic-keyed-assignment", 21, 31)],
        ),
        row("(password= {c})", &[("magic-keyed-assignment", 11, 21)]),
        row(
            "password= {c}; echo done",
            &[("magic-keyed-assignment", 10, 20)],
        ),
        row("password= {c};", &[("magic-keyed-assignment", 10, 20)]),
        row(
            "password= {c} # rotate",
            &[("magic-keyed-assignment", 10, 20)],
        ),
        row("password=\n{c}&x9", &[("magic-keyed-assignment", 10, 20)]),
        row("password=\n{c}|cat", &[("magic-keyed-assignment", 10, 20)]),
        row("password= {c},", &[("magic-keyed-assignment", 10, 21)]),
        row(
            "token= {c} \\\n  --flag",
            &[("magic-keyed-assignment", 7, 17)],
        ),
        row("api_key=\n{p}\n", &[("magic-keyed-assignment", 9, 25)]),
        row("api_key= {p}", &[("magic-keyed-assignment", 9, 25)]),
        row("password= {w}{c}", &[("magic-keyed-assignment", 10, 25)]),
        row("API_TOKEN=\n{c}>", &[("magic-keyed-assignment", 11, 21)]),
        row("password=\t{c}\t\n", &[("magic-keyed-assignment", 10, 20)]),
        row("(password= {c} )", &[("magic-keyed-assignment", 11, 21)]),
        row(
            "msg=\"password= {c} \"",
            &[("magic-keyed-assignment", 15, 25)],
        ),
        row("password= {c} ;", &[("magic-keyed-assignment", 10, 20)]),
        row("password= {c} | cat", &[("magic-keyed-assignment", 10, 20)]),
        row("password= {c} <", &[("magic-keyed-assignment", 10, 20)]),
        row(
            "password= {c}\u{2028}next line",
            &[("magic-keyed-assignment", 10, 20)],
        ),
        row(
            "token= {c} \\\\\\n  --flag",
            &[("magic-keyed-assignment", 7, 17)],
        ),
        row(
            "API_TOKEN=\n{m};tail9",
            &[("magic-keyed-assignment", 11, 23)],
        ),
        row("password={c};tail9;", &[("magic-keyed-assignment", 9, 19)]),
        row(
            "API_TOKEN=\nDB_PASSWORD= {c}",
            &[("magic-keyed-assignment", 11, 23)],
        ),
        row(
            "API_TOKEN=\nDB_PASSWORD=\ndb.password=\n{c}",
            &[
                ("magic-keyed-assignment", 11, 23),
                ("magic-keyed-assignment", 37, 47),
            ],
        ),
        row(
            "db.password =\nkeycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[("magic-keyed-assignment", 72, 82)],
        ),
        row(
            "API_TOKEN=\nself.token = api_token = {c}",
            &[("magic-keyed-assignment", 36, 46)],
        ),
        row(
            "db.password =\n#keycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[
                ("magic-keyed-assignment", 14, 27),
                ("magic-keyed-assignment", 73, 83),
            ],
        ),
        row(
            "db.password =\n\u{a0}keycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[("magic-keyed-assignment", 74, 84)],
        ),
        row(
            "db.password\u{a0}=\nkeycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[("magic-keyed-assignment", 73, 83)],
        ),
        row(
            "db.password =\n\u{2022} keycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[("magic-keyed-assignment", 14, 17)],
        ),
        row(
            "db.password =\n13\u{2192}keycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[
                ("magic-keyed-assignment", 14, 31),
                ("magic-keyed-assignment", 77, 87),
            ],
        ),
        row(
            "db.password =\n\\#keycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[
                ("magic-keyed-assignment", 14, 28),
                ("magic-keyed-assignment", 74, 84),
            ],
        ),
        row(
            "db.password= \\nkeycloak.url = https://sso.example.org/auth?client_secret={c}",
            &[
                ("magic-keyed-assignment", 13, 27),
                ("magic-keyed-assignment", 73, 83),
            ],
        ),
        row(
            "token ==keycloak.url = https://sso.example.org/auth?secret={c}",
            &[
                ("magic-keyed-assignment", 7, 20),
                ("magic-keyed-assignment", 59, 69),
            ],
        ),
        row(
            "api_token= {c} --verbose",
            &[("magic-keyed-assignment", 11, 21)],
        ),
        row(
            "api_token=\n{c} and more words",
            &[("magic-keyed-assignment", 11, 21)],
        ),
        row("API_KEY= run command", &[("magic-keyed-assignment", 9, 12)]),
        row(
            "API_TOKEN=\nDATABASE_URL=postgres://localhost/db",
            &[("magic-keyed-assignment", 11, 47)],
        ),
        row(
            "API_TOKEN=\n#comment7",
            &[("magic-keyed-assignment", 11, 20)],
        ),
        row("API_TOKEN=\n}", &[("magic-keyed-assignment", 11, 12)]),
        row(
            "API_TOKEN=\n[section]",
            &[("magic-keyed-assignment", 11, 20)],
        ),
        row("API_TOKEN=\n---", &[("magic-keyed-assignment", 11, 14)]),
        row("API_TOKEN=\nURL=", &[("magic-keyed-assignment", 11, 15)]),
        row("KEY=   # comment", &[("magic-keyed-assignment", 7, 8)]),
        row(
            "if (e.key === \"Enter\") {}",
            &[("magic-keyed-assignment", 11, 13)],
        ),
        row("token == other1", &[("magic-keyed-assignment", 7, 8)]),
        row("google-auth==2.49.1", &[("magic-keyed-assignment", 12, 19)]),
        row("password=={c}", &[("magic-keyed-assignment", 9, 20)]),
        row(
            "out\\napi_token= {i}\\nnext line",
            &[("magic-keyed-assignment", 16, 46)],
        ),
        row(
            "api_token= 4096\\nnext",
            &[("magic-keyed-assignment", 11, 21)],
        ),
        row(
            "api_token= true\\nnext",
            &[("magic-keyed-assignment", 11, 21)],
        ),
        row(
            "password= null\\nnext",
            &[("magic-keyed-assignment", 10, 20)],
        ),
        row(
            "password= cfg.value\\nnext",
            &[("magic-keyed-assignment", 10, 25)],
        ),
        row(
            "password= {t}\\nnext",
            &[("magic-keyed-assignment", 10, 20)],
        ),
        row("x\\npublic_key={i}", &[("magic-keyed-assignment", 14, 38)]),
        row("x\\tpublic_key={i}", &[("magic-keyed-assignment", 14, 38)]),
        row(
            "C:\\\\npublic_key={i}",
            &[("magic-keyed-assignment", 16, 40)],
        ),
        row(
            "api_token= {c}{_1020}\\u000anext",
            &[("magic-keyed-assignment", 11, 21)],
        ),
        row(
            "password={y}{_1020}\\nnext",
            &[("magic-keyed-assignment", 9, 11)],
        ),
        row(
            "password= {c};x9 \\u2713 done",
            &[("magic-keyed-assignment", 10, 20)],
        ),
        row(
            "{\"msg\":\"password= {c} \",\"user\":\"bob\"}",
            &[("magic-keyed-assignment", 18, 28)],
        ),
        row(
            "msg=\"password= {s} \";echo ok",
            &[("magic-keyed-assignment", 15, 22)],
        ),
        row(
            "echo \"API_TOKEN= {c} \">>.env",
            &[("magic-keyed-assignment", 17, 27)],
        ),
        row("/* password= {c} */", &[("magic-keyed-assignment", 13, 23)]),
        row(
            "<!-- password= {c} -->",
            &[("magic-keyed-assignment", 15, 25)],
        ),
        row("[password= {s} ,]", &[("magic-keyed-assignment", 11, 18)]),
        row("password= {s} (", &[("magic-keyed-assignment", 10, 17)]),
        row(
            "curl -d \\\"password= {c}\\\" https://api.example.org/v1",
            &[("magic-keyed-assignment", 20, 32)],
        ),
        row("deploy --password {s} --verbose", &[]),
        row("deploy --api-key \"{i}\" --host h", &[]),
        row("deploy --password {x} --verbose", &[]),
        row("password: {s}", &[]),
        row("password: {q}", &[]),
        row("  POSTGRES_PASSWORD: {d}", &[]),
        row("X-Api-Key: {i}", &[]),
        row("out\\npassword: {s}\\nuser: bob", &[]),
        row("PGPASSWORD={s} psql -h host", &[]),
        row(
            "MYSQL_ROOT_PASSWORD={s}",
            &[("magic-keyed-assignment", 20, 27)],
        ),
        row(
            "dbpassword=\"{c} two\" run",
            &[("magic-keyed-assignment-double-quoted", 12, 26)],
        ),
        row("mypassword={y}", &[]),
        row("password={d}", &[]),
        row("{\"password\":\"{d}\"}", &[]),
        row("{\"password\": {d}}", &[]),
        row("password={n}.5", &[]),
        row("password={n}-abcd", &[("magic-keyed-assignment", 9, 18)]),
        row("max_tokens=4096", &[]),
        row("git clone https://user:{s}@example.org/r.git", &[]),
        row("MY_SECRET=https://user:{s}@example.org/x", &[]),
        row("Authorization: Basic {b}", &[]),
        row(
            "curl -H \"Authorization: Basic {b}\" https://example.org",
            &[],
        ),
        row("mysql -uroot -p{s} app", &[]),
        row("sshpass -p {s} ssh host", &[]),
        row("docker login -u bob -p {s} registry", &[]),
        row("mysql app\u{2028}ssh -p2222 host", &[]),
        row("docker login{_300}-p 9234", &[]),
        row("{h}\n{k}\n{k}\nabcd12==\nnext line", &[]),
        row("{h}  \n{k}\n{k}\n", &[]),
        row("{h}\n{k}\n{k}\n-----END RSA PRIVATE KEY-----\n", &[]),
        row(
            "ANTHROPIC_API_KEY: sk-ant-api03-{i}{i}",
            &[("magic-anthropic-api-key", 19, 80)],
        ),
        row("api_key: {i}   # rotate", &[]),
        row("token: ghp_{i}{i}", &[]),
    ]
}

fn conservative_values(report: &ScanReport) -> Vec<(&str, usize, usize)> {
    report
        .findings
        .iter()
        .map(|finding: &Finding| {
            (
                finding.rule_id.as_str(),
                finding.value_span.start(),
                finding.value_span.end(),
            )
        })
        .collect()
}

#[test]
fn the_document_of_main_reports_what_the_build_of_main_reported() {
    for row in rows() {
        let report = scan(&BASE, ScanProfile::Conservative, &row.input);
        assert_eq!(conservative_values(&report), row.main, "{:?}", row.input);
    }
}

#[test]
fn the_embedded_rules_report_every_finding_main_reports_on_recorded_inputs() {
    let mut reported = 0;
    for row in rows() {
        assert_embedded_reports_every_base_finding(&row.input);
        // The added rules leave the report of `main`'s rules in place, entry for entry.
        let embedded = scan(&EMBEDDED, ScanProfile::Conservative, &row.input);
        let kept: Vec<_> = conservative_values(&embedded)
            .into_iter()
            .filter(|(rule, ..)| !ADDED_RULES.contains(rule))
            .collect();
        assert_eq!(kept, row.main, "{:?}", row.input);
        reported += row.main.len();
    }
    assert!(reported >= 100, "the rows hold {reported} findings of main");
}

/// The parts of a line the keyed, detached, quoted, and added forms are built from. A
/// generated line is `prefix key separator quote value quote tail`.
const PREFIXES: [&str; 14] = [
    "",
    " ",
    "#",
    "export ",
    "(",
    "{\"",
    "/* ",
    "13:",
    "\u{2022} ",
    "curl -d \\\"",
    "\\n",
    "x\\t",
    "- ",
    "db.password =\n",
];
const KEYS: [&str; 24] = [
    "password",
    "api_key",
    "DB_PASSWORD",
    "PGPASSWORD",
    "token",
    "clientSecret",
    "db.password",
    "x-api-key",
    "secret",
    "public_key",
    "Authorization",
    "credential",
    "auth",
    "max_tokens",
    "--password",
    "--api-key",
    "-p",
    "mysql -uroot -p",
    "sshpass -p",
    "docker login -p",
    "https://user",
    "keycloak.url",
    "\"password\"",
    "'secret'",
];
const SEPARATORS: [&str; 26] = [
    "=",
    " = ",
    "= ",
    " =",
    ":",
    ": ",
    " : ",
    "==",
    "===",
    "=\n",
    "\n=",
    "\n=\n",
    ":\n",
    " ",
    "\t",
    "\u{a0}",
    "\u{a0}=",
    "=\u{a0}",
    "=\u{2028}",
    "\u{2028}",
    "=\r\n",
    "",
    ": Bearer ",
    ": Basic ",
    ":\u{a0}",
    "=\\n",
];
const QUOTES: [&str; 6] = ["", "", "\"", "'", "`", "\\\""];
const VALUES: [&str; 20] = [
    "{c}",
    "{i}",
    "{p}",
    "{m}",
    "{w}{c}",
    "{s}",
    "{d}",
    "{b}",
    "{k}",
    "4096",
    "true",
    "null",
    "cfg.value",
    "$VAR",
    "<name>",
    "Pa55'!!",
    "Q7r9$\\+v9",
    "ab;cd9",
    "a b",
    "",
];
const TAILS: [&str; 30] = [
    "",
    "",
    " ",
    ";",
    ";tail9",
    " # note",
    "\\n",
    "\\nnext",
    "\\u000anext",
    "\u{2028}next",
    " next word",
    ")",
    "\" ",
    "\"}",
    ",",
    " */",
    "&x9",
    "|cat",
    "$x9",
    "\\",
    "@host/path",
    "\t",
    " ;",
    " <",
    " )",
    "\\\" https://host/v1",
    ";x9 \\u2713 done",
    "==",
    "=",
    "\u{a0}next",
];
const JOINERS: [&str; 6] = ["\n", "\r\n", "\u{2028}", "\\n", " ", "\x0B"];

type LineParts = (usize, usize, usize, usize, usize, usize, usize);

fn line((prefix, key, separator, open, value, close, tail): LineParts) -> String {
    fill(
        &[
            PREFIXES[prefix],
            KEYS[key],
            SEPARATORS[separator],
            QUOTES[open],
            VALUES[value],
            QUOTES[close],
            TAILS[tail],
        ]
        .concat(),
    )
}

fn line_parts() -> impl proptest::strategy::Strategy<Value = LineParts> {
    (
        0..PREFIXES.len(),
        0..KEYS.len(),
        0..SEPARATORS.len(),
        0..QUOTES.len(),
        0..VALUES.len(),
        0..QUOTES.len(),
        0..TAILS.len(),
    )
}

/// A window of the redactor ends at an arbitrary byte of a text, so each generated text is
/// also scanned from and to an offset inside it, moved back to a character boundary.
fn cut(input: &str, from: usize, to: usize) -> &str {
    let floor = |mut at: usize| {
        while !input.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let (from, to) = (
        floor(from % (input.len() + 1)),
        floor(to % (input.len() + 1)),
    );
    &input[from.min(to)..from.max(to)]
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(6_000))]

    #[test]
    fn the_embedded_rules_report_every_finding_main_reports_on_generated_lines(
        lines in proptest::collection::vec(line_parts(), 1..4),
        joiner in 0..JOINERS.len(),
        (from, to) in (0usize..4096, 0usize..4096),
    ) {
        let input = lines.into_iter().map(line).collect::<Vec<_>>().join(JOINERS[joiner]);
        assert_embedded_reports_every_base_finding(&input);
        assert_embedded_reports_every_base_finding(cut(&input, 0, to));
        assert_embedded_reports_every_base_finding(cut(&input, from, to));
    }
}

/// Every part in one table, for text that follows no line grammar.
fn parts() -> Vec<&'static str> {
    [
        &PREFIXES[..],
        &KEYS[..],
        &SEPARATORS[..],
        &QUOTES[..],
        &VALUES[..],
        &TAILS[..],
        &JOINERS[..],
    ]
    .concat()
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(6_000))]

    #[test]
    fn the_embedded_rules_report_every_finding_main_reports_on_generated_text(
        picks in proptest::collection::vec(0usize..4096, 0..24),
        (from, to) in (0usize..4096, 0usize..4096),
    ) {
        let parts = parts();
        let input = fill(&picks.iter().map(|pick| parts[pick % parts.len()]).collect::<String>());
        assert_embedded_reports_every_base_finding(&input);
        assert_embedded_reports_every_base_finding(cut(&input, from, to));
    }
}
