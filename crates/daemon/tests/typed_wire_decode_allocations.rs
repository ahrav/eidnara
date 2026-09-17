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

const REQUEST_40: &[u8] =
    include_bytes!("fixtures/typed-wire-decode/decode-40msgs_2KiB_mixed.json");
const REQUEST_200: &[u8] =
    include_bytes!("fixtures/typed-wire-decode/decode-200msgs_2KiB_mixed.json");

const MAX_EVENTS_PER_MESSAGE: usize = 16;
const MESSAGE_COUNT: usize = 40;
const MAX_EVENTS: usize = MAX_EVENTS_PER_MESSAGE * MESSAGE_COUNT;
const PEAK_MULTIPLE: usize = 3;

/// The scratch pool request decodes are charged against: the public resident floor less one maximum body and one egress frame.
const DECLARED_SCRATCH_POOL: usize =
    (MIN_RESIDENT_BYTES - 2 * MAX_BODY_LEN as u64 - HEADER_LEN as u64) as usize;

/// The exact source bytes of the request's `messages` array.
fn messages_array_bytes(body: &[u8]) -> &[u8] {
    #[derive(serde::Deserialize)]
    struct Envelope<'a> {
        #[serde(borrow)]
        messages: &'a serde_json::value::RawValue,
    }
    let envelope: Envelope<'_> = serde_json::from_slice(body).expect("request envelope");
    envelope.messages.get().as_bytes()
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

/// The struct bodies are compared to their exact field sets, so an envelope restored under any name or type alias fails here; the crate-wide scan catches callers of the removed entries by name.
#[test]
fn source_has_no_envelope_tree_or_replay_entry() {
    let wire = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../memory-store/src/lib.rs"
    ))
    .expect("memory-store source");
    let field_lines = |name: &str| -> Vec<String> {
        let start = wire
            .find(&format!("pub struct {name} {{"))
            .unwrap_or_else(|| panic!("{name} is defined"));
        let end = start + wire[start..].find("\n}\n").expect("struct end");
        wire[start..end]
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.starts_with("///") && !line.starts_with("#["))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(
        field_lines("WireMessage"),
        [
            "pub role: String,",
            "content: Vec<WireBlock>,",
            "pub origin: Option<MessageOrigin>,",
            "pub provider_extras: ProviderExtras,",
            "pub meta: HarnessMeta,",
        ]
    );
    assert_eq!(
        field_lines("WireBlock"),
        ["kind: BlockKind,", "pub provider_extras: ProviderExtras,"]
    );
    for handwritten in [
        "impl<'de> Deserialize<'de> for WireMessage",
        "impl Serialize for WireMessage",
        "impl<'de> Deserialize<'de> for WireBlock",
        "impl Serialize for WireBlock",
    ] {
        assert!(!wire.contains(handwritten), "{handwritten} exists");
    }
    let crates = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    // Assembled so this file does not itself match the search.
    let needles = [
        concat!("original", "()"),
        concat!("mark_", "modified"),
        concat!("mark_fully_", "typed"),
        concat!("WireMessage", "Data"),
        concat!("WireBlock", "Data"),
    ];
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
            } else if path.extension().is_some_and(|ext| ext == "rs") {
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
        "replay references remain (a comment or string literal naming one is also a hit):\n{}",
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

/// Per request the decode-plus-projection peak is the decoded text once, the canonical block string's growth buffer at up to twice the text, and its `Arc<str>` copy: four times the decode charge at the text ceiling.
const DECODE_PROJECTION_PEAK_MULTIPLE: usize = 4;
/// Served output over the live request and projection adds the canonicalizer's growth buffer, its exact-size reorder copy, and the `Arc<[u8]>` copy: seven times the decode charge at the text ceiling, above the declared pool for one request.
const SERVED_OWNER_SET_PEAK_MULTIPLE: usize = 7;

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
        let charged = meter.needed();
        assert!(
            ledger.peak_live_bytes <= DECLARED_SCRATCH_POOL,
            "{label}: decode and projection peaked at {} bytes against a declared pool of \
             {DECLARED_SCRATCH_POOL}",
            ledger.peak_live_bytes
        );
        assert!(
            ledger.peak_live_bytes <= DECODE_PROJECTION_PEAK_MULTIPLE * charged,
            "{label}: decode and projection peaked at {} bytes, above {DECODE_PROJECTION_PEAK_MULTIPLE} times the decode charge of {charged}",
            ledger.peak_live_bytes
        );
        let (request, projection) = owners;
        assert!(
            projection.blocks.iter().all(|block| {
                request.messages.iter().any(|message| {
                    message
                        .ck
                        .content()
                        .get(block.block_index)
                        .is_some_and(|shell| std::ptr::eq(block.wire.as_ref(), shell))
                })
            }),
            "{label}: projection blocks share the request's shells"
        );
        eprintln!(
            "{label}: decode+projection peak {} bytes, live at handoff {}, decode charge {charged}, body {} bytes, pool {DECLARED_SCRATCH_POOL}",
            ledger.peak_live_bytes,
            ledger.live_bytes_at_close,
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
        let owner_set_peak =
            ledger.live_bytes_at_close.max(0) as usize + served_ledger.peak_live_bytes;
        assert!(
            owner_set_peak <= SERVED_OWNER_SET_PEAK_MULTIPLE * charged,
            "{label}: request, projection, and served output peaked at {owner_set_peak} bytes, above {SERVED_OWNER_SET_PEAK_MULTIPLE} times the decode charge of {charged}"
        );
        eprintln!(
            "{label}: request, projection, and served output peak {owner_set_peak} bytes ({} retained by served output), pool {DECLARED_SCRATCH_POOL}",
            served_ledger.live_bytes_at_close
        );
        drop((request, projection, served));
    }
}
