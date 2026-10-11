//! Canonical served JSON uses serde's encoding with recursively sorted object fields.
//!
//! The encoder writes serde_json's compact spelling and records byte spans in one
//! serialization. Reordering copies those spans whole, so number and string forms stay as
//! serde wrote them.

#[cfg(test)]
use std::cell::Cell;
use std::fmt::Write as _;
use std::io;
use std::ops::Range;
use std::sync::Arc;

use serde::Serialize;
use serde::ser::{Error as _, Impossible};
use serde_json::ser::{CompactFormatter, Formatter};

#[derive(Default)]
struct ObjectSpans {
    bytes: Range<usize>,
    fields: Vec<(Range<usize>, usize)>,
}

/// The objects of one serialization. Entries past `len` belong to an earlier document and
/// keep their field capacity for the next one.
#[derive(Default)]
struct ObjectPool {
    items: Vec<ObjectSpans>,
    len: usize,
}

impl ObjectPool {
    fn begin(&mut self, start: usize) -> usize {
        let index = self.len;
        match self.items.get_mut(index) {
            Some(object) => {
                object.bytes = start..0;
                object.fields.clear();
            }
            None => self.items.push(ObjectSpans {
                bytes: start..0,
                fields: Vec::new(),
            }),
        }
        self.len += 1;
        index
    }

    fn live(&self) -> &[ObjectSpans] {
        &self.items[..self.len]
    }

    fn live_mut(&mut self) -> &mut [ObjectSpans] {
        &mut self.items[..self.len]
    }
}

/// Writes serde_json's compact spelling of a value into `text` and records each object's
/// field spans as it goes, so the canonical reorder can move whole spans afterwards.
#[derive(Default)]
struct Encoder {
    text: String,
    objects: ObjectPool,
    stack: Vec<usize>,
    /// Open containers at the current write position.
    depth: usize,
    /// The deepest container nesting the document reaches; the root container is depth 1.
    max_depth: usize,
    /// serde_json's float spelling, written before it is copied into `text`.
    number: Vec<u8>,
}

impl Encoder {
    fn reset(&mut self) {
        self.text.clear();
        self.objects.len = 0;
        self.stack.clear();
        self.depth = 0;
        self.max_depth = 0;
    }

    fn enter_container(&mut self) {
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
    }

    fn begin_array(&mut self) {
        self.enter_container();
        self.text.push('[');
    }

    fn end_array(&mut self) {
        self.depth -= 1;
        self.text.push(']');
    }

    fn begin_array_value(&mut self, first: bool) {
        if !first {
            self.text.push(',');
        }
    }

    fn begin_object(&mut self) {
        self.enter_container();
        let index = self.objects.begin(self.text.len());
        self.stack.push(index);
        self.text.push('{');
    }

    fn end_object(&mut self) {
        self.text.push('}');
        self.depth -= 1;
        let index = self.stack.pop().expect("serde closes an open object");
        self.objects.items[index].bytes.end = self.text.len();
    }

    fn begin_object_key(&mut self, first: bool) {
        if !first {
            self.text.push(',');
        }
        let index = *self.stack.last().expect("serde keys belong to an object");
        let start = self.text.len();
        self.objects.items[index].fields.push((start..0, 0));
    }

    fn end_object_key(&mut self) {
        let index = *self.stack.last().expect("serde keys belong to an object");
        let end = self.text.len();
        self.objects.items[index]
            .fields
            .last_mut()
            .expect("key began")
            .1 = end;
    }

    fn begin_object_value(&mut self) {
        self.text.push(':');
    }

    fn end_object_value(&mut self) {
        let index = *self.stack.last().expect("serde values belong to an object");
        let end = self.text.len();
        let (field, _) = self.objects.items[index]
            .fields
            .last_mut()
            .expect("key began");
        field.end = end;
    }

    fn string(&mut self, value: &str) {
        push_escaped(&mut self.text, value);
    }

    fn display(&mut self, value: impl std::fmt::Display) {
        write!(self.text, "{value}").expect("writing to a String cannot fail");
    }

    /// Writes a finite float with serde_json's own formatter, so the digits match its output.
    fn finite_float(
        &mut self,
        write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    ) -> serde_json::Result<()> {
        self.number.clear();
        write(&mut self.number).map_err(serde_json::Error::io)?;
        let digits = std::str::from_utf8(&self.number).map_err(serde_json::Error::custom)?;
        self.text.push_str(digits);
        Ok(())
    }

    /// Opens the `{"variant":` wrapper serde_json writes around a variant's content.
    fn begin_variant(&mut self, variant: &str) {
        self.begin_object();
        self.begin_object_key(true);
        self.string(variant);
        self.end_object_key();
        self.begin_object_value();
    }

    /// Closes the wrapper [`Encoder::begin_variant`] opened.
    fn end_variant(&mut self) {
        self.end_object_value();
        self.end_object();
    }
}

/// The byte each character escapes to after `\`, `u` for `\u00XX`, or 0 when serde_json
/// writes it unchanged.
static ESCAPE: [u8; 256] = {
    let mut table = [0u8; 256];
    let mut byte = 0;
    while byte < 0x20 {
        table[byte] = b'u';
        byte += 1;
    }
    table[0x08] = b'b';
    table[0x09] = b't';
    table[0x0a] = b'n';
    table[0x0c] = b'f';
    table[0x0d] = b'r';
    table[b'"' as usize] = b'"';
    table[b'\\' as usize] = b'\\';
    table
};

/// Flags the bytes of `word` below 0x20, equal to `"`, or equal to `\`. Every such byte sets
/// its high bit; a byte above a flagged one can also be set by the borrow, so callers check
/// each flagged byte against [`ESCAPE`].
fn escape_flags(word: u64) -> u64 {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let control = word.wrapping_sub(ONES * 0x20) & !word & HIGH;
    let quote = word ^ (ONES * u64::from(b'"'));
    let quote = quote.wrapping_sub(ONES) & !quote & HIGH;
    let backslash = word ^ (ONES * u64::from(b'\\'));
    let backslash = backslash.wrapping_sub(ONES) & !backslash & HIGH;
    control | quote | backslash
}

fn push_escape(out: &mut String, byte: u8, escape: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('\\');
    out.push(char::from(escape));
    if escape == b'u' {
        out.push_str("00");
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}

/// Appends `value` as a JSON string with serde_json's escapes. Runs between escapes are
/// copied whole; eight bytes are checked at a time while they hold nothing to escape.
fn push_escaped(out: &mut String, value: &str) {
    let bytes = value.as_bytes();
    out.reserve(bytes.len() + 2);
    out.push('"');
    let mut start = 0;
    let mut index = 0;
    while let Some(chunk) = bytes.get(index..index + 8) {
        let flags = escape_flags(u64::from_le_bytes(
            chunk.try_into().expect("the chunk holds eight bytes"),
        ));
        if flags == 0 {
            index += 8;
            continue;
        }
        index += (flags.trailing_zeros() / 8) as usize;
        let escape = ESCAPE[usize::from(bytes[index])];
        if escape != 0 {
            out.push_str(&value[start..index]);
            push_escape(out, bytes[index], escape);
            start = index + 1;
        }
        index += 1;
    }
    while index < bytes.len() {
        let escape = ESCAPE[usize::from(bytes[index])];
        if escape != 0 {
            out.push_str(&value[start..index]);
            push_escape(out, bytes[index], escape);
            start = index + 1;
        }
        index += 1;
    }
    out.push_str(&value[start..]);
    out.push('"');
}

/// serde_json writes these struct names' single field verbatim.
const RAW_VALUE_TOKEN: &str = "$serde_json::private::RawValue";
const NUMBER_TOKEN: &str = "$serde_json::private::Number";

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Empty,
    First,
    Rest,
}

/// An open array or object, or a struct whose one field serde_json writes verbatim.
enum Compound<'a> {
    Container {
        encoder: &'a mut Encoder,
        state: State,
    },
    Verbatim {
        encoder: &'a mut Encoder,
    },
}

impl<'a> serde::Serializer for &'a mut Encoder {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    fn serialize_bool(self, value: bool) -> serde_json::Result<()> {
        self.text.push_str(if value { "true" } else { "false" });
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_i16(self, value: i16) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_i32(self, value: i32) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_i64(self, value: i64) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_i128(self, value: i128) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_u16(self, value: u16) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_u32(self, value: u32) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_u128(self, value: u128) -> serde_json::Result<()> {
        self.display(value);
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> serde_json::Result<()> {
        if !value.is_finite() {
            return self.serialize_unit();
        }
        self.finite_float(|out| CompactFormatter.write_f32(out, value))
    }

    fn serialize_f64(self, value: f64) -> serde_json::Result<()> {
        if !value.is_finite() {
            return self.serialize_unit();
        }
        self.finite_float(|out| CompactFormatter.write_f64(out, value))
    }

    fn serialize_char(self, value: char) -> serde_json::Result<()> {
        self.string(value.encode_utf8(&mut [0; 4]));
        Ok(())
    }

    fn serialize_str(self, value: &str) -> serde_json::Result<()> {
        self.string(value);
        Ok(())
    }

    fn serialize_bytes(self, value: &[u8]) -> serde_json::Result<()> {
        self.begin_array();
        for (index, byte) in value.iter().enumerate() {
            self.begin_array_value(index == 0);
            self.display(byte);
        }
        self.end_array();
        Ok(())
    }

    fn serialize_none(self) -> serde_json::Result<()> {
        self.serialize_unit()
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> serde_json::Result<()> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> serde_json::Result<()> {
        self.text.push_str("null");
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> serde_json::Result<()> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> serde_json::Result<()> {
        self.string(variant);
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        self.begin_variant(variant);
        value.serialize(&mut *self)?;
        self.end_variant();
        Ok(())
    }

    fn serialize_seq(self, len: Option<usize>) -> serde_json::Result<Compound<'a>> {
        self.begin_array();
        let state = if len == Some(0) {
            self.end_array();
            State::Empty
        } else {
            State::First
        };
        Ok(Compound::Container {
            encoder: self,
            state,
        })
    }

    fn serialize_tuple(self, len: usize) -> serde_json::Result<Compound<'a>> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> serde_json::Result<Compound<'a>> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> serde_json::Result<Compound<'a>> {
        self.begin_variant(variant);
        self.serialize_seq(Some(len))
    }

    fn serialize_map(self, len: Option<usize>) -> serde_json::Result<Compound<'a>> {
        self.begin_object();
        let state = if len == Some(0) {
            self.end_object();
            State::Empty
        } else {
            State::First
        };
        Ok(Compound::Container {
            encoder: self,
            state,
        })
    }

    fn serialize_struct(self, name: &'static str, len: usize) -> serde_json::Result<Compound<'a>> {
        if name == RAW_VALUE_TOKEN || name == NUMBER_TOKEN {
            return Ok(Compound::Verbatim { encoder: self });
        }
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> serde_json::Result<Compound<'a>> {
        self.begin_variant(variant);
        self.serialize_map(Some(len))
    }

    fn collect_str<T: ?Sized + std::fmt::Display>(self, value: &T) -> serde_json::Result<()> {
        self.string(&value.to_string());
        Ok(())
    }
}

impl Compound<'_> {
    fn element<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        let Compound::Container { encoder, state } = self else {
            return Err(serde_json::Error::custom("expected RawValue"));
        };
        encoder.begin_array_value(*state == State::First);
        *state = State::Rest;
        value.serialize(&mut **encoder)
    }

    fn key<T: ?Sized + Serialize>(&mut self, key: &T) -> serde_json::Result<()> {
        let Compound::Container { encoder, state } = self else {
            return Err(serde_json::Error::custom("expected RawValue"));
        };
        encoder.begin_object_key(*state == State::First);
        *state = State::Rest;
        key.serialize(KeyEncoder(encoder))?;
        encoder.end_object_key();
        Ok(())
    }

    fn value<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        let Compound::Container { encoder, .. } = self else {
            return Err(serde_json::Error::custom("expected RawValue"));
        };
        encoder.begin_object_value();
        value.serialize(&mut **encoder)?;
        encoder.end_object_value();
        Ok(())
    }

    fn field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        if let Compound::Verbatim { encoder } = self {
            return if key == RAW_VALUE_TOKEN || key == NUMBER_TOKEN {
                value.serialize(VerbatimEncoder(encoder))
            } else {
                Err(serde_json::Error::custom("expected some value"))
            };
        }
        self.key(key)?;
        self.value(value)
    }

    /// Closes the container; `close` writes the bracket of a container that is still open.
    fn finish(self, close: fn(&mut Encoder), variant: bool) -> serde_json::Result<()> {
        if let Compound::Container { encoder, state } = self {
            if state != State::Empty {
                close(encoder);
            }
            if variant {
                encoder.end_variant();
            }
        }
        Ok(())
    }
}

impl serde::ser::SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.element(value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_array, false)
    }
}

impl serde::ser::SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.element(value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_array, false)
    }
}

impl serde::ser::SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.element(value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_array, false)
    }
}

impl serde::ser::SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.element(value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_array, true)
    }
}

impl serde::ser::SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> serde_json::Result<()> {
        self.key(key)
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> serde_json::Result<()> {
        self.value(value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_object, false)
    }
}

impl serde::ser::SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        self.field(key, value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_object, false)
    }
}

impl serde::ser::SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = serde_json::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        self.field(key, value)
    }

    fn end(self) -> serde_json::Result<()> {
        self.finish(Encoder::end_object, true)
    }
}

fn key_must_be_a_string() -> serde_json::Error {
    serde_json::Error::custom("key must be a string")
}

/// Writes a map key as serde_json does: strings as they are, and booleans, integers, and
/// finite floats as quoted text.
struct KeyEncoder<'a>(&'a mut Encoder);

impl KeyEncoder<'_> {
    fn quoted(self, value: impl std::fmt::Display) -> serde_json::Result<()> {
        self.0.text.push('"');
        self.0.display(value);
        self.0.text.push('"');
        Ok(())
    }

    fn quoted_float(
        self,
        finite: bool,
        write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    ) -> serde_json::Result<()> {
        if !finite {
            return Err(serde_json::Error::custom(
                "float key must be finite (got NaN or +/-inf)",
            ));
        }
        self.0.text.push('"');
        self.0.finite_float(write)?;
        self.0.text.push('"');
        Ok(())
    }
}

impl serde::Serializer for KeyEncoder<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Impossible<(), serde_json::Error>;
    type SerializeTuple = Impossible<(), serde_json::Error>;
    type SerializeTupleStruct = Impossible<(), serde_json::Error>;
    type SerializeTupleVariant = Impossible<(), serde_json::Error>;
    type SerializeMap = Impossible<(), serde_json::Error>;
    type SerializeStruct = Impossible<(), serde_json::Error>;
    type SerializeStructVariant = Impossible<(), serde_json::Error>;

    fn serialize_str(self, value: &str) -> serde_json::Result<()> {
        self.0.string(value);
        Ok(())
    }

    fn serialize_char(self, value: char) -> serde_json::Result<()> {
        self.0.string(value.encode_utf8(&mut [0; 4]));
        Ok(())
    }

    fn serialize_bool(self, value: bool) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_i8(self, value: i8) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_i16(self, value: i16) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_i32(self, value: i32) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_i64(self, value: i64) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_i128(self, value: i128) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_u8(self, value: u8) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_u16(self, value: u16) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_u32(self, value: u32) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_u64(self, value: u64) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_u128(self, value: u128) -> serde_json::Result<()> {
        self.quoted(value)
    }

    fn serialize_f32(self, value: f32) -> serde_json::Result<()> {
        self.quoted_float(value.is_finite(), |out| {
            CompactFormatter.write_f32(out, value)
        })
    }

    fn serialize_f64(self, value: f64) -> serde_json::Result<()> {
        self.quoted_float(value.is_finite(), |out| {
            CompactFormatter.write_f64(out, value)
        })
    }

    fn serialize_bytes(self, _value: &[u8]) -> serde_json::Result<()> {
        Err(key_must_be_a_string())
    }

    fn serialize_none(self) -> serde_json::Result<()> {
        Err(key_must_be_a_string())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> serde_json::Result<()> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> serde_json::Result<()> {
        Err(key_must_be_a_string())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> serde_json::Result<()> {
        Err(key_must_be_a_string())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> serde_json::Result<()> {
        self.0.string(variant);
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> serde_json::Result<()> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> serde_json::Result<()> {
        Err(key_must_be_a_string())
    }

    fn serialize_seq(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeSeq> {
        Err(key_must_be_a_string())
    }

    fn serialize_tuple(self, _len: usize) -> serde_json::Result<Self::SerializeTuple> {
        Err(key_must_be_a_string())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleStruct> {
        Err(key_must_be_a_string())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleVariant> {
        Err(key_must_be_a_string())
    }

    fn serialize_map(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeMap> {
        Err(key_must_be_a_string())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeStruct> {
        Err(key_must_be_a_string())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeStructVariant> {
        Err(key_must_be_a_string())
    }

    fn collect_str<T: ?Sized + std::fmt::Display>(self, value: &T) -> serde_json::Result<()> {
        self.0.string(&value.to_string());
        Ok(())
    }
}

/// Writes the string field of serde_json's raw-value and number structs verbatim.
struct VerbatimEncoder<'a>(&'a mut Encoder);

/// Refuses each listed scalar the way serde_json's raw-value emitter does.
macro_rules! refuse_scalars {
    ($($method:ident($scalar:ty)),* $(,)?) => {
        $(
            fn $method(self, _value: $scalar) -> serde_json::Result<()> {
                Err(expected_raw_value())
            }
        )*
    };
}

fn expected_raw_value() -> serde_json::Error {
    serde_json::Error::custom("expected RawValue")
}

impl serde::Serializer for VerbatimEncoder<'_> {
    type Ok = ();
    type Error = serde_json::Error;
    type SerializeSeq = Impossible<(), serde_json::Error>;
    type SerializeTuple = Impossible<(), serde_json::Error>;
    type SerializeTupleStruct = Impossible<(), serde_json::Error>;
    type SerializeTupleVariant = Impossible<(), serde_json::Error>;
    type SerializeMap = Impossible<(), serde_json::Error>;
    type SerializeStruct = Impossible<(), serde_json::Error>;
    type SerializeStructVariant = Impossible<(), serde_json::Error>;

    fn serialize_str(self, value: &str) -> serde_json::Result<()> {
        self.0.text.push_str(value);
        Ok(())
    }

    refuse_scalars! {
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
    }

    fn serialize_bytes(self, _value: &[u8]) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_none(self) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _value: &T) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_unit(self) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> serde_json::Result<()> {
        Err(expected_raw_value())
    }

    fn serialize_seq(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeSeq> {
        Err(expected_raw_value())
    }

    fn serialize_tuple(self, _len: usize) -> serde_json::Result<Self::SerializeTuple> {
        Err(expected_raw_value())
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleStruct> {
        Err(expected_raw_value())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeTupleVariant> {
        Err(expected_raw_value())
    }

    fn serialize_map(self, _len: Option<usize>) -> serde_json::Result<Self::SerializeMap> {
        Err(expected_raw_value())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeStruct> {
        Err(expected_raw_value())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> serde_json::Result<Self::SerializeStructVariant> {
        Err(expected_raw_value())
    }
}

fn copy_sorted_range(text: &str, objects: &[ObjectSpans], range: Range<usize>, out: &mut String) {
    let mut position = range.start;
    let mut index = objects.partition_point(|object| object.bytes.start < position);
    while let Some(object) = objects
        .get(index)
        .filter(|object| object.bytes.start < range.end)
    {
        out.push_str(&text[position..object.bytes.start]);
        out.push('{');
        for (field_index, (field, _)) in object.fields.iter().enumerate() {
            if field_index != 0 {
                out.push(',');
            }
            copy_sorted_range(text, objects, field.clone(), out);
        }
        out.push('}');
        position = object.bytes.end;
        index = objects.partition_point(|object| object.bytes.start < position);
    }
    out.push_str(&text[position..range.end]);
}

/// A message's canonical bytes together with the deepest container nesting they hold,
/// measured during the one serialization pass; reordering fields moves whole spans and
/// leaves the nesting unchanged.
pub(crate) struct CanonicalMessage {
    pub(crate) bytes: Vec<u8>,
    /// The root object is depth 1, so a message with scalar-only blocks measures 3.
    pub(crate) nesting_depth: usize,
}

/// Serializes a wire message once, preserving serde's scalar forms and field omissions, and
/// reports the canonical bytes with their nesting depth.
pub(crate) fn canonical_message(
    message: &memory_store::WireMessage,
) -> serde_json::Result<CanonicalMessage> {
    let encoded = serialize_with_spans(message)?;
    let nesting_depth = encoded.max_depth;
    Ok(CanonicalMessage {
        bytes: finalize(encoded),
        nesting_depth,
    })
}

#[cfg(feature = "test-support")]
pub fn canonical_served_bytes_for_test(message: &memory_store::WireMessage) -> Vec<u8> {
    canonical_message(message)
        .expect("CK wire message values must always serialize")
        .bytes
}

/// The one canonical text of a block: projection identity, served fingerprint
/// fallback, and decoded sidecar fingerprints all hash exactly these bytes.
pub(crate) fn canonical_block_bytes(block: &memory_store::WireBlock) -> serde_json::Result<String> {
    encode_text(block)
}

/// Encodes a block array in canonical form for prompt text that embeds a message's
/// complete content.
pub(crate) fn canonical_blocks_text(
    blocks: &[memory_store::WireBlock],
) -> serde_json::Result<String> {
    encode_text(&blocks)
}

/// [`canonical_block_bytes`] as a shared string, encoded through `scratch` so a run of blocks
/// reuses one set of serialization buffers.
pub(crate) fn canonical_block_text(
    scratch: &mut CanonicalScratch,
    block: &memory_store::WireBlock,
) -> serde_json::Result<Arc<str>> {
    scratch.encoder.reset();
    block.serialize(&mut scratch.encoder)?;
    #[cfg(test)]
    FINALIZATIONS.with(|count| count.set(count.get() + 1));
    let CanonicalScratch { encoder, sorted } = scratch;
    if !sort_all_fields(&encoder.text, encoder.objects.live_mut()) {
        let text = Arc::from(encoder.text.as_str());
        release_oversized(&mut encoder.text);
        return Ok(text);
    }
    sorted.clear();
    sorted.reserve(encoder.text.len());
    copy_sorted_range(
        &encoder.text,
        encoder.objects.live(),
        0..encoder.text.len(),
        sorted,
    );
    release_oversized(&mut encoder.text);
    let text = Arc::from(sorted.as_str());
    release_oversized(sorted);
    Ok(text)
}

/// The largest buffer capacity a [`CanonicalScratch`] keeps between blocks. A larger buffer is
/// freed after its contents are copied, limiting scratch capacity retained between blocks.
const RETAINED_SCRATCH_BYTES: usize = 1 << 20;

fn release_oversized(buffer: &mut String) {
    if buffer.capacity() > RETAINED_SCRATCH_BYTES {
        *buffer = String::new();
    }
}

#[cfg(feature = "test-support")]
pub fn canonical_block_bytes_for_test(block: &memory_store::WireBlock) -> String {
    canonical_block_bytes(block).expect("CK wire blocks must always serialize")
}

/// Output of the single serialization pass: the compact text and every recorded
/// object's field spans in source order.
struct Encoded {
    bytes: String,
    objects: Vec<ObjectSpans>,
    /// The deepest container nesting in `bytes`; 0 for a scalar document.
    max_depth: usize,
}

#[cfg(test)]
thread_local! {
    pub(crate) static FINALIZATIONS: Cell<usize> = const { Cell::new(0) };
}

fn encode(value: &impl Serialize) -> serde_json::Result<Vec<u8>> {
    Ok(finalize(serialize_with_spans(value)?))
}

fn encode_text(value: &impl Serialize) -> serde_json::Result<String> {
    encode(value).map(|bytes| {
        String::from_utf8(bytes).expect("the encoder writes a String and copies whole spans")
    })
}

fn serialize_with_spans(value: &impl Serialize) -> serde_json::Result<Encoded> {
    let mut encoder = Encoder::default();
    value.serialize(&mut encoder)?;
    let mut objects = encoder.objects.items;
    objects.truncate(encoder.objects.len);
    Ok(Encoded {
        bytes: encoder.text,
        objects,
        max_depth: encoder.max_depth,
    })
}

/// Serialization buffers that keep their capacity from one encoding to the next.
#[derive(Default)]
pub(crate) struct CanonicalScratch {
    encoder: Encoder,
    /// The reordered copy of a disordered document.
    sorted: String,
}

/// Returns the serialization buffer itself when every object is already in
/// canonical order; the reorder copy produces the same bytes, so only a
/// disordered document pays for a second buffer.
fn finalize(mut encoded: Encoded) -> Vec<u8> {
    #[cfg(test)]
    FINALIZATIONS.with(|count| count.set(count.get() + 1));
    if !sort_all_fields(&encoded.bytes, &mut encoded.objects) {
        return encoded.bytes.into_bytes();
    }
    let mut out = String::with_capacity(encoded.bytes.len());
    copy_sorted_range(
        &encoded.bytes,
        &encoded.objects,
        0..encoded.bytes.len(),
        &mut out,
    );
    out.into_bytes()
}

/// `|=` never short-circuits, so a disordered object late in the document is
/// sorted and reported even when every earlier object is already ordered.
fn sort_all_fields(text: &str, objects: &mut [ObjectSpans]) -> bool {
    let mut changed = false;
    for object in objects {
        changed |= sort_fields(text.as_bytes(), &mut object.fields);
    }
    changed
}

/// Keys without a backslash compare as raw bytes between the quotes: serde writes non-ASCII
/// unescaped, and UTF-8 byte order equals `str` order. The quotes are excluded because `"`
/// sorts after space, which would misorder `"a"` against `"a b"`. Escaped keys decode first:
/// sorting escaped spellings would misorder control characters. Returns whether any field
/// moved.
fn sort_fields(bytes: &[u8], fields: &mut [(Range<usize>, usize)]) -> bool {
    if fields.len() < 2 {
        return false;
    }
    let raw_key = |(field, key_end): &(Range<usize>, usize)| &bytes[field.start + 1..key_end - 1];
    if fields.iter().all(|field| !raw_key(field).contains(&b'\\')) {
        if fields.is_sorted_by_key(raw_key) {
            return false;
        }
        fields.sort_by_key(raw_key);
        return true;
    }
    let decoded_key = |field: &(Range<usize>, usize)| {
        serde_json::from_slice::<String>(&bytes[field.0.start..field.1])
            .expect("serde emits valid string keys")
    };
    fields.sort_by_cached_key(decoded_key);
    // Field starts are recorded in source order and are strictly increasing, so a
    // permutation that keeps them increasing is the identity.
    !fields.is_sorted_by_key(|(field, _)| field.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emits `pairs` as a map in the given order.
    struct Ordered<'a>(&'a [(&'a str, serde_json::Value)]);

    impl Serialize for Ordered<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map(self.0.iter().map(|(key, value)| (*key, value)))
        }
    }

    fn reference(value: &impl Serialize) -> Vec<u8> {
        serde_json::to_vec(&serde_json::to_value(value).unwrap()).unwrap()
    }

    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for (index, head) in items.iter().enumerate() {
            let mut rest = items.to_vec();
            rest.remove(index);
            for mut tail in permutations(&rest) {
                tail.insert(0, head.clone());
                out.push(tail);
            }
        }
        out
    }

    fn finalizations() -> usize {
        FINALIZATIONS.with(Cell::get)
    }

    #[test]
    fn every_key_permutation_reports_change_exactly_when_disordered() {
        // Each triple is listed in decoded order. Values carry the source index,
        // so a stable sort of equal keys is observable.
        let triples: [[&str; 3]; 6] = [
            ["a", "b", "c"],
            ["\n", "a", "\u{e9}"],
            ["a", "a b", "a\""],
            ["\u{1}", "!", "\u{1F600}"],
            ["a", "a", "b"],
            ["\n", "\n", "a"],
        ];
        for sorted_keys in triples {
            for permutation in permutations(&sorted_keys) {
                let pairs: Vec<(&str, serde_json::Value)> = permutation
                    .iter()
                    .enumerate()
                    .map(|(index, key)| (*key, serde_json::json!(index)))
                    .collect();
                let source = Ordered(&pairs);
                // `reference` would collapse duplicate keys; a stable sort keeps them.
                let mut stable = pairs.clone();
                stable.sort_by_key(|(key, _)| *key);
                let expected = serde_json::to_vec(&Ordered(&stable)).unwrap();
                let mut encoded = serialize_with_spans(&source).unwrap();
                let changed = sort_all_fields(&encoded.bytes, &mut encoded.objects);
                let disordered = !permutation.is_sorted();
                assert_eq!(changed, disordered, "{permutation:?}");
                assert_eq!(encode(&source).unwrap(), expected, "{permutation:?}");
                assert_eq!(
                    serde_json::to_vec(&source).unwrap() == expected,
                    !disordered,
                    "{permutation:?}"
                );
            }
        }
    }

    #[test]
    fn nesting_depth_counts_arrays_and_objects_from_the_root_container() {
        let depth = |value: &serde_json::Value| serialize_with_spans(value).unwrap().max_depth;
        assert_eq!(depth(&serde_json::json!("scalar")), 0);
        assert_eq!(depth(&serde_json::json!([])), 1);
        assert_eq!(depth(&serde_json::json!({"a": 1})), 1);
        assert_eq!(depth(&serde_json::json!({"a": [1, {"b": []}], "c": 2})), 4);
        assert_eq!(depth(&serde_json::json!([[["[{\"]"]], [1]])), 3);
    }

    #[test]
    fn canonical_message_depth_matches_a_scan_of_the_reordered_bytes() {
        use memory_store::{BlockKind, HarnessMeta, ProviderExtras, WireBlock, WireMessage};
        let message = WireMessage::from_parts(
            "assistant",
            vec![WireBlock::bare(BlockKind::ToolCall {
                id: "call".into(),
                name: "read".into(),
                input: serde_json::json!({"z": [{"y": [["]}"]]}], "a": 1}),
                provider_executed: false,
            })],
            None,
            ProviderExtras::new(),
            HarnessMeta::default(),
        );
        let canonical = canonical_message(&message).unwrap();
        assert_eq!(canonical.bytes, encode(&message).unwrap());
        let mut depth = 0usize;
        let mut deepest = 0usize;
        let mut quoted = false;
        let mut escaped = false;
        for &byte in &canonical.bytes {
            match (quoted, escaped, byte) {
                (true, true, _) => escaped = false,
                (true, false, b'\\') => escaped = true,
                (true, false, b'"') => quoted = false,
                (true, false, _) => {}
                (false, _, b'"') => quoted = true,
                (false, _, b'[' | b'{') => {
                    depth += 1;
                    deepest = deepest.max(depth);
                }
                (false, _, b']' | b'}') => depth -= 1,
                _ => {}
            }
        }
        assert_eq!(canonical.nesting_depth, deepest);
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            canonical.nesting_depth,
            crate::edit_recipe::validate_json_nesting(&value).unwrap()
        );
    }

    #[test]
    fn aggregate_order_decision_visits_every_object() {
        // `serde_json::Value` maps are already sorted, so child disorder needs a
        // declaration-ordered struct.
        #[derive(Serialize)]
        struct Za {
            z: u8,
            a: u8,
        }
        #[derive(Serialize)]
        struct Late {
            a: serde_json::Value,
            b: Vec<Za>,
        }
        let late = Late {
            a: serde_json::json!({"b": 1}),
            b: vec![Za { z: 0, a: 0 }, Za { z: 2, a: 3 }],
        };
        let mut encoded = serialize_with_spans(&late).unwrap();
        let ordered_objects: Vec<bool> = encoded
            .objects
            .iter()
            .map(|object| {
                object
                    .fields
                    .is_sorted_by_key(|(field, key_end)| &encoded.bytes[field.start..*key_end])
            })
            .collect();
        assert_eq!(ordered_objects, [true, true, false, false]);
        assert!(sort_all_fields(&encoded.bytes, &mut encoded.objects));
        assert_eq!(encode(&late).unwrap(), reference(&late));

        // Only the last object in the document is disordered.
        #[derive(Serialize)]
        struct Tail {
            a: serde_json::Value,
            b: Vec<serde_json::Value>,
            c: Za,
        }
        let tail = Tail {
            a: serde_json::json!({"a": 1}),
            b: vec![serde_json::json!({"a": 1, "b": 2})],
            c: Za { z: 1, a: 2 },
        };
        let mut encoded = serialize_with_spans(&tail).unwrap();
        assert!(sort_all_fields(&encoded.bytes, &mut encoded.objects));
        assert_eq!(encode(&tail).unwrap(), reference(&tail));

        // Root disordered, every child ordered.
        let root_only = Ordered(&[
            ("b", serde_json::json!({"a": 1, "b": 2})),
            ("a", serde_json::json!({})),
        ]);
        let mut encoded = serialize_with_spans(&root_only).unwrap();
        assert!(sort_all_fields(&encoded.bytes, &mut encoded.objects));
        assert_eq!(encode(&root_only).unwrap(), reference(&root_only));

        // Everything ordered, including empty and singleton objects and escaped keys.
        let ordered = Ordered(&[
            ("\n", serde_json::json!({})),
            (
                "a",
                serde_json::json!({"only": [1, {"k": {"x": 1, "y": 2}}]}),
            ),
            ("b", serde_json::json!({"\u{1}": 1, "!": 2, "a b": 3})),
        ]);
        let mut encoded = serialize_with_spans(&ordered).unwrap();
        assert!(!sort_all_fields(&encoded.bytes, &mut encoded.objects));
        assert_eq!(encode(&ordered).unwrap(), reference(&ordered));
    }

    /// Copying the completed buffer through unchanged source-order tables reproduces
    /// it byte for byte, whole and per object, whether or not the input is ordered.
    #[test]
    fn unchanged_span_copy_is_identity() {
        let cases = [
            r#"{}"#,
            r#"{"a":1}"#,
            r#"{"z":{},"a":[{},{"b":[]}]}"#,
            r#"{"a":[1,{"z":"}{,\"","a":"\\"},2],"b":"\u0001,{}"}"#,
            r#"{"\n":true,"!":false,"\u00e9":"\ud83d\ude00","😀":null}"#,
            r#"{"n":[-0.0,0.0,1.0,1e30,1e-30,-9223372036854775808,18446744073709551615]}"#,
            r#"{"b":{"a b":{"y":1,"x":2},"a":[{"k":1,"j":2}]},"a":null}"#,
        ];
        for case in cases {
            let value: serde_json::Value = serde_json::from_str(case).unwrap();
            let map = value.as_object().unwrap();
            let ordered: Vec<(&str, serde_json::Value)> = map
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect();
            let mut reversed = ordered.clone();
            reversed.reverse();
            // A root with at least two keys makes `ordered` and `reversed` distinct.
            assert_eq!(
                serde_json::to_vec(&Ordered(&ordered)).unwrap()
                    == serde_json::to_vec(&Ordered(&reversed)).unwrap(),
                ordered.len() < 2,
                "{case}"
            );
            for source in [Ordered(&ordered), Ordered(&reversed)] {
                let encoded = serialize_with_spans(&source).unwrap();
                let mut whole = String::new();
                copy_sorted_range(
                    &encoded.bytes,
                    &encoded.objects,
                    0..encoded.bytes.len(),
                    &mut whole,
                );
                assert_eq!(whole, encoded.bytes, "{case}");
                for object in &encoded.objects {
                    let mut part = String::new();
                    copy_sorted_range(
                        &encoded.bytes,
                        &encoded.objects,
                        object.bytes.clone(),
                        &mut part,
                    );
                    assert_eq!(part, encoded.bytes[object.bytes.clone()], "{case}");
                    for (field, _) in &object.fields {
                        let mut part = String::new();
                        copy_sorted_range(
                            &encoded.bytes,
                            &encoded.objects,
                            field.clone(),
                            &mut part,
                        );
                        assert_eq!(part, encoded.bytes[field.clone()], "{case}");
                    }
                }
            }
        }
    }

    #[test]
    fn canonical_input_returns_the_serialization_buffer_and_disordered_input_does_not() {
        let ordered = Ordered(&[
            ("\n", serde_json::json!({"a": 1, "b": [{"c": 2}]})),
            ("a", serde_json::json!("x".repeat(4096))),
        ]);
        let encoded = serialize_with_spans(&ordered).unwrap();
        let (ptr, len, capacity) = (
            encoded.bytes.as_ptr(),
            encoded.bytes.len(),
            encoded.bytes.capacity(),
        );
        let before = finalizations();
        let out = finalize(encoded);
        assert_eq!(finalizations(), before + 1);
        assert_eq!(out.as_ptr(), ptr);
        assert_eq!(out.len(), len);
        assert_eq!(out.capacity(), capacity, "no shrink on the ownership path");
        assert_eq!(out, reference(&ordered));

        let disordered = Ordered(&[
            ("a", serde_json::json!("x".repeat(4096))),
            ("\n", serde_json::json!({"b": [{"c": 2}], "a": 1})),
        ]);
        let encoded = serialize_with_spans(&disordered).unwrap();
        let ptr = encoded.bytes.as_ptr();
        let out = finalize(encoded);
        assert_ne!(out.as_ptr(), ptr);
        assert_eq!(out.capacity(), out.len());
        assert_eq!(out, reference(&disordered));
    }

    #[test]
    fn serialization_error_returns_before_finalization() {
        struct Failing {
            visits: Cell<usize>,
            after_prefix: bool,
        }
        impl Serialize for Failing {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::{Error, SerializeMap};
                self.visits.set(self.visits.get() + 1);
                if !self.after_prefix {
                    return Err(S::Error::custom("refused before writing"));
                }
                let mut outer = serializer.serialize_map(None)?;
                outer.serialize_entry("z", &1)?;
                outer.serialize_entry("a", &OpenInner)?;
                unreachable!("the nested value refuses")
            }
        }
        /// Fails between a nested object's first entry and its close, so the
        /// formatter stack holds two open objects at the error.
        struct OpenInner;
        impl Serialize for OpenInner {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::{Error, SerializeMap};
                let mut inner = serializer.serialize_map(None)?;
                inner.serialize_entry("b", &1)?;
                Err(S::Error::custom("refused with an open nested object"))
            }
        }
        for after_prefix in [false, true] {
            let failing = Failing {
                visits: Cell::new(0),
                after_prefix,
            };
            let before = finalizations();
            let error = encode(&failing).unwrap_err();
            assert_eq!(finalizations(), before, "no finalization on error");
            assert_eq!(failing.visits.get(), 1);
            let expected = if after_prefix {
                "refused with an open nested object"
            } else {
                "refused before writing"
            };
            assert_eq!(error.to_string(), expected);
            assert!(serialize_with_spans(&failing).is_err());
            assert_eq!(failing.visits.get(), 2);
        }
        // Successful canonical and disordered controls each finalize exactly once.
        let before = finalizations();
        encode(&Ordered(&[("a", serde_json::json!(1))])).unwrap();
        encode(&Ordered(&[
            ("b", serde_json::json!(1)),
            ("a", serde_json::json!(2)),
        ]))
        .unwrap();
        assert_eq!(finalizations(), before + 2);
    }

    #[test]
    fn canonical_encoding_visits_each_serialize_implementation_once() {
        struct Counted<'a>(&'a Cell<usize>);
        impl Serialize for Counted<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.0.set(self.0.get() + 1);
                serde_json::json!({"z": [{"z": 1.0, "a": -0.0}], "\n": true, "a": "é"})
                    .serialize(serializer)
            }
        }
        #[derive(Serialize)]
        struct Shell<'a> {
            z: Counted<'a>,
            a: Counted<'a>,
        }
        let count = Cell::new(0);
        let bytes = encode(&Shell {
            z: Counted(&count),
            a: Counted(&count),
        })
        .unwrap();
        assert_eq!(count.get(), 2);
        assert_eq!(bytes, "{\"a\":{\"\\n\":true,\"a\":\"é\",\"z\":[{\"a\":-0.0,\"z\":1.0}]},\"z\":{\"\\n\":true,\"a\":\"é\",\"z\":[{\"a\":-0.0,\"z\":1.0}]}}".as_bytes());
    }

    #[test]
    fn canonical_encoding_preserves_nested_scalars_and_decoded_key_order() {
        let values: serde_json::Value = serde_json::from_str(
            r#"[{},[],null,true,false,-0.0,0.0,1.0,1e30,1e-30,-9223372036854775808,18446744073709551615,{"😀":{},"é":[],"\\":false,"/":true,"\n":null,"\u0001":"\"\\\b\f\n\r\t"}]"#,
        )
        .unwrap();
        #[derive(Serialize)]
        struct Shell<'a> {
            z: &'a serde_json::Value,
            a: [&'a serde_json::Value; 2],
        }
        for value in values.as_array().unwrap() {
            let shell = Shell {
                z: value,
                a: [value, &values],
            };
            let expected = serde_json::to_vec(&serde_json::to_value(&shell).unwrap()).unwrap();
            assert_eq!(encode(&shell).unwrap(), expected);
            assert_ne!(serde_json::to_vec(&shell).unwrap(), expected);
        }
    }

    #[test]
    fn canonical_encoding_orders_prefix_and_escaped_keys_like_decoded_strings() {
        // Quoted spellings misorder each pair: `"a"` sorts after `"a b"` because `"` > ` `,
        // and `"\n"` sorts after `"!"` because `\` > `!`.
        let cases = [
            r#"{"a b":1,"a":2}"#,
            r#"{"a\"":1,"a":2,"a b":3}"#,
            r#"{"!":1,"\n":2}"#,
            r#"{"\u0001":1,"\t":2,"\n":3," ":4}"#,
            r#"{"é":1,"z":2,"\u00e9x":3}"#,
            r#"{"b":{"a b":{"y":1,"x":2},"a":[{"k":1,"j":2}]},"a":null}"#,
        ];
        for case in cases {
            let value: serde_json::Value = serde_json::from_str(case).unwrap();
            let expected = serde_json::to_vec(&value).unwrap();
            let map = value.as_object().unwrap();
            let mut reversed: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            reversed.reverse();
            struct Reversed<'a>(&'a [(&'a String, &'a serde_json::Value)]);
            impl Serialize for Reversed<'_> {
                fn serialize<S: serde::Serializer>(
                    &self,
                    serializer: S,
                ) -> Result<S::Ok, S::Error> {
                    serializer.collect_map(self.0.iter().map(|(key, value)| (*key, *value)))
                }
            }
            assert_eq!(encode(&value).unwrap(), expected, "{case}");
            assert_eq!(encode(&Reversed(&reversed)).unwrap(), expected, "{case}");
            assert_ne!(
                serde_json::to_vec(&Reversed(&reversed)).unwrap(),
                expected,
                "{case}"
            );
        }
    }

    /// Serializes through every entry point serde offers, including map keys serde_json
    /// quotes, a raw value, empty containers, and non-finite floats.
    #[derive(Serialize)]
    struct EveryMethod {
        unit: (),
        unit_struct: UnitStruct,
        newtype: Newtype,
        tuple_struct: TupleStruct,
        integers: (i8, i16, i32, i64, u8, u16, u32, u64),
        wide: (i128, u128),
        floats: (f32, f64, f64, f64, f64, f32),
        chars: (char, char, char),
        variants: Vec<Variant>,
        bytes: Bytes,
        collected: Collected,
        none: Option<u8>,
        some: Option<&'static str>,
        empty_seq: Vec<u8>,
        empty_map: std::collections::BTreeMap<String, u8>,
        integer_keys: std::collections::BTreeMap<i64, u8>,
        bool_keys: std::collections::BTreeMap<bool, u8>,
        char_keys: std::collections::BTreeMap<char, u8>,
        variant_keys: std::collections::BTreeMap<Variant, u8>,
        float_keys: FloatKeys,
        raw: Box<serde_json::value::RawValue>,
        strings: Vec<String>,
    }

    #[derive(Serialize)]
    struct UnitStruct;

    #[derive(Serialize)]
    struct Newtype(&'static str);

    #[derive(Serialize)]
    struct TupleStruct(u16, char);

    #[derive(Serialize, PartialEq, Eq, PartialOrd, Ord)]
    enum Variant {
        Unit,
        Newtype(u8),
        Tuple(i8, String),
        Struct { z: i32, a: Option<bool> },
        EmptyTuple(),
        EmptyStruct {},
    }

    struct Bytes(&'static [u8]);

    impl Serialize for Bytes {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bytes(self.0)
        }
    }

    struct Collected;

    impl Serialize for Collected {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(&format_args!("a\"b{}\\\u{1}", 7))
        }
    }

    struct FloatKeys;

    impl Serialize for FloatKeys {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map([(1.5f64, 1), (-0.0, 2), (1e30, 3)])
        }
    }

    /// Strings whose escapes fall on either side of the eight-byte word boundaries.
    fn boundary_strings() -> Vec<String> {
        let mut strings = Vec::new();
        for length in [0, 1, 7, 8, 9, 15, 16, 17, 24] {
            for escape in [
                '"', '\\', '\n', '\u{0}', '\u{1f}', '\u{8}', '\u{c}', '\r', '\t',
            ] {
                for position in 0..length {
                    let mut text: Vec<char> =
                        "abcdefghijklmnopqrstuvwx".chars().take(length).collect();
                    text[position] = escape;
                    strings.push(text.into_iter().collect());
                }
            }
        }
        strings.push("\u{7f} é 😀 \u{2028} ünïcödé \u{fffd}".to_string());
        strings.push((0u8..0x80).map(char::from).collect());
        strings.push("\"".repeat(17));
        strings
    }

    #[test]
    fn encoder_text_equals_serde_json_for_every_serializer_method() {
        let value = EveryMethod {
            unit: (),
            unit_struct: UnitStruct,
            newtype: Newtype("new\ttype"),
            tuple_struct: TupleStruct(7, 'é'),
            integers: (
                i8::MIN,
                i16::MIN,
                i32::MIN,
                i64::MIN,
                u8::MAX,
                u16::MAX,
                u32::MAX,
                u64::MAX,
            ),
            wide: (i128::MIN, u128::MAX),
            floats: (1.5, -0.0, 1e300, f64::NAN, f64::NEG_INFINITY, f32::INFINITY),
            chars: ('"', '\u{1}', '😀'),
            variants: vec![
                Variant::Unit,
                Variant::Newtype(3),
                Variant::Tuple(-1, "t\"".to_string()),
                Variant::Struct { z: 2, a: None },
                Variant::Struct {
                    z: -2,
                    a: Some(true),
                },
                Variant::EmptyTuple(),
                Variant::EmptyStruct {},
            ],
            bytes: Bytes(&[0, 1, 255]),
            collected: Collected,
            none: None,
            some: Some("some"),
            empty_seq: Vec::new(),
            empty_map: std::collections::BTreeMap::new(),
            integer_keys: [(-3, 1), (12, 2)].into_iter().collect(),
            bool_keys: [(false, 1), (true, 2)].into_iter().collect(),
            char_keys: [('\n', 1), ('z', 2)].into_iter().collect(),
            variant_keys: [(Variant::Unit, 1)].into_iter().collect(),
            float_keys: FloatKeys,
            raw: serde_json::value::RawValue::from_string(r#"{"z":[1, 2],"a":"\n"}"#.to_string())
                .unwrap(),
            strings: boundary_strings(),
        };
        let expected = serde_json::to_string(&value).unwrap();
        assert_eq!(serialize_with_spans(&value).unwrap().bytes, expected);
        for text in boundary_strings() {
            assert_eq!(
                serialize_with_spans(&text).unwrap().bytes,
                serde_json::to_string(&text).unwrap(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn encoder_refuses_the_map_keys_serde_json_refuses() {
        let unit_key: std::collections::BTreeMap<(), u8> = [((), 1)].into_iter().collect();
        assert!(serde_json::to_string(&unit_key).is_err());
        assert!(encode(&unit_key).is_err());
        let sequence_key: std::collections::BTreeMap<Vec<u8>, u8> =
            [(vec![1], 1)].into_iter().collect();
        assert!(serde_json::to_string(&sequence_key).is_err());
        assert!(encode(&sequence_key).is_err());
        struct NanKey;
        impl Serialize for NanKey {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_map([(f64::NAN, 1)])
            }
        }
        assert!(serde_json::to_string(&NanKey).is_err());
        assert!(encode(&NanKey).is_err());
    }

    fn generated_string() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        proptest::collection::vec(
            prop_oneof![
                Just('"'),
                Just('\\'),
                (0u8..0x20).prop_map(char::from),
                Just('\u{7f}'),
                Just('é'),
                Just('😀'),
                proptest::char::range('a', 'e'),
                any::<char>(),
            ],
            0..40,
        )
        .prop_map(|chars| chars.into_iter().collect())
    }

    fn generated_value() -> impl proptest::strategy::Strategy<Value = serde_json::Value> {
        use proptest::prelude::*;
        let leaf = prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::from),
            any::<i64>().prop_map(serde_json::Value::from),
            any::<u64>().prop_map(serde_json::Value::from),
            any::<f64>()
                .prop_filter("finite", |value| value.is_finite())
                .prop_map(serde_json::Value::from),
            generated_string().prop_map(serde_json::Value::from),
        ];
        leaf.prop_recursive(4, 64, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::from),
                proptest::collection::btree_map(generated_string(), inner, 0..6)
                    .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
            ]
        })
    }

    /// Emits a value's objects with their fields reversed, so every object with two or more
    /// fields arrives disordered.
    struct ReversedObjects<'a>(&'a serde_json::Value);

    impl Serialize for ReversedObjects<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self.0 {
                serde_json::Value::Object(map) => serializer.collect_map(
                    map.iter()
                        .rev()
                        .map(|(key, value)| (key, ReversedObjects(value))),
                ),
                serde_json::Value::Array(items) => {
                    serializer.collect_seq(items.iter().map(ReversedObjects))
                }
                scalar => scalar.serialize(serializer),
            }
        }
    }

    #[test]
    fn scratch_keeps_small_buffers_and_frees_oversized_ones() {
        let block =
            |text: String| memory_store::WireBlock::bare(memory_store::BlockKind::Text { text });
        let mut scratch = CanonicalScratch::default();
        let small = block("a".repeat(1024));
        let reference = serde_json::to_string(&serde_json::to_value(&small).unwrap()).unwrap();
        assert_eq!(
            &*canonical_block_text(&mut scratch, &small).unwrap(),
            reference
        );
        assert!(scratch.encoder.text.capacity() >= reference.len());
        assert!(scratch.sorted.capacity() >= reference.len());

        let large = block("b".repeat(2 * RETAINED_SCRATCH_BYTES));
        let reference = serde_json::to_string(&serde_json::to_value(&large).unwrap()).unwrap();
        assert_eq!(
            &*canonical_block_text(&mut scratch, &large).unwrap(),
            reference
        );
        assert_eq!(scratch.encoder.text.capacity(), 0);
        assert_eq!(scratch.sorted.capacity(), 0);
        assert_eq!(&*canonical_block_text(&mut scratch, &small).unwrap(), {
            serde_json::to_string(&serde_json::to_value(&small).unwrap()).unwrap()
        });
    }

    proptest::proptest! {
        #[test]
        fn encoder_matches_serde_json_on_generated_values(value in generated_value()) {
            proptest::prop_assert_eq!(
                serialize_with_spans(&value).unwrap().bytes,
                serde_json::to_string(&value).unwrap()
            );
            let expected = serde_json::to_vec(&value).unwrap();
            proptest::prop_assert_eq!(&encode(&value).unwrap(), &expected);
            proptest::prop_assert_eq!(&encode(&ReversedObjects(&value)).unwrap(), &expected);
            let mut scratch = CanonicalScratch::default();
            let block = memory_store::WireBlock::bare(memory_store::BlockKind::ToolCall {
                id: "call".to_string(),
                name: "tool".to_string(),
                input: value.clone(),
                provider_executed: false,
            });
            let reference = serde_json::to_string(&serde_json::to_value(&block).unwrap()).unwrap();
            proptest::prop_assert_eq!(
                &*canonical_block_text(&mut scratch, &block).unwrap(),
                reference.as_str()
            );
            proptest::prop_assert_eq!(canonical_block_bytes(&block).unwrap(), reference);
        }
    }
}
