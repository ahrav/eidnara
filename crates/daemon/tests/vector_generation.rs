mod support;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use daemon::projection_gates::{Admission, EntryPoint, HookGate, ProjectionHook};
use daemon::vector_generation::{
    BuiltVectors, CODES_FILE, ExpectedVectors, FileFault, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE,
    SIDECAR_FILE, VectorRefusal, VectorSidecar, build, stage, verify,
};
use host_runtime::generation::{
    CurrentProfile, GENERATIONS_DIR_NAME, GenerationError, GenerationManifest, GenerationStore,
    ProfileEvent, SEARCH_PROFILE_NAME, SourceSpec, StageMeta, VECTOR_PROFILE_NAME, VECTOR_TARGET,
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
    work_dirs: std::cell::Cell<usize>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = GenerationStore::open(Some(root.path())).unwrap();
        let tx = LifecycleTransactionLock::acquire_exclusive(Some(root.path())).unwrap();
        let identity = identity("test-incarnation", DIMENSION);
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
            &self.store,
            &self.tx,
            &self.gate,
            admission,
            &self.identity,
            &BTreeSet::new(),
        )
    }

    fn verify(&self, digest: &str) -> Result<VectorSidecar, VectorRefusal> {
        verify(&self.store, digest, &self.expected()).map(|verified| verified.sidecar)
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

    fn selector_bytes(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.lifecycle_dir().join(name)).ok()
    }

    fn write_selector(&self, name: &str, bytes: &[u8]) {
        let path = self.lifecycle_dir().join(name);
        let _ = fs::remove_file(&path);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
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
            SIDECAR_FILE
        ]
    );
    assert_eq!(first.sidecar.rows, 4);
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

    let verified = verify(&fixture.store, &digest, &fixture.expected()).unwrap();
    assert_eq!(verified.sidecar, first.sidecar);
    verified.generation.pin().unwrap();
    let with_checkpoint = ExpectedVectors {
        checkpoint: Some(&export().checkpoint),
        ..fixture.expected()
    };
    assert!(verify(&fixture.store, &digest, &with_checkpoint).is_ok());
}

#[test]
fn build_refuses_no_rows_out_of_order_rows_and_rows_outside_the_layout() {
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
    assert!(
        fs::read_dir(&dir).unwrap().next().is_none(),
        "a refused build writes nothing"
    );
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
            verify(&fixture.store, &digest, &expected).unwrap_err(),
            VectorRefusal::Identity { field },
            "a different {field} is refused at an equal dimension"
        );
    }
    assert_eq!(
        verify(
            &fixture.store,
            &digest,
            &ExpectedVectors {
                unit_norm_tolerance: 2e-3,
                ..fixture.expected()
            }
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
            }
        )
        .unwrap_err(),
        VectorRefusal::Identity {
            field: "kernel_incarnation_id"
        }
    );
    let other_checkpoint = ProjectionCheckpoint {
        checkpoint_commit_seq: 10,
        ..export().checkpoint
    };
    assert_eq!(
        verify(
            &fixture.store,
            &digest,
            &ExpectedVectors {
                checkpoint: Some(&other_checkpoint),
                ..fixture.expected()
            }
        )
        .unwrap_err(),
        VectorRefusal::Identity {
            field: "checkpoint"
        }
    );
}

#[test]
fn the_vector_selector_refuses_other_owners_and_leaves_the_search_and_host_selectors_untouched() {
    let fixture = Fixture::new();
    let seed = fixture.stage_search_seed();
    fixture
        .store
        .select_search(&seed, &fixture.tx, &mut |_| Ok(()))
        .unwrap();
    let search_before = fixture.selector_bytes(SEARCH_PROFILE_NAME).unwrap();

    assert!(
        fixture.select_vector(&seed).is_err(),
        "a search seed belongs to another owner"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );

    let digest = fixture.stage(&fixture.build()).unwrap();
    fixture.select_vector(&digest).unwrap();
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Current(digest.clone())
    );
    assert_eq!(
        fixture.store.read_search_current().unwrap(),
        CurrentProfile::Current(seed.clone())
    );
    assert_eq!(
        fixture.store.read_current().unwrap(),
        CurrentProfile::Absent
    );
    assert_eq!(
        fixture.selector_bytes(SEARCH_PROFILE_NAME).unwrap(),
        search_before,
        "the search selector is byte-for-byte unchanged"
    );
    assert_eq!(
        fixture.store.reconcile_vector(&fixture.tx).unwrap(),
        CurrentProfile::Current(digest.clone())
    );

    // Both owners' selections survive a prune that removes an unselected third generation.
    let mut other = export();
    other.checkpoint.hold_id = "hold-9".to_owned();
    let third = fixture
        .stage(&build(&fixture.expected(), &other, &fixture.work_dir()).unwrap())
        .unwrap();
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 1);
    let mut retained = vec![seed, digest];
    retained.sort();
    assert_eq!(fixture.generations(), retained);
    assert!(!fixture.generations().contains(&third));
}

#[test]
fn a_selector_cut_before_the_rename_leaves_no_selection_and_one_after_it_is_reconciled() {
    let fixture = Fixture::new();
    let digest = fixture.stage(&fixture.build()).unwrap();
    for cut in [
        ProfileEvent::BeforeRename,
        ProfileEvent::AfterRename,
        ProfileEvent::BeforeDirectorySync,
        ProfileEvent::AfterDirectorySync,
    ] {
        let fixture = Fixture::new();
        let digest = fixture.stage(&fixture.build()).unwrap();
        let mut seen = Vec::new();
        let outcome = fixture
            .store
            .select_vector(&digest, &fixture.tx, &mut |event| {
                seen.push(event);
                if event == cut {
                    Err(GenerationError::NativePayloadInvalid { detail: "cut" })
                } else {
                    Ok(())
                }
            });
        assert!(outcome.is_err(), "{cut:?}");
        assert_eq!(
            *seen.last().unwrap(),
            cut,
            "the cut fired where it was placed"
        );
        let expected = if cut == ProfileEvent::BeforeRename {
            CurrentProfile::Absent
        } else {
            CurrentProfile::Current(digest.clone())
        };
        assert_eq!(
            fixture.store.read_vector_current().unwrap(),
            expected,
            "{cut:?}"
        );
        assert_eq!(
            fixture.store.reconcile_vector(&fixture.tx).unwrap(),
            expected,
            "{cut:?}"
        );
        let temps: Vec<String> = fs::read_dir(fixture.lifecycle_dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(&format!(".{VECTOR_PROFILE_NAME}")))
            .collect();
        assert!(
            temps.is_empty() || cut != ProfileEvent::BeforeRename,
            "{cut:?}: {temps:?}"
        );
    }
    let _ = digest;
}

#[test]
fn prune_discard_and_exchange_repair_respect_the_vector_selector() {
    let fixture = Fixture::new();
    let built = fixture.build();
    let digest = fixture.stage(&built).unwrap();
    fixture.select_vector(&digest).unwrap();

    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert_eq!(fixture.generations(), vec![digest.clone()]);

    assert!(
        fixture
            .store
            .discard_unselected(
                &built.sidecar.stage_manifest(),
                &fixture.tx,
                &BTreeSet::new()
            )
            .is_err(),
        "a selected vector generation cannot be discarded"
    );
    assert_eq!(fixture.generations(), vec![digest.clone()]);

    // Corrupt the selected generation in place, then stage the same bytes again: exchange repair refuses to replace a selected target.
    let dir = fixture.generation_dir(&digest);
    let pristine_codes = fs::read(dir.join(CODES_FILE)).unwrap();
    let mut codes = pristine_codes.clone();
    codes[0] ^= 0xff;
    fs::write(dir.join(CODES_FILE), &codes).unwrap();
    assert!(fixture.store.validate(&digest).is_err());
    let again = fixture.build();
    assert!(
        matches!(fixture.stage(&again), Err(VectorRefusal::Store(_))),
        "the corrupt selected target is protected from exchange"
    );
    assert_eq!(
        fixture.generations(),
        vec![digest.clone()],
        "no staging temp survives the refusal"
    );

    // Unselected, the same corrupt target is repaired by exchange.
    fs::remove_file(fixture.lifecycle_dir().join(VECTOR_PROFILE_NAME)).unwrap();
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );
    let repaired = fixture.stage(&again).unwrap();
    assert_eq!(repaired, digest);
    assert!(fixture.verify(&digest).is_ok());
    assert_eq!(fs::read(dir.join(CODES_FILE)).unwrap(), pristine_codes);

    // A vector selector of unknown schema quarantines: prune keeps every generation and removes only temps; discard, exchange repair, and selection refuse.
    fixture.write_selector(VECTOR_PROFILE_NAME, br#"{"schema":9,"current":"zz"}"#);
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Quarantined
    );
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_generations, 0);
    assert!(report.quarantined >= 1);
    assert_eq!(fixture.generations(), vec![digest.clone()]);
    assert!(matches!(
        fixture.select_vector(&digest),
        Err(GenerationError::UnsupportedStateSchema)
    ));
    assert!(matches!(
        fixture.store.discard_unselected(
            &built.sidecar.stage_manifest(),
            &fixture.tx,
            &BTreeSet::new()
        ),
        Err(GenerationError::UnsupportedStateSchema)
    ));
    fs::write(dir.join(CODES_FILE), &codes).unwrap();
    assert!(
        matches!(fixture.stage(&again), Err(VectorRefusal::Quarantined)),
        "exchange repair refuses under a quarantined owner selector"
    );
    fs::write(dir.join(CODES_FILE), &pristine_codes).unwrap();
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
