mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use daemon::projection_gates::{Admission, EntryPoint, HookGate, ProjectionHook, RuntimeManifest};
use daemon::vector_generation::{
    BuiltVectors, CODES_FILE, ExpectedVectors, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE, SIDECAR_FILE,
    VECTOR_TARGET, VectorRefusal, VectorSidecar, build, stage, verify,
};
use host_runtime::generation::{
    CurrentProfile, GENERATIONS_DIR_NAME, GenerationError, GenerationStore, SEARCH_PROFILE_NAME,
    SourceSpec, StageMeta, VECTOR_PROFILE_NAME,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use kernel::source_identity::OccurrenceClass;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::Metric;
use retrieval::dense::export::ExportedRow;
use retrieval::dense::scalar::ScalarRecipe;
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

fn rows() -> Vec<ExportedRow> {
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
    rows.into_iter()
        .map(|(occurrence_id, vector)| ExportedRow {
            occurrence_id,
            class: OccurrenceClass::Messages,
            vector,
        })
        .collect()
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

fn checkpoint() -> ProjectionCheckpoint {
    ProjectionCheckpoint {
        snapshot_commit_seq: 3,
        checkpoint_commit_seq: 9,
        hold_id: "hold-7".to_owned(),
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
        }
    }

    fn expected(&self) -> ExpectedVectors<'_> {
        ExpectedVectors {
            identity: &self.identity,
            generation: &self.generation,
            metric: Metric::InnerProduct,
            unit_norm_tolerance: TOLERANCE,
            recipe: ScalarRecipe::SymmetricInt8V1,
        }
    }

    fn build_in(&self, name: &str) -> BuiltVectors {
        let dir = self.root.path().join(name);
        fs::create_dir(&dir).unwrap();
        build(&self.expected(), &checkpoint(), &rows(), &dir).unwrap()
    }

    fn stage(&self, built: &BuiltVectors) -> Result<String, VectorRefusal> {
        stage(
            built,
            &self.store,
            &self.tx,
            &self.gate,
            &self.admission,
            &self.identity,
            &BTreeSet::new(),
        )
    }

    fn generation_dir(&self, digest: &str) -> std::path::PathBuf {
        self.root
            .path()
            .join("eidnara")
            .join("lifecycle")
            .join(GENERATIONS_DIR_NAME)
            .join(digest)
    }

    fn generations(&self) -> Vec<String> {
        let dir = self
            .root
            .path()
            .join("eidnara")
            .join("lifecycle")
            .join(GENERATIONS_DIR_NAME);
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn select_vector(&self, digest: &str) -> Result<(), GenerationError> {
        self.store
            .select_vector(digest, VECTOR_TARGET, &self.tx, &mut |_| Ok(()))
    }

    fn stage_search_seed(&self) -> String {
        let dir = self.root.path().join("seed-source");
        fs::create_dir(&dir).unwrap();
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

/// Rewrites one staged file and every hash that names it, so only the meaning disagrees.
fn rehash(fixture: &Fixture, digest: &str, path: &str, bytes: &[u8]) -> String {
    let dir = fixture.generation_dir(digest);
    fs::write(dir.join(path), bytes).unwrap();
    let mut sidecar: VectorSidecar =
        serde_json::from_slice(&fs::read(dir.join(SIDECAR_FILE)).unwrap()).unwrap();
    for file in &mut sidecar.files {
        if file.path == path {
            file.size = bytes.len() as u64;
            file.sha256 = sha(bytes);
        }
    }
    fs::write(dir.join(SIDECAR_FILE), sidecar.canonical_bytes()).unwrap();
    let manifest = sidecar.stage_manifest();
    fs::write(dir.join("manifest.json"), manifest.canonical_bytes()).unwrap();
    let new_digest = manifest.digest();
    fs::rename(&dir, fixture.generation_dir(&new_digest)).unwrap();
    new_digest
}

#[test]
fn paired_fresh_builds_produce_identical_names_bytes_sidecar_manifest_and_digest() {
    let fixture = Fixture::new();
    let first = fixture.build_in("first");
    let second = fixture.build_in("second");
    assert_eq!(first.sidecar, second.sidecar);
    assert_eq!(first.digest(), second.digest());
    assert_eq!(file_bytes(&first.dir), file_bytes(&second.dir));
    let files = file_bytes(&first.dir);
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
    assert_eq!(first.sidecar.stage_meta().target, VECTOR_TARGET);
    assert_eq!(
        first.sidecar.stage_meta().inputs_lock_sha256,
        first.sidecar.sha256()
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
}

#[test]
fn build_refuses_no_rows_out_of_order_rows_and_rows_outside_the_layout() {
    let fixture = Fixture::new();
    let dir = fixture.root.path().join("work");
    fs::create_dir(&dir).unwrap();
    assert_eq!(
        build(&fixture.expected(), &checkpoint(), &[], &dir),
        Err(VectorRefusal::NoRows)
    );
    let mut reversed = rows();
    reversed.reverse();
    assert_eq!(
        build(&fixture.expected(), &checkpoint(), &reversed, &dir),
        Err(VectorRefusal::RowOrder { index: 1 })
    );
    let mut duplicated = rows();
    duplicated[1].occurrence_id = duplicated[0].occurrence_id.clone();
    assert_eq!(
        build(&fixture.expected(), &checkpoint(), &duplicated, &dir),
        Err(VectorRefusal::RowOrder { index: 1 })
    );
    let mut unnormalized = rows();
    unnormalized[2].vector = vec![1.0; 8];
    assert!(matches!(
        build(&fixture.expected(), &checkpoint(), &unnormalized, &dir),
        Err(VectorRefusal::Rows(_))
    ));
    assert!(
        fs::read_dir(&dir).unwrap().next().is_none(),
        "a refused build writes nothing"
    );
}

#[test]
fn verification_refuses_missing_extra_truncated_corrupt_and_rehashed_state() {
    let fixture = Fixture::new();
    let built = fixture.build_in("work");
    let digest = fixture.stage(&built).unwrap();
    let dir = fixture.generation_dir(&digest);
    let pristine = file_bytes(&dir);
    let restore = |fixture: &Fixture| {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture.generation_dir(&digest);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        for (name, bytes) in &pristine {
            fs::write(dir.join(name), bytes).unwrap();
            fs::set_permissions(dir.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(
            fixture.store.validate(&digest).is_ok(),
            "the restored control validates"
        );
    };

    fs::remove_file(dir.join(CODES_FILE)).unwrap();
    assert!(
        matches!(
            verify(&fixture.store, &digest, &fixture.expected()),
            Err(VectorRefusal::Store(_))
        ),
        "missing file"
    );
    restore(&fixture);

    fs::write(dir.join("extra.bin"), b"x").unwrap();
    assert!(
        matches!(
            verify(&fixture.store, &digest, &fixture.expected()),
            Err(VectorRefusal::Store(_))
        ),
        "extra file"
    );
    restore(&fixture);

    let scales = fs::read(dir.join(SCALES_FILE)).unwrap();
    fs::write(dir.join(SCALES_FILE), &scales[..scales.len() - 4]).unwrap();
    assert!(
        matches!(
            verify(&fixture.store, &digest, &fixture.expected()),
            Err(VectorRefusal::Store(_))
        ),
        "truncated file"
    );
    restore(&fixture);

    let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
    codes[3] ^= 0x01;
    fs::write(dir.join(CODES_FILE), &codes).unwrap();
    assert!(
        matches!(
            verify(&fixture.store, &digest, &fixture.expected()),
            Err(VectorRefusal::Store(_))
        ),
        "corrupt file"
    );
    restore(&fixture);

    // Rehashed: every hash agrees with the bytes, only the meaning is wrong.
    let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
    codes[3] = codes[3].wrapping_add(1);
    let rehashed = rehash(&fixture, &digest, CODES_FILE, &codes);
    assert!(
        fixture.store.validate(&rehashed).is_ok(),
        "the store sees a consistent generation"
    );
    assert_eq!(
        verify(&fixture.store, &rehashed, &fixture.expected()).unwrap_err(),
        VectorRefusal::File {
            path: CODES_FILE,
            detail: "codes are not the rows encoded under the scales".to_owned()
        }
    );
    fs::rename(fixture.generation_dir(&rehashed), &dir).unwrap();
    restore(&fixture);

    let mut scales = fs::read(dir.join(SCALES_FILE)).unwrap();
    scales[0] ^= 0x10;
    let rehashed = rehash(&fixture, &digest, SCALES_FILE, &scales);
    assert_eq!(
        verify(&fixture.store, &rehashed, &fixture.expected()).unwrap_err(),
        VectorRefusal::File {
            path: SCALES_FILE,
            detail: "scales differ from the calibration provenance".to_owned()
        }
    );
    fs::rename(fixture.generation_dir(&rehashed), &dir).unwrap();
    restore(&fixture);

    let mut ids: Vec<String> =
        serde_json::from_slice(&fs::read(dir.join(ROW_IDS_FILE)).unwrap()).unwrap();
    ids.swap(0, 1);
    let rehashed = rehash(
        &fixture,
        &digest,
        ROW_IDS_FILE,
        &serde_json::to_vec(&ids).unwrap(),
    );
    assert_eq!(
        verify(&fixture.store, &rehashed, &fixture.expected()).unwrap_err(),
        VectorRefusal::File {
            path: ROW_IDS_FILE,
            detail: "identifiers do not number the rows in order".to_owned()
        }
    );
    fs::rename(fixture.generation_dir(&rehashed), &dir).unwrap();
    restore(&fixture);

    assert!(
        verify(&fixture.store, &digest, &fixture.expected()).is_ok(),
        "the control verifies"
    );
}

#[test]
fn verification_refuses_another_owner_an_unknown_sidecar_schema_and_a_foreign_model_space() {
    let fixture = Fixture::new();
    let seed = fixture.stage_search_seed();
    assert_eq!(
        verify(&fixture.store, &seed, &fixture.expected()).unwrap_err(),
        VectorRefusal::NotVectors("manifest target")
    );

    let built = fixture.build_in("work");
    let digest = fixture.stage(&built).unwrap();
    let dir = fixture.generation_dir(&digest);
    let mut sidecar: VectorSidecar =
        serde_json::from_slice(&fs::read(dir.join(SIDECAR_FILE)).unwrap()).unwrap();
    sidecar.schema = 2;
    let bytes = sidecar.canonical_bytes();
    let mut manifest = sidecar.stage_manifest();
    for file in &mut manifest.files {
        if file.path == SIDECAR_FILE {
            file.size = bytes.len() as u64;
            file.sha256 = sha(&bytes);
        }
    }
    fs::write(dir.join(SIDECAR_FILE), &bytes).unwrap();
    fs::write(dir.join("manifest.json"), manifest.canonical_bytes()).unwrap();
    let unknown = manifest.digest();
    fs::rename(&dir, fixture.generation_dir(&unknown)).unwrap();
    assert_eq!(
        verify(&fixture.store, &unknown, &fixture.expected()).unwrap_err(),
        VectorRefusal::NotVectors("sidecar schema")
    );
    fs::rename(fixture.generation_dir(&unknown), &dir).unwrap();
    fs::write(dir.join(SIDECAR_FILE), built.sidecar.canonical_bytes()).unwrap();
    fs::write(
        dir.join("manifest.json"),
        built.sidecar.stage_manifest().canonical_bytes(),
    )
    .unwrap();

    let mut other_model = fixture.identity.clone();
    other_model.embedding_model = "another-model".to_owned();
    let mut other_generation = fixture.generation.clone();
    other_generation.embedding_model = "another-model".to_owned();
    let expected = ExpectedVectors {
        identity: &other_model,
        generation: &other_generation,
        ..fixture.expected()
    };
    assert_eq!(
        verify(&fixture.store, &digest, &expected).unwrap_err(),
        VectorRefusal::Identity {
            field: "embedding_model"
        },
        "a different model space is refused at an equal dimension"
    );
    let other_recipe = ExpectedVectors {
        unit_norm_tolerance: 2e-3,
        ..fixture.expected()
    };
    assert_eq!(
        verify(&fixture.store, &digest, &other_recipe).unwrap_err(),
        VectorRefusal::Identity {
            field: "unit_norm_tolerance"
        }
    );
    let mut other_epoch = fixture.generation.clone();
    other_epoch.generation_epoch = 2;
    let expected = ExpectedVectors {
        generation: &other_epoch,
        ..fixture.expected()
    };
    assert_eq!(
        verify(&fixture.store, &digest, &expected).unwrap_err(),
        VectorRefusal::Identity {
            field: "generation_epoch"
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
    let search_before = fs::read(
        fixture
            .root
            .path()
            .join("eidnara/lifecycle")
            .join(SEARCH_PROFILE_NAME),
    )
    .unwrap();

    assert!(
        fixture.select_vector(&seed).is_err(),
        "a search seed belongs to another owner"
    );
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );

    let built = fixture.build_in("work");
    let digest = fixture.stage(&built).unwrap();
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
        fs::read(
            fixture
                .root
                .path()
                .join("eidnara/lifecycle")
                .join(SEARCH_PROFILE_NAME)
        )
        .unwrap(),
        search_before,
        "the search selector is byte-for-byte unchanged"
    );
    assert_eq!(
        fixture.store.reconcile_vector(&fixture.tx).unwrap(),
        CurrentProfile::Current(digest)
    );
}

#[test]
fn prune_discard_and_exchange_repair_respect_the_vector_selector() {
    let fixture = Fixture::new();
    let built = fixture.build_in("work");
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

    // Corrupt the selected generation in place, then stage the same bytes again: exchange repair must refuse to replace a selected target.
    let dir = fixture.generation_dir(&digest);
    let mut codes = fs::read(dir.join(CODES_FILE)).unwrap();
    codes[0] ^= 0xff;
    fs::write(dir.join(CODES_FILE), &codes).unwrap();
    assert!(fixture.store.validate(&digest).is_err());
    let again = fixture.build_in("again");
    assert!(
        matches!(fixture.stage(&again), Err(VectorRefusal::Store(_))),
        "the corrupt selected target is protected from exchange"
    );
    assert_eq!(
        fixture.generations(),
        vec![digest.clone()],
        "no staging temp survives the refusal"
    );

    // An unselected corrupt target of the same digest is repaired by exchange.
    fs::write(
        fixture
            .root
            .path()
            .join("eidnara/lifecycle")
            .join(VECTOR_PROFILE_NAME),
        b"",
    )
    .unwrap();
    fs::remove_file(
        fixture
            .root
            .path()
            .join("eidnara/lifecycle")
            .join(VECTOR_PROFILE_NAME),
    )
    .unwrap();
    assert_eq!(
        fixture.store.read_vector_current().unwrap(),
        CurrentProfile::Absent
    );
    let repaired = fixture.stage(&again).unwrap();
    assert_eq!(repaired, digest);
    assert!(verify(&fixture.store, &digest, &fixture.expected()).is_ok());

    // A vector selector of unknown schema quarantines: prune keeps every generation and removes only temps.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(
            fixture
                .root
                .path()
                .join("eidnara/lifecycle")
                .join(VECTOR_PROFILE_NAME),
        )
        .unwrap();
    fs::write(
        fixture
            .root
            .path()
            .join("eidnara/lifecycle")
            .join(VECTOR_PROFILE_NAME),
        br#"{"schema":9,"current":"zz"}"#,
    )
    .unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(
        &mut fs::metadata(
            fixture
                .root
                .path()
                .join("eidnara/lifecycle")
                .join(VECTOR_PROFILE_NAME),
        )
        .unwrap()
        .permissions(),
        0o600,
    );
    fs::set_permissions(
        fixture
            .root
            .path()
            .join("eidnara/lifecycle")
            .join(VECTOR_PROFILE_NAME),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
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
}

#[test]
fn a_denied_admission_stages_nothing() {
    let fixture = Fixture::new();
    let built = fixture.build_in("work");
    let total: u64 = built
        .sidecar
        .stage_manifest()
        .files
        .iter()
        .map(|f| f.size)
        .sum();
    let mut evaluator = passing_evaluator(&fixture.identity, 0, &ProjectionHook::ALL);
    let RuntimeManifest { limits, .. } = &mut evaluator.manifest;
    limits.insert("capture_disk_bytes".to_owned(), total - 1);
    fixture.gate.install(evaluator);
    let admission = fixture
        .gate
        .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
        .unwrap();
    let refused = stage(
        &built,
        &fixture.store,
        &fixture.tx,
        &fixture.gate,
        &admission,
        &fixture.identity,
        &BTreeSet::new(),
    );
    assert!(
        matches!(refused, Err(VectorRefusal::Admission(_))),
        "{refused:?}"
    );
    assert!(
        fixture.generations().is_empty(),
        "a refused admission mutates nothing"
    );

    let mut evaluator = passing_evaluator(&fixture.identity, 0, &ProjectionHook::ALL);
    evaluator
        .manifest
        .limits
        .insert("capture_disk_bytes".to_owned(), total);
    fixture.gate.install(evaluator);
    let admission = fixture
        .gate
        .admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
        .unwrap();
    let digest = stage(
        &built,
        &fixture.store,
        &fixture.tx,
        &fixture.gate,
        &admission,
        &fixture.identity,
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(fixture.generations(), vec![digest]);
}

#[test]
fn an_incomplete_staging_temp_left_by_a_cut_is_swept_and_does_not_reach_the_store() {
    let fixture = Fixture::new();
    let temp = fixture
        .root
        .path()
        .join("eidnara/lifecycle")
        .join(GENERATIONS_DIR_NAME)
        .join("tmp-0123456789abcdef");
    fs::create_dir(&temp).unwrap();
    fs::write(temp.join(ROWS_FILE), b"partial").unwrap();
    let built = fixture.build_in("work");
    let digest = fixture.stage(&built).unwrap();
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_temps, 1);
    assert_eq!(
        report.removed_generations, 1,
        "the unselected, unprotected generation is reclaimed"
    );
    assert!(fixture.generations().is_empty());
    let _ = digest;
}
