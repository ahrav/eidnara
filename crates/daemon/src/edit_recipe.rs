//! Two-source edit recipe: the application contract a transform response carries instead of
//! a complete output array.
//!
//! A recipe names whole messages from two sources, the request's own input and one previously
//! applied output, and inserts complete literal values for everything else. Operation order is
//! output order; omission deletes. Every reference is an index into a captured source, valid only
//! while the caller's copy of that source is unchanged, so the applier validates the whole recipe,
//! sizes the result with checked arithmetic, and only then builds a new vector of shared handles.
//! It never edits a source.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::ops::Range;
use std::sync::Arc;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Revision tokens are opaque; this bound keeps a hostile daemon or client from turning them into a payload.
pub const MAX_REVISION_BYTES: usize = 128;
/// The reconstructed array is bounded on its own, apart from the wire frame that carried the recipe.
pub const MAX_RECONSTRUCTED_BYTES: usize = crate::dispatch::MAX_WIRE_BODY_BYTES;
/// Largest integer both languages read exactly; JavaScript indexes above it lose precision.
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
/// Bounds equality tests per output message to prevent shared keys from making a build quadratic.
pub const MAX_CONFIRM_PROBES: usize = 8;
const MAX_JSON_NESTING: usize = 127;

/// Opaque, nonempty, at most [`MAX_REVISION_BYTES`] UTF-8 bytes. Neither a hash nor authorization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Revision(String);

impl Revision {
    pub fn parse(value: &str) -> Result<Self, RecipeError> {
        if value.is_empty() {
            return Err(RecipeError::EmptyRevision);
        }
        if value.len() > MAX_REVISION_BYTES {
            return Err(RecipeError::RevisionTooLong { bytes: value.len() });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Input,
    Previous,
}

impl Source {
    pub const fn wire_id(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Previous => "previous",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Operation {
    /// `count` whole messages from `source` starting at `start`, in source order.
    Keep {
        source: Source,
        start: u64,
        count: u64,
    },
    /// Complete final values; never a merge into an original.
    Insert { values: Vec<Arc<Value>> },
}

/// The application contract of one `status: ok` response. `from_json` is the validating entry
/// point; `apply` trusts the field-level invariants it establishes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recipe {
    pub base_revision: Revision,
    pub output_revision: Revision,
    /// Present exactly when an operation keeps from `previous`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_output_revision: Option<Revision>,
    pub operations: Vec<Operation>,
}

/// One captured source: its revision, its whole-message values, and each value's canonical JSON
/// length. The lengths come from capture or serialization time; the applier never re-serializes a
/// kept value.
#[derive(Debug, Clone, Copy)]
pub struct SourceBase<'a> {
    pub revision: &'a Revision,
    pub values: &'a [Arc<Value>],
    pub lengths: &'a [usize],
}

/// A reconstructed array and the per-entry lengths needed to retain it as a future source.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedRecipe {
    pub values: Vec<Arc<Value>>,
    pub lengths: Vec<usize>,
    /// Canonical JSON size of the array, including brackets and commas.
    pub bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeError {
    EmptyRevision,
    RevisionTooLong { bytes: usize },
    WrongBaseRevision,
    WrongPreviousRevision,
    MissingPreviousBase,
    UnusedPreviousRevision,
    UnknownOperation(String),
    UnknownSource(String),
    MissingField(&'static str),
    UnknownField(String),
    UnsafeInteger(&'static str),
    ZeroCount,
    EmptyInsert,
    BackwardRange { source: Source, index: usize },
    OutOfBounds { source: Source, index: usize },
    Overflow,
    OutputTooLarge { bytes: usize },
    LengthMismatch { source: Source },
    Malformed(String),
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRevision => write!(f, "revision is empty"),
            Self::RevisionTooLong { bytes } => {
                write!(f, "revision is {bytes} bytes; limit {MAX_REVISION_BYTES}")
            }
            Self::WrongBaseRevision => write!(f, "base_revision does not name the input"),
            Self::WrongPreviousRevision => {
                write!(
                    f,
                    "previous_output_revision does not name the previous output"
                )
            }
            Self::MissingPreviousBase => write!(f, "a previous keep has no previous base"),
            Self::UnusedPreviousRevision => {
                write!(
                    f,
                    "previous_output_revision is present without a previous keep"
                )
            }
            Self::UnknownOperation(op) => write!(f, "unknown operation {op:?}"),
            Self::UnknownSource(source) => write!(f, "unknown source {source:?}"),
            Self::MissingField(field) => write!(f, "missing field {field}"),
            Self::UnknownField(field) => write!(f, "unknown field {field:?}"),
            Self::UnsafeInteger(field) => write!(f, "{field} is not a safe nonnegative integer"),
            Self::ZeroCount => write!(f, "keep count is zero"),
            Self::EmptyInsert => write!(f, "insert has no values"),
            Self::BackwardRange { source, index } => write!(
                f,
                "operation {index} keeps a {} range that repeats or moves backward",
                source.wire_id()
            ),
            Self::OutOfBounds { source, index } => {
                write!(
                    f,
                    "operation {index} keeps past the end of {}",
                    source.wire_id()
                )
            }
            Self::Overflow => write!(f, "range or size arithmetic overflowed"),
            Self::OutputTooLarge { bytes } => write!(
                f,
                "reconstructed array is {bytes} bytes; limit {MAX_RECONSTRUCTED_BYTES}"
            ),
            Self::LengthMismatch { source } => {
                write!(f, "{} lengths do not match its values", source.wire_id())
            }
            Self::Malformed(detail) => write!(f, "malformed recipe: {detail}"),
        }
    }
}

impl std::error::Error for RecipeError {}

/// Reads a JSON number as an index or count both languages read exactly. JavaScript cannot see the
/// lexical form, so `1.0`, `1e3`, and `-0` count as integers here too.
fn safe_integer(value: &Value, field: &'static str) -> Result<u64, RecipeError> {
    if let Some(n) = value.as_u64() {
        return (n <= MAX_SAFE_INTEGER)
            .then_some(n)
            .ok_or(RecipeError::UnsafeInteger(field));
    }
    match value.as_f64() {
        Some(v) if v.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER as f64).contains(&v) => Ok(v as u64),
        _ => Err(RecipeError::UnsafeInteger(field)),
    }
}

fn required<'a>(
    map: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a Value, RecipeError> {
    map.get(field).ok_or(RecipeError::MissingField(field))
}

fn reject_unknown_fields(
    map: &serde_json::Map<String, Value>,
    known: &[&str],
) -> Result<(), RecipeError> {
    match map.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(RecipeError::UnknownField(key.clone())),
        None => Ok(()),
    }
}

enum ParsedOperation<'a> {
    Keep {
        source: Source,
        start: u64,
        count: u64,
    },
    Insert {
        values: &'a [Value],
    },
}

impl ParsedOperation<'_> {
    fn into_owned(self) -> Operation {
        match self {
            Self::Keep {
                source,
                start,
                count,
            } => Operation::Keep {
                source,
                start,
                count,
            },
            Self::Insert { values } => Operation::Insert {
                values: values.iter().cloned().map(Arc::new).collect(),
            },
        }
    }
}

impl Operation {
    fn parse_json(value: &Value) -> Result<ParsedOperation<'_>, RecipeError> {
        let map = value
            .as_object()
            .ok_or_else(|| RecipeError::Malformed("operation is not an object".into()))?;
        let op = required(map, "op")?
            .as_str()
            .ok_or_else(|| RecipeError::Malformed("op is not a string".into()))?;
        match op {
            "keep" => {
                reject_unknown_fields(map, &["op", "source", "start", "count"])?;
                let source = match required(map, "source")?.as_str() {
                    Some("input") => Source::Input,
                    Some("previous") => Source::Previous,
                    Some(other) => return Err(RecipeError::UnknownSource(other.to_owned())),
                    None => return Err(RecipeError::Malformed("source is not a string".into())),
                };
                let start = safe_integer(required(map, "start")?, "start")?;
                let count = safe_integer(required(map, "count")?, "count")?;
                if count == 0 {
                    return Err(RecipeError::ZeroCount);
                }
                Ok(ParsedOperation::Keep {
                    source,
                    start,
                    count,
                })
            }
            "insert" => {
                reject_unknown_fields(map, &["op", "values"])?;
                let values = required(map, "values")?
                    .as_array()
                    .ok_or_else(|| RecipeError::Malformed("values is not an array".into()))?;
                if values.is_empty() {
                    return Err(RecipeError::EmptyInsert);
                }
                Ok(ParsedOperation::Insert { values })
            }
            other => Err(RecipeError::UnknownOperation(other.to_owned())),
        }
    }

    /// Field-level validation: opcode, source, integer domain, count, and nonempty values.
    /// Range validity against a base belongs to [`Recipe::apply`].
    pub fn from_json(value: &Value) -> Result<Self, RecipeError> {
        Self::parse_json(value).map(ParsedOperation::into_owned)
    }
}

enum ContainerChildren<'a> {
    Array(std::slice::Iter<'a, Value>),
    Object(serde_json::map::Values<'a>),
}

impl<'a> ContainerChildren<'a> {
    fn new(value: &'a Value) -> Option<Self> {
        match value {
            Value::Array(values) => Some(Self::Array(values.iter())),
            Value::Object(values) => Some(Self::Object(values.values())),
            _ => None,
        }
    }
}

impl<'a> Iterator for ContainerChildren<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Array(values) => values.next(),
            Self::Object(values) => values.next(),
        }
    }
}

/// Validates the depth bound and returns the maximum pending container frames.
fn validate_json_nesting(value: &Value) -> Result<usize, RecipeError> {
    let Some(children) = ContainerChildren::new(value) else {
        return Ok(0);
    };
    let mut work = vec![(0usize, children)];
    let mut max_pending = work.len();
    while let Some((depth, children)) = work.last_mut() {
        let next = children.next().map(|child| (*depth + 1, child));
        let Some((depth, child)) = next else {
            work.pop();
            continue;
        };
        let Some(children) = ContainerChildren::new(child) else {
            continue;
        };
        if depth >= MAX_JSON_NESTING {
            return Err(RecipeError::Malformed(
                "recipe exceeds the JSON nesting limit".into(),
            ));
        }
        work.push((depth, children));
        max_pending = max_pending.max(work.len());
    }
    Ok(max_pending)
}

impl Recipe {
    /// Structural validation of a decoded response body. Base-relative checks run in [`Self::apply`].
    pub fn from_json(value: &Value) -> Result<Self, RecipeError> {
        validate_json_nesting(value)?;
        let map = value
            .as_object()
            .ok_or_else(|| RecipeError::Malformed("recipe is not an object".into()))?;
        let revision = |value: &Value, field: &'static str| -> Result<Revision, RecipeError> {
            value
                .as_str()
                .ok_or_else(|| RecipeError::Malformed(format!("{field} is not a string")))
                .and_then(Revision::parse)
        };
        let base_revision = revision(required(map, "base_revision")?, "base_revision")?;
        let output_revision = revision(required(map, "output_revision")?, "output_revision")?;
        let previous_output_revision = map
            .get("previous_output_revision")
            .map(|value| revision(value, "previous_output_revision"))
            .transpose()?;
        let operation_values = required(map, "operations")?
            .as_array()
            .ok_or_else(|| RecipeError::Malformed("operations is not an array".into()))?;
        for value in operation_values {
            Operation::parse_json(value)?;
        }
        let operations = operation_values
            .iter()
            .map(Operation::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let uses_previous = operations.iter().any(|operation| {
            matches!(
                operation,
                Operation::Keep {
                    source: Source::Previous,
                    ..
                }
            )
        });
        if uses_previous && previous_output_revision.is_none() {
            return Err(RecipeError::MissingPreviousBase);
        }
        if !uses_previous && previous_output_revision.is_some() {
            return Err(RecipeError::UnusedPreviousRevision);
        }
        Ok(Self {
            base_revision,
            output_revision,
            previous_output_revision,
            operations,
        })
    }

    /// Validates every operation against the bases, sizes the result with checked arithmetic, and
    /// only then allocates the output. A failure leaves the caller with nothing to roll back. The
    /// returned length is the canonical JSON size of the reconstructed array, brackets and commas
    /// included.
    pub fn apply(
        &self,
        input: SourceBase<'_>,
        previous: Option<SourceBase<'_>>,
    ) -> Result<AppliedRecipe, RecipeError> {
        if input.values.len() != input.lengths.len() {
            return Err(RecipeError::LengthMismatch {
                source: Source::Input,
            });
        }
        if previous.is_some_and(|base| base.values.len() != base.lengths.len()) {
            return Err(RecipeError::LengthMismatch {
                source: Source::Previous,
            });
        }
        if &self.base_revision != input.revision {
            return Err(RecipeError::WrongBaseRevision);
        }
        let mut cursors = [0u64; 2];
        let mut entries = 0usize;
        // Brackets first; each entry then pays its bytes plus one comma after the first.
        let mut bytes = 2usize;
        let mut inserted_lengths = Vec::new();
        for (index, operation) in self.operations.iter().enumerate() {
            match operation {
                Operation::Keep {
                    source,
                    start,
                    count,
                } => {
                    let base = match source {
                        Source::Input => input,
                        Source::Previous => match (previous, &self.previous_output_revision) {
                            (Some(base), Some(revision)) if base.revision == revision => base,
                            (Some(_), Some(_)) => return Err(RecipeError::WrongPreviousRevision),
                            _ => return Err(RecipeError::MissingPreviousBase),
                        },
                    };
                    let end = start.checked_add(*count).ok_or(RecipeError::Overflow)?;
                    let cursor = &mut cursors[*source as usize];
                    if *start < *cursor {
                        return Err(RecipeError::BackwardRange {
                            source: *source,
                            index,
                        });
                    }
                    if end > base.values.len() as u64 {
                        return Err(RecipeError::OutOfBounds {
                            source: *source,
                            index,
                        });
                    }
                    *cursor = end;
                    // `end <= len <= usize::MAX`, so both casts are exact.
                    let range: Range<usize> = *start as usize..end as usize;
                    for length in &base.lengths[range] {
                        bytes = add_entry(bytes, entries, *length)?;
                        entries += 1;
                    }
                }
                Operation::Insert { values } => {
                    for value in values {
                        let length = canonical_len(value)?;
                        bytes = add_entry(bytes, entries, length)?;
                        entries += 1;
                        inserted_lengths.push(length);
                    }
                }
            }
        }
        if bytes > MAX_RECONSTRUCTED_BYTES {
            return Err(RecipeError::OutputTooLarge { bytes });
        }
        let mut output = Vec::with_capacity(entries);
        let mut lengths = Vec::with_capacity(entries);
        let mut inserted_index = 0usize;
        for operation in &self.operations {
            match operation {
                Operation::Keep {
                    source,
                    start,
                    count,
                } => {
                    let base = match source {
                        Source::Input => input,
                        Source::Previous => previous.ok_or(RecipeError::MissingPreviousBase)?,
                    };
                    let end = start.checked_add(*count).ok_or(RecipeError::Overflow)?;
                    let range: Range<usize> = *start as usize..end as usize;
                    output.extend(base.values[range.clone()].iter().cloned());
                    lengths.extend_from_slice(&base.lengths[range]);
                }
                Operation::Insert { values } => {
                    let end = inserted_index + values.len();
                    output.extend(values.iter().cloned());
                    lengths.extend_from_slice(&inserted_lengths[inserted_index..end]);
                    inserted_index = end;
                }
            }
        }
        Ok(AppliedRecipe {
            values: output,
            lengths,
            bytes,
        })
    }
}

/// One whole message of a source or of the final output, with the provenance key that nominates
/// it as a candidate. Keys only nominate; the builder confirms every keep by full value equality.
#[derive(Debug, Clone)]
pub struct Keyed<'a, K> {
    pub key: K,
    pub value: &'a Arc<Value>,
}

/// Ascending positions per key within one source, so a nomination at or after the cursor is one
/// binary search rather than a scan.
struct KeyIndex<K> {
    positions: HashMap<K, Vec<usize>>,
}

impl<K: Hash + Eq + Clone> KeyIndex<K> {
    fn new(entries: &[Keyed<'_, K>]) -> Self {
        let mut positions: HashMap<K, Vec<usize>> = HashMap::with_capacity(entries.len());
        for (position, entry) in entries.iter().enumerate() {
            positions
                .entry(entry.key.clone())
                .or_default()
                .push(position);
        }
        Self { positions }
    }

    /// The shared `remaining` budget bounds probes across previous and input lookups.
    fn confirm(
        &self,
        entries: &[Keyed<'_, K>],
        cursor: usize,
        wanted: &Keyed<'_, K>,
        remaining: &mut usize,
    ) -> Option<usize> {
        let positions = self.positions.get(&wanted.key)?;
        let first = positions.partition_point(|position| *position < cursor);
        for position in positions[first..].iter().take(*remaining).copied() {
            *remaining -= 1;
            let candidate = entries[position].value;
            if Arc::ptr_eq(candidate, wanted.value) || candidate == wanted.value {
                return Some(position);
            }
        }
        None
    }
}

/// Builds the recipe that reconstructs `output` from `input` and, when present, `previous`.
///
/// Each output entry prefers an equal `previous` message, then an equal `input` message.
/// Anything else becomes a literal: an unmatched message, a message that repeats or moves backward
/// relative to its source cursor, or a match beyond [`MAX_CONFIRM_PROBES`] same-key candidates.
/// Source cursors advance independently. Adjacent keeps of one source and adjacent inserts coalesce.
pub fn build_recipe<K: Hash + Eq + Clone>(
    output: &[Keyed<'_, K>],
    input: (&Revision, &[Keyed<'_, K>]),
    previous: Option<(&Revision, &[Keyed<'_, K>])>,
    output_revision: Revision,
) -> Recipe {
    let input_index = KeyIndex::new(input.1);
    let previous_index = previous.map(|(_, entries)| KeyIndex::new(entries));
    let mut cursors = [0usize; 2];
    let mut operations: Vec<Operation> = Vec::new();
    let mut used_previous = false;
    for entry in output {
        let mut remaining = MAX_CONFIRM_PROBES;
        let previous_hit = previous.and_then(|(_, entries)| {
            previous_index
                .as_ref()?
                .confirm(
                    entries,
                    cursors[Source::Previous as usize],
                    entry,
                    &mut remaining,
                )
                .map(|position| (Source::Previous, position))
        });
        let hit = previous_hit.or_else(|| {
            input_index
                .confirm(
                    input.1,
                    cursors[Source::Input as usize],
                    entry,
                    &mut remaining,
                )
                .map(|position| (Source::Input, position))
        });
        match hit {
            Some((source, position)) => {
                cursors[source as usize] = position + 1;
                used_previous |= source == Source::Previous;
                match operations.last_mut() {
                    Some(Operation::Keep {
                        source: last,
                        start,
                        count,
                    }) if *last == source && *start + *count == position as u64 => *count += 1,
                    _ => operations.push(Operation::Keep {
                        source,
                        start: position as u64,
                        count: 1,
                    }),
                }
            }
            None => match operations.last_mut() {
                Some(Operation::Insert { values }) => values.push(Arc::clone(entry.value)),
                _ => operations.push(Operation::Insert {
                    values: vec![Arc::clone(entry.value)],
                }),
            },
        }
    }
    Recipe {
        base_revision: input.0.clone(),
        output_revision,
        previous_output_revision: previous
            .filter(|_| used_previous)
            .map(|(revision, _)| revision.clone()),
        operations,
    }
}

fn add_entry(bytes: usize, entries: usize, length: usize) -> Result<usize, RecipeError> {
    let separator = usize::from(entries > 0);
    let sum = bytes
        .checked_add(length)
        .and_then(|sum| sum.checked_add(separator))
        .ok_or(RecipeError::Overflow)?;
    if sum > MAX_RECONSTRUCTED_BYTES {
        return Err(RecipeError::OutputTooLarge { bytes: sum });
    }
    Ok(sum)
}

/// Compact `serde_json` length, the same rule the client's `serdeJsonCompact` follows. A value
/// parsed from `1.0` re-emits as `1.0` here and as `1` in JavaScript, so the two languages agree on
/// acceptance but may differ by a few bytes in size for non-integer numeric content.
pub fn canonical_len(value: &Value) -> Result<usize, RecipeError> {
    crate::dispatch::measure_json(value).map_err(|error| match error {
        crate::dispatch::PreparedOutputError::BodyTooLarge { len, .. } => {
            RecipeError::OutputTooLarge { bytes: len }
        }
        crate::dispatch::PreparedOutputError::LengthOverflow => RecipeError::Overflow,
        other => RecipeError::Malformed(other.to_string()),
    })
}

struct PreservedValue(Value);

#[derive(Clone, Copy)]
struct PreservedValueSeed {
    depth: usize,
}

struct PreservedValueVisitor {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for PreservedValueSeed {
    type Value = PreservedValue;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(PreservedValueVisitor { depth: self.depth })
    }
}

impl<'de> Visitor<'de> for PreservedValueVisitor {
    type Value = PreservedValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::Number(value.into())))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(PreservedValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(PreservedValue(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        if self.depth >= MAX_JSON_NESTING {
            return Err(de::Error::custom("recipe exceeds the JSON nesting limit"));
        }
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(PreservedValueSeed {
            depth: self.depth + 1,
        })? {
            values.push(value.0);
        }
        Ok(PreservedValue(Value::Array(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        if self.depth >= MAX_JSON_NESTING {
            return Err(de::Error::custom("recipe exceeds the JSON nesting limit"));
        }
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(PreservedValueSeed {
                depth: self.depth + 1,
            })?;
            values.insert(key, value.0);
        }
        Ok(PreservedValue(Value::Object(values)))
    }
}

impl<'de> Deserialize<'de> for PreservedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        PreservedValueSeed { depth: 0 }.deserialize(deserializer)
    }
}

impl<'de> Deserialize<'de> for Recipe {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = PreservedValue::deserialize(deserializer)?.0;
        Recipe::from_json(&value).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeSeed;
    use serde_json::json;

    struct NestedArray {
        remaining: usize,
    }

    struct OneElement {
        remaining: usize,
        emitted: bool,
    }

    impl<'de> Deserializer<'de> for NestedArray {
        type Error = serde::de::value::Error;

        fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            if self.remaining == 0 {
                visitor.visit_unit()
            } else {
                visitor.visit_seq(OneElement {
                    remaining: self.remaining,
                    emitted: false,
                })
            }
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum
            identifier ignored_any
        }
    }

    impl<'de> SeqAccess<'de> for OneElement {
        type Error = serde::de::value::Error;

        fn next_element_seed<T: DeserializeSeed<'de>>(
            &mut self,
            seed: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            if self.emitted {
                return Ok(None);
            }
            self.emitted = true;
            seed.deserialize(NestedArray {
                remaining: self.remaining - 1,
            })
            .map(Some)
        }
    }

    fn shared(values: &[Value]) -> Vec<Arc<Value>> {
        values.iter().cloned().map(Arc::new).collect()
    }

    fn lengths(values: &[Arc<Value>]) -> Vec<usize> {
        values
            .iter()
            .map(|value| canonical_len(value).expect("value length"))
            .collect()
    }

    fn revision(text: &str) -> Revision {
        Revision::parse(text).expect("valid revision")
    }

    #[test]
    fn revision_bounds_are_exact() {
        assert_eq!(Revision::parse(""), Err(RecipeError::EmptyRevision));
        let exact = "r".repeat(MAX_REVISION_BYTES);
        assert!(Revision::parse(&exact).is_ok());
        let over = format!("{exact}r");
        assert_eq!(
            Revision::parse(&over),
            Err(RecipeError::RevisionTooLong {
                bytes: MAX_REVISION_BYTES + 1
            })
        );
        // Multibyte text counts bytes, not characters.
        let emoji = "😀".repeat(33);
        assert!(Revision::parse(&emoji).is_err());
    }

    #[test]
    fn keeps_two_sources_and_inserts_literals_in_order() {
        let input = shared(&[json!({"id": "a"}), json!({"id": "b"}), json!({"id": "c"})]);
        let previous = shared(&[json!({"id": "p0"}), json!({"id": "p1"})]);
        let recipe = Recipe::from_json(&json!({
            "base_revision": "base-1",
            "output_revision": "out-1",
            "previous_output_revision": "prev-1",
            "operations": [
                {"op": "keep", "source": "previous", "start": 0, "count": 2},
                {"op": "insert", "values": [{"id": "s"}]},
                {"op": "keep", "source": "input", "start": 2, "count": 1},
            ],
        }))
        .expect("valid recipe");
        let base = revision("base-1");
        let prev = revision("prev-1");
        let applied = recipe
            .apply(
                SourceBase {
                    revision: &base,
                    values: &input,
                    lengths: &lengths(&input),
                },
                Some(SourceBase {
                    revision: &prev,
                    values: &previous,
                    lengths: &lengths(&previous),
                }),
            )
            .expect("applies");
        let output = &applied.values;
        assert_eq!(output.len(), 4);
        let rendered: Vec<&Value> = output.iter().map(Arc::as_ref).collect();
        assert_eq!(
            applied.bytes,
            serde_json::to_vec(&rendered).expect("serializes").len()
        );
        assert!(Arc::ptr_eq(&output[0], &previous[0]));
        assert!(Arc::ptr_eq(&output[1], &previous[1]));
        assert_eq!(*output[2], json!({"id": "s"}));
        assert!(Arc::ptr_eq(&output[3], &input[2]));
        assert_eq!(*input[0], json!({"id": "a"}));
        assert_eq!(applied.lengths, lengths(output));
    }

    #[test]
    fn sizes_the_output_before_allocating_and_rejects_one_byte_over() {
        // Two kept entries plus brackets and one comma exactly fill the limit.
        let input = shared(&[json!(1), json!(2)]);
        let half = (MAX_RECONSTRUCTED_BYTES - 3) / 2;
        let exact = [half, MAX_RECONSTRUCTED_BYTES - 3 - half];
        let recipe = Recipe::from_json(&json!({
            "base_revision": "b",
            "output_revision": "o",
            "operations": [{"op": "keep", "source": "input", "start": 0, "count": 2}],
        }))
        .expect("valid");
        let base = revision("b");
        let accepted = recipe.apply(
            SourceBase {
                revision: &base,
                values: &input,
                lengths: &exact,
            },
            None,
        );
        assert_eq!(
            accepted,
            Ok(AppliedRecipe {
                values: input.clone(),
                lengths: exact.to_vec(),
                bytes: MAX_RECONSTRUCTED_BYTES,
            })
        );
        let over = [exact[0] + 1, exact[1]];
        let rejected = recipe.apply(
            SourceBase {
                revision: &base,
                values: &input,
                lengths: &over,
            },
            None,
        );
        assert_eq!(
            rejected,
            Err(RecipeError::OutputTooLarge {
                bytes: MAX_RECONSTRUCTED_BYTES + 1
            })
        );
        let huge = [usize::MAX, 1];
        assert_eq!(
            recipe.apply(
                SourceBase {
                    revision: &base,
                    values: &input,
                    lengths: &huge,
                },
                None,
            ),
            Err(RecipeError::Overflow)
        );
    }

    #[test]
    fn integral_floats_and_negative_zero_read_as_indexes() {
        let input = shared(&[json!(1), json!(2)]);
        let base = revision("b");
        let recipe = Recipe::from_json(&json!({
            "base_revision": "b",
            "output_revision": "o",
            "operations": [{"op": "keep", "source": "input", "start": -0.0, "count": 1e0}],
        }))
        .expect("integral floats are safe integers");
        let output = recipe
            .apply(
                SourceBase {
                    revision: &base,
                    values: &input,
                    lengths: &lengths(&input),
                },
                None,
            )
            .expect("applies");
        assert_eq!(output.values.len(), 1);
        assert_eq!(
            Recipe::from_json(&json!({
                "base_revision": "b",
                "output_revision": "o",
                "operations": [{"op": "keep", "source": "input", "start": 0, "count": 1.5}],
            })),
            Err(RecipeError::UnsafeInteger("count"))
        );
    }

    #[test]
    fn rejects_range_and_base_faults_without_output() {
        let input = shared(&[json!(1), json!(2), json!(3)]);
        let base = revision("b");
        let input_base = SourceBase {
            revision: &base,
            values: &input,
            lengths: &lengths(&input),
        };
        let short = [1usize];
        assert_eq!(
            Recipe::from_json(&json!({
                "base_revision": "b",
                "output_revision": "o",
                "operations": [],
            }))
            .expect("valid")
            .apply(
                SourceBase {
                    lengths: &short,
                    ..input_base
                },
                None
            ),
            Err(RecipeError::LengthMismatch {
                source: Source::Input
            })
        );
        assert_eq!(
            Recipe::from_json(&json!({
                "base_revision": "b",
                "output_revision": "o",
                "operations": [],
            }))
            .expect("valid")
            .apply(
                input_base,
                Some(SourceBase {
                    revision: &base,
                    values: &input,
                    lengths: &short,
                })
            ),
            Err(RecipeError::LengthMismatch {
                source: Source::Previous
            })
        );
        let apply = |operations: Value| {
            Recipe::from_json(&json!({
                "base_revision": "b",
                "output_revision": "o",
                "operations": operations,
            }))
            .and_then(|recipe| recipe.apply(input_base, None))
        };
        assert_eq!(
            apply(json!([{"op": "keep", "source": "input", "start": 1, "count": 3}])),
            Err(RecipeError::OutOfBounds {
                source: Source::Input,
                index: 0
            })
        );
        assert_eq!(
            apply(json!([
                {"op": "keep", "source": "input", "start": 1, "count": 1},
                {"op": "keep", "source": "input", "start": 1, "count": 1},
            ])),
            Err(RecipeError::BackwardRange {
                source: Source::Input,
                index: 1
            })
        );
        assert_eq!(
            apply(json!([
                {"op": "keep", "source": "input", "start": 2, "count": 1},
                {"op": "keep", "source": "input", "start": 0, "count": 1},
            ])),
            Err(RecipeError::BackwardRange {
                source: Source::Input,
                index: 1
            })
        );
        assert_eq!(
            apply(
                json!([{"op": "keep", "source": "input", "start": MAX_SAFE_INTEGER, "count": 1}])
            ),
            Err(RecipeError::OutOfBounds {
                source: Source::Input,
                index: 0
            })
        );
        assert_eq!(
            apply(json!([{"op": "keep", "source": "input", "start": 0, "count": 0}])),
            Err(RecipeError::ZeroCount)
        );
        assert_eq!(
            apply(json!([{"op": "keep", "source": "input", "start": 1.5, "count": 1}])),
            Err(RecipeError::UnsafeInteger("start"))
        );
        assert_eq!(
            apply(json!([{"op": "keep", "source": "input", "start": -1, "count": 1}])),
            Err(RecipeError::UnsafeInteger("start"))
        );
        assert_eq!(
            apply(
                json!([{"op": "keep", "source": "input", "start": MAX_SAFE_INTEGER + 1, "count": 1}])
            ),
            Err(RecipeError::UnsafeInteger("start"))
        );
        assert_eq!(
            apply(json!([{"op": "insert", "values": []}])),
            Err(RecipeError::EmptyInsert)
        );
        assert_eq!(
            apply(json!([{"op": "replace", "values": [1]}])),
            Err(RecipeError::UnknownOperation("replace".into()))
        );
        assert_eq!(
            apply(json!([{"op": "keep", "source": "cache", "start": 0, "count": 1}])),
            Err(RecipeError::UnknownSource("cache".into()))
        );
        assert_eq!(
            apply(json!([{"op": "keep", "source": "input", "start": 0, "count": 1, "path": "x"}])),
            Err(RecipeError::UnknownField("path".into()))
        );
        assert_eq!(
            apply(json!([{"op": "keep", "source": "previous", "start": 0, "count": 1}])),
            Err(RecipeError::MissingPreviousBase)
        );
        // A valid prefix followed by a malformed final operation rejects the whole recipe.
        assert_eq!(
            apply(json!([
                {"op": "keep", "source": "input", "start": 0, "count": 2},
                {"op": "keep", "source": "input", "start": 2, "count": 2},
            ])),
            Err(RecipeError::OutOfBounds {
                source: Source::Input,
                index: 1
            })
        );
    }

    #[test]
    fn revision_binding_covers_both_sources() {
        let input = shared(&[json!(1)]);
        let previous = shared(&[json!(2)]);
        let base = revision("b");
        let other = revision("other");
        let prev = revision("p");
        let recipe = Recipe::from_json(&json!({
            "base_revision": "b",
            "output_revision": "o",
            "previous_output_revision": "p",
            "operations": [{"op": "keep", "source": "previous", "start": 0, "count": 1}],
        }))
        .expect("valid");
        let input_base = SourceBase {
            revision: &base,
            values: &input,
            lengths: &lengths(&input),
        };
        assert_eq!(
            recipe.apply(
                SourceBase {
                    revision: &other,
                    ..input_base
                },
                None
            ),
            Err(RecipeError::WrongBaseRevision)
        );
        assert_eq!(
            recipe.apply(input_base, None),
            Err(RecipeError::MissingPreviousBase)
        );
        assert_eq!(
            recipe.apply(
                input_base,
                Some(SourceBase {
                    revision: &other,
                    values: &previous,
                    lengths: &lengths(&previous),
                })
            ),
            Err(RecipeError::WrongPreviousRevision)
        );
        assert!(
            recipe
                .apply(
                    input_base,
                    Some(SourceBase {
                        revision: &prev,
                        values: &previous,
                        lengths: &lengths(&previous),
                    })
                )
                .is_ok()
        );
        let unused = Recipe::from_json(&json!({
            "base_revision": "b",
            "output_revision": "o",
            "previous_output_revision": "p",
            "operations": [],
        }));
        assert_eq!(unused, Err(RecipeError::UnusedPreviousRevision));
    }

    #[test]
    fn empty_operations_reconstruct_an_empty_array_and_round_trip() {
        let recipe = Recipe::from_json(&json!({
            "base_revision": "b",
            "output_revision": "o",
            "operations": [],
        }))
        .expect("valid");
        let base = revision("b");
        let input = shared(&[json!(1)]);
        let output = recipe
            .apply(
                SourceBase {
                    revision: &base,
                    values: &input,
                    lengths: &lengths(&input),
                },
                None,
            )
            .expect("applies");
        assert_eq!(
            output,
            AppliedRecipe {
                values: Vec::new(),
                lengths: Vec::new(),
                bytes: 2,
            }
        );
        let encoded = serde_json::to_value(&recipe).expect("serializes");
        assert_eq!(
            encoded,
            json!({"base_revision": "b", "output_revision": "o", "operations": []})
        );
        let decoded: Recipe = serde_json::from_value(encoded).expect("deserializes");
        assert_eq!(decoded, recipe);
        let mixed = json!({
            "base_revision": "b",
            "output_revision": "o",
            "previous_output_revision": "p",
            "operations": [
                {"op": "keep", "source": "previous", "start": 0, "count": 1},
                {"op": "insert", "values": [{"id": "s"}]},
            ],
        });
        let decoded: Recipe = serde_json::from_value(mixed.clone()).expect("deserializes");
        assert_eq!(serde_json::to_value(&decoded).expect("serializes"), mixed);
    }

    fn keyed<'a>(values: &'a [Arc<Value>]) -> Vec<Keyed<'a, u64>> {
        // Provenance here is the message id; repeated ids nominate several positions.
        values
            .iter()
            .map(|value| Keyed {
                key: value["id"].as_u64().unwrap_or(u64::MAX),
                value,
            })
            .collect()
    }

    fn rendered(recipe: &Recipe) -> Value {
        serde_json::to_value(recipe).expect("serializes")
    }

    #[test]
    fn builder_prefers_previous_then_input_and_inserts_the_rest() {
        // AE2: a large transformed prefix already applied, plus one new unchanged input message.
        let input = shared(&[
            json!({"id": 1}),
            json!({"id": 2}),
            json!({"id": 3}),
            json!({"id": 4}),
        ]);
        let previous = shared(&[json!({"id": 1}), json!({"id": 20, "summary": true})]);
        let output = shared(&[
            json!({"id": 1}),
            json!({"id": 20, "summary": true}),
            json!({"id": 4}),
            json!({"id": 99, "instruction": true}),
        ]);
        let base = revision("base");
        let prev = revision("prev");
        let recipe = build_recipe(
            &keyed(&output),
            (&base, &keyed(&input)),
            Some((&prev, &keyed(&previous))),
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe),
            json!({
                "base_revision": "base",
                "output_revision": "out",
                "previous_output_revision": "prev",
                "operations": [
                    {"op": "keep", "source": "previous", "start": 0, "count": 2},
                    {"op": "keep", "source": "input", "start": 3, "count": 1},
                    {"op": "insert", "values": [{"id": 99, "instruction": true}]},
                ],
            })
        );
        // The recipe reconstructs the output through the applier, sharing every kept handle.
        let AppliedRecipe {
            values: applied, ..
        } = recipe
            .apply(
                SourceBase {
                    revision: &base,
                    values: &input,
                    lengths: &lengths(&input),
                },
                Some(SourceBase {
                    revision: &prev,
                    values: &previous,
                    lengths: &lengths(&previous),
                }),
            )
            .expect("applies");
        let applied: Vec<Value> = applied.iter().map(|value| (**value).clone()).collect();
        let expected: Vec<Value> = output.iter().map(|value| (**value).clone()).collect();
        assert_eq!(applied, expected);
    }

    #[test]
    fn builder_confirms_by_equality_not_by_key() {
        let input = shared(&[json!({"id": 1, "text": "old"})]);
        let output = shared(&[json!({"id": 1, "text": "new"})]);
        let base = revision("base");
        let recipe = build_recipe(
            &keyed(&output),
            (&base, &keyed(&input)),
            None,
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([{"op": "insert", "values": [{"id": 1, "text": "new"}]}])
        );
        assert_eq!(recipe.previous_output_revision, None);
    }

    #[test]
    fn builder_keeps_cursors_monotone_and_uses_each_candidate_at_most_once() {
        // Output repeats message 1 and moves 3 before 2; only forward matches become keeps.
        let input = shared(&[json!({"id": 1}), json!({"id": 2}), json!({"id": 3})]);
        let output = shared(&[
            json!({"id": 1}),
            json!({"id": 3}),
            json!({"id": 2}),
            json!({"id": 1}),
        ]);
        let base = revision("base");
        let recipe = build_recipe(
            &keyed(&output),
            (&base, &keyed(&input)),
            None,
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([
                {"op": "keep", "source": "input", "start": 0, "count": 1},
                {"op": "keep", "source": "input", "start": 2, "count": 1},
                {"op": "insert", "values": [{"id": 2}, {"id": 1}]},
            ])
        );
        // Duplicate provenance: two input entries share a key; each is used at most once, in order.
        let duplicated = shared(&[json!({"id": 7}), json!({"id": 7}), json!({"id": 7})]);
        let recipe = build_recipe(
            &keyed(&duplicated),
            (&base, &keyed(&duplicated[..2])),
            None,
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([
                {"op": "keep", "source": "input", "start": 0, "count": 2},
                {"op": "insert", "values": [{"id": 7}]},
            ])
        );
    }

    #[test]
    fn builder_tests_at_most_max_confirm_probes_candidates_per_nomination() {
        assert_eq!(MAX_CONFIRM_PROBES, 8);
        // One key nominates MAX_CONFIRM_PROBES + 1 input positions; only the last equals the
        // output. The scan stops after MAX_CONFIRM_PROBES candidates and emits a literal.
        let bucket = MAX_CONFIRM_PROBES + 1;
        let input = shared(
            &(0..bucket)
                .map(|n| json!({"id": 7, "n": n}))
                .collect::<Vec<_>>(),
        );
        let output = shared(&[json!({"id": 7, "n": bucket - 1})]);
        let base = revision("base");
        let recipe = build_recipe(
            &keyed(&output),
            (&base, &keyed(&input)),
            None,
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([{"op": "insert", "values": [{"id": 7, "n": bucket - 1}]}])
        );
        // With exactly MAX_CONFIRM_PROBES candidates the last one is still tested and kept.
        let recipe = build_recipe(
            &keyed(&output[..]),
            (&base, &keyed(&input[1..])),
            None,
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([{"op": "keep", "source": "input", "start": 7, "count": 1}])
        );
        // Spent positions do not count against the budget.
        let output = shared(&[json!({"id": 7, "n": 1}), json!({"id": 7, "n": bucket - 1})]);
        let recipe = build_recipe(
            &keyed(&output),
            (&base, &keyed(&input)),
            None,
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([
                {"op": "keep", "source": "input", "start": 1, "count": 1},
                {"op": "keep", "source": "input", "start": bucket - 1, "count": 1},
            ])
        );
    }

    #[test]
    fn builder_shares_confirm_probe_budget_across_sources() {
        let previous = shared(
            &(0..MAX_CONFIRM_PROBES)
                .map(|n| json!({"id": 7, "n": n}))
                .collect::<Vec<_>>(),
        );
        let input = shared(&[json!({"id": 7, "n": 99})]);
        let output = shared(&[json!({"id": 7, "n": 99})]);
        let base = revision("base");
        let prev = revision("prev");
        let recipe = build_recipe(
            &keyed(&output),
            (&base, &keyed(&input)),
            Some((&prev, &keyed(&previous))),
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe)["operations"],
            json!([{"op": "insert", "values": [{"id": 7, "n": 99}]}])
        );
    }

    #[test]
    fn builder_emits_empty_operations_for_empty_output_and_omits_unused_previous() {
        let input = shared(&[json!({"id": 1})]);
        let previous = shared(&[json!({"id": 2})]);
        let base = revision("base");
        let prev = revision("prev");
        let recipe = build_recipe(
            &[],
            (&base, &keyed(&input)),
            Some((&prev, &keyed(&previous))),
            revision("out"),
        );
        assert_eq!(
            rendered(&recipe),
            json!({"base_revision": "base", "output_revision": "out", "operations": []})
        );
    }

    #[test]
    fn canonical_len_matches_compact_serialization() {
        let value = json!({"b": [1, 2.5, "x\ny"], "a": null, "é": true});
        assert_eq!(
            canonical_len(&value).expect("measures"),
            serde_json::to_vec(&value).expect("serializes").len()
        );
    }

    #[test]
    fn size_accumulator_rejects_the_first_byte_over_the_cap() {
        assert_eq!(
            add_entry(MAX_RECONSTRUCTED_BYTES, 1, 4),
            Err(RecipeError::OutputTooLarge {
                bytes: MAX_RECONSTRUCTED_BYTES + 5,
            }),
        );
    }

    #[test]
    fn wide_nesting_check_keeps_pending_work_depth_bounded() {
        let recipe = json!({
            "base_revision": "b",
            "output_revision": "o",
            "operations": [],
            "padding": vec![Value::Null; 10_000],
        });
        assert!(validate_json_nesting(&recipe).expect("valid depth") <= 127);
    }

    #[test]
    fn preserved_value_rejects_depth_before_deserializing_the_full_tree() {
        assert!(PreservedValue::deserialize(NestedArray { remaining: 140 }).is_err());
    }

    #[test]
    fn serde_round_trip_preserves_raw_value_marker_literals() {
        let marker = Value::Object(
            [(
                crate::metered_decode::RAW_VALUE_TOKEN.to_owned(),
                Value::String("[1]".to_owned()),
            )]
            .into_iter()
            .collect(),
        );
        let recipe = Recipe::from_json(&json!({
            "base_revision": "b",
            "output_revision": "o",
            "operations": [{ "op": "insert", "values": [marker] }],
        }))
        .expect("valid recipe");
        let encoded = serde_json::to_value(&recipe).expect("serialize recipe");
        let decoded = serde_json::from_value::<Recipe>(encoded).expect("deserialize recipe");
        assert_eq!(decoded, recipe);
    }

    #[test]
    fn serde_nesting_limit_leaves_room_for_123_literal_containers() {
        let recipe_text = |depth: usize| {
            format!(
                r#"{{"base_revision":"b","output_revision":"o","operations":[{{"op":"insert","values":[{}null{}]}}]}}"#,
                "[".repeat(depth),
                "]".repeat(depth)
            )
        };
        assert!(serde_json::from_str::<Recipe>(&recipe_text(123)).is_ok());
        assert!(serde_json::from_str::<Recipe>(&recipe_text(124)).is_err());

        let recipe_value = |depth: usize| {
            let mut literal = Value::Null;
            for _ in 0..depth {
                literal = Value::Array(vec![literal]);
            }
            json!({
                "base_revision": "b",
                "output_revision": "o",
                "operations": [{ "op": "insert", "values": [literal] }],
            })
        };
        assert!(Recipe::from_json(&recipe_value(123)).is_ok());
        assert!(Recipe::from_json(&recipe_value(124)).is_err());
    }
}
