//! The both-store backup and restore rehearsal over real files: a published proposal survives restoring the Kernel through its own verified backup and the Memory Store from its closed file family, taken as one pair; every mismatched restore is refused rather than reconciled. A backup Memory Store beside a replaced Kernel refuses the receipt's binding, the read, and the activation gate; a replaced Memory Store beside the restored Kernel holds no receipt, so nothing reads and the gate closes on its incarnation. No path migrates, resets, or rebinds anything.

mod support;

use std::path::Path;
use std::sync::Arc;

use daemon::curator::activation::{
    Closed, IDENTITY_SCHEMA, LiveIdentity, RuntimeIdentityRecord, evaluate,
};
use daemon::curator::settlement::{ReadRefusal, read_selected_proposal};
use daemon::curator::steps::STEP_VERSION;
use daemon::curator::worker::job_binding;
use kernel::KernelStore;
use memory_store::MemoryStore;
use memory_store::curator_ledger::{CuratorLedgerError, CuratorLedgerRefusal};
use support::curator_publish::{
    PROJECT, activate_module_authority, begin_job, commit_memory_domain, kernel_incarnation,
    now_ms, publish,
};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn open_pair(kernel_dir: &Path, store_dir: &Path) -> (Arc<KernelStore>, Arc<MemoryStore>) {
    (
        Arc::new(KernelStore::open(kernel_dir).unwrap()),
        Arc::new(
            MemoryStore::open(&MemoryStore::test_descriptor(store_dir, "eidnara-restore")).unwrap(),
        ),
    )
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

/// The Memory Store's backup is its closed SQLite family copied file for file; it publishes no backup of its own.
fn copy_store(from: &Path, to: &Path) {
    private_dir(to);
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

/// The owner's record for one deployment pair.
fn record(kernel_incarnation: &str, memstore_incarnation: &str) -> RuntimeIdentityRecord {
    serde_json::from_value(serde_json::json!({
        "schema": IDENTITY_SCHEMA,
        "model": "claude-test",
        "prompt_template_version": LiveIdentity::prompt_template_version(),
        "step_schema_version": STEP_VERSION,
        "scanner_ruleset_version": LiveIdentity::scanner_ruleset_version(),
        "egress_policy_version": context_core::curator_policy_union::CURATOR_POLICY_UNION_VERSION,
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
        memstore_incarnation: store.curator_store_incarnation().unwrap(),
        provider: "api.anthropic.com/v1/messages@2023-06-01".into(),
        credentials: vec![("ANTHROPIC_API_KEY".into(), "fp-1".into())],
    }
}

#[test]
fn a_consistent_pair_restores_and_every_mismatched_restore_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let kernel_dir = root.path().join("kernel");
    let store_dir = root.path().join("store");
    let backup = root.path().join("backup");
    private_dir(&kernel_dir);
    private_dir(&store_dir);
    private_dir(&backup.join("kernel"));
    let now = now_ms();
    // The pair is backed up together, with no publication between the two captures: the Kernel through its verified backup, the Memory Store as its closed file family.
    let (identity, binding, kernel_id, store_id, kernel_backup) = {
        let (kernel, store) = open_pair(&kernel_dir, &store_dir);
        commit_memory_domain(&kernel);
        let generation = activate_module_authority(&store, &root.path().join("project"));
        let kernel_id = kernel_incarnation(&kernel);
        let begun = begin_job(&store, &kernel_id, generation, 1, now);
        publish(&kernel, &store, DIGEST, &kernel_id, &begun, now);
        let binding = job_binding(DIGEST, &begun.job);
        assert!(
            read_selected_proposal(
                &kernel,
                &store,
                PROJECT,
                &begun.job.causal_identity,
                &binding,
                now + 1
            )
            .is_ok()
        );
        let store_id = store.curator_store_incarnation().unwrap();
        let manifest = kernel
            .backup(kernel::BackupRequest {
                destination_directory: backup.join("kernel"),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
                capture_pin_expires_at: None,
            })
            .unwrap();
        (
            begun.job.causal_identity,
            binding,
            kernel_id,
            store_id,
            manifest.destination_path,
        )
    };
    copy_store(&store_dir, &backup.join("store"));
    let owner_record = record(&kernel_id, &store_id);

    // Both restored from the pair into fresh directories: the same incarnations, the receipt still selects the proposal, the review hold is live, and the owner's record still matches.
    let restored = root.path().join("restored");
    private_dir(&restored.join("kernel"));
    copy_store(&backup.join("store"), &restored.join("store"));
    {
        let (kernel, store) = open_pair(&restored.join("kernel"), &restored.join("store"));
        kernel.restore(&kernel_backup).unwrap();
        assert_eq!(kernel_incarnation(&kernel), kernel_id);
        assert_eq!(store.curator_store_incarnation().unwrap(), store_id);
        let read =
            read_selected_proposal(&kernel, &store, PROJECT, &identity, &binding, now + 2).unwrap();
        assert!(read.review_expires_at > now);
        assert!(evaluate(&owner_record, &live(&kernel, &store)).is_ok());
    }

    // Backup Memory Store beside a replaced Kernel: the receipt binds the old Kernel, so the read, a resumed receipt, and the gate all refuse; the row is exactly as backed up.
    let replaced_kernel = root.path().join("replaced-kernel");
    private_dir(&replaced_kernel);
    {
        let (kernel, store) = open_pair(&replaced_kernel, &restored.join("store"));
        assert_ne!(kernel_incarnation(&kernel), kernel_id);
        assert_eq!(
            read_selected_proposal(&kernel, &store, PROJECT, &identity, &binding, now + 3),
            Err(ReadRefusal::IncarnationMismatch)
        );
        let resumed = store.begin_curator_receipt(
            PROJECT,
            &identity,
            &kernel_incarnation(&kernel),
            "claim-x",
            now + 3,
        );
        assert!(
            matches!(
                resumed,
                Err(CuratorLedgerError::Refused(
                    CuratorLedgerRefusal::BindingMismatch
                ))
            ),
            "{resumed:?}"
        );
        assert_eq!(
            evaluate(&owner_record, &live(&kernel, &store)),
            Err(Closed::IdentityMismatch("kernel incarnation"))
        );
        let receipt = store
            .lookup_curator_receipt(PROJECT, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.kernel_incarnation_id, kernel_id);
    }

    // Restored Kernel beside a replaced Memory Store: no receipt exists, so nothing is selected, and the gate closes on the store's incarnation; the Kernel's staged proposal is not served on anyone's say-so.
    let replaced_store = root.path().join("replaced-store");
    private_dir(&replaced_store);
    {
        let (kernel, store) = open_pair(&restored.join("kernel"), &replaced_store);
        assert_eq!(kernel_incarnation(&kernel), kernel_id);
        assert_ne!(store.curator_store_incarnation().unwrap(), store_id);
        assert_eq!(
            read_selected_proposal(&kernel, &store, PROJECT, &identity, &binding, now + 4),
            Err(ReadRefusal::NotSelected)
        );
        assert_eq!(
            evaluate(&owner_record, &live(&kernel, &store)),
            Err(Closed::IdentityMismatch("memory store incarnation"))
        );
    }
}
