use std::sync::Arc;

use shm_transport::descriptor::{
    DESCRIPTOR_SCHEMA_VERSION, HardwareProfileId, TransportDescriptor,
};
use shm_transport::profile::{
    AdmissionController, AdmissionError, HostLimits, ProfileConfig, ResourceCharges, TargetProfile,
    WorkerTopology, host_test_ring_profile, ring_profile,
};
#[test]
fn fixed_ring_identity_survives_profile_validation() {
    let profile = ring_profile(HardwareProfileId::new("fixed-ring-contract").unwrap()).unwrap();

    assert_eq!(
        profile.descriptor().schema_version(),
        DESCRIPTOR_SCHEMA_VERSION
    );
    assert!(profile.descriptor().hardware_matches("fixed-ring-contract"));
}

#[test]
fn debug_redacts_profile_admission_and_quarantine_record() {
    let sentinel = "SENTINEL_profile_id";
    let profile = ring_profile(HardwareProfileId::new(sentinel).unwrap()).unwrap();
    let controller = Arc::new(AdmissionController::new(HostLimits {
        descriptors: 1024,
        arena_bytes: 1 << 30,
        leases: 1024,
        mappings: 1024,
        file_descriptors: 1024,
        workers: 1024,
        client_instances: 1024,
        pinned_workers: 0,
    }));
    let admission = controller.admit(&profile, None).unwrap();
    let formatted_profile = format!("{profile:?}");
    let formatted_admission = format!("{admission:?}");
    let record = admission.quarantine().unwrap();
    let formatted_record = format!("{record:?}");

    assert_eq!(formatted_profile, "TargetProfile(<redacted>)");
    assert_eq!(formatted_admission, "Admission(<redacted>)");
    assert_eq!(formatted_record, "QuarantineRecord(<redacted>)");
    for formatted in [formatted_profile, formatted_admission, formatted_record] {
        assert!(!formatted.contains("SENTINEL"));
    }
}

#[test]
fn host_admission_retains_quarantined_commitments() {
    let profile = ring_profile(HardwareProfileId::new("contract-host").unwrap()).unwrap();
    let charges = profile.charges();
    let controller = Arc::new(AdmissionController::new(HostLimits {
        descriptors: charges.descriptors,
        arena_bytes: charges.arena_bytes,
        leases: charges.leases,
        mappings: charges.mappings,
        file_descriptors: charges.file_descriptors,
        workers: charges.workers,
        client_instances: charges.client_instances,
        pinned_workers: 0,
    }));
    let admission = controller.admit(&profile, None).unwrap();
    assert_eq!(controller.snapshot().unwrap().active, charges);
    let _quarantine = admission.quarantine().unwrap();
    assert_eq!(
        controller.snapshot().unwrap().quarantined,
        ResourceCharges {
            pinned_workers: 0,
            ..charges
        }
    );
    assert!(matches!(
        controller.admit(&profile, None),
        Err(AdmissionError::DescriptorLimit)
            | Err(AdmissionError::ArenaByteLimit)
            | Err(AdmissionError::LeaseLimit)
            | Err(AdmissionError::MappingLimit)
            | Err(AdmissionError::FileDescriptorLimit)
            | Err(AdmissionError::ClientInstanceLimit)
    ));
}

#[test]
fn exact_aggregate_capacity_admits_n_and_rejects_n_plus_one_without_charging() {
    // A nonzero worker charge makes `WorkerLimit` reachable in the rejection set.
    let profile = host_test_ring_profile().unwrap();
    let one = profile.charges();
    assert!(one.workers > 0);
    let count = 3;
    let controller = Arc::new(AdmissionController::new(HostLimits {
        descriptors: one.descriptors * count,
        arena_bytes: one.arena_bytes * count,
        leases: one.leases * count,
        mappings: one.mappings * count,
        file_descriptors: one.file_descriptors * count,
        workers: one.workers * count,
        client_instances: one.client_instances * count,
        pinned_workers: one.pinned_workers * count,
    }));
    let admissions: Vec<_> = (0..count)
        .map(|_| {
            controller
                .admit(&profile, None)
                .expect("capacity admission")
        })
        .collect();
    let full = controller.snapshot().unwrap();
    assert_eq!(full.active.client_instances, count);
    assert!(matches!(
        controller.admit(&profile, None),
        Err(AdmissionError::DescriptorLimit)
            | Err(AdmissionError::ArenaByteLimit)
            | Err(AdmissionError::LeaseLimit)
            | Err(AdmissionError::MappingLimit)
            | Err(AdmissionError::FileDescriptorLimit)
            | Err(AdmissionError::WorkerLimit)
            | Err(AdmissionError::ClientInstanceLimit)
    ));
    assert_eq!(controller.snapshot().unwrap(), full);
    drop(admissions);
    let reclaimed = controller.snapshot().unwrap();
    assert_eq!(reclaimed.active, ResourceCharges::ZERO);
    assert_eq!(reclaimed.quarantined, ResourceCharges::ZERO);
}

#[test]
fn worker_limit_is_the_only_limit_that_refuses_a_second_fused_admission() {
    let profile = host_test_ring_profile().unwrap();
    let one = profile.charges();
    // Every limit but `workers` has room for many admissions.
    let controller = Arc::new(AdmissionController::new(HostLimits {
        descriptors: one.descriptors * 16,
        arena_bytes: one.arena_bytes * 16,
        leases: one.leases * 16,
        mappings: one.mappings * 16,
        file_descriptors: one.file_descriptors * 16,
        workers: one.workers,
        client_instances: one.client_instances * 16,
        pinned_workers: 0,
    }));
    let _first = controller.admit(&profile, None).unwrap();
    assert!(matches!(
        controller.admit(&profile, None),
        Err(AdmissionError::WorkerLimit)
    ));
    assert!(matches!(
        controller.can_admit(&profile, None),
        Err(AdmissionError::WorkerLimit)
    ));
}

fn span_profile(max_spans: usize) -> TargetProfile {
    TargetProfile::new(ProfileConfig {
        descriptor: TransportDescriptor::new(HardwareProfileId::new("contract-spans").unwrap()),
        descriptor_depth: 8,
        arena_bytes: shm_transport::MIN_ARENA_BYTES,
        max_spans,
        max_leases: 8,
        mappings: 2,
        pinned_workers: 0,
        worker_topology: WorkerTopology::CallerThread,
    })
    .unwrap()
}

#[test]
fn released_admissions_recompute_active_span_charge() {
    let wide = span_profile(2);
    let narrow = span_profile(1);
    let controller = Arc::new(AdmissionController::new(HostLimits {
        descriptors: 1024,
        arena_bytes: 1 << 30,
        leases: 1024,
        mappings: 1024,
        file_descriptors: 1024,
        workers: 1024,
        client_instances: 1024,
        pinned_workers: 0,
    }));

    // The active span charge equals the maximum among live admissions.
    let wide_admission = controller.admit(&wide, None).unwrap();
    let narrow_admission = controller.admit(&narrow, None).unwrap();
    assert_eq!(controller.snapshot().unwrap().active.spans_per_frame, 2);
    wide_admission.release();
    assert_eq!(controller.snapshot().unwrap().active.spans_per_frame, 1);
    drop(narrow_admission);
    assert_eq!(controller.snapshot().unwrap().active.spans_per_frame, 0);

    // Quarantine removes the span charge from the active maximum.
    let wide_admission = controller.admit(&wide, None).unwrap();
    let _quarantine = wide_admission.quarantine().unwrap();
    let snapshot = controller.snapshot().unwrap();
    assert_eq!(snapshot.active.spans_per_frame, 0);
    assert_eq!(snapshot.quarantined.spans_per_frame, 2);
}

/// The profile id is a wire literal both peers compare byte for byte, so the test spells it
/// and the depth rather than reading the constants it is checking.
#[test]
fn host_test_ring_profile_names_one_geometry() {
    let profile = host_test_ring_profile().unwrap();
    assert!(profile.descriptor().hardware_matches("host-test-ring-v1"));
    assert_eq!(profile.descriptor_depth(), 8);
    assert_eq!(profile.max_leases(), 8);
    // `Ring::create` refuses a profile that allows fewer spans than a wrapping reservation
    // needs, so the span bound is part of the geometry the id promises.
    assert_eq!(profile.max_spans(), 2);
    assert_eq!(profile.charges().spans_per_frame, 2);
    // `Ring::create` reads the per-direction arena size from the profile; the literal
    // keeps this assertion independent of `MIN_ARENA_BYTES`.
    assert_eq!(profile.arena_bytes(), 67_108_864);
    // One arena per logical direction is what one connection charges: two 64 MiB arenas.
    assert_eq!(profile.charges().arena_bytes, 134_217_728);
    assert_eq!(profile.charges().descriptors, 16);
}
