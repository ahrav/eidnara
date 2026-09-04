//! Replays the checked-in fuzz corpus seeds through each strict decoder, so the decoders
//! keep accepting the valid seed and rejecting the malformed ones without a fuzzer run.
#![deny(clippy::undocumented_unsafe_blocks)]

use std::fs;
use std::path::Path;

use shm_transport::backend::ring::RingGrant;
use shm_transport::descriptor::HardwareProfileId;
use shm_transport::harness;
use shm_transport::profile::ring_profile;

const EXPECTED_SEEDS: [&str; 5] = ["empty", "all-zero", "all-ff", "valid", "near-valid"];
const REJECTED_SEEDS: [&str; 4] = ["empty", "all-zero", "all-ff", "near-valid"];

/// The grant fixtures freeze a `total_bytes` computed with 4 KiB pages; `RingGrant::decode`
/// recomputes the layout with the host page size and rejects the fixture on any other size.
const FIXTURE_PAGE_SIZE: i64 = 4096;

fn host_page_size() -> i64 {
    // SAFETY: sysconf reads a process-wide constant and has no preconditions.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) }
}

fn fixture_page_size_matches_host() -> bool {
    host_page_size() == FIXTURE_PAGE_SIZE
}

fn replay(target: &str, decoder: fn(&[u8]) -> bool, valid_seed_is_page_size_dependent: bool) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/corpus")
        .join(target);
    for seed in EXPECTED_SEEDS {
        assert!(
            dir.join(seed).is_file(),
            "corpus seed {target}/{seed} is missing"
        );
    }
    let assert_valid_accepted =
        !valid_seed_is_page_size_dependent || fixture_page_size_matches_host();
    if !assert_valid_accepted {
        eprintln!(
            "{target}/valid encodes a {FIXTURE_PAGE_SIZE}-byte-page layout; host pages are {} bytes, \
             so acceptance is not asserted",
            host_page_size()
        );
    }
    let mut replayed = 0usize;
    for entry in fs::read_dir(&dir).expect("corpus directory is readable") {
        let path = entry.expect("corpus entry is readable").path();
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).expect("corpus file is readable");
        let accepted = decoder(&bytes);
        let name = path.file_name().and_then(|name| name.to_str());
        if name == Some("valid") && assert_valid_accepted {
            assert!(accepted, "corpus seed {target}/valid must be accepted");
        }
        if name.is_some_and(|name| REJECTED_SEEDS.contains(&name)) {
            assert!(
                !accepted,
                "corpus seed {target}/{} must be rejected",
                name.unwrap_or_default()
            );
        }
        replayed += 1;
    }
    assert!(
        replayed >= EXPECTED_SEEDS.len(),
        "corpus for {target} lost seeds"
    );
}

#[test]
fn frame_descriptor_corpus_replays_without_panic() {
    replay("frame_descriptor", harness::frame_descriptor, false);
}

#[test]
fn provider_grant_corpus_replays_without_panic() {
    replay("provider_grant", harness::provider_grant, true);
}

#[test]
fn provider_sample_corpus_replays_without_panic() {
    replay("provider_sample", harness::provider_sample, false);
}

/// The `provider_grant/valid` seed is also the frozen encoding of the depth-32 ring profile,
/// so a change to grant layout or profile geometry shows up here as a fixture diff.
#[test]
fn golden_grant_fixture_matches_the_frozen_ring_profile_encoding() {
    const GOLDEN_GRANT_HEX: &str = "0300d489c07ee46333a5fe7901df356f6f460000000020000000000000\
                                    0000000004000000002000000000000000004000040000000000000000";
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus/provider_grant/valid");
    let bytes = std::fs::read(path).expect("golden grant fixture is readable");
    let text: String = bytes.iter().fold(String::new(), |mut text, byte| {
        use std::fmt::Write;
        write!(text, "{byte:02x}").unwrap();
        text
    });
    assert_eq!(
        text,
        GOLDEN_GRANT_HEX.replace(char::is_whitespace, ""),
        "the checked-in fixture bytes moved unexpectedly"
    );
    if fixture_page_size_matches_host() {
        let grant = RingGrant::decode_slice(&bytes).expect("golden grant fixture decodes");
        assert_eq!(
            grant.encode().as_slice(),
            bytes.as_slice(),
            "golden grant fixture must round-trip byte-exactly"
        );
    } else {
        eprintln!(
            "host pages are {} bytes; skipping the {FIXTURE_PAGE_SIZE}-byte-page decode check",
            host_page_size()
        );
    }
    let frozen = ring_profile(HardwareProfileId::new("ring-contract-host").unwrap()).unwrap();
    let field =
        |range: std::ops::Range<usize>| u64::from_le_bytes(bytes[range].try_into().unwrap());
    assert_eq!(
        u16::from_le_bytes([bytes[0], bytes[1]]),
        3,
        "layout version"
    );
    assert_eq!(
        &bytes[2..18],
        &[
            0xd4, 0x89, 0xc0, 0x7e, 0xe4, 0x63, 0x33, 0xa5, 0xfe, 0x79, 0x01, 0xdf, 0x35, 0x6f,
            0x6f, 0x46
        ],
        "incarnation identity"
    );
    assert_eq!(
        u32::from_le_bytes(bytes[18..22].try_into().unwrap()),
        0,
        "host-to-peer lane"
    );
    assert_eq!(field(22..30), frozen.descriptor_depth() as u64);
    assert_eq!(field(30..38), frozen.arena_bytes() as u64);
    assert_eq!(field(38..46), frozen.max_leases() as u64);
}
