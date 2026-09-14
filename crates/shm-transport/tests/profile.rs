use std::sync::Arc;

use shm_transport::backend::ring::Ring;
use shm_transport::descriptor::HardwareProfileId;
use shm_transport::pool::{ClassSpec, MappingLayout, PoolGeometry, ledger_bytes};
use shm_transport::profile::{
    AdmissionController, AdmissionError, HostLimits, ProfileError, ResourceCharges,
    host_payload_pool_profile, pool_profile,
};

fn small_geometry() -> PoolGeometry {
    PoolGeometry::new(
        2,
        1,
        [
            ClassSpec::new(4096, 2),
            ClassSpec::new(8192, 1),
            ClassSpec::new(16384, 1),
            ClassSpec::new(32768, 1),
            ClassSpec::new(64 * 1024 * 1024 + 4096, 1),
        ],
        ClassSpec::new(4096, 1),
        ClassSpec::new(32768, 1),
    )
    .unwrap()
}

fn generous_limits() -> HostLimits {
    HostLimits {
        descriptors: 1 << 20,
        mapping_bytes: 1 << 40,
        ledger_bytes: 1 << 30,
        leases: 1 << 20,
        mappings: 1024,
        file_descriptors: 1024,
        wake_handles: 1024,
        workers: 1024,
        client_instances: 1024,
        pinned_workers: 0,
    }
}

fn exact_limits(charges: ResourceCharges, connections: u64) -> HostLimits {
    HostLimits {
        descriptors: charges.descriptors * connections,
        mapping_bytes: charges.mapping_bytes * connections,
        ledger_bytes: charges.ledger_bytes * connections,
        leases: charges.leases * connections,
        mappings: charges.mappings * connections,
        file_descriptors: charges.file_descriptors * connections,
        wake_handles: charges.wake_handles * connections,
        workers: charges.workers * connections,
        client_instances: charges.client_instances * connections,
        pinned_workers: 0,
    }
}

#[test]
fn debug_redacts_profile_admission_and_quarantine_record() {
    let sentinel = "SENTINEL_profile_id";
    let profile =
        pool_profile(HardwareProfileId::new(sentinel).unwrap(), small_geometry()).unwrap();
    let controller = Arc::new(AdmissionController::new(generous_limits()));
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
    let profile = pool_profile(
        HardwareProfileId::new("contract-host").unwrap(),
        small_geometry(),
    )
    .unwrap();
    let charges = profile.charges();
    let controller = Arc::new(AdmissionController::new(exact_limits(charges, 1)));
    let admission = controller.admit(&profile, None).unwrap();
    assert_eq!(controller.snapshot().unwrap().active, charges);
    let _quarantine = admission.quarantine().unwrap();
    assert_eq!(
        controller.snapshot().unwrap().quarantined,
        ResourceCharges {
            workers: 0,
            pinned_workers: 0,
            ..charges
        }
    );
    assert_eq!(controller.snapshot().unwrap().active, ResourceCharges::ZERO);
    // Quarantined commitments still count against every limit but the worker limit.
    assert!(matches!(
        controller.admit(&profile, None),
        Err(AdmissionError::DescriptorLimit)
    ));
}

#[test]
fn exact_aggregate_capacity_admits_n_and_rejects_n_plus_one_without_charging() {
    let profile = pool_profile(
        HardwareProfileId::new("contract-n").unwrap(),
        small_geometry(),
    )
    .unwrap();
    let charges = profile.charges();
    let controller = Arc::new(AdmissionController::new(exact_limits(charges, 3)));
    let admissions: Vec<_> = (0..3)
        .map(|_| controller.admit(&profile, None).unwrap())
        .collect();
    let full = controller.snapshot().unwrap();
    assert!(matches!(
        controller.admit(&profile, None),
        Err(AdmissionError::DescriptorLimit)
    ));
    assert_eq!(controller.snapshot().unwrap(), full);
    drop(admissions);
    let reclaimed = controller.snapshot().unwrap();
    assert_eq!(reclaimed.active, ResourceCharges::ZERO);
    assert_eq!(reclaimed.quarantined, ResourceCharges::ZERO);
}

#[test]
fn every_limit_is_checked_in_field_order() {
    let profile = pool_profile(
        HardwareProfileId::new("contract-order").unwrap(),
        small_geometry(),
    )
    .unwrap();
    let one = profile.charges();
    let mut limits = exact_limits(one, 16);
    limits.workers = 1024;
    type Tighten = fn(&mut HostLimits, ResourceCharges);
    let cases: [(&str, Tighten, AdmissionError); 8] = [
        (
            "descriptors",
            |l, c| l.descriptors = c.descriptors - 1,
            AdmissionError::DescriptorLimit,
        ),
        (
            "mapping_bytes",
            |l, c| l.mapping_bytes = c.mapping_bytes - 1,
            AdmissionError::MappingByteLimit,
        ),
        (
            "ledger_bytes",
            |l, c| l.ledger_bytes = c.ledger_bytes - 1,
            AdmissionError::LedgerByteLimit,
        ),
        (
            "leases",
            |l, c| l.leases = c.leases - 1,
            AdmissionError::LeaseLimit,
        ),
        (
            "mappings",
            |l, c| l.mappings = c.mappings - 1,
            AdmissionError::MappingLimit,
        ),
        (
            "file_descriptors",
            |l, c| l.file_descriptors = c.file_descriptors - 1,
            AdmissionError::FileDescriptorLimit,
        ),
        (
            "wake_handles",
            |l, c| l.wake_handles = c.wake_handles - 1,
            AdmissionError::WakeHandleLimit,
        ),
        (
            "client_instances",
            |l, c| l.client_instances = c.client_instances - 1,
            AdmissionError::ClientInstanceLimit,
        ),
    ];
    for (name, tighten, expected) in cases {
        let mut tightened = limits;
        tighten(&mut tightened, one);
        let controller = Arc::new(AdmissionController::new(tightened));
        assert_eq!(
            controller.admit(&profile, None).err(),
            Some(expected),
            "{name} is the first exceeded limit"
        );
        assert_eq!(controller.snapshot().unwrap().active, ResourceCharges::ZERO);
    }
}

#[test]
fn worker_limit_is_the_only_limit_that_refuses_a_second_fused_admission() {
    let profile = host_payload_pool_profile().unwrap();
    let one = profile.charges();
    let mut limits = exact_limits(one, 16);
    limits.workers = one.workers;
    let controller = Arc::new(AdmissionController::new(limits));
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

#[test]
fn split_admission_settles_worker_and_backing_charges_independently() {
    let profile = pool_profile(
        HardwareProfileId::new("contract-split").unwrap(),
        small_geometry(),
    )
    .unwrap();
    let one = profile.charges();
    let controller = Arc::new(AdmissionController::new(generous_limits()));
    let admission = controller.admit(&profile, None).unwrap();
    let (worker, backing) = admission.split();
    assert_eq!(controller.snapshot().unwrap().active, one);
    drop(worker);
    let after_worker = controller.snapshot().unwrap().active;
    assert_eq!(after_worker.workers, 0);
    assert_eq!(after_worker.mapping_bytes, one.mapping_bytes);
    assert_eq!(after_worker.leases, one.leases);
    // Quarantine keeps the backing counted; a second quarantine of the same charge is refused.
    let backing = Arc::new(backing);
    backing.quarantine().unwrap();
    assert!(matches!(
        backing.quarantine(),
        Err(AdmissionError::ChargeUnderflow)
    ));
    let snapshot = controller.snapshot().unwrap();
    assert_eq!(snapshot.active, ResourceCharges::ZERO);
    assert_eq!(snapshot.quarantined.mapping_bytes, one.mapping_bytes);
    drop(backing);
    assert_eq!(
        controller.snapshot().unwrap().quarantined.mapping_bytes,
        one.mapping_bytes,
        "a quarantined charge is never refunded by drop"
    );

    // An uncertain retention keeps the charge active forever.
    let (worker, backing) = controller.admit(&profile, None).unwrap().split();
    drop(worker);
    backing.retain_uncertain();
    assert!(!backing.is_active());
    drop(backing);
    let snapshot = controller.snapshot().unwrap();
    assert_eq!(snapshot.active.mapping_bytes, one.mapping_bytes);
}

/// The profile id is a wire literal both peers compare byte for byte, so the test spells it
/// and the inventory rather than reading the constants it is checking.
#[test]
fn host_payload_pool_profile_names_one_geometry_and_complete_charges() {
    let profile = host_payload_pool_profile().unwrap();
    assert!(
        profile
            .descriptor()
            .hardware_matches("host-payload-pool-v1")
    );
    assert_eq!(profile.descriptor().schema_version(), 4);
    let geometry = profile.geometry();
    assert_eq!(geometry.ordinary_descriptors(), 32);
    assert_eq!(geometry.reserved_descriptors(), 16);
    assert_eq!(geometry.block_count(), 187);
    assert_eq!(geometry.arena_bytes().unwrap(), 95_817_728);
    let layout = MappingLayout::new(geometry, 4096).unwrap();
    assert_eq!(profile.mapping_bytes_per_direction(), layout.total as u64);
    let charges = profile.charges();
    assert_eq!(charges.descriptors, 96);
    assert_eq!(charges.leases, 374);
    assert_eq!(charges.mapping_bytes, 2 * layout.total as u64);
    assert_eq!(charges.ledger_bytes, 2 * ledger_bytes(geometry));
    assert_eq!(charges.mappings, 2);
    assert_eq!(charges.file_descriptors, 6);
    assert_eq!(charges.wake_handles, 2);
    assert_eq!(charges.workers, 1);
    assert_eq!(charges.client_instances, 1);
    assert!(charges.committed_bytes().unwrap() > charges.mapping_bytes);
    // The charge is the actual object size both directions create.
    let ring = Ring::create(&profile, 0).unwrap();
    assert_eq!(ring.object_size() as u64 * 2, charges.mapping_bytes);
}

#[test]
fn profile_refuses_a_geometry_that_cannot_place_a_maximum_frame() {
    let geometry = PoolGeometry::new(
        2,
        1,
        [
            ClassSpec::new(4096, 2),
            ClassSpec::new(8192, 1),
            ClassSpec::new(16384, 1),
            ClassSpec::new(32768, 1),
            ClassSpec::new(64 * 1024 * 1024, 1),
        ],
        ClassSpec::new(4096, 1),
        ClassSpec::new(32768, 1),
    )
    .unwrap();
    assert_eq!(
        pool_profile(HardwareProfileId::new("contract-small").unwrap(), geometry).err(),
        Some(ProfileError::LargestClassBelowMaximumFrame)
    );
}
