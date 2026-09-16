//! Publishes and recovers one complete vector composition: one base layer plus bounded, ordered delta layers, named together by an immutable composition generation the vector selector points at.
//!
//! Publication stages the composition, then moves the selector in one rename, so an interruption leaves the old complete selection or the new one and never a mix. The selector's `members.json` makes the lifecycle store retain every member while the composition is selected or pinned by a reader.
//! How far an attempt got is tracked as a ladder of separate facts; an unknown outcome is settled by reading the selector back and comparing digests, never by treating an error as a rollback.
//! Recovery takes the selected composition when it verifies; otherwise it examines the other compositions newest first under a bound and takes the first whose every member verifies, reporting the selector as stale rather than silently repointing it.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;

use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationManifest, GenerationStore, MEMBERS_FILE_NAME,
    ManifestFile, ProfileEvent, StageMeta, VECTOR_SELECTION_TARGET, ValidatedGeneration,
    WireMembers,
};
use host_runtime::lifecycle::LifecycleTransactionLock;

use crate::projection_gates::Denial;
use crate::vector_generation::{
    ExpectedVectors, Staging, VectorIdentity, VectorRefusal, VerifiedVectors, sha256_hex, verify,
    write_new,
};

pub const COMPOSITION_FILE: &str = "composition.json";
pub const COMPOSITION_SCHEMA: u32 = 1;
pub const MEMBERS_SCHEMA: u32 = 1;

/// Field order is the wire order; `canonical_bytes` is the only encoding.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub schema: u32,
    /// Publication order among compositions of one namespace; a publication must exceed the selected composition's, and recovery falls back in descending order.
    pub sequence: u64,
    pub embedding_model: String,
    pub tokenizer_fingerprint: String,
    pub vector_dimension: u32,
    pub metric: String,
    pub unit_norm_tolerance: f64,
    pub quantizer_recipe: String,
    pub generation_epoch: u64,
    pub kernel_incarnation_id: String,
    pub base: String,
    /// Delta digests in application order; a later delta wins over an earlier one and every delta wins over the base.
    pub deltas: Vec<String>,
}

impl Composition {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("composition serialization cannot fail")
    }

    pub fn members(&self) -> Vec<String> {
        std::iter::once(self.base.clone())
            .chain(self.deltas.iter().cloned())
            .collect()
    }

    pub fn identity(&self) -> VectorIdentity {
        VectorIdentity {
            embedding_model: self.embedding_model.clone(),
            tokenizer_fingerprint: self.tokenizer_fingerprint.clone(),
            vector_dimension: self.vector_dimension,
            metric: self.metric.clone(),
            unit_norm_tolerance_bits: self.unit_norm_tolerance.to_bits(),
            quantizer_recipe: self.quantizer_recipe.clone(),
            generation_epoch: self.generation_epoch,
            kernel_incarnation_id: self.kernel_incarnation_id.clone(),
        }
    }

    fn members_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&WireMembers {
            schema: MEMBERS_SCHEMA,
            members: self.members(),
        })
        .expect("members serialization cannot fail")
    }

    pub fn stage_meta(&self) -> StageMeta {
        StageMeta {
            target: VECTOR_SELECTION_TARGET.to_owned(),
            release_contract_sha256: self.identity().compatibility_sha256(),
            inputs_lock_sha256: sha256_hex(&self.canonical_bytes()),
            source_payload_manifest_sha256: self.base.clone(),
        }
    }

    pub fn stage_manifest(&self) -> GenerationManifest {
        let composition = self.canonical_bytes();
        let members = self.members_bytes();
        GenerationManifest::from_files(
            &self.stage_meta(),
            vec![
                ManifestFile {
                    path: COMPOSITION_FILE.to_owned(),
                    mode: 0o600,
                    size: composition.len() as u64,
                    sha256: sha256_hex(&composition),
                },
                ManifestFile {
                    path: MEMBERS_FILE_NAME.to_owned(),
                    mode: 0o600,
                    size: members.len() as u64,
                    sha256: sha256_hex(&members),
                },
            ],
        )
    }

    pub fn digest(&self) -> String {
        self.stage_manifest().digest()
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompositionRefusal {
    #[error("the base masks {tombstones} occurrences; only a delta carries tombstones")]
    BaseTombstones { tombstones: u64 },
    #[error("delta {index} repeats a member digest")]
    DuplicateMember { index: usize },
    #[error("{count} deltas exceed the {max} delta bound")]
    DeltasOverBound { count: usize, max: usize },
    #[error("delta {index} does not follow its predecessor's checkpoint")]
    CheckpointOrder { index: usize },
    #[error("member {digest} does not carry the composition's identity: {field}")]
    MemberIdentity { digest: String, field: &'static str },
    #[error("sequence {sequence} does not exceed the selected composition's {selected}")]
    Sequence { sequence: u64, selected: u64 },
    #[error("the composition is not a vector composition: {0}")]
    NotComposition(&'static str),
    #[error("member {digest}: {refusal}")]
    Member {
        digest: String,
        refusal: VectorRefusal,
    },
    #[error("staging: {0}")]
    Stage(VectorRefusal),
    #[error("delta admission: {0}")]
    Deltas(Denial),
    #[error("the lifecycle store refused: {0}")]
    Store(String),
    #[error(
        "the vector selector, or a generation the store holds, carries a state schema this build does not know"
    )]
    Quarantined,
    #[error("insufficient storage")]
    InsufficientStorage,
    #[error("i/o failure: {0}")]
    Io(String),
}

impl From<GenerationError> for CompositionRefusal {
    fn from(error: GenerationError) -> Self {
        match error {
            GenerationError::InsufficientStorage => Self::InsufficientStorage,
            GenerationError::UnsupportedStateSchema => Self::Quarantined,
            other => Self::Store(other.to_string()),
        }
    }
}

/// One base and its ordered deltas, each already verified under the same expectation.
pub struct CompositionSpec<'a> {
    pub expected: &'a ExpectedVectors<'a>,
    pub sequence: u64,
    pub base: &'a VerifiedVectors,
    pub deltas: &'a [VerifiedVectors],
    pub max_deltas: NonZeroUsize,
}

/// Names a composition after checking its topology: at most `max_deltas` distinct deltas, every member carrying the expectation's identity, and checkpoints that never move backwards from the base through each delta.
///
/// # Errors
///
/// Any topology or identity disagreement; nothing is staged.
pub fn compose(spec: &CompositionSpec<'_>) -> Result<Composition, CompositionRefusal> {
    check_topology(spec.expected, spec.base, spec.deltas, spec.max_deltas)?;
    let identity = VectorIdentity::from_expected(spec.expected);
    Ok(Composition {
        schema: COMPOSITION_SCHEMA,
        sequence: spec.sequence,
        embedding_model: identity.embedding_model,
        tokenizer_fingerprint: identity.tokenizer_fingerprint,
        vector_dimension: identity.vector_dimension,
        metric: identity.metric,
        unit_norm_tolerance: spec.expected.unit_norm_tolerance,
        quantizer_recipe: identity.quantizer_recipe,
        generation_epoch: identity.generation_epoch,
        kernel_incarnation_id: identity.kernel_incarnation_id,
        base: spec.base.digest.clone(),
        deltas: spec
            .deltas
            .iter()
            .map(|delta| delta.digest.clone())
            .collect(),
    })
}

/// The one-base, bounded, ordered-member contract, checked the same way at composition and at verification.
/// The checkpoint rule is the conservative reading of the not yet frozen base/delta relation: every delta's snapshot and checkpoint are at or after the checkpoint of the layer before it.
fn check_topology(
    expected: &ExpectedVectors<'_>,
    base: &VerifiedVectors,
    deltas: &[VerifiedVectors],
    max_deltas: NonZeroUsize,
) -> Result<(), CompositionRefusal> {
    if deltas.len() > max_deltas.get() {
        return Err(CompositionRefusal::DeltasOverBound {
            count: deltas.len(),
            max: max_deltas.get(),
        });
    }
    if base.sidecar.tombstones != 0 {
        return Err(CompositionRefusal::BaseTombstones {
            tombstones: base.sidecar.tombstones,
        });
    }
    let identity = VectorIdentity::from_expected(expected);
    let check_member = |member: &VerifiedVectors| {
        VectorIdentity::from_sidecar(&member.sidecar)
            .first_mismatch(&identity)
            .map_or(Ok(()), |field| {
                Err(CompositionRefusal::MemberIdentity {
                    digest: member.digest.clone(),
                    field,
                })
            })
    };
    check_member(base)?;
    let mut seen = BTreeSet::from([base.digest.clone()]);
    let mut previous = base.sidecar.checkpoint().checkpoint_commit_seq;
    for (index, delta) in deltas.iter().enumerate() {
        if !seen.insert(delta.digest.clone()) {
            return Err(CompositionRefusal::DuplicateMember { index });
        }
        check_member(delta)?;
        let checkpoint = delta.sidecar.checkpoint();
        if checkpoint.snapshot_commit_seq < previous || checkpoint.checkpoint_commit_seq < previous
        {
            return Err(CompositionRefusal::CheckpointOrder { index });
        }
        previous = checkpoint.checkpoint_commit_seq;
    }
    Ok(())
}

/// How far one publication attempt is known to have gone; each rung is recorded when its step returns, so a lost reply leaves the attempt on the last rung that returned without implying the next did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Nothing reached the store.
    NotStaged,
    /// The composition generation is in the store; the selector is unchanged.
    Staged,
    /// The selector rename returned, so a reader can see the new selection.
    Acknowledged,
    /// The containing directory sync returned.
    Durable,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{refusal}")]
pub struct PublishFailure {
    pub progress: Progress,
    pub refusal: CompositionRefusal,
}

/// Stages `composition` through `staging` and points the vector selector at it. `observer` sees every selector event after this function has recorded it. A composition whose sequence does not exceed the selected composition's is refused, so two publishers under the same lock cannot reorder the namespace.
///
/// # Errors
///
/// A failure carries how far the attempt is known to have gone; a failure at or after `Acknowledged` is an unknown outcome the caller settles with [`reconcile`].
pub fn publish(
    composition: &Composition,
    staging: &Staging<'_>,
    work_dir: &Path,
    observer: &mut dyn FnMut(ProfileEvent) -> Result<(), GenerationError>,
) -> Result<String, Box<PublishFailure>> {
    let mut progress = Progress::NotStaged;
    let fail = |progress: Progress, refusal| Box::new(PublishFailure { progress, refusal });
    if let Some(selected) =
        selected_sequence(staging.store).map_err(|refusal| fail(progress, refusal))?
        && composition.sequence <= selected
    {
        return Err(fail(
            progress,
            CompositionRefusal::Sequence {
                sequence: composition.sequence,
                selected,
            },
        ));
    }
    staging
        .ledger
        .admit_deltas(staging.admission, composition.deltas.len())
        .map_err(|denial| fail(progress, CompositionRefusal::Deltas(denial)))?;
    // Members were verified before composing; the store re-validates each on selection, so a member reclaimed meanwhile refuses the selection rather than exposing a partial set.
    let staged = write_new(
        &work_dir.join(COMPOSITION_FILE),
        &composition.canonical_bytes(),
    )
    .and_then(|()| {
        write_new(
            &work_dir.join(MEMBERS_FILE_NAME),
            &composition.members_bytes(),
        )
    })
    .and_then(|()| {
        staging.stage_manifest(
            &composition.stage_manifest(),
            &composition.stage_meta(),
            |path| work_dir.join(path),
        )
    });
    // The store holds its own copies once staged, and a retry through the same `work_dir` creates these names again.
    for name in [COMPOSITION_FILE, MEMBERS_FILE_NAME] {
        let _ = std::fs::remove_file(work_dir.join(name));
    }
    let digest = staged.map_err(|refusal| fail(progress, CompositionRefusal::Stage(refusal)))?;
    progress = Progress::Staged;
    let selected = staging
        .store
        .select_vector(&digest, staging.transaction, &mut |event| {
            match event {
                ProfileEvent::AfterRename => progress = Progress::Acknowledged,
                ProfileEvent::AfterDirectorySync => progress = Progress::Durable,
                ProfileEvent::BeforeRename | ProfileEvent::BeforeDirectorySync => {}
            }
            observer(event)
        });
    match selected {
        Ok(()) => Ok(digest),
        Err(error) => Err(fail(progress, error.into())),
    }
}

/// The sequence of the composition the selector names, or `None` when nothing is selected or the selection is not a readable composition; the latter must not block a repair publication.
fn selected_sequence(store: &GenerationStore) -> Result<Option<u64>, CompositionRefusal> {
    match store.read_vector_current()? {
        CurrentProfile::Quarantined => Err(CompositionRefusal::Quarantined),
        CurrentProfile::Absent => Ok(None),
        CurrentProfile::Current(digest) => {
            Ok(record(store, &digest).map(|(_, record)| record.sequence))
        }
    }
}

/// The validated generation and record of `digest`, or `None` when its manifest does not name a vector composition or its record does not decode. The manifest is read before the files are hashed, so a store full of other owners' generations costs one manifest read each.
fn record(store: &GenerationStore, digest: &str) -> Option<(ValidatedGeneration, Composition)> {
    if store.manifest(digest).ok()?.target != VECTOR_SELECTION_TARGET {
        return None;
    }
    let generation = store.validate(digest).ok()?;
    let record =
        serde_json::from_slice(&generation.read_verified_file(COMPOSITION_FILE).ok()?).ok()?;
    Some((generation, record))
}

/// What the selector durably names after an attempt with an unknown outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciled {
    /// The attempted composition is the selection; the attempt took effect.
    Published,
    /// Another composition, or none, is the selection: the attempt did not take effect, or a later publication replaced it. Whatever is named is complete.
    Other(Option<String>),
    /// The selector carries an unknown schema; nothing can be concluded and nothing may be mutated.
    Quarantined,
}

/// Reads the selector back and syncs its directory, then decides by digest identity whether `intended` took effect.
pub fn reconcile(
    store: &GenerationStore,
    transaction: &LifecycleTransactionLock,
    intended: &str,
) -> Result<Reconciled, CompositionRefusal> {
    match store.reconcile_vector(transaction)? {
        CurrentProfile::Current(current) if current == intended => Ok(Reconciled::Published),
        CurrentProfile::Current(current) => Ok(Reconciled::Other(Some(current))),
        CurrentProfile::Absent => Ok(Reconciled::Other(None)),
        CurrentProfile::Quarantined => Ok(Reconciled::Quarantined),
    }
}

/// A composition whose generation, record, and every member were verified. `record` retains the composition generation's directory descriptor, so a reader pins the record verification saw.
pub struct VerifiedComposition {
    pub digest: String,
    pub composition: Composition,
    pub record: ValidatedGeneration,
    pub base: VerifiedVectors,
    pub deltas: Vec<VerifiedVectors>,
}

impl std::fmt::Debug for VerifiedComposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedComposition")
            .field("digest", &self.digest)
            .field("composition", &self.composition)
            .finish_non_exhaustive()
    }
}

/// Verifies one composition generation and every member under `expected`, and re-checks the topology under `max_deltas` so a composition current admission would refuse does not verify.
///
/// # Errors
///
/// A generation of another owner or schema, a record that is not canonical or does not agree with its members file, a record whose identity is not `expected`, a member that fails [`verify`], or a topology outside the contract.
pub fn verify_composition(
    store: &GenerationStore,
    digest: &str,
    expected: &ExpectedVectors<'_>,
    max_deltas: NonZeroUsize,
) -> Result<VerifiedComposition, CompositionRefusal> {
    let generation = store.validate(digest)?;
    verify_validated(store, generation, expected, max_deltas)
}

/// Verifies the composition generation `digest` and its record without opening a member: the record is a canonical composition bound to its manifest and carrying `expected`.
///
/// # Errors
///
/// A generation of another owner or schema, or a record that is not canonical, does not agree with its members file, or carries another identity.
pub fn verify_record(
    store: &GenerationStore,
    digest: &str,
    expected: &ExpectedVectors<'_>,
) -> Result<Composition, CompositionRefusal> {
    verify_record_of(&store.validate(digest)?, expected)
}

fn verify_record_of(
    generation: &ValidatedGeneration,
    expected: &ExpectedVectors<'_>,
) -> Result<Composition, CompositionRefusal> {
    if generation.manifest.target != VECTOR_SELECTION_TARGET {
        return Err(CompositionRefusal::NotComposition("manifest target"));
    }
    let bytes = generation.read_verified_file(COMPOSITION_FILE)?;
    let composition: Composition =
        serde_json::from_slice(&bytes).map_err(|_| CompositionRefusal::NotComposition("record"))?;
    if composition.schema != COMPOSITION_SCHEMA {
        return Err(CompositionRefusal::NotComposition("record schema"));
    }
    if composition.canonical_bytes() != bytes {
        return Err(CompositionRefusal::NotComposition("record not canonical"));
    }
    // The manifest names the members file by hash and the store verified that hash, so equal manifests mean the members file agrees with the record.
    if composition.stage_manifest() != generation.manifest {
        return Err(CompositionRefusal::NotComposition("manifest binding"));
    }
    if composition
        .identity()
        .first_mismatch(&VectorIdentity::from_expected(expected))
        .is_some()
    {
        return Err(CompositionRefusal::NotComposition("identity"));
    }
    Ok(composition)
}

fn verify_validated(
    store: &GenerationStore,
    generation: ValidatedGeneration,
    expected: &ExpectedVectors<'_>,
    max_deltas: NonZeroUsize,
) -> Result<VerifiedComposition, CompositionRefusal> {
    let composition = verify_record_of(&generation, expected)?;
    let base = verify_member(store, &composition.base, expected)?;
    let deltas = composition
        .deltas
        .iter()
        .map(|digest| verify_member(store, digest, expected))
        .collect::<Result<Vec<_>, _>>()?;
    check_topology(expected, &base, &deltas, max_deltas)?;
    Ok(VerifiedComposition {
        digest: generation.digest.clone(),
        composition,
        record: generation,
        base,
        deltas,
    })
}

/// # Errors
///
/// Returns the failing member's `digest` in [`CompositionRefusal::Member`].
pub fn verify_member(
    store: &GenerationStore,
    digest: &str,
    expected: &ExpectedVectors<'_>,
) -> Result<VerifiedVectors, CompositionRefusal> {
    verify(store, digest, expected).map_err(|refusal| CompositionRefusal::Member {
        digest: digest.to_owned(),
        refusal,
    })
}

/// What the vector selector named when recovery ran, relative to the composition recovery chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorState {
    /// The selector names the recovered composition.
    Current,
    /// The selector names a digest that did not verify; the recovered composition is the newest other one that did. The selector is left as it is.
    Stale(String),
    /// No selector exists; the recovered composition was found among the store's generations.
    Absent,
}

#[derive(Debug)]
pub struct Recovered {
    pub composition: VerifiedComposition,
    pub selector: SelectorState,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Unavailable {
    #[error("the vector selector carries a state schema this build does not know")]
    QuarantinedSelector,
    #[error("no vector composition verifies under the expectation; {examined} were examined")]
    NoCompatibleTarget { examined: usize },
    #[error("the lifecycle store refused: {0}")]
    Store(String),
}

/// One composition recovery may take, with what the selector said about it when it was listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub digest: String,
    pub selector: SelectorState,
}

/// The compositions recovery examines, in order: the selected digest when `current` names one, then at most `bound` other records by descending sequence, equal sequences by descending digest. Only manifests and records are read; no member is opened. An acknowledged selection is never displaced by a newer composition that was staged but not selected.
///
/// # Errors
///
/// A quarantined selector or a store that cannot list its generations; neither mutates the store.
pub fn candidates(
    store: &GenerationStore,
    current: &CurrentProfile,
    bound: NonZeroUsize,
) -> Result<Vec<Candidate>, Unavailable> {
    let selected = match current {
        CurrentProfile::Quarantined => return Err(Unavailable::QuarantinedSelector),
        CurrentProfile::Current(digest) => Some(digest.as_str()),
        CurrentProfile::Absent => None,
    };
    let mut others: Vec<(u64, String)> = Vec::new();
    for digest in store
        .digests()
        .map_err(|error| Unavailable::Store(error.to_string()))?
    {
        if selected == Some(digest.as_str()) {
            continue;
        }
        if let Some((_, record)) = record(store, &digest) {
            others.push((record.sequence, digest));
        }
    }
    // Newest first; equal sequences fall back to digest order so the choice is deterministic.
    others.sort_by(|(left_sequence, left), (right_sequence, right)| {
        right_sequence
            .cmp(left_sequence)
            .then_with(|| right.cmp(left))
    });
    let fallback = match selected {
        Some(current) => SelectorState::Stale(current.to_owned()),
        None => SelectorState::Absent,
    };
    Ok(selected
        .map(|digest| Candidate {
            digest: digest.to_owned(),
            selector: SelectorState::Current,
        })
        .into_iter()
        .chain(
            others
                .into_iter()
                .take(bound.get())
                .map(|(_, digest)| Candidate {
                    digest,
                    selector: fallback.clone(),
                }),
        )
        .collect())
}

/// Fully verifies the [`candidates`] in order and takes the first that passes. `_transaction` keeps a concurrent mutator from reclaiming what recovery is examining.
///
/// # Errors
///
/// A quarantined selector or no verifying composition within the bound; neither mutates the store.
pub fn recover(
    store: &GenerationStore,
    _transaction: &LifecycleTransactionLock,
    expected: &ExpectedVectors<'_>,
    max_deltas: NonZeroUsize,
    bound: NonZeroUsize,
) -> Result<Recovered, Unavailable> {
    let current = store
        .read_vector_current()
        .map_err(|error| Unavailable::Store(error.to_string()))?;
    let candidates = candidates(store, &current, bound)?;
    let examined = candidates.len();
    for candidate in candidates {
        if let Ok(composition) = verify_composition(store, &candidate.digest, expected, max_deltas)
        {
            return Ok(Recovered {
                composition,
                selector: candidate.selector,
            });
        }
    }
    Err(Unavailable::NoCompatibleTarget { examined })
}
