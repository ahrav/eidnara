//! Canonical served JSON uses serde's encoding with recursively sorted object fields.
//!
//! Formatter hooks record byte spans during the single serialization. Reordering
//! copies those spans without decoding values or changing number and string forms.

use std::cell::Cell;
use std::io::{self, Write};
use std::ops::Range;

use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter};

#[derive(Default)]
struct ObjectSpans {
    bytes: Range<usize>,
    fields: Vec<(Range<usize>, usize)>,
}

struct SpanWriter<'a> {
    bytes: Vec<u8>,
    position: &'a Cell<usize>,
}

impl Write for SpanWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        self.position.set(self.bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct SpanFormatter<'a> {
    position: &'a Cell<usize>,
    objects: &'a mut Vec<ObjectSpans>,
    stack: Vec<usize>,
}

impl Formatter for SpanFormatter<'_> {
    fn begin_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.stack.push(self.objects.len());
        self.objects.push(ObjectSpans {
            bytes: self.position.get()..0,
            ..Default::default()
        });
        CompactFormatter.begin_object(writer)
    }

    fn end_object<W: ?Sized + Write>(&mut self, writer: &mut W) -> io::Result<()> {
        CompactFormatter.end_object(writer)?;
        let index = self.stack.pop().expect("serde closes an open object");
        self.objects[index].bytes.end = self.position.get();
        Ok(())
    }

    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        CompactFormatter.begin_object_key(writer, first)?;
        let index = *self.stack.last().expect("serde keys belong to an object");
        self.objects[index].fields.push((self.position.get()..0, 0));
        Ok(())
    }

    fn end_object_key<W: ?Sized + Write>(&mut self, _writer: &mut W) -> io::Result<()> {
        let index = *self.stack.last().expect("serde keys belong to an object");
        self.objects[index].fields.last_mut().expect("key began").1 = self.position.get();
        Ok(())
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, _writer: &mut W) -> io::Result<()> {
        let index = *self.stack.last().expect("serde values belong to an object");
        let (field, _) = self.objects[index].fields.last_mut().expect("key began");
        field.end = self.position.get();
        Ok(())
    }
}

fn copy_sorted_range(
    bytes: &[u8],
    objects: &[ObjectSpans],
    range: Range<usize>,
    out: &mut Vec<u8>,
) {
    let mut position = range.start;
    let mut index = objects.partition_point(|object| object.bytes.start < position);
    while let Some(object) = objects
        .get(index)
        .filter(|object| object.bytes.start < range.end)
    {
        out.extend_from_slice(&bytes[position..object.bytes.start]);
        out.push(b'{');
        for (field_index, (field, _)) in object.fields.iter().enumerate() {
            if field_index != 0 {
                out.push(b',');
            }
            copy_sorted_range(bytes, objects, field.clone(), out);
        }
        out.push(b'}');
        position = object.bytes.end;
        index = objects.partition_point(|object| object.bytes.start < position);
    }
    out.extend_from_slice(&bytes[position..range.end]);
}

/// Serializes a wire message once, preserving serde's scalar forms and field omissions.
pub(crate) fn to_vec(message: &memory_store::WireMessage) -> serde_json::Result<Vec<u8>> {
    encode(message)
}

fn encode(value: &impl Serialize) -> serde_json::Result<Vec<u8>> {
    let position = Cell::new(0);
    let mut objects = Vec::new();
    let writer = SpanWriter {
        bytes: Vec::new(),
        position: &position,
    };
    let formatter = SpanFormatter {
        position: &position,
        objects: &mut objects,
        stack: Vec::new(),
    };
    let mut serializer = serde_json::Serializer::with_formatter(writer, formatter);
    value.serialize(&mut serializer)?;
    let bytes = serializer.into_inner().bytes;
    for object in &mut objects {
        // Decode keys only: sorting escaped spellings would misorder control characters.
        object.fields.sort_by_cached_key(|(field, key_end)| {
            serde_json::from_slice::<String>(&bytes[field.start..*key_end])
                .expect("serde emits valid string keys")
        });
    }
    let mut out = Vec::with_capacity(bytes.len());
    copy_sorted_range(&bytes, &objects, 0..bytes.len(), &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
