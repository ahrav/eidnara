//! These tests verify restoration of matched Kernel and Memory Store backups, required artifacts, and mismatched-pair refusal.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use daemon::memory_reviewer::activation::{
    Closed, IDENTITY_SCHEMA, LiveIdentity, RuntimeIdentityRecord, evaluate,
};
use daemon::memory_reviewer::settlement::{ReadRefusal, read_selected_proposal};
use daemon::memory_reviewer::steps::STEP_VERSION;
use kernel::{KernelError, KernelStore};
use memory_store::MemoryStore;
use memory_store::memory_reviewer_ledger::{
    MemoryReviewerLedgerError, MemoryReviewerLedgerRefusal,
};
use support::memory_reviewer_publish::{
    PROJECT, activate_module_authority, begin_job, commit_memory_domain, kernel_incarnation,
    now_ms, publish,
};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EVIDENCE: &[u8] = b"the workspace builds with bun";

/// The daemon stores the Memory Store at `eidnara/context/` and the Kernel at `eidnara/context/kernel/`.
struct DataDir {
    root: PathBuf,
}

impl DataDir {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn context(&self) -> PathBuf {
        self.root.join("eidnara").join("context")
    }

    fn kernel(&self) -> PathBuf {
        self.context().join("kernel")
    }

    fn open_kernel(&self) -> Arc<KernelStore> {
        private_dir(&self.kernel());
        Arc::new(KernelStore::open(self.kernel()).unwrap())
    }

    fn open_store(&self) -> Result<Arc<MemoryStore>, memory_store::MemoryStoreError> {
        MemoryStore::open(&daemon::managed_store_descriptor(&self.root).unwrap()).map(Arc::new)
    }

    fn open_pair(&self) -> (Arc<KernelStore>, Arc<MemoryStore>) {
        (self.open_kernel(), self.open_store().unwrap())
    }
}

/// A private directory, as the Kernel's backup destination and every store root require.
fn private_dir(path: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap();
}

/// Copies the closed `store.db` files; the `.lease` sidecar and the `kernel/` root beside them are left behind.
fn copy_store_family(from: &Path, to: &Path) {
    private_dir(to);
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().unwrap().is_file() && name.starts_with("store.db") {
            std::fs::copy(entry.path(), to.join(&name)).unwrap();
        }
    }
}

/// Copies the Kernel's artifact objects, which the database backup does not carry.
fn copy_artifacts(from_root: &Path, to_root: &Path) {
    fn copy_tree(from: &Path, to: &Path) {
        private_dir(to);
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }
    let objects = Path::new("artifacts").join("objects");
    copy_tree(&from_root.join(&objects), &to_root.join(&objects));
}

/// One evidence artifact in the memory domain, as ingestion leaves it.
fn ingest_evidence(kernel: &KernelStore) -> kernel::ArtifactHandle {
    kernel
        .ingest_artifact(kernel::ArtifactIngestRequest {
            intent: kernel::CommitIntent {
                producer: "memory_reviewer-test".to_string(),
                operation_key: "evidence".to_string(),
                request_digest: "1".repeat(64),
                actor: "test".to_string(),
                cause: "fixture".to_string(),
            },
            payload: EVIDENCE.to_vec(),
            evidence_id: "evidence-1".to_string(),
            object_id: "evidence-object-1".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "memory".to_string(),
            source_kind: "repository".to_string(),
            source_id: "src/build.md".to_string(),
            source_revision: 1,
            media_type: "text/plain".to_string(),
            retention_class: "canonical".to_string(),
            retain_until: None,
            asserted_sensitivity: kernel::Sensitivity::Normal,
            provider_egress: kernel::ProviderEgress::RemoteAllowed,
            provenance: None,
        })
        .unwrap()
}

/// The owner's record for one deployment pair.
fn record(kernel_incarnation: &str, memstore_incarnation: &str) -> RuntimeIdentityRecord {
    serde_json::from_value(serde_json::json!({
        "schema": IDENTITY_SCHEMA,
        "model": "claude-test",
        "prompt_template_version": LiveIdentity::prompt_template_version(),
        "step_schema_version": STEP_VERSION,
        "scanner_ruleset_version": LiveIdentity::scanner_ruleset_version(),
        "egress_policy_version": context_core::memory_reviewer_policy_union::MEMORY_REVIEWER_POLICY_UNION_VERSION,
        "kernel_baseline_digest": kernel::kernel_baseline_digest().unwrap(),
        "memstore_baseline_digest": memory_store::baseline_digest(),
        "kernel_incarnation": kernel_incarnation,
        "memstore_incarnation": memstore_incarnation,
        "provider": "api.anthropic.com/v1/messages@2023-06-01",
        "credential": "ANTHROPIC_API_KEY",
        "credential_fingerprint": "fp-1",
        "provider_retention": {
            "attested_by": "deployment owner",
            "attested_on": "2026-09-18",
            "retention_terms": "test",
            "finite_work_exposure_acknowledged": true
        }
    }))
    .unwrap()
}

fn live(kernel: &KernelStore, store: &MemoryStore) -> LiveIdentity {
    LiveIdentity {
        kernel_baseline_digest: kernel::kernel_baseline_digest().unwrap().to_string(),
        memstore_baseline_digest: memory_store::baseline_digest(),
        kernel_incarnation: kernel_incarnation(kernel),
        memstore_incarnation: store.memory_reviewer_store_incarnation().unwrap(),
        provider: "api.anthropic.com/v1/messages@2023-06-01".into(),
        credentials: vec![("ANTHROPIC_API_KEY".into(), "fp-1".into())],
    }
}

#[test]
fn a_consistent_pair_restores_and_every_mismatched_restore_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let live_dir = DataDir::new(root.path().join("live"));
    let backup = root.path().join("backup");
    private_dir(&backup.join("kernel"));
    let now = now_ms();
    // The live pair: a committed memory domain, one evidence artifact, and a published proposal.
    let (identity, evidence) = {
        let (kernel, store) = live_dir.open_pair();
        commit_memory_domain(&kernel);
        let evidence = ingest_evidence(&kernel);
        let generation = activate_module_authority(&store, &root.path().join("project"));
        let kernel_id = kernel_incarnation(&kernel);
        let begun = begin_job(&kernel, &store, DIGEST, &kernel_id, generation, 1, now);
        publish(&kernel, &store, DIGEST, &kernel_id, &begun, now);
        assert!(
            read_selected_proposal(
                &kernel,
                &store,
                PROJECT,
                &begun.job.causal_identity,
                now + 1
            )
            .is_ok()
        );
        (begun.job.causal_identity, evidence)
    };
    // A second open advances the Memory Store's writer epoch, as a deployment that has restarted stands. The pair is backed up together, with no publication between the two captures: the Kernel through its verified backup, the Memory Store as its closed file family.
    let (kernel_id, store_id, kernel_backup) = {
        let (kernel, store) = live_dir.open_pair();
        let manifest = kernel
            .backup(kernel::BackupRequest {
                destination_directory: backup.join("kernel"),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
                capture_pin_expires_at: None,
            })
            .unwrap();
        assert!(
            manifest.capture_pin_id.is_some(),
            "live evidence pins the capture; the artifact bytes stay in the live root"
        );
        (
            kernel_incarnation(&kernel),
            store.memory_reviewer_store_incarnation().unwrap(),
            manifest.destination_path,
        )
    };
    copy_store_family(&live_dir.context(), &backup.join("store"));
    let owner_record = record(&kernel_id, &store_id);

    // Both restored from the pair into a fresh data directory. The Kernel backup carries no artifact bytes, so the restore is refused until the live root's objects are carried too; the Memory Store family opens without its lease sidecar because the lease is issued above the fence epoch the family records. Then the same incarnations, the receipt still selects the proposal, the review hold is live, the evidence reads, and the owner's record still matches.
    let restored = DataDir::new(root.path().join("restored"));
    {
        let kernel = restored.open_kernel();
        assert_eq!(
            kernel.restore(&kernel_backup).unwrap_err(),
            KernelError::InvalidRestore,
            "a root without the referenced artifacts refuses the backup"
        );
        copy_artifacts(&live_dir.kernel(), &restored.kernel());
        kernel.restore(&kernel_backup).unwrap();
        assert_eq!(kernel_incarnation(&kernel), kernel_id);
        assert_eq!(kernel.read_artifact(&evidence).unwrap(), EVIDENCE);
    }
    copy_store_family(&backup.join("store"), &restored.context());
    {
        let (kernel, store) = restored.open_pair();
        assert_eq!(kernel_incarnation(&kernel), kernel_id);
        assert_eq!(store.memory_reviewer_store_incarnation().unwrap(), store_id);
        let read = read_selected_proposal(&kernel, &store, PROJECT, &identity, now + 2).unwrap();
        assert!(read.review_expires_at > now);
        assert!(evaluate(&owner_record, &live(&kernel, &store)).is_ok());
    }

    // Backup Memory Store beside a replaced Kernel: the receipt binds the old Kernel, so the read, a resumed receipt, and the gate all refuse; the row is exactly as backed up.
    let replaced_kernel = root.path().join("replaced-kernel");
    private_dir(&replaced_kernel);
    {
        let kernel = Arc::new(KernelStore::open(&replaced_kernel).unwrap());
        let store = restored.open_store().unwrap();
        assert_ne!(kernel_incarnation(&kernel), kernel_id);
        assert_eq!(
            read_selected_proposal(&kernel, &store, PROJECT, &identity, now + 3),
            Err(ReadRefusal::IncarnationMismatch)
        );
        let resumed = store.begin_memory_reviewer_receipt(
            PROJECT,
            &identity,
            &kernel_incarnation(&kernel),
            "claim-x",
            now + 3,
        );
        assert!(
            matches!(
                resumed,
                Err(MemoryReviewerLedgerError::Refused(
                    MemoryReviewerLedgerRefusal::BindingMismatch
                ))
            ),
            "{resumed:?}"
        );
        assert_eq!(
            evaluate(&owner_record, &live(&kernel, &store)),
            Err(Closed::IdentityMismatch("kernel incarnation"))
        );
        let receipt = store
            .lookup_memory_reviewer_receipt(PROJECT, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.kernel_incarnation_id, kernel_id);
    }

    // Restored Kernel beside a replaced Memory Store: no receipt exists, so nothing is selected, and the gate closes on the store's incarnation; the Kernel's staged proposal is not served on anyone's say-so.
    let replaced_store = DataDir::new(root.path().join("replaced-store"));
    {
        let kernel = restored.open_kernel();
        let store = replaced_store.open_store().unwrap();
        assert_eq!(kernel_incarnation(&kernel), kernel_id);
        assert_ne!(store.memory_reviewer_store_incarnation().unwrap(), store_id);
        assert_eq!(
            read_selected_proposal(&kernel, &store, PROJECT, &identity, now + 4),
            Err(ReadRefusal::NotSelected)
        );
        assert_eq!(
            evaluate(&owner_record, &live(&kernel, &store)),
            Err(Closed::IdentityMismatch("memory store incarnation"))
        );
    }
}
