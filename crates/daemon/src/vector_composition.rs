//! Publishes and recovers one complete vector composition: one base layer plus bounded, ordered delta layers, named together by an immutable composition generation the vector selector points at.
//!
//! Publication stages the composition, then moves the selector in one rename, so an interruption leaves the old complete selection or the new one and never a mix. The selector's `members.json` makes the lifecycle store retain every member while the composition is selected, with or without readers.
//! Attempt, acknowledgement, and durability are tracked as separate facts; an unknown outcome is settled by reading the selector back and comparing digests, never by treating an error as a rollback.
//! Recovery takes the selected composition when it verifies; otherwise it examines the other compositions newest first under a bound and takes the first whose every member verifies, reporting the selector as stale rather than silently repointing it.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::Path;

use host_runtime::generation::{
    CurrentProfile, GenerationError, GenerationManifest, GenerationStore, MEMBERS_FILE_NAME,
    ManifestFile, ProfileEvent, StageMeta, VECTOR_SELECTION_TARGET, WireMembers,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use retrieval::dense::codec::Metric;
use retrieval::dense::scalar::ScalarRecipe;
use sha2::{Digest, Sha256};

use crate::vector_generation::{
    ExpectedVectors, Staging, VectorRefusal, VerifiedVectors, read_verified, verify,
};

pub const COMPOSITION_FILE: &str = "composition.json";
pub const COMPOSITION_SCHEMA: u32 = 1;
pub const MEMBERS_SCHEMA: u32 = 1;

/// Field order is the wire order; `canonical_bytes` is the only encoding.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub schema: u32,
    /// Publication order among compositions of one namespace; recovery prefers the highest that verifies.
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

    fn members_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&WireMembers {
            schema: MEMBERS_SCHEMA,
            members: self.members(),
        })
        .expect("members serialization cannot fail")
    }

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
            target: VECTOR_SELECTION_TARGET.to_owned(),
            release_contract_sha256: sha256_hex(&compatibility),
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
    #[error("the base layer carries tombstones")]
    BaseWithTombstones,
    #[error("delta {index} repeats a member digest")]
    DuplicateMember { index: usize },
    #[error("{count} deltas exceed the {max} delta bound")]
    DeltasOverBound { count: usize, max: usize },
    #[error("delta {index} does not follow its predecessor's checkpoint")]
    CheckpointOrder { index: usize },
    #[error("member {digest} does not carry the composition's identity: {field}")]
    MemberIdentity { digest: String, field: &'static str },
    #[error("the composition is not a vector composition: {0}")]
    NotComposition(&'static str),
    #[error("member {digest}: {refusal}")]
    Member {
        digest: String,
        refusal: VectorRefusal,
    },
    #[error("staging: {0}")]
    Stage(VectorRefusal),
    #[error("selection: {0}")]
    Select(String),
    #[error("the vector selector carries a state schema this build does not know")]
    Quarantined,
    #[error("i/o failure: {0}")]
    Io(String),
}

/// One base and its ordered deltas, each already verified under the same expectation.
pub struct CompositionSpec<'a> {
    pub expected: &'a ExpectedVectors<'a>,
    pub sequence: u64,
    pub base: &'a VerifiedVectors,
    pub deltas: &'a [VerifiedVectors],
    pub max_deltas: NonZeroUsize,
}

/// Names a composition after checking its topology: a base without tombstones, at most `max_deltas` distinct deltas, every member carrying the expectation's identity, and checkpoints that never move backwards from the base through each delta.
///
/// # Errors
///
/// Any topology or identity disagreement; nothing is staged.
pub fn compose(spec: &CompositionSpec<'_>) -> Result<Composition, CompositionRefusal> {
    if spec.base.sidecar.tombstones != 0 {
        return Err(CompositionRefusal::BaseWithTombstones);
    }
    if spec.deltas.len() > spec.max_deltas.get() {
        return Err(CompositionRefusal::DeltasOverBound {
            count: spec.deltas.len(),
            max: spec.max_deltas.get(),
        });
    }
    let expected = spec.expected;
    let mut seen = BTreeSet::from([spec.base.digest.clone()]);
    let mut previous = spec.base.sidecar.checkpoint().checkpoint_commit_seq;
    check_member(spec.base, expected)?;
    for (index, delta) in spec.deltas.iter().enumerate() {
        if !seen.insert(delta.digest.clone()) {
            return Err(CompositionRefusal::DuplicateMember { index });
        }
        check_member(delta, expected)?;
        let checkpoint = delta.sidecar.checkpoint();
        if checkpoint.snapshot_commit_seq < previous || checkpoint.checkpoint_commit_seq < previous
        {
            return Err(CompositionRefusal::CheckpointOrder { index });
        }
        previous = checkpoint.checkpoint_commit_seq;
    }
    let generation = expected.generation;
    Ok(Composition {
        schema: COMPOSITION_SCHEMA,
        sequence: spec.sequence,
        embedding_model: generation.embedding_model.clone(),
        tokenizer_fingerprint: generation.tokenizer_fingerprint.clone(),
        vector_dimension: generation.vector_dimension,
        metric: expected.metric.name().to_owned(),
        unit_norm_tolerance: expected.unit_norm_tolerance,
        quantizer_recipe: expected.recipe.id().to_owned(),
        generation_epoch: generation.generation_epoch,
        kernel_incarnation_id: expected.kernel_incarnation_id.to_owned(),
        base: spec.base.digest.clone(),
        deltas: spec
            .deltas
            .iter()
            .map(|delta| delta.digest.clone())
            .collect(),
    })
}

/// A verified member still carries the sidecar it was verified with; comparing it again here binds the composition to that identity rather than to the caller's memory of it.
fn check_member(
    member: &VerifiedVectors,
    expected: &ExpectedVectors<'_>,
) -> Result<(), CompositionRefusal> {
    let generation = expected.generation;
    let sidecar = &member.sidecar;
    let checks: [(&'static str, bool); 6] = [
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
        (
            "recipe",
            ScalarRecipe::from_id(&sidecar.quantizer_recipe) == Some(expected.recipe)
                && Metric::from_name(&sidecar.metric) == Some(expected.metric)
                && sidecar.unit_norm_tolerance.to_bits() == expected.unit_norm_tolerance.to_bits(),
        ),
        (
            "generation_epoch",
            sidecar.generation_epoch == generation.generation_epoch,
        ),
        (
            "kernel_incarnation_id",
            sidecar.kernel_incarnation_id == expected.kernel_incarnation_id,
        ),
    ];
    match checks.into_iter().find(|(_, holds)| !holds) {
        Some((field, _)) => Err(CompositionRefusal::MemberIdentity {
            digest: member.digest.clone(),
            field,
        }),
        None => Ok(()),
    }
}

/// How far one publication attempt is known to have gone; each fact is recorded when its step returns, so a lost reply leaves the later ones `false` without implying they did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    pub digest: String,
    /// The composition generation is in the store.
    pub staged: bool,
    /// The selector rename returned, so a reader can see the new selection.
    pub acknowledged: bool,
    /// The containing directory sync returned.
    pub durable: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{refusal}")]
pub struct PublishFailure {
    pub publication: Publication,
    pub refusal: CompositionRefusal,
}

/// Stages `composition` through `staging` and points the vector selector at it. `observer` sees every selector event after this function has recorded it.
///
/// # Errors
///
/// A failure carries how far the attempt is known to have gone; a failure after `acknowledged` is an unknown outcome the caller settles with [`reconcile`].
pub fn publish(
    composition: &Composition,
    staging: &Staging<'_>,
    work_dir: &Path,
    observer: &mut dyn FnMut(ProfileEvent) -> Result<(), GenerationError>,
) -> Result<Publication, Box<PublishFailure>> {
    let mut publication = Publication {
        digest: composition.digest(),
        staged: false,
        acknowledged: false,
        durable: false,
    };
    let fail = |publication: &Publication, refusal| {
        Box::new(PublishFailure {
            publication: publication.clone(),
            refusal,
        })
    };
    let io = |error: std::io::Error| CompositionRefusal::Io(error.kind().to_string());
    std::fs::write(
        work_dir.join(COMPOSITION_FILE),
        composition.canonical_bytes(),
    )
    .and_then(|()| {
        std::fs::write(
            work_dir.join(MEMBERS_FILE_NAME),
            composition.members_bytes(),
        )
    })
    .map_err(|error| fail(&publication, io(error)))?;
    // Members were verified before composing; the store re-validates each on selection, so a member reclaimed meanwhile refuses the selection rather than exposing a partial set.
    let digest = staging
        .stage_manifest(
            &composition.stage_manifest(),
            &composition.stage_meta(),
            |path| work_dir.join(path),
        )
        .map_err(|refusal| fail(&publication, CompositionRefusal::Stage(refusal)))?;
    publication.staged = true;
    let selected = staging
        .store
        .select_vector(&digest, staging.transaction, &mut |event| {
            match event {
                ProfileEvent::AfterRename => publication.acknowledged = true,
                ProfileEvent::AfterDirectorySync => publication.durable = true,
                ProfileEvent::BeforeRename | ProfileEvent::BeforeDirectorySync => {}
            }
            observer(event)
        });
    match selected {
        Ok(()) => Ok(publication),
        Err(GenerationError::UnsupportedStateSchema) => {
            Err(fail(&publication, CompositionRefusal::Quarantined))
        }
        Err(error) => Err(fail(
            &publication,
            CompositionRefusal::Select(error.to_string()),
        )),
    }
}

/// What the selector durably names after an attempt with an unknown outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciled {
    /// The attempted composition is the selection; the attempt took effect.
    Published,
    /// Another composition, or none, is the selection; the attempt did not take effect and the prior selection stands complete.
    Prior(Option<String>),
    /// The selector carries an unknown schema; nothing can be concluded and nothing may be mutated.
    Quarantined,
}

/// Reads the selector back and syncs its directory, then decides by digest identity whether `intended` took effect.
pub fn reconcile(
    store: &GenerationStore,
    transaction: &LifecycleTransactionLock,
    intended: &str,
) -> Result<Reconciled, CompositionRefusal> {
    match store
        .reconcile_vector(transaction)
        .map_err(|error| CompositionRefusal::Select(error.to_string()))?
    {
        CurrentProfile::Current(current) if current == intended => Ok(Reconciled::Published),
        CurrentProfile::Current(current) => Ok(Reconciled::Prior(Some(current))),
        CurrentProfile::Absent => Ok(Reconciled::Prior(None)),
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

/// Verifies one composition generation and every member under `expected`.
///
/// # Errors
///
/// A generation of another owner or schema, a record that is not canonical or does not agree with its members file, a record whose identity is not `expected`, or a member that fails [`verify`].
pub fn verify_composition(
    store: &GenerationStore,
    digest: &str,
    expected: &ExpectedVectors<'_>,
) -> Result<VerifiedComposition, CompositionRefusal> {
    let generation = store.validate(digest).map_err(|error| match error {
        GenerationError::UnsupportedStateSchema => CompositionRefusal::Quarantined,
        other => CompositionRefusal::Select(other.to_string()),
    })?;
    if generation.manifest.target != VECTOR_SELECTION_TARGET {
        return Err(CompositionRefusal::NotComposition("manifest target"));
    }
    let bytes = read_verified(&generation, COMPOSITION_FILE)
        .map_err(|error| CompositionRefusal::Select(error.to_string()))?;
    let composition: Composition =
        serde_json::from_slice(&bytes).map_err(|_| CompositionRefusal::NotComposition("record"))?;
    if composition.schema != COMPOSITION_SCHEMA {
        return Err(CompositionRefusal::NotComposition("record schema"));
    }
    if composition.canonical_bytes() != bytes {
        return Err(CompositionRefusal::NotComposition("record not canonical"));
    }
    if composition.stage_manifest() != generation.manifest {
        return Err(CompositionRefusal::NotComposition("manifest binding"));
    }
    let members = generation
        .members()
        .map_err(|_| CompositionRefusal::NotComposition("members"))?;
    if members != composition.members() {
        return Err(CompositionRefusal::NotComposition(
            "members disagree with the record",
        ));
    }
    let generation_identity = expected.generation;
    let identity_holds = composition.embedding_model == generation_identity.embedding_model
        && composition.tokenizer_fingerprint == generation_identity.tokenizer_fingerprint
        && composition.vector_dimension == generation_identity.vector_dimension
        && Metric::from_name(&composition.metric) == Some(expected.metric)
        && composition.unit_norm_tolerance.to_bits() == expected.unit_norm_tolerance.to_bits()
        && ScalarRecipe::from_id(&composition.quantizer_recipe) == Some(expected.recipe)
        && composition.generation_epoch == generation_identity.generation_epoch
        && composition.kernel_incarnation_id == expected.kernel_incarnation_id;
    if !identity_holds {
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
    let spec = CompositionSpec {
        expected,
        sequence: composition.sequence,
        base: &base,
        deltas: &deltas,
        max_deltas: NonZeroUsize::new(deltas.len().max(1)).expect("at least one"),
    };
    if compose(&spec)? != composition {
        return Err(CompositionRefusal::NotComposition("topology"));
    }
    Ok(VerifiedComposition {
        digest: digest.to_owned(),
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
    /// Composition generations examined, newest first, before one verified.
    pub examined: usize,
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

/// Takes the selected composition when it verifies. Otherwise examines the store's other compositions newest first, at most `bound` of them, and takes the first that verifies. An acknowledged selection is never displaced by a newer composition that was staged but not selected.
///
/// # Errors
///
/// A quarantined selector or no verifying composition within the bound; neither mutates the store.
pub fn recover(
    store: &GenerationStore,
    expected: &ExpectedVectors<'_>,
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
        if let Ok(composition) = verify_composition(store, current, expected) {
            return Ok(Recovered {
                composition,
                selector: SelectorState::Current,
                examined,
            });
        }
    }
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for digest in store.digests().map_err(store_error)? {
        if selected.as_deref() == Some(digest.as_str()) {
            continue;
        }
        let Ok(generation) = store.validate(&digest) else {
            continue;
        };
        if generation.manifest.target != VECTOR_SELECTION_TARGET {
            continue;
        }
        let Ok(bytes) = read_verified(&generation, COMPOSITION_FILE) else {
            continue;
        };
        if let Ok(composition) = serde_json::from_slice::<Composition>(&bytes) {
            candidates.push((composition.sequence, digest));
        }
    }
    // Newest first; equal sequences fall back to digest order so the choice is deterministic.
    candidates.sort_by(|(left_sequence, left), (right_sequence, right)| {
        right_sequence
            .cmp(left_sequence)
            .then_with(|| right.cmp(left))
    });
    for (_, digest) in candidates
        .into_iter()
        .take(bound.get().saturating_sub(examined))
    {
        examined += 1;
        if let Ok(composition) = verify_composition(store, &digest, expected) {
            let selector = match &selected {
                Some(current) => SelectorState::Stale(current.clone()),
                None => SelectorState::Absent,
            };
            return Ok(Recovered {
                composition,
                selector,
                examined,
            });
        }
    }
    Err(Unavailable::NoCompatibleTarget { examined })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
