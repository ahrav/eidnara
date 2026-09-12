//! The fail-closed gate every projection hook consults before it runs. A hook is admitted only when the runtime manifest enables it and every evidence gate passes under the projection identity the daemon runs with; installing another manifest or identity cancels the grant's token and nothing further is admitted under the old evidence, while work already admitted keeps its owners until it joins.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use kernel::source_identity::OccurrenceClass;
use retrieval::ProjectionIdentity;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::coverage::ProjectionCoverage;

/// Every product hook the projection has, with the classes each one touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectionHook {
    MessageCleanup,
    EmbeddingBootstrap,
    EmbeddingRouting,
    EmbeddingRegistry,
    EmbeddingBackfill,
    EmbeddingIdentityGc,
    PromotedMemoryEmbeddings,
    GitIngest,
    GitDurableRows,
    GitJobs,
    GitSweeps,
    GitLeases,
}

const DENSE: [OccurrenceClass; 4] = [
    OccurrenceClass::Messages,
    OccurrenceClass::CanonicalClaims,
    OccurrenceClass::PromotedMemory,
    OccurrenceClass::GitCommits,
];

impl ProjectionHook {
    pub const ALL: [ProjectionHook; 12] = [
        Self::MessageCleanup,
        Self::EmbeddingBootstrap,
        Self::EmbeddingRouting,
        Self::EmbeddingRegistry,
        Self::EmbeddingBackfill,
        Self::EmbeddingIdentityGc,
        Self::PromotedMemoryEmbeddings,
        Self::GitIngest,
        Self::GitDurableRows,
        Self::GitJobs,
        Self::GitSweeps,
        Self::GitLeases,
    ];

    /// The manifest key of the hook.
    pub fn id(self) -> &'static str {
        match self {
            Self::MessageCleanup => "search_projection.message_cleanup",
            Self::EmbeddingBootstrap => "search_projection.embedding.bootstrap",
            Self::EmbeddingRouting => "search_projection.embedding.routing",
            Self::EmbeddingRegistry => "search_projection.embedding.registry",
            Self::EmbeddingBackfill => "search_projection.embedding.backfill",
            Self::EmbeddingIdentityGc => "search_projection.embedding.identity_gc",
            Self::PromotedMemoryEmbeddings => "search_projection.promoted_memory.embeddings",
            Self::GitIngest => "search_projection.git.ingest",
            Self::GitDurableRows => "search_projection.git.durable_rows",
            Self::GitJobs => "search_projection.git.jobs",
            Self::GitSweeps => "search_projection.git.sweeps",
            Self::GitLeases => "search_projection.git.leases",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|hook| hook.id() == id)
    }

    /// The classes whose coverage the hook needs before it may run.
    pub fn classes(self) -> &'static [OccurrenceClass] {
        match self {
            Self::MessageCleanup => &[OccurrenceClass::Messages],
            Self::EmbeddingBootstrap
            | Self::EmbeddingRouting
            | Self::EmbeddingRegistry
            | Self::EmbeddingBackfill
            | Self::EmbeddingIdentityGc => &DENSE,
            Self::PromotedMemoryEmbeddings => &[OccurrenceClass::PromotedMemory],
            Self::GitIngest
            | Self::GitDurableRows
            | Self::GitJobs
            | Self::GitSweeps
            | Self::GitLeases => &[OccurrenceClass::GitCommits],
        }
    }
}

/// Where a hook asks for admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryPoint {
    Startup,
    Reload,
    Dispatch,
    Explicit,
}

impl EntryPoint {
    pub const ALL: [EntryPoint; 4] = [Self::Startup, Self::Reload, Self::Dispatch, Self::Explicit];

    /// The contract's name for the entry point.
    pub fn id(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Reload => "reload",
            Self::Dispatch => "dispatch",
            Self::Explicit => "explicit",
        }
    }
}

/// The gates a denial names. The capability gate denies through [`Denial::Unsupported`], which names the harness and capability instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    ClassCoverage,
    Freshness,
    Resource,
    BothHarness,
}

/// The part of the projection identity a grant is bound to. The kernel incarnation is excluded: it changes on every kernel restart while the evidence about the projection's content, model, and limits stays valid; every other field invalidates the evidence when it changes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvalidationIdentity {
    pub schema_version: u32,
    pub tokenizer_fingerprint: String,
    pub embedding_model: String,
    pub projection_policy_version: String,
    pub identity_contract_version: String,
    pub limit_manifest_protocol_version: String,
    pub vector_dimension: u32,
    pub generation_epoch: u64,
}

impl From<&ProjectionIdentity> for InvalidationIdentity {
    // Destructured without `..` so a field added to the projection identity is an explicit include-or-exclude decision here rather than a silent exclusion from invalidation.
    fn from(identity: &ProjectionIdentity) -> Self {
        let ProjectionIdentity {
            schema_version,
            kernel_incarnation_id: _,
            projection_policy_version,
            identity_contract_version,
            limit_manifest_protocol_version,
            embedding_model,
            tokenizer_fingerprint,
            vector_dimension,
            generation_epoch,
        } = identity;
        Self {
            schema_version: *schema_version,
            tokenizer_fingerprint: tokenizer_fingerprint.clone(),
            embedding_model: embedding_model.clone(),
            projection_policy_version: projection_policy_version.clone(),
            identity_contract_version: identity_contract_version.clone(),
            limit_manifest_protocol_version: limit_manifest_protocol_version.clone(),
            vector_dimension: *vector_dimension,
            generation_epoch: *generation_epoch,
        }
    }
}

/// The limits a runtime manifest carries, all as nonnegative integers. The gates read `catchup_lag_commits` and `decoded_heap_high_water_bytes`; the rest bound the slices the hooks run.
pub const REQUIRED_LIMITS: [&str; 27] = [
    "export_page_rows",
    "export_page_encoded_bytes",
    "export_row_standalone_encoded_bytes",
    "export_live_decoded_bytes",
    "catchup_batch_commits",
    "catchup_batch_encoded_bytes",
    "catchup_batch_source_bytes",
    "local_transaction_rows",
    "local_transaction_bytes",
    "pending_count",
    "pending_bytes",
    "embedding_input_bytes",
    "embedding_input_tokens",
    "supervisor_slice_ms",
    "retry_attempts",
    "lease_duration_ms",
    "capture_reference_count",
    "capture_hold_expiry_ms",
    "capture_disk_bytes",
    "catchup_lag_commits",
    "B_catchup_ms",
    "B_recovery_ms",
    "B_authorized_recovery_ms",
    "embedding_recovery_attempts",
    "query_service_opportunities",
    "physical_drain_ms",
    "decoded_heap_high_water_bytes",
];

const MANIFEST_FIELDS: [&str; 4] = [
    "protocol_version",
    "invalidation_identity",
    "limits",
    "hooks",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestRefusal {
    #[error("the manifest is not an object of exactly the runtime fields")]
    Shape,
    #[error("the manifest carries no protocol version")]
    MissingProtocolVersion,
    #[error("the manifest's protocol version {manifest} is not its identity's {identity}")]
    ProtocolMismatch { manifest: String, identity: String },
    #[error("limit {0} is missing")]
    MissingLimit(String),
    #[error("limit {0} is not a nonnegative integer")]
    NonNumericLimit(String),
    #[error("{0} is not a runtime limit")]
    UnknownLimit(String),
    #[error("{0} is not a projection hook")]
    UnknownHook(String),
    #[error("hook {0} carries a flag that is not a boolean")]
    MalformedFlag(String),
}

/// What product code reads from the manifest: nothing about the campaign that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeManifest {
    pub protocol_version: String,
    pub identity: InvalidationIdentity,
    pub limits: BTreeMap<String, u64>,
    pub enabled: BTreeMap<ProjectionHook, bool>,
}

impl RuntimeManifest {
    /// Parses the runtime schema: `protocol_version`, `invalidation_identity`, `limits`, and `hooks`, each hook an object of one boolean `enabled`. A changed limit ships under a new protocol version, so a version that is not the identity's is refused rather than applied to evidence gathered under the old limits.
    ///
    /// # Errors
    ///
    /// Any other top-level field, a missing or malformed identity, a missing, non-integer, or unknown limit, an unknown hook, or a non-boolean flag is a [`ManifestRefusal`].
    pub fn parse(value: &Value) -> Result<Self, ManifestRefusal> {
        let object = value.as_object().ok_or(ManifestRefusal::Shape)?;
        if object
            .keys()
            .any(|key| !MANIFEST_FIELDS.contains(&key.as_str()))
        {
            return Err(ManifestRefusal::Shape);
        }
        let protocol_version = object
            .get("protocol_version")
            .and_then(Value::as_str)
            .filter(|version| !version.is_empty())
            .ok_or(ManifestRefusal::MissingProtocolVersion)?
            .to_owned();
        let identity: InvalidationIdentity = object
            .get("invalidation_identity")
            .cloned()
            .and_then(|identity| serde_json::from_value(identity).ok())
            .ok_or(ManifestRefusal::Shape)?;
        if identity.limit_manifest_protocol_version != protocol_version {
            return Err(ManifestRefusal::ProtocolMismatch {
                manifest: protocol_version,
                identity: identity.limit_manifest_protocol_version,
            });
        }
        let limits_object = object
            .get("limits")
            .and_then(Value::as_object)
            .ok_or(ManifestRefusal::Shape)?;
        let mut limits = BTreeMap::new();
        for name in REQUIRED_LIMITS {
            let value = limits_object
                .get(name)
                .ok_or_else(|| ManifestRefusal::MissingLimit(name.to_owned()))?;
            let limit = value
                .as_u64()
                .ok_or_else(|| ManifestRefusal::NonNumericLimit(name.to_owned()))?;
            limits.insert(name.to_owned(), limit);
        }
        if let Some(extra) = limits_object
            .keys()
            .find(|key| !REQUIRED_LIMITS.contains(&key.as_str()))
        {
            return Err(ManifestRefusal::UnknownLimit(extra.clone()));
        }
        let hooks_object = object
            .get("hooks")
            .and_then(Value::as_object)
            .ok_or(ManifestRefusal::Shape)?;
        let mut enabled = BTreeMap::new();
        for (id, entry) in hooks_object {
            let hook = ProjectionHook::from_id(id)
                .ok_or_else(|| ManifestRefusal::UnknownHook(id.clone()))?;
            let flag = entry
                .as_object()
                .filter(|entry| entry.keys().all(|key| key == "enabled"))
                .and_then(|entry| entry.get("enabled"))
                .and_then(Value::as_bool)
                .ok_or_else(|| ManifestRefusal::MalformedFlag(id.clone()))?;
            enabled.insert(hook, flag);
        }
        Ok(Self {
            protocol_version,
            identity,
            limits,
            enabled,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityEvidence {
    Supported,
    Unsupported,
}

/// Whether a harness must prove the capability or must prove it stays off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDisposition {
    Required,
    OptionalDisabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    pub disposition: CapabilityDisposition,
}

/// Every hook checks every row for both harnesses: a projection whose raw spans, sessions, or repository scopes were captured wrong is wrong for every class it serves.
pub const CAPABILITIES: [Capability; 8] = [
    Capability {
        name: "message_text_block_capture",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "durable_session_identity",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "tool_result_native_string_capture",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "tool_result_revision_identity",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "multipart_error_result_capture",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "git_repository_scope_declaration",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "projection_hook_activation",
        disposition: CapabilityDisposition::Required,
    },
    Capability {
        name: "dense_raw_tool_embedding",
        disposition: CapabilityDisposition::OptionalDisabled,
    },
];

pub const HARNESSES: [&str; 2] = kernel::source_identity::HARNESSES;

/// Observers whose decoded-heap high-water measurement the resource gate accepts. Logical admission charges are accounting, not a measurement, and no charge observer is named here.
pub const APPROVED_OBSERVERS: [&str; 1] = ["rp2.9.decoded-heap-high-water"];

/// A live decoded-heap high-water measurement and the observer that took it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEvidence {
    pub observer: String,
    pub decoded_heap_high_water_bytes: u64,
}

/// One harness's full-path run under one identity. A failed run carries no text: the denial names the harness and the campaign record holds the cause (CC11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessRun {
    Passed { identity: InvalidationIdentity },
    Failed,
}

/// Everything the gates read, gathered under `identity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub identity: InvalidationIdentity,
    pub coverage: Option<ProjectionCoverage>,
    pub resource: Option<ResourceEvidence>,
    /// `(harness, capability)` to what the harness proved.
    pub capabilities: BTreeMap<(String, String), CapabilityEvidence>,
    pub harness_runs: BTreeMap<String, HarnessRun>,
}

/// Why a hook was denied. Variants name gates, hooks, harnesses, class codes, and sizes, never content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Denial {
    #[error("no manifest is installed")]
    NoManifest,
    #[error("hook {} is not enabled", .0.id())]
    Disabled(ProjectionHook),
    #[error("the manifest's identity is not the projection's")]
    ManifestIdentity,
    #[error("the evidence was gathered under another identity")]
    EvidenceIdentity,
    #[error("{0:?} evidence is missing")]
    Missing(Gate),
    #[error("{0:?} evidence failed: {1}")]
    Failed(Gate, String),
    #[error("the coverage observation trails the kernel by {lag} commits, above {max}")]
    Stale { lag: i64, max: u64 },
    #[error("observer {0} is not an approved decoded-heap observer")]
    UnapprovedObserver(String),
    #[error("{harness} does not satisfy capability {capability}")]
    Unsupported { harness: String, capability: String },
    #[error("{limit} observed at {observed}, above {max}")]
    LimitExceeded {
        limit: String,
        observed: u64,
        max: u64,
    },
}

/// A grant. `invalidated` fires when the gate's manifest or identity changes after the grant; the holder cancels its budget and lets admitted work join.
#[derive(Debug, Clone)]
pub struct Admission {
    pub hook: ProjectionHook,
    pub entry: EntryPoint,
    pub invalidated: CancellationToken,
}

/// One manifest, the projection identity the daemon runs with, and one evidence packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceEvaluator {
    pub manifest: RuntimeManifest,
    pub current: InvalidationIdentity,
    pub evidence: Evidence,
}

impl EvidenceEvaluator {
    /// Judges `hook`: its flag, the manifest's and the evidence's identity, then class coverage, freshness, resource, capability, and both-harness evidence. The first failure is the denial.
    ///
    /// # Errors
    ///
    /// Returns the [`Denial`].
    pub fn judge(&self, hook: ProjectionHook) -> Result<(), Denial> {
        if !self.manifest.enabled.get(&hook).copied().unwrap_or(false) {
            return Err(Denial::Disabled(hook));
        }
        if self.manifest.identity != self.current {
            return Err(Denial::ManifestIdentity);
        }
        if self.evidence.identity != self.current {
            return Err(Denial::EvidenceIdentity);
        }
        let coverage = self
            .evidence
            .coverage
            .as_ref()
            .ok_or(Denial::Missing(Gate::ClassCoverage))?;
        self.class_coverage(hook, coverage)?;
        self.freshness(coverage)?;
        self.resource()?;
        self.capability()?;
        self.both_harness()
    }

    fn limit(&self, name: &str) -> Result<u64, Denial> {
        self.manifest
            .limits
            .get(name)
            .copied()
            .ok_or_else(|| Denial::Failed(Gate::Resource, format!("limit {name} is absent")))
    }

    fn class_coverage(
        &self,
        hook: ProjectionHook,
        coverage: &ProjectionCoverage,
    ) -> Result<(), Denial> {
        // The report carries the projection identity it was read under and the registered generation its counts were taken for; a report for an older generation still names the current identity, so both are bound.
        let generation = &coverage.report.generation;
        if InvalidationIdentity::from(&coverage.report.identity) != self.current
            || generation.embedding_model != self.current.embedding_model
            || generation.tokenizer_fingerprint != self.current.tokenizer_fingerprint
            || generation.vector_dimension != self.current.vector_dimension
            || generation.generation_epoch != self.current.generation_epoch
        {
            return Err(Denial::EvidenceIdentity);
        }
        for class in hook.classes() {
            if !coverage
                .report
                .classes
                .iter()
                .any(|reported| reported.class == *class)
            {
                return Err(Denial::Failed(
                    Gate::ClassCoverage,
                    format!("class {} is not reported", class.code()),
                ));
            }
        }
        Ok(())
    }

    // The tip is the one the coverage packet was judged at, so the lag describes that observation and not a tip read elsewhere.
    fn freshness(&self, coverage: &ProjectionCoverage) -> Result<(), Denial> {
        let max = self.limit("catchup_lag_commits")?;
        let lag = coverage
            .kernel_snapshot
            .tip
            .checked_sub(coverage.report.checkpoint.checkpoint_commit_seq)
            .ok_or(Denial::Failed(
                Gate::Freshness,
                "the lag is outside the commit-sequence domain".to_owned(),
            ))?;
        match u64::try_from(lag) {
            Ok(lag) if lag <= max => Ok(()),
            _ => Err(Denial::Stale { lag, max }),
        }
    }

    fn resource(&self) -> Result<(), Denial> {
        let resource = self
            .evidence
            .resource
            .as_ref()
            .ok_or(Denial::Missing(Gate::Resource))?;
        if !APPROVED_OBSERVERS.contains(&resource.observer.as_str()) {
            return Err(Denial::UnapprovedObserver(resource.observer.clone()));
        }
        let max = self.limit("decoded_heap_high_water_bytes")?;
        if resource.decoded_heap_high_water_bytes > max {
            return Err(Denial::LimitExceeded {
                limit: "decoded_heap_high_water_bytes".to_owned(),
                observed: resource.decoded_heap_high_water_bytes,
                max,
            });
        }
        Ok(())
    }

    fn capability(&self) -> Result<(), Denial> {
        for capability in CAPABILITIES {
            for harness in HARNESSES {
                let proved = self
                    .evidence
                    .capabilities
                    .get(&(harness.to_owned(), capability.name.to_owned()))
                    .copied();
                let ok = match capability.disposition {
                    CapabilityDisposition::Required => {
                        proved == Some(CapabilityEvidence::Supported)
                    }
                    // Proof that the optional capability is off or no proof at all keeps it off; proof that it runs is a mismatch.
                    CapabilityDisposition::OptionalDisabled => {
                        proved != Some(CapabilityEvidence::Supported)
                    }
                };
                if !ok {
                    return Err(Denial::Unsupported {
                        harness: harness.to_owned(),
                        capability: capability.name.to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    fn both_harness(&self) -> Result<(), Denial> {
        for harness in HARNESSES {
            match self.evidence.harness_runs.get(harness) {
                Some(HarnessRun::Passed { identity }) if *identity == self.current => {}
                Some(HarnessRun::Passed { .. }) => return Err(Denial::EvidenceIdentity),
                Some(HarnessRun::Failed) => {
                    return Err(Denial::Failed(Gate::BothHarness, harness.to_owned()));
                }
                None => return Err(Denial::Missing(Gate::BothHarness)),
            }
        }
        Ok(())
    }
}

/// One admission request the gate judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    pub hook: ProjectionHook,
    pub entry: EntryPoint,
    pub verdict: Result<(), Denial>,
}

struct GateState {
    evaluator: Option<EvidenceEvaluator>,
    /// The token every grant under `evaluator` carries; replaced with the evaluator.
    invalidated: CancellationToken,
}

/// The shared gate every hook consults. It starts closed. `install` and `close` cancel the previous grant's token before the new state is visible, and a group of hooks is judged under one state, so no grant spans two manifests.
pub struct HookGate {
    state: Mutex<GateState>,
    #[cfg(feature = "test-support")]
    ledger: Mutex<Vec<LedgerEntry>>,
}

impl HookGate {
    /// A gate with no manifest denies every hook.
    pub fn closed() -> Self {
        Self {
            state: Mutex::new(GateState {
                evaluator: None,
                invalidated: CancellationToken::new(),
            }),
            #[cfg(feature = "test-support")]
            ledger: Mutex::new(Vec::new()),
        }
    }

    pub fn install(&self, evaluator: EvidenceEvaluator) {
        self.replace(Some(evaluator));
    }

    /// Removes the evaluator: nothing is admitted until another is installed.
    pub fn close(&self) {
        self.replace(None);
    }

    fn replace(&self, evaluator: Option<EvidenceEvaluator>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.invalidated.cancel();
        state.invalidated = CancellationToken::new();
        state.evaluator = evaluator;
    }

    /// Every request judged so far, oldest first.
    #[cfg(feature = "test-support")]
    pub fn ledger(&self) -> Vec<LedgerEntry> {
        self.ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Admits all hooks at `entry` when each judge succeeds under the same gate state. A poisoned gate denies as a closed one does, and so does an empty request.
    ///
    /// # Errors
    ///
    /// Returns the first [`Denial`] in `hooks` order; every hook is still judged.
    pub fn admit_all(
        &self,
        hooks: &[ProjectionHook],
        entry: EntryPoint,
    ) -> Result<Vec<Admission>, Denial> {
        if hooks.is_empty() {
            return Err(Denial::NoManifest);
        }
        let state: Option<MutexGuard<'_, GateState>> = self.state.lock().ok();
        let verdicts: Vec<Result<Admission, Denial>> = hooks
            .iter()
            .map(|hook| {
                let state = state.as_deref().ok_or(Denial::NoManifest)?;
                let evaluator = state.evaluator.as_ref().ok_or(Denial::NoManifest)?;
                evaluator.judge(*hook)?;
                Ok(Admission {
                    hook: *hook,
                    entry,
                    invalidated: state.invalidated.clone(),
                })
            })
            .collect();
        drop(state);
        #[cfg(feature = "test-support")]
        self.ledger
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend(
                hooks
                    .iter()
                    .zip(&verdicts)
                    .map(|(hook, verdict)| LedgerEntry {
                        hook: *hook,
                        entry,
                        verdict: verdict.as_ref().map(|_| ()).map_err(Clone::clone),
                    }),
            );
        verdicts.into_iter().collect()
    }

    /// # Errors
    ///
    /// Returns the [`Denial`] of `hook`.
    pub fn admit(&self, hook: ProjectionHook, entry: EntryPoint) -> Result<Admission, Denial> {
        self.admit_all(&[hook], entry)
            .map(|mut admissions| admissions.remove(0))
    }
}
