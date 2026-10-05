use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use aws_smithy_async::time::{SharedTimeSource, StaticTimeSource};
use aws_smithy_runtime_api::client::http::{
    HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpClient,
    SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_runtime_api::http::StatusCode;
use aws_smithy_types::DateTime;
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::date_time::Format;
use host_runtime::model_execution::aws_helper::{
    HelperFailure, HelperReport, HelperRequest, Renewal, acquire,
};
use host_runtime::model_execution::aws_profile::{CapturedProfileInput, admit};

const SSO_CONFIG: &str = "\
[profile dev]
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Dev
[sso-session corp]
sso_region = us-east-1
sso_start_url = https://d-1234567890.awsapps.com/start
";

const ROLE_CONFIG: &str = "\
[profile app]
role_arn = arn:aws:iam::444455556666:role/app
source_profile = dev
";

#[derive(Clone, Default)]
struct Script {
    responses: Arc<Mutex<VecDeque<(u16, String)>>>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for Script {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Script")
    }
}

impl Script {
    fn new(responses: &[(u16, &str)]) -> Self {
        let script = Self::default();
        script
            .responses
            .lock()
            .unwrap()
            .extend(responses.iter().map(|(s, b)| (*s, (*b).to_owned())));
        script
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl HttpClient for Script {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

impl HttpConnector for Script {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let path = request.uri().split('?').next().unwrap_or_default();
        self.seen
            .lock()
            .unwrap()
            .push(format!("{} {path}", request.method()));
        let (status, body) = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((599, String::new()));
        let response =
            HttpResponse::new(StatusCode::try_from(status).unwrap(), SdkBody::from(body));
        HttpConnectorFuture::ready(Ok(response))
    }
}

/// The synthetic clock every case runs at; fixtures express times relative to it.
fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000)
}

fn rfc3339(at: SystemTime) -> String {
    DateTime::from(at).fmt(Format::DateTime).unwrap()
}

fn token_path(home: &Path) -> PathBuf {
    home.join(".aws/sso/cache/ee0bfd2552fbd840c02cc48b6e823320543c450f.json")
}

fn write_token(home: &Path, expires_in: Duration) {
    let now = now();
    let token = format!(
        r#"{{"accessToken":"access-canary","expiresAt":"{}","refreshToken":"refresh-canary","clientId":"client","clientSecret":"client-secret","registrationExpiresAt":"{}","region":"us-east-1","startUrl":"https://d-1234567890.awsapps.com/start"}}"#,
        rfc3339(now + expires_in),
        rfc3339(now + Duration::from_secs(86_400 * 30)),
    );
    std::fs::create_dir_all(token_path(home).parent().unwrap()).unwrap();
    std::fs::write(token_path(home), token).unwrap();
}

fn request(profile: &str, config: &str) -> HelperRequest {
    let graph = admit(CapturedProfileInput {
        profile,
        region: "us-west-2",
        config: config.as_bytes(),
        credentials: b"",
    })
    .expect("fixture admits");
    HelperRequest::from_graph(&graph)
}

fn portal_credentials() -> String {
    let expiration = now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 3_600_000;
    format!(
        r#"{{"roleCredentials":{{"accessKeyId":"ASIAPORTAL","secretAccessKey":"portal-secret","sessionToken":"portal-token","expiration":{expiration}}}}}"#
    )
}

const STS_OK: &str = r#"<AssumeRoleResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><AssumeRoleResult><Credentials><AccessKeyId>ASIAROLE</AccessKeyId><SecretAccessKey>role-secret</SecretAccessKey><SessionToken>role-token</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration></Credentials><AssumedRoleUser><Arn>arn:aws:sts::444455556666:assumed-role/app/eidnara-credentials</Arn><AssumedRoleId>AROA:eidnara-credentials</AssumedRoleId></AssumedRoleUser></AssumeRoleResult><ResponseMetadata><RequestId>1</RequestId></ResponseMetadata></AssumeRoleResponse>"#;

const STS_DENIED: &str = r#"<ErrorResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><Error><Type>Sender</Type><Code>AccessDenied</Code><Message>denied</Message></Error><RequestId>1</RequestId></ErrorResponse>"#;

const OIDC_OK: &str = r#"{"accessToken":"renewed-access","expiresIn":28800,"refreshToken":"renewed-refresh","tokenType":"Bearer"}"#;

fn run(request: &HelperRequest, script: &Script) -> HelperReport {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let http = SharedHttpClient::new(script.clone());
    let clock = SharedTimeSource::new(StaticTimeSource::new(now()));
    runtime.block_on(acquire(request, http, clock, None))
}

fn expect_credentials(report: &HelperReport, key: &str, renewal: Renewal) {
    match report {
        HelperReport::Credentials {
            access_key_id,
            renewal: observed,
            expires_at_unix_seconds,
            ..
        } => {
            assert_eq!(access_key_id, key);
            assert_eq!(*observed, renewal);
            assert!(*expires_at_unix_seconds > 0);
        }
        _ => panic!("expected credentials"),
    }
}

fn expect_failure(report: &HelperReport, failure: HelperFailure, renewal: Renewal) {
    match report {
        HelperReport::Failed {
            failure: observed,
            renewal: observed_renewal,
        } => assert_eq!((*observed, *observed_renewal), (failure, renewal)),
        _ => panic!("expected failure"),
    }
}

// One test owns the process environment: the SDK reads `HOME` from it.
#[test]
fn helper_acquires_through_the_pinned_sdk_and_reports_renewal_start() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: this binary runs this single test, so no other thread reads the
    // environment while it changes.
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAAMBIENTPOISON");
        std::env::set_var("AWS_PROFILE", "ambient");
        std::env::set_var("AWS_REGION", "eu-west-1");
        std::env::set_var("AWS_DEFAULT_REGION", "eu-west-1");
        std::env::set_var("AWS_CONFIG_FILE", "/nonexistent/poison");
        std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", "/nonexistent/poison");
    }
    let sso = request("dev", SSO_CONFIG);
    let role = request("app", &format!("{ROLE_CONFIG}{SSO_CONFIG}"));

    write_token(home.path(), Duration::from_secs(3600));
    let script = Script::new(&[(200, &portal_credentials())]);
    expect_credentials(&run(&sso, &script), "ASIAPORTAL", Renewal::NotStarted);
    assert_eq!(
        script.seen(),
        ["GET https://portal.sso.us-east-1.amazonaws.com/federation/credentials"]
    );

    let script = Script::new(&[(200, &portal_credentials()), (200, STS_OK)]);
    expect_credentials(&run(&role, &script), "ASIAROLE", Renewal::NotStarted);
    assert_eq!(
        script.seen(),
        [
            "GET https://portal.sso.us-east-1.amazonaws.com/federation/credentials",
            "POST https://sts.us-west-2.amazonaws.com/"
        ]
    );

    let script = Script::new(&[(200, &portal_credentials()), (403, STS_DENIED)]);
    let report = run(&role, &script);
    expect_failure(&report, HelperFailure::ProviderError, Renewal::NotStarted);
    assert_eq!(
        script.seen()[1],
        "POST https://sts.us-west-2.amazonaws.com/",
        "the denial comes from STS"
    );

    let script = Script::new(&[(503, "")]);
    let report = run(&sso, &script);
    expect_failure(&report, HelperFailure::ProviderError, Renewal::NotStarted);
    assert_eq!(script.seen().len(), 1, "one attempt per call");

    write_token(home.path(), Duration::from_secs(60));
    let script = Script::new(&[(200, OIDC_OK), (200, &portal_credentials())]);
    let renewed = run(&sso, &script);
    expect_credentials(&renewed, "ASIAPORTAL", Renewal::Started);
    let renewed = serde_json::to_string(&renewed).unwrap();
    for canary in [
        "renewed-access",
        "renewed-refresh",
        "refresh-canary",
        "client-secret",
    ] {
        assert!(!renewed.contains(canary), "{canary} reached the report");
    }
    assert_eq!(
        script.seen(),
        [
            "POST https://oidc.us-east-1.amazonaws.com/token",
            "GET https://portal.sso.us-east-1.amazonaws.com/federation/credentials"
        ]
    );
    let successor = std::fs::read_to_string(token_path(home.path())).unwrap();
    assert!(successor.contains("renewed-access") && successor.contains("renewed-refresh"));

    write_token(home.path(), Duration::from_secs(60));
    let script = Script::new(&[(500, "{}")]);
    let report = run(&sso, &script);
    assert!(matches!(
        report,
        HelperReport::Failed {
            renewal: Renewal::Started,
            ..
        }
    ));
    assert_eq!(
        script.seen()[0],
        "POST https://oidc.us-east-1.amazonaws.com/token"
    );

    write_token(home.path(), Duration::from_secs(60));
    let script = Script::new(&[(500, "{}"), (200, &portal_credentials())]);
    let report = run(&sso, &script);
    expect_credentials(&report, "ASIAPORTAL", Renewal::Started);

    std::fs::remove_file(token_path(home.path())).unwrap();
    let script = Script::new(&[]);
    let report = run(&sso, &script);
    assert!(matches!(
        report,
        HelperReport::Failed {
            renewal: Renewal::NotStarted,
            ..
        }
    ));
    assert!(script.seen().is_empty(), "missing login makes no request");

    let mut noncanonical = request("dev", SSO_CONFIG);
    let with_process = noncanonical.config.replace(
        "[profile dev]\n",
        "[profile dev]\ncredential_process = /bin/sh -c 'touch ran'\n",
    );
    *noncanonical.config = with_process;
    let script = Script::new(&[]);
    expect_failure(
        &run(&noncanonical, &script),
        HelperFailure::InvalidInput,
        Renewal::NotStarted,
    );
    assert!(script.seen().is_empty());
    assert!(!home.path().join("ran").exists());

    write_token(home.path(), Duration::from_secs(3600));
    for (variable, value, sent) in [
        ("AWS_ENDPOINT_URL", "https://attacker.example", 0),
        ("AWS_ENDPOINT_URL_SSO", "https://attacker.example", 0),
        ("AWS_ENDPOINT_URL_STS", "https://sts.amazonaws.com", 1),
    ] {
        // SAFETY: as above; this single test owns the environment.
        unsafe { std::env::set_var(variable, value) };
        let script = Script::new(&[(200, &portal_credentials()), (200, STS_OK)]);
        expect_failure(
            &run(&role, &script),
            HelperFailure::DestinationRefused,
            Renewal::NotStarted,
        );
        assert_eq!(script.seen().len(), sent, "{variable}");
        // SAFETY: as above.
        unsafe { std::env::remove_var(variable) };
    }

    write_token(home.path(), Duration::from_secs(3600));
    let unexpiring = portal_credentials().replace(r#","expiration""#, r#","unused""#);
    let script = Script::new(&[(200, &unexpiring)]);
    let report = run(&sso, &script);
    expect_failure(&report, HelperFailure::Incomplete, Renewal::NotStarted);

    let script = Script::new(&[(200, &portal_credentials())]);
    let report = serde_json::to_string(&run(&sso, &script)).unwrap();
    for canary in ["access-canary", "refresh-canary", "client-secret", "poison"] {
        assert!(!report.contains(canary), "{canary} reached the report");
    }
}
