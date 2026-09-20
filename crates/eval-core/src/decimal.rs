//! Times cross the wire as canonical decimal text because the valid-time domain
//! passes canonical JSON's 2^53 - 1 cap; only the `i64::to_string` form is read back.

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let text = String::deserialize(deserializer)?;
    let value: i64 = text.parse().map_err(serde::de::Error::custom)?;
    if value.to_string() != text {
        return Err(serde::de::Error::custom(format!(
            "{text:?} is not the canonical decimal form"
        )));
    }
    Ok(value)
}
