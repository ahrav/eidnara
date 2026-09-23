//! The witness package: the original failure, the minimized scenario that
//! still reproduces it, and the shrink log naming the transformations it is
//! 1-minimal under.

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::canonical_json_encode;
use context_core::redaction::{RedactionErrorKind, Redactor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cassette::scan_for_secrets;
use crate::event::CausalEdge;
use crate::failure_class::Slice;
use crate::generator::{Mode, WorldConfig, generate_all};
use crate::manifest::{CLAIM_BOUNDARY_EXCLUSIONS, ClaimBoundary};
use crate::residue::ResidueEntry;
use crate::shrink::{Element, FailurePredicate, History, Minimality, Scenario, ShrinkReport};
use crate::stream::Tape;

pub const WITNESS_SCHEMA: &str = "eval-witness/v1";
pub const WITNESS_DIGEST_PROTOCOL: &str = "eval-witness-digest/v1";
/// The four excluded claims may be named only inside the claim-boundary
/// block; anywhere else in a witness they are a claim the witness cannot make.
pub const FORBIDDEN_CLAIM_PHRASES: [&str; 4] = CLAIM_BOUNDARY_EXCLUSIONS;

/// The failure as the campaign observed it, before any shrinking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalFailure {
    pub eval_run_id: String,
    pub tape: Tape,
    pub trace_digest: String,
    pub causal_trace: Vec<CausalEdge>,
    pub predicate: FailurePredicate,
    /// The coverage markers the failing run fired.
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

/// A count-triggered failure's compact form: regenerate both worlds and apply
/// the deletions instead of carrying every surviving event verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiplicityRecipe {
    pub aged: Generation,
    pub natural_fresh: Generation,
    pub deleted: BTreeSet<Element>,
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
    ClaimBoundaryMismatch,
    ForbiddenClaim {
        path: String,
        phrase: String,
    },
    LiveRelabelledReplayable,
    PredicateDisagrees,
    MinimizedDigestMismatch,
    /// The minimized scenario still fails only in multiplicity, so the
    /// compact form is required; or it carries one it does not need.
    RecipeRequired,
    RecipeWithoutMultiplicity,
    RecipeDisagrees {
        world: &'static str,
    },
    ResidueDrift {
        missing: BTreeSet<ResidueEntry>,
        unexpected: BTreeSet<ResidueEntry>,
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
        for (field, text, len) in [
            ("eval_run_id", &self.original.eval_run_id, 64),
            ("trace_digest", &self.original.trace_digest, 64),
        ] {
            if !context_core::canonical_json::is_lower_hex(text, len) {
                return Err(WitnessError::NotHex { field });
            }
        }
        self.check_recipe()?;
        let value = serde_json::to_value(self).map_err(|e| WitnessError::Shape(e.to_string()))?;
        check_claims(&value, "")
    }

    /// A 1-minimal scenario whose surviving aged events still repeat a kind
    /// carries the compact form, which must regenerate exactly the minimized
    /// logs; a scenario without a repeated kind carries none.
    fn check_recipe(&self) -> Result<(), WitnessError> {
        let multiplicities = multiplicities(&self.minimized);
        let repeated = multiplicities.values().any(|count| *count > 1);
        let minimal = matches!(self.shrink.minimality, Minimality::OneMinimal { .. });
        match &self.recipe {
            None if minimal && repeated => Err(WitnessError::RecipeRequired),
            None => Ok(()),
            Some(_) if !repeated => Err(WitnessError::RecipeWithoutMultiplicity),
            Some(recipe) => {
                if recipe.multiplicities != multiplicities || recipe.deleted != self.shrink.deleted
                {
                    return Err(WitnessError::RecipeDisagrees { world: "aged" });
                }
                let regenerate = |generation: &Generation| {
                    generate_all(generation.root_seed, &generation.config, Mode::Generate)
                        .map(|world| world.log)
                };
                let mut aged = regenerate(&recipe.aged);
                let mut natural_fresh = regenerate(&recipe.natural_fresh);
                for element in &recipe.deleted {
                    match element {
                        Element::Event {
                            history: History::Aged,
                            id,
                        } => aged = aged.map(|log| log.without(id)),
                        Element::Event {
                            history: History::NaturalFresh,
                            id,
                        } => natural_fresh = natural_fresh.map(|log| log.without(id)),
                        Element::Episode { .. } => {}
                    }
                }
                if aged.as_ref() != Ok(&self.minimized.aged) {
                    return Err(WitnessError::RecipeDisagrees { world: "aged" });
                }
                if natural_fresh.as_ref() != Ok(&self.minimized.natural_fresh) {
                    return Err(WitnessError::RecipeDisagrees {
                        world: "natural_fresh",
                    });
                }
                Ok(())
            }
        }
    }

    /// Refuses when the schemas a replaying process declares classify any
    /// field differently from the recorded residue.
    pub fn check_residue(&self, current: &BTreeSet<ResidueEntry>) -> Result<(), WitnessError> {
        if self.residue == *current {
            return Ok(());
        }
        Err(WitnessError::ResidueDrift {
            missing: self.residue.difference(current).cloned().collect(),
            unexpected: current.difference(&self.residue).cloned().collect(),
        })
    }

    /// The one serializer: refuses before a byte leaves. With a redactor, a
    /// detected secret refuses the whole package rather than substituting a
    /// placeholder; the byte bound is the envelope's artifact limit.
    pub fn serialize(
        &self,
        redactor: Option<&Redactor>,
        max_bytes: u64,
    ) -> Result<Value, WitnessError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|e| WitnessError::Shape(e.to_string()))?;
        let bytes = canonical_json_encode(&value)
            .map_err(|e| WitnessError::Shape(e.to_string()))?
            .len() as u64;
        if bytes > max_bytes {
            return Err(WitnessError::TooLarge {
                bytes,
                bound: max_bytes,
            });
        }
        if let Some(redactor) = redactor {
            scan_for_secrets(redactor, &value).map_err(WitnessError::RedactionRefused)?;
        }
        Ok(value)
    }
}

/// How many aged events each payload kind contributes.
pub fn multiplicities(scenario: &Scenario) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for event in &scenario.aged.events {
        *counts.entry(event.payload.kind().to_string()).or_insert(0) += 1;
    }
    counts
}

/// Every string leaf outside `claim_boundary` is scanned for the excluded
/// claims; the first hit names its path.
fn check_claims(value: &Value, path: &str) -> Result<(), WitnessError> {
    match value {
        Value::String(text) => {
            let lowered = text.to_ascii_lowercase();
            for phrase in FORBIDDEN_CLAIM_PHRASES {
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

/// Refuses a missing block by name, a field the type would drop, then
/// everything `validate` refuses.
pub fn parse_witness(value: &Value) -> Result<WitnessPackage, WitnessError> {
    let package =
        WitnessPackage::deserialize(value).map_err(|e| WitnessError::Shape(e.to_string()))?;
    package.validate()?;
    let again = serde_json::to_value(&package).map_err(|e| WitnessError::Shape(e.to_string()))?;
    if again != *value {
        return Err(WitnessError::Lossy);
    }
    Ok(package)
}
