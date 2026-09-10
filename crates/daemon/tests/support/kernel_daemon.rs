//! `KernelDaemon` starts a `Handler` on test storage with its kernel store open
//! and one route bound, and builds `kernel.*` requests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daemon::dispatch::PreparedOutcome;
use daemon::kernel_routes::KernelState;
use daemon::{Handler, dev_descriptor_at};
use host_runtime::{
    BindOutcome, CompositeComponent, HostInit, PrimaryComponent, RouteHandle, RouteIdentity,
};
use serde_json::{Value, json};
use storage::StorageDescriptor;

pub const SESSION: &str = "stage1-session";
pub const DOMAIN: &str = "stage1-domain";

pub struct KernelDaemon {
    handler: Handler,
    route: RouteHandle,
    project: PathBuf,
    // Fields drop in declaration order; the directory must outlive the handler
    // that holds files inside it.
    _data: tempfile::TempDir,
}

impl KernelDaemon {
    pub async fn start() -> Self {
        Self::start_with_project_config(None).await
    }

    /// Starts the daemon with `project_config` written to the project's
    /// `.eidnara/eidnara.jsonc` before the route binds, so the binding reads it.
    pub async fn start_with_project_config(project_config: Option<Value>) -> Self {
        let data = tempfile::tempdir().unwrap();
        let descriptor: StorageDescriptor = dev_descriptor_at(data.path().to_str().unwrap());
        let handler = Handler::new();
        handler.disable_kernel_sampler_for_test();
        let init = HostInit {
            host_capabilities: Vec::new(),
            storage: Some(serde_json::to_value(&descriptor).unwrap()),
        };
        PrimaryComponent::initialize(&handler, init).await.unwrap();
        PrimaryComponent::activate(&handler).await.unwrap();
        let started = Instant::now();
        while handler.kernel_state() != KernelState::Ready {
            assert_ne!(
                handler.kernel_state(),
                KernelState::Unavailable,
                "kernel store unavailable: {:?}",
                handler.kernel_unavailable_reason_for_test()
            );
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "kernel state stayed {:?}",
                handler.kernel_state()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let project = data.path().join("project");
        fs::create_dir_all(&project).unwrap();
        if let Some(config) = project_config {
            let config_dir = project.join(".eidnara");
            fs::create_dir_all(&config_dir).unwrap();
            fs::write(
                config_dir.join("eidnara.jsonc"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();
        }
        let route = RouteHandle {
            channel: 7,
            epoch: 1,
        };
        let identity = RouteIdentity {
            project_root: project.clone(),
            harness: "test".to_owned(),
            session: SESSION.to_owned(),
            consumer_module_id: None,
            consumer_launch_nonce: None,
            consumer_capabilities: Vec::new(),
            admission_facts: None,
            credential_fingerprints: std::collections::BTreeMap::new(),
        };
        assert!(matches!(
            handler.bind(route, identity).await,
            BindOutcome::Accept
        ));
        Self {
            handler,
            route,
            project,
            _data: data,
        }
    }

    pub async fn call(&self, request: Value) -> Value {
        match self
            .handler
            .dispatch_value_for_test(self.route, request)
            .await
        {
            PreparedOutcome::Response(output) => {
                let mut bytes = Vec::new();
                output.measure().unwrap().write_to(&mut bytes).unwrap();
                serde_json::from_slice(&bytes).unwrap()
            }
            PreparedOutcome::Error { code, message } => {
                panic!("kernel route answered {code}: {message}")
            }
            PreparedOutcome::Streamed => panic!("kernel route streamed"),
        }
    }

    /// Seeds the digest from `key`, matching `tests/kernel_routes.rs`, so a retry under the same key replays.
    pub async fn commit(&self, key: &str, operations: Vec<Value>) -> Value {
        self.commit_with_digest_seed(key, key, operations).await
    }

    /// The kernel refuses a stored `key` whose digest differs, so a `digest_seed` other than `key` reaches that path.
    pub async fn commit_with_digest_seed(
        &self,
        key: &str,
        digest_seed: &str,
        operations: Vec<Value>,
    ) -> Value {
        self.call(commit_request(&self.project, key, digest_seed, operations))
            .await
    }

    pub async fn read(
        &self,
        surface: &str,
        as_of: Option<i64>,
        object_ids: Option<&[&str]>,
    ) -> Value {
        let mut request = read_request(&self.project, surface, as_of);
        if let Some(ids) = object_ids {
            request["object_ids"] = json!(ids);
        }
        self.call(request).await
    }

    pub async fn eligibility(&self, destination: &str, candidates: Vec<Value>) -> Value {
        self.call(json!({
            "method": "kernel.eligibility.batch",
            "v": 1,
            "session_id": SESSION,
            "project_root": self.project.to_str().unwrap(),
            "destination": destination,
            "candidates": candidates,
        }))
        .await
    }

    pub fn store(&self) -> Arc<kernel::KernelStore> {
        self.handler.kernel_store_for_test().unwrap()
    }

    pub fn tip(&self) -> i64 {
        self.store().tip().unwrap()
    }

    pub async fn shutdown(self) {
        self.handler.shutdown().await.unwrap();
    }
}

fn digest(seed: &str) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(seed.as_bytes()))
}

fn commit_request(project: &Path, key: &str, digest_seed: &str, operations: Vec<Value>) -> Value {
    json!({
        "method": "kernel.commit",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "intent": {
            "producer": "plugin",
            "operation_key": key,
            "request_digest": digest(digest_seed),
            "actor": "assistant",
            "cause": "ctx_memory",
        },
        "tokens": [],
        "operations": operations,
        "source_kind": "assistant",
    })
}

fn read_request(project: &Path, surface: &str, as_of: Option<i64>) -> Value {
    json!({
        "method": "kernel.read",
        "v": 1,
        "session_id": SESSION,
        "project_root": project.to_str().unwrap(),
        "surface": surface,
        "as_of": as_of,
        "gated": false,
    })
}

fn decision_spec(index: i64) -> Value {
    json!({
        "decision_id": format!("decision-{index}"),
        "object_id": format!("decision-object-{index}"),
        "domain_id": DOMAIN,
        "decision_kind": "memory",
        "payload": {"summary": format!("decision {index}"), "rationale": format!("because {index}")},
        "source_id": "memory-lineage",
        "source_revision": index,
    })
}

pub fn insert_decision(index: i64) -> Value {
    json!({"op": "insert_decision", "spec": decision_spec(index)})
}

pub fn candidate(object_id: &str, source_revision: i64) -> Value {
    json!({"object_id": object_id, "source_revision": source_revision})
}

pub fn state_kind(value: &Value) -> &str {
    value["state"]["kind"].as_str().unwrap()
}

pub fn state_reason(value: &Value) -> Option<&str> {
    value["state"]["reason"].as_str()
}

pub fn retire_decision(object_id: &str) -> Value {
    json!({"op": "retire_decision", "object_id": object_id})
}

pub fn object_ids(read: &Value) -> Vec<String> {
    let mut ids: Vec<String> = read["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["object"]["object_id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

pub fn verdicts(response: &Value) -> Vec<(String, String)> {
    response["verdicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["object_id"].as_str().unwrap().to_string(),
                entry["verdict"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}
