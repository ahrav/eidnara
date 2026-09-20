use context_core::canonical_json::{ContractError, protocol_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ELIGIBILITY_SPEC_PROTOCOL: &str = "eidnara-eligibility-spec-v1";
/// The digest of `serialize_spec()`; `crates/kernel/testdata/eligibility-spec-v1.json` pins the same value.
pub const ELIGIBILITY_SPEC_DIGEST: &str =
    "bab845acdac8a0998bfba7ee10f1fd55008bb615cc8535ea5b5a50521fdd9fcd";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Retracted,
    Superseded,
    Stale,
    WrongScope,
    Hidden,
    ProviderSensitive,
}

impl Verdict {
    pub const ALL: [Self; 7] = [
        Self::Ok,
        Self::Retracted,
        Self::Superseded,
        Self::Stale,
        Self::WrongScope,
        Self::Hidden,
        Self::ProviderSensitive,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Normal,
    Sensitive,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Hidden,
    Visible,
    Labeled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    AutoInject,
    AutoSearch,
    ExplicitSearch,
}

impl Surface {
    pub const ALL: [Self; 3] = [Self::AutoInject, Self::AutoSearch, Self::ExplicitSearch];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEligibility {
    Allowed,
    Denied,
}

/// The serving view's class for an admitted object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServedClass {
    pub sensitivity: Sensitivity,
    pub visibility: Visibility,
    pub auto_inject: Visibility,
    pub auto_search: Visibility,
}

/// What the registry says about an object it has seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateFacts {
    pub superseded: bool,
    pub invalidated: bool,
    pub revision_matches: bool,
    pub in_scope: bool,
    pub registry_sensitivity: Sensitivity,
}

/// Everything the kernel's judge reads, as values; `state` is `None` for an
/// object the registry has never seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactTuple {
    pub state: Option<StateFacts>,
    pub served: Option<ServedClass>,
    pub artifact: Option<ArtifactEligibility>,
    pub destination: Destination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceVerdict {
    pub verdict: Verdict,
    pub visibility: Visibility,
}

impl SurfaceVerdict {
    pub fn permits(self) -> bool {
        self.verdict == Verdict::Ok && self.visibility != Visibility::Hidden
    }
}

/// The kernel's `judge` tests these in the order [`PREDICATES`] lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredicateKind {
    StateAbsent,
    Superseded,
    Invalidated,
    RevisionDiffers,
    OutOfScope,
    SensitivityDeniesDestination,
    UnservedOrHidden,
    ArtifactDenied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    pub name: PredicateKind,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vector {
    pub facts: FactTuple,
    pub verdict: Verdict,
}

/// The kernel's verdict partition as a value: the predicates in short-circuit
/// order, the named derivations, and the vector set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EligibilitySpec {
    pub version: String,
    pub verdicts: Vec<Verdict>,
    pub predicates: Vec<Predicate>,
    pub sensitivity_derivation: String,
    pub visibility_derivation: String,
    pub supersession_derivation: String,
    pub vectors: Vec<Vector>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    SpecDrift { expected: String, found: String },
    NotCanonical(ContractError),
}

debug_display!(SpecError);

/// The first predicate that holds names the verdict; none holding is `Ok`.
pub const PREDICATES: [Predicate; 8] = [
    Predicate {
        name: PredicateKind::StateAbsent,
        verdict: Verdict::Retracted,
    },
    Predicate {
        name: PredicateKind::Superseded,
        verdict: Verdict::Superseded,
    },
    Predicate {
        name: PredicateKind::Invalidated,
        verdict: Verdict::Retracted,
    },
    Predicate {
        name: PredicateKind::RevisionDiffers,
        verdict: Verdict::Stale,
    },
    Predicate {
        name: PredicateKind::OutOfScope,
        verdict: Verdict::WrongScope,
    },
    Predicate {
        name: PredicateKind::SensitivityDeniesDestination,
        verdict: Verdict::ProviderSensitive,
    },
    Predicate {
        name: PredicateKind::UnservedOrHidden,
        verdict: Verdict::Hidden,
    },
    Predicate {
        name: PredicateKind::ArtifactDenied,
        verdict: Verdict::ProviderSensitive,
    },
];

const SENSITIVITY_DERIVATION: &str = "served.sensitivity if served else state.registry_sensitivity; secret denies every destination and sensitive denies remote";
const VISIBILITY_DERIVATION: &str = "hidden if unserved else served.{auto_inject, auto_search, visibility} by surface; permits = verdict == ok and visibility != hidden";
const SUPERSESSION_DERIVATION: &str = "a supersession invalidates the predecessor in the same envelope, so superseded implies invalidated";

fn holds(kind: PredicateKind, facts: &FactTuple) -> bool {
    let state = facts.state;
    match kind {
        PredicateKind::StateAbsent => state.is_none(),
        PredicateKind::Superseded => state.is_some_and(|s| s.superseded),
        PredicateKind::Invalidated => state.is_some_and(|s| s.invalidated),
        PredicateKind::RevisionDiffers => state.is_some_and(|s| !s.revision_matches),
        PredicateKind::OutOfScope => state.is_none_or(|s| !s.in_scope),
        PredicateKind::SensitivityDeniesDestination => {
            let registry = state.map_or(Sensitivity::Normal, |s| s.registry_sensitivity);
            let sensitivity = facts.served.map_or(registry, |served| served.sensitivity);
            matches!(
                (sensitivity, facts.destination),
                (Sensitivity::Secret, _) | (Sensitivity::Sensitive, Destination::Remote)
            )
        }
        PredicateKind::UnservedOrHidden => facts
            .served
            .is_none_or(|served| served.visibility == Visibility::Hidden),
        PredicateKind::ArtifactDenied => facts.artifact == Some(ArtifactEligibility::Denied),
    }
}

pub fn judge(facts: &FactTuple) -> Verdict {
    judge_with(&PREDICATES, facts)
}

/// Evaluates `predicates` in order; the first that holds names the verdict.
pub fn judge_with(predicates: &[Predicate], facts: &FactTuple) -> Verdict {
    predicates
        .iter()
        .find(|predicate| holds(predicate.name, facts))
        .map_or(Verdict::Ok, |predicate| predicate.verdict)
}

pub fn visibility_on(facts: &FactTuple, surface: Surface) -> Visibility {
    facts
        .served
        .map_or(Visibility::Hidden, |served| match surface {
            Surface::AutoInject => served.auto_inject,
            Surface::AutoSearch => served.auto_search,
            Surface::ExplicitSearch => served.visibility,
        })
}

pub fn judge_surface(facts: &FactTuple, surface: Surface) -> SurfaceVerdict {
    SurfaceVerdict {
        verdict: judge(facts),
        visibility: visibility_on(facts, surface),
    }
}

/// Vectors mirror the kernel's independently authored eligibility table,
/// including its precedence pairs; each row states what differs from `LIVE`.
#[rustfmt::skip]
const VECTORS: [Vector; 22] = {
    use ArtifactEligibility::{Allowed, Denied};
    use Destination::{Local, Remote};
    use Sensitivity::{Normal, Secret, Sensitive};
    use Verdict::*;
    use Visibility::{Hidden as H, Labeled as L, Visible as V};
    const fn served(sensitivity: Sensitivity, visibility: Visibility, auto: Visibility) -> Option<ServedClass> {
        Some(ServedClass { sensitivity, visibility, auto_inject: auto, auto_search: auto })
    }
    const OK: StateFacts = StateFacts { superseded: false, invalidated: false, revision_matches: true, in_scope: true, registry_sensitivity: Normal };
    const LIVE: FactTuple = FactTuple { state: Some(OK), served: served(Normal, L, H), artifact: None, destination: Local };
    const fn row(facts: FactTuple, verdict: Verdict) -> Vector { Vector { facts, verdict } }
    [
        row(LIVE, Ok),
        row(FactTuple { served: served(Normal, V, V), destination: Remote, ..LIVE }, Ok),
        row(FactTuple { state: None, served: None, ..LIVE }, Retracted),
        row(FactTuple { state: Some(StateFacts { invalidated: true, ..OK }), served: None, ..LIVE }, Retracted),
        row(FactTuple { state: Some(StateFacts { superseded: true, invalidated: true, ..OK }), served: None, ..LIVE }, Superseded),
        row(FactTuple { state: Some(StateFacts { superseded: true, invalidated: true, revision_matches: false, in_scope: false, registry_sensitivity: Secret }), served: None, destination: Remote, ..LIVE }, Superseded),
        row(FactTuple { state: Some(StateFacts { revision_matches: false, ..OK }), ..LIVE }, Stale),
        row(FactTuple { state: Some(StateFacts { revision_matches: false, in_scope: false, ..OK }), destination: Remote, ..LIVE }, Stale),
        row(FactTuple { state: Some(StateFacts { invalidated: true, in_scope: false, ..OK }), served: None, ..LIVE }, Retracted),
        row(FactTuple { state: Some(StateFacts { invalidated: true, revision_matches: false, ..OK }), served: None, ..LIVE }, Retracted),
        row(FactTuple { state: Some(StateFacts { in_scope: false, ..OK }), ..LIVE }, WrongScope),
        row(FactTuple { state: Some(StateFacts { in_scope: false, registry_sensitivity: Secret, ..OK }), served: served(Secret, H, H), ..LIVE }, WrongScope),
        row(FactTuple { state: Some(StateFacts { registry_sensitivity: Secret, ..OK }), served: served(Secret, H, H), ..LIVE }, ProviderSensitive),
        row(FactTuple { state: Some(StateFacts { registry_sensitivity: Secret, ..OK }), served: None, ..LIVE }, ProviderSensitive),
        row(FactTuple { state: Some(StateFacts { registry_sensitivity: Sensitive, ..OK }), served: served(Sensitive, L, H), ..LIVE }, Ok),
        row(FactTuple { state: Some(StateFacts { registry_sensitivity: Sensitive, ..OK }), served: served(Sensitive, L, H), destination: Remote, ..LIVE }, ProviderSensitive),
        row(FactTuple { served: None, ..LIVE }, Hidden),
        row(FactTuple { served: served(Normal, H, H), ..LIVE }, Hidden),
        row(FactTuple { served: served(Normal, H, H), artifact: Some(Denied), ..LIVE }, Hidden),
        row(FactTuple { artifact: Some(Allowed), ..LIVE }, Ok),
        row(FactTuple { artifact: Some(Denied), destination: Remote, ..LIVE }, ProviderSensitive),
        row(FactTuple { artifact: Some(Denied), ..LIVE }, ProviderSensitive),
    ]
};

pub fn spec() -> EligibilitySpec {
    EligibilitySpec {
        version: ELIGIBILITY_SPEC_PROTOCOL.to_string(),
        verdicts: Verdict::ALL.to_vec(),
        predicates: PREDICATES.to_vec(),
        sensitivity_derivation: SENSITIVITY_DERIVATION.to_string(),
        visibility_derivation: VISIBILITY_DERIVATION.to_string(),
        supersession_derivation: SUPERSESSION_DERIVATION.to_string(),
        vectors: VECTORS.to_vec(),
    }
}

pub fn serialize_spec() -> Value {
    serde_json::to_value(spec()).expect("spec serializes")
}

/// Parses a fixture only when its canonical value digests to the pinned constant.
pub fn check_spec(fixture: &Value) -> Result<EligibilitySpec, SpecError> {
    let found =
        protocol_digest(ELIGIBILITY_SPEC_PROTOCOL, fixture).map_err(SpecError::NotCanonical)?;
    if found != ELIGIBILITY_SPEC_DIGEST {
        return Err(SpecError::SpecDrift {
            expected: ELIGIBILITY_SPEC_DIGEST.to_string(),
            found,
        });
    }
    Ok(EligibilitySpec::deserialize(fixture)
        .expect("a value digesting to ELIGIBILITY_SPEC_DIGEST is serialize_spec()"))
}
