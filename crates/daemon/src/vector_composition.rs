//! Publishes and recovers one complete vector composition: one base layer plus bounded, ordered delta layers, named together by an immutable composition generation the vector selector points at.
//!
//! Publication stages the composition, then moves the selector in one rename, so an interruption leaves the old complete selection or the new one and never a mix. The selector's `members.json` makes the lifecycle store retain every member while the composition is selected or pinned by a reader.
//! How far an attempt got is tracked as a ladder of separate facts; an unknown outcome is settled by reading the selector back and comparing digests, never by treating an error as a rollback.
//! Recovery takes the selected composition when it verifies; otherwise it examines the other compositions newest first under a bound and takes the first whose every member verifies, reporting the selector as stale rather than silently repointing it.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;

use host_runtime::generation::ValidatedGeneration;
use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationManifest, GenerationStore, MEMBERS_FILE_NAME,
    ManifestFile, ProfileEvent, StageMeta, VECTOR_SELECTION_TARGET, WireMembers,
};
use host_runtime::lifecycle::LifecycleTransactionLock;

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
    #[error("the lifecycle store refused: {0}")]
    Store(String),
    #[error(
        "the vector selector, or a generation the store holds, carries a state schema this build does not know"
    )]
    Quarantined,
    #[error("insufficient storage")]
    InsufficientStorage,
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
    write_new(
        &work_dir.join(COMPOSITION_FILE),
        &composition.canonical_bytes(),
    )
    .and_then(|()| {
        write_new(
            &work_dir.join(MEMBERS_FILE_NAME),
            &composition.members_bytes(),
        )
    })
    .map_err(|refusal| fail(progress, CompositionRefusal::Stage(refusal)))?;
    // Members were verified before composing; the store re-validates each on selection, so a member reclaimed meanwhile refuses the selection rather than exposing a partial set.
    let digest = staging
        .stage_manifest(
            &composition.stage_manifest(),
            &composition.stage_meta(),
            |path| work_dir.join(path),
        )
        .map_err(|refusal| fail(progress, CompositionRefusal::Stage(refusal)))?;
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

/// Returns the selected composition's sequence, or `None` when no composition is selected or its record fails validation.
///
/// # Errors
///
/// Returns `Quarantined` when the selector is quarantined or the selected record has an unknown schema.
fn selected_sequence(store: &GenerationStore) -> Result<Option<u64>, CompositionRefusal> {
    match store.read_vector_current()? {
        CurrentProfile::Quarantined => Err(CompositionRefusal::Quarantined),
        CurrentProfile::Absent => Ok(None),
        CurrentProfile::Current(digest) => Ok(record(store, &digest)?
            .filter(|_| store.validate(&digest).is_ok())
            .map(|record| record.sequence)),
    }
}

/// Reads records only from manifests targeting `VECTOR_SELECTION_TARGET`. Full inventory and member verification still belong inside the candidate bound.
///
/// # Errors
///
/// Returns `Quarantined` when the record has an unknown schema.
fn record(
    store: &GenerationStore,
    digest: &str,
) -> Result<Option<Composition>, CompositionRefusal> {
    let Ok(manifest) = store.manifest(digest) else {
        return Ok(None);
    };
    if manifest.target != VECTOR_SELECTION_TARGET {
        return Ok(None);
    }
    let Ok(bytes) = store.read_manifest_file(digest, COMPOSITION_FILE) else {
        return Ok(None);
    };
    match decode_record(&manifest, &bytes) {
        Ok(record) => Ok(Some(record)),
        Err(CompositionRefusal::Quarantined) => Err(CompositionRefusal::Quarantined),
        Err(_) => Ok(None),
    }
}

fn decode_record(
    manifest: &GenerationManifest,
    bytes: &[u8],
) -> Result<Composition, CompositionRefusal> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| CompositionRefusal::NotComposition("record"))?;
    match value.get("schema").and_then(serde_json::Value::as_u64) {
        Some(schema) if schema == u64::from(COMPOSITION_SCHEMA) => {}
        Some(_) => return Err(CompositionRefusal::Quarantined),
        None => return Err(CompositionRefusal::NotComposition("record")),
    }
    let composition: Composition =
        serde_json::from_value(value).map_err(|_| CompositionRefusal::NotComposition("record"))?;
    if composition.canonical_bytes() != bytes {
        return Err(CompositionRefusal::NotComposition("record not canonical"));
    }
    // Matching manifests bind the composition to the members file.
    if composition.stage_manifest() != *manifest {
        return Err(CompositionRefusal::NotComposition("manifest binding"));
    }
    Ok(composition)
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

/// A composition whose generation, record, and every member were verified.
pub struct VerifiedComposition {
    pub digest: String,
    pub composition: Composition,
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

fn verify_validated(
    store: &GenerationStore,
    generation: ValidatedGeneration,
    expected: &ExpectedVectors<'_>,
    max_deltas: NonZeroUsize,
) -> Result<VerifiedComposition, CompositionRefusal> {
    if generation.manifest.target != VECTOR_SELECTION_TARGET {
        return Err(CompositionRefusal::NotComposition("manifest target"));
    }
    let bytes = generation.read_verified_file(COMPOSITION_FILE)?;
    let composition = decode_record(&generation.manifest, &bytes)?;
    // Refuse before opening members: the bound limits verification work, not just the returned topology.
    if composition.deltas.len() > max_deltas.get() {
        return Err(CompositionRefusal::DeltasOverBound {
            count: composition.deltas.len(),
            max: max_deltas.get(),
        });
    }
    if composition
        .identity()
        .first_mismatch(&VectorIdentity::from_expected(expected))
        .is_some()
    {
        return Err(CompositionRefusal::NotComposition("identity"));
    }
    let member = |digest: &str| {
        verify(store, digest, expected).map_err(|refusal| CompositionRefusal::Member {
            digest: digest.to_owned(),
            refusal,
        })
    };
    let base = member(&composition.base)?;
    let deltas = composition
        .deltas
        .iter()
        .map(|digest| member(digest))
        .collect::<Result<Vec<_>, _>>()?;
    check_topology(expected, &base, &deltas, max_deltas)?;
    Ok(VerifiedComposition {
        digest: generation.digest.clone(),
        composition,
        base,
        deltas,
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

/// Takes the selected composition when it verifies. After selected-composition verification fails or the selector is absent, retains the newest `bound` candidates and fully verifies them in descending order. Discovery retains no generation descriptors.
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
    let store_error = |error: GenerationError| Unavailable::Store(error.to_string());
    let selected = match store.read_vector_current().map_err(store_error)? {
        CurrentProfile::Quarantined => return Err(Unavailable::QuarantinedSelector),
        CurrentProfile::Current(digest) => Some(digest),
        CurrentProfile::Absent => None,
    };
    let mut examined = 0;
    if let Some(current) = &selected {
        examined += 1;
        if let Ok(composition) = verify_composition(store, current, expected, max_deltas) {
            return Ok(Recovered {
                composition,
                selector: SelectorState::Current,
            });
        }
    }
    let mut candidates = BTreeSet::new();
    for digest in store.digests().map_err(store_error)? {
        if selected.as_deref() == Some(digest.as_str()) {
            continue;
        }
        if let Ok(Some(record)) = record(store, &digest) {
            candidates.insert((record.sequence, digest));
            if candidates.len() > bound.get() {
                candidates.pop_first();
            }
        }
    }
    // Newest first; equal sequences fall back to descending digest order.
    for (_, digest) in candidates.into_iter().rev() {
        examined += 1;
        if let Ok(composition) = verify_composition(store, &digest, expected, max_deltas) {
            let selector = match &selected {
                Some(current) => SelectorState::Stale(current.clone()),
                None => SelectorState::Absent,
            };
            return Ok(Recovered {
                composition,
                selector,
            });
        }
    }
    Err(Unavailable::NoCompatibleTarget { examined })
}
