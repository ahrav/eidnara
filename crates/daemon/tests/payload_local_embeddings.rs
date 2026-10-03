//! External-runtime witnesses for a built host payload package.

#![cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use host_runtime::{Client, RequestOptions, RouteIdentity, RouteTarget, TargetKind};
use serde_json::Value;
use sha2::Digest as _;

const BIN: &str = env!("CARGO_BIN_EXE_eidnara-host");
const BUNDLE_DIR: &str = "payload/model/gte-modernbert-base-f32";
const ORT_LIBRARY: &str = "payload/ort/libonnxruntime.so";
const SETTLE_DEADLINE: Duration = Duration::from_secs(900);

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn payload_dir() -> PathBuf {
    std::env::var_os("EIDNARA_HOST_TEST_PAYLOAD_DIR")
        .map(PathBuf::from)
        .expect("EIDNARA_HOST_TEST_PAYLOAD_DIR must name a built payload package directory")
}

fn manifest_digest(dir: &Path) -> String {
    let bytes = std::fs::read(dir.join("payload-manifest.json")).expect("payload manifest");
    sha256(bytes.strip_suffix(b"\n").unwrap_or(&bytes))
}

fn lifecycle(data: &Path, args: &[&str]) -> Value {
    let output = Command::new(BIN)
        .args(args)
        .env_clear()
        .env("XDG_DATA_HOME", data)
        .env("EIDNARA_HOST_TEST_PHASE_CAP_MS", "30000")
        .output()
        .expect("eidnara-host runs");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "eidnara-host {args:?} printed no result: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

struct Daemon {
    data: tempfile::TempDir,
}

impl Daemon {
    fn start(payload: &Path) -> Self {
        let data = tempfile::tempdir().expect("data root");
        let digest = manifest_digest(payload);
        let result = lifecycle(
            data.path(),
            &[
                "start",
                "--payload-dir",
                payload.to_str().expect("payload path"),
                "--payload-manifest-digest",
                &digest,
            ],
        );
        assert_eq!(result["ok"], true, "start failed: {result}");
        Self { data }
    }

    async fn client(&self) -> Client {
        let publication = host_runtime::runtime_dir_path(Some(self.data.path()))
            .expect("runtime dir")
            .join(host_runtime::CONNECTION_FILE_NAME);
        Client::connect(&publication)
            .await
            .expect("published daemon authenticates")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new(BIN)
            .arg("stop")
            .env_clear()
            .env("XDG_DATA_HOME", self.data.path())
            .output();
    }
}

async fn settled_lane_state(client: &Client) -> (String, Duration) {
    let started = tokio::time::Instant::now();
    loop {
        let status = client.host_status().await.expect("host status");
        let state =
            status.metrics["components"]["local_embeddings"]["metrics"]["local_embeddings_state"]
                .as_str()
                .unwrap_or("absent")
                .to_owned();
        if state != "starting" {
            return (state, started.elapsed());
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "the lane never settled: {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
#[ignore = "requires EIDNARA_HOST_TEST_PAYLOAD_DIR; run with --ignored"]
async fn a_built_payload_reaches_ready_and_serves_a_certified_embedding() {
    let payload = payload_dir();
    let daemon = Daemon::start(&payload);
    let client = daemon.client().await;
    let (state, elapsed) = settled_lane_state(&client).await;
    assert_eq!(state, "ready");
    eprintln!("local_embeddings ready after {elapsed:?} from first health read");

    let bundle = payload.join(BUNDLE_DIR);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).expect("manifest"))
            .expect("manifest json");
    let corpus: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("corpus.json")).expect("corpus"))
            .expect("corpus json");
    let route = client
        .open_route(
            RouteTarget {
                module_id: "local_embeddings".to_owned(),
                kind: TargetKind::ManagementSurface,
            },
            RouteIdentity {
                project_root: daemon.data.path().to_path_buf(),
                harness: "opencode".to_owned(),
                session: "payload-witness".to_owned(),
                consumer_module_id: None,
                consumer_launch_nonce: None,
                consumer_capabilities: Vec::new(),
                admission_facts: None,
                credential_fingerprints: Default::default(),
            },
        )
        .await
        .expect("local_embeddings route opens");
    let probe = &corpus["items"][0];
    let body = serde_json::json!({
        "method": "embed.query",
        "params": {
            "text": probe["text"],
            "model": manifest["model"],
            "required_fingerprint": manifest["fingerprint"],
            "required_epoch": manifest["table_epoch"],
            "allow_equivalent": false,
            "accept_declared": false,
        }
    });
    let response = client
        .request(
            route,
            serde_json::to_vec(&body).expect("request body"),
            RequestOptions::default(),
        )
        .await
        .expect("embed.query answers");
    let response: Value = serde_json::from_slice(&response.body).expect("response json");
    let result = &response["result"];
    assert_eq!(result["fingerprint"], manifest["fingerprint"], "{response}");
    assert_eq!(result["model"], manifest["model"], "{response}");
    assert_eq!(result["vectors"].as_array().map(Vec::len), Some(1));
    let got = result["vectors"][0]["vector"].as_array().expect("vector");
    let expected = probe["expected"].as_array().expect("expected");
    assert_eq!(got.len(), 768);
    assert_eq!(expected.len(), got.len());
    let tolerance = corpus["tolerance"].as_f64().expect("tolerance");
    for (got, expected) in got.iter().zip(expected) {
        let drift =
            (got.as_f64().expect("component") - expected.as_f64().expect("component")).abs();
        assert!(
            drift <= tolerance,
            "served vector drifts {drift} from the corpus"
        );
    }
}

fn copy_payload(source: &Path, into: &Path) {
    let status = Command::new("cp")
        .args(["-a", "--reflink=auto"])
        .arg(source.join("payload"))
        .arg(source.join("payload-manifest.json"))
        .arg(into)
        .status()
        .expect("cp runs");
    assert!(status.success(), "payload copy failed");
}

fn payload_with_broken_runtime(source: &Path, into: &Path) {
    copy_payload(source, into);
    let broken = std::fs::read(into.join("payload/native/shm_native.node")).expect("addon bytes");
    std::fs::write(into.join(ORT_LIBRARY), &broken).expect("replace runtime");
    let manifest_path = into.join("payload-manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("manifest"))
            .expect("manifest json");
    for entry in manifest["files"].as_array_mut().expect("files") {
        if entry["path"] == ORT_LIBRARY {
            entry["size"] = broken.len().into();
            entry["sha256"] = sha256(&broken).into();
        }
    }
    let mut bytes = serde_json::to_vec(&manifest).expect("manifest bytes");
    bytes.push(b'\n');
    std::fs::write(&manifest_path, bytes).expect("manifest write");
}

#[tokio::test]
#[ignore = "requires EIDNARA_HOST_TEST_PAYLOAD_DIR; run with --ignored"]
async fn a_valid_payload_whose_runtime_fails_to_initialize_reports_degraded() {
    // Health exposes no failure detail, so the ready witness over the unmodified payload is this test's control.
    let copy = tempfile::tempdir().expect("payload copy");
    payload_with_broken_runtime(&payload_dir(), copy.path());
    let daemon = Daemon::start(copy.path());
    let client = daemon.client().await;
    let (state, _) = settled_lane_state(&client).await;
    assert_eq!(state, "degraded");
}

#[tokio::test]
#[ignore = "requires EIDNARA_HOST_TEST_PAYLOAD_DIR; run with --ignored"]
async fn a_corrupt_or_missing_manifest_listed_file_refuses_startup() {
    let source = payload_dir();
    let digest = manifest_digest(&source);
    for (case, damage) in [
        ("corrupt ORT library", ORT_LIBRARY),
        (
            "missing corpus",
            "payload/model/gte-modernbert-base-f32/corpus.json",
        ),
    ] {
        let copy = tempfile::tempdir().expect("payload copy");
        copy_payload(&source, copy.path());
        let target = copy.path().join(damage);
        if case.starts_with("corrupt") {
            let mut bytes = std::fs::read(&target).expect("target bytes");
            bytes[0] ^= 0x01;
            std::fs::write(&target, bytes).expect("corrupt");
        } else {
            std::fs::remove_file(&target).expect("remove");
        }
        let data = tempfile::tempdir().expect("data root");
        let result = lifecycle(
            data.path(),
            &[
                "start",
                "--payload-dir",
                copy.path().to_str().expect("payload path"),
                "--payload-manifest-digest",
                &digest,
            ],
        );
        assert_eq!(result["ok"], false, "{case}: {result}");
        assert_eq!(
            result["reason"], "native_payload_invalid",
            "{case}: {result}"
        );
        let publication = host_runtime::runtime_dir_path(Some(data.path()))
            .expect("runtime dir")
            .join(host_runtime::CONNECTION_FILE_NAME);
        assert!(!publication.exists(), "{case}: no host published health");
    }
}
