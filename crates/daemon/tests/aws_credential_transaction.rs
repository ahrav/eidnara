#![cfg(debug_assertions)]

use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use host_runtime::CancellationToken;
use host_runtime::model_execution::aws_helper::HelperFailure;
use host_runtime::model_execution::aws_transaction::{
    CaptureFailure, HelperLimits, OwnerSource, PrivateToken, RenewalEvidence, TransactionFailure,
    TransactionInput, TransactionOutcome, TransactionSlot,
};
use host_runtime::model_execution::subprocess::group_registry::{self, StateRoot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

const BIN: &str = env!("CARGO_BIN_EXE_eidnara-host");

// The host's spawn queue is process-wide and sized for one helper at a time, so the
// tests in this binary run one transaction sequence at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const TOKEN_FILE: &str = "ee0bfd2552fbd840c02cc48b6e823320543c450f.json";
const START_URL: &str = "https://d-1234567890.awsapps.com/start";

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

const STS_OK: &str = r#"<AssumeRoleResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><AssumeRoleResult><Credentials><AccessKeyId>ASIAROLE</AccessKeyId><SecretAccessKey>role-secret</SecretAccessKey><SessionToken>role-token</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration></Credentials><AssumedRoleUser><Arn>arn:aws:sts::444455556666:assumed-role/app/s</Arn><AssumedRoleId>AROA:s</AssumedRoleId></AssumedRoleUser></AssumeRoleResult><ResponseMetadata><RequestId>1</RequestId></ResponseMetadata></AssumeRoleResponse>"#;
const STS_DENIED: &str = r#"<ErrorResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><Error><Type>Sender</Type><Code>AccessDenied</Code><Message>denied</Message></Error><RequestId>1</RequestId></ErrorResponse>"#;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Route {
    Token,
    Portal,
    Sts,
}

struct Reply {
    status: u16,
    body: String,
    gate: Option<Arc<Semaphore>>,
}

#[derive(Default)]
struct FakeState {
    replies: HashMap<Route, VecDeque<Reply>>,
    seen: Vec<(Route, String)>,
}

#[derive(Clone)]
struct Fake {
    origin: String,
    state: Arc<Mutex<FakeState>>,
}

impl Fake {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FakeState::default()));
        let serving = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(stream, Arc::clone(&serving)));
            }
        });
        Self { origin, state }
    }

    fn reply(&self, route: Route, status: u16, body: &str) {
        self.push(route, status, body, None);
    }

    fn held(&self, route: Route, status: u16, body: &str) -> Arc<Semaphore> {
        let gate = Arc::new(Semaphore::new(0));
        self.push(route, status, body, Some(Arc::clone(&gate)));
        gate
    }

    fn push(&self, route: Route, status: u16, body: &str, gate: Option<Arc<Semaphore>>) {
        let reply = Reply {
            status,
            body: body.to_owned(),
            gate,
        };
        let mut state = self.state.lock().unwrap();
        state.replies.entry(route).or_default().push_back(reply);
    }

    fn seen(&self) -> Vec<(Route, String)> {
        self.state.lock().unwrap().seen.clone()
    }
}

async fn serve(mut stream: tokio::net::TcpStream, state: Arc<Mutex<FakeState>>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let (head, body) = loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
        let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buffer[..end]).to_lowercase();
        let length = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if buffer.len() >= end + 4 + length {
            let body = String::from_utf8_lossy(&buffer[end + 4..end + 4 + length]).to_string();
            break (head, body);
        }
    };
    let line = head.lines().next().unwrap_or_default();
    let route = if line.contains("/token") {
        Route::Token
    } else if line.contains("/federation/credentials") {
        Route::Portal
    } else {
        Route::Sts
    };
    let carried = match route {
        Route::Portal => head
            .lines()
            .find_map(|l| l.strip_prefix("x-amz-sso_bearer_token:"))
            .unwrap_or_default()
            .trim()
            .to_owned(),
        Route::Token => serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["refreshToken"].as_str().map(str::to_owned))
            .unwrap_or_default(),
        Route::Sts => String::new(),
    };
    let reply = {
        let mut state = state.lock().unwrap();
        state.seen.push((route, carried));
        state.replies.get_mut(&route).and_then(VecDeque::pop_front)
    };
    let reply = reply.unwrap_or(Reply {
        status: 599,
        body: String::new(),
        gate: None,
    });
    if let Some(gate) = &reply.gate {
        let _ = gate.acquire().await;
    }
    let response = format!(
        "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

fn rfc3339(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).unwrap().as_secs();
    chrono::DateTime::from_timestamp(i64::try_from(secs).unwrap(), 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn token_json(access: &str, expires_in: Duration) -> String {
    let now = SystemTime::now();
    format!(
        r#"{{"accessToken":"{access}","expiresAt":"{}","refreshToken":"refresh-{access}","clientId":"client","clientSecret":"client-secret","registrationExpiresAt":"{}","region":"us-east-1","startUrl":"{START_URL}","extra":"dropped"}}"#,
        rfc3339(now + expires_in),
        rfc3339(now + Duration::from_secs(86_400 * 30)),
    )
}

fn portal_ok() -> String {
    let expiration = (SystemTime::now() + Duration::from_secs(3600))
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!(
        r#"{{"roleCredentials":{{"accessKeyId":"ASIAPORTAL","secretAccessKey":"portal-secret","sessionToken":"portal-token","expiration":{expiration}}}}}"#
    )
}

fn oidc_ok(access: &str) -> String {
    format!(
        r#"{{"accessToken":"{access}","expiresIn":28800,"refreshToken":"refresh-{access}","tokenType":"Bearer"}}"#
    )
}

struct Owner {
    _dir: tempfile::TempDir,
    source: OwnerSource,
    state: StateRoot,
}

impl Owner {
    fn new(config: &str, profile: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let aws = dir.path().join("home/.aws");
        let cache = aws.join("sso/cache");
        std::fs::create_dir_all(&cache).unwrap();
        for path in [
            dir.path().join("home"),
            aws.clone(),
            aws.join("sso"),
            cache.clone(),
        ] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        let owner = Self {
            source: OwnerSource {
                profile: profile.into(),
                region: "us-west-2".into(),
                config_file: aws.join("config"),
                credentials_file: aws.join("credentials"),
                sso_cache_root: cache,
            },
            state: StateRoot::resolve(Some(&data)).unwrap(),
            _dir: dir,
        };
        write_private(&owner.source.config_file, config);
        owner
    }

    fn token_path(&self) -> PathBuf {
        self.source.sso_cache_root.join(TOKEN_FILE)
    }

    fn write_token(&self, body: &str) {
        write_private(&self.token_path(), body);
    }

    fn token_bytes(&self) -> Vec<u8> {
        std::fs::read(self.token_path()).unwrap()
    }

    async fn run(
        &self,
        slot: &mut TransactionSlot,
        fake: &Fake,
        predecessor: Option<&PrivateToken>,
        limits: HelperLimits,
    ) -> TransactionOutcome {
        let input = TransactionInput {
            source: &self.source,
            predecessor,
            executable: Path::new(BIN),
            state_root: &self.state,
            budget: Duration::from_secs(30),
            limits,
            test_origin: Some(fake.origin.clone()),
        };
        slot.run(input, &CancellationToken::new()).await
    }

    fn scratch_dirs(&self) -> usize {
        std::fs::read_dir(self.state.run_root().unwrap())
            .unwrap()
            .count()
    }
}

fn write_private(path: &Path, body: &str) {
    let staged = path.with_extension("staged");
    std::fs::write(&staged, body).unwrap();
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::rename(staged, path).unwrap();
}

fn short_wall() -> HelperLimits {
    HelperLimits {
        wall: Duration::from_secs(2),
        ..HelperLimits::default()
    }
}

fn assert_no_helper_processes(owner: &Owner) {
    let home = format!("HOME={}", owner.state.path().display());
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        let ours = environ
            .split(|b| *b == 0)
            .any(|var| var.starts_with(home.as_bytes()));
        assert!(!ours, "a helper outlived its transaction");
    }
}

fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settled_sso_transaction_returns_a_row_and_leaves_the_owner_cache_alone() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let before = (owner.token_bytes(), modified(&owner.token_path()));
    let fake = Fake::start().await;
    fake.reply(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(outcome.failure, None);
    assert_eq!(outcome.row.expect("row").access_key_id, "ASIAPORTAL");
    assert_eq!(outcome.renewal, RenewalEvidence::NotStarted);
    assert!(outcome.successor.is_none() && !outcome.lost_succession);
    assert_eq!(fake.seen(), [(Route::Portal, "owner-access".to_owned())]);
    let after = (owner.token_bytes(), modified(&owner.token_path()));
    assert_eq!(before, after, "the owner cache is never written");
    assert_eq!(
        owner.scratch_dirs(),
        0,
        "scratch is removed before the outcome"
    );
    assert!(!slot.is_unresolved());
    assert_no_helper_processes(&owner);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tighter_inherited_hard_limit_caps_the_helper_instead_of_refusing_it() {
    let _serial = SERIAL.lock().await;
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    // The inherited 8 MiB hard stack limit, as `ulimit -s 8192` or systemd
    // `LimitSTACK=8M` sets it, caps the helper's 16 MiB stack request. Raising a hard
    // limit requires privilege, so lowering it is irreversible for this test process.
    let inherited = 8 * 1024 * 1024;
    let current = getrlimit(Resource::Stack);
    let hard = current
        .maximum
        .map_or(inherited, |hard| hard.min(inherited));
    setrlimit(
        Resource::Stack,
        Rlimit {
            current: Some(current.current.map_or(hard, |soft| soft.min(hard))),
            maximum: Some(hard),
        },
    )
    .unwrap();
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let fake = Fake::start().await;
    fake.reply(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(outcome.failure, None);
    assert!(outcome.row.is_some());
    assert_eq!(owner.scratch_dirs(), 0);
    assert_no_helper_processes(&owner);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotated_successor_survives_role_failure_and_an_external_login_beats_it() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(&format!("{ROLE_CONFIG}{SSO_CONFIG}"), "app");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let owner_bytes = owner.token_bytes();
    let fake = Fake::start().await;
    fake.reply(Route::Token, 200, &oidc_ok("r2"));
    fake.reply(Route::Portal, 200, &portal_ok());
    fake.reply(Route::Sts, 403, STS_DENIED);
    let mut slot = TransactionSlot::default();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        outcome.failure,
        Some(TransactionFailure::Helper(HelperFailure::ProviderError))
    );
    assert!(outcome.row.is_none());
    assert_eq!(outcome.renewal, RenewalEvidence::Started);
    assert!(!outcome.lost_succession);
    let r2 = outcome
        .successor
        .expect("the rotated successor survives the role failure");
    assert_eq!(
        owner.token_bytes(),
        owner_bytes,
        "the owner cache is unchanged"
    );

    fake.reply(Route::Portal, 200, &portal_ok());
    fake.reply(Route::Sts, 200, STS_OK);
    let next = owner
        .run(&mut slot, &fake, Some(&r2), HelperLimits::default())
        .await;
    assert_eq!(next.row.expect("row").access_key_id, "ASIAROLE");
    let portal_bearers: Vec<_> = fake
        .seen()
        .into_iter()
        .filter(|(route, _)| *route == Route::Portal)
        .map(|(_, bearer)| bearer)
        .collect();
    assert_eq!(
        portal_bearers,
        ["r2", "r2"],
        "the successor serves the next renewal"
    );

    owner.write_token(&token_json("r3", Duration::from_secs(3600)));
    fake.reply(Route::Portal, 200, &portal_ok());
    fake.reply(Route::Sts, 200, STS_OK);
    let replaced = owner
        .run(&mut slot, &fake, Some(&r2), HelperLimits::default())
        .await;
    assert!(replaced.row.is_some());
    let last_portal = fake
        .seen()
        .into_iter()
        .rev()
        .find(|(route, _)| *route == Route::Portal)
        .unwrap();
    assert_eq!(
        last_portal.1, "r3",
        "an external login replaces the private successor"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_observed_withdrawal_defeats_the_stale_result() {
    let _serial = SERIAL.lock().await;
    for mutation in ["replace", "delete", "malformed"] {
        let owner = Owner::new(SSO_CONFIG, "dev");
        owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
        let fake = Fake::start().await;
        let gate = fake.held(Route::Portal, 200, &portal_ok());
        let mut slot = TransactionSlot::default();
        let run = owner.run(&mut slot, &fake, None, HelperLimits::default());
        let mutate = async {
            while fake.seen().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            match mutation {
                "replace" => owner.write_token(&token_json("other", Duration::from_secs(3600))),
                "delete" => std::fs::remove_file(owner.token_path()).unwrap(),
                _ => owner.write_token("{not json"),
            }
            gate.add_permits(1);
        };
        let (outcome, ()) = tokio::join!(run, mutate);
        assert_eq!(
            outcome.failure,
            Some(TransactionFailure::Withdrawn),
            "{mutation}"
        );
        assert!(
            outcome.row.is_none() && outcome.successor.is_none(),
            "{mutation}"
        );
        assert_eq!(owner.scratch_dirs(), 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_killed_before_saving_reports_lost_succession() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let fake = Fake::start().await;
    let _never = fake.held(Route::Token, 200, &oidc_ok("r2"));
    let mut slot = TransactionSlot::default();
    let started = Instant::now();
    let outcome = owner.run(&mut slot, &fake, None, short_wall()).await;
    assert!(
        started.elapsed() >= Duration::from_secs(2),
        "{:?} {:?} {:?}",
        started.elapsed(),
        outcome.failure,
        outcome.renewal
    );
    assert_eq!(outcome.failure, Some(TransactionFailure::HelperUnreported));
    assert_eq!(outcome.renewal, RenewalEvidence::Unknown);
    assert!(outcome.successor.is_none());
    assert!(
        outcome.lost_succession,
        "a possible rotation without a successor needs login"
    );
    assert_eq!(owner.scratch_dirs(), 0);
    assert!(!slot.is_unresolved());
    assert_no_helper_processes(&owner);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_helper_killed_after_saving_has_its_successor_salvaged() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let fake = Fake::start().await;
    fake.reply(Route::Token, 200, &oidc_ok("r2"));
    let _never = fake.held(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let outcome = owner.run(&mut slot, &fake, None, short_wall()).await;
    assert_eq!(outcome.failure, Some(TransactionFailure::HelperUnreported));
    assert_eq!(outcome.renewal, RenewalEvidence::Unknown);
    let successor = outcome.successor.expect("the saved successor is salvaged");
    assert!(!outcome.lost_succession);
    assert_eq!(fake.seen()[0], (Route::Token, "refresh-r1".to_owned()));

    fake.reply(Route::Portal, 200, &portal_ok());
    let next = owner
        .run(&mut slot, &fake, Some(&successor), HelperLimits::default())
        .await;
    assert!(next.row.is_some());
    assert_eq!(fake.seen().last().unwrap().1, "r2");
    assert_no_helper_processes(&owner);
}

#[tokio::test(flavor = "multi_thread")]
async fn resource_limits_fail_without_partial_adoption() {
    let _serial = SERIAL.lock().await;
    use RenewalEvidence::{NotStarted, Started, Unknown};
    let limit = |change: fn(&mut HelperLimits)| {
        let mut limits = HelperLimits::default();
        change(&mut limits);
        limits
    };
    let unhandled = Some(TransactionFailure::Helper(HelperFailure::Unhandled));
    let unreported = Some(TransactionFailure::HelperUnreported);
    let provider = Some(TransactionFailure::Helper(HelperFailure::ProviderError));
    let cases = [
        (
            "descriptors",
            limit(|l| l.open_files = 4),
            unhandled,
            NotStarted,
            0,
        ),
        (
            "address space",
            limit(|l| l.address_space_bytes = 16 << 20),
            unreported,
            NotStarted,
            0,
        ),
        ("cpu", limit(|l| l.cpu_seconds = 0), unreported, Unknown, 0),
        (
            "successor save",
            limit(|l| l.file_size_bytes = 256),
            provider,
            Started,
            1,
        ),
    ];
    for (name, limits, failure, renewal, requests) in cases {
        let owner = Owner::new(SSO_CONFIG, "dev");
        owner.write_token(&token_json("r1", Duration::from_secs(60)));
        let owner_bytes = owner.token_bytes();
        let fake = Fake::start().await;
        fake.reply(Route::Token, 200, &oidc_ok(&"r2".repeat(200)));
        fake.reply(Route::Portal, 200, &portal_ok());
        let mut slot = TransactionSlot::default();
        let outcome = owner.run(&mut slot, &fake, None, limits).await;
        assert_eq!(outcome.failure, failure, "{name}");
        // SIGXCPU can land before or after the request reaches the helper; either way
        // only a delivered request leaves renewal unknown.
        if name.starts_with("cpu") {
            assert!(matches!(outcome.renewal, NotStarted | Unknown), "{name}");
        } else {
            assert_eq!(outcome.renewal, renewal, "{name}");
        }
        assert_eq!(fake.seen().len(), requests, "{name}: requests reached");
        assert!(outcome.row.is_none(), "{name}: no row");
        assert!(outcome.successor.is_none(), "{name}: no partial successor");
        assert_eq!(
            outcome.lost_succession,
            outcome.renewal != NotStarted,
            "{name}"
        );
        assert_eq!(
            owner.token_bytes(),
            owner_bytes,
            "{name}: owner cache unchanged"
        );
        assert_eq!(owner.scratch_dirs(), 0, "{name}: scratch removed");
        assert!(!slot.is_unresolved(), "{name}");
        assert_no_helper_processes(&owner);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawal_after_rotation_discards_the_successor() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let fake = Fake::start().await;
    fake.reply(Route::Token, 200, &oidc_ok("r2"));
    let gate = fake.held(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let run = owner.run(&mut slot, &fake, None, HelperLimits::default());
    let mutate = async {
        while fake.seen().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        owner.write_token(&token_json("relogin", Duration::from_secs(3600)));
        gate.add_permits(1);
    };
    let (outcome, ()) = tokio::join!(run, mutate);
    assert_eq!(outcome.failure, Some(TransactionFailure::Withdrawn));
    assert!(outcome.row.is_none() && outcome.successor.is_none());
    assert!(
        !outcome.lost_succession,
        "the new external login supersedes the rotation"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_metadata_change_keeps_the_rotated_successor() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let fake = Fake::start().await;
    fake.reply(Route::Token, 200, &oidc_ok("r2"));
    let gate = fake.held(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let run = owner.run(&mut slot, &fake, None, HelperLimits::default());
    let touch = async {
        while fake.seen().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let path = owner.token_path();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        gate.add_permits(1);
    };
    let (outcome, ()) = tokio::join!(run, touch);
    assert_eq!(outcome.failure, None, "a chmod is not a new login");
    let r2 = outcome
        .successor
        .expect("the successor survives a metadata change");

    fake.reply(Route::Portal, 200, &portal_ok());
    let next = owner
        .run(&mut slot, &fake, Some(&r2), HelperLimits::default())
        .await;
    assert!(next.row.is_some());
    assert_eq!(
        fake.seen().last().unwrap().1,
        "r2",
        "the rotated successor, never stale r1"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_owner_token_after_rotation_reports_lost_succession() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let fake = Fake::start().await;
    fake.reply(Route::Token, 200, &oidc_ok("r2"));
    let gate = fake.held(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let run = owner.run(&mut slot, &fake, None, HelperLimits::default());
    let delete = async {
        while fake.seen().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::remove_file(owner.token_path()).unwrap();
        gate.add_permits(1);
    };
    let (outcome, ()) = tokio::join!(run, delete);
    assert_eq!(outcome.failure, Some(TransactionFailure::Withdrawn));
    assert!(outcome.successor.is_none());
    assert!(
        outcome.lost_succession,
        "a logout after rotation needs a new login"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edited_session_refuses_the_private_predecessor() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("r1", Duration::from_secs(60)));
    let fake = Fake::start().await;
    fake.reply(Route::Token, 200, &oidc_ok("r2"));
    fake.reply(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    let r2 = outcome.successor.expect("rotated successor");
    let seen = fake.seen().len();
    write_private(
        &owner.source.config_file,
        &SSO_CONFIG.replace("d-1234567890", "d-0987654321"),
    );
    let outcome = owner
        .run(&mut slot, &fake, Some(&r2), HelperLimits::default())
        .await;
    assert_eq!(outcome.failure, Some(TransactionFailure::LoginRequired));
    assert_eq!(
        fake.seen().len(),
        seen,
        "no request with a predecessor for another session"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unproven_scratch_cleanup_leaves_the_slot_unresolved() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let fake = Fake::start().await;
    let gate = fake.held(Route::Portal, 200, &portal_ok());
    let run_root = owner.state.run_root().unwrap();
    let mut slot = TransactionSlot::default();
    let run = owner.run(&mut slot, &fake, None, HelperLimits::default());
    let lock_scratch = async {
        while fake.seen().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::set_permissions(&run_root, std::fs::Permissions::from_mode(0o500)).unwrap();
        gate.add_permits(1);
    };
    let (outcome, ()) = tokio::join!(run, lock_scratch);
    std::fs::set_permissions(&run_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(outcome.failure, Some(TransactionFailure::CleanupUnproven));
    assert!(
        outcome.row.is_none(),
        "no row from an unsettled transaction"
    );
    assert!(slot.is_unresolved());
    let refused = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(refused.failure, Some(TransactionFailure::CleanupUnproven));
    assert_eq!(fake.seen().len(), 1, "an unresolved slot launches nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_during_the_helper_run_reports_cancelled() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let fake = Fake::start().await;
    let _never = fake.held(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let cancel = CancellationToken::new();
    let input = TransactionInput {
        source: &owner.source,
        predecessor: None,
        executable: Path::new(BIN),
        state_root: &owner.state,
        budget: Duration::from_secs(30),
        limits: HelperLimits::default(),
        test_origin: Some(fake.origin.clone()),
    };
    let run = slot.run(input, &cancel);
    let cancel_once_running = async {
        while fake.seen().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cancel.cancel();
    };
    let started = Instant::now();
    let (outcome, ()) = tokio::join!(run, cancel_once_running);
    assert!(
        started.elapsed() < HelperLimits::default().wall,
        "cancellation ends the run before the wall limit: {:?}",
        started.elapsed()
    );
    assert_eq!(outcome.failure, Some(TransactionFailure::Cancelled));
    assert!(outcome.row.is_none() && outcome.successor.is_none());
    assert_eq!(owner.scratch_dirs(), 0);
    assert!(!slot.is_unresolved());
    assert_no_helper_processes(&owner);
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_failure_and_cancellation_leave_the_slot_reusable() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let fake = Fake::start().await;
    let mut slot = TransactionSlot::default();
    let input = TransactionInput {
        source: &owner.source,
        predecessor: None,
        executable: Path::new("/nonexistent/eidnara-host"),
        state_root: &owner.state,
        budget: Duration::from_secs(30),
        limits: HelperLimits::default(),
        test_origin: Some(fake.origin.clone()),
    };
    let outcome = slot.run(input, &CancellationToken::new()).await;
    assert_eq!(outcome.failure, Some(TransactionFailure::Spawn));
    assert_eq!(outcome.renewal, RenewalEvidence::NotStarted);
    assert!(!outcome.lost_succession && !slot.is_unresolved());

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let input = TransactionInput {
        source: &owner.source,
        predecessor: None,
        executable: Path::new(BIN),
        state_root: &owner.state,
        budget: Duration::from_secs(30),
        limits: HelperLimits::default(),
        test_origin: Some(fake.origin.clone()),
    };
    let outcome = slot.run(input, &cancelled).await;
    assert_eq!(outcome.failure, Some(TransactionFailure::Cancelled));
    assert!(!slot.is_unresolved());
    assert!(fake.seen().is_empty());

    fake.reply(Route::Portal, 200, &portal_ok());
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert!(
        outcome.row.is_some(),
        "the slot admits the next transaction"
    );
    assert_eq!(owner.scratch_dirs(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_transaction_leaves_the_slot_unresolved() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let fake = Fake::start().await;
    let _never = fake.held(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let run = owner.run(&mut slot, &fake, None, HelperLimits::default());
    let dropped = tokio::time::timeout(Duration::from_millis(500), run).await;
    assert!(dropped.is_err(), "the transaction was still running");
    assert!(
        slot.is_unresolved(),
        "an unsettled transaction never frees the slot"
    );
    let refused = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(refused.failure, Some(TransactionFailure::CleanupUnproven));
}

#[tokio::test(flavor = "multi_thread")]
async fn sticky_shared_ancestors_admit_only_private_descendants() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    // Every fixture lives below the shared sticky temporary directory.
    let sticky = std::fs::metadata(std::env::temp_dir())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(
        sticky & 0o1022,
        0o1022,
        "the temporary directory is shared and sticky"
    );
    let private = owner.source.config_file.parent().unwrap().join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    let fake = Fake::start().await;
    let mut slot = TransactionSlot::default();
    let mut owner = owner;

    owner.source.config_file = private.join("config");
    write_private(&owner.source.config_file, SSO_CONFIG);
    fake.reply(Route::Portal, 200, &portal_ok());
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert!(
        outcome.row.is_some(),
        "a private descendant of a sticky directory admits: {:?}",
        outcome.failure
    );

    std::fs::set_permissions(
        &owner.source.config_file,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        outcome.failure,
        Some(TransactionFailure::Capture(CaptureFailure::Unsafe)),
        "a readable file below a shared ancestor"
    );

    std::fs::set_permissions(
        &owner.source.config_file,
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        outcome.failure,
        Some(TransactionFailure::Capture(CaptureFailure::Unsafe)),
        "a shared descendant of a sticky directory"
    );

    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::hard_link(&owner.source.config_file, private.join("alias")).unwrap();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        outcome.failure,
        Some(TransactionFailure::Capture(CaptureFailure::Unsafe)),
        "a hard-linked file below a shared ancestor"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_owner_token_needs_a_new_login() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    let token = token_json("owner-access", Duration::from_secs(3600)).replace(
        r#""extra":"dropped""#,
        &format!(r#""extra":"{}""#, "x".repeat(64 * 1024)),
    );
    owner.write_token(&token);
    let fake = Fake::start().await;
    let mut slot = TransactionSlot::default();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(outcome.failure, Some(TransactionFailure::LoginRequired));
    assert!(fake.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unsafe_owner_files_are_refused_before_any_helper() {
    let _serial = SERIAL.lock().await;
    let fake = Fake::start().await;
    let refused = |outcome: TransactionOutcome| match outcome.failure {
        Some(TransactionFailure::Capture(failure)) => failure,
        other => panic!("expected a capture refusal, got {other:?}"),
    };
    let owner = Owner::new(SSO_CONFIG, "dev");
    let mut slot = TransactionSlot::default();
    let config = owner.source.config_file.clone();
    let mode = |path: &Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };

    mode(&config, 0o620);
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        refused(outcome),
        CaptureFailure::Unsafe,
        "group-writable config"
    );

    std::fs::remove_file(&config).unwrap();
    std::os::unix::fs::symlink("/etc/hostname", &config).unwrap();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(refused(outcome), CaptureFailure::Unsafe, "symlinked config");

    std::fs::remove_file(&config).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &config,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(refused(outcome), CaptureFailure::Unsafe, "FIFO config");

    std::fs::remove_file(&config).unwrap();
    write_private(
        &config,
        &format!("{SSO_CONFIG}#{}\n", "x".repeat(256 * 1024)),
    );
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        refused(outcome),
        CaptureFailure::TooLarge,
        "oversized config"
    );

    write_private(&config, SSO_CONFIG);
    write_private(&owner.source.credentials_file, "[keys]\n");
    mode(&owner.source.credentials_file, 0o640);
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        refused(outcome),
        CaptureFailure::Unsafe,
        "readable credentials"
    );

    std::fs::remove_file(&owner.source.credentials_file).unwrap();
    let aws = config.parent().unwrap();
    mode(aws, 0o777);
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        refused(outcome),
        CaptureFailure::Unsafe,
        "replaceable ancestor"
    );
    mode(aws, 0o700);

    std::fs::remove_file(&config).unwrap();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(refused(outcome), CaptureFailure::Missing, "missing config");

    write_private(&config, SSO_CONFIG);
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        outcome.failure,
        Some(TransactionFailure::LoginRequired),
        "missing token"
    );
    let foreign = token_json("x", Duration::from_secs(3600)).replace("us-east-1", "eu-west-1");
    owner.write_token(&foreign);
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert_eq!(
        outcome.failure,
        Some(TransactionFailure::LoginRequired),
        "foreign region"
    );
    let process = SSO_CONFIG.replace(
        "[profile dev]\n",
        "[profile dev]\ncredential_process = /bin/sh -c 'touch ran'\n",
    );
    write_private(&config, &process);
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert!(
        matches!(outcome.failure, Some(TransactionFailure::Admission(_))),
        "a process source never reaches the helper"
    );
    assert!(!config.with_file_name("ran").exists());
    assert!(fake.seen().is_empty(), "no refusal reaches the network");
    assert_eq!(owner.scratch_dirs(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn abandoned_scratch_is_cleanup_debt_never_a_restart_credential() {
    let _serial = SERIAL.lock().await;
    let owner = Owner::new(SSO_CONFIG, "dev");
    owner.write_token(&token_json("owner-access", Duration::from_secs(3600)));
    let run_root = owner.state.run_root().unwrap();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap()
        .trim()
        .replace('-', "");
    let valid = token_json("abandoned", Duration::from_secs(3600));
    for (name, token) in [
        ("valid", valid.clone()),
        ("truncated", valid[..40].to_owned()),
        ("foreign", "{}".to_owned()),
    ] {
        let dead = run_root.join(format!("aws-helper-{boot}-999999999-1-{name}"));
        let cache = dead.join(".aws/sso/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join(TOKEN_FILE), token).unwrap();
    }
    let fake = Fake::start().await;
    fake.reply(Route::Portal, 200, &portal_ok());
    let mut slot = TransactionSlot::default();
    let outcome = owner
        .run(&mut slot, &fake, None, HelperLimits::default())
        .await;
    assert!(outcome.row.is_some());
    assert_eq!(fake.seen(), [(Route::Portal, "owner-access".to_owned())]);
    let removed = group_registry::sweep_orphaned_run_dirs(&owner.state).unwrap();
    assert_eq!(removed, 3, "the startup sweep removes abandoned scratch");
    assert_eq!(owner.scratch_dirs(), 0);
}
