//! Builds one immutable vector generation from the original rows a projection holds and verifies a staged one independently of its manifest.
//!
//! The generation is five files: the original-row artifact, the int8 codes, the scales, the row identifiers, and a sidecar that names every one of them by size and hash and binds the model, tokenizer, dimension, metric, normalization, recipe, calibration provenance, source checkpoint, and epoch.
//! The sidecar's hash rides in the outer manifest's inputs slot, so the generation digest is a function of every input; two builds over byte-identical inputs produce the same directory name, the same bytes, and the same digest.
//! Verification re-derives the codes from the rows and the scales and compares bytes, so a state whose hashes were rewritten to match each other is still refused when its meaning changed.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use host_runtime::generation::{
    GenerationError, GenerationManifest, GenerationStore, ManifestFile, SourceSpec, StageMeta,
    ValidatedGeneration,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::export::ExportedRow;
use retrieval::dense::scalar::{self, ScalarRecipe, Scales};
use rustix::fs::OFlags;
use sha2::{Digest, Sha256};

use crate::projection_gates::{Admission, Denial, HookGate, InvalidationIdentity};
use crate::search_seed::hex;

pub const VECTOR_TARGET: &str = "vector-generation";
pub const ROWS_FILE: &str = "rows.f32";
pub const CODES_FILE: &str = "codes.int8";
pub const SCALES_FILE: &str = "scales.f32";
pub const ROW_IDS_FILE: &str = "row-ids.json";
pub const SIDECAR_FILE: &str = "vector-sidecar.json";
pub const SIDECAR_SCHEMA: u32 = 1;
/// The disk limit the admission manifest names for staged bytes; the vector build charges its whole inventory against it.
const STAGE_DISK_LIMIT: &str = "capture_disk_bytes";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Field order is the wire order; `canonical_bytes` is the only encoding and its hash is the sidecar's identity.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorSidecar {
    pub schema: u32,
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: u32,
    pub metric: String,
    pub unit_norm_tolerance: f64,
    pub quantizer_recipe: String,
    pub calibrated_rows: u64,
    pub scales_sha256: String,
    pub generation_id: String,
    pub generation_epoch: u64,
    pub kernel_incarnation_id: String,
    pub snapshot_commit_seq: i64,
    pub checkpoint_commit_seq: i64,
    pub hold_id: String,
    pub rows: u64,
    pub files: Vec<SidecarFile>,
}

impl VectorSidecar {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("sidecar serialization cannot fail")
    }

    pub fn sha256(&self) -> String {
        hex(&Sha256::digest(self.canonical_bytes()))
    }

    pub fn metric(&self) -> Option<Metric> {
        match self.metric.as_str() {
            "inner_product" => Some(Metric::InnerProduct),
            _ => None,
        }
    }

    pub fn layout(&self) -> Option<RowLayout> {
        Some(RowLayout {
            dimension: self.vector_dimension,
            metric: self.metric()?,
            unit_norm_tolerance: self.unit_norm_tolerance,
        })
    }

    fn file(&self, path: &str) -> Option<&SidecarFile> {
        self.files.iter().find(|file| file.path == path)
    }

    /// The compatibility identity's digest fills the contract slot, the sidecar's hash the inputs slot, and the row artifact's hash the payload slot; no release contract or inputs lock exists for a vector generation.
    pub fn stage_meta(&self) -> StageMeta {
        let compatibility = serde_json::to_vec(&[
            serde_json::Value::from(self.embedding_model.as_str()),
            serde_json::Value::from(self.tokenizer_fingerprint.as_str()),
            serde_json::Value::from(self.vector_dimension),
            serde_json::Value::from(self.metric.as_str()),
            serde_json::Value::from(self.unit_norm_tolerance.to_bits()),
            serde_json::Value::from(self.quantizer_recipe.as_str()),
            serde_json::Value::from(self.generation_epoch),
        ])
        .expect("identity serialization cannot fail");
        StageMeta {
            target: VECTOR_TARGET.to_owned(),
            release_contract_sha256: hex(&Sha256::digest(&compatibility)),
            inputs_lock_sha256: self.sha256(),
            source_payload_manifest_sha256: self
                .file(ROWS_FILE)
                .map(|file| file.sha256.clone())
                .unwrap_or_default(),
        }
    }

    pub fn stage_manifest(&self) -> GenerationManifest {
        let mut files: Vec<ManifestFile> = self
            .files
            .iter()
            .map(|file| ManifestFile {
                path: file.path.clone(),
                mode: 0o600,
                size: file.size,
                sha256: file.sha256.clone(),
            })
            .collect();
        let sidecar = self.canonical_bytes();
        files.push(ManifestFile {
            path: SIDECAR_FILE.to_owned(),
            mode: 0o600,
            size: sidecar.len() as u64,
            sha256: hex(&Sha256::digest(&sidecar)),
        });
        GenerationManifest::from_files(&self.stage_meta(), files)
    }
}

/// The identity a caller expects a generation to carry, checked against the sidecar field by field.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedVectors<'a> {
    pub identity: &'a ProjectionIdentity,
    pub generation: &'a VectorGeneration,
    pub metric: Metric,
    pub unit_norm_tolerance: f64,
    pub recipe: ScalarRecipe,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VectorRefusal {
    #[error("the generation and the identity disagree on {field}")]
    Identity { field: &'static str },
    #[error("no rows were supplied")]
    NoRows,
    #[error("row {index} is out of identifier order or repeats a row")]
    RowOrder { index: usize },
    #[error("original rows: {0}")]
    Rows(codec::ArtifactRejection),
    #[error("calibration: {0}")]
    Calibration(scalar::CalibrationRejection),
    #[error("admission refused staging: {0}")]
    Admission(Denial),
    #[error("the staged bytes are not the built bytes")]
    BytesChanged,
    #[error("the lifecycle store refused the generation: {0}")]
    Store(String),
    #[error("insufficient storage for the generation")]
    InsufficientStorage,
    #[error("the generation is not a vector generation: {0}")]
    NotVectors(&'static str),
    #[error("{path}: {detail}")]
    File { path: &'static str, detail: String },
    #[error("i/o failure: {0}")]
    Io(String),
}

impl From<GenerationError> for VectorRefusal {
    fn from(error: GenerationError) -> Self {
        match error {
            GenerationError::InsufficientStorage => Self::InsufficientStorage,
            other => Self::Store(other.to_string()),
        }
    }
}

/// The files of one build, written into a work directory and described by their sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltVectors {
    pub sidecar: VectorSidecar,
    pub dir: PathBuf,
}

impl BuiltVectors {
    pub fn digest(&self) -> String {
        self.sidecar.stage_manifest().digest()
    }
}

/// Writes rows, codes, scales, and identifiers under `work_dir` and describes them in a sidecar bound to `expected` and `checkpoint`.
/// Rows must arrive in strictly increasing identifier order so the artifact, the codes, and the identifier list agree on row numbering across builds.
///
/// # Errors
///
/// No rows, rows out of order, a row outside the layout, a calibration refusal, or an I/O failure; nothing is staged.
pub fn build(
    expected: &ExpectedVectors<'_>,
    checkpoint: &ProjectionCheckpoint,
    rows: &[ExportedRow],
    work_dir: &Path,
) -> Result<BuiltVectors, VectorRefusal> {
    if rows.is_empty() {
        return Err(VectorRefusal::NoRows);
    }
    if let Some(index) =
        (1..rows.len()).find(|i| rows[*i - 1].occurrence_id >= rows[*i].occurrence_id)
    {
        return Err(VectorRefusal::RowOrder { index });
    }
    let layout = RowLayout {
        dimension: expected.generation.vector_dimension,
        metric: expected.metric,
        unit_norm_tolerance: expected.unit_norm_tolerance,
    };
    let vectors = rows.iter().map(|row| row.vector.as_slice());
    let rows_bytes = codec::encode_rows(&layout, vectors.clone()).map_err(VectorRefusal::Rows)?;
    let calibration =
        scalar::calibrate(&layout, vectors.clone()).map_err(VectorRefusal::Calibration)?;
    let mut codes = Vec::with_capacity(rows.len() * layout.dimension as usize);
    for row in rows {
        let encoded =
            scalar::encode(&layout, &calibration.scales, &row.vector).map_err(|rejection| {
                VectorRefusal::Rows(codec::ArtifactRejection::Row {
                    index: 0,
                    rejection,
                })
            })?;
        codes.extend(scalar::encode_codes(&encoded.codes));
    }
    let scales_bytes = calibration.scales.encode();
    let ids: Vec<&str> = rows.iter().map(|row| row.occurrence_id.as_str()).collect();
    let ids_bytes = serde_json::to_vec(&ids).expect("identifier serialization cannot fail");
    let files = [
        (ROWS_FILE, rows_bytes),
        (CODES_FILE, codes),
        (SCALES_FILE, scales_bytes),
        (ROW_IDS_FILE, ids_bytes),
    ];
    let mut inventory = Vec::with_capacity(files.len());
    for (path, bytes) in &files {
        write_new(&work_dir.join(path), bytes)?;
        inventory.push(SidecarFile {
            path: (*path).to_owned(),
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
        });
    }
    let sidecar = VectorSidecar {
        schema: SIDECAR_SCHEMA,
        embedding_model: expected.identity.embedding_model.clone(),
        tokenizer_fingerprint: expected.identity.tokenizer_fingerprint.clone(),
        vector_dimension: layout.dimension,
        metric: expected.metric.name().to_owned(),
        unit_norm_tolerance: layout.unit_norm_tolerance,
        quantizer_recipe: expected.recipe.id().to_owned(),
        calibrated_rows: calibration.identity.calibrated_rows,
        scales_sha256: hex(&calibration.identity.scales_digest),
        generation_id: expected.generation.generation_id.clone(),
        generation_epoch: expected.generation.generation_epoch,
        kernel_incarnation_id: expected.identity.kernel_incarnation_id.clone(),
        snapshot_commit_seq: checkpoint.snapshot_commit_seq,
        checkpoint_commit_seq: checkpoint.checkpoint_commit_seq,
        hold_id: checkpoint.hold_id.clone(),
        rows: rows.len() as u64,
        files: inventory,
    };
    write_new(&work_dir.join(SIDECAR_FILE), &sidecar.canonical_bytes())?;
    Ok(BuiltVectors {
        sidecar,
        dir: work_dir.to_path_buf(),
    })
}

/// Stages a build into `store` after the admission gate accepts its whole inventory against the staged-bytes limit; a refused admission stages nothing.
/// `_transaction` is the caller's exclusive hold on the store's transaction lock; hold it until the digest is pinned or protected.
///
/// # Errors
///
/// An admission denial, the store's refusal, or a digest that differs from the built manifest's, which means the files changed between build and stage.
pub fn stage(
    built: &BuiltVectors,
    store: &GenerationStore,
    _transaction: &LifecycleTransactionLock,
    gate: &HookGate,
    admission: &Admission,
    identity: &ProjectionIdentity,
    protected: &BTreeSet<String>,
) -> Result<String, VectorRefusal> {
    let manifest = built.sidecar.stage_manifest();
    let bytes: u64 = manifest.files.iter().map(|file| file.size).sum();
    gate.check_limits(
        admission,
        &InvalidationIdentity::from(identity),
        &[(STAGE_DISK_LIMIT, bytes)],
    )
    .map_err(VectorRefusal::Admission)?;
    let sources: Vec<SourceSpec> = manifest
        .files
        .iter()
        .map(|file| SourceSpec {
            rel_path: file.path.clone(),
            source: built.dir.join(&file.path),
            executable: false,
            expected_size: Some(file.size),
            expected_sha256: Some(file.sha256.clone()),
        })
        .collect();
    let digest = store.stage(&sources, &built.sidecar.stage_meta(), protected)?;
    if digest != manifest.digest() {
        return Err(VectorRefusal::BytesChanged);
    }
    Ok(digest)
}

/// A generation whose files, sidecar, and meaning were all checked.
pub struct VerifiedVectors {
    pub digest: String,
    pub sidecar: VectorSidecar,
    /// Retains the directory descriptor and, once pinned, the shared lock.
    pub generation: ValidatedGeneration,
}

impl std::fmt::Debug for VerifiedVectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedVectors")
            .field("digest", &self.digest)
            .field("sidecar", &self.sidecar)
            .finish_non_exhaustive()
    }
}

/// Verifies `digest` independently of its manifest: the store checks inventory, sizes, modes, and hashes; this checks that the manifest is a vector manifest bound to the sidecar, that the sidecar carries `expected`, and that the rows, scales, codes, and identifiers decode and agree with one another under the recipe.
///
/// # Errors
///
/// Any disagreement refuses; nothing is selected or pinned.
pub fn verify(
    store: &GenerationStore,
    digest: &str,
    expected: &ExpectedVectors<'_>,
) -> Result<VerifiedVectors, VectorRefusal> {
    let generation = store.validate(digest)?;
    let manifest = &generation.manifest;
    if manifest.target != VECTOR_TARGET {
        return Err(VectorRefusal::NotVectors("manifest target"));
    }
    let sidecar_bytes = read_verified(&generation, SIDECAR_FILE)?;
    let sidecar: VectorSidecar =
        serde_json::from_slice(&sidecar_bytes).map_err(|_| VectorRefusal::NotVectors("sidecar"))?;
    if sidecar.schema != SIDECAR_SCHEMA {
        return Err(VectorRefusal::NotVectors("sidecar schema"));
    }
    if sidecar.canonical_bytes() != sidecar_bytes || sidecar.stage_manifest() != *manifest {
        return Err(VectorRefusal::NotVectors("manifest binding"));
    }
    check_identity(&sidecar, expected)?;
    let layout = sidecar
        .layout()
        .ok_or(VectorRefusal::NotVectors("metric"))?;
    let rows = codec::decode_rows(&read_verified(&generation, ROWS_FILE)?, &layout)
        .map_err(VectorRefusal::Rows)?;
    let file = |path: &'static str, detail: &str| VectorRefusal::File {
        path,
        detail: detail.to_owned(),
    };
    if rows.rows.len() as u64 != sidecar.rows {
        return Err(file(ROWS_FILE, "row count differs from the sidecar"));
    }
    let scales = Scales::decode(&read_verified(&generation, SCALES_FILE)?, layout.dimension)
        .map_err(|rejection| file(SCALES_FILE, &rejection.to_string()))?;
    if hex(&scales.digest()) != sidecar.scales_sha256 {
        return Err(file(
            SCALES_FILE,
            "scales differ from the calibration provenance",
        ));
    }
    let ids: Vec<String> = serde_json::from_slice(&read_verified(&generation, ROW_IDS_FILE)?)
        .map_err(|_| file(ROW_IDS_FILE, "not a JSON array of identifiers"))?;
    if ids.len() != rows.rows.len() || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(file(
            ROW_IDS_FILE,
            "identifiers do not number the rows in order",
        ));
    }
    let mut expected_codes = Vec::with_capacity(rows.rows.len() * layout.dimension as usize);
    for row in &rows.rows {
        let encoded = scalar::encode(&layout, &scales, row)
            .map_err(|rejection| file(CODES_FILE, &rejection.to_string()))?;
        expected_codes.extend(scalar::encode_codes(&encoded.codes));
    }
    if read_verified(&generation, CODES_FILE)? != expected_codes {
        return Err(file(
            CODES_FILE,
            "codes are not the rows encoded under the scales",
        ));
    }
    Ok(VerifiedVectors {
        digest: digest.to_owned(),
        sidecar,
        generation,
    })
}

/// Every field is compared, so a different model space is refused even at an equal dimension.
fn check_identity(
    sidecar: &VectorSidecar,
    expected: &ExpectedVectors<'_>,
) -> Result<(), VectorRefusal> {
    let identity = |field| VectorRefusal::Identity { field };
    let generation = expected.generation;
    let checks: [(&'static str, bool); 10] = [
        (
            "embedding_model",
            sidecar.embedding_model == expected.identity.embedding_model,
        ),
        (
            "tokenizer_fingerprint",
            sidecar.tokenizer_fingerprint == expected.identity.tokenizer_fingerprint,
        ),
        (
            "vector_dimension",
            sidecar.vector_dimension == generation.vector_dimension,
        ),
        ("metric", sidecar.metric() == Some(expected.metric)),
        (
            "unit_norm_tolerance",
            sidecar.unit_norm_tolerance.to_bits() == expected.unit_norm_tolerance.to_bits(),
        ),
        (
            "quantizer_recipe",
            ScalarRecipe::from_id(&sidecar.quantizer_recipe) == Some(expected.recipe),
        ),
        (
            "generation_id",
            sidecar.generation_id == generation.generation_id,
        ),
        (
            "generation_epoch",
            sidecar.generation_epoch == generation.generation_epoch,
        ),
        (
            "kernel_incarnation_id",
            sidecar.kernel_incarnation_id == expected.identity.kernel_incarnation_id,
        ),
        (
            "generation identity",
            generation.embedding_model == expected.identity.embedding_model
                && generation.tokenizer_fingerprint == expected.identity.tokenizer_fingerprint
                && generation.vector_dimension == expected.identity.vector_dimension
                && generation.generation_epoch == expected.identity.generation_epoch,
        ),
    ];
    checks
        .into_iter()
        .find(|(_, holds)| !holds)
        .map_or(Ok(()), |(field, _)| Err(identity(field)))
}

fn read_verified(
    generation: &ValidatedGeneration,
    path: &'static str,
) -> Result<Vec<u8>, VectorRefusal> {
    let fd = generation.open_verified_file(path)?;
    let mut file = fs::File::from(fd);
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut file, &mut bytes).map_err(|error| VectorRefusal::File {
        path,
        detail: error.kind().to_string(),
    })?;
    Ok(bytes)
}

/// `O_EXCL` so a build never overwrites a file another build left behind; the caller owns the work directory.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), VectorRefusal> {
    let io = |error: io::Error| VectorRefusal::Io(error.kind().to_string());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
        .open(path)
        .map_err(io)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io)
}
