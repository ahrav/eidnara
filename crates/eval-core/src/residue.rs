use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use context_core::canonical_json::{ContractError, canonical_json_encode, protocol_digest};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const TRACE_DIGEST_PROTOCOL: &str = "eval-trace/v1";

/// The only clock-named fields permitted under [`Rule::Keep`].
pub const CLOCK_FIELD_KEEP_ALLOWLIST: [&str; 3] = ["now_ms", "observed_at_ms", "valid_time_ms"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Keep,
    Drop,
    /// `Presence` preserves only whether the value is null.
    Presence,
    /// Relative renumbers identifiers by first appearance, so runs with different
    /// IDs for the same objects compare equal; merged or swapped IDs differ.
    Relative,
}

/// Field names are snake_case, so `at` and `ms` are matched as tokens
/// (`created_at`, `created_at_ns`, `now_ms`).
pub fn is_clock_named(field: &str) -> bool {
    let token = |wanted: &str| field.split('_').any(|token| token == wanted);
    token("at")
        || token("ms")
        || field.contains("time")
        || field.contains("clock")
        || field.contains("deadline")
}

/// Host environment, filesystem location, boot, Unix identity, and store
/// incarnation values never enter a digest, so a field named for one may not be
/// `Keep`. Field names are snake_case, so `host`, `pid`, `ppid`, `root`, `path`,
/// `boot`, `uid`, `euid`, `gid`, and `egid` are matched as tokens (`host_name`,
/// `writer_pid`, `project_root`, `boot_id`, `owner_uid`) as well as `hostname`
/// and `process_id` anywhere. This is a name heuristic that
/// catches schema mistakes early; host neutrality itself is established by the
/// two-process digest equality test, not by this list.
pub fn is_never_kept(field: &str) -> bool {
    let token = |wanted: &str| field.split('_').any(|token| token == wanted);
    field.contains("hostname")
        || token("host")
        || field.contains("cwd")
        || token("pid")
        || token("ppid")
        || field.contains("process_id")
        || token("root")
        || token("path")
        || token("boot")
        || token("uid")
        || token("euid")
        || token("gid")
        || token("egid")
        || field.contains("incarnation")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidueError {
    ClockFieldKept { type_name: String, field: String },
    HostFieldKept { type_name: String, field: String },
    DuplicateType { type_name: String },
    DuplicateField { type_name: String, field: String },
    FieldNotSnakeCase { type_name: String, field: String },
    UnclassifiedField { type_name: String, field: String },
    MissingField { type_name: String, field: String },
    UnknownType { type_name: String },
    NotAnObject { type_name: String },
    NotCanonical(ContractError),
}

/// Variant names and fields are the message; callers match on the variant.
impl fmt::Display for ResidueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for ResidueError {}

impl From<ContractError> for ResidueError {
    fn from(error: ContractError) -> Self {
        Self::NotCanonical(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationSchema {
    type_name: String,
    rules: BTreeMap<String, Rule>,
}

impl ObservationSchema {
    /// Refuses a field not spelled in snake_case, a field declared twice, `Keep`
    /// on a host or incarnation field, and `Keep` on a clock-named field unless
    /// [`CLOCK_FIELD_KEEP_ALLOWLIST`] names it.
    pub fn new<'a>(
        type_name: &str,
        rules_iter: impl IntoIterator<Item = (&'a str, Rule)>,
    ) -> Result<Self, ResidueError> {
        let mut rules = BTreeMap::new();
        for (field, rule) in rules_iter {
            // The gates below match snake_case tokens, so nothing else is admitted:
            // lowercase ASCII words and digits joined by single underscores.
            if field.split('_').any(|token| {
                token.is_empty()
                    || !token
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            }) {
                return Err(ResidueError::FieldNotSnakeCase {
                    type_name: type_name.to_string(),
                    field: field.to_string(),
                });
            }
            if rules.insert(field.to_string(), rule).is_some() {
                return Err(ResidueError::DuplicateField {
                    type_name: type_name.to_string(),
                    field: field.to_string(),
                });
            }
        }
        for (field, _) in rules.iter().filter(|(_, rule)| **rule == Rule::Keep) {
            if is_never_kept(field) {
                return Err(ResidueError::HostFieldKept {
                    type_name: type_name.to_string(),
                    field: field.clone(),
                });
            }
            if is_clock_named(field) && !CLOCK_FIELD_KEEP_ALLOWLIST.contains(&field.as_str()) {
                return Err(ResidueError::ClockFieldKept {
                    type_name: type_name.to_string(),
                    field: field.clone(),
                });
            }
        }
        Ok(Self {
            type_name: type_name.to_string(),
            rules,
        })
    }

    pub fn rules(&self) -> &BTreeMap<String, Rule> {
        &self.rules
    }

    pub fn residue(&self) -> impl Iterator<Item = ResidueEntry> + '_ {
        self.rules
            .iter()
            .filter(|(_, rule)| **rule != Rule::Keep)
            .map(|(field, rule)| ResidueEntry {
                type_name: self.type_name.clone(),
                field: field.clone(),
                rule: *rule,
            })
    }

    /// Applies every rule to `observation`, whose field set must equal the schema's.
    pub(crate) fn reduce(
        &self,
        observation: &Value,
        relative: &mut RelativeDomains,
    ) -> Result<Value, ResidueError> {
        let Some(fields) = observation.as_object() else {
            return Err(ResidueError::NotAnObject {
                type_name: self.type_name.clone(),
            });
        };
        let observed: BTreeSet<&str> = fields.keys().map(String::as_str).collect();
        let classified: BTreeSet<&str> = self.rules.keys().map(String::as_str).collect();
        if let Some(field) = observed.difference(&classified).next() {
            return Err(ResidueError::UnclassifiedField {
                type_name: self.type_name.clone(),
                field: field.to_string(),
            });
        }
        if let Some(field) = classified.difference(&observed).next() {
            return Err(ResidueError::MissingField {
                type_name: self.type_name.clone(),
                field: field.to_string(),
            });
        }
        // Every fallible step runs before the first mutation, so a refused
        // observation leaves `relative` exactly as it found it, and a recorded
        // entry always digests.
        let mut reduced = Map::new();
        let mut renumber = Vec::new();
        for (field, value) in fields {
            match self.rules[field] {
                Rule::Keep => {
                    canonical_json_encode(value)?;
                    reduced.insert(field.clone(), value.clone())
                }
                Rule::Drop => None,
                Rule::Presence => reduced.insert(field.clone(), Value::Bool(!value.is_null())),
                Rule::Relative => {
                    renumber.push((field, canonical_json_encode(value)?));
                    None
                }
            };
        }
        for (field, key) in renumber {
            let index = relative.renumber(&self.type_name, field, key);
            reduced.insert(field.clone(), Value::from(index));
        }
        Ok(Value::Object(reduced))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidueEntry {
    pub type_name: String,
    pub field: String,
    pub rule: Rule,
}

/// Relative values are numbered by first appearance for each `(type, field)`.
#[derive(Debug, Default, Clone)]
pub(crate) struct RelativeDomains {
    seen: BTreeMap<(String, String), BTreeMap<String, u64>>,
}

impl RelativeDomains {
    /// `key` is the canonical encoding of the observed value.
    fn renumber(&mut self, type_name: &str, field: &str, key: String) -> u64 {
        let domain = self
            .seen
            .entry((type_name.to_string(), field.to_string()))
            .or_default();
        let next = domain.len() as u64 + 1;
        *domain.entry(key).or_insert(next)
    }
}

#[derive(Debug, Clone)]
pub struct SemanticTrace {
    schemas: BTreeMap<String, ObservationSchema>,
    relative: RelativeDomains,
    entries: Vec<Value>,
}

impl SemanticTrace {
    pub fn new(schemas: impl IntoIterator<Item = ObservationSchema>) -> Result<Self, ResidueError> {
        let mut registered = BTreeMap::new();
        for schema in schemas {
            if registered.contains_key(&schema.type_name) {
                return Err(ResidueError::DuplicateType {
                    type_name: schema.type_name,
                });
            }
            registered.insert(schema.type_name.clone(), schema);
        }
        Ok(Self {
            schemas: registered,
            relative: RelativeDomains::default(),
            entries: Vec::new(),
        })
    }

    pub fn record(&mut self, type_name: &str, observation: &Value) -> Result<(), ResidueError> {
        let Some(schema) = self.schemas.get(type_name) else {
            return Err(ResidueError::UnknownType {
                type_name: type_name.to_string(),
            });
        };
        let fields = schema.reduce(observation, &mut self.relative)?;
        self.entries
            .push(serde_json::json!({"type": type_name, "fields": fields}));
        Ok(())
    }

    pub fn residue(&self) -> Vec<ResidueEntry> {
        self.schemas
            .values()
            .flat_map(ObservationSchema::residue)
            .collect()
    }

    pub fn digest(&self) -> Result<String, ResidueError> {
        Ok(protocol_digest(
            TRACE_DIGEST_PROTOCOL,
            &Value::Array(self.entries.clone()),
        )?)
    }
}
