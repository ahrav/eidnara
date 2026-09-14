//! The daemon reads `runtime-manifest.json` and `campaign-evidence.json` from `<home>/search-admission/` to renew one [`HookGate`] for the selected projection. Coverage is not a record: the daemon observes it on that projection and supplies it at every refresh. No selected projection, a missing or refused record, or a closed owner leaves the gate closed. [`HookGate::renew`] keeps the grants the new evidence still admits and cancels the hooks it withdraws; neither record enables a hook by itself. Both records are read anew at every refresh, so a writer publishes each by rename, and a pair whose identities disagree is denied by the evaluator rather than installed as approval.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use retrieval::ProjectionIdentity;
use serde::Deserialize;

use crate::coverage::ProjectionCoverage;
use crate::projection_gates::{
    CAPABILITIES, CapabilityEvidence, Evidence, EvidenceEvaluator, HARNESSES, HarnessRun, HookGate,
    InvalidationIdentity, ManifestRefusal, Renewal, ResourceEvidence, RuntimeManifest,
};
use crate::projection_lifecycle::{RecordRead, read_owner_only_record};

pub const ADMISSION_DIR: &str = "search-admission";
pub const MANIFEST_RECORD: &str = "runtime-manifest.json";
pub const EVIDENCE_RECORD: &str = "campaign-evidence.json";
/// Longest record name echoed in a refusal; the rest of a name stays in the record.
const NAME_BYTES: usize = 64;

/// Why an admission record was not accepted. Variants name the record, the check, and at most `NAME_BYTES` of an offending key, never record content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputRefusal {
    #[error("{0} is absent")]
    Missing(&'static str),
    #[error("{record} could not be read: {reason}")]
    Unreadable {
        record: &'static str,
        reason: String,
    },
    #[error("{record} is refused: {reason}")]
    Refused {
        record: &'static str,
        reason: &'static str,
    },
    #[error("{0} is not a record of its schema")]
    Malformed(&'static str),
    #[error(transparent)]
    Manifest(#[from] ManifestRefusal),
    #[error("{0} is not a supported harness")]
    UnknownHarness(String),
    #[error("{0} is not a projection capability")]
    UnknownCapability(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HarnessRunRecord {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct CampaignRecord {
    invalidation_identity: InvalidationIdentity,
    resource: Option<ResourceEvidence>,
    capabilities: BTreeMap<String, BTreeMap<String, CapabilityEvidence>>,
    harness_runs: BTreeMap<String, HarnessRunRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionInputs {
    manifest: RuntimeManifest,
    campaign: CampaignRecord,
}

impl AdmissionInputs {
    pub fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }

    /// Reads both records under `<home>/search-admission/`.
    ///
    /// # Errors
    ///
    /// Returns the first [`InputRefusal`]: a missing, unreadable, refused, or malformed record; a [`ManifestRefusal`]; or an unknown harness or capability name.
    pub fn read(home: &Path) -> Result<Self, InputRefusal> {
        let dir = home.join(ADMISSION_DIR);
        let manifest = RuntimeManifest::parse(&read_record(&dir, MANIFEST_RECORD)?)?;
        let campaign: CampaignRecord = serde_json::from_value(read_record(&dir, EVIDENCE_RECORD)?)
            .map_err(|_| InputRefusal::Malformed(EVIDENCE_RECORD))?;
        for harness in campaign
            .capabilities
            .keys()
            .chain(campaign.harness_runs.keys())
        {
            if !HARNESSES.contains(&harness.as_str()) {
                return Err(InputRefusal::UnknownHarness(bounded_name(harness)));
            }
        }
        for capability in campaign.capabilities.values().flat_map(BTreeMap::keys) {
            if !CAPABILITIES.iter().any(|known| known.name == capability) {
                return Err(InputRefusal::UnknownCapability(bounded_name(capability)));
            }
        }
        Ok(Self { manifest, campaign })
    }

    /// The evaluator for the projection at `current` with the coverage the daemon observed on it.
    pub fn evaluator(
        self,
        current: &ProjectionIdentity,
        coverage: Option<ProjectionCoverage>,
    ) -> EvidenceEvaluator {
        let campaign = self.campaign;
        let identity = campaign.invalidation_identity;
        EvidenceEvaluator {
            manifest: self.manifest,
            current: InvalidationIdentity::from(current),
            evidence: Evidence {
                coverage,
                resource: campaign.resource,
                capabilities: campaign
                    .capabilities
                    .into_iter()
                    .flat_map(|(harness, proved)| {
                        proved.into_iter().map(move |(capability, evidence)| {
                            ((harness.clone(), capability), evidence)
                        })
                    })
                    .collect(),
                harness_runs: campaign
                    .harness_runs
                    .into_iter()
                    .map(|(harness, record)| {
                        let run = match record {
                            HarnessRunRecord::Passed => HarnessRun::Passed {
                                identity: identity.clone(),
                            },
                            HarnessRunRecord::Failed => HarnessRun::Failed,
                        };
                        (harness, run)
                    })
                    .collect(),
                identity,
            },
        }
    }
}

fn bounded_name(name: &str) -> String {
    let end = name
        .char_indices()
        .map(|(index, _)| index)
        .nth(NAME_BYTES)
        .unwrap_or(name.len());
    name[..end].escape_debug().to_string()
}

fn read_record(dir: &Path, record: &'static str) -> Result<serde_json::Value, InputRefusal> {
    let bytes = match read_owner_only_record(dir, record) {
        Ok(RecordRead::Bytes(bytes)) => bytes,
        Ok(RecordRead::Absent) => return Err(InputRefusal::Missing(record)),
        Ok(RecordRead::Refused(reason)) => return Err(InputRefusal::Refused { record, reason }),
        Err(unreadable) => {
            return Err(InputRefusal::Unreadable {
                record,
                reason: unreadable.0,
            });
        }
    };
    serde_json::from_slice(&bytes).map_err(|_| InputRefusal::Malformed(record))
}

/// The selected family's identity and the coverage observed on it; `None` coverage means the observation was unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedProjection<'a> {
    pub identity: &'a ProjectionIdentity,
    pub coverage: Option<&'a ProjectionCoverage>,
}

/// Why a refresh left the gate closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Closed {
    #[error(transparent)]
    Inputs(#[from] InputRefusal),
    #[error("no projection is selected")]
    NoProjection,
    #[error("the admission owner is closed")]
    ShutDown,
}

/// What a refresh did to the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a Closed refresh has already closed the gate; report the reason"]
pub enum Refresh {
    Installed(Renewal),
    Closed(Closed),
}

/// The gate the daemon's hooks consult and the home its records are read from.
pub struct ProjectionAdmission {
    home: PathBuf,
    gate: Arc<HookGate>,
    /// Held across a whole refresh so two refreshes cannot install their records out of order, and set once the owner closes so no later refresh reopens the gate.
    closed: Mutex<bool>,
}

impl ProjectionAdmission {
    /// The gate starts closed; [`ProjectionAdmission::refresh`] opens it.
    pub fn for_home(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            gate: Arc::new(HookGate::for_home(home)),
            closed: Mutex::new(false),
        }
    }

    pub fn gate(&self) -> &Arc<HookGate> {
        &self.gate
    }

    /// Installs the evaluator the records describe for `selected`, or closes the gate when nothing is selected or either record is refused.
    pub fn refresh(&self, selected: Option<SelectedProjection<'_>>) -> Refresh {
        let closed = self.closed.lock().unwrap_or_else(|p| p.into_inner());
        if *closed {
            return Refresh::Closed(Closed::ShutDown);
        }
        let Some(SelectedProjection { identity, coverage }) = selected else {
            self.gate.close();
            return Refresh::Closed(Closed::NoProjection);
        };
        match AdmissionInputs::read(&self.home) {
            Ok(inputs) => Refresh::Installed(
                self.gate
                    .renew(inputs.evaluator(identity, coverage.cloned())),
            ),
            Err(refusal) => {
                self.gate.close();
                Refresh::Closed(Closed::Inputs(refusal))
            }
        }
    }

    /// Closes the gate for good: every grant is cancelled and no refresh reopens it.
    pub fn close(&self) {
        let mut closed = self.closed.lock().unwrap_or_else(|p| p.into_inner());
        *closed = true;
        self.gate.close();
    }
}
