//! The deployment owner's side of the MemoryReviewer activation gate for tests over real stores: the activation record a matching deployment writes, written with the permissions the gate requires, and the public-run backend an internal-only supervisor carries.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use daemon::memory_reviewer::activation::{
    ACTIVATION_DIR, IDENTITY_RECORD, IDENTITY_SCHEMA, LiveIdentity,
};
use daemon::memory_reviewer::steps::STEP_VERSION;
use host_runtime::model_execution::backend::{
    BackendFuture, BackendRequest, EventSink, LlmExecutionBackend,
};
use tokio_util::sync::CancellationToken;

/// Internal launches need a backend only for public runs; the MemoryReviewer's sender speaks to the provider itself.
pub struct NoPublicModel;

impl LlmExecutionBackend for NoPublicModel {
    fn execute(
        &self,
        _request: BackendRequest,
        _events: EventSink,
        _cancel: CancellationToken,
    ) -> BackendFuture {
        Box::pin(async { unreachable!("no public run is started") })
    }
}

/// The identity terms one deployment's activation record names.
pub struct Deployment<'a> {
    pub model: &'a str,
    pub kernel_incarnation: &'a str,
    pub memstore_incarnation: &'a str,
    pub provider: &'a str,
    pub credential: &'a str,
    pub credential_fingerprint: &'a str,
}

/// The schema 1 activation record for exactly `deployment`, with the current template, step, scanner, policy, and baseline versions.
pub fn activation_record(deployment: &Deployment<'_>) -> serde_json::Value {
    serde_json::json!({
        "schema": IDENTITY_SCHEMA,
        "model": deployment.model,
        "prompt_template_version": LiveIdentity::prompt_template_version(),
        "step_schema_version": STEP_VERSION,
        "scanner_ruleset_version": LiveIdentity::scanner_ruleset_version(),
        "egress_policy_version": context_core::memory_reviewer_policy_union::MEMORY_REVIEWER_POLICY_UNION_VERSION,
        "kernel_baseline_digest": kernel::kernel_baseline_digest().unwrap(),
        "memstore_baseline_digest": memory_store::baseline_digest(),
        "kernel_incarnation": deployment.kernel_incarnation,
        "memstore_incarnation": deployment.memstore_incarnation,
        "provider": deployment.provider,
        "credential": deployment.credential,
        "credential_fingerprint": deployment.credential_fingerprint,
        "provider_retention": {
            "attested_by": "deployment owner",
            "attested_on": "2026-09-18",
            "retention_terms": "test peer; nothing leaves the host",
            "finite_work_exposure_acknowledged": true
        }
    })
}

/// Writes `record` under `home` as the owner does: the directory `0700`, the record `0600`.
pub fn write_activation_record(home: &Path, record: &serde_json::Value) {
    let dir = home.join(ACTIVATION_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join(IDENTITY_RECORD);
    std::fs::write(&path, serde_json::to_vec(record).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
