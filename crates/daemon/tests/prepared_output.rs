use std::cell::Cell;
use std::io::{self, Write};
use std::sync::Arc;

use daemon::dispatch::{
    MAX_WIRE_BODY_BYTES, PreparedOutput, PreparedOutputError, PreparedSegment, RecipeSegment,
};
use serde_json::json;

fn reserved_vec(output: &PreparedOutput) -> Result<Vec<u8>, PreparedOutputError> {
    let measured = output.measure()?;
    let mut destination = Vec::with_capacity(measured.len());
    measured.write_to(&mut destination)?;
    Ok(destination)
}

#[test]
fn json_measurement_matches_small_and_facade_sized_bytes() {
    for value in [
        json!({"ok": true, "items": [1, 2, 3]}),
        json!({
            "content": [{"type": "text", "text": "x".repeat(900 * 1024)}],
            "isError": false,
        }),
    ] {
        let expected = serde_json::to_vec(&value).unwrap();
        let output = PreparedOutput::json(value);
        let measured = output.measure().unwrap();
        assert_eq!(measured.len(), expected.len());
        assert_eq!(reserved_vec(&output).unwrap(), expected);
    }
}

#[test]
fn recipe_segments_write_keeps_and_inserts_as_prepared_bytes() {
    let operations = vec![
        RecipeSegment::Keep(PreparedSegment::exact(Arc::from(
            br#"{"op":"keep","source":"input","start":0,"count":1}"#.as_slice(),
        ))),
        RecipeSegment::Insert(vec![
            PreparedSegment::exact(Arc::from(
                br#"{"mid":"m1","content":[{"kind":{"text":"hello"}}]}"#.as_slice(),
            )),
            PreparedSegment::exact(Arc::from(
                br#"{"mid":"m2","content":[{"kind":{"text":"cached"}}]}"#.as_slice(),
            )),
        ]),
        RecipeSegment::Keep(PreparedSegment::exact(Arc::from(
            br#"{"op":"keep","source":"previous","start":2,"count":3}"#.as_slice(),
        ))),
    ];
    let output = PreparedOutput::transform_recipe(
        json!({"status": "ok", "operations": null, "cache_ttl": "1h"}),
        operations,
    )
    .unwrap();
    let expected = br#"{"cache_ttl":"1h","operations":[{"op":"keep","source":"input","start":0,"count":1},{"op":"insert","values":[{"mid":"m1","content":[{"kind":{"text":"hello"}}]},{"mid":"m2","content":[{"kind":{"text":"cached"}}]}]},{"op":"keep","source":"previous","start":2,"count":3}],"status":"ok"}"#;

    let measured = output.measure().unwrap();
    assert_eq!(measured.len(), expected.len());
    assert_eq!(reserved_vec(&output).unwrap(), expected);
    // Every operation is JSON the applier can parse.
    let parsed: serde_json::Value = serde_json::from_slice(expected).unwrap();
    assert_eq!(parsed["operations"].as_array().unwrap().len(), 3);
}

#[test]
fn recipe_insert_depth_agrees_for_encoded_and_value_segments() {
    for depth in [0, 123, 124] {
        let mut literal = json!("[brackets] {braces} \\\"quotes\\\" \\\\ backslash");
        for _ in 0..depth {
            literal = json!([literal]);
        }
        let bytes = serde_json::to_vec(&literal).unwrap();
        for segment in [
            PreparedSegment::exact(Arc::from(bytes.clone())),
            PreparedSegment::value(Arc::new(literal.clone()), bytes.len()),
        ] {
            let output = PreparedOutput::transform_recipe(
                json!({"base_revision": "b", "output_revision": "o", "operations": null}),
                vec![RecipeSegment::Insert(vec![segment])],
            );
            if depth <= 123 {
                let parsed: serde_json::Value =
                    serde_json::from_slice(&reserved_vec(&output.unwrap()).unwrap()).unwrap();
                daemon::edit_recipe::Recipe::from_json(&parsed).unwrap();
                assert_eq!(parsed["operations"][0]["values"][0], literal);
            } else {
                assert!(matches!(
                    output,
                    Err(PreparedOutputError::RecipeNestingTooDeep)
                ));
            }
        }
    }
}

#[test]
fn recipe_envelope_requires_the_operations_placeholder() {
    assert!(matches!(
        PreparedOutput::transform_recipe(json!({"status": "ok"}), Vec::new()),
        Err(PreparedOutputError::InvalidTransformEnvelope)
    ));
    assert!(matches!(
        PreparedOutput::transform_recipe(json!([]), Vec::new()),
        Err(PreparedOutputError::InvalidTransformEnvelope)
    ));
}

struct ReservationWriter<'a> {
    reserved: &'a Cell<bool>,
    writes: &'a Cell<usize>,
    bytes: Vec<u8>,
}

impl<'a> ReservationWriter<'a> {
    fn reserve(reserved: &'a Cell<bool>, writes: &'a Cell<usize>, capacity: usize) -> Self {
        reserved.set(true);
        Self {
            reserved,
            writes,
            bytes: Vec::with_capacity(capacity),
        }
    }
}

impl Write for ReservationWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(self.reserved.get(), "copy occurred before reservation");
        self.writes.set(self.writes.get() + 1);
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn cached_bytes_copy_only_after_destination_reservation() {
    let expected = br#"{"cached":true,"value":"stable"}"#.to_vec();
    let output = PreparedOutput::cached_bytes(expected.clone());
    let reserved = Cell::new(false);
    let writes = Cell::new(0);

    let measured = output.measure().unwrap();
    assert!(!reserved.get());
    assert_eq!(writes.get(), 0);

    let mut destination = ReservationWriter::reserve(&reserved, &writes, measured.len());
    measured.write_to(&mut destination).unwrap();
    assert!(writes.get() > 0);
    assert_eq!(destination.bytes, expected);
}

#[derive(Default)]
struct CountingSink {
    written: usize,
}

impl Write for CountingSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.written = self.written.checked_add(bytes.len()).unwrap();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn exactly_at_wire_cap_succeeds_without_destination_allocation() {
    let output = PreparedOutput::cached_bytes(vec![0x5a; MAX_WIRE_BODY_BYTES]);
    let measured = output.measure().unwrap();
    assert_eq!(measured.len(), MAX_WIRE_BODY_BYTES);

    let mut destination = CountingSink::default();
    assert_eq!(
        measured.write_to(&mut destination).unwrap(),
        MAX_WIRE_BODY_BYTES
    );
    assert_eq!(destination.written, MAX_WIRE_BODY_BYTES);
}

#[test]
fn cap_plus_one_and_arithmetic_overflow_fail_before_write() {
    let envelope = json!({"operations": null});
    let fixed_len = br#"{"operations":[]}"#.len();
    let cap_plus_one = PreparedOutput::transform_recipe(
        envelope.clone(),
        vec![RecipeSegment::Keep(PreparedSegment::inconsistent_for_test(
            Arc::from([]),
            MAX_WIRE_BODY_BYTES + 1 - fixed_len,
        ))],
    )
    .unwrap();
    assert!(matches!(
        cap_plus_one.measure(),
        Err(PreparedOutputError::BodyTooLarge {
            len,
            max: MAX_WIRE_BODY_BYTES
        }) if len == MAX_WIRE_BODY_BYTES + 1
    ));

    let overflow = PreparedOutput::transform_recipe(
        envelope,
        vec![RecipeSegment::Keep(PreparedSegment::inconsistent_for_test(
            Arc::from([]),
            usize::MAX,
        ))],
    )
    .unwrap();
    assert!(matches!(
        overflow.measure(),
        Err(PreparedOutputError::LengthOverflow)
    ));
}

struct FailAfter {
    remaining: usize,
    accepted: usize,
}

impl Write for FailAfter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("injected serializer failure"));
        }
        let written = bytes.len().min(self.remaining);
        self.remaining -= written;
        self.accepted += written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn destination_failure_retains_no_partial_terminal() {
    let output = PreparedOutput::json(json!({"value": "destination must fail after bytes"}));
    let measured = output.measure().unwrap();
    let mut destination = FailAfter {
        remaining: 5,
        accepted: 0,
    };
    let mut terminal = None;

    let result = measured.write_to(&mut destination);
    if result.is_ok() {
        terminal = Some(destination.accepted);
    }

    assert!(matches!(result, Err(PreparedOutputError::Write(_))));
    assert!(destination.accepted > 0);
    assert_eq!(terminal, None);
}

#[test]
fn inconsistent_source_reports_length_mismatch_without_emission() {
    let output = PreparedOutput::transform_recipe(
        json!({"operations": null}),
        vec![RecipeSegment::Keep(PreparedSegment::inconsistent_for_test(
            Arc::from(b"1".as_slice()),
            2,
        ))],
    )
    .unwrap();
    let measured = output.measure().unwrap();
    let mut destination = Vec::with_capacity(measured.len());
    let mut terminal = None;

    let result = measured.write_to(&mut destination);
    if result.is_ok() {
        terminal = Some(destination.clone());
    }

    let expected = br#"{"operations":[1]}"#;
    // The segment claims one more byte than it holds, so measured is the written envelope plus one.
    assert!(matches!(
        result,
        Err(PreparedOutputError::LengthMismatch { measured, written })
            if written == expected.len() && measured == expected.len() + 1
    ));
    assert_eq!(destination, expected);
    assert_eq!(terminal, None);
}
