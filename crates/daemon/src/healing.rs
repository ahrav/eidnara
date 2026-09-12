//! Serializer profiles and their request-healing coverage.
//!
//! Profiles identify cleanup applied before provider dispatch. Residual flags
//! identify work not covered by the selected serializer.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SerializerProfile {
    OwnedLlmRunner,
    /// OwnedBroca has identical serializer semantics to OwnedLlmRunner but retains a distinct wire ID.
    OwnedBroca,
    ClaudeCodeAnthropic,
    OpencodeAiSdk,
    Pi,
}

const ALL_PROFILES: [SerializerProfile; 5] = [
    SerializerProfile::OwnedLlmRunner,
    SerializerProfile::OwnedBroca,
    SerializerProfile::ClaudeCodeAnthropic,
    SerializerProfile::OpencodeAiSdk,
    SerializerProfile::Pi,
];

impl SerializerProfile {
    pub const fn wire_id(self) -> &'static str {
        match self {
            Self::OwnedLlmRunner => "owned-llmrunner",
            Self::OwnedBroca => "owned-broca",
            Self::ClaudeCodeAnthropic => "claude-code-anthropic",
            Self::OpencodeAiSdk => "opencode-aisdk",
            Self::Pi => "pi",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owned-llmrunner" => Some(Self::OwnedLlmRunner),
            "owned-broca" => Some(Self::OwnedBroca),
            "claude-code-anthropic" => Some(Self::ClaudeCodeAnthropic),
            "opencode-aisdk" => Some(Self::OpencodeAiSdk),
            "pi" => Some(Self::Pi),
            _ => None,
        }
    }

    pub const fn all() -> &'static [Self] {
        &ALL_PROFILES
    }
}

impl Serialize for SerializerProfile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.wire_id())
    }
}

impl<'de> Deserialize<'de> for SerializerProfile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::custom(format!("unknown serializer profile {value:?}"))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealingCoverage {
    /// The serializer removes empty text and reasoning blocks before provider dispatch.
    pub drops_empty_content: bool,
    /// The serializer supplies an empty reasoning field for providers that require one.
    pub autofills_reasoning: bool,
    /// The serializer coalesces adjacent assistant messages before provider dispatch.
    pub merges_consecutive_assistants: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuirkResidual {
    /// Empty blocks can still reach a provider that rejects them, so module reductions must
    /// use non-empty placeholders rather than relying on the serializer to drop empties.
    pub requires_non_anthropic_empty_sentinels: bool,
    /// Adjacent assistants can be merged downstream, so only one reasoning block can survive
    /// a consecutive-assistant run.
    pub strips_reasoning_from_merged_assistants: bool,
}

pub const fn coverage(profile: SerializerProfile) -> HealingCoverage {
    match profile {
        SerializerProfile::OwnedLlmRunner
        | SerializerProfile::OwnedBroca
        | SerializerProfile::Pi => HealingCoverage {
            drops_empty_content: true,
            autofills_reasoning: true,
            merges_consecutive_assistants: false,
        },
        SerializerProfile::ClaudeCodeAnthropic => HealingCoverage {
            drops_empty_content: false,
            autofills_reasoning: false,
            merges_consecutive_assistants: false,
        },
        SerializerProfile::OpencodeAiSdk => HealingCoverage {
            drops_empty_content: false,
            autofills_reasoning: false,
            merges_consecutive_assistants: true,
        },
    }
}

/// All current profiles support tail reclamation.
///
/// Every shipping profile is a full-array consumer: the provider request is rebuilt from
/// the transformed array each pass, so prefix and tail rewrites both round-trip and the
/// profile default is true. Full-array apply is the only serving path — the
/// Thalamus gateway does not byte-splice the live tail — and the fail-open arm
/// forwards the current raw request, never stale or retained bytes, so Claude
/// Code is full-array too, which is what keeps phantom reclaims (mutations
/// frozen by the module that a splice never carries into the real context)
/// out of the serving path. A fenced pass forwards strictly more
/// current content, and any tail mutation a fence skips simply reapplies on the next
/// healthy pass. Prefix folding remains available regardless of the tail setting.
///
/// The exhaustive match forces each added profile to choose this behavior.
pub const fn tail_reclaim(profile: SerializerProfile) -> bool {
    // The match is exhaustive so each future profile requires an explicit decision.
    match profile {
        SerializerProfile::ClaudeCodeAnthropic
        | SerializerProfile::OwnedLlmRunner
        | SerializerProfile::OwnedBroca
        | SerializerProfile::Pi
        | SerializerProfile::OpencodeAiSdk => true,
    }
}

pub const fn quirk_residual(profile: SerializerProfile) -> QuirkResidual {
    match profile {
        SerializerProfile::OpencodeAiSdk => QuirkResidual {
            requires_non_anthropic_empty_sentinels: true,
            strips_reasoning_from_merged_assistants: true,
        },
        SerializerProfile::OwnedLlmRunner
        | SerializerProfile::OwnedBroca
        | SerializerProfile::ClaudeCodeAnthropic
        | SerializerProfile::Pi => QuirkResidual {
            requires_non_anthropic_empty_sentinels: false,
            strips_reasoning_from_merged_assistants: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_profiles_parse_and_round_trip_wire_ids() {
        for profile in SerializerProfile::all() {
            assert_eq!(SerializerProfile::parse(profile.wire_id()), Some(*profile));
            assert_eq!(serde_json::to_value(profile).unwrap(), profile.wire_id());
        }
        assert_eq!(SerializerProfile::parse(""), None);
        assert_eq!(SerializerProfile::parse("unknown"), None);
    }

    #[test]
    fn wire_ids_coverage_and_residual_are_pinned_per_profile_and_every_profile_reclaims_the_tail() {
        let drops_and_autofills = HealingCoverage {
            drops_empty_content: true,
            autofills_reasoning: true,
            merges_consecutive_assistants: false,
        };
        let no_coverage = HealingCoverage {
            drops_empty_content: false,
            autofills_reasoning: false,
            merges_consecutive_assistants: false,
        };
        let no_residual = QuirkResidual {
            requires_non_anthropic_empty_sentinels: false,
            strips_reasoning_from_merged_assistants: false,
        };
        let expected = [
            (
                SerializerProfile::OwnedLlmRunner,
                "owned-llmrunner",
                drops_and_autofills,
                no_residual,
            ),
            (
                SerializerProfile::OwnedBroca,
                "owned-broca",
                drops_and_autofills,
                no_residual,
            ),
            (
                SerializerProfile::ClaudeCodeAnthropic,
                "claude-code-anthropic",
                no_coverage,
                no_residual,
            ),
            (
                SerializerProfile::OpencodeAiSdk,
                "opencode-aisdk",
                HealingCoverage {
                    drops_empty_content: false,
                    autofills_reasoning: false,
                    merges_consecutive_assistants: true,
                },
                QuirkResidual {
                    requires_non_anthropic_empty_sentinels: true,
                    strips_reasoning_from_merged_assistants: true,
                },
            ),
            (
                SerializerProfile::Pi,
                "pi",
                drops_and_autofills,
                no_residual,
            ),
        ];
        assert_eq!(expected.len(), SerializerProfile::all().len());
        for (profile, wire_id, expected_coverage, expected_residual) in expected {
            assert!(SerializerProfile::all().contains(&profile), "{profile:?}");
            assert_eq!(profile.wire_id(), wire_id);
            assert_eq!(coverage(profile), expected_coverage, "{profile:?}");
            assert_eq!(quirk_residual(profile), expected_residual, "{profile:?}");
            assert!(tail_reclaim(profile), "{profile:?} must reclaim the tail");
        }
    }
}
