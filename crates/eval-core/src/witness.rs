//! The witness package: the original failure, the minimized scenario that
//! still reproduces it, and the shrink log naming the transformations it is
//! 1-minimal under.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::canonical_json_encode;
use context_core::redaction::{RedactionErrorKind, Redactor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cassette::scan_for_secrets;
use crate::eligibility::serialize_spec;
use crate::event::{CausalEdge, EventId, EventLog};
use crate::failure_class::Slice;
use crate::fault::validate_episodes;
use crate::generator::{Mode, WorldConfig, generate_all};
use crate::manifest::{CLAIM_BOUNDARY_EXCLUSIONS, ClaimBoundary};
use crate::markers::MARKERS;
use crate::residue::{ResidueEntry, residue_contradiction};
use crate::shrink::{
    CandidateVerdict, Element, FailurePredicate, History, Minimality, Scenario, ShrinkReport,
    ShrinkReportError, Transformation, WitnessClass,
};
use crate::stream::Tape;

pub const WITNESS_SCHEMA: &str = "eval-witness/v1";
pub const WITNESS_DIGEST_PROTOCOL: &str = "eval-witness-digest/v1";

/// The failure as the campaign observed it, before any shrinking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalFailure {
    pub eval_run_id: String,
    pub tape: Tape,
    pub trace_digest: String,
    pub causal_trace: Vec<CausalEdge>,
    pub predicate: FailurePredicate,
    /// The shrink markers the shell recorded for this failure.
    pub coverage: BTreeSet<String>,
}

/// How to regenerate one world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub config: WorldConfig,
    #[serde(with = "crate::decimal")]
    pub root_seed: u64,
}

/// A count-triggered failure's compact form beside the minimized scenario:
/// regenerate both worlds and apply the report's deletions. `multiplicities`
/// counts the surviving aged events of each kind whose single deletion
/// changed the outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiplicityRecipe {
    pub aged: Generation,
    pub natural_fresh: Generation,
    pub multiplicities: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessPackage {
    pub schema: String,
    pub original: OriginalFailure,
    pub slice: Slice,
    /// `false` for every live slice: a recorded live run is evidence of what
    /// happened once, not of what a fresh process reproduces.
    pub replayable: bool,
    pub residue: BTreeSet<ResidueEntry>,
    pub minimized: Scenario,
    pub recipe: Option<MultiplicityRecipe>,
    pub shrink: ShrinkReport,
    pub claim_boundary: ClaimBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessError {
    SchemaMismatch {
        found: String,
    },
    /// The embedded report refuses on its own terms: schema, oracle, or its
    /// accounting against its candidate ledger.
    ShrinkReport(ShrinkReportError),
    ClaimBoundaryMismatch,
    ForbiddenClaim {
        path: String,
        phrase: String,
    },
    LiveRelabelledReplayable,
    PredicateDisagrees,
    MinimizedDigestMismatch,
    /// The minimized scenario is not one a child could replay: its episodes
    /// are not a valid set or the pair compiler refuses it.
    MinimizedNotReplayable {
        refusal: String,
    },
    /// The failure predicate names a task other than the one a replay
    /// evaluates, the minimized scenario's first pair.
    PredicateNamesAnotherTask {
        task: String,
    },
    /// The minimized scenario still fails only in multiplicity, so the
    /// compact form is required; or it carries one it does not need.
    RecipeRequired,
    RecipeWithoutMultiplicity,
    RecipeMultiplicitiesDisagree,
    /// The regenerated world is not the minimized one, or its declared size
    /// is not what the minimized log and the deletions account for.
    RecipeDisagrees {
        history: History,
    },
    /// The report claims 1-minimality without a rejected replay record for
    /// this single deletion from the minimized scenario.
    MinimalityUnsupported {
        element: Element,
    },
    /// The 1-minimality claim names other transformations than the ones the
    /// original held elements for.
    TransformationsDisagree {
        expected: Vec<Transformation>,
    },
    /// The coverage signature names a marker the registry does not have.
    UnregisteredMarker {
        name: String,
    },
    ResidueDrift {
        missing: BTreeSet<ResidueEntry>,
        unexpected: BTreeSet<ResidueEntry>,
    },
    /// The recorded residue holds a `Keep` rule or two rules for one field,
    /// which no schema declares; no replay could ever match it.
    ResidueContradiction {
        type_name: String,
        field: String,
    },
    NotHex {
        field: &'static str,
    },
    TooLarge {
        bytes: u64,
        bound: u64,
    },
    RedactionRefused(RedactionErrorKind),
    Shape(String),
    Lossy,
}

debug_display!(WitnessError);

impl WitnessPackage {
    pub fn validate(&self) -> Result<(), WitnessError> {
        if self.schema != WITNESS_SCHEMA {
            return Err(WitnessError::SchemaMismatch {
                found: self.schema.clone(),
            });
        }
        if self.claim_boundary != ClaimBoundary::pinned() {
            return Err(WitnessError::ClaimBoundaryMismatch);
        }
        if self.slice == Slice::Live && self.replayable {
            return Err(WitnessError::LiveRelabelledReplayable);
        }
        if self.original.predicate != self.shrink.predicate {
            return Err(WitnessError::PredicateDisagrees);
        }
        if self.minimized.digest() != self.shrink.minimized_digest {
            return Err(WitnessError::MinimizedDigestMismatch);
        }
        // A replayable witness is one a child can replay: the minimized
        // scenario compiles under the pinned fixture and its episodes are a
        // valid set, as the shrinker required of every accepted candidate.
        validate_episodes(&self.minimized.episodes).map_err(|refusal| {
            WitnessError::MinimizedNotReplayable {
                refusal: format!("{refusal:?}"),
            }
        })?;
        let set = self
            .minimized
            .compile(&serialize_spec())
            .map_err(|refusal| WitnessError::MinimizedNotReplayable {
                refusal: refusal.kind().to_string(),
            })?;
        // A child evaluates the first pair's task; a failure's subject is that
        // task or the predicate cannot be reproduced, only slipped from.
        if let WitnessClass::Failure { task, .. } = &self.original.predicate.witness_class {
            let evaluated = set.pairs.first().map(|pair| pair.task.id.as_str());
            if evaluated != Some(task.as_str()) {
                return Err(WitnessError::PredicateNamesAnotherTask { task: task.clone() });
            }
        }
        // What the report deleted is gone from the minimized scenario.
        let elements = self.minimized.elements();
        if self
            .shrink
            .deleted
            .iter()
            .any(|element| elements.contains(element))
        {
            return Err(WitnessError::ShrinkReport(
                ShrinkReportError::Inconsistent { field: "deleted" },
            ));
        }
        if let Some(entry) = residue_contradiction(&self.residue) {
            return Err(WitnessError::ResidueContradiction {
                type_name: entry.type_name.clone(),
                field: entry.field.clone(),
            });
        }
        for (field, text, len) in [
            ("eval_run_id", &self.original.eval_run_id, 64),
            ("trace_digest", &self.original.trace_digest, 64),
        ] {
            if !context_core::canonical_json::is_lower_hex(text, len) {
                return Err(WitnessError::NotHex { field });
            }
        }
        self.check_recipe()?;
        self.check_minimality()?;
        // The report's own accounting, then what only the package can check:
        // the survivors it declares are the minimized scenario's elements.
        self.shrink.validate().map_err(WitnessError::ShrinkReport)?;
        if self.shrink.remaining != self.minimized.elements().len() as u64 {
            return Err(WitnessError::ShrinkReport(
                ShrinkReportError::Inconsistent { field: "remaining" },
            ));
        }
        let value = serde_json::to_value(self).map_err(|e| WitnessError::Shape(e.to_string()))?;
        check_claims(&value, "")?;
        for name in &self.original.coverage {
            if !MARKERS.iter().any(|marker| marker.name == name) {
                return Err(WitnessError::UnregisteredMarker { name: name.clone() });
            }
        }
        Ok(())
    }

    /// `OneMinimal` claims every single deletion from the minimized scenario
    /// was replayed and rejected; the report must carry that record for each,
    /// under the digest of the scenario that deletion produces, and name
    /// exactly the transformations the original held elements for, in the
    /// shrinker's order.
    fn check_minimality(&self) -> Result<(), WitnessError> {
        let Minimality::OneMinimal { transformations } = &self.shrink.minimality else {
            return Ok(());
        };
        let elements = self.minimized.elements();
        let held: BTreeSet<Transformation> = elements
            .iter()
            .chain(&self.shrink.deleted)
            .map(Element::transformation)
            .collect();
        let expected: Vec<Transformation> = Transformation::ORDER
            .into_iter()
            .filter(|transformation| held.contains(transformation))
            .collect();
        if *transformations != expected {
            return Err(WitnessError::TransformationsDisagree { expected });
        }
        for element in elements {
            let mut deleted = self.shrink.deleted.clone();
            deleted.insert(element.clone());
            let digest = self
                .minimized
                .without(&BTreeSet::from([element.clone()]))
                .digest();
            let rejected = self.shrink.candidates.iter().any(|record| {
                record.deleted == deleted
                    && record.scenario_digest == digest
                    && !matches!(
                        record.verdict,
                        CandidateVerdict::Reproduced | CandidateVerdict::Unknown { .. }
                    )
            });
            if !rejected {
                return Err(WitnessError::MinimalityUnsupported { element });
            }
        }
        Ok(())
    }

    /// A kind is count-triggered when the minimized scenario keeps more than
    /// one aged event of it and deleting any one alone changed the outcome
    /// (`Slipped` or `NotReproduced`). A 1-minimal scenario with such a kind
    /// carries the compact form; one without, or one whose minimality is not
    /// established, carries none; the form must regenerate exactly the
    /// minimized logs.
    fn check_recipe(&self) -> Result<(), WitnessError> {
        let triggered = self.count_triggered();
        let minimal = matches!(self.shrink.minimality, Minimality::OneMinimal { .. });
        match &self.recipe {
            None if minimal && !triggered.is_empty() => Err(WitnessError::RecipeRequired),
            None => Ok(()),
            Some(_) if !minimal || triggered.is_empty() => {
                Err(WitnessError::RecipeWithoutMultiplicity)
            }
            Some(recipe) if recipe.multiplicities != triggered => {
                Err(WitnessError::RecipeMultiplicitiesDisagree)
            }
            Some(recipe) => {
                self.regenerates(History::Aged, &recipe.aged, &self.minimized.aged)?;
                self.regenerates(
                    History::NaturalFresh,
                    &recipe.natural_fresh,
                    &self.minimized.natural_fresh,
                )
            }
        }
    }

    /// The kinds a compact recipe must count, with their counts. A record
    /// counts only under the digest of the scenario its deletion produces.
    pub fn count_triggered(&self) -> BTreeMap<String, u64> {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for event in &self.minimized.aged.events {
            let element = Element::Event {
                history: History::Aged,
                id: event.id.clone(),
            };
            let digest = self
                .minimized
                .without(&BTreeSet::from([element.clone()]))
                .digest();
            let mut deleted = self.shrink.deleted.clone();
            deleted.insert(element);
            let changed = self.shrink.candidates.iter().any(|record| {
                record.deleted == deleted
                    && record.scenario_digest == digest
                    && matches!(
                        record.verdict,
                        CandidateVerdict::Slipped { .. } | CandidateVerdict::NotReproduced
                    )
            });
            if changed {
                let payload = serde_json::to_value(&event.payload).expect("payload serializes");
                let kind = payload["kind"]
                    .as_str()
                    .expect("payload is tagged")
                    .to_string();
                *counts.entry(kind).or_insert(0) += 1;
            }
        }
        counts.retain(|_, count| *count > 1);
        counts
    }

    /// The declared size is checked before anything is generated, so a parsed
    /// package cannot demand an unbounded regeneration.
    fn regenerates(
        &self,
        history: History,
        generation: &Generation,
        minimized: &EventLog,
    ) -> Result<(), WitnessError> {
        let disagrees = || WitnessError::RecipeDisagrees { history };
        let deleted: Vec<&EventId> = self
            .shrink
            .deleted
            .iter()
            .filter_map(|element| match element {
                Element::Event { history: h, id } if *h == history => Some(id),
                _ => None,
            })
            .collect();
        if generation.config.declared_events() != (minimized.events.len() + deleted.len()) as u64 {
            return Err(disagrees());
        }
        let world = generate_all(generation.root_seed, &generation.config, Mode::Generate)
            .map_err(|_| disagrees())?;
        // The aged generation is the original's world: its decision tape is
        // the one the failure recorded, not just its log.
        if history == History::Aged && world.tape != self.original.tape {
            return Err(disagrees());
        }
        let mut log = world.log;
        for id in deleted {
            log = log.without(id);
        }
        if log != *minimized {
            return Err(disagrees());
        }
        Ok(())
    }

    /// The one serializer: refuses before a byte leaves. A detected secret
    /// refuses the whole package rather than substituting a placeholder; the
    /// byte bound is the envelope's artifact limit over the canonical bytes,
    /// which are the bytes to publish.
    pub fn serialize(
        &self,
        redactor: &Redactor,
        artifact_bytes: u64,
    ) -> Result<(Value, String), WitnessError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|e| WitnessError::Shape(e.to_string()))?;
        let text = canonical_json_encode(&value).map_err(|e| WitnessError::Shape(e.to_string()))?;
        if text.len() as u64 > artifact_bytes {
            return Err(WitnessError::TooLarge {
                bytes: text.len() as u64,
                bound: artifact_bytes,
            });
        }
        scan_for_secrets(redactor, &text).map_err(WitnessError::RedactionRefused)?;
        Ok((value, text))
    }
}

/// Refuses when `current` classifies any field differently from `recorded`.
pub fn residue_drift(
    recorded: &BTreeSet<ResidueEntry>,
    current: &BTreeSet<ResidueEntry>,
) -> Result<(), WitnessError> {
    if recorded == current {
        return Ok(());
    }
    Err(WitnessError::ResidueDrift {
        missing: recorded.difference(current).cloned().collect(),
        unexpected: current.difference(recorded).cloned().collect(),
    })
}

/// Every string leaf outside `claim_boundary` is scanned for the excluded
/// claims, which may be named only inside that block; the first hit names
/// its path.
fn check_claims(value: &Value, path: &str) -> Result<(), WitnessError> {
    match value {
        Value::String(text) => {
            let lowered = text.to_ascii_lowercase();
            for phrase in CLAIM_BOUNDARY_EXCLUSIONS {
                if lowered.contains(phrase) {
                    return Err(WitnessError::ForbiddenClaim {
                        path: path.to_string(),
                        phrase: phrase.to_string(),
                    });
                }
            }
            Ok(())
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .try_for_each(|(index, item)| check_claims(item, &format!("{path}[{index}]"))),
        Value::Object(fields) => fields
            .iter()
            .filter(|(key, _)| !(path.is_empty() && *key == "claim_boundary"))
            .try_for_each(|(key, item)| check_claims(item, &format!("{path}/{key}"))),
        _ => Ok(()),
    }
}

/// Refuses an integer outside the canonical range, a missing block by name,
/// a field the type would drop, then everything `validate` refuses.
pub fn parse_witness(value: &Value) -> Result<WitnessPackage, WitnessError> {
    canonical_json_encode(value).map_err(|e| WitnessError::Shape(e.to_string()))?;
    let package =
        WitnessPackage::deserialize(value).map_err(|e| WitnessError::Shape(e.to_string()))?;
    package.validate()?;
    let again = serde_json::to_value(&package).map_err(|e| WitnessError::Shape(e.to_string()))?;
    if again != *value {
        return Err(WitnessError::Lossy);
    }
    Ok(package)
}
