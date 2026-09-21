use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use context_core::canonical_json::{
    ContractError, canonical_json_encode, is_lower_hex, protocol_digest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::census::{Construction, EvaluatedSurface, Reachability};
use crate::identity::{IdentityError, RunIdentity, eval_run_id};
use crate::pairs::{RECENCY_BASELINE_VERSION, recency_bound};
use crate::residue::{ObservationSchema, RelativeDomains, ResidueEntry, ResidueError, Rule};

pub const MANIFEST_SCHEMA: &str = "eval-manifest/v8";
pub const MANIFEST_DIGEST_PROTOCOL: &str = "eval-manifest-digest/v8";

/// Sorted; a field added to [`Manifest`] without a schema version bump fails the closure test.
pub const REQUIRED_FIELDS: [&str; 30] = [
    "analysis_family_digest",
    "arm_rates",
    "attestation",
    "claim_boundary",
    "component_versions",
    "construction",
    "cut_receipts",
    "end_ms",
    "envelope_bounds",
    "envelope_peaks",
    "error",
    "eval_run_id",
    "execution_mode",
    "failure_class_table_digest",
    "ingestion",
    "memory_reviewer_model_calls",
    "reachability",
    "recency_baseline",
    "residue",
    "result_digest",
    "retry_lineage",
    "run_identity",
    "sample_epoch",
    "sample_ids",
    "sample_order",
    "schema",
    "start_ms",
    "status",
    "tokenizer_profile",
    "witness_digest",
];

/// Wall-clock stamps and measured peaks remain in the manifest but are excluded from its digest.
pub const DROPPED_FIELDS: [&str; 3] = ["end_ms", "envelope_peaks", "start_ms"];

pub const CLAIM_BOUNDARY_SCHEMA: &str = "claim-boundary/v1";
pub const CLAIM_BOUNDARY_EXCLUSIONS: [&str; 4] = [
    "scheduler-order independence",
    "power-loss durability",
    "wall-clock retention behavior under manipulated age",
    "live-model quality",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub eval_run_id: String,
    /// The frozen analysis family a paired campaign's results are read under,
    /// recorded before the first outcome; `None` for a run that reports no
    /// paired statistics.
    pub analysis_family_digest: Option<String>,
    pub run_identity: RunIdentity,
    pub start_ms: i64,
    pub end_ms: i64,
    pub status: RunStatus,
    pub error: Option<String>,
    pub sample_ids: Vec<String>,
    pub sample_order: Vec<String>,
    pub sample_epoch: u64,
    pub retry_lineage: Vec<String>,
    pub result_digest: String,
    pub witness_digest: String,
    pub attestation: Attestation,
    pub tokenizer_profile: TokenizerProfile,
    pub cut_receipts: Vec<CutReceipt>,
    pub residue: BTreeSet<ResidueEntry>,
    pub construction: Construction,
    pub execution_mode: ExecutionMode,
    /// The failure-class truth table the run's classes come from; must equal
    /// [`crate::FAILURE_CLASS_TABLE_DIGEST`].
    pub failure_class_table_digest: String,
    pub ingestion: Ingestion,
    pub memory_reviewer_model_calls: MemoryReviewerModelCalls,
    pub reachability: Reachability,
    /// The recency-only baseline a paired campaign's falsification pairs
    /// were checked against, with its window per surface; `None` for a run
    /// that compiled no pair set.
    pub recency_baseline: Option<RecencyBaseline>,
    pub claim_boundary: ClaimBoundary,
    pub component_versions: ComponentVersions,
    pub envelope_bounds: ResourceLimits,
    pub envelope_peaks: ResourceLimits,
    pub arm_rates: BTreeMap<String, ArmRates>,
}

/// The control's version and the most-recent-k window it read on each surface,
/// recorded so both clauses of stop condition (b) are judged against one
/// declared baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecencyBaseline {
    pub version: String,
    pub bounds: BTreeMap<EvaluatedSurface, u32>,
}

/// How the world was driven: generated, replayed from a tape, or enumerated
/// over fact tuples for the reducer differential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Generate,
    ReplayTape,
    Enumerate,
}

/// How the world's units reached the store. No ingestion adapter has a
/// production caller, so adapter ingestion is labelled as such and never as
/// validated real ingestion; the direct-database path serves only non-aged
/// fixtures; the transform route is the harness's own path, one turn at a
/// time through one store incarnation, the honest label for an arm the
/// daemon built itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ingestion {
    #[serde(rename = "adapter-ingested, production caller: none")]
    AdapterIngestedNoProductionCaller,
    #[serde(rename = "direct-database, non-aged")]
    DirectDatabaseNonAged,
    #[serde(rename = "transform-route, turn by turn")]
    TransformRouteTurnByTurn,
}

/// MemoryReviewer model traffic bypasses `LlmExecutionBackend`, so a run
/// either replays it through the keyed TLS peer or declares the reviewer
/// excluded; a manifest without this declaration is refused as any missing
/// field is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryReviewerModelCalls {
    Cassette,
    Excluded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Incomplete,
    Refused,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Attestation {
    None,
    Signed {
        signer: String,
        signature_digest: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenizerProfile {
    pub name: String,
    pub revision: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CutReceipt {
    pub cut: Cut,
    pub outcome: CutOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cut {
    AfterAtomicTransition,
    AtQuiescence,
    AfterRecovery,
    AfterFaultPhase,
    EndOfRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutOutcome {
    Reached,
    NotReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimBoundary {
    pub schema: String,
    pub exclusions: Vec<String>,
}

impl ClaimBoundary {
    pub fn pinned() -> Self {
        Self {
            schema: CLAIM_BOUNDARY_SCHEMA.to_string(),
            exclusions: CLAIM_BOUNDARY_EXCLUSIONS
                .iter()
                .map(|text| text.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentVersions {
    pub generator: String,
    pub event_schema: String,
    pub reducer: String,
    pub oracles: String,
    pub execution_image: String,
    pub task_corpus: String,
    pub judge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    pub elapsed_ms: u64,
    pub store_bytes: u64,
    pub cassette_bytes: u64,
    pub artifact_bytes: u64,
    pub temp_roots: u64,
    pub retained_artifacts: u64,
    pub processes: u64,
}

/// Rates are canonical decimal strings; canonical JSON has no fractional numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmRates {
    pub miss_rate: String,
    pub refusal_rate: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    NotAnObject,
    MissingField(String),
    UnknownField(String),
    SchemaMismatch {
        found: String,
    },
    Shape(String),
    RunIdMismatch {
        declared: String,
        derived: String,
    },
    GeneratorVersionMismatch,
    ClaimBoundaryMismatch,
    FailureClassTableMismatch {
        found: String,
    },
    DirectDatabaseAged,
    ResidueIncomplete {
        field: String,
    },
    ResidueContradiction {
        type_name: String,
        field: String,
    },
    SampleOrderNotAPermutation,
    MalformedDigest {
        field: String,
    },
    MalformedDecimal {
        field: String,
        value: String,
    },
    RateOutOfRange {
        field: String,
        value: String,
    },
    EmptyComponent {
        field: String,
    },
    /// The recorded recency baseline is not the one the compiler enforces.
    RecencyBaselineMismatch {
        field: &'static str,
    },
    Identity(IdentityError),
    Residue(ResidueError),
    NotCanonical(ContractError),
}

/// Variant names and fields are the message; callers match on the variant.
impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for ManifestError {}

impl From<IdentityError> for ManifestError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}

impl From<ResidueError> for ManifestError {
    fn from(error: ResidueError) -> Self {
        Self::Residue(error)
    }
}

impl From<ContractError> for ManifestError {
    fn from(error: ContractError) -> Self {
        Self::NotCanonical(error)
    }
}

/// One encoding per value: unsigned digits, no leading zero unless the integer
/// part is `0`, and a fraction with no trailing zero: `0`, `12`, `0.25`.
pub fn is_canonical_decimal(text: &str) -> bool {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let (integer, fraction) = match text.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (text, None),
    };
    digits(integer)
        && (integer == "0" || !integer.starts_with('0'))
        && fraction.is_none_or(|fraction| digits(fraction) && !fraction.ends_with('0'))
}

pub fn parse_manifest(value: &Value) -> Result<Manifest, ManifestError> {
    let fields = value.as_object().ok_or(ManifestError::NotAnObject)?;
    let present: BTreeSet<&str> = fields.keys().map(String::as_str).collect();
    let required: BTreeSet<&str> = REQUIRED_FIELDS.into_iter().collect();
    if let Some(field) = required.difference(&present).next() {
        return Err(ManifestError::MissingField(field.to_string()));
    }
    if let Some(field) = present.difference(&required).next() {
        return Err(ManifestError::UnknownField(field.to_string()));
    }
    match fields["schema"].as_str() {
        Some(MANIFEST_SCHEMA) => {}
        found => {
            return Err(ManifestError::SchemaMismatch {
                found: found.unwrap_or_default().to_string(),
            });
        }
    }
    let manifest: Manifest = serde_json::from_value(value.clone())
        .map_err(|error| ManifestError::Shape(error.to_string()))?;
    manifest.validate()?;
    Ok(manifest)
}

impl Manifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(ManifestError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        // Digestible on both runtimes: no integer may leave the canonical safe range.
        canonical_json_encode(&self.to_value())?;
        let derived = eval_run_id(&self.run_identity)?;
        if derived != self.eval_run_id {
            return Err(ManifestError::RunIdMismatch {
                declared: self.eval_run_id.clone(),
                derived,
            });
        }
        if self.component_versions.generator != self.run_identity.generator_version {
            return Err(ManifestError::GeneratorVersionMismatch);
        }
        if self.claim_boundary != ClaimBoundary::pinned() {
            return Err(ManifestError::ClaimBoundaryMismatch);
        }
        if self.failure_class_table_digest != crate::failure_class::FAILURE_CLASS_TABLE_DIGEST {
            return Err(ManifestError::FailureClassTableMismatch {
                found: self.failure_class_table_digest.clone(),
            });
        }
        if self.ingestion == Ingestion::DirectDatabaseNonAged
            && self.construction == Construction::Replay
        {
            return Err(ManifestError::DirectDatabaseAged);
        }
        for entry in Self::field_schema().residue() {
            if !self.residue.contains(&entry) {
                return Err(ManifestError::ResidueIncomplete { field: entry.field });
            }
        }
        // Residue lists only non-`Keep` rules, one per `(type_name, field)`.
        let mut classified = BTreeSet::new();
        for entry in &self.residue {
            if entry.rule == Rule::Keep
                || !classified.insert((entry.type_name.as_str(), entry.field.as_str()))
            {
                return Err(ManifestError::ResidueContradiction {
                    type_name: entry.type_name.clone(),
                    field: entry.field.clone(),
                });
            }
        }
        let ids: BTreeSet<&str> = self.sample_ids.iter().map(String::as_str).collect();
        let ordered: BTreeSet<&str> = self.sample_order.iter().map(String::as_str).collect();
        if ids.len() != self.sample_ids.len()
            || ordered.len() != self.sample_order.len()
            || ids != ordered
        {
            return Err(ManifestError::SampleOrderNotAPermutation);
        }
        let mut digests = vec![
            ("result_digest".to_string(), &self.result_digest),
            ("witness_digest".to_string(), &self.witness_digest),
            (
                "tokenizer_profile.digest".to_string(),
                &self.tokenizer_profile.digest,
            ),
        ];
        if let Attestation::Signed {
            signature_digest, ..
        } = &self.attestation
        {
            digests.push(("attestation.signature_digest".to_string(), signature_digest));
        }
        for (index, prior) in self.retry_lineage.iter().enumerate() {
            digests.push((format!("retry_lineage[{index}]"), prior));
        }
        if let Some(digest) = &self.analysis_family_digest {
            digests.push(("analysis_family_digest".to_string(), digest));
        }
        for (field, digest) in digests {
            if !is_lower_hex(digest, 64) {
                return Err(ManifestError::MalformedDigest { field });
            }
        }
        for (arm, rates) in &self.arm_rates {
            for (field, rate) in [
                ("miss_rate", &rates.miss_rate),
                ("refusal_rate", &rates.refusal_rate),
            ] {
                let field = format!("arm_rates[{arm}].{field}");
                if !is_canonical_decimal(rate) {
                    return Err(ManifestError::MalformedDecimal {
                        field,
                        value: rate.clone(),
                    });
                }
                // Canonical, so within [0, 1] means exactly `0`, `1`, or `0.<digits>`.
                if rate != "1" && !rate.starts_with('0') {
                    return Err(ManifestError::RateOutOfRange {
                        field,
                        value: rate.clone(),
                    });
                }
            }
        }
        let versions = &self.component_versions;
        let signer = match &self.attestation {
            Attestation::Signed { signer, .. } => signer,
            Attestation::None => "-",
        };
        for (field, text) in [
            (
                "component_versions.event_schema",
                versions.event_schema.as_str(),
            ),
            ("component_versions.reducer", &versions.reducer),
            ("component_versions.oracles", &versions.oracles),
            (
                "component_versions.execution_image",
                &versions.execution_image,
            ),
            ("component_versions.task_corpus", &versions.task_corpus),
            ("component_versions.judge", &versions.judge),
            ("tokenizer_profile.name", &self.tokenizer_profile.name),
            (
                "tokenizer_profile.revision",
                &self.tokenizer_profile.revision,
            ),
            ("attestation.signer", signer),
        ] {
            if text.is_empty() {
                return Err(ManifestError::EmptyComponent {
                    field: field.to_string(),
                });
            }
        }
        if let Some(baseline) = &self.recency_baseline {
            if baseline.version != RECENCY_BASELINE_VERSION {
                return Err(ManifestError::RecencyBaselineMismatch { field: "version" });
            }
            if baseline.bounds.is_empty() {
                return Err(ManifestError::RecencyBaselineMismatch { field: "bounds" });
            }
            for (surface, bound) in &baseline.bounds {
                let declared = std::num::NonZeroU32::new(*bound);
                if recency_bound(*surface, declared) != Ok(*bound) {
                    return Err(ManifestError::RecencyBaselineMismatch { field: "bounds" });
                }
            }
        }
        Ok(())
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("manifest serializes")
    }

    /// Manifest residue rules keep every field except [`DROPPED_FIELDS`].
    pub fn field_schema() -> ObservationSchema {
        ObservationSchema::new(
            MANIFEST_SCHEMA,
            REQUIRED_FIELDS.into_iter().map(|field| {
                let rule = if DROPPED_FIELDS.contains(&field) {
                    Rule::Drop
                } else {
                    Rule::Keep
                };
                (field, rule)
            }),
        )
        .expect("manifest field rules pass the clock gate")
    }

    /// Re-parses first, so a manifest built in code is refused on the same terms as one read from disk.
    pub fn digest(&self) -> Result<String, ManifestError> {
        let value = self.to_value();
        parse_manifest(&value)?;
        let reduced = Self::field_schema().reduce(&value, &mut RelativeDomains::default())?;
        Ok(protocol_digest(MANIFEST_DIGEST_PROTOCOL, &reduced)?)
    }
}
