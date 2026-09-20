use std::collections::BTreeMap;

use context_core::canonical_json::protocol_digest;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const RANDOM_SCHEMA_VERSION: &str = "eval-random/v1";
pub const CANDIDATES_DIGEST_PROTOCOL: &str = "eval-candidates/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceKind {
    TimeGap,
    ObservationLag,
    TextWord,
    Cites,
    CorrectionTarget,
    InvalidationTarget,
    RenameTarget,
}

impl ChoiceKind {
    /// The axis label that keys this kind's draws.
    pub fn axis(self) -> &'static str {
        match self {
            Self::TextWord => "text",
            Self::RenameTarget => "topology",
            Self::TimeGap
            | Self::ObservationLag
            | Self::Cites
            | Self::CorrectionTarget
            | Self::InvalidationTarget => "evolution",
        }
    }
}

/// `occurrence` counts earlier same-kind choices at the same `(actor, site)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceSite {
    pub kind: ChoiceKind,
    pub actor: String,
    pub site: String,
    pub occurrence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    #[serde(flatten)]
    pub site: ChoiceSite,
    pub candidates_digest: String,
    pub selected_index: u32,
}

/// `identity` binds the tape to the seed, config, and generator it was recorded under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tape {
    pub identity: String,
    pub entries: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayRefusal {
    TapeMismatch {
        expected: String,
        found: String,
    },
    MissingChoice {
        entry: usize,
    },
    SiteMismatch {
        entry: usize,
        expected: ChoiceSite,
        found: ChoiceSite,
    },
    ChangedCandidates {
        entry: usize,
        expected: String,
        found: String,
    },
    IndexOutOfRange {
        entry: usize,
        selected_index: u32,
        candidates: usize,
    },
    UnconsumedChoices {
        consumed: usize,
        recorded: usize,
    },
}

debug_display!(ReplayRefusal);

/// The first 64 bits of the protocol digest over the draw key, reduced by `% candidates`.
pub fn keyed_draw(root_seed: u64, site: &ChoiceSite) -> u64 {
    let key = json!({
        "root_seed": root_seed.to_string(),
        "axis": site.kind.axis(),
        "kind": site.kind,
        "actor": site.actor,
        "site": site.site,
        "occurrence": site.occurrence,
    });
    let hex = protocol_digest(RANDOM_SCHEMA_VERSION, &key).expect("draw key is canonical");
    u64::from_str_radix(&hex[..16], 16).expect("digest is lowercase hex")
}

enum Source {
    Generate { root_seed: u64 },
    Replay { tape: Tape, cursor: usize },
}

pub(crate) struct Chooser {
    identity: String,
    source: Source,
    occurrences: BTreeMap<(String, String, ChoiceKind), u32>,
    recorded: Vec<Choice>,
}

impl Chooser {
    pub(crate) fn generate(root_seed: u64, identity: String) -> Self {
        Self {
            identity,
            source: Source::Generate { root_seed },
            occurrences: BTreeMap::new(),
            recorded: Vec::new(),
        }
    }

    pub(crate) fn replay(tape: Tape, identity: String) -> Result<Self, ReplayRefusal> {
        if tape.identity != identity {
            return Err(ReplayRefusal::TapeMismatch {
                expected: identity,
                found: tape.identity,
            });
        }
        Ok(Self {
            source: Source::Replay { tape, cursor: 0 },
            ..Self::generate(0, identity)
        })
    }

    /// `candidates` must be non-empty; the choice is recorded in both modes.
    pub(crate) fn choose<T: Serialize>(
        &mut self,
        kind: ChoiceKind,
        actor: &str,
        site: &str,
        candidates: &[T],
    ) -> Result<usize, ReplayRefusal> {
        assert!(!candidates.is_empty(), "a choice needs a candidate");
        let occurrence = self
            .occurrences
            .entry((actor.to_string(), site.to_string(), kind))
            .or_insert(0);
        let site = ChoiceSite {
            kind,
            actor: actor.to_string(),
            site: site.to_string(),
            occurrence: *occurrence,
        };
        *occurrence += 1;
        let value = serde_json::to_value(candidates).expect("candidates serialize");
        let digest = protocol_digest(CANDIDATES_DIGEST_PROTOCOL, &value).expect("canonical");
        let entry = self.recorded.len();
        let selected = match &mut self.source {
            Source::Generate { root_seed } => {
                (keyed_draw(*root_seed, &site) % candidates.len() as u64) as u32
            }
            Source::Replay { tape, cursor } => {
                let choice = tape
                    .entries
                    .get(*cursor)
                    .ok_or(ReplayRefusal::MissingChoice { entry })?;
                *cursor += 1;
                if choice.site != site {
                    return Err(ReplayRefusal::SiteMismatch {
                        entry,
                        expected: site,
                        found: choice.site.clone(),
                    });
                }
                if choice.candidates_digest != digest {
                    return Err(ReplayRefusal::ChangedCandidates {
                        entry,
                        expected: digest,
                        found: choice.candidates_digest.clone(),
                    });
                }
                if choice.selected_index as usize >= candidates.len() {
                    return Err(ReplayRefusal::IndexOutOfRange {
                        entry,
                        selected_index: choice.selected_index,
                        candidates: candidates.len(),
                    });
                }
                choice.selected_index
            }
        };
        self.recorded.push(Choice {
            site,
            candidates_digest: digest,
            selected_index: selected,
        });
        Ok(selected as usize)
    }

    pub(crate) fn finish(self) -> Tape {
        Tape {
            identity: self.identity,
            entries: self.recorded,
        }
    }
}
