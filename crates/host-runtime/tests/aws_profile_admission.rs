use host_runtime::model_execution::aws_profile::{
    AdmissionError, AdmittedGraph, CapturedProfileInput, DEFAULT_ROLE_SESSION_NAME,
    MAX_CONFIG_FILE_BYTES, RootIdentity, admit,
};
use sha2::{Digest, Sha256};

const REGION: &str = "us-west-2";

const SSO_SESSION: &str = "\
[sso-session corp]
sso_region = us-east-1
sso_start_url = https://d-1234567890.awsapps.com/start
sso_registration_scopes = sso:account:access
";

fn admit_config(profile: &str, config: &str) -> Result<AdmittedGraph, AdmissionError> {
    admit_files(profile, config, "")
}

fn admit_files(
    profile: &str,
    config: &str,
    credentials: &str,
) -> Result<AdmittedGraph, AdmissionError> {
    admit(CapturedProfileInput {
        profile,
        region: REGION,
        config: config.as_bytes(),
        credentials: credentials.as_bytes(),
    })
}

fn sso_profile(name: &str) -> String {
    format!(
        "[profile {name}]\nsso_session = corp\nsso_account_id = 111122223333\nsso_role_name = Dev\nregion = {REGION}\n"
    )
}

fn role_profile(name: &str, source: &str) -> String {
    format!(
        "[profile {name}]\nrole_arn = arn:aws:iam::444455556666:role/{name}-role\nsource_profile = {source}\n"
    )
}

fn sso_config(extra: &str) -> String {
    format!("{}{SSO_SESSION}{extra}", sso_profile("dev"))
}

fn assert_round_trip(graph: &AdmittedGraph) {
    let emitted = graph.emit_config();
    assert_eq!(
        emitted.capacity(),
        emitted.len(),
        "emission is sized exactly"
    );
    let reparsed = admit_config(&graph.identity().profile, &emitted).expect("emission admits");
    assert_eq!(reparsed.identity(), graph.identity());
    assert_eq!(*reparsed.emit_config(), *emitted);
}

#[test]
fn modern_sso_root_admits_and_round_trips() {
    let graph = admit_config("dev", &sso_config("")).expect("sso admits");
    let identity = graph.identity();
    assert!(identity.roles.is_empty());
    assert_eq!(
        identity.root,
        RootIdentity::Sso {
            profile: "dev".into(),
            session_name: "corp".into(),
            start_url: "https://d-1234567890.awsapps.com/start".into(),
            sso_region: "us-east-1".into(),
            account_id: "111122223333".into(),
            role_name: "Dev".into(),
        }
    );
    assert_eq!(identity.region, REGION);
    assert_round_trip(&graph);
}

#[test]
fn absent_sso_scope_normalizes_to_account_access() {
    let config = sso_config("").replace("sso_registration_scopes = sso:account:access\n", "");
    let graph = admit_config("dev", &config).expect("scope defaults");
    assert!(
        graph
            .emit_config()
            .contains("sso_registration_scopes = sso:account:access")
    );
}

#[test]
fn four_role_edges_ending_in_sso_admit_in_selected_order() {
    let config = format!(
        "{}{}{}{}{}",
        role_profile("r0", "r1"),
        role_profile("r1", "r2"),
        role_profile("r2", "r3"),
        role_profile("r3", "dev"),
        sso_config("")
    );
    let graph = admit_config("r0", &config).expect("four edges admit");
    let identity = graph.identity();
    let order: Vec<_> = identity
        .roles
        .iter()
        .map(|edge| edge.profile.as_str())
        .collect();
    assert_eq!(order, ["r0", "r1", "r2", "r3"]);
    for edge in &identity.roles {
        assert_eq!(edge.session_name, DEFAULT_ROLE_SESSION_NAME);

        assert_eq!(edge.external_id, None);
    }
    assert_round_trip(&graph);
}

#[test]
fn five_role_edges_are_refused() {
    let config = format!(
        "{}{}{}{}{}{}",
        role_profile("r0", "r1"),
        role_profile("r1", "r2"),
        role_profile("r2", "r3"),
        role_profile("r3", "r4"),
        role_profile("r4", "dev"),
        sso_config("")
    );
    assert_eq!(
        admit_config("r0", &config).unwrap_err(),
        AdmissionError::GraphTooLarge
    );
}

#[test]
fn static_root_feeding_a_role_admits_from_either_file() {
    let config = format!(
        "{}role_session_name = build@ci\nexternal_id = ext-123\nduration_seconds = 3600\n",
        role_profile("app", "keys")
    );
    let credentials = "[keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = c2VjcmV0/K+=xxxxxxx\n";
    let graph = admit_files("app", &config, credentials).expect("static root admits");
    let identity = graph.identity();
    assert_eq!(identity.roles[0].session_name, "build@ci");
    assert_eq!(identity.roles[0].external_id.as_deref(), Some("ext-123"));
    let RootIdentity::Static { access_key_id, .. } = &identity.root else {
        panic!("static root");
    };
    assert_eq!(access_key_id, "AKIAIOSFODNN7EXAMPLE");
    let secret: [u8; 32] = Sha256::digest(b"c2VjcmV0/K+=xxxxxxx").into();
    assert!(
        matches!(identity.root, RootIdentity::Static { secret_sha256, .. } if secret_sha256 == secret)
    );
    assert_round_trip(&graph);

    let in_config = format!(
        "{config}[profile keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = c2VjcmV0/K+=xxxxxxx\n"
    );
    let same = admit_config("app", &in_config).expect("config-file root admits");
    assert_eq!(same.identity(), identity);
}

#[test]
fn self_referencing_role_uses_its_own_root() {
    let config = "[profile app]\nrole_arn = arn:aws:iam::444455556666:role/path/to/app\nsource_profile = app\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMIK7MDENGbPxRfiCY\n";
    let graph = admit_config("app", config).expect("self reference admits");
    assert_eq!(graph.identity().roles.len(), 1);
    assert!(matches!(graph.identity().root, RootIdentity::Static { .. }));
    assert_round_trip(&graph);

    let sso = format!(
        "{}role_arn = arn:aws:iam::444455556666:role/app\nsource_profile = dev\n{SSO_SESSION}",
        sso_profile("dev")
    );
    let graph = admit_config("dev", &sso).expect("sso self reference admits");
    assert_eq!(graph.identity().roles.len(), 1);
    assert_round_trip(&graph);
}

#[test]
fn unsupported_roots_are_refused() {
    let static_only = "[profile keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMIK7MDENGbPxRfiCY\n";
    assert_eq!(
        admit_config("keys", static_only).unwrap_err(),
        AdmissionError::StaticRootWithoutRole
    );
    let session_key = format!(
        "{}[profile keys]\naws_access_key_id = ASIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMIK7MDENGbPxRfiCY\n",
        role_profile("app", "keys")
    );
    assert_eq!(
        admit_config("app", &session_key).unwrap_err(),
        AdmissionError::TemporaryStaticRoot
    );
    let with_token = format!(
        "{}aws_session_token = token\n",
        session_key.replace("ASIA", "AKIA")
    );
    assert_eq!(
        admit_config("app", &with_token).unwrap_err(),
        AdmissionError::ForbiddenOption("aws_session_token")
    );
    let half_key = format!(
        "{}[profile keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\n",
        role_profile("app", "keys")
    );
    assert_eq!(
        admit_config("app", &half_key).unwrap_err(),
        AdmissionError::IncompleteSource
    );
    let missing_root = role_profile("app", "absent");
    assert_eq!(
        admit_config("app", &missing_root).unwrap_err(),
        AdmissionError::MissingSection
    );
    let empty_root = format!(
        "{}[profile keys]\nregion = {REGION}\n",
        role_profile("app", "keys")
    );
    assert_eq!(
        admit_config("app", &empty_root).unwrap_err(),
        AdmissionError::NoCredentialSource
    );
    assert_eq!(
        admit_config("absent", &sso_config("")).unwrap_err(),
        AdmissionError::MissingSection
    );
}

#[test]
fn cycles_are_refused() {
    let config = format!("{}{}", role_profile("a", "b"), role_profile("b", "a"));
    assert_eq!(
        admit_config("a", &config).unwrap_err(),
        AdmissionError::Cycle
    );
}

#[test]
fn forbidden_options_on_selected_sections_are_refused() {
    for option in [
        "mfa_serial = arn:aws:iam::444455556666:mfa/user",
        "credential_process = /bin/creds",
        "credential_source = Ec2InstanceMetadata",
        "web_identity_token_file = /tmp/token",
        "login_session = arn:aws:signin:::session",
        "sso_start_url = https://legacy.awsapps.com/start",
        "sso_region = us-east-1",
        "endpoint_url = https://attacker.example",
        "use_fips_endpoint = true",
        "use_dualstack_endpoint = true",
        "sts_regional_endpoints = legacy",
        "services = custom",
        "ca_bundle = /tmp/ca.pem",
    ] {
        let key = option.split(' ').next().unwrap();
        let root = format!("{}{option}\n{SSO_SESSION}", sso_profile("dev"));
        assert_eq!(
            admit_config("dev", &root).unwrap_err(),
            AdmissionError::ForbiddenOption(key),
            "{key} on the root"
        );
        let nested = format!("{}{option}\n{}", role_profile("app", "dev"), sso_config(""));
        assert_eq!(
            admit_config("app", &nested).unwrap_err(),
            AdmissionError::ForbiddenOption(key),
            "{key} on a role edge"
        );
    }
    for key in [
        "endpoint_url",
        "use_fips_endpoint",
        "use_dualstack_endpoint",
        "services",
        "ca_bundle",
    ] {
        let session = sso_config("").replace(
            "sso_registration_scopes",
            &format!("{key} = value\nsso_registration_scopes"),
        );
        assert_eq!(
            admit_config("dev", &session).unwrap_err(),
            AdmissionError::ForbiddenOption(key),
            "{key} on the sso-session"
        );
    }
}

#[test]
fn ambiguous_profiles_are_refused() {
    let sso_and_static = format!(
        "{}aws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMIK7MDENGbPxRfiCY\n{SSO_SESSION}",
        sso_profile("dev")
    );
    assert_eq!(
        admit_config("dev", &sso_and_static).unwrap_err(),
        AdmissionError::AmbiguousProfile
    );
    let role_with_unused_sso = format!(
        "{}sso_account_id = 111122223333\n",
        role_profile("app", "keys")
    );
    assert_eq!(
        admit_config("app", &role_with_unused_sso).unwrap_err(),
        AdmissionError::AmbiguousProfile
    );
    let source_with_keys_and_role = format!(
        "{}{}aws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMIK7MDENGbPxRfiCY\n",
        role_profile("app", "mid"),
        role_profile("mid", "mid")
    );
    assert_eq!(
        admit_config("app", &source_with_keys_and_role).unwrap_err(),
        AdmissionError::AmbiguousProfile
    );
    let stray_source = format!(
        "{}source_profile = other\n{SSO_SESSION}",
        sso_profile("dev")
    );
    assert_eq!(
        admit_config("dev", &stray_source).unwrap_err(),
        AdmissionError::AmbiguousProfile
    );
    let sso_without_role = sso_config("").replace("sso_role_name = Dev\n", "");
    assert_eq!(
        admit_config("dev", &sso_without_role).unwrap_err(),
        AdmissionError::IncompleteSource
    );
    let missing_session = sso_profile("dev");
    assert_eq!(
        admit_config("dev", &missing_session).unwrap_err(),
        AdmissionError::MissingSection
    );
}

#[test]
fn duration_and_scope_accept_only_their_fixed_values() {
    for (value, expected) in [
        ("3600", Ok(())),
        ("900", Err(AdmissionError::InvalidValue("duration_seconds"))),
        (
            "7200",
            Err(AdmissionError::InvalidValue("duration_seconds")),
        ),
        (
            "03600",
            Err(AdmissionError::InvalidValue("duration_seconds")),
        ),
    ] {
        let config = format!(
            "{}duration_seconds = {value}\n{}",
            role_profile("app", "dev"),
            sso_config("")
        );
        assert_eq!(
            admit_config("app", &config).map(|_| ()),
            expected,
            "{value}"
        );
    }
    let scope = sso_config("").replace(
        "sso:account:access",
        "sso:account:access codewhisperer:completions",
    );
    assert_eq!(
        admit_config("dev", &scope).unwrap_err(),
        AdmissionError::InvalidValue("sso_registration_scopes")
    );
}

#[test]
fn regions_must_agree_and_stay_commercial() {
    let conflict = sso_config("").replace(&format!("region = {REGION}"), "region = eu-west-1");
    assert_eq!(
        admit_config("dev", &conflict).unwrap_err(),
        AdmissionError::RegionConflict
    );
    for region in [
        "cn-north-1",
        "us-gov-west-1",
        "us-iso-east-1",
        "us-west",
        "",
        "US-WEST-2",
    ] {
        let result = admit(CapturedProfileInput {
            profile: "dev",
            region,
            config: sso_config("").as_bytes(),
            credentials: b"",
        });
        assert_eq!(
            result.unwrap_err(),
            AdmissionError::InvalidSelector,
            "{region}"
        );
    }
    let sso_region = sso_config("").replace("sso_region = us-east-1", "sso_region = cn-north-1");
    assert_eq!(
        admit_config("dev", &sso_region).unwrap_err(),
        AdmissionError::InvalidValue("sso_region")
    );
}

#[test]
fn multiline_and_oversized_values_are_refused() {
    let continued = sso_config("").replace(
        "sso_role_name = Dev\n",
        "sso_role_name = Dev\n  continued\n",
    );
    assert_eq!(
        admit_config("dev", &continued).unwrap_err(),
        AdmissionError::InvalidValue("sso_role_name")
    );
    let long_arn = format!(
        "[profile app]\nrole_arn = arn:aws:iam::444455556666:role/{}\nsource_profile = dev\n{}",
        "a".repeat(65),
        sso_config("")
    );
    assert_eq!(
        admit_config("app", &long_arn).unwrap_err(),
        AdmissionError::InvalidValue("role_arn")
    );
    for url in [
        "http://d-1.awsapps.com/start",
        "https://user@d-1.awsapps.com/start",
        "https://d-1.awsapps.com/start?x=1",
        "https://d-1.awsapps.com/start#frag",
        "https:///start",
        "https://start",
        "https://127.0.0.1/start",
        "https://d-1.awsapps.com:0/start",
        "https://d-1.awsapps.com:+443/start",
        "https://d-1.awsapps.com:99999/start",
    ] {
        let config = sso_config("").replace("https://d-1234567890.awsapps.com/start", url);
        assert_eq!(
            admit_config("dev", &config).unwrap_err(),
            AdmissionError::InvalidValue("sso_start_url"),
            "{url}"
        );
    }
    let partition = role_profile("app", "dev").replace("arn:aws:", "arn:aws-cn:");
    assert_eq!(
        admit_config("app", &format!("{partition}{}", sso_config(""))).unwrap_err(),
        AdmissionError::InvalidValue("role_arn")
    );
    let oversized = format!("{}#{}\n", sso_config(""), "x".repeat(MAX_CONFIG_FILE_BYTES));
    assert_eq!(
        admit_config("dev", &oversized).unwrap_err(),
        AdmissionError::FileTooLarge
    );
    let invalid_utf8 = admit(CapturedProfileInput {
        profile: "dev",
        region: REGION,
        config: b"[profile dev]\nregion = \xff\n",
        credentials: b"",
    });
    assert_eq!(invalid_utf8.unwrap_err(), AdmissionError::Unparseable);
    for profile in ["", "dev profile", "dev]", &"p".repeat(257)] {
        let result = admit(CapturedProfileInput {
            profile,
            region: REGION,
            config: sso_config("").as_bytes(),
            credentials: b"",
        });
        assert_eq!(
            result.unwrap_err(),
            AdmissionError::InvalidSelector,
            "{profile:?}"
        );
    }
}

#[test]
fn unselected_poisoned_profiles_do_not_reach_the_emission() {
    let poisoned = "\
[profile evil]
credential_process = /bin/steal
endpoint_url = https://attacker.example
[sso-session other]
sso_region = cn-north-1
sso_start_url = http://attacker.example
[services custom]
sts =
  endpoint_url = https://attacker.example
";
    let graph = admit_config("dev", &sso_config(poisoned)).expect("unselected sections ignored");
    let emitted = graph.emit_config();
    for needle in [
        "evil",
        "attacker",
        "other",
        "services",
        "credential_process",
    ] {
        assert!(
            !emitted.contains(needle),
            "{needle} leaked into the emission"
        );
    }
    assert_round_trip(&graph);
}

#[test]
fn formatting_preserves_semantics_and_edits_change_identity() {
    let config = format!("{}{}", role_profile("app", "dev"), sso_config(""));
    let base = admit_config("app", &config).expect("base admits");
    let reformatted = format!(
        "; leading comment\n{SSO_SESSION}\n\n[profile dev]\nSSO_SESSION=corp\n# note\nsso_account_id=111122223333\noutput = json\nsso_role_name =Dev\n[profile app]\nsource_profile=dev\nrole_arn   =  arn:aws:iam::444455556666:role/app-role\nduration_seconds = 3600\n"
    );
    let same = admit_config("app", &reformatted).expect("reformatted admits");
    assert_eq!(same.identity(), base.identity());
    assert_eq!(*same.emit_config(), *base.emit_config());

    let edited = config.replace("app-role", "other-role");
    let edited = admit_config("app", &edited).expect("edited admits");
    assert_ne!(edited.identity(), base.identity());

    let static_config = role_profile("app", "keys");
    let keys = |secret: &str| {
        format!(
            "[keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = {secret}\n"
        )
    };
    let first =
        admit_files("app", &static_config, &keys("firstsecret00000")).expect("first admits");
    let rotated =
        admit_files("app", &static_config, &keys("secondsecret0000")).expect("rotated admits");
    assert_ne!(first.identity(), rotated.identity());
    assert_eq!(first.identity().roles, rotated.identity().roles);
}

#[test]
fn duplicate_sections_follow_sdk_merge_semantics() {
    let config = format!(
        "{}[profile app]\nrole_arn = arn:aws:iam::444455556666:role/later\n{}",
        role_profile("app", "dev"),
        sso_config("")
    );
    let graph = admit_config("app", &config).expect("merged sections admit");
    assert_eq!(
        graph.identity().roles[0].role_arn,
        "arn:aws:iam::444455556666:role/later"
    );
    assert_eq!(graph.identity().roles[0].source_profile, "dev");
}

#[test]
fn diagnostics_carry_no_captured_values() {
    let config = format!(
        "{}[profile keys]\naws_access_key_id = ASIACANARYCANARY0000\naws_secret_access_key = secretcanary0000\n",
        role_profile("app", "keys")
    );
    let error = admit_config("app", &config).unwrap_err();
    for rendered in [error.to_string(), format!("{error:?}")] {
        assert!(!rendered.contains("CANARY") && !rendered.contains("canary"));
    }
    let graph = admit_files(
        "app",
        &role_profile("app", "keys"),
        "[keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = secretcanary0000\n",
    )
    .expect("static root admits");
    assert!(!format!("{graph:?}").contains("secretcanary0000"));
    assert!(graph.emit_config().contains("secretcanary0000"));
}

#[test]
fn line_breaks_in_wide_fields_are_refused() {
    let secret = format!(
        "{}[profile keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMIK7MDENG\n  [profile x]\n",
        role_profile("app", "keys")
    );
    assert_eq!(
        admit_config("app", &secret).unwrap_err(),
        AdmissionError::InvalidValue("aws_secret_access_key")
    );
    let url = sso_config("").replace("awsapps.com/start\n", "awsapps.com/start\n  [profile x]\n");
    assert_eq!(
        admit_config("dev", &url).unwrap_err(),
        AdmissionError::InvalidValue("sso_start_url")
    );
}

#[test]
fn credentials_file_keys_override_config_file_keys() {
    let config = format!(
        "{}[profile keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = fromconfig000000\n",
        role_profile("app", "keys")
    );
    let credentials = "[keys]\naws_secret_access_key = fromcredentials0\n";
    let graph = admit_files("app", &config, credentials).expect("merged root admits");
    let expected: [u8; 32] = Sha256::digest(b"fromcredentials0").into();
    assert!(matches!(
        graph.identity().root,
        RootIdentity::Static { secret_sha256, .. } if secret_sha256 == expected
    ));
}

#[test]
fn a_self_referencing_fifth_edge_is_refused() {
    let fifth = format!(
        "{}role_arn = arn:aws:iam::444455556666:role/r4\nsource_profile = r4\n",
        sso_profile("r4")
    );
    let config = format!(
        "{}{}{}{}{fifth}{SSO_SESSION}",
        role_profile("r0", "r1"),
        role_profile("r1", "r2"),
        role_profile("r2", "r3"),
        role_profile("r3", "r4"),
    );
    assert_eq!(
        admit_config("r0", &config).unwrap_err(),
        AdmissionError::GraphTooLarge
    );
}

#[test]
fn role_to_sso_emission_is_canonical() {
    let config = format!(
        "{}external_id = ext-1\n{}",
        role_profile("app", "dev"),
        sso_config("")
    );
    let graph = admit_config("app", &config).expect("admits");
    let expected = "\
[profile app]
region = us-west-2
role_arn = arn:aws:iam::444455556666:role/app-role
source_profile = dev
role_session_name = eidnara-credentials
external_id = ext-1
[profile dev]
region = us-west-2
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Dev
[sso-session corp]
sso_region = us-east-1
sso_start_url = https://d-1234567890.awsapps.com/start
sso_registration_scopes = sso:account:access
";
    assert_eq!(*graph.emit_config(), expected);
}

#[test]
fn static_secrets_follow_the_aws_secret_alphabet() {
    for (secret, admitted) in [
        ("abcdefghijklmnop", true),
        ("abcdefghijklmno", false),
        (&"a".repeat(129)[..], false),
        ("abcdefghijklmno!", false),
        ("abcdefghijklmno-", false),
    ] {
        let credentials = format!(
            "[keys]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = {secret}\n"
        );
        let result = admit_files("app", &role_profile("app", "keys"), &credentials);
        if admitted {
            result.expect("secret admits");
        } else {
            assert_eq!(
                result.unwrap_err(),
                AdmissionError::InvalidValue("aws_secret_access_key"),
                "{secret}"
            );
        }
    }
}

#[test]
fn start_urls_with_an_explicit_port_admit() {
    let config = sso_config("").replace(
        "https://d-1234567890.awsapps.com/start",
        "https://d-1.awsapps.com:8443/start",
    );
    admit_config("dev", &config).expect("explicit port admits");
}
