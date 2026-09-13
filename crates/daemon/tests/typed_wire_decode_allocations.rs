//! `record_window` measures allocations during `serde_json::from_slice::<IngressMessages>` on the extracted `messages` array bytes.
//! The window counts every allocation and reallocation the owning thread makes: wire shells, payload `Value`s, unescape scratch, and serde's tagged-enum buffering.
//! Fixture setup, other threads, the request envelope, native messages, and projection are outside the window; the other tests here measure them against their own denominators and never subtract one peak from another.
//!
//! The recorder is thread-owned, so harness threads never enter the ledger.

#![cfg(feature = "test-support")]

#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;

use alloc_recorder::record_window;
use daemon::metered_decode::{ResidentMeter, ResidentReserve, decode_metered, footprint_of};
use daemon::transform::TransformRequest;
use daemon::wire::{IngressMessages, project_messages};
use host_runtime::config::MIN_RESIDENT_BYTES;
use host_runtime::wire::{HEADER_LEN, MAX_BODY_LEN};
use memory_store::WireMessage;

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

const REQUEST_40: &[u8] = include_bytes!(
    "../../../docs/properties/typed-wire-decode/resources/evidence/eg1-decode-projection/before/corpus/decode-40msgs_2KiB_mixed.json"
);
const REQUEST_200: &[u8] = include_bytes!(
    "../../../docs/properties/typed-wire-decode/resources/evidence/eg1-decode-projection/before/corpus/decode-200msgs_2KiB_mixed.json"
);

const MAX_EVENTS_PER_MESSAGE: usize = 16;
const MESSAGE_COUNT: usize = 40;
const MAX_EVENTS: usize = MAX_EVENTS_PER_MESSAGE * MESSAGE_COUNT;
const PEAK_MULTIPLE: usize = 3;

/// The scratch pool request decodes are charged against: the public resident floor less one maximum body and one egress frame.
const DECLARED_SCRATCH_POOL: usize =
    (MIN_RESIDENT_BYTES - 2 * MAX_BODY_LEN as u64 - HEADER_LEN as u64) as usize;

/// Finds the first `"messages":` occurrence and returns its balanced JSON array slice, ignoring brackets inside strings.
fn messages_array_bytes(body: &[u8]) -> &[u8] {
    let key = b"\"messages\":";
    let start = body
        .windows(key.len())
        .position(|window| window == key)
        .expect("the request carries a messages array")
        + key.len();
    assert_eq!(body[start], b'[');
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in body[start..].iter().enumerate() {
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
            b'"' => in_string = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &body[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("the messages array is unterminated");
}

struct Unmetered;

impl ResidentReserve for Unmetered {
    fn try_reserve(&self, _bytes: usize) -> Option<host_runtime::wire::ByteCharge> {
        Some(host_runtime::wire::ByteCharge::none())
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}

#[test]
fn message_decode_stays_within_the_allocation_budget() {
    let messages_json = messages_array_bytes(REQUEST_40);
    let (messages, ledger) =
        record_window(|| serde_json::from_slice::<IngressMessages>(messages_json).expect("decode"));
    assert!(!ledger.overflow, "ledger overflow");
    assert_eq!(messages.len(), MESSAGE_COUNT);
    let budget = PEAK_MULTIPLE * messages_json.len();
    assert!(
        ledger.allocation_events <= MAX_EVENTS,
        "{} allocation events for {MESSAGE_COUNT} messages; the budget is {MAX_EVENTS}",
        ledger.allocation_events
    );
    assert!(
        ledger.peak_live_bytes < budget,
        "peak {} bytes for {} messages JSON bytes; the budget is under {budget}",
        ledger.peak_live_bytes,
        messages_json.len()
    );
    let request: TransformRequest = serde_json::from_slice(REQUEST_40).expect("request decode");
    assert_eq!(
        *messages, *request.messages,
        "the gated value equals the production request's messages"
    );
    eprintln!(
        "messages gate: {} events, peak {} bytes, messages JSON {} bytes",
        ledger.allocation_events,
        ledger.peak_live_bytes,
        messages_json.len()
    );
    drop(messages);
}

#[test]
fn a_restored_envelope_tree_fails_the_gate() {
    let messages_json = messages_array_bytes(REQUEST_40);
    let (messages, ledger) = record_window(|| {
        let envelopes: Vec<serde_json::Value> =
            serde_json::from_slice(messages_json).expect("tree decode");
        let typed: Vec<daemon::wire::IngressMessage> = envelopes
            .iter()
            .map(|envelope| serde_json::from_value(envelope.clone()).expect("typed"))
            .collect();
        (envelopes, typed)
    });
    assert!(!ledger.overflow, "ledger overflow");
    assert_eq!(messages.1.len(), MESSAGE_COUNT);
    assert!(
        ledger.allocation_events > MAX_EVENTS,
        "a retained envelope per message allocated only {} events",
        ledger.allocation_events
    );
    assert!(
        ledger.peak_live_bytes >= PEAK_MULTIPLE * messages_json.len(),
        "a retained envelope per message peaked at only {} bytes",
        ledger.peak_live_bytes
    );
    eprintln!(
        "restored envelope control: {} events, peak {} bytes",
        ledger.allocation_events, ledger.peak_live_bytes
    );
    drop(messages);
}

#[test]
fn source_has_no_envelope_tree_or_replay_entry() {
    let wire = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../memory-store/src/lib.rs"
    ))
    .expect("memory-store source");
    for name in ["WireMessage", "WireBlock"] {
        let start = wire
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("{name} is defined"));
        let end = start + wire[start..].find("\n}\n").expect("struct end");
        let definition = &wire[start..end];
        assert!(
            !definition.contains("Value"),
            "{name} holds a Value field:\n{definition}"
        );
        let attributes = &wire[..start];
        let derive = attributes.rfind("#[derive(").expect("derive attribute");
        assert!(
            attributes[derive..].contains("Serialize")
                && attributes[derive..].contains("Deserialize"),
            "{name} derives its serde implementations"
        );
        for handwritten in [
            format!("impl<'de> Deserialize<'de> for {name}"),
            format!("impl Serialize for {name}"),
            format!("struct {name}Data"),
        ] {
            assert!(!wire.contains(&handwritten), "{handwritten} exists");
        }
    }
    let crates = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let needles = [
        "original()",
        "mark_modified",
        "mark_fully_typed",
        "WireMessageData",
        "WireBlockData",
    ];
    let this_file = std::path::Path::new(file!())
        .file_name()
        .expect("file name")
        .to_owned();
    let mut hits = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(crates)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("crate directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && path.file_name() != Some(this_file.as_os_str())
            {
                let source = std::fs::read_to_string(&path).expect("source file");
                for (line_number, line) in source.lines().enumerate() {
                    if needles.iter().any(|needle| line.contains(needle)) {
                        hits.push(format!("{}:{}: {line}", path.display(), line_number + 1));
                    }
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "dead replay references remain:\n{}",
        hits.join("\n")
    );
}

#[test]
fn whole_request_decode_fits_its_resident_charge() {
    for (label, body) in [("40 messages", REQUEST_40), ("200 messages", REQUEST_200)] {
        let meter = ResidentMeter::new(&Unmetered);
        meter
            .reserve_unescape_scratch(body)
            .expect("the scratch charge is granted");
        let (request, ledger) = record_window(|| {
            decode_metered::<TransformRequest>(body, &meter).expect("direct decode")
        });
        assert!(!ledger.overflow, "{label}: ledger overflow");
        assert!(
            ledger.peak_live_bytes <= meter.needed(),
            "{label}: the direct decode peaked at {} bytes but the meter charged {}",
            ledger.peak_live_bytes,
            meter.needed()
        );
        assert!(meter.needed() >= footprint_of(body), "{label}");
        eprintln!(
            "{label}: whole-request direct decode {} events, peak {} bytes, body {} bytes, charged {}",
            ledger.allocation_events,
            ledger.peak_live_bytes,
            body.len(),
            meter.needed()
        );
        drop(request);
    }
}

fn near_cap_text_request() -> Vec<u8> {
    let mut body = Vec::from(
        br#"{"kind":"transform","session_id":"s","render_config":"r","messages":[{"mid":"m","ordinal":0,"ck":{"role":"user","content":[{"kind":{"type":"text","text":""#
            .as_slice(),
    );
    body.resize(body.len() + (31 << 20), b'x');
    body.extend_from_slice(br#""}}]}}]}"#);
    assert!(body.len() <= 32 << 20);
    body
}

#[test]
fn decode_and_projection_fit_the_declared_pool() {
    let near_cap = near_cap_text_request();
    for (label, body) in [
        ("40 messages", REQUEST_40),
        ("200 messages", REQUEST_200),
        ("near-cap text", near_cap.as_slice()),
    ] {
        let meter = ResidentMeter::new(&Unmetered);
        meter
            .reserve_unescape_scratch(body)
            .expect("the scratch charge is granted");
        let (owners, ledger) = record_window(|| {
            let request = decode_metered::<TransformRequest>(body, &meter).expect("direct decode");
            let projection = project_messages(&request.messages).expect("projection");
            (request, projection)
        });
        assert!(!ledger.overflow, "{label}: ledger overflow");
        assert!(
            ledger.peak_live_bytes <= DECLARED_SCRATCH_POOL,
            "{label}: decode and projection peaked at {} bytes against a declared pool of \
             {DECLARED_SCRATCH_POOL}",
            ledger.peak_live_bytes
        );
        let (request, projection) = owners;
        assert!(
            projection.blocks.iter().all(|block| {
                request.messages.iter().any(|message| {
                    std::ptr::eq(
                        block.wire.as_ref(),
                        &message.ck.content()[block.block_index],
                    )
                })
            }),
            "{label}: projection blocks share the request's shells"
        );
        eprintln!(
            "{label}: decode+projection peak {} bytes, live at handoff {}, decode charge {}, body {} bytes, pool {DECLARED_SCRATCH_POOL}",
            ledger.peak_live_bytes,
            ledger.live_bytes_at_close,
            meter.needed(),
            body.len()
        );

        let (served, served_ledger) = record_window(|| {
            request
                .messages
                .iter()
                .map(|message| {
                    let ck: &WireMessage = &message.ck;
                    daemon::transform::served_message_for_test(ck.clone())
                })
                .collect::<Vec<_>>()
        });
        assert!(!served_ledger.overflow, "{label}: ledger overflow");
        eprintln!(
            "{label}: served output over the live request and projection: peak {} more bytes, {} retained",
            served_ledger.peak_live_bytes, served_ledger.live_bytes_at_close
        );
        drop((request, projection, served));
    }
}

#[test]
fn the_frozen_corpora_are_the_recorded_bodies() {
    use sha2::{Digest, Sha256};
    for (body, expected) in [
        (
            REQUEST_40,
            "928e93739612f0033db4fe6d68afda1ebe458383a2b75b6257f6e3e2320b2acf",
        ),
        (
            REQUEST_200,
            "f45d91797f9f97807a06e4466fd4d09bbfb03a0edbd4394cc9590375fb07e8d1",
        ),
    ] {
        let digest = Sha256::digest(body);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, expected);
    }
}
