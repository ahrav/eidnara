mod support;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use daemon::projection_gates::{Admission, EntryPoint, HookGate, ProjectionHook};
use daemon::vector_generation::{
    BuiltVectors, CODES_FILE, ExpectedVectors, FileFault, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE,
    SIDECAR_FILE, Staging, TOMBSTONES_FILE, VECTOR_TARGET, VectorRefusal, VectorSidecar, build,
    stage, verify,
};
use host_runtime::generation::{
    CurrentProfile, GENERATIONS_DIR_NAME, GenerationError, GenerationManifest, GenerationStore,
    SourceSpec, StageMeta,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::Metric;
use retrieval::dense::export::{ExportedRow, LiveRows};
use retrieval::dense::scalar::{ScalarRecipe, Scales, encode, encode_codes};
use sha2::{Digest, Sha256};
use support::projection_gate::{identity, passing_evaluator};

const DIMENSION: u32 = 8;
const TOLERANCE: f64 = 1e-3;
const KERNEL: &str = "test-incarnation";

fn unit(raw: [f32; 8]) -> Vec<f32> {
    let norm = raw
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    raw.iter()
        .map(|value| (f64::from(*value) / norm) as f32)
        .collect()
}

fn export() -> LiveRows {
    let mut rows = vec![
        (
            "0a".repeat(32),
            unit([0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1]),
        ),
        (
            "1b".repeat(32),
            unit([0.7, 0.3, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ),
        (
            "2c".repeat(32),
            unit([-0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0]),
        ),
        (
            "3d".repeat(32),
            unit([0.1, 0.0, 0.0, 0.0, 0.0, 0.9, 0.0, 0.0]),
        ),
    ];
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    LiveRows {
        generation: generation(&identity(KERNEL, DIMENSION)),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 3,
            checkpoint_commit_seq: 9,
            hold_id: "hold-7".to_owned(),
        },
        rows: rows
            .into_iter()
            .map(|(occurrence_id, vector)| ExportedRow {
                occurrence_id,
                vector,
            })
            .collect(),
        tombstones: Vec::new(),
    }
}

fn generation(identity: &ProjectionIdentity) -> VectorGeneration {
    VectorGeneration {
        generation_id: "gen-vectors-1".to_owned(),
        embedding_model: identity.embedding_model.clone(),
        tokenizer_fingerprint: identity.tokenizer_fingerprint.clone(),
        vector_dimension: identity.vector_dimension,
        generation_epoch: identity.generation_epoch,
    }
}

struct Fixture {
    root: tempfile::TempDir,
    store: GenerationStore,
    tx: LifecycleTransactionLock,
    gate: HookGate,
    admission: Admission,
    identity: ProjectionIdentity,
    generation: VectorGeneration,
    protected: BTreeSet<String>,
    work_dirs: std::cell::Cell<usize>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = GenerationStore::open(Some(root.path())).unwrap();
        let tx = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
        let identity = identity(KERNEL, DIMENSION);
        let gate = HookGate::closed();
        gate.install(passing_evaluator(&identity, 0, &ProjectionHook::ALL));
        let admission = gate
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
            .unwrap();
        let generation = generation(&identity);
        Self {
            root,
            store,
            tx,
            gate,
            admission,
            identity,
            generation,
            protected: BTreeSet::new(),
            work_dirs: std::cell::Cell::new(0),
        }
    }

    fn expected(&self) -> ExpectedVectors<'_> {
        ExpectedVectors {
            generation: &self.generation,
            kernel_incarnation_id: &self.identity.kernel_incarnation_id,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            recipe: ScalarRecipe::SymmetricInt8V1,
            checkpoint: None,
        }
    }

    fn work_dir(&self) -> PathBuf {
        let index = self.work_dirs.get();
        self.work_dirs.set(index + 1);
        let dir = self.root.path().join(format!("work-{index}"));
        fs::create_dir(&dir).unwrap();
        dir
    }

    fn build(&self) -> BuiltVectors {
        build(&self.expected(), &export(), &self.work_dir()).unwrap()
    }

    fn stage(&self, built: &BuiltVectors) -> Result<String, VectorRefusal> {
        self.stage_with(built, &self.admission)
    }

    fn stage_with(
        &self,
        built: &BuiltVectors,
        admission: &Admission,
    ) -> Result<String, VectorRefusal> {
        stage(
            built,
            &Staging {
                store: &self.store,
                transaction: &self.tx,
                gate: &self.gate,
                admission,
                identity: &self.identity,
                protected: &self.protected,
            },
        )
    }

    fn verify(&self, digest: &str) -> Result<VectorSidecar, VectorRefusal> {
        verify(&self.store, digest, &self.expected(), u64::MAX).map(|verified| verified.sidecar)
    }

    fn lifecycle_dir(&self) -> PathBuf {
        self.root.path().join("eidnara").join("lifecycle")
    }

    fn generation_dir(&self, digest: &str) -> PathBuf {
        self.lifecycle_dir().join(GENERATIONS_DIR_NAME).join(digest)
    }

    fn generations(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.lifecycle_dir().join(GENERATIONS_DIR_NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn select_vector(&self, digest: &str) -> Result<(), GenerationError> {
        self.store.select_vector(digest, &self.tx, &mut |_| Ok(()))
    }

    fn stage_search_seed(&self) -> String {
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

    /// Runs `tamper` on a copy of the staged generation's files, restages the result under whatever digest its manifest now hashes to, and returns that digest; the original generation is untouched.
    fn restaged<F: FnOnce(&Path)>(&self, digest: &str, tamper: F) -> String {
        let source = self.generation_dir(digest);
        let scratch = self.work_dir();
        for entry in fs::read_dir(&source).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), scratch.join(entry.file_name())).unwrap();
        }
        tamper(&scratch);
        let manifest: GenerationManifest =
            serde_json::from_slice(&fs::read(scratch.join("manifest.json")).unwrap()).unwrap();
        let new_digest = manifest.digest();
        let target = self.generation_dir(&new_digest);
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        for entry in fs::read_dir(&scratch).unwrap() {
            let entry = entry.unwrap();
            let dest = target.join(entry.file_name());
            fs::copy(entry.path(), &dest).unwrap();
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).unwrap();
        }
        new_digest
    }
}

fn file_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_sidecar(dir: &Path) -> VectorSidecar {
    serde_json::from_slice(&fs::read(dir.join(SIDECAR_FILE)).unwrap()).unwrap()
}

/// Writes the sidecar and a manifest that hashes it, so every hash agrees with every byte.
fn write_bound(dir: &Path, sidecar: &VectorSidecar) {
    fs::write(dir.join(SIDECAR_FILE), sidecar.canonical_bytes()).unwrap();
    fs::write(
        dir.join("manifest.json"),
        sidecar.stage_manifest().canonical_bytes(),
    )
    .unwrap();
}

/// Replaces one payload file and updates its sidecar inventory entry, leaving every other field alone.
fn rehash_file(dir: &Path, path: &str, bytes: &[u8]) {
    fs::write(dir.join(path), bytes).unwrap();
    let mut sidecar = read_sidecar(dir);
    for file in &mut sidecar.files {
        if file.path == path {
            file.size = bytes.len() as u64;
            file.sha256 = sha(bytes);
        }
    }
    write_bound(dir, &sidecar);
}

#[test]
fn paired_fresh_builds_produce_identical_names_bytes_sidecar_manifest_and_digest() {
    let fixture = Fixture::new();
    let first = fixture.build();
    let second = fixture.build();
    assert_eq!(first.sidecar, second.sidecar);
    assert_eq!(first.digest(), second.digest());
    let files = file_bytes(&first.dir);
    assert_eq!(files, file_bytes(&second.dir));
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            CODES_FILE,
            ROW_IDS_FILE,
            ROWS_FILE,
            SCALES_FILE,
            TOMBSTONES_FILE,
            SIDECAR_FILE
        ]
    );
    assert_eq!(first.sidecar.rows, 4);
    assert_eq!(first.sidecar.tombstones, 0);
    assert_eq!(first.sidecar.calibrated_rows, 4);
    assert_eq!(first.sidecar.metric, "inner_product");
    assert_eq!(
        first.sidecar.quantizer_recipe,
        ScalarRecipe::SymmetricInt8V1.id()
    );
    assert_eq!(first.sidecar.checkpoint(), export().checkpoint);
    assert_eq!(first.sidecar.stage_meta().target, VECTOR_TARGET);
    assert_eq!(
        first.sidecar.stage_meta().inputs_lock_sha256,
        first.sidecar.sha256()
    );

    // A changed input changes the payload hash, the sidecar, and the directory name.
    let mut perturbed = export();
    perturbed.rows[2].vector[0] = perturbed.rows[2].vector[0].next_up();
    let other = build(&fixture.expected(), &perturbed, &fixture.work_dir()).unwrap();
    assert_ne!(other.digest(), first.digest());
    assert_ne!(other.sidecar.files[0].sha256, first.sidecar.files[0].sha256);
    let mut other_hold = export();
    other_hold.checkpoint.hold_id = "hold-8".to_owned();
    let other = build(&fixture.expected(), &other_hold, &fixture.work_dir()).unwrap();
    assert_ne!(other.digest(), first.digest());
    assert_eq!(
        other.sidecar.files, first.sidecar.files,
        "the payload is the same; only the provenance moved"
    );

    let digest = fixture.stage(&first).unwrap();
    assert_eq!(digest, first.digest());
    let published = file_bytes(&fixture.generation_dir(&digest));
    let again = fixture.stage(&second).unwrap();
    assert_eq!(
        again, digest,
        "an identical retry names the same generation"
    );
    assert_eq!(
        file_bytes(&fixture.generation_dir(&digest)),
        published,
        "published bytes are immutable on identical retries"
    );
    assert_eq!(fixture.generations(), vec![digest.clone()]);
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent,
        "staging selects nothing"
    );

    let verified = verify(&fixture.store, &digest, &fixture.expected(), u64::MAX).unwrap();
    assert_eq!(verified.sidecar, first.sidecar);
    verified.generation.pin().unwrap();
    let with_checkpoint = ExpectedVectors {
        checkpoint: Some(&export().checkpoint),
        ..fixture.expected()
    };
    assert!(verify(&fixture.store, &digest, &with_checkpoint, u64::MAX).is_ok());

    // The bound is the manifest's payload total, including the sidecar; one byte under it refuses before any payload is read.
    let total: u64 = first
        .sidecar
        .stage_manifest()
        .files
        .iter()
        .map(|file| file.size)
        .sum();
    assert!(verify(&fixture.store, &digest, &fixture.expected(), total).is_ok());
    assert_eq!(
        verify(&fixture.store, &digest, &fixture.expected(), total - 1).map(|v| v.digest),
        Err(VectorRefusal::OverBound {
            bytes: total,
            max: total - 1
        })
    );
}

#[test]
fn build_refuses_no_rows_disordered_rows_or_tombstones_and_rows_outside_the_layout() {
    let fixture = Fixture::new();
    let dir = fixture.work_dir();
    let mut empty = export();
    empty.rows.clear();
    assert_eq!(
        build(&fixture.expected(), &empty, &dir),
        Err(VectorRefusal::NoRows)
    );
    let mut reversed = export();
    reversed.rows.reverse();
    assert_eq!(
        build(&fixture.expected(), &reversed, &dir),
        Err(VectorRefusal::RowOrder { index: 1 })
    );
    let mut duplicated = export();
    duplicated.rows[1].occurrence_id = duplicated.rows[0].occurrence_id.clone();
    assert_eq!(
        build(&fixture.expected(), &duplicated, &dir),
        Err(VectorRefusal::RowOrder { index: 1 })
    );
    let mut unnormalized = export();
    unnormalized.rows[2].vector = vec![1.0; 8];
    assert!(matches!(
        build(&fixture.expected(), &unnormalized, &dir),
        Err(VectorRefusal::Rows(_))
    ));
    let mut unordered_tombstones = export();
    unordered_tombstones.tombstones = vec!["ff".repeat(32), "ee".repeat(32)];
    assert_eq!(
        build(&fixture.expected(), &unordered_tombstones, &dir),
        Err(VectorRefusal::TombstoneOrder { index: 1 })
    );
    let mut both = export();
    both.tombstones = vec![both.rows[2].occurrence_id.clone()];
    assert_eq!(
        build(&fixture.expected(), &both, &dir),
        Err(VectorRefusal::ListedAndTombstoned { index: 2 })
    );
    assert!(
        fs::read_dir(&dir).unwrap().next().is_none(),
        "a refused build writes nothing"
    );
}

#[test]
fn build_refuses_an_export_whose_generation_or_kernel_is_not_the_expected_one() {
    let fixture = Fixture::new();
    let dir = fixture.work_dir();
    for (field, mutate) in [
        (
            "generation_id",
            Box::new(|e: &mut LiveRows| e.generation.generation_id = "gen-vectors-2".to_owned())
                as Box<dyn Fn(&mut LiveRows)>,
        ),
        (
            "embedding_model",
            Box::new(|e: &mut LiveRows| e.generation.embedding_model = "another-model".to_owned()),
        ),
        (
            "tokenizer_fingerprint",
            Box::new(|e: &mut LiveRows| e.generation.tokenizer_fingerprint = "f".repeat(64)),
        ),
        (
            "vector_dimension",
            Box::new(|e: &mut LiveRows| e.generation.vector_dimension = 4),
        ),
        (
            "generation_epoch",
            Box::new(|e: &mut LiveRows| e.generation.generation_epoch = 2),
        ),
        (
            "kernel_incarnation_id",
            Box::new(|e: &mut LiveRows| e.kernel_incarnation_id = "other-kernel".to_owned()),
        ),
    ] {
        let mut foreign = export();
        mutate(&mut foreign);
        assert_eq!(
            build(&fixture.expected(), &foreign, &dir),
            Err(VectorRefusal::Export { field }),
            "rows read under another {field} do not become this generation"
        );
    }
    assert!(
        fs::read_dir(&dir).unwrap().next().is_none(),
        "a refused build writes nothing"
    );
    assert!(build(&fixture.expected(), &export(), &dir).is_ok());
}

#[test]
fn build_refuses_an_export_whose_checkpoint_is_not_the_one_the_caller_named() {
    let fixture = Fixture::new();
    let dir = fixture.work_dir();
    let mut pinned = export().checkpoint;
    pinned.hold_id = "hold-8".to_owned();
    let expected = ExpectedVectors {
        checkpoint: Some(&pinned),
        ..fixture.expected()
    };
    assert_eq!(
        build(&expected, &export(), &dir),
        Err(VectorRefusal::Export {
            field: "checkpoint"
        }),
        "rows read at another checkpoint do not become the pinned generation"
    );
    assert!(
        fs::read_dir(&dir).unwrap().next().is_none(),
        "a refused build writes nothing"
    );
    let expected = ExpectedVectors {
        checkpoint: Some(&export().checkpoint),
        ..fixture.expected()
    };
    assert!(build(&expected, &export(), &dir).is_ok());
}

#[test]
fn staging_refuses_a_build_whose_provenance_is_not_the_admitted_identity() {
    let fixture = Fixture::new();
    let built = fixture.build();
    for (field, mutate) in [
        (
            "embedding_model",
            Box::new(|i: &mut ProjectionIdentity| i.embedding_model = "another-model".to_owned())
                as Box<dyn Fn(&mut ProjectionIdentity)>,
        ),
        (
            "tokenizer_fingerprint",
            Box::new(|i: &mut ProjectionIdentity| i.tokenizer_fingerprint = "f".repeat(64)),
        ),
        (
            "vector_dimension",
            Box::new(|i: &mut ProjectionIdentity| i.vector_dimension = 4),
        ),
        (
            "generation_epoch",
            Box::new(|i: &mut ProjectionIdentity| i.generation_epoch = 2),
        ),
        (
            "kernel_incarnation_id",
            Box::new(|i: &mut ProjectionIdentity| {
                i.kernel_incarnation_id = "other-kernel".to_owned()
            }),
        ),
    ] {
        // The gate holds evidence for identity B and admits under it; the build carries identity A.
        let mut other = fixture.identity.clone();
        mutate(&mut other);
        let gate = HookGate::closed();
        gate.install(passing_evaluator(&other, 0, &ProjectionHook::ALL));
        let admission = gate
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
            .unwrap();
        assert_eq!(
            stage(
                &built,
                &Staging {
                    store: &fixture.store,
                    transaction: &fixture.tx,
                    gate: &gate,
                    admission: &admission,
                    identity: &other,
                    protected: &BTreeSet::new(),
                },
            ),
            Err(VectorRefusal::Identity { field }),
            "a build for one {field} is not staged under another's evidence"
        );
        assert!(
            fixture.generations().is_empty(),
            "{field}: a refused staging creates no generation"
        );
    }
    assert!(fixture.stage(&built).is_ok());
}

#[test]
fn verification_refuses_missing_extra_truncated_and_corrupt_files() {
    let fixture = Fixture::new();
    let digest = fixture.stage(&fixture.build()).unwrap();
    let dir = fixture.generation_dir(&digest);
    let pristine = file_bytes(&dir);
    let restore = || {
        for entry in fs::read_dir(&dir).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        for (name, bytes) in &pristine {
            fs::write(dir.join(name), bytes).unwrap();
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
    };
    type Tamper = Box<dyn Fn(&Path)>;
    let tampers: [(&str, Tamper); 4] = [
        (
            "missing",
            Box::new(|dir: &Path| fs::remove_file(dir.join(CODES_FILE)).unwrap()),
        ),
        (
            "extra",
            Box::new(|dir: &Path| fs::write(dir.join("extra.bin"), b"x").unwrap()),
        ),
        (
            "truncated",
            Box::new(|dir: &Path| {
                let scales = fs::read(dir.join(SCALES_FILE)).unwrap();
                fs::write(dir.join(SCALES_FILE), &scales[..scales.len() - 4]).unwrap();
            }),
        ),
        (
            "corrupt",
            Box::new(|dir: &Path| {
                let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
                codes[3] ^= 0x01;
                fs::write(dir.join(CODES_FILE), &codes).unwrap();
            }),
        ),
    ];
    for (name, tamper) in tampers {
        tamper(&dir);
        assert!(
            matches!(fixture.verify(&digest), Err(VectorRefusal::Store(_))),
            "{name}: the store refuses before any meaning is read"
        );
        assert!(
            fixture.select_vector(&digest).is_err(),
            "{name}: an unvalidatable generation is never selected"
        );
        assert_eq!(
            fixture.store.read_vector_current().unwrap(),
            CurrentProfile::Absent
        );
        restore();
    }
    assert!(fixture.verify(&digest).is_ok(), "the control verifies");
}

#[test]
fn verification_refuses_rehashed_state_whose_meaning_changed() {
    let fixture = Fixture::new();
    let built = fixture.build();
    let digest = fixture.stage(&built).unwrap();
    let layout = built.sidecar.layout().unwrap();

    // Codes edited, every hash rewritten to match.
    let codes = fixture.restaged(&digest, |dir| {
        let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
        codes[3] = codes[3].wrapping_add(1);
        rehash_file(dir, CODES_FILE, &codes);
    });
    assert!(
        fixture.store.validate(&codes).is_ok(),
        "the store sees a consistent generation"
    );
    assert_eq!(
        fixture.verify(&codes).unwrap_err(),
        VectorRefusal::File {
            path: CODES_FILE,
            fault: FileFault::Codes
        }
    );

    // Arbitrary scales, codes re-encoded under them, provenance and every hash rewritten to match:
    // only recalibrating the rows can tell these scales are not the recipe's.
    let scales = fixture.restaged(&digest, |dir| {
        let arbitrary = Scales::from_values(vec![0.5; 8], DIMENSION).unwrap();
        let mut codes = Vec::new();
        for row in &export().rows {
            codes.extend(encode_codes(
                &encode(&layout, &arbitrary, &row.vector).unwrap().codes,
            ));
        }
        rehash_file(dir, CODES_FILE, &codes);
        rehash_file(dir, SCALES_FILE, &arbitrary.encode());
        let mut sidecar = read_sidecar(dir);
        sidecar.scales_sha256 = sha(&arbitrary.encode());
        write_bound(dir, &sidecar);
    });
    assert!(fixture.store.validate(&scales).is_ok());
    assert_eq!(
        fixture.verify(&scales).unwrap_err(),
        VectorRefusal::File {
            path: SCALES_FILE,
            fault: FileFault::Calibration
        }
    );

    let provenance = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.calibrated_rows = 3;
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&provenance).unwrap_err(),
        VectorRefusal::File {
            path: SCALES_FILE,
            fault: FileFault::Calibration
        }
    );

    let ids = fixture.restaged(&digest, |dir| {
        let mut ids: Vec<String> =
            serde_json::from_slice(&fs::read(dir.join(ROW_IDS_FILE)).unwrap()).unwrap();
        ids.swap(0, 1);
        rehash_file(dir, ROW_IDS_FILE, &serde_json::to_vec(&ids).unwrap());
    });
    assert_eq!(
        fixture.verify(&ids).unwrap_err(),
        VectorRefusal::File {
            path: ROW_IDS_FILE,
            fault: FileFault::Identifiers
        }
    );
    let fewer_ids = fixture.restaged(&digest, |dir| {
        let mut ids: Vec<String> =
            serde_json::from_slice(&fs::read(dir.join(ROW_IDS_FILE)).unwrap()).unwrap();
        ids.pop();
        rehash_file(dir, ROW_IDS_FILE, &serde_json::to_vec(&ids).unwrap());
    });
    assert_eq!(
        fixture.verify(&fewer_ids).unwrap_err(),
        VectorRefusal::File {
            path: ROW_IDS_FILE,
            fault: FileFault::Identifiers
        }
    );

    // Tombstones edited, every hash rewritten to match: a listed row cannot also be masked, and the count must be the sidecar's.
    let listed_tombstone = fixture.restaged(&digest, |dir| {
        let ids: Vec<String> =
            serde_json::from_slice(&fs::read(dir.join(ROW_IDS_FILE)).unwrap()).unwrap();
        rehash_file(
            dir,
            TOMBSTONES_FILE,
            &serde_json::to_vec(&[&ids[0]]).unwrap(),
        );
        let mut sidecar = read_sidecar(dir);
        sidecar.tombstones = 1;
        write_bound(dir, &sidecar);
    });
    assert!(fixture.store.validate(&listed_tombstone).is_ok());
    assert_eq!(
        fixture.verify(&listed_tombstone).unwrap_err(),
        VectorRefusal::File {
            path: TOMBSTONES_FILE,
            fault: FileFault::Identifiers
        }
    );
    let tombstone_count = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.tombstones = 1;
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&tombstone_count).unwrap_err(),
        VectorRefusal::File {
            path: TOMBSTONES_FILE,
            fault: FileFault::Identifiers
        }
    );
    let disordered_tombstones = fixture.restaged(&digest, |dir| {
        rehash_file(
            dir,
            TOMBSTONES_FILE,
            &serde_json::to_vec(&["ff".repeat(32), "ee".repeat(32)]).unwrap(),
        );
        let mut sidecar = read_sidecar(dir);
        sidecar.tombstones = 2;
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&disordered_tombstones).unwrap_err(),
        VectorRefusal::File {
            path: TOMBSTONES_FILE,
            fault: FileFault::Identifiers
        }
    );

    let row_count = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.rows = 5;
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&row_count).unwrap_err(),
        VectorRefusal::File {
            path: ROWS_FILE,
            fault: FileFault::RowCount
        }
    );

    // A checkpoint the projection schema could not hold is refused even when no checkpoint is expected.
    for (name, tamper) in [
        (
            "negative snapshot",
            Box::new(|s: &mut VectorSidecar| s.snapshot_commit_seq = -1)
                as Box<dyn Fn(&mut VectorSidecar)>,
        ),
        (
            "checkpoint before snapshot",
            Box::new(|s: &mut VectorSidecar| s.checkpoint_commit_seq = s.snapshot_commit_seq - 1),
        ),
        (
            "empty hold",
            Box::new(|s: &mut VectorSidecar| s.hold_id.clear()),
        ),
    ] {
        let impossible = fixture.restaged(&digest, |dir| {
            let mut sidecar = read_sidecar(dir);
            tamper(&mut sidecar);
            write_bound(dir, &sidecar);
        });
        assert!(fixture.store.validate(&impossible).is_ok(), "{name}");
        assert_eq!(
            fixture.verify(&impossible).unwrap_err(),
            VectorRefusal::NotVectors("checkpoint"),
            "{name}"
        );
    }

    // An extra file inventoried by the sidecar and hashed by the manifest is still not a vector generation.
    let extra = fixture.restaged(&digest, |dir| {
        fs::write(dir.join("extra.bin"), b"x").unwrap();
        let mut sidecar = read_sidecar(dir);
        sidecar.files.push(daemon::vector_generation::SidecarFile {
            path: "extra.bin".to_owned(),
            size: 1,
            sha256: sha(b"x"),
        });
        write_bound(dir, &sidecar);
    });
    assert!(fixture.store.validate(&extra).is_ok());
    assert_eq!(
        fixture.verify(&extra).unwrap_err(),
        VectorRefusal::NotVectors("inventory")
    );

    // A manifest that hashes the same sidecar but carries another mode is not bound to it.
    let binding = fixture.restaged(&digest, |dir| {
        let sidecar = read_sidecar(dir);
        let mut manifest = sidecar.stage_manifest();
        manifest.release_contract_sha256 = "f".repeat(64);
        fs::write(dir.join("manifest.json"), manifest.canonical_bytes()).unwrap();
    });
    assert_eq!(
        fixture.verify(&binding).unwrap_err(),
        VectorRefusal::NotVectors("manifest binding")
    );

    // Pretty-printed sidecar bytes hash differently from the canonical form the manifest names.
    let noncanonical = fixture.restaged(&digest, |dir| {
        let sidecar = read_sidecar(dir);
        let pretty = serde_json::to_vec_pretty(&sidecar).unwrap();
        let mut manifest = sidecar.stage_manifest();
        for file in &mut manifest.files {
            if file.path == SIDECAR_FILE {
                file.size = pretty.len() as u64;
                file.sha256 = sha(&pretty);
            }
        }
        fs::write(dir.join(SIDECAR_FILE), &pretty).unwrap();
        fs::write(dir.join("manifest.json"), manifest.canonical_bytes()).unwrap();
    });
    assert_eq!(
        fixture.verify(&noncanonical).unwrap_err(),
        VectorRefusal::NotVectors("sidecar not canonical")
    );

    assert!(fixture.verify(&digest).is_ok(), "the control verifies");
}

#[test]
fn verification_refuses_another_owner_unknown_formats_and_a_foreign_model_space() {
    let fixture = Fixture::new();
    let seed = fixture.stage_search_seed();
    assert_eq!(
        fixture.verify(&seed).unwrap_err(),
        VectorRefusal::NotVectors("manifest target")
    );

    let digest = fixture.stage(&fixture.build()).unwrap();
    let schema = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.schema = 2;
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&schema).unwrap_err(),
        VectorRefusal::NotVectors("sidecar schema")
    );
    let metric = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.metric = "cosine".to_owned();
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&metric).unwrap_err(),
        VectorRefusal::Identity { field: "metric" }
    );
    let recipe = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.quantizer_recipe = "scalar-int8-symmetric.v2".to_owned();
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        fixture.verify(&recipe).unwrap_err(),
        VectorRefusal::Identity {
            field: "quantizer_recipe"
        }
    );
    let unknown_field = fixture.restaged(&digest, |dir| {
        let sidecar = read_sidecar(dir);
        let mut value: serde_json::Value = serde_json::to_value(&sidecar).unwrap();
        value["extra"] = serde_json::Value::from(1);
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut manifest = sidecar.stage_manifest();
        for file in &mut manifest.files {
            if file.path == SIDECAR_FILE {
                file.size = bytes.len() as u64;
                file.sha256 = sha(&bytes);
            }
        }
        fs::write(dir.join(SIDECAR_FILE), &bytes).unwrap();
        fs::write(dir.join("manifest.json"), manifest.canonical_bytes()).unwrap();
    });
    assert_eq!(
        fixture.verify(&unknown_field).unwrap_err(),
        VectorRefusal::NotVectors("sidecar")
    );

    for (field, mutate) in [
        (
            "embedding_model",
            Box::new(|g: &mut VectorGeneration| g.embedding_model = "another-model".to_owned())
                as Box<dyn Fn(&mut VectorGeneration)>,
        ),
        (
            "tokenizer_fingerprint",
            Box::new(|g: &mut VectorGeneration| g.tokenizer_fingerprint = "f".repeat(64)),
        ),
        (
            "vector_dimension",
            Box::new(|g: &mut VectorGeneration| g.vector_dimension += 1),
        ),
        (
            "generation_id",
            Box::new(|g: &mut VectorGeneration| g.generation_id = "gen-vectors-2".to_owned()),
        ),
        (
            "generation_epoch",
            Box::new(|g: &mut VectorGeneration| g.generation_epoch = 2),
        ),
    ] {
        let mut other = fixture.generation.clone();
        mutate(&mut other);
        let expected = ExpectedVectors {
            generation: &other,
            ..fixture.expected()
        };
        assert_eq!(
            verify(&fixture.store, &digest, &expected, u64::MAX).unwrap_err(),
            VectorRefusal::Identity { field },
            "a different {field} is refused"
        );
    }
    assert_eq!(
        verify(
            &fixture.store,
            &digest,
            &ExpectedVectors {
                unit_norm_tolerance: 2e-3,
                ..fixture.expected()
            },
            u64::MAX,
        )
        .unwrap_err(),
        VectorRefusal::Identity {
            field: "unit_norm_tolerance"
        }
    );
    assert_eq!(
        verify(
            &fixture.store,
            &digest,
            &ExpectedVectors {
                kernel_incarnation_id: "other",
                ..fixture.expected()
            },
            u64::MAX,
        )
        .unwrap_err(),
        VectorRefusal::Identity {
            field: "kernel_incarnation_id"
        }
    );
    for other_checkpoint in [
        ProjectionCheckpoint {
            snapshot_commit_seq: 0,
            ..export().checkpoint
        },
        ProjectionCheckpoint {
            checkpoint_commit_seq: 10,
            ..export().checkpoint
        },
        ProjectionCheckpoint {
            hold_id: "another-hold".to_owned(),
            ..export().checkpoint
        },
    ] {
        assert_eq!(
            verify(
                &fixture.store,
                &digest,
                &ExpectedVectors {
                    checkpoint: Some(&other_checkpoint),
                    ..fixture.expected()
                },
                u64::MAX,
            )
            .unwrap_err(),
            VectorRefusal::Identity {
                field: "checkpoint"
            }
        );
    }
    let mut other_generation = fixture.generation.clone();
    other_generation.embedding_model = "another-model".to_owned();
    other_generation.generation_id = "another-generation".to_owned();
    assert_eq!(
        verify(
            &fixture.store,
            &digest,
            &ExpectedVectors {
                generation: &other_generation,
                kernel_incarnation_id: "another-kernel",
                ..fixture.expected()
            },
            u64::MAX,
        )
        .unwrap_err(),
        VectorRefusal::Identity {
            field: "embedding_model"
        }
    );
    let zero_tolerance = fixture.restaged(&digest, |dir| {
        let mut sidecar = read_sidecar(dir);
        sidecar.unit_norm_tolerance = 0.0;
        write_bound(dir, &sidecar);
    });
    assert_eq!(
        verify(
            &fixture.store,
            &zero_tolerance,
            &ExpectedVectors {
                unit_norm_tolerance: -0.0,
                ..fixture.expected()
            },
            u64::MAX,
        )
        .unwrap_err(),
        VectorRefusal::Identity {
            field: "unit_norm_tolerance"
        }
    );
}

#[test]
fn a_denied_admission_stages_nothing() {
    let fixture = Fixture::new();
    let built = fixture.build();
    let total: u64 = built
        .sidecar
        .stage_manifest()
        .files
        .iter()
        .map(|f| f.size)
        .sum();
    let admit_with_limit = |limit: u64| {
        let mut evaluator = passing_evaluator(&fixture.identity, 0, &ProjectionHook::ALL);
        evaluator
            .manifest
            .limits
            .insert("capture_disk_bytes".to_owned(), limit);
        fixture.gate.install(evaluator);
        fixture
            .gate
            .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
            .unwrap()
    };
    let refused = fixture.stage_with(&built, &admit_with_limit(total - 1));
    assert!(
        matches!(refused, Err(VectorRefusal::Admission(_))),
        "{refused:?}"
    );
    assert!(
        fixture.generations().is_empty(),
        "a refused admission mutates nothing"
    );
    let digest = fixture
        .stage_with(&built, &admit_with_limit(total))
        .unwrap();
    assert_eq!(fixture.generations(), vec![digest]);
}

#[test]
fn staging_residue_is_swept_or_quarantined_and_never_verified_or_selected() {
    let fixture = Fixture::new();
    let generations = fixture.lifecycle_dir().join(GENERATIONS_DIR_NAME);
    // Residue of a cut during the copy: an incomplete staging temp.
    let temp = generations.join("tmp-0123456789abcdef");
    fs::create_dir(&temp).unwrap();
    fs::write(temp.join(ROWS_FILE), b"partial").unwrap();
    // Residue of a cut during the manifest write: a digest-named directory with a torn manifest.
    let built = fixture.build();
    let torn = fixture.digest_dir_with_torn_manifest(&built);
    assert!(fixture.store.validate(&torn).is_err());
    assert!(matches!(
        fixture.verify(&torn),
        Err(VectorRefusal::Store(_))
    ));
    assert!(fixture.select_vector(&torn).is_err());

    let digest = fixture.stage(&built).unwrap();
    assert_ne!(digest, torn);
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_temps, 1, "the incomplete temp is swept");
    assert_eq!(
        report.removed_generations, 2,
        "the torn residue and the unselected generation are reclaimed"
    );
    assert!(fixture.generations().is_empty());
}

impl Fixture {
    /// Plants a digest-named directory whose manifest is cut mid-write, as a crash between the manifest write and the rename would leave after the rename.
    fn digest_dir_with_torn_manifest(&self, built: &BuiltVectors) -> String {
        let manifest = built.sidecar.stage_manifest().canonical_bytes();
        let torn_digest = "e".repeat(64);
        let dir = self.generation_dir(&torn_digest);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        for (name, bytes) in file_bytes(&built.dir) {
            fs::write(dir.join(&name), bytes).unwrap();
            fs::set_permissions(dir.join(&name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(dir.join("manifest.json"), &manifest[..manifest.len() / 2]).unwrap();
        fs::set_permissions(dir.join("manifest.json"), fs::Permissions::from_mode(0o600)).unwrap();
        torn_digest
    }
}
