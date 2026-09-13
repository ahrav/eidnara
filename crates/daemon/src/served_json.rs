//! Canonical served JSON uses serde's encoding with recursively sorted object fields.
//!
//! Formatter hooks record byte spans during the single serialization. Reordering
//! copies those spans without decoding values or changing number and string forms.

use std::cell::Cell;
use std::io::{self, Write};
use std::ops::Range;

use serde::Serialize;
use serde::ser::Error as _;
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

#[cfg(feature = "test-support")]
pub fn canonical_served_bytes_for_test(message: &memory_store::WireMessage) -> Vec<u8> {
    to_vec(message).expect("CK wire message values must always serialize")
}

/// The one canonical text of a block: projection identity, served fingerprint
/// fallback, and decoded sidecar fingerprints all hash exactly these bytes.
pub(crate) fn canonical_block_bytes(block: &memory_store::WireBlock) -> serde_json::Result<String> {
    let bytes = encode(block)?;
    // serde emits valid UTF-8 and the copier moves whole spans, so this check
    // cannot fail; it stays because the crate forbids the unchecked conversion.
    String::from_utf8(bytes).map_err(|error| serde_json::Error::custom(error.to_string()))
}

#[cfg(feature = "test-support")]
pub fn canonical_block_bytes_for_test(block: &memory_store::WireBlock) -> String {
    canonical_block_bytes(block).expect("CK wire blocks must always serialize")
}

/// Output of the single serialization pass: the compact bytes and every recorded
/// object's field spans in source order.
struct Encoded {
    bytes: Vec<u8>,
    objects: Vec<ObjectSpans>,
}

#[cfg(test)]
thread_local! {
    static FINALIZATIONS: Cell<usize> = const { Cell::new(0) };
}

fn encode(value: &impl Serialize) -> serde_json::Result<Vec<u8>> {
    Ok(finalize(serialize_with_spans(value)?))
}

fn serialize_with_spans(value: &impl Serialize) -> serde_json::Result<Encoded> {
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
    Ok(Encoded {
        bytes: serializer.into_inner().bytes,
        objects,
    })
}

/// Returns the serialization buffer itself when every object is already in
/// canonical order; the reorder copy produces the same bytes, so only a
/// disordered document pays for a second buffer.
fn finalize(mut encoded: Encoded) -> Vec<u8> {
    #[cfg(test)]
    FINALIZATIONS.with(|count| count.set(count.get() + 1));
    if !sort_all_fields(&encoded.bytes, &mut encoded.objects) {
        return encoded.bytes;
    }
    let mut out = Vec::with_capacity(encoded.bytes.len());
    copy_sorted_range(
        &encoded.bytes,
        &encoded.objects,
        0..encoded.bytes.len(),
        &mut out,
    );
    out
}

/// `|=` never short-circuits, so a disordered object late in the document is
/// sorted and reported even when every earlier object is already ordered.
fn sort_all_fields(bytes: &[u8], objects: &mut [ObjectSpans]) -> bool {
    let mut changed = false;
    for object in objects {
        changed |= sort_fields(bytes, &mut object.fields);
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
        // Each triple is listed in decoded order: raw ASCII, escape-only, prefix and
        // quote, and mixed Unicode.
        let triples: [[&str; 3]; 4] = [
            ["a", "b", "c"],
            ["\n", "a", "\u{e9}"],
            ["a", "a b", "a\""],
            ["\u{1}", "!", "\u{1F600}"],
        ];
        for sorted_keys in triples {
            for permutation in permutations(&sorted_keys) {
                let pairs: Vec<(&str, serde_json::Value)> = permutation
                    .iter()
                    .enumerate()
                    .map(|(index, key)| (*key, serde_json::json!(index)))
                    .collect();
                let source = Ordered(&pairs);
                let expected = reference(&source);
                let mut encoded = serialize_with_spans(&source).unwrap();
                let changed = sort_all_fields(&encoded.bytes, &mut encoded.objects);
                let disordered = permutation != sorted_keys;
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
            let mut reversed: Vec<(&str, serde_json::Value)> = map
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect();
            reversed.reverse();
            for source in [Ordered(&reversed)] {
                let encoded = serialize_with_spans(&source).unwrap();
                let mut whole = Vec::new();
                copy_sorted_range(
                    &encoded.bytes,
                    &encoded.objects,
                    0..encoded.bytes.len(),
                    &mut whole,
                );
                assert_eq!(whole, encoded.bytes, "{case}");
                for object in &encoded.objects {
                    let mut part = Vec::new();
                    copy_sorted_range(
                        &encoded.bytes,
                        &encoded.objects,
                        object.bytes.clone(),
                        &mut part,
                    );
                    assert_eq!(part, encoded.bytes[object.bytes.clone()], "{case}");
                    for (field, _) in &object.fields {
                        let mut part = Vec::new();
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
                outer.serialize_key("a")?;
                let mut inner = serde_json::Map::new();
                inner.insert("b".into(), serde_json::json!(1));
                outer.serialize_value(&inner)?;
                outer.serialize_key("open")?;
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
}
