use host_runtime::model_execution::aws_profile::{AdmissionError, CapturedProfileInput, admit};

const CAPTURED: &str = "\
[profile dev]
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Dev
[sso-session corp]
sso_region = us-east-1
sso_start_url = https://d-1234567890.awsapps.com/start
";

const AMBIENT: &str = "\
[profile ambient]
sso_session = corp
sso_account_id = 999999999999
sso_role_name = Ambient
[profile dev]
sso_account_id = 999999999999
[sso-session corp]
sso_region = us-east-1
sso_start_url = https://ambient.awsapps.com/start
";

#[test]
fn ambient_environment_and_files_never_reach_admission() {
    let home = tempfile::tempdir().expect("home");
    let aws = home.path().join(".aws");
    std::fs::create_dir(&aws).expect("aws dir");
    std::fs::write(aws.join("config"), AMBIENT).expect("config");
    std::fs::write(aws.join("credentials"), AMBIENT).expect("credentials");
    let override_file = home.path().join("override");
    std::fs::write(&override_file, AMBIENT).expect("override");
    // SAFETY: this test binary holds one test, so no other thread reads the
    // environment while it changes.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("AWS_CONFIG_FILE", &override_file);
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &override_file);
        std::env::set_var("AWS_PROFILE", "ambient");
        std::env::set_var("AWS_REGION", "eu-west-1");
    }
    let input = |profile| CapturedProfileInput {
        profile,
        region: "us-west-2",
        config: CAPTURED.as_bytes(),
        credentials: b"",
    };
    assert_eq!(
        admit(input("ambient")).unwrap_err(),
        AdmissionError::MissingSection
    );
    let graph = admit(input("dev")).expect("captured profile admits");
    let emitted = graph.emit_config();
    assert!(emitted.contains("111122223333"));
    assert!(!emitted.contains("999999999999") && !emitted.contains("ambient"));
}
