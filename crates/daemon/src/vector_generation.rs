//! Builds one immutable vector generation from the original rows a projection holds and verifies a staged one independently of its manifest.
//!
//! The generation is five files: the original-row artifact, the int8 codes, the scales, the row identifiers, and a sidecar that names every one of them by size and hash and binds the model, tokenizer, dimension, metric, normalization, recipe, calibration provenance, source checkpoint, and epoch.
//! The sidecar's hash rides in the outer manifest's inputs slot, so the generation digest is a function of every input; two builds over byte-identical inputs produce the same directory name, the same bytes, and the same digest.
//! Verification recalibrates the scales and re-derives the codes from the rows and compares bytes, so a state whose hashes were rewritten to match each other is still refused when its meaning changed.

use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use host_runtime::generation::{
    GenerationError, GenerationManifest, GenerationStore, ManifestFile, StageMeta,
    ValidatedGeneration,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::export::LiveRows;
use retrieval::dense::scalar::{self, ScalarRecipe, Scales};
use rustix::fs::OFlags;
use sha2::{Digest, Sha256};

use crate::projection_gates::{Admission, Denial, HookGate, InvalidationIdentity};
use crate::search_seed::manifest_sources;

/// The manifest target of one vector layer: a base or a delta of original rows, codes, and scales.
pub const VECTOR_TARGET: &str = "vector-generation";
pub const ROWS_FILE: &str = "rows.f32";
pub const CODES_FILE: &str = "codes.int8";
pub const SCALES_FILE: &str = "scales.f32";
pub const ROW_IDS_FILE: &str = "row-ids.json";
pub const TOMBSTONES_FILE: &str = "tombstones.json";
pub const SIDECAR_FILE: &str = "vector-sidecar.json";
pub const SIDECAR_SCHEMA: u32 = 1;
/// Every file the sidecar inventories, in the order the build writes them.
const PAYLOAD_FILES: [&str; 5] = [
    ROWS_FILE,
    CODES_FILE,
    SCALES_FILE,
    ROW_IDS_FILE,
    TOMBSTONES_FILE,
];
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
    pub tombstones: u64,
    pub files: Vec<SidecarFile>,
}

impl VectorSidecar {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("sidecar serialization cannot fail")
    }

    pub fn sha256(&self) -> String {
        sha256_hex(&self.canonical_bytes())
    }

    pub fn layout(&self) -> Option<RowLayout> {
        Some(RowLayout {
            dimension: self.vector_dimension,
            metric: Metric::from_name(&self.metric)?,
            unit_norm_tolerance: self.unit_norm_tolerance,
        })
    }

    pub fn checkpoint(&self) -> ProjectionCheckpoint {
        ProjectionCheckpoint {
            snapshot_commit_seq: self.snapshot_commit_seq,
            checkpoint_commit_seq: self.checkpoint_commit_seq,
            hold_id: self.hold_id.clone(),
        }
    }

    fn inventories_exactly(&self) -> bool {
        self.files.len() == PAYLOAD_FILES.len()
            && self
                .files
                .iter()
                .zip(PAYLOAD_FILES)
                .all(|(file, path)| file.path == path)
    }

    /// The compatibility identity's digest fills the contract slot, the sidecar's hash the inputs slot, and the row artifact's hash the payload slot; no release contract or inputs lock exists for a vector generation.
    pub fn stage_meta(&self) -> StageMeta {
        StageMeta {
            target: VECTOR_TARGET.to_owned(),
            release_contract_sha256: VectorIdentity::from_sidecar(self).compatibility_sha256(),
            inputs_lock_sha256: self.sha256(),
            source_payload_manifest_sha256: self
                .files
                .iter()
                .find(|file| file.path == ROWS_FILE)
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
            sha256: sha256_hex(&sidecar),
        });
        GenerationManifest::from_files(&self.stage_meta(), files)
    }
}

/// The compatibility identity every layer and composition carries: what must agree before two artifacts share a metric space. Metric and recipe are their textual names, which `Metric::name` and `ScalarRecipe::id` map to one to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorIdentity {
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: u32,
    pub metric: String,
    pub unit_norm_tolerance_bits: u64,
    pub quantizer_recipe: String,
    pub generation_epoch: u64,
    pub kernel_incarnation_id: String,
}

impl VectorIdentity {
    pub fn from_expected(expected: &ExpectedVectors<'_>) -> Self {
        Self {
            embedding_model: expected.generation.embedding_model.clone(),
            tokenizer_fingerprint: expected.generation.tokenizer_fingerprint.clone(),
            vector_dimension: expected.generation.vector_dimension,
            metric: expected.metric.name().to_owned(),
            unit_norm_tolerance_bits: expected.unit_norm_tolerance.to_bits(),
            quantizer_recipe: expected.recipe.id().to_owned(),
            generation_epoch: expected.generation.generation_epoch,
            kernel_incarnation_id: expected.kernel_incarnation_id.to_owned(),
        }
    }

    pub fn from_sidecar(sidecar: &VectorSidecar) -> Self {
        Self {
            embedding_model: sidecar.embedding_model.clone(),
            tokenizer_fingerprint: sidecar.tokenizer_fingerprint.clone(),
            vector_dimension: sidecar.vector_dimension,
            metric: sidecar.metric.clone(),
            unit_norm_tolerance_bits: sidecar.unit_norm_tolerance.to_bits(),
            quantizer_recipe: sidecar.quantizer_recipe.clone(),
            generation_epoch: sidecar.generation_epoch,
            kernel_incarnation_id: sidecar.kernel_incarnation_id.clone(),
        }
    }

    /// The first field on which `self` and `other` disagree, in the order the fields are declared.
    pub fn first_mismatch(&self, other: &Self) -> Option<&'static str> {
        let checks: [(&'static str, bool); 8] = [
            (
                "embedding_model",
                self.embedding_model == other.embedding_model,
            ),
            (
                "tokenizer_fingerprint",
                self.tokenizer_fingerprint == other.tokenizer_fingerprint,
            ),
            (
                "vector_dimension",
                self.vector_dimension == other.vector_dimension,
            ),
            ("metric", self.metric == other.metric),
            (
                "unit_norm_tolerance",
                self.unit_norm_tolerance_bits == other.unit_norm_tolerance_bits,
            ),
            (
                "quantizer_recipe",
                self.quantizer_recipe == other.quantizer_recipe,
            ),
            (
                "generation_epoch",
                self.generation_epoch == other.generation_epoch,
            ),
            (
                "kernel_incarnation_id",
                self.kernel_incarnation_id == other.kernel_incarnation_id,
            ),
        ];
        checks
            .into_iter()
            .find(|(_, holds)| !holds)
            .map(|(field, _)| field)
    }

    /// The digest of the fields that decide compatibility, for a manifest's contract slot; the kernel incarnation is provenance, not compatibility, and stays out.
    pub fn compatibility_sha256(&self) -> String {
        let fields = serde_json::to_vec(&[
            serde_json::Value::from(self.embedding_model.as_str()),
            serde_json::Value::from(self.tokenizer_fingerprint.as_str()),
            serde_json::Value::from(self.vector_dimension),
            serde_json::Value::from(self.metric.as_str()),
            serde_json::Value::from(self.unit_norm_tolerance_bits),
            serde_json::Value::from(self.quantizer_recipe.as_str()),
            serde_json::Value::from(self.generation_epoch),
        ])
        .expect("identity serialization cannot fail");
        sha256_hex(&fields)
    }
}

/// What a caller expects a generation to carry, compared with the sidecar field by field.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedVectors<'a> {
    pub generation: &'a VectorGeneration,
    pub kernel_incarnation_id: &'a str,
    pub metric: Metric,
    pub unit_norm_tolerance: f64,
    pub recipe: ScalarRecipe,
    /// The checkpoint the rows were read at; `None` accepts any checkpoint and reads it from the sidecar.
    pub checkpoint: Option<&'a ProjectionCheckpoint>,
}

/// Which payload file a verified generation disagrees with itself about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFault {
    /// The row artifact holds another number of rows than the sidecar declares.
    RowCount,
    /// The scales are not the calibration of the rows, or the sidecar's provenance does not name them.
    Calibration,
    /// The identifiers do not number the rows in strictly increasing order, or the tombstones are not as many as declared, strictly increasing, and disjoint from them.
    Identifiers,
    /// The codes are not the rows encoded under the scales.
    Codes,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VectorRefusal {
    #[error("the generation and the expectation disagree on {field}")]
    Identity { field: &'static str },
    #[error("no rows were supplied")]
    NoRows,
    #[error("row {index} is out of identifier order or repeats a row")]
    RowOrder { index: usize },
    #[error("tombstone {index} is out of identifier order or repeats a tombstone")]
    TombstoneOrder { index: usize },
    #[error("row {index} is both listed and tombstoned by the layer")]
    ListedAndTombstoned { index: usize },
    #[error("original rows: {0}")]
    Rows(codec::ArtifactRejection),
    #[error("calibration: {0}")]
    Calibration(scalar::CalibrationRejection),
    #[error("admission refused staging: {0}")]
    Admission(Denial),
    #[error("the lifecycle store refused the generation: {0}")]
    Store(String),
    #[error("the generation carries a state schema this build does not know")]
    Quarantined,
    #[error("insufficient storage for the generation")]
    InsufficientStorage,
    #[error("the generation is not a vector generation: {0}")]
    NotVectors(&'static str),
    #[error("{path}: {fault:?}")]
    File {
        path: &'static str,
        fault: FileFault,
    },
    #[error("i/o failure: {0}")]
    Io(String),
}

impl From<GenerationError> for VectorRefusal {
    fn from(error: GenerationError) -> Self {
        match error {
            GenerationError::InsufficientStorage => Self::InsufficientStorage,
            GenerationError::UnsupportedStateSchema => Self::Quarantined,
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

/// Writes rows, codes, scales, identifiers, and tombstones under `work_dir` and describes them in a sidecar bound to `expected` and the export's checkpoint.
/// Rows and tombstones must arrive in strictly increasing identifier order, and no occurrence may be both, so the artifact, the codes, the identifier list, and the resolver agree on what the layer says across builds.
/// Files are created exclusively and never synced: the store copies and syncs them when it stages, so the work directory is scratch, and a retry needs a fresh one.
///
/// # Errors
///
/// No rows, rows or tombstones out of order, an occurrence both listed and tombstoned, a row outside the layout, a calibration refusal, or an I/O failure; nothing is staged.
pub fn build(
    expected: &ExpectedVectors<'_>,
    export: &LiveRows,
    work_dir: &Path,
) -> Result<BuiltVectors, VectorRefusal> {
    let rows = &export.rows;
    if rows.is_empty() {
        return Err(VectorRefusal::NoRows);
    }
    if let Some(index) =
        (1..rows.len()).find(|i| rows[*i - 1].occurrence_id >= rows[*i].occurrence_id)
    {
        return Err(VectorRefusal::RowOrder { index });
    }
    let tombstones = &export.tombstones;
    if let Some(index) = (1..tombstones.len()).find(|i| tombstones[*i - 1] >= tombstones[*i]) {
        return Err(VectorRefusal::TombstoneOrder { index });
    }
    if let Some(index) = rows
        .iter()
        .position(|row| tombstones.binary_search(&row.occurrence_id).is_ok())
    {
        return Err(VectorRefusal::ListedAndTombstoned { index });
    }
    if export.checkpoint.snapshot_commit_seq > export.checkpoint.checkpoint_commit_seq {
        return Err(VectorRefusal::Identity {
            field: "checkpoint",
        });
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
    let codes =
        encode_all(&layout, &calibration.scales, vectors).map_err(|(index, rejection)| {
            VectorRefusal::Rows(codec::ArtifactRejection::Row { index, rejection })
        })?;
    let ids: Vec<&str> = rows.iter().map(|row| row.occurrence_id.as_str()).collect();
    let payloads = [
        rows_bytes,
        codes,
        calibration.scales.encode(),
        serde_json::to_vec(&ids).expect("identifier serialization cannot fail"),
        serde_json::to_vec(tombstones).expect("identifier serialization cannot fail"),
    ];
    let mut inventory = Vec::with_capacity(payloads.len());
    for (path, bytes) in PAYLOAD_FILES.iter().zip(&payloads) {
        write_new(&work_dir.join(path), bytes)?;
        inventory.push(SidecarFile {
            path: (*path).to_owned(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        });
    }
    let sidecar = VectorSidecar {
        schema: SIDECAR_SCHEMA,
        embedding_model: expected.generation.embedding_model.clone(),
        tokenizer_fingerprint: expected.generation.tokenizer_fingerprint.clone(),
        vector_dimension: layout.dimension,
        metric: expected.metric.name().to_owned(),
        unit_norm_tolerance: layout.unit_norm_tolerance,
        quantizer_recipe: expected.recipe.id().to_owned(),
        calibrated_rows: calibration.identity.calibrated_rows,
        scales_sha256: sha256_hex(&calibration.scales.encode()),
        generation_id: expected.generation.generation_id.clone(),
        generation_epoch: expected.generation.generation_epoch,
        kernel_incarnation_id: expected.kernel_incarnation_id.to_owned(),
        snapshot_commit_seq: export.checkpoint.snapshot_commit_seq,
        checkpoint_commit_seq: export.checkpoint.checkpoint_commit_seq,
        hold_id: export.checkpoint.hold_id.clone(),
        rows: rows.len() as u64,
        tombstones: tombstones.len() as u64,
        files: inventory,
    };
    write_new(&work_dir.join(SIDECAR_FILE), &sidecar.canonical_bytes())?;
    Ok(BuiltVectors {
        sidecar,
        dir: work_dir.to_path_buf(),
    })
}

/// Everything a stager needs from the lifecycle and the admission gate. `transaction` is the caller's exclusive hold on the store's transaction lock; hold it until the digest is pinned or protected.
pub struct Staging<'a> {
    pub store: &'a GenerationStore,
    pub transaction: &'a LifecycleTransactionLock,
    pub gate: &'a HookGate,
    pub admission: &'a Admission,
    pub identity: &'a ProjectionIdentity,
    /// Digests a corrupt same-digest target may never be exchange-repaired over.
    pub protected: &'a BTreeSet<String>,
}

impl Staging<'_> {
    /// Charges the manifest's whole inventory against the staged-bytes limit, then stages the files `resolve` names for each manifest path; a refused admission stages nothing.
    ///
    /// # Errors
    ///
    /// An admission denial or the store's refusal.
    pub(crate) fn stage_manifest(
        &self,
        manifest: &GenerationManifest,
        meta: &StageMeta,
        resolve: impl Fn(&str) -> PathBuf,
    ) -> Result<String, VectorRefusal> {
        let bytes: u64 = manifest.files.iter().map(|file| file.size).sum();
        self.gate
            .check_limits(
                self.admission,
                &InvalidationIdentity::from(self.identity),
                &[(STAGE_DISK_LIMIT, bytes)],
            )
            .map_err(VectorRefusal::Admission)?;
        let sources = manifest_sources(manifest, |path| Some(resolve(path)))
            .expect("every manifest path resolves under the work directory");
        let digest = self.store.stage(&sources, meta, self.protected)?;
        // The store checked every source against the manifest's size and hash, so the digest it returns is the manifest's.
        debug_assert_eq!(digest, manifest.digest());
        Ok(digest)
    }
}

/// Stages a build through `staging`.
///
/// # Errors
///
/// An admission denial or the store's refusal.
pub fn stage(built: &BuiltVectors, staging: &Staging<'_>) -> Result<String, VectorRefusal> {
    staging.stage_manifest(
        &built.sidecar.stage_manifest(),
        &built.sidecar.stage_meta(),
        |path| built.dir.join(path),
    )
}

/// A generation whose files, sidecar, and meaning were all checked. The row and code artifacts stay open on the descriptors verification read them through, and the tables it decoded stay with it, so a reader takes the files as verified without opening or hashing them again.
pub struct VerifiedVectors {
    pub digest: String,
    pub sidecar: VectorSidecar,
    /// Retains the directory descriptor and, once pinned, the shared lock.
    pub generation: ValidatedGeneration,
    pub rows: File,
    pub codes: File,
    pub scales: Scales,
    pub occurrence_ids: Vec<String>,
    pub tombstones: Vec<String>,
}

impl std::fmt::Debug for VerifiedVectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedVectors")
            .field("digest", &self.digest)
            .field("sidecar", &self.sidecar)
            .finish_non_exhaustive()
    }
}

/// Verifies `digest` independently of its manifest: the store checks inventory, sizes, modes, and hashes; this checks that the manifest is a vector manifest bound to a canonical sidecar, that the sidecar carries `expected`, and that the rows, scales, codes, and identifiers agree with one another under the recipe: the scales are the calibration of the rows, and the codes are the rows encoded under them.
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
    let sidecar_bytes = generation.read_verified_file(SIDECAR_FILE)?;
    let sidecar: VectorSidecar =
        serde_json::from_slice(&sidecar_bytes).map_err(|_| VectorRefusal::NotVectors("sidecar"))?;
    if sidecar.schema != SIDECAR_SCHEMA {
        return Err(VectorRefusal::NotVectors("sidecar schema"));
    }
    if sidecar.canonical_bytes() != sidecar_bytes {
        return Err(VectorRefusal::NotVectors("sidecar not canonical"));
    }
    if !sidecar.inventories_exactly() {
        return Err(VectorRefusal::NotVectors("inventory"));
    }
    if sidecar.stage_manifest() != *manifest {
        return Err(VectorRefusal::NotVectors("manifest binding"));
    }
    check_identity(&sidecar, expected)?;
    let layout = sidecar
        .layout()
        .ok_or(VectorRefusal::NotVectors("metric"))?;
    let rows_file = File::from(generation.open_verified_file(ROWS_FILE)?);
    let rows = codec::decode_rows(&read_all(&rows_file)?, &layout).map_err(VectorRefusal::Rows)?;
    let fault = |path, fault| VectorRefusal::File { path, fault };
    if rows.rows.len() as u64 != sidecar.rows {
        return Err(fault(ROWS_FILE, FileFault::RowCount));
    }
    let vectors = rows.rows.iter().map(Vec::as_slice);
    let calibration =
        scalar::calibrate(&layout, vectors.clone()).map_err(VectorRefusal::Calibration)?;
    let scales_bytes = generation.read_verified_file(SCALES_FILE)?;
    if calibration.scales.encode() != scales_bytes
        || calibration.identity.calibrated_rows != sidecar.calibrated_rows
        || sha256_hex(&scales_bytes) != sidecar.scales_sha256
    {
        return Err(fault(SCALES_FILE, FileFault::Calibration));
    }
    let ids: Vec<String> = serde_json::from_slice(&generation.read_verified_file(ROW_IDS_FILE)?)
        .map_err(|_| fault(ROW_IDS_FILE, FileFault::Identifiers))?;
    if ids.len() != rows.rows.len() || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(fault(ROW_IDS_FILE, FileFault::Identifiers));
    }
    let tombstones: Vec<String> =
        serde_json::from_slice(&generation.read_verified_file(TOMBSTONES_FILE)?)
            .map_err(|_| fault(TOMBSTONES_FILE, FileFault::Identifiers))?;
    if tombstones.len() as u64 != sidecar.tombstones
        || tombstones.windows(2).any(|pair| pair[0] >= pair[1])
        || ids.iter().any(|id| tombstones.binary_search(id).is_ok())
    {
        return Err(fault(TOMBSTONES_FILE, FileFault::Identifiers));
    }
    let codes = encode_all(&layout, &calibration.scales, vectors)
        .map_err(|_| fault(CODES_FILE, FileFault::Codes))?;
    let codes_file = File::from(generation.open_verified_file(CODES_FILE)?);
    if read_all(&codes_file)? != codes {
        return Err(fault(CODES_FILE, FileFault::Codes));
    }
    Ok(VerifiedVectors {
        digest: digest.to_owned(),
        sidecar,
        generation,
        rows: rows_file,
        codes: codes_file,
        scales: calibration.scales,
        occurrence_ids: ids,
        tombstones,
    })
}

/// The whole of a verified file, read from its start whatever the descriptor's position.
fn read_all(file: &File) -> Result<Vec<u8>, VectorRefusal> {
    use std::io::Read;
    let mut bytes = Vec::new();
    let mut reader = file
        .try_clone()
        .map_err(|error| VectorRefusal::Io(error.kind().to_string()))?;
    std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(0))
        .map_err(|error| VectorRefusal::Io(error.kind().to_string()))?;
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| VectorRefusal::Io(error.kind().to_string()))?;
    Ok(bytes)
}

/// Every field is compared, so a different model space is refused even at an equal dimension.
pub(crate) fn check_identity(
    sidecar: &VectorSidecar,
    expected: &ExpectedVectors<'_>,
) -> Result<(), VectorRefusal> {
    let generation = expected.generation;
    let checks = [
        (
            "embedding_model",
            sidecar.embedding_model == generation.embedding_model,
        ),
        (
            "tokenizer_fingerprint",
            sidecar.tokenizer_fingerprint == generation.tokenizer_fingerprint,
        ),
        (
            "vector_dimension",
            sidecar.vector_dimension == generation.vector_dimension,
        ),
        ("metric", sidecar.metric == expected.metric.name()),
        (
            "unit_norm_tolerance",
            sidecar.unit_norm_tolerance.to_bits() == expected.unit_norm_tolerance.to_bits(),
        ),
        (
            "quantizer_recipe",
            sidecar.quantizer_recipe == expected.recipe.id(),
        ),
        (
            "generation_epoch",
            sidecar.generation_epoch == generation.generation_epoch,
        ),
        (
            "kernel_incarnation_id",
            sidecar.kernel_incarnation_id == expected.kernel_incarnation_id,
        ),
        (
            "generation_id",
            sidecar.generation_id == generation.generation_id,
        ),
    ];
    if let Some((field, _)) = checks.into_iter().find(|(_, holds)| !holds) {
        return Err(VectorRefusal::Identity { field });
    }
    if expected.checkpoint.is_some_and(|checkpoint| {
        sidecar.snapshot_commit_seq != checkpoint.snapshot_commit_seq
            || sidecar.checkpoint_commit_seq != checkpoint.checkpoint_commit_seq
            || sidecar.hold_id != checkpoint.hold_id
    }) {
        return Err(VectorRefusal::Identity {
            field: "checkpoint",
        });
    }
    if sidecar.snapshot_commit_seq > sidecar.checkpoint_commit_seq {
        return Err(VectorRefusal::Identity {
            field: "checkpoint",
        });
    }
    Ok(())
}

/// The codes of every row, concatenated in row order; the first row the recipe refuses is reported with its index.
fn encode_all<'a>(
    layout: &RowLayout,
    scales: &Scales,
    rows: impl Iterator<Item = &'a [f32]>,
) -> Result<Vec<u8>, (usize, codec::RowRejection)> {
    let mut codes = Vec::new();
    for (index, row) in rows.enumerate() {
        let encoded =
            scalar::encode(layout, scales, row).map_err(|rejection| (index, rejection))?;
        codes.extend(scalar::encode_codes(&encoded.codes));
    }
    Ok(codes)
}

/// `O_EXCL` so a build never overwrites a file another build left behind.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), VectorRefusal> {
    let io = |error: io::Error| VectorRefusal::Io(error.kind().to_string());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::CLOEXEC).bits() as i32)
        .open(path)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
