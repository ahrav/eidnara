//! Replays the checked-in fuzz corpus seeds through each strict decoder, so the decoders
//! keep accepting the valid seed and rejecting the malformed ones without a fuzzer run.

use std::fs;
use std::path::Path;

use shm_transport::backend::ring::RingGrant;
use shm_transport::descriptor::HardwareProfileId;
use shm_transport::harness;
use shm_transport::profile::ring_profile;

const EXPECTED_SEEDS: [&str; 5] = ["empty", "all-zero", "all-ff", "valid", "near-valid"];

fn replay(target: &str, decoder: fn(&[u8]) -> bool) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/corpus")
        .join(target);
    for seed in EXPECTED_SEEDS {
        assert!(
            dir.join(seed).is_file(),
            "corpus seed {target}/{seed} is missing"
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
        if path.file_name().is_some_and(|name| name == "valid") {
            assert!(accepted, "corpus seed {target}/valid must be accepted");
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
    replay("frame_descriptor", harness::frame_descriptor);
}

#[test]
fn provider_grant_corpus_replays_without_panic() {
    replay("provider_grant", harness::provider_grant);
}

#[test]
fn provider_sample_corpus_replays_without_panic() {
    replay("provider_sample", harness::provider_sample);
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
    let grant = RingGrant::decode_slice(&bytes).expect("golden grant fixture decodes");
    assert_eq!(
        grant.encode().as_slice(),
        bytes.as_slice(),
        "golden grant fixture must round-trip byte-exactly"
    );
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
