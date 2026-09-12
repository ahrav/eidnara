//! Resident-byte admission enforced inside the decode.
//!
//! A request body is charged against the host scratch pool as each decoded value increases
//! its estimated resident footprint, and the meter charges the pool in steps as the
//! footprint grows. A body whose footprint exceeds the pool's capacity can never be
//! admitted and is refused as too large; a body that fits but finds the pool short, because
//! other requests hold it, is refused as transient. The decode stops at the first refused
//! charge, so no further value is built; the values built before it are released with the
//! meter, and an admitted body's charges are released when the response has settled.
//!
//! Two costs follow from charging inside the decode rather than before it. A body whose
//! footprint exceeds the capacity holds pool bytes for the values it built until the
//! footprint crosses the capacity, so a doomed decode occupies the pool for the length of
//! its failing parse; the byte cap bounds that parse to one transform body. And serde_json
//! unescapes a string into its scratch buffer before the visitor sees it, so a decode run
//! without [`ResidentMeter::reserve_unescape_scratch`] allocates an escaped string, up to
//! the body's length, before its charge is taken; the handler takes that charge from the
//! bytes first, and serde_json reuses the buffer across strings.

use std::fmt;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use host_runtime::handler::RequestCtx;
use host_runtime::wire::ByteCharge;
use serde::de::{
    DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use serde_json::Value;

use crate::retained_size;

/// The estimate doubles counted node storage for `Vec` and map growth.
const VALUE_NODE_SLACK: usize = 2;

/// The decoded `Value` tree and the typed request coexist during `serde_json::from_value` on
/// the tree-decode lane; the direct decode of an unpaged transform body retains at most as
/// much, so one estimate covers both lanes.
const RETAINED_NODE_COPIES: usize = 2;

const VALUE_NODE_CHARGE_BYTES: usize = std::mem::size_of::<Value>() * VALUE_NODE_SLACK;

/// `native_messages` stores `Arc<Value>` handles; their slack charge plus the `Arc` allocation
/// must not exceed the typed-request node charge.
const _: () = assert!(
    std::mem::size_of::<Arc<Value>>() * VALUE_NODE_SLACK
        + retained_size::ARC_ALLOCATION_OVERHEAD_BYTES
        + std::mem::size_of::<Value>()
        <= VALUE_NODE_CHARGE_BYTES
);

/// The fixed headroom covers allocations that do not scale with the body.
const VALUE_ENVELOPE_BYTES: usize = 4096;

/// Each string byte is charged this many times: the decoded `Value`, plus the `original`
/// JSON that `WireMessage` retains for lossless pass-through, plus the `original` that each
/// `WireBlock` retains, all hold their own copy of a block's text at the same time.
const RETAINED_STRING_COPIES: usize = 3;

/// The estimate doubles the longest escaped string for the unescape buffer's growth.
const UNESCAPE_SCRATCH_SLACK: usize = 2;

/// The charge for one visited value; object keys are values too.
const NODE_BYTES: usize = VALUE_NODE_CHARGE_BYTES * RETAINED_NODE_COPIES;

const CHARGE_STEP_BYTES: usize = 1024 * 1024;

/// The scratch pool a request's decode is charged against. The meter is held across the
/// handler's awaits, so a reserve is shared between threads.
pub trait ResidentReserve: Sync {
    /// Holds `bytes` of the pool without waiting; `None` when they are not free.
    fn try_reserve(&self, bytes: usize) -> Option<ByteCharge>;
    /// The most one request can ever hold.
    fn capacity(&self) -> usize;
}

impl ResidentReserve for RequestCtx {
    fn try_reserve(&self, bytes: usize) -> Option<ByteCharge> {
        self.try_reserve_resident(bytes)
    }

    fn capacity(&self) -> usize {
        self.resident_capacity()
    }
}

/// A reserve that grants everything and holds nothing, for counting a footprint against a
/// stated capacity.
struct CountOnly {
    capacity: usize,
}

impl ResidentReserve for CountOnly {
    fn try_reserve(&self, _bytes: usize) -> Option<ByteCharge> {
        Some(ByteCharge::none())
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Why the meter refused a charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The footprint exceeds the pool's capacity; no release can admit the body.
    Permanent,
    /// The footprint fits the pool, but the bytes were held by other requests.
    Transient,
}

/// A transient refusal as the meter saw it, recorded for the pool-shortfall witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortfallMarker {
    /// The footprint the decode had reached when the charge was refused.
    pub needed: usize,
    /// The bytes the decode already held.
    pub charged: usize,
    /// The pool's capacity; `needed` is at most this, or the refusal would be permanent.
    pub capacity: usize,
}

/// The transient refusals seen since the process started, the campaign's shortfall count.
#[cfg(any(test, feature = "test-support"))]
static SHORTFALLS: AtomicUsize = AtomicUsize::new(0);

#[cfg(any(test, feature = "test-support"))]
pub fn shortfall_count() -> usize {
    SHORTFALLS.load(Ordering::Relaxed)
}

/// Charges one request's decode against a reserve as the footprint grows. The counters are
/// atomic only so the meter can live across the handler's awaits; one decode drives it at a
/// time.
pub struct ResidentMeter<'r> {
    reserve: &'r dyn ResidentReserve,
    needed: AtomicUsize,
    charged: AtomicUsize,
    charges: Mutex<Vec<ByteCharge>>,
    refusal: AtomicU8,
    shortfall: Mutex<Option<ShortfallMarker>>,
    longest_escaped: AtomicUsize,
}

const NO_REFUSAL: u8 = 0;
const PERMANENT: u8 = 1;
const TRANSIENT: u8 = 2;

impl<'r> ResidentMeter<'r> {
    pub fn new(reserve: &'r dyn ResidentReserve) -> Self {
        Self {
            reserve,
            needed: AtomicUsize::new(0),
            charged: AtomicUsize::new(0),
            charges: Mutex::new(Vec::new()),
            refusal: AtomicU8::new(NO_REFUSAL),
            shortfall: Mutex::new(None),
            longest_escaped: AtomicUsize::new(0),
        }
    }

    /// The footprint the decode has reached so far.
    pub fn needed(&self) -> usize {
        self.needed.load(Ordering::Relaxed)
    }

    /// The bytes held from the reserve.
    pub fn charged(&self) -> usize {
        self.charged.load(Ordering::Relaxed)
    }

    /// The most the reserve lets one request hold.
    pub fn capacity(&self) -> usize {
        self.reserve.capacity()
    }

    pub fn refusal(&self) -> Option<Refusal> {
        match self.refusal.load(Ordering::Relaxed) {
            PERMANENT => Some(Refusal::Permanent),
            TRANSIENT => Some(Refusal::Transient),
            _ => None,
        }
    }

    /// The transient refusal this meter recorded, if any.
    pub fn shortfall(&self) -> Option<ShortfallMarker> {
        *self
            .shortfall
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Starts the footprint over for a second decode of the same body while keeping the
    /// bytes already held, which the second decode uses before charging more.
    pub fn restart(&self) {
        self.needed.store(0, Ordering::Relaxed);
        self.refusal.store(NO_REFUSAL, Ordering::Relaxed);
        self.longest_escaped.store(0, Ordering::Relaxed);
    }

    /// Gives every held byte back. A refused decode releases at once rather than at the end
    /// of the request, so the pool is not held while the refusal is classified and sent.
    pub fn release(&self) {
        self.charges
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.charged.store(0, Ordering::Relaxed);
    }

    fn node(&self) -> Result<(), Refusal> {
        self.need(NODE_BYTES)
    }

    fn text(&self, len: usize) -> Result<(), Refusal> {
        self.need(len.saturating_mul(RETAINED_STRING_COPIES))
    }

    fn escaped_text(&self, len: usize) -> Result<(), Refusal> {
        let longest = self.longest_escaped.load(Ordering::Relaxed);
        if len <= longest {
            return Ok(());
        }
        self.longest_escaped.store(len, Ordering::Relaxed);
        self.need((len - longest).saturating_mul(UNESCAPE_SCRATCH_SLACK))
    }

    /// Charges the unescape buffer for the longest escaped string in `body` before any
    /// decode runs. serde_json grows that buffer to a string's length before the visitor
    /// sees the string, so this is the one body-proportional allocation a decode makes
    /// ahead of its charge; taking the charge from the bytes puts it under the pool first.
    /// A visited string never exceeds the raw length counted here, so the decodes charge
    /// nothing more for the buffer.
    pub fn reserve_unescape_scratch(&self, body: &[u8]) -> Result<(), Refusal> {
        self.escaped_text(longest_escaped_string(body))
    }

    fn need(&self, bytes: usize) -> Result<(), Refusal> {
        if let Some(refusal) = self.refusal() {
            return Err(refusal);
        }
        let base = match self.needed() {
            0 => VALUE_ENVELOPE_BYTES,
            needed => needed,
        };
        let Some(needed) = base.checked_add(bytes) else {
            return Err(self.refuse(Refusal::Permanent));
        };
        self.needed.store(needed, Ordering::Relaxed);
        let charged = self.charged();
        if needed <= charged {
            return Ok(());
        }
        let capacity = self.reserve.capacity();
        if needed > capacity {
            return Err(self.refuse(Refusal::Permanent));
        }
        let shortfall = needed - charged;
        let step = shortfall
            .max(needed.min(CHARGE_STEP_BYTES))
            .min(capacity - charged);
        // The exact shortfall is the admission decision; a larger batch only saves
        // acquisitions. Halve a refused batch toward `shortfall` rather than retrying it once
        // per value.
        let mut attempt = step;
        let charge = loop {
            if let Some(charge) = self.reserve.try_reserve(attempt) {
                break Some((charge, attempt));
            }
            if attempt == shortfall {
                break None;
            }
            attempt = (attempt / 2).max(shortfall);
        };
        match charge {
            Some((charge, bytes)) => {
                self.charges
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(charge);
                self.charged.store(charged + bytes, Ordering::Relaxed);
                Ok(())
            }
            None => {
                #[cfg(any(test, feature = "test-support"))]
                SHORTFALLS.fetch_add(1, Ordering::Relaxed);
                *self
                    .shortfall
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ShortfallMarker {
                    needed,
                    charged,
                    capacity,
                });
                Err(self.refuse(Refusal::Transient))
            }
        }
    }

    fn refuse(&self, refusal: Refusal) -> Refusal {
        let code = match refusal {
            Refusal::Permanent => PERMANENT,
            Refusal::Transient => TRANSIENT,
        };
        self.refusal.store(code, Ordering::Relaxed);
        refusal
    }
}

/// How a metered decode ended without a value.
#[derive(Debug)]
pub enum DecodeFailure {
    /// The reserve refused a charge; the body was not decoded past that point.
    Refused(Refusal),
    /// The body did not decode as `T`.
    Invalid(serde_json::Error),
}

/// Decodes `body` as `T`, charging `meter` as values are visited. Trailing bytes are an
/// error, as they are for `serde_json::from_slice`.
pub fn decode_metered<'de, T: serde::Deserialize<'de>>(
    body: &'de [u8],
    meter: &ResidentMeter<'_>,
) -> Result<T, DecodeFailure> {
    let mut inner = serde_json::Deserializer::from_slice(body);
    let decoded = T::deserialize(Metered {
        inner: &mut inner,
        meter,
    });
    // A refused charge outranks whatever the decode made of the error, including a decode
    // that recovered from it; the body was never admitted.
    if let Some(refusal) = meter.refusal() {
        meter.release();
        return Err(DecodeFailure::Refused(refusal));
    }
    let value = decoded.map_err(DecodeFailure::Invalid)?;
    match inner.end() {
        Ok(()) => Ok(value),
        Err(error) => Err(DecodeFailure::Invalid(error)),
    }
}

/// The footprint `body` reaches when decoded, counted without charging any pool. A body that
/// stops decoding partway yields the footprint of the part that decoded.
pub fn footprint_of(body: &[u8]) -> usize {
    let reserve = CountOnly {
        capacity: usize::MAX,
    };
    let meter = ResidentMeter::new(&reserve);
    let _ = decode_metered::<SkippedValue>(body, &meter);
    meter.needed()
}

/// The raw length of the longest string in `body` that holds an escape, from its bytes
/// alone; zero when no string does. Raw length is at least the decoded length.
fn longest_escaped_string(body: &[u8]) -> usize {
    if !body.contains(&b'\\') {
        return 0;
    }
    let mut longest = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut current = 0;
    let mut has_escape = false;
    for &byte in body {
        if !in_string {
            if byte == b'"' {
                in_string = true;
                current = 0;
                has_escape = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            current += 1;
        } else if byte == b'\\' {
            escaped = true;
            has_escape = true;
            current += 1;
        } else if byte == b'"' {
            in_string = false;
            if has_escape {
                longest = longest.max(current);
            }
        } else {
            current += 1;
        }
    }
    if in_string && has_escape {
        longest = longest.max(current);
    }
    longest
}

/// Whether the footprint `body` reaches when decoded exceeds `capacity`, counted without
/// charging any pool. The count stops at the value that crosses `capacity`, so a body far
/// above it is not parsed to its end.
#[cfg(test)]
pub(crate) fn footprint_exceeds(body: &[u8], capacity: usize) -> bool {
    let reserve = CountOnly { capacity };
    let meter = ResidentMeter::new(&reserve);
    matches!(
        decode_metered::<SkippedValue>(body, &meter),
        Err(DecodeFailure::Refused(Refusal::Permanent))
    )
}

/// Whether `body`'s bytes alone prove its footprint exceeds `capacity`. Such a body is
/// refused before it is decoded, so it never holds pool bytes while it is parsed.
pub fn footprint_floor_exceeds(body: &[u8], capacity: usize) -> bool {
    // Every value starts at a byte of its own, so a body this short cannot reach the
    // capacity and is not scanned.
    let most = body
        .len()
        .saturating_mul(NODE_BYTES)
        .saturating_add(VALUE_ENVELOPE_BYTES);
    most > capacity && footprint_floor(body) > capacity
}

/// Returns a lower bound on the footprint `body` reaches when decoded, from its bytes alone:
/// one node per value start outside a string, with string bytes left out. A value starts at
/// an opening quote, `[`, `{`, the first byte of a number, or the first letter of a literal,
/// so for well-formed JSON the node count equals the one the meter visits and the bound is
/// at most the footprint. Bytes past a parse error can only raise the bound, so a malformed
/// body the bound refuses is one the decode could not have admitted whole.
pub(crate) fn footprint_floor(body: &[u8]) -> usize {
    let mut values: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_number = false;
    for &byte in body {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => {
                values += 1;
                in_string = true;
                in_number = false;
            }
            b'[' | b'{' | b't' | b'f' | b'n' => {
                values += 1;
                in_number = false;
            }
            b'-' | b'0'..=b'9' => {
                if !in_number {
                    values += 1;
                    in_number = true;
                }
            }
            // A sign or exponent inside a number, or the `e` of a literal; neither starts a value.
            b'.' | b'e' | b'E' | b'+' => {}
            _ => in_number = false,
        }
    }
    if values == 0 {
        return 0;
    }
    values
        .saturating_mul(NODE_BYTES)
        .saturating_add(VALUE_ENVELOPE_BYTES)
}

/// Skips one value through `deserialize_any`, so every check the deserializer applies to a
/// `Value` parse applies to it: the nesting limit, number range, string escapes, and UTF-8.
/// `IgnoredAny` is not used because serde_json skips it without those checks and without
/// visiting its contents, so a body the tree decode refuses would pass, and through the
/// meter an ignored subtree would go uncounted.
/// An object whose first key is [`RAW_VALUE_TOKEN`] is read as a `Value` parse reads it: its
/// value must be a string and no key may follow it, so the count stops where that parse stops.
#[derive(Debug)]
pub(crate) struct SkippedValue;

/// The object key serde_json's `raw_value` feature reserves. A `Value` parse reads an
/// object whose first key is this token as one boxed JSON document: its value must be a
/// string holding a document and no key may follow it. A `Value` re-read through
/// `from_value` sees the object's keys sorted, where the token sorts before any letter, so
/// a retained `Value` holding the token at any position meets the rule on the tree lane.
/// serde_json keeps the token private; the `raw_value_token_matches_serde_json` test
/// detects a change to it.
pub(crate) const RAW_VALUE_TOKEN: &str = "$serde_json::private::RawValue";

/// A key of a skipped object, and whether it is [`RAW_VALUE_TOKEN`].
struct SkippedKey {
    raw_value_token: bool,
}

impl<'de> serde::Deserialize<'de> for SkippedKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;

        impl Visitor<'_> for KeyVisitor {
            type Value = SkippedKey;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object key")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(SkippedKey {
                    raw_value_token: value == RAW_VALUE_TOKEN,
                })
            }
        }

        deserializer.deserialize_str(KeyVisitor)
    }
}

/// The value under a leading [`RAW_VALUE_TOKEN`] key; a `Value` parse accepts only a string there.
struct RawDocument;

impl<'de> serde::Deserialize<'de> for RawDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DocumentVisitor;

        impl Visitor<'_> for DocumentVisitor {
            type Value = RawDocument;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("raw value")
            }

            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(RawDocument)
            }
        }

        deserializer.deserialize_str(DocumentVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for SkippedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SkipVisitor)
    }
}

/// The visitor behind [`SkippedValue`]; wrapped in a [`MeteredVisitor`], it counts what it
/// skips.
struct SkipVisitor;

impl<'de> Visitor<'de> for SkipVisitor {
    type Value = SkippedValue;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<SkippedValue, E> {
        Ok(SkippedValue)
    }

    fn visit_i64<E>(self, _: i64) -> Result<SkippedValue, E> {
        Ok(SkippedValue)
    }

    fn visit_u64<E>(self, _: u64) -> Result<SkippedValue, E> {
        Ok(SkippedValue)
    }

    fn visit_f64<E>(self, _: f64) -> Result<SkippedValue, E> {
        Ok(SkippedValue)
    }

    fn visit_str<E>(self, _: &str) -> Result<SkippedValue, E> {
        Ok(SkippedValue)
    }

    fn visit_unit<E>(self) -> Result<SkippedValue, E> {
        Ok(SkippedValue)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SkippedValue, A::Error> {
        while seq.next_element::<SkippedValue>()?.is_some() {}
        Ok(SkippedValue)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<SkippedValue, A::Error> {
        let mut first = true;
        while let Some(key) = map.next_key::<SkippedKey>()? {
            if first && key.raw_value_token {
                // Returning here leaves the deserializer to refuse a following key, as it
                // does after a `Value` parse reads the document.
                map.next_value::<RawDocument>()?;
                return Ok(SkippedValue);
            }
            first = false;
            map.next_value::<SkippedValue>()?;
        }
        Ok(SkippedValue)
    }
}

/// The message a refused charge surfaces as; callers classify the refusal from the meter,
/// not from this text.
const REFUSED_CHARGE: &str = "resident budget refused the decode";

fn refused<E: serde::de::Error>() -> E {
    E::custom(REFUSED_CHARGE)
}

/// A deserializer that forwards every call to `inner` and counts every value the visitor
/// receives against `meter`.
struct Metered<'m, D> {
    inner: D,
    meter: &'m ResidentMeter<'m>,
}

macro_rules! forward_metered {
    ($($method:ident),* $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
                self.inner.$method(MeteredVisitor { visitor, meter: self.meter })
            }
        )*
    };
}

impl<'de, 'm, D: Deserializer<'de>> Deserializer<'de> for Metered<'m, D> {
    type Error = D::Error;

    forward_metered!(
        deserialize_any,
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_i128,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_u128,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_option,
        deserialize_unit,
        deserialize_seq,
        deserialize_map,
        deserialize_identifier,
    );

    /// An ignored value is skipped through `deserialize_any` so its contents are counted and
    /// checked like any other value; serde_json's own skip would visit nothing and charge
    /// one node for a subtree of any size, and the two lanes would then admit different
    /// bodies.
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_any(MeteredVisitor {
            visitor: SkipVisitor,
            meter: self.meter,
        })?;
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_unit_struct(
            name,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_newtype_struct(
            name,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_tuple(
            len,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_tuple_struct(
            name,
            len,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_struct(
            name,
            fields,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_enum(
            name,
            variants,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

/// Counts each value it receives, then hands it to the wrapped visitor; nested access is
/// wrapped so nested values are counted too.
struct MeteredVisitor<'m, V> {
    visitor: V,
    meter: &'m ResidentMeter<'m>,
}

macro_rules! visit_scalar {
    ($($method:ident: $ty:ty),* $(,)?) => {
        $(
            fn $method<E: serde::de::Error>(self, value: $ty) -> Result<Self::Value, E> {
                self.meter.node().map_err(|_| refused::<E>())?;
                self.visitor.$method(value)
            }
        )*
    };
}

impl<'de, 'm, V: Visitor<'de>> Visitor<'de> for MeteredVisitor<'m, V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        self.visitor.expecting(formatter)
    }

    visit_scalar!(
        visit_bool: bool,
        visit_i8: i8,
        visit_i16: i16,
        visit_i32: i32,
        visit_i64: i64,
        visit_i128: i128,
        visit_u8: u8,
        visit_u16: u16,
        visit_u32: u32,
        visit_u64: u64,
        visit_u128: u128,
        visit_f32: f32,
        visit_f64: f64,
        visit_char: char,
    );

    /// serde_json takes this path, not `visit_borrowed_str`, for a string it unescaped.
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.count_text::<E>(value.len())?;
        self.meter
            .escaped_text(value.len())
            .map_err(|_| refused::<E>())?;
        self.visitor.visit_str(value)
    }

    fn visit_borrowed_str<E: serde::de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        self.count_text::<E>(value.len())?;
        self.visitor.visit_borrowed_str(value)
    }

    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.count_text::<E>(value.len())?;
        self.visitor.visit_string(value)
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        self.count_text::<E>(value.len())?;
        self.visitor.visit_bytes(value)
    }

    fn visit_borrowed_bytes<E: serde::de::Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
        self.count_text::<E>(value.len())?;
        self.visitor.visit_borrowed_bytes(value)
    }

    fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        self.count_text::<E>(value.len())?;
        self.visitor.visit_byte_buf(value)
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        self.meter.node().map_err(|_| refused::<E>())?;
        self.visitor.visit_none()
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        self.meter.node().map_err(|_| refused::<E>())?;
        self.visitor.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        self.visitor.visit_some(Metered {
            inner: deserializer,
            meter: self.meter,
        })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.visitor.visit_newtype_struct(Metered {
            inner: deserializer,
            meter: self.meter,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        self.meter.node().map_err(|_| refused::<A::Error>())?;
        self.visitor.visit_seq(MeteredSeq {
            seq,
            meter: self.meter,
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        self.meter.node().map_err(|_| refused::<A::Error>())?;
        self.visitor.visit_map(MeteredMap {
            map,
            meter: self.meter,
        })
    }

    /// The variant identifier is counted through the seed, so it stands in for the enum's
    /// own node.
    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
        self.visitor.visit_enum(MeteredEnum {
            data,
            meter: self.meter,
        })
    }
}

impl<'de, 'm, V: Visitor<'de>> MeteredVisitor<'m, V> {
    fn count_text<E: serde::de::Error>(&self, len: usize) -> Result<(), E> {
        self.meter.node().map_err(|_| refused::<E>())?;
        self.meter.text(len).map_err(|_| refused::<E>())
    }
}

/// Wraps a seed so the value it decodes is read through a metered deserializer.
struct MeteredSeed<'m, S> {
    seed: S,
    meter: &'m ResidentMeter<'m>,
}

impl<'de, 'm, S: DeserializeSeed<'de>> DeserializeSeed<'de> for MeteredSeed<'m, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        self.seed.deserialize(Metered {
            inner: deserializer,
            meter: self.meter,
        })
    }
}

struct MeteredSeq<'m, A> {
    seq: A,
    meter: &'m ResidentMeter<'m>,
}

impl<'de, 'm, A: SeqAccess<'de>> SeqAccess<'de> for MeteredSeq<'m, A> {
    type Error = A::Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        self.seq.next_element_seed(MeteredSeed {
            seed,
            meter: self.meter,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.seq.size_hint()
    }
}

struct MeteredMap<'m, A> {
    map: A,
    meter: &'m ResidentMeter<'m>,
}

impl<'de, 'm, A: MapAccess<'de>> MapAccess<'de> for MeteredMap<'m, A> {
    type Error = A::Error;

    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        self.map.next_key_seed(MeteredSeed {
            seed,
            meter: self.meter,
        })
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        self.map.next_value_seed(MeteredSeed {
            seed,
            meter: self.meter,
        })
    }

    fn size_hint(&self) -> Option<usize> {
        self.map.size_hint()
    }
}

struct MeteredEnum<'m, A> {
    data: A,
    meter: &'m ResidentMeter<'m>,
}

impl<'de, 'm, A: EnumAccess<'de>> EnumAccess<'de> for MeteredEnum<'m, A> {
    type Error = A::Error;
    type Variant = MeteredVariant<'m, A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), A::Error> {
        let (value, variant) = self.data.variant_seed(MeteredSeed {
            seed,
            meter: self.meter,
        })?;
        Ok((
            value,
            MeteredVariant {
                variant,
                meter: self.meter,
            },
        ))
    }
}

struct MeteredVariant<'m, A> {
    variant: A,
    meter: &'m ResidentMeter<'m>,
}

impl<'de, 'm, A: VariantAccess<'de>> VariantAccess<'de> for MeteredVariant<'m, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.variant.unit_variant()
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, A::Error> {
        self.variant.newtype_variant_seed(MeteredSeed {
            seed,
            meter: self.meter,
        })
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.variant.tuple_variant(
            len,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.variant.struct_variant(
            fields,
            MeteredVisitor {
                visitor,
                meter: self.meter,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dense(values: usize) -> Vec<u8> {
        let mut body = Vec::from(b"[0".as_slice());
        for _ in 1..values {
            body.extend_from_slice(b",0");
        }
        body.push(b']');
        body
    }

    /// Classifying a refusal must not parse a body to its end: the count stops at the value
    /// that crosses the capacity.
    #[test]
    fn a_capacity_bound_count_stops_at_the_value_that_crosses_it() {
        let body = dense(200_000);
        let footprint = footprint_of(&body);
        let capacity = footprint / 4;

        let reserve = CountOnly { capacity };
        let meter = ResidentMeter::new(&reserve);
        let failure = decode_metered::<SkippedValue>(&body, &meter).unwrap_err();
        assert!(matches!(
            failure,
            DecodeFailure::Refused(Refusal::Permanent)
        ));
        assert!(meter.needed() > capacity);
        assert!(
            meter.needed() <= capacity + NODE_BYTES,
            "the count reached {} against a capacity of {capacity}",
            meter.needed()
        );

        assert!(footprint_exceeds(&body, footprint - 1));
        assert!(!footprint_exceeds(&body, footprint));
        assert!(
            !footprint_exceeds(b"", 0),
            "an undecodable body reaches nothing"
        );
    }

    /// The count follows the `Value` parse through the raw-value token: a leading token reads
    /// one string and ends the object, a later token is an ordinary key, and an escaped
    /// string carries its unescape charge on both paths.
    #[test]
    fn footprint_of_counts_what_the_value_decode_charges() {
        let unbounded = CountOnly {
            capacity: usize::MAX,
        };
        for body in [
            format!(r#"{{"x":{{"{RAW_VALUE_TOKEN}":1}}}}"#),
            format!(r#"{{"x":{{"{RAW_VALUE_TOKEN}":"[1]"}}}}"#),
            format!(r#"{{"x":{{"{RAW_VALUE_TOKEN}":"[1]","y":2}}}}"#),
            format!(r#"{{"x":{{"a":1,"{RAW_VALUE_TOKEN}":1}}}}"#),
            format!(r#"{{"x":[{{"{RAW_VALUE_TOKEN}":1}}]}}"#),
            format!(r#"{{"{RAW_VALUE_TOKEN}":"\"a\\nb\""}}"#),
            r#"{"a":"x\ny","b":"longer\tescaped","c":"z"}"#.to_string(),
        ] {
            let meter = ResidentMeter::new(&unbounded);
            let _ = decode_metered::<Value>(body.as_bytes(), &meter);
            assert_eq!(footprint_of(body.as_bytes()), meter.needed(), "{body}");
        }
    }

    /// The byte scan counts the values the meter visits; the floor excludes string bytes,
    /// which the meter retains.
    #[test]
    fn footprint_floor_counts_the_values_the_meter_visits() {
        let deep = format!("{}1{}", "[".repeat(127), "]".repeat(127));
        for body in [
            "[0,0,0]",
            "[[],{},[[]]]",
            "[-1e-5,1.5E+3,-0,true,false,null,12]",
            "{}",
            "7",
            deep.as_str(),
        ] {
            assert_eq!(
                footprint_floor(body.as_bytes()),
                footprint_of(body.as_bytes()),
                "{body}"
            );
        }
        let strings = br#"{"a":"x,y:z","b":["\"q\"",""]}"#;
        let decoded_text = "a".len() + "x,y:z".len() + "b".len() + "\"q\"".len();
        let longest_escaped = "\"q\"".len();
        assert_eq!(
            footprint_floor(strings)
                + RETAINED_STRING_COPIES * decoded_text
                + UNESCAPE_SCRATCH_SLACK * longest_escaped,
            footprint_of(strings)
        );
        assert_eq!(footprint_floor(b""), 0);
        assert_eq!(footprint_floor(b"   "), 0);

        // A dense body far above the capacity is refused from its bytes; one that fits is not.
        let body = dense(200_000);
        let footprint = footprint_of(&body);
        assert!(footprint_floor_exceeds(&body, footprint - 1));
        assert!(!footprint_floor_exceeds(&body, footprint));
        // A body too short to reach the capacity is not scanned; the answer is the same.
        assert!(!footprint_floor_exceeds(b"[0,0,0]", 1 << 20));
    }
}
