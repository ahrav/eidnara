//! The host-executable helper tests redirect admitted destinations to a loopback fake
//! through a debug-only seam, so they compile only with debug assertions.
#![cfg(debug_assertions)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use host_runtime::model_execution::aws_helper::{
    HELPER_ARG, HelperFailure, HelperReport, HelperRequest, MAX_REQUEST_BYTES, Renewal,
};
use host_runtime::model_execution::aws_profile::{CapturedProfileInput, admit};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const BIN: &str = env!("CARGO_BIN_EXE_eidnara-host");
const TOKEN_FILE: &str = ".aws/sso/cache/ee0bfd2552fbd840c02cc48b6e823320543c450f.json";

const SSO_CONFIG: &str = "\
[profile dev]
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Dev
[sso-session corp]
sso_region = us-east-1
sso_start_url = https://d-1234567890.awsapps.com/start
";

fn canonical_request() -> Vec<u8> {
    let graph = admit(CapturedProfileInput {
        profile: "dev",
        region: "us-west-2",
        config: SSO_CONFIG.as_bytes(),
        credentials: b"",
    })
    .expect("fixture admits");
    serde_json::to_vec(&HelperRequest::from_graph(&graph)).unwrap()
}

fn write_token(home: &std::path::Path) {
    let expires = SystemTime::now() + Duration::from_secs(3600);
    let secs = expires.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let at = chrono::DateTime::from_timestamp(i64::try_from(secs).unwrap(), 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ");
    let path = home.join(TOKEN_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            r#"{{"accessToken":"access-canary","expiresAt":"{at}","region":"us-east-1","startUrl":"https://d-1234567890.awsapps.com/start"}}"#
        ),
    )
    .unwrap();
}

/// One-request-per-connection HTTP/1.1 fake that answers the SSO portal and records
/// each request line.
async fn fake_portal(listener: TcpListener, seen: Arc<Mutex<Vec<String>>>) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            }
        }
        let head = String::from_utf8_lossy(&buffer).to_string();
        let line = head.lines().next().unwrap_or_default().to_owned();
        let bearer = head
            .lines()
            .any(|l| l.eq_ignore_ascii_case("x-amz-sso_bearer_token: access-canary"));
        seen.lock().unwrap().push(format!("{line} bearer={bearer}"));
        let expiration = (SystemTime::now() + Duration::from_secs(3600))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let body = format!(
            r#"{{"roleCredentials":{{"accessKeyId":"ASIAFAKEPORTAL","secretAccessKey":"portal-secret","sessionToken":"portal-token","expiration":{expiration}}}}}"#
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.ok();
    }
}

struct Portal {
    runtime: tokio::runtime::Runtime,
    origin: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Portal {
    fn start() -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        runtime.spawn(fake_portal(listener, Arc::clone(&seen)));
        Self {
            runtime,
            origin,
            seen,
        }
    }

    fn seen(&self) -> Vec<String> {
        let _ = &self.runtime;
        self.seen.lock().unwrap().clone()
    }
}

fn run_helper(
    home: &std::path::Path,
    origin: &str,
    extra_env: &[(&str, &str)],
    stdin: &[u8],
) -> (HelperReport, String) {
    let mut child = Command::new(BIN)
        .arg(HELPER_ARG)
        .env_clear()
        .env("HOME", home)
        .env("AWS_ACCESS_KEY_ID", "AKIAAMBIENTPOISON")
        .env("AWS_SECRET_ACCESS_KEY", "ambient-poison-secret")
        .env("AWS_PROFILE", "ambient")
        .env("AWS_REGION", "eu-west-1")
        .env("AWS_CONFIG_FILE", "/nonexistent/poison")
        .env("EIDNARA_HOST_TEST_AWS_ORIGIN", origin)
        .envs(extra_env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("helper spawns");
    child.stdin.take().unwrap().write_all(stdin).ok();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty(), "helper wrote to stderr");
    let stdout = String::from_utf8(output.stdout).unwrap();
    (
        serde_json::from_str(&stdout).expect("bounded report"),
        stdout,
    )
}

fn assert_failed(report: &HelperReport, failure: HelperFailure) {
    assert!(matches!(
        report,
        HelperReport::Failed { failure: observed, renewal: Renewal::NotStarted } if *observed == failure
    ));
}

#[test]
fn the_host_executable_acquires_in_helper_mode_before_host_initialization() {
    let home = tempfile::tempdir().unwrap();
    write_token(home.path());
    let portal = Portal::start();

    let (report, stdout) = run_helper(home.path(), &portal.origin, &[], &canonical_request());
    let HelperReport::Credentials {
        access_key_id,
        session_token,
        renewal,
        ..
    } = report
    else {
        panic!("expected credentials");
    };
    assert_eq!(access_key_id, "ASIAFAKEPORTAL");
    assert_eq!(*session_token, "portal-token");
    assert_eq!(renewal, Renewal::NotStarted);
    assert!(!stdout.contains("access-canary") && !stdout.contains("poison"));
    let seen = portal.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0].starts_with("GET /federation/credentials?"),
        "{seen:?}"
    );
    assert!(seen[0].ends_with("bearer=true"), "{seen:?}");
    assert!(!home.path().join(".aws/config").exists());

    let foreign = [("AWS_ENDPOINT_URL_SSO", "https://attacker.example")];
    let (report, _) = run_helper(home.path(), &portal.origin, &foreign, &canonical_request());
    assert_failed(&report, HelperFailure::DestinationRefused);
    assert_eq!(
        portal.seen().len(),
        1,
        "a refused destination sends nothing"
    );
}

#[test]
fn the_host_executable_refuses_invalid_input_without_network() {
    let home = tempfile::tempdir().unwrap();
    let portal = Portal::start();
    let mut noncanonical: serde_json::Value = serde_json::from_slice(&canonical_request()).unwrap();
    noncanonical["config"] = SSO_CONFIG
        .replace(
            "[profile dev]\n",
            "[profile dev]\ncredential_process = /bin/true\n",
        )
        .into();
    for stdin in [
        b"not json".to_vec(),
        br#"{"profile":"dev","region":"us-west-2","config":"","extra":1}"#.to_vec(),
        serde_json::to_vec(&noncanonical).unwrap(),
        vec![b' '; MAX_REQUEST_BYTES + 1],
    ] {
        let (report, _) = run_helper(home.path(), &portal.origin, &[], &stdin);
        assert_failed(&report, HelperFailure::InvalidInput);
    }
    let (report, _) = run_helper(home.path(), &portal.origin, &[], &canonical_request());
    assert!(matches!(
        report,
        HelperReport::Failed {
            renewal: Renewal::NotStarted,
            ..
        }
    ));
    assert!(portal.seen().is_empty(), "no request without a login");
}
