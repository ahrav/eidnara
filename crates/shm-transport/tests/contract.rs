//! Contract tests for the strict decoders and the close-state machine: descriptor validation,
//! completion validation, grant identity, profile identifiers, redaction, and lifecycle edges.
#![deny(clippy::undocumented_unsafe_blocks)]

use shm_transport::backend::ring::{PoolGrant, Ring, RingError};
use shm_transport::descriptor::{
    CompletionRecord, DESCRIPTOR_SCHEMA_VERSION, DescriptorError, HardwareProfileId, Incarnation,
    PayloadIdentity, PoolDescriptor, TransportDescriptor, WIRE_V3_HEADER_BYTES, WIRE_V3_VERSION,
    check_wire_header,
};
use shm_transport::lifecycle::{CloseState, Lifecycle, LifecycleError};
use shm_transport::pool::{BlockClass, ClassSpec, Inventory, MAX_FRAME_BYTES, PoolGeometry};
use shm_transport::profile::{host_payload_pool_profile, pool_profile};

fn header(body_len: usize) -> [u8; WIRE_V3_HEADER_BYTES] {
    let mut header = [0u8; WIRE_V3_HEADER_BYTES];
    header[..4].copy_from_slice(&(body_len as u32).to_le_bytes());
    header[4] = WIRE_V3_VERSION;
    header
}

fn geometry() -> PoolGeometry {
    PoolGeometry::host_payload_pool()
}

#[test]
fn descriptor_rejects_every_untrusted_identity_block_and_length_failure() {
    let geometry = geometry();
    let valid = PoolDescriptor::from_untrusted(7, 3, 2, 100);
    let validated = valid.validate(7, &geometry).unwrap();
    assert_eq!(validated.sequence(), 7);
    assert_eq!(validated.block(), 3);
    assert_eq!(validated.generation(), 2);
    assert_eq!(validated.body_len(), 100);
    assert_eq!(validated.placement().class, BlockClass::Ordinary(0));

    let cases: [(PoolDescriptor, u64, DescriptorError); 9] = [
        (
            PoolDescriptor::from_untrusted(0, 3, 2, 100),
            0,
            DescriptorError::InvalidSequence,
        ),
        (
            PoolDescriptor::from_untrusted(8, 3, 2, 100),
            7,
            DescriptorError::InvalidSequence,
        ),
        (
            PoolDescriptor::from_untrusted(7, u64::from(geometry.block_count()), 2, 100),
            7,
            DescriptorError::InvalidBlock,
        ),
        (
            PoolDescriptor::from_untrusted(7, u64::MAX, 2, 100),
            7,
            DescriptorError::InvalidBlock,
        ),
        (
            PoolDescriptor::from_untrusted(7, 3, 0, 100),
            7,
            DescriptorError::InvalidGeneration,
        ),
        (
            PoolDescriptor::from_untrusted(7, 3, 2, MAX_FRAME_BYTES as u64 + 1),
            7,
            DescriptorError::FrameTooLarge,
        ),
        (
            PoolDescriptor::from_untrusted(7, 3, 2, u64::MAX),
            7,
            DescriptorError::FrameTooLarge,
        ),
        (
            PoolDescriptor::from_untrusted(7, 3, 2, 4096 - 20),
            7,
            DescriptorError::BodyExceedsBlock,
        ),
        (
            PoolDescriptor::from_untrusted(7, 186, 2, 32 * 1024 - 20),
            7,
            DescriptorError::BodyExceedsBlock,
        ),
    ];
    for (descriptor, expected_sequence, error) in cases {
        assert_eq!(
            descriptor.validate(expected_sequence, &geometry).err(),
            Some(error)
        );
    }
    // The largest ordinary class admits exactly the protocol maximum and no more.
    let largest = geometry.first_block(BlockClass::Ordinary(4));
    assert!(
        PoolDescriptor::from_untrusted(1, u64::from(largest), 1, MAX_FRAME_BYTES as u64)
            .validate(1, &geometry)
            .is_ok()
    );
    assert_eq!(
        PoolDescriptor::from_untrusted(1, u64::from(largest), 1, MAX_FRAME_BYTES as u64 + 1)
            .validate(1, &geometry)
            .err(),
        Some(DescriptorError::FrameTooLarge)
    );
    // A zero-length body is legal on every class.
    for block in [
        0,
        geometry.first_block(BlockClass::Control),
        geometry.first_block(BlockClass::Terminal),
    ] {
        assert!(
            PoolDescriptor::from_untrusted(1, u64::from(block), 1, 0)
                .validate(1, &geometry)
                .is_ok()
        );
    }
}

#[test]
fn completion_rejects_stale_future_and_wrong_pool_returns() {
    let geometry = geometry();
    assert_eq!(
        CompletionRecord::from_untrusted(5, 3).validate(&geometry, Some(3)),
        Ok(5)
    );
    assert_eq!(
        CompletionRecord::from_untrusted(5, 2).validate(&geometry, Some(3)),
        Err(DescriptorError::StaleCompletion)
    );
    assert_eq!(
        CompletionRecord::from_untrusted(5, 4).validate(&geometry, Some(3)),
        Err(DescriptorError::FutureCompletion)
    );
    assert_eq!(
        CompletionRecord::from_untrusted(5, 3).validate(&geometry, None),
        Err(DescriptorError::FutureCompletion)
    );
    assert_eq!(
        CompletionRecord::from_untrusted(5, 0).validate(&geometry, Some(3)),
        Err(DescriptorError::InvalidGeneration)
    );
    assert_eq!(
        CompletionRecord::from_untrusted(u64::from(geometry.block_count()), 3)
            .validate(&geometry, Some(3)),
        Err(DescriptorError::InvalidBlock)
    );
    assert_eq!(
        CompletionRecord::from_untrusted(u64::MAX, 3).validate(&geometry, Some(3)),
        Err(DescriptorError::InvalidBlock)
    );
}

#[test]
fn wire_header_check_is_shared_by_producer_and_consumer() {
    assert!(check_wire_header(&header(10), 10).is_ok());
    assert_eq!(
        check_wire_header(&header(10), 11),
        Err(DescriptorError::WireHeaderMismatch)
    );
    let mut wrong_version = header(0);
    wrong_version[4] = WIRE_V3_VERSION + 1;
    assert_eq!(
        check_wire_header(&wrong_version, 0),
        Err(DescriptorError::WireHeaderMismatch)
    );
}

#[test]
fn sole_identifiers_are_four_and_the_pool_profile() {
    assert_eq!(DESCRIPTOR_SCHEMA_VERSION, 4);
    assert_eq!(shm_transport::backend::retained::LAYOUT_VERSION, 4);
    let profile = host_payload_pool_profile().unwrap();
    assert!(
        profile
            .descriptor()
            .hardware_matches("host-payload-pool-v1")
    );
    assert!(!profile.descriptor().hardware_matches("host-test-ring-v1"));
    let ring = Ring::create(&profile, 0).unwrap();
    let bytes = ring.grant().encode();
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 4);
    // The one supported layout version: any other value fails before a mapping exists.
    let mut old = bytes;
    old[0..2].copy_from_slice(&3u16.to_le_bytes());
    assert_eq!(PoolGrant::decode(old), Err(RingError::InvalidGrant));
}

#[test]
fn class_boundaries_are_full_frame_capacity_minus_the_header() {
    let geometry = geometry();
    let boundaries: Vec<u64> = geometry.classes()[..5]
        .iter()
        .map(|class| class.body_capacity())
        .collect();
    assert_eq!(
        boundaries,
        [
            4096 - 21,
            64 * 1024 - 21,
            1024 * 1024 - 21,
            8 * 1024 * 1024 - 21,
            64 * 1024 * 1024 + 4096 - 21,
        ]
    );
    for (index, boundary) in boundaries.iter().enumerate().take(4) {
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, *boundary),
            Some(BlockClass::Ordinary(index as u8))
        );
        assert_eq!(
            geometry.class_for(Inventory::Ordinary, boundary + 1),
            Some(BlockClass::Ordinary(index as u8 + 1))
        );
    }
    assert_eq!(
        geometry.class(BlockClass::Terminal).body_capacity(),
        32 * 1024 - 21
    );
    assert!(geometry.class(BlockClass::Terminal).body_capacity() >= 25_406);
    assert!(geometry.class(BlockClass::Terminal).block_bytes >= 25_427);
    assert_eq!(
        geometry.class(BlockClass::Control).body_capacity(),
        4096 - 21
    );
}

#[test]
fn hardware_profile_id_deserialization_enforces_constructor_rules() {
    let valid = HardwareProfileId::new("gpu-a100.v2_x").unwrap();
    let encoded = serde_json::to_string(&valid).unwrap();
    assert_eq!(encoded, "\"gpu-a100.v2_x\"");
    let decoded: HardwareProfileId = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, valid);

    let too_long = format!("\"{}\"", "a".repeat(65));
    for rejected in [
        "\"\"",
        "\"has space\"",
        "\"ünïcode\"",
        "\"slash/id\"",
        too_long.as_str(),
    ] {
        assert!(
            serde_json::from_str::<HardwareProfileId>(rejected).is_err(),
            "{rejected} deserialized"
        );
    }
}

#[test]
fn lifecycle_accepts_only_diagram_edges_and_quarantine_is_terminal() {
    assert_eq!(
        Lifecycle::new().advance(CloseState::ReleasingSamples),
        Err(LifecycleError::InvalidTransition)
    );

    let mut skipped_revoke = Lifecycle::new();
    skipped_revoke.advance(CloseState::Quiescing).unwrap();
    assert_eq!(
        skipped_revoke.advance(CloseState::RevokingJsOnEnv),
        Err(LifecycleError::InvalidTransition)
    );

    let mut late_quarantine = Lifecycle::new();
    for state in [
        CloseState::Quiescing,
        CloseState::DrainingPublished,
        CloseState::StoppingEnvScheduling,
        CloseState::RevokingJsOnEnv,
        CloseState::AsyncCleanupJoin,
    ] {
        late_quarantine.advance(state).unwrap();
    }
    assert_eq!(
        late_quarantine.advance(CloseState::Quarantined),
        Err(LifecycleError::InvalidTransition)
    );

    let mut lifecycle = Lifecycle::new();
    lifecycle.mark_prepared().unwrap();
    assert!(lifecycle.must_fail_closed());
    for state in [
        CloseState::Quiescing,
        CloseState::DrainingPublished,
        CloseState::StoppingEnvScheduling,
        CloseState::RevokingJsOnEnv,
        CloseState::AsyncCleanupJoin,
        CloseState::AwaitingRustScopes,
        CloseState::ReleasingSamples,
        CloseState::DroppingTransport,
        CloseState::Joined,
    ] {
        lifecycle.advance(state).unwrap();
    }
    assert!(lifecycle.reusable());
    assert_eq!(
        lifecycle.advance(CloseState::Open),
        Err(LifecycleError::Terminal)
    );

    let mut quarantined = Lifecycle::new();
    for state in [
        CloseState::Quiescing,
        CloseState::DrainingPublished,
        CloseState::StoppingEnvScheduling,
        CloseState::RevokingJsOnEnv,
        CloseState::Quarantined,
    ] {
        quarantined.advance(state).unwrap();
    }
    assert!(!quarantined.reusable());
    assert_eq!(
        quarantined.advance(CloseState::Joined),
        Err(LifecycleError::Terminal)
    );
}

#[test]
fn debug_and_errors_redact_every_sentinel() {
    let sentinel = "SENTINEL_descriptor_token_object_incarnation_address";
    let transport = TransportDescriptor::new(HardwareProfileId::new(sentinel).unwrap());
    let incarnation = Incarnation::from_bytes(*b"SENTINEL-SECRET!");
    let identity = PayloadIdentity::new(incarnation, 0x5345_4e54, 0x494e, 0x454c);
    let descriptor = PoolDescriptor::from_untrusted(0x5345, 0x4e54, 0x494e, 0x454c);
    let completion = CompletionRecord::from_untrusted(0x5345, 0x4e54);
    let profile = pool_profile(
        HardwareProfileId::new(sentinel).unwrap(),
        PoolGeometry::new(
            1,
            1,
            [
                ClassSpec::new(4096, 1),
                ClassSpec::new(8192, 1),
                ClassSpec::new(16384, 1),
                ClassSpec::new(32768, 1),
                ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
            ],
            ClassSpec::new(4096, 1),
            ClassSpec::new(32768, 1),
        )
        .unwrap(),
    )
    .unwrap();
    let ring = Ring::create(&profile, 1).unwrap();
    for formatted in [
        format!("{transport:?}"),
        format!("{incarnation:?}"),
        format!("{identity:?}"),
        format!("{descriptor:?}"),
        format!("{completion:?}"),
        format!("{:?}", DescriptorError::WrongIncarnation),
        format!("{ring:?}"),
        format!("{:?}", ring.grant()),
        format!("{:?}", ring.retained()),
        format!("{:?}", ring.attachment().unwrap()),
    ] {
        assert!(!formatted.contains("SENTINEL"), "{formatted}");
        assert!(!formatted.contains(sentinel));
        assert!(!formatted.contains("0x"));
    }
}

/// The protocol document's identifier and geometry tables are wire literals; this test reads
/// them so the document is a test input and a drift between prose and code fails here.
#[test]
fn payload_pool_protocol_document_agrees_with_the_implementation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/payload-pool-protocol.md");
    let doc = std::fs::read_to_string(&path).expect("read docs/payload-pool-protocol.md");
    let row = |needle: &str| {
        assert!(doc.contains(needle), "protocol document lacks `{needle}`");
    };
    row(&format!(
        "| Descriptor schema | `{DESCRIPTOR_SCHEMA_VERSION}` |"
    ));
    row(&format!(
        "| Mapping layout version | `{}` |",
        shm_transport::backend::retained::LAYOUT_VERSION
    ));
    row(&format!(
        "| Hardware profile | `{}` |",
        shm_transport::profile::HOST_PAYLOAD_POOL_PROFILE
    ));
    let geometry = PoolGeometry::host_payload_pool();
    let inventories = [
        "Ordinary", "Ordinary", "Ordinary", "Ordinary", "Ordinary", "Control", "Terminal",
    ];
    for (index, (class, inventory)) in geometry.classes().iter().zip(inventories).enumerate() {
        let bytes = class.block_bytes;
        let formatted = if bytes == 64 * 1024 * 1024 + 4096 {
            "67,112,960 (64 MiB + 4 KiB)".to_owned()
        } else {
            let digits = bytes.to_string();
            let mut grouped = String::new();
            for (position, digit) in digits.chars().enumerate() {
                if position != 0 && (digits.len() - position).is_multiple_of(3) {
                    grouped.push(',');
                }
                grouped.push(digit);
            }
            grouped
        };
        row(&format!(
            "| {inventory} | {index} | {formatted} | {} |",
            class.count
        ));
    }
    row(&format!(
        "Descriptor slots per direction: {} ordinary plus {} reserved, a queue depth\nof {}.",
        geometry.ordinary_descriptors(),
        geometry.reserved_descriptors(),
        geometry.descriptor_depth()
    ));
    row(&format!(
        "`PoolGrant` is {} little-endian bytes",
        PoolGrant::encoded_len()
    ));
    row("Block backing per direction is 95,817,728 bytes");
    assert_eq!(geometry.arena_bytes().unwrap(), 95_817_728);
    row("Total at 4 KiB pages: 95,825,920 bytes per direction.");
    assert_eq!(
        shm_transport::pool::MappingLayout::new(&geometry, 4096)
            .unwrap()
            .total,
        95_825_920
    );
    row("magic `0x4d43_5348_4d50_3034`");
}
