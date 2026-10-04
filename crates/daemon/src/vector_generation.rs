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
use std::sync::Arc;

use host_runtime::generation::{
    FILE_HASH_BUFFER_BYTES, GenerationError, GenerationManifest, GenerationStore, ManifestFile,
    RETAINED_FILE_BYTES, StageMeta, StreamedFile, ValidatedGeneration,
};
use host_runtime::lifecycle::{LifecycleTransactionLock, PAYLOAD_MANIFEST_DIGEST_LEN};
use retrieval::ProjectionIdentity;
use retrieval::batch::{ProjectionCheckpoint, VectorGeneration};
use retrieval::dense::codec::{self, Metric, RowLayout};
use retrieval::dense::export::LiveRows;
use retrieval::dense::scalar::{self, ScalarRecipe, Scales};
use rustix::fs::OFlags;
use sha2::{Digest, Sha256};

use crate::projection_gates::{Admission, Denial, HookGate, InvalidationIdentity};
use crate::search_seed::manifest_sources;
use crate::vector_admission::{Ledger, ResourceClass};

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
        exact_json(self)
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
        self.meta_with(self.sha256())
    }

    /// [`Self::stage_meta`] with the sidecar's hash already taken, so the sidecar is not serialized for it.
    fn meta_with(&self, sidecar_sha256: String) -> StageMeta {
        StageMeta {
            target: VECTOR_TARGET.to_owned(),
            release_contract_sha256: compatibility_sha256(&(
                self.embedding_model.as_str(),
                self.tokenizer_fingerprint.as_str(),
                self.vector_dimension,
                self.metric.as_str(),
                self.unit_norm_tolerance.to_bits(),
                self.quantizer_recipe.as_str(),
                self.generation_epoch,
            )),
            inputs_lock_sha256: sidecar_sha256,
            source_payload_manifest_sha256: self
                .files
                .iter()
                .find(|file| file.path == ROWS_FILE)
                .map(|file| file.sha256.clone())
                .unwrap_or_default(),
        }
    }

    pub fn stage_manifest(&self) -> GenerationManifest {
        let (size, sha256) = {
            let sidecar = self.canonical_bytes();
            (sidecar.len() as u64, sha256_hex(&sidecar))
        };
        self.manifest_with(size, sha256)
    }

    /// [`Self::stage_manifest`] from the size and hash of the sidecar's canonical bytes, so a caller that holds or has hashed them serializes nothing.
    fn manifest_with(&self, size: u64, sha256: String) -> GenerationManifest {
        let mut files = Vec::with_capacity(self.files.len() + 1);
        files.extend(self.files.iter().map(|file| ManifestFile {
            path: file.path.clone(),
            mode: 0o600,
            size: file.size,
            sha256: file.sha256.clone(),
        }));
        files.push(ManifestFile {
            path: SIDECAR_FILE.to_owned(),
            mode: 0o600,
            size,
            sha256: sha256.clone(),
        });
        GenerationManifest::from_files(&self.meta_with(sha256), files)
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
        compatibility_sha256(&(
            self.embedding_model.as_str(),
            self.tokenizer_fingerprint.as_str(),
            self.vector_dimension,
            self.metric.as_str(),
            self.unit_norm_tolerance_bits,
            self.quantizer_recipe.as_str(),
            self.generation_epoch,
        ))
    }
}

/// Tuple order defines the hashed JSON array and forms part of the compatibility contract.
type CompatibilityFields<'a> = (&'a str, &'a str, u32, &'a str, u64, &'a str, u64);

fn compatibility_sha256(fields: &CompatibilityFields<'_>) -> String {
    let mut hasher = Sha256::new();
    serde_json::to_writer(&mut hasher, fields).expect("identity serialization cannot fail");
    format!("{:x}", hasher.finalize())
}

/// What a caller expects a generation to carry, compared with the sidecar field by field.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedVectors<'a> {
    pub generation: &'a VectorGeneration,
    pub kernel_incarnation_id: &'a str,
    pub metric: Metric,
    pub unit_norm_tolerance: f64,
    pub recipe: ScalarRecipe,
    /// The checkpoint the rows were read at; `None` accepts any checkpoint the projection schema could hold and reads it from the sidecar.
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
    #[error("the export and the expectation disagree on {field}")]
    Export { field: &'static str },
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
    #[error("reservation: {0}")]
    Reservation(crate::vector_admission::Refusal),
    #[error("the lifecycle store refused the generation: {0}")]
    Store(String),
    #[error("the generation carries a state schema this build does not know")]
    Quarantined,
    #[error("the generation's {bytes} payload bytes exceed the verification bound of {max}")]
    OverBound { bytes: u64, max: u64 },
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

/// Writes rows, codes, scales, identifiers, and tombstones under `work_dir` and describes them in a sidecar bound to `expected` and the export's generation, kernel incarnation, and checkpoint.
/// The export names the generation, kernel, and checkpoint it read the rows under; a disagreement with `expected` is refused rather than stamped, so the sidecar's provenance is the rows' provenance.
/// Rows and tombstones must arrive in strictly increasing identifier order, and no occurrence may be both, so the artifact, the codes, the identifier list, and the resolver agree on what the layer says across builds.
/// Files are created exclusively and never synced: the store copies and syncs them when it stages, so the work directory is scratch, and a retry needs a fresh one.
///
/// # Errors
///
/// An export of another generation, kernel, or named checkpoint, no rows, rows or tombstones out of order, an occurrence both listed and tombstoned, a row outside the layout, a calibration refusal, or an I/O failure; nothing is staged.
pub fn build(
    expected: &ExpectedVectors<'_>,
    export: &LiveRows,
    work_dir: &Path,
) -> Result<BuiltVectors, VectorRefusal> {
    check_export(export, expected)?;
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
        exact_json(&ids),
        exact_json(tombstones),
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
    let sidecar = sidecar(
        expected,
        &export.checkpoint,
        SidecarCounts {
            rows: rows.len() as u64,
            tombstones: tombstones.len() as u64,
            calibrated_rows: calibration.identity.calibrated_rows,
        },
        sha256_hex(&calibration.scales.encode()),
        inventory,
    );
    write_new(&work_dir.join(SIDECAR_FILE), &sidecar.canonical_bytes())?;
    Ok(BuiltVectors {
        sidecar,
        dir: work_dir.to_path_buf(),
    })
}

struct SidecarCounts {
    rows: u64,
    tombstones: u64,
    calibrated_rows: u64,
}

fn sidecar(
    expected: &ExpectedVectors<'_>,
    checkpoint: &ProjectionCheckpoint,
    counts: SidecarCounts,
    scales_sha256: String,
    files: Vec<SidecarFile>,
) -> VectorSidecar {
    VectorSidecar {
        schema: SIDECAR_SCHEMA,
        embedding_model: expected.generation.embedding_model.clone(),
        tokenizer_fingerprint: expected.generation.tokenizer_fingerprint.clone(),
        vector_dimension: expected.generation.vector_dimension,
        metric: expected.metric.name().to_owned(),
        unit_norm_tolerance: expected.unit_norm_tolerance,
        quantizer_recipe: expected.recipe.id().to_owned(),
        calibrated_rows: counts.calibrated_rows,
        scales_sha256,
        generation_id: expected.generation.generation_id.clone(),
        generation_epoch: expected.generation.generation_epoch,
        kernel_incarnation_id: expected.kernel_incarnation_id.to_owned(),
        snapshot_commit_seq: checkpoint.snapshot_commit_seq,
        checkpoint_commit_seq: checkpoint.checkpoint_commit_seq,
        hold_id: checkpoint.hold_id.clone(),
        rows: counts.rows,
        tombstones: counts.tombstones,
        files,
    }
}

/// `resident` is a build's peak heap use; `disk` is the total bytes it writes, including the sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildFootprint {
    pub resident: u64,
    pub disk: u64,
}

/// Estimates resources before any row is decoded. `disk` matches the emitted files: identifiers are serialized and the sidecar is sized with 64-byte hash placeholders. `resident` includes the decoded rows and every artifact the build retains at once.
pub fn footprint<'a>(
    expected: &ExpectedVectors<'_>,
    checkpoint: &ProjectionCheckpoint,
    ids: impl IntoIterator<Item = &'a str>,
    tombstones: impl IntoIterator<Item = &'a str>,
) -> BuildFootprint {
    let dimension = u64::from(expected.generation.vector_dimension);
    let (rows, ids_file) = json_list_bytes(ids);
    let (tombstone_count, tombstones_file) = json_list_bytes(tombstones);
    let row_bytes = rows.saturating_mul(dimension).saturating_mul(4);
    let rows_file = row_bytes.saturating_add(codec::ARTIFACT_HEADER_BYTES as u64);
    let codes_file = rows.saturating_mul(dimension);
    let scales_file = dimension.saturating_mul(4);
    let placeholder = || "0".repeat(64);
    let files = PAYLOAD_FILES
        .iter()
        .zip([
            rows_file,
            codes_file,
            scales_file,
            ids_file,
            tombstones_file,
        ])
        .map(|(path, size)| SidecarFile {
            path: (*path).to_owned(),
            size,
            sha256: placeholder(),
        })
        .collect();
    let sidecar_file = sidecar(
        expected,
        checkpoint,
        SidecarCounts {
            rows,
            tombstones: tombstone_count,
            calibrated_rows: rows,
        },
        placeholder(),
        files,
    )
    .canonical_bytes()
    .len() as u64;
    let payloads = [
        rows_file,
        codes_file,
        scales_file,
        ids_file,
        tombstones_file,
    ]
    .into_iter()
    .fold(0u64, u64::saturating_add);
    // Peak resident memory also holds the calibration's running maxima, the calibration's scales, one borrowed identifier reference per serialized identifier, and the sidecar itself beside its serialized bytes: every string it owns appears in them, so the file's size bounds the struct's heap.
    let resident = row_bytes
        .saturating_add(payloads)
        .saturating_add(scales_file.saturating_mul(2))
        .saturating_add(sidecar_file.saturating_mul(2))
        .saturating_add(rows.saturating_mul(std::mem::size_of::<&str>() as u64));
    BuildFootprint {
        resident,
        disk: payloads.saturating_add(sidecar_file),
    }
}

struct Counting(u64);

impl Write for Counting {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serializes into a buffer of exactly the output's length, so the heap holds one copy of the JSON rather than a growing buffer's old and new halves.
fn exact_json<T: serde::Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut counted = Counting(0);
    serde_json::to_writer(&mut counted, value).expect("JSON serialization cannot fail");
    let mut bytes = Vec::with_capacity(usize::try_from(counted.0).expect("a counted length fits"));
    serde_json::to_writer(&mut bytes, value).expect("JSON serialization cannot fail");
    debug_assert_eq!(bytes.len() as u64, counted.0);
    bytes
}

/// Checks serialized output incrementally against an existing encoding, keeping comparison state constant-sized.
struct Matching<'a> {
    rest: &'a [u8],
    matches: bool,
}

impl Write for Matching<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.rest.strip_prefix(bytes) {
            Some(rest) if self.matches => self.rest = rest,
            _ => self.matches = false,
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Whether `value`'s JSON is exactly `bytes`, the encoding [`exact_json`] writes.
fn is_exact_json<T: serde::Serialize + ?Sized>(value: &T, bytes: &[u8]) -> bool {
    let mut matching = Matching {
        rest: bytes,
        matches: true,
    };
    serde_json::to_writer(&mut matching, value).expect("JSON serialization cannot fail");
    matching.matches && matching.rest.is_empty()
}

fn json_list_bytes<'a>(items: impl IntoIterator<Item = &'a str>) -> (u64, u64) {
    let mut count = 0u64;
    let mut bytes = Counting(1);
    for item in items {
        if count > 0 {
            bytes.0 = bytes.0.saturating_add(1);
        }
        serde_json::to_writer(&mut bytes, item).expect("identifier serialization cannot fail");
        count += 1;
    }
    (count, bytes.0.saturating_add(1))
}

/// Build cleanup unlinks the build's files under `dir` and removes `dir`; it leaves `dir` standing when other entries remain.
///
/// # Errors
///
/// Cleanup cannot unlink a file the build wrote or cannot remove the directory.
pub(crate) fn remove_build(dir: &Path) -> io::Result<()> {
    let mut first = None;
    for name in PAYLOAD_FILES.iter().chain(std::iter::once(&SIDECAR_FILE)) {
        if let Err(error) = fs::remove_file(dir.join(name))
            && error.kind() != io::ErrorKind::NotFound
            && first.is_none()
        {
            first = Some(error);
        }
    }
    match first {
        Some(error) => Err(error),
        None => fs::remove_dir(dir),
    }
}

/// Everything a stager needs from the lifecycle, the admission gate, and the ledger. `transaction` is the caller's exclusive hold on the store's transaction lock; hold it until the digest is pinned or protected.
pub struct Staging<'a> {
    pub store: &'a GenerationStore,
    pub transaction: &'a LifecycleTransactionLock,
    pub gate: &'a HookGate,
    pub admission: &'a Admission,
    pub identity: &'a ProjectionIdentity,
    pub ledger: &'a Arc<Ledger>,
    /// Digests a corrupt same-digest target may never be exchange-repaired over.
    pub protected: &'a BTreeSet<String>,
}

impl Staging<'_> {
    /// Charges the manifest's whole inventory against the staged-bytes limit, reserves it plus the `manifest.json` the store writes beside it in the ledger's disk pool on top of what the store already holds, then stages the files `resolve` names for each manifest path; a refused admission or reservation stages nothing. The reservation ends with the copy: the bytes then belong to the store, which the next disk reservation counts. A manifest the store already holds is reserved the same way, because the store copies the inventory into a staging temp before it finds the occupant and publishes nothing twice.
    ///
    /// # Errors
    ///
    /// An admission denial, a refused reservation, or the store's refusal.
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
        // The store's walk counts `manifest.json`, so the reservation covers it or an exact-bound staging would leave the store over the limit.
        let on_disk = bytes
            .checked_add(manifest.canonical_bytes().len() as u64)
            .ok_or(VectorRefusal::Reservation(
                crate::vector_admission::Refusal::Overflow {
                    pool: crate::vector_admission::Pool::Disk,
                },
            ))?;
        let _staging = self
            .ledger
            .reserve_disk(self.admission, ResourceClass::Staging, on_disk, self.store)
            .map_err(VectorRefusal::Reservation)?;
        let sources = manifest_sources(manifest, |path| Some(resolve(path)))
            .expect("every manifest path resolves under the work directory");
        let digest = self.store.stage(&sources, meta, self.protected)?;
        // The store checked every source against the manifest's size and hash, so the digest it returns is the manifest's.
        debug_assert_eq!(digest, manifest.digest());
        Ok(digest)
    }
}

/// Stages a build through `staging`.
/// The build's provenance must be the identity the admission is bound to: a sidecar for another model, tokenizer, dimension, epoch, or kernel incarnation is refused before the gate is consulted, so no generation is published under another identity's evidence and none is published that `verify` under this identity would refuse.
///
/// # Errors
///
/// A build of another identity, an admission denial, or the store's refusal.
pub fn stage(built: &BuiltVectors, staging: &Staging<'_>) -> Result<String, VectorRefusal> {
    let sidecar = &built.sidecar;
    let identity = staging.identity;
    let field = |field| VectorRefusal::Identity { field };
    if sidecar.embedding_model != identity.embedding_model {
        return Err(field("embedding_model"));
    }
    if sidecar.tokenizer_fingerprint != identity.tokenizer_fingerprint {
        return Err(field("tokenizer_fingerprint"));
    }
    if sidecar.vector_dimension != identity.vector_dimension {
        return Err(field("vector_dimension"));
    }
    if sidecar.generation_epoch != identity.generation_epoch {
        return Err(field("generation_epoch"));
    }
    if sidecar.kernel_incarnation_id != identity.kernel_incarnation_id {
        return Err(field("kernel_incarnation_id"));
    }
    staging.stage_manifest(&sidecar.stage_manifest(), &sidecar.stage_meta(), |path| {
        built.dir.join(path)
    })
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

/// The files whose decoded contents [`VerifiedVectors`] keeps in memory: `occurrence_ids`, `tombstones`, `scales`, and `sidecar`. Rows and codes stay behind their descriptors.
pub const RESIDENT_FILES: [&str; 4] = [ROW_IDS_FILE, TOMBSTONES_FILE, SCALES_FILE, SIDECAR_FILE];

/// Returns the manifest-declared bytes of the [`RESIDENT_FILES`] plus the squared weights the decoded scales keep beside them, an f64 for every four-byte scale: the charge for one generation's decoded tables.
pub fn resident_bytes(manifest: &GenerationManifest) -> u64 {
    manifest
        .files
        .iter()
        .filter(|file| RESIDENT_FILES.contains(&file.path.as_str()))
        .fold(0u64, |total, file| {
            let weights = if file.path == SCALES_FILE {
                file.size.saturating_mul(2)
            } else {
                0
            };
            total.saturating_add(file.size).saturating_add(weights)
        })
}

/// Verifies `digest` independently of its manifest: the store checks inventory, sizes, modes, and hashes; this checks that the manifest is a vector manifest bound to a canonical sidecar, that the sidecar carries `expected`, and that the rows, scales, codes, and identifiers agree with one another under the recipe: the scales are the calibration of the rows, and the codes are the rows encoded under them.
/// Verification streams the row and code artifacts one chunk of rows at a time in one pass that calibrates the rows and compares the stored codes with the rows encoded under the stored scales, then requires the stored scales to be the calibration, and keeps only the resident tables. [`verification_bytes`] bounds the heap it holds at once; a generation over `max_bytes` is refused before the sidecar is read on the manifest's share of the bound, and before any table or payload is read on the whole of it. The store checks each file's inventory entry, mode, and size, and verification reads each file once through a stream that hashes the bytes it returns; the store finishes every hash before any refusal about meaning is returned, so a file whose bytes diverge from the manifest refuses as the store refuses it, and the bound limits memory, not I/O.
///
/// # Errors
///
/// Any disagreement refuses; nothing is selected or pinned.
pub fn verify(
    store: &GenerationStore,
    digest: &str,
    expected: &ExpectedVectors<'_>,
    max_bytes: u64,
) -> Result<VerifiedVectors, VectorRefusal> {
    let mut pending = store.validate_streaming(digest, &VERIFIED_FILES)?;
    let mut streams = VERIFIED_FILES.map(|path| pending.stream(path));
    let meaning = check_meaning(pending.manifest(), &mut streams, expected, max_bytes);
    // Bytes that diverge from the manifest refuse as the store refuses them, ahead of any refusal about their meaning.
    let mut generation = pending.finish(streams.into_iter().flatten())?;
    let tables = meaning?;
    Ok(VerifiedVectors {
        digest: digest.to_owned(),
        sidecar: tables.sidecar,
        rows: File::from(generation.take_verified_file(ROWS_FILE)?),
        codes: File::from(generation.take_verified_file(CODES_FILE)?),
        generation,
        scales: tables.scales,
        occurrence_ids: tables.occurrence_ids,
        tombstones: tables.tombstones,
    })
}

/// The files [`verify`] reads, each through the stream that hashes it as it is read, so the bytes checked are the bytes hashed.
const VERIFIED_FILES: [&str; 6] = [
    SIDECAR_FILE,
    ROWS_FILE,
    CODES_FILE,
    SCALES_FILE,
    ROW_IDS_FILE,
    TOMBSTONES_FILE,
];

/// What [`check_meaning`] decodes and [`VerifiedVectors`] keeps.
struct Tables {
    sidecar: VectorSidecar,
    scales: Scales,
    occurrence_ids: Vec<String>,
    tombstones: Vec<String>,
}

/// A file's stream, or the store's refusal when the manifest names no such file.
fn opened(
    stream: &mut Result<StreamedFile, GenerationError>,
) -> Result<&mut StreamedFile, VectorRefusal> {
    stream
        .as_mut()
        .map_err(|error| VectorRefusal::Store(error.to_string()))
}

/// The checks of [`verify`] past the store's, reading [`VERIFIED_FILES`] through `streams` in that order.
fn check_meaning(
    manifest: &GenerationManifest,
    streams: &mut [Result<StreamedFile, GenerationError>; 6],
    expected: &ExpectedVectors<'_>,
    max_bytes: u64,
) -> Result<Tables, VectorRefusal> {
    let [
        sidecar_file,
        rows_file,
        codes_file,
        scales_file,
        ids_file,
        tombstones_file,
    ] = streams.each_mut();
    if manifest.target != VECTOR_TARGET {
        return Err(VectorRefusal::NotVectors("manifest target"));
    }
    let within = |bytes: u64| {
        if bytes > max_bytes {
            Err(VectorRefusal::OverBound {
                bytes,
                max: max_bytes,
            })
        } else {
            Ok(())
        }
    };
    within(manifest_verification_bytes(manifest))?;
    let sidecar_bytes = opened(sidecar_file)?.read_rest()?;
    let sidecar: VectorSidecar =
        serde_json::from_slice(&sidecar_bytes).map_err(|_| VectorRefusal::NotVectors("sidecar"))?;
    if sidecar.schema != SIDECAR_SCHEMA {
        return Err(VectorRefusal::NotVectors("sidecar schema"));
    }
    if !is_exact_json(&sidecar, &sidecar_bytes) {
        return Err(VectorRefusal::NotVectors("sidecar not canonical"));
    }
    // The binding needs only the canonical bytes' size and hash, so the bytes go before the bound manifest is built.
    let (sidecar_size, sidecar_sha256) = (sidecar_bytes.len() as u64, sha256_hex(&sidecar_bytes));
    drop(sidecar_bytes);
    if !sidecar.inventories_exactly() {
        return Err(VectorRefusal::NotVectors("inventory"));
    }
    if sidecar.manifest_with(sidecar_size, sidecar_sha256) != *manifest {
        return Err(VectorRefusal::NotVectors("manifest binding"));
    }
    // The projection schema holds no checkpoint without these, so a sidecar naming one came from no export.
    if sidecar.snapshot_commit_seq < 0
        || sidecar.checkpoint_commit_seq < sidecar.snapshot_commit_seq
        || sidecar.hold_id.is_empty()
    {
        return Err(VectorRefusal::NotVectors("checkpoint"));
    }
    check_identity(&sidecar, expected)?;
    let layout = RowLayout {
        dimension: sidecar.vector_dimension,
        metric: expected.metric,
        unit_norm_tolerance: sidecar.unit_norm_tolerance,
    };
    within(verification_bytes(manifest, &sidecar))?;
    let fault = |path, fault| VectorRefusal::File { path, fault };
    let rows_file = opened(rows_file)?;
    let row_count = open_rows(rows_file, &layout)?;
    if row_count != sidecar.rows {
        return Err(fault(ROWS_FILE, FileFault::RowCount));
    }
    let dimension = layout.dimension as usize;
    // Codes encoded under the stored scales are the codes encoded under the calibration whenever the two scales are equal, which is checked before any code refusal, so one pass over the rows both calibrates and compares the codes.
    // Each refusal about the scales or codes files is held until every check that precedes it in refusal order has passed.
    let scales_bytes = opened(scales_file).and_then(|file| Ok(file.read_rest()?));
    let stored_scales = scales_bytes
        .as_ref()
        .ok()
        .and_then(|bytes| Scales::decode(bytes, layout.dimension).ok());
    let codes_file = opened(codes_file)?;
    let mut codes_verdict = if row_count.checked_mul(dimension as u64) == Some(codes_file.size()) {
        Ok(())
    } else {
        Err(fault(CODES_FILE, FileFault::Codes))
    };
    let encoder = stored_scales.as_ref().map(scalar::Encoder::new);
    let chunk_rows =
        (row_chunk_bytes(layout.dimension) / codec::row_bytes(layout.dimension)) as usize;
    let mut stored = vec![0u8; chunk_rows * dimension];
    let mut calibrator = scalar::Calibrator::new(&layout).map_err(VectorRefusal::Calibration)?;
    // The decoder validates each row under the layout before it reaches the calibrator or the encoder; the stored codes of each row chunk are read as the chunk starts.
    for_each_row(rows_file, &layout, row_count, |index, row| {
        calibrator.push_validated(row);
        let (Some(encoder), Ok(())) = (&encoder, &codes_verdict) else {
            return Ok(());
        };
        let slot = index % chunk_rows;
        if slot == 0 {
            let chunk = (row_count - index as u64).min(chunk_rows as u64) as usize;
            if let Err(error) = codes_file.read_exact(&mut stored[..chunk * dimension]) {
                codes_verdict = Err(io_refusal(error));
                return Ok(());
            }
        }
        if !encoder.matches(
            &layout,
            row,
            &stored[slot * dimension..(slot + 1) * dimension],
        ) {
            codes_verdict = Err(fault(CODES_FILE, FileFault::Codes));
        }
        Ok(())
    })?;
    drop((stored, encoder));
    drop(stored_scales);
    let calibration = calibrator.finish().map_err(VectorRefusal::Calibration)?;
    let scales_bytes = scales_bytes?;
    if calibration.scales.encode() != scales_bytes
        || calibration.identity.calibrated_rows != sidecar.calibrated_rows
        || sha256_hex(&scales_bytes) != sidecar.scales_sha256
    {
        return Err(fault(SCALES_FILE, FileFault::Calibration));
    }
    drop(scales_bytes);
    let ids = decode_list(&opened(ids_file)?.read_rest()?, row_count)
        .ok_or_else(|| fault(ROW_IDS_FILE, FileFault::Identifiers))?;
    if ids.len() as u64 != row_count || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(fault(ROW_IDS_FILE, FileFault::Identifiers));
    }
    let tombstones = decode_list(&opened(tombstones_file)?.read_rest()?, sidecar.tombstones)
        .ok_or_else(|| fault(TOMBSTONES_FILE, FileFault::Identifiers))?;
    if tombstones.len() as u64 != sidecar.tombstones
        || tombstones.windows(2).any(|pair| pair[0] >= pair[1])
        || share_an_entry(&ids, &tombstones)
    {
        return Err(fault(TOMBSTONES_FILE, FileFault::Identifiers));
    }
    codes_verdict?;
    Ok(Tables {
        sidecar,
        scales: calibration.scales,
        occurrence_ids: ids,
        tombstones,
    })
}

/// Bytes of one streamed row chunk: whole rows of `dimension` f32 coordinates filling [`VERIFY_CHUNK_BYTES`], at least one row.
fn row_chunk_bytes(dimension: u32) -> u64 {
    let row = codec::row_bytes(dimension);
    (VERIFY_CHUNK_BYTES / row.max(1)).max(1) * row
}

fn manifest_heap(manifest: &GenerationManifest) -> u64 {
    let strings = [
        &manifest.target,
        &manifest.release_contract_sha256,
        &manifest.inputs_lock_sha256,
    ]
    .into_iter()
    .chain(&manifest.source_payload_manifest_sha256)
    .chain(
        manifest
            .files
            .iter()
            .flat_map(|file| [&file.path, &file.sha256]),
    )
    .map(String::capacity)
    .sum::<usize>();
    (strings + manifest.files.capacity() * size_of::<ManifestFile>()) as u64
}

/// Verification retains the resident tables, the validated manifest, one descriptor slot per manifest file, and two manifest digests: one in the validated generation and one in the result.
fn kept_bytes(manifest: &GenerationManifest) -> u64 {
    let slots = manifest.files.len() * RETAINED_FILE_BYTES;
    resident_bytes(manifest)
        .saturating_add(manifest_heap(manifest))
        .saturating_add(slots as u64)
        .saturating_add(2 * PAYLOAD_MANIFEST_DIGEST_LEN as u64)
}

/// While reading the tables, verification holds one of these beside what it retains: the sidecar's bytes beside the decoded sidecar, the bound manifest beside the stage metadata it copies, the store's hash buffer, or one identifier list's JSON beside its strings.
fn table_scratch_bytes(manifest: &GenerationManifest) -> u64 {
    let size = |path: &str| {
        manifest
            .files
            .iter()
            .find(|file| file.path == path)
            .map_or(0, |file| file.size)
    };
    [
        size(SIDECAR_FILE),
        manifest_heap(manifest).saturating_mul(2),
        FILE_HASH_BUFFER_BYTES as u64,
        size(ROW_IDS_FILE),
        size(TOMBSTONES_FILE),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
}

/// The share of [`verification_bytes`] the manifest alone decides, judged before the sidecar is read.
fn manifest_verification_bytes(manifest: &GenerationManifest) -> u64 {
    kept_bytes(manifest).saturating_add(table_scratch_bytes(manifest))
}

/// Heap bytes verification holds at once under `manifest` and its decoded `sidecar`.
/// It retains the resident tables, the manifest, and one string slot per declared row and tombstone, and beside them holds the larger of the table scratch and one row pass: a row chunk, the stored codes of its rows, the artifact header, and per coordinate the decoded row, the running maxima, the stored scales decoded, their reciprocals, and the scales file (four bytes each); after the pass the calibrated scales and their encoding take the place of the row, the maxima, and the stored scales.
pub fn verification_bytes(manifest: &GenerationManifest, sidecar: &VectorSidecar) -> u64 {
    let chunk = row_chunk_bytes(sidecar.vector_dimension);
    let row_pass = chunk
        .saturating_add(chunk / 4)
        .saturating_add(codec::ARTIFACT_HEADER_BYTES as u64)
        .saturating_add(u64::from(sidecar.vector_dimension).saturating_mul(20));
    let slots = sidecar
        .rows
        .saturating_add(sidecar.tombstones)
        .saturating_mul(size_of::<String>() as u64);
    kept_bytes(manifest)
        .saturating_add(slots)
        .saturating_add(table_scratch_bytes(manifest).max(row_pass))
}

/// Verification reads the row artifact in chunks of about this many bytes, and the codes of the same rows beside them.
pub const VERIFY_CHUNK_BYTES: u64 = 1 << 18;

/// Whether two strictly increasing lists share an entry, decided in one merge walk.
fn share_an_entry(left: &[String], right: &[String]) -> bool {
    let (mut l, mut r) = (0, 0);
    while let (Some(a), Some(b)) = (left.get(l), right.get(r)) {
        match a.cmp(b) {
            std::cmp::Ordering::Less => l += 1,
            std::cmp::Ordering::Greater => r += 1,
            std::cmp::Ordering::Equal => return true,
        }
    }
    false
}

/// Decodes a JSON string list into a vector sized to the sidecar's declared count and refuses a longer list at the first extra entry.
struct DeclaredList(usize);

impl<'de> serde::de::DeserializeSeed<'de> for DeclaredList {
    type Value = Vec<String>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> serde::de::Visitor<'de> for DeclaredList {
    type Value = Vec<String>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a list of at most {} strings", self.0)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut list = Vec::with_capacity(self.0);
        while list.len() < self.0 {
            match seq.next_element()? {
                Some(item) => list.push(item),
                None => return Ok(list),
            }
        }
        match seq.next_element::<serde::de::IgnoredAny>()? {
            None => Ok(list),
            Some(_) => Err(serde::de::Error::invalid_length(self.0 + 1, &self)),
        }
    }
}

/// `None` for malformed JSON or a list longer than `declared`.
fn decode_list(bytes: &[u8], declared: u64) -> Option<Vec<String>> {
    use serde::de::DeserializeSeed;
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let list = DeclaredList(usize::try_from(declared).ok()?)
        .deserialize(&mut deserializer)
        .ok()?;
    deserializer.end().ok()?;
    Some(list)
}

fn io_refusal(error: io::Error) -> VectorRefusal {
    VectorRefusal::Io(error.kind().to_string())
}

/// Checks the original-row artifact's header and that the file holds exactly the rows it declares, and returns their count.
fn open_rows(file: &mut StreamedFile, layout: &RowLayout) -> Result<u64, VectorRefusal> {
    let size = file.size();
    let mut header =
        vec![0u8; codec::ARTIFACT_HEADER_BYTES.min(usize::try_from(size).unwrap_or(0))];
    file.read_exact(&mut header).map_err(io_refusal)?;
    let count = codec::decode_header(&header, layout).map_err(VectorRefusal::Rows)?;
    codec::check_body(count, size - codec::ARTIFACT_HEADER_BYTES as u64, layout)
        .map_err(VectorRefusal::Rows)?;
    Ok(count)
}

/// Calls `each` with the index and decoded coordinates of each of the `count` rows that follow the header, in file order, read one chunk at a time; a row outside the layout refuses at its index.
fn for_each_row(
    file: &mut StreamedFile,
    layout: &RowLayout,
    count: u64,
    mut each: impl FnMut(usize, &[f32]) -> Result<(), VectorRefusal>,
) -> Result<(), VectorRefusal> {
    let row_bytes = codec::row_bytes(layout.dimension) as usize;
    let mut chunk = vec![0u8; row_chunk_bytes(layout.dimension) as usize];
    let mut decoder = codec::RowDecoder::new(layout);
    let count = count as usize;
    let mut index = 0;
    while index < count {
        let bytes = &mut chunk
            [..((count - index) * row_bytes).min(row_chunk_bytes(layout.dimension) as usize)];
        file.read_exact(bytes).map_err(io_refusal)?;
        for raw in bytes.chunks_exact(row_bytes) {
            each(
                index,
                decoder.decode(index, raw).map_err(VectorRefusal::Rows)?,
            )?;
            index += 1;
        }
    }
    Ok(())
}

fn check_export(export: &LiveRows, expected: &ExpectedVectors<'_>) -> Result<(), VectorRefusal> {
    let generation = expected.generation;
    let field = |field| VectorRefusal::Export { field };
    if export.generation.generation_id != generation.generation_id {
        return Err(field("generation_id"));
    }
    if export.generation.embedding_model != generation.embedding_model {
        return Err(field("embedding_model"));
    }
    if export.generation.tokenizer_fingerprint != generation.tokenizer_fingerprint {
        return Err(field("tokenizer_fingerprint"));
    }
    if export.generation.vector_dimension != generation.vector_dimension {
        return Err(field("vector_dimension"));
    }
    if export.generation.generation_epoch != generation.generation_epoch {
        return Err(field("generation_epoch"));
    }
    if export.kernel_incarnation_id != expected.kernel_incarnation_id {
        return Err(field("kernel_incarnation_id"));
    }
    if expected
        .checkpoint
        .is_some_and(|checkpoint| export.checkpoint != *checkpoint)
    {
        return Err(field("checkpoint"));
    }
    Ok(())
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
    let mut codes = Vec::with_capacity(rows.size_hint().0 * layout.dimension as usize);
    for (index, row) in rows.enumerate() {
        let encoded =
            scalar::encode(layout, scales, row).map_err(|rejection| (index, rejection))?;
        codes.extend(encoded.codes.iter().map(|code| *code as u8));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_bytes_saturate_on_declared_sizes_that_overflow() {
        let file = |path: &str, size: u64| ManifestFile {
            path: path.to_owned(),
            mode: 0o600,
            size,
            sha256: "0".repeat(64),
        };
        let manifest = GenerationManifest {
            schema: 1,
            target: VECTOR_TARGET.to_owned(),
            release_contract_sha256: "a".repeat(64),
            inputs_lock_sha256: "b".repeat(64),
            source_payload_manifest_sha256: None,
            files: vec![
                file(ROWS_FILE, u64::MAX),
                file(ROW_IDS_FILE, u64::MAX),
                file(SCALES_FILE, 1),
            ],
        };
        assert_eq!(resident_bytes(&manifest), u64::MAX);
    }

    #[test]
    fn the_compatibility_digest_hashes_the_fields_as_one_json_array() {
        let identity = VectorIdentity {
            embedding_model: "model \"quoted\"".to_owned(),
            tokenizer_fingerprint: "tok".to_owned(),
            vector_dimension: 384,
            metric: "inner_product".to_owned(),
            unit_norm_tolerance_bits: 1e-3f64.to_bits(),
            quantizer_recipe: "scalar-int8-symmetric.v1".to_owned(),
            generation_epoch: 7,
            kernel_incarnation_id: "kernel".to_owned(),
        };
        let array = serde_json::to_vec(&serde_json::json!([
            identity.embedding_model,
            identity.tokenizer_fingerprint,
            identity.vector_dimension,
            identity.metric,
            identity.unit_norm_tolerance_bits,
            identity.quantizer_recipe,
            identity.generation_epoch,
        ]))
        .unwrap();
        assert_eq!(identity.compatibility_sha256(), sha256_hex(&array));
    }

    #[test]
    fn exact_json_matches_only_the_whole_encoding() {
        let value = ["alpha", "beta"];
        let bytes = exact_json(&value);
        assert!(is_exact_json(&value, &bytes));
        assert!(!is_exact_json(&value, &bytes[..bytes.len() - 1]));
        assert!(!is_exact_json(&value, &[bytes.as_slice(), b" "].concat()));
        assert!(!is_exact_json(&value, br#"["alpha","beta "]"#));
    }

    #[test]
    fn sorted_lists_share_an_entry_wherever_it_falls() {
        let list = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>()
        };
        let ids = list(&["b", "d", "f", "h"]);
        for shared in ["b", "f", "h"] {
            let mut other = list(&["a", "c", "g"]);
            other.push(shared.to_owned());
            other.sort();
            assert!(share_an_entry(&ids, &other), "{shared}");
            assert!(share_an_entry(&other, &ids), "{shared}");
        }
        assert!(!share_an_entry(&ids, &list(&["a", "c", "e", "g", "i"])));
        assert!(!share_an_entry(&ids, &[]));
        assert!(!share_an_entry(&[], &ids));
    }

    #[test]
    fn a_list_decodes_up_to_its_declared_count_and_refuses_more() {
        let list = br#"["a","b","c"]"#;
        assert_eq!(
            decode_list(list, 3),
            Some(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()])
        );
        assert_eq!(decode_list(list, 4).map(|list| list.len()), Some(3));
        assert_eq!(decode_list(list, 2), None);
        assert_eq!(decode_list(br#"["a","b"] []"#, 2), None);
        assert_eq!(decode_list(br#"["a",1]"#, 2), None);
        assert_eq!(decode_list(b"[]", 0), Some(Vec::new()));
    }
}
