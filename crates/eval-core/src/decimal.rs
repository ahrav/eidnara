//! Serde `with` module for integers carried as decimal text. Deserialization
//! accepts text only when parsing it and formatting the value with `Display`
//! reproduces it exactly, so one value has one encoding.

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<T: Display, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: FromStr + Display,
    T::Err: Display,
    D: Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    let value: T = text.parse().map_err(serde::de::Error::custom)?;
    if value.to_string() != text {
        return Err(serde::de::Error::custom(format!(
            "{text:?} is not the canonical decimal form"
        )));
    }
    Ok(value)
}
