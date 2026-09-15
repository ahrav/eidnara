//! A lifecycle store with one admitted gate, plus builders for layers and compositions over a fixed eight-dimensional corpus, shared by the composition and reader tests.

use std::collections::BTreeSet;
use std::fs;
use std::num::NonZeroUsize;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::projection_gate::{identity, passing_evaluator};
use daemon::projection_gates::{Admission, EntryPoint, HookGate, ProjectionHook};
use daemon::vector_composition::{
    Composition, CompositionRefusal, CompositionSpec, SelectorState, Unavailable, compose, publish,
    recover, verify_composition,
};
use daemon::vector_generation::{ExpectedVectors, Staging, VerifiedVectors, build, stage, verify};
use host_runtime::generation::{GENERATIONS_DIR_NAME, GenerationStore, SourceSpec, StageMeta};
use host_runtime::lifecycle::LifecycleTransactionLock;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::Metric;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::scalar::ScalarRecipe;

pub const DIMENSION: u32 = 8;
pub const TOLERANCE: f64 = 1e-3;

pub fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    raw.iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

pub fn rows(seed: u8) -> Vec<ExportedRow> {
    let mut rows: Vec<(String, Vec<f32>)> = (0..4u8)
        .map(|i| {
            let mut raw = [0.05f32; 8];
            raw[usize::from(i)] = 0.9;
            raw[7] = f32::from(seed) / 100.0;
            (format!("{:02x}", i + seed).repeat(32), unit(raw))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter()
        .map(|(occurrence_id, vector)| ExportedRow {
            occurrence_id,
            vector,
        })
        .collect()
}

pub fn export(seed: u8, checkpoint: i64) -> LiveRows {
    LiveRows {
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: checkpoint - 1,
            checkpoint_commit_seq: checkpoint,
            hold_id: "hold-7".to_owned(),
        },
        rows: rows(seed),
        tombstones: Vec::new(),
    }
}

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub store: GenerationStore,
    /// The exclusive lifecycle transaction the fixture stages and publishes under; `release_transaction` gives it up so a reader's shared protection can be taken.
    pub tx: Option<LifecycleTransactionLock>,
    pub gate: HookGate,
    pub admission: Admission,
    pub identity: ProjectionIdentity,
    pub generation: VectorGeneration,
    pub protected: BTreeSet<String>,
    work_dirs: std::cell::Cell<usize>,
}

impl Fixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = GenerationStore::open(Some(root.path())).unwrap();
        let tx = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
        let identity = identity("test-incarnation", DIMENSION);
        let gate = HookGate::closed();
        gate.install(passing_evaluator(&identity, 0, &ProjectionHook::ALL));
        let admission = gate
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
            .unwrap();
        let generation = VectorGeneration {
            generation_id: "gen-vectors-1".to_owned(),
            embedding_model: identity.embedding_model.clone(),
            tokenizer_fingerprint: identity.tokenizer_fingerprint.clone(),
            vector_dimension: identity.vector_dimension,
            generation_epoch: identity.generation_epoch,
        };
        Self {
            root,
            store,
            tx: Some(tx),
            gate,
            admission,
            identity,
            generation,
            protected: BTreeSet::new(),
            work_dirs: std::cell::Cell::new(0),
        }
    }

    pub fn transaction(&self) -> &LifecycleTransactionLock {
        self.tx
            .as_ref()
            .expect("the fixture holds the lifecycle transaction")
    }

    pub fn release_transaction(&mut self) {
        self.tx = None;
    }

    pub fn reacquire_transaction(&mut self) {
        self.tx =
            Some(LifecycleTransactionLock::acquire_exclusive(Some(self.root.path())).unwrap());
    }

    pub fn expected(&self) -> ExpectedVectors<'_> {
        ExpectedVectors {
            generation: &self.generation,
            kernel_incarnation_id: &self.identity.kernel_incarnation_id,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            recipe: ScalarRecipe::SymmetricInt8V1,
            checkpoint: None,
        }
    }

    pub fn work_dir(&self) -> PathBuf {
        let index = self.work_dirs.get();
        self.work_dirs.set(index + 1);
        let dir = self.root.path().join(format!("work-{index}"));
        fs::create_dir(&dir).unwrap();
        dir
    }

    pub fn staging(&self) -> Staging<'_> {
        Staging {
            store: &self.store,
            transaction: self.transaction(),
            gate: &self.gate,
            admission: &self.admission,
            identity: &self.identity,
            protected: &self.protected,
        }
    }

    /// Builds, stages, and verifies one layer.
    pub fn layer(&self, seed: u8, checkpoint: i64) -> VerifiedVectors {
        self.layer_masking(seed, checkpoint, &[])
    }

    pub fn layer_masking(
        &self,
        seed: u8,
        checkpoint: i64,
        tombstones: &[String],
    ) -> VerifiedVectors {
        let mut export = export(seed, checkpoint);
        export.tombstones = tombstones.to_vec();
        self.layer_from(&export)
    }

    /// Builds, stages, and verifies one layer from an explicit export.
    pub fn layer_from(&self, export: &LiveRows) -> VerifiedVectors {
        let built = build(&self.expected(), export, &self.work_dir()).unwrap();
        let digest = stage(&built, &self.staging()).unwrap();
        verify(&self.store, &digest, &self.expected()).unwrap()
    }

    pub fn compose(
        &self,
        sequence: u64,
        base: &VerifiedVectors,
        deltas: &[VerifiedVectors],
    ) -> Result<Composition, CompositionRefusal> {
        let expected = self.expected();
        compose(&CompositionSpec {
            expected: &expected,
            sequence,
            base,
            deltas,
            max_deltas: NonZeroUsize::new(4).unwrap(),
        })
    }

    pub fn publish(&self, composition: &Composition) -> Result<String, CompositionRefusal> {
        publish(composition, &self.staging(), &self.work_dir(), &mut |_| {
            Ok(())
        })
        .map_err(|failure| failure.refusal)
    }

    pub fn verify_composition(&self, digest: &str) -> Result<Vec<String>, CompositionRefusal> {
        verify_composition(
            &self.store,
            digest,
            &self.expected(),
            NonZeroUsize::new(4).unwrap(),
        )
        .map(|verified| verified.composition.members())
    }

    pub fn lifecycle_dir(&self) -> PathBuf {
        self.root.path().join("eidnara").join("lifecycle")
    }

    pub fn generation_dir(&self, digest: &str) -> PathBuf {
        self.lifecycle_dir().join(GENERATIONS_DIR_NAME).join(digest)
    }

    pub fn generations(&self) -> BTreeSet<String> {
        fs::read_dir(self.lifecycle_dir().join(GENERATIONS_DIR_NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    pub fn selector_bytes(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.lifecycle_dir().join(name)).ok()
    }

    pub fn write_selector(&self, name: &str, bytes: &[u8]) {
        let path = self.lifecycle_dir().join(name);
        let _ = fs::remove_file(&path);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    pub fn corrupt(&self, digest: &str, file: &str) {
        let path = self.generation_dir(digest).join(file);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&path, bytes).unwrap();
    }

    pub fn stage_search_seed(&self) -> String {
        let dir = self.work_dir();
        let path = dir.join("search.sqlite");
        fs::write(&path, b"not really a database").unwrap();
        let meta = StageMeta {
            target: "search-projection-seed".to_owned(),
            release_contract_sha256: "a".repeat(64),
            inputs_lock_sha256: "b".repeat(64),
            source_payload_manifest_sha256: "c".repeat(64),
        };
        self.store
            .stage(
                &[SourceSpec {
                    rel_path: "search.sqlite".to_owned(),
                    source: path,
                    executable: false,
                    expected_size: None,
                    expected_sha256: None,
                }],
                &meta,
                &BTreeSet::new(),
            )
            .unwrap()
    }

    pub fn recover(&self) -> Result<(String, SelectorState), Unavailable> {
        recover(
            &self.store,
            self.transaction(),
            &self.expected(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(8).unwrap(),
        )
        .map(|recovered| (recovered.composition.digest, recovered.selector))
    }
}
