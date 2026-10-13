//! The vector ledger against the gate's limits: exact bounds, overflow, competing reservations, static residents, the disk pool's view of the store, delta admission, and release by drop alone.

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};

use daemon::projection_gates::{
    Denial, EntryPoint, Gate, HookGate, InvalidationIdentity, ProjectionHook,
};
use daemon::vector_admission::{
    Census, DELTA_LIMIT, DISK_LIMIT, Ledger, Pool, RESIDENT_LIMIT, Refusal, ResourceClass,
};
use daemon::vector_composition::{CompositionRefusal, Progress, publish};
use daemon::vector_generation::{ROWS_FILE, VectorRefusal, build, stage, stage_observed};
use host_runtime::generation::PruneReport;
use support::projection_gate::{identity, passing_evaluator};
use support::vector_store::{Fixture, export, occupant_inode};

const DIMENSION: u32 = 8;

fn held(ledger: &Ledger, class: ResourceClass) -> u64 {
    ledger.census().held.get(&class).copied().unwrap_or(0)
}

/// A gate and ledger under the test identity with `limit` set to `value`, every other limit unbounded.
fn ledger_with(limit: &str, value: u64) -> (Arc<HookGate>, Arc<Ledger>) {
    let identity = identity("test-incarnation", DIMENSION);
    let gate = Arc::new(HookGate::closed());
    let mut evaluator = passing_evaluator(&identity, 0, &ProjectionHook::ALL);
    evaluator.manifest.limits.insert(limit.to_owned(), value);
    gate.install(evaluator);
    let ledger = Ledger::new(Arc::clone(&gate), InvalidationIdentity::from(&identity));
    (gate, ledger)
}

fn admit(gate: &HookGate) -> daemon::projection_gates::Admission {
    gate.admit(ProjectionHook::EmbeddingBootstrap, EntryPoint::Explicit)
        .unwrap()
}

fn exceeded(limit: &str, observed: u64, max: u64) -> Refusal {
    Refusal::Denied(Denial::LimitExceeded {
        limit: limit.to_owned(),
        observed,
        max,
    })
}

#[test]
fn the_exact_bound_admits_one_more_byte_refuses_and_overflow_never_reaches_the_gate() {
    let (gate, ledger) = ledger_with(RESIDENT_LIMIT, 100);
    let grant = admit(&gate);
    let text = ledger.reserve(&grant, ResourceClass::Text, 60).unwrap();
    let scratch = ledger.reserve(&grant, ResourceClass::Scratch, 40).unwrap();
    assert_eq!(ledger.census().resident, 100);
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::RowBuffers, 1)
            .unwrap_err(),
        exceeded(RESIDENT_LIMIT, 101, 100),
        "the pool is judged as a whole, whatever class asks"
    );
    assert_eq!(
        held(&ledger, ResourceClass::RowBuffers),
        0,
        "a refusal records nothing"
    );
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::RowBuffers, u64::MAX)
            .unwrap_err(),
        Refusal::Overflow {
            pool: Pool::Resident
        }
    );
    drop(scratch);
    assert_eq!(ledger.census().resident, 60);
    let again = ledger
        .reserve(&grant, ResourceClass::RowBuffers, 40)
        .unwrap();
    assert_eq!(
        (again.class(), again.bytes()),
        (ResourceClass::RowBuffers, 40)
    );
    drop((text, again));
    assert_eq!(ledger.census(), Census::default());

    // A class is bound to its pool: a disk class asked of the resident entry point is refused, and so is the reverse.
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::Staging, 1)
            .unwrap_err(),
        Refusal::WrongPool { pool: Pool::Disk }
    );
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .ledger
            .reserve_disk(&fixture.admission, ResourceClass::Text, 1, &fixture.store)
            .unwrap_err(),
        Refusal::WrongPool {
            pool: Pool::Resident
        }
    );
}

#[test]
fn competing_reservations_admit_exactly_what_fits_and_a_cancelled_grant_releases_nothing() {
    let (gate, ledger) = ledger_with(RESIDENT_LIMIT, 5 * 10);
    let grant = admit(&gate);
    let barrier = Arc::new(Barrier::new(12));
    let workers: Vec<_> = (0..12)
        .map(|_| {
            let ledger = Arc::clone(&ledger);
            let grant = grant.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                ledger.reserve(&grant, ResourceClass::Text, 10)
            })
        })
        .collect();
    let outcomes: Vec<Result<_, _>> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    let admitted: Vec<_> = outcomes.iter().filter(|o| o.is_ok()).collect();
    assert_eq!(admitted.len(), 5, "twelve racers, room for five");
    assert!(
        outcomes
            .iter()
            .filter_map(|o| o.as_ref().err())
            .all(|refusal| matches!(
                refusal,
                Refusal::Denied(Denial::LimitExceeded {
                    observed: 60,
                    max: 50,
                    ..
                })
            ))
    );
    assert_eq!(ledger.census().resident, 50);

    // Cancelling the grant withdraws new reservations but releases none that are held.
    grant.invalidated.cancel();
    assert_eq!(ledger.census().resident, 50);
    drop(outcomes);
    assert_eq!(ledger.census(), Census::default());
    assert_eq!(
        ledger.reserve(&grant, ResourceClass::Text, 1).unwrap_err(),
        Refusal::Denied(Denial::Invalidated)
    );
}

#[test]
fn static_residents_are_charged_once_and_an_absent_limit_fails_closed() {
    let (gate, ledger) = ledger_with(RESIDENT_LIMIT, u64::MAX);
    let grant = admit(&gate);
    let model = ledger
        .reserve(&grant, ResourceClass::ModelMemory, 1 << 30)
        .unwrap();
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::ModelMemory, 1)
            .unwrap_err(),
        Refusal::AlreadyCharged {
            class: ResourceClass::ModelMemory
        },
        "the model is never counted twice"
    );
    let _tokenizer = ledger
        .reserve(&grant, ResourceClass::TokenizerCache, 1 << 10)
        .unwrap();
    drop(model);
    assert!(
        ledger
            .reserve(&grant, ResourceClass::ModelMemory, 1 << 30)
            .is_ok(),
        "released, the class may be charged again"
    );

    let identity = identity("test-incarnation", DIMENSION);
    let gate = Arc::new(HookGate::closed());
    let mut evaluator = passing_evaluator(&identity, 0, &ProjectionHook::ALL);
    evaluator.manifest.limits.remove(RESIDENT_LIMIT);
    gate.install(evaluator);
    let ledger = Ledger::new(Arc::clone(&gate), InvalidationIdentity::from(&identity));
    let grant = admit(&gate);
    assert_eq!(
        ledger.reserve(&grant, ResourceClass::Text, 1).unwrap_err(),
        Refusal::Denied(Denial::Failed(
            Gate::Resource,
            format!("limit {RESIDENT_LIMIT} is absent")
        )),
        "no owner value, no admission"
    );
    assert_eq!(ledger.census(), Census::default());
}

#[test]
fn the_disk_pool_counts_the_store_and_staging_is_refused_before_it_would_exceed_the_bound() {
    let mut fixture = Fixture::new();
    let first = fixture.layer(1, 10);
    let store_bytes = Ledger::store_bytes(&fixture.store).unwrap();
    assert!(store_bytes > 0);
    let built = build(&fixture.expected(), &export(2, 12), &fixture.work_dir()).unwrap();
    // The payload inventory plus the `manifest.json` the store writes beside it: every byte the staging leaves in the store.
    let manifest = built.sidecar.stage_manifest();
    let staged_bytes: u64 = manifest.files.iter().map(|f| f.size).sum::<u64>()
        + manifest.canonical_bytes().len() as u64;

    fixture.set_limit(DISK_LIMIT, store_bytes + staged_bytes - 1);
    let refusal = stage(&built, &fixture.staging()).unwrap_err();
    assert_eq!(
        refusal,
        VectorRefusal::Reservation(exceeded(
            DISK_LIMIT,
            store_bytes + staged_bytes,
            store_bytes + staged_bytes - 1
        ))
    );
    assert_eq!(
        fixture.generations(),
        BTreeSet::from([first.digest.clone()]),
        "nothing was staged"
    );
    assert_eq!(
        fixture.ledger.census(),
        Census::default(),
        "a refused reservation holds nothing"
    );

    fixture.set_limit(DISK_LIMIT, store_bytes + staged_bytes);
    let digest = stage(&built, &fixture.staging()).unwrap();
    assert!(fixture.generations().contains(&digest));
    assert_eq!(
        held(&fixture.ledger, ResourceClass::Staging),
        0,
        "the staging reservation ends with the copy; the bytes are the store's"
    );
    assert_eq!(
        Ledger::store_bytes(&fixture.store).unwrap(),
        store_bytes + staged_bytes,
        "the store grew by exactly the reserved bytes, so it stays within the bound it was admitted under"
    );
    // A compactor's scratch reservation is judged against the same pool and the same store total; a resident reservation meanwhile leaves the disk pool alone.
    let _resident = fixture
        .ledger
        .reserve(&fixture.admission, ResourceClass::Text, 1 << 20)
        .unwrap();
    let compaction = fixture.ledger.reserve_disk(
        &fixture.admission,
        ResourceClass::CompactionScratch,
        1,
        &fixture.store,
    );
    assert!(matches!(
        compaction,
        Err(Refusal::Denied(Denial::LimitExceeded { .. }))
    ));
    // Restaging a manifest the store already holds confirms the valid occupant in place. It adds no bytes, so it succeeds with the disk pool at the store's total and even below it, where a second copy could not be reserved, and the store keeps the one directory.
    let settled = Ledger::store_bytes(&fixture.store).unwrap();
    let before = occupant_inode(&fixture.generation_dir(&digest));
    for limit in [settled, settled - 1] {
        fixture.set_limit(DISK_LIMIT, limit);
        assert_eq!(stage(&built, &fixture.staging()).unwrap(), digest);
        assert_eq!(Ledger::store_bytes(&fixture.store).unwrap(), settled);
        assert_eq!(
            occupant_inode(&fixture.generation_dir(&digest)),
            before,
            "the confirmed generation is the one publication of its bytes"
        );
    }
    // The staged-bytes admission precedes the confirmation: a denied retry fires no store event and leaves the store as it was.
    fixture.set_limit(DISK_LIMIT, u64::MAX);
    let inventory: u64 = manifest.files.iter().map(|f| f.size).sum();
    fixture.set_limit("capture_disk_bytes", inventory - 1);
    let mut events = Vec::new();
    let denied = stage_observed(&built, &fixture.staging(), &mut |event| {
        events.push(event);
        Ok(())
    });
    assert!(
        matches!(denied, Err(VectorRefusal::Admission(_))),
        "{denied:?}"
    );
    assert!(events.is_empty(), "a denied admission confirms nothing");
    assert_eq!(Ledger::store_bytes(&fixture.store).unwrap(), settled);
    fixture.set_limit("capture_disk_bytes", u64::MAX);

    // A readable manifest over a corrupt payload triggers an exchange repair, so staging reserves `staged_bytes` before replacing the occupant.
    fixture.corrupt(&digest, ROWS_FILE);
    assert!(fixture.store.manifest(&digest).is_ok());
    assert!(fixture.store.validate(&digest).is_err());
    fixture.set_limit(DISK_LIMIT, settled + staged_bytes - 1);
    assert_eq!(
        stage(&built, &fixture.staging()).unwrap_err(),
        VectorRefusal::Reservation(exceeded(
            DISK_LIMIT,
            settled + staged_bytes,
            settled + staged_bytes - 1
        ))
    );
    assert!(
        fixture.store.validate(&digest).is_err(),
        "a refused reservation repairs nothing"
    );
    fixture.set_limit(DISK_LIMIT, settled + staged_bytes);
    assert_eq!(stage(&built, &fixture.staging()).unwrap(), digest);
    assert!(fixture.store.validate(&digest).is_ok());
    assert_eq!(fixture.ledger.census().disk, 0);
}

#[test]
fn the_store_walk_counts_bytes_nested_inside_a_generation() {
    let fixture = Fixture::new();
    fixture.layer(1, 10);
    let flat = Ledger::store_bytes(&fixture.store).unwrap();
    let payload = vec![7u8; 4096];
    fixture.stage_foreign("host-release", "payload/bin/launcher", &payload);
    let nested = Ledger::store_bytes(&fixture.store).unwrap();
    assert!(
        nested >= flat + payload.len() as u64,
        "a generation's nested files are part of the store's total: {flat} then {nested}"
    );
}

#[test]
fn delta_admission_uses_the_gate_and_a_refused_publication_stages_nothing() {
    let mut fixture = Fixture::new();
    let base = fixture.layer(1, 10);
    let one = fixture.layer(2, 12);
    let two = fixture.layer(3, 14);
    let before = fixture.generations();
    fixture.set_limit(DELTA_LIMIT, 1);
    let composition = fixture.compose(1, &base, &[one, two]).unwrap();
    let failure = publish(
        &composition,
        fixture.predecessor().as_deref(),
        &fixture.staging(),
        Some(&fixture.activation()),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(failure.progress, Progress::NotStaged);
    assert_eq!(
        failure.refusal,
        CompositionRefusal::Deltas(Denial::LimitExceeded {
            limit: DELTA_LIMIT.to_owned(),
            observed: 2,
            max: 1
        })
    );
    assert_eq!(fixture.generations(), before);
    let within = fixture.compose(1, &base, &[fixture.layer(4, 16)]).unwrap();
    assert!(fixture.publish(&within).is_ok(), "the exact bound admits");
    assert_eq!(fixture.ledger.census(), Census::default());
}

/// A grant from `other` stays live across changes to `gate`, so each refusal below comes from `gate`'s own state on the reservation right after the change.
#[test]
fn every_gate_change_reaches_the_next_reservation_under_a_grant_the_change_did_not_cancel() {
    let (gate, ledger) = ledger_with(RESIDENT_LIMIT, 100);
    let (other, _) = ledger_with(RESIDENT_LIMIT, 100);
    let grant = admit(&other);
    drop(ledger.reserve(&grant, ResourceClass::Text, 60).unwrap());
    ledger.admit_deltas(&grant, 2).unwrap();

    let identity = identity("test-incarnation", DIMENSION);
    let mut lowered = passing_evaluator(&identity, 0, &ProjectionHook::ALL);
    lowered
        .manifest
        .limits
        .insert(RESIDENT_LIMIT.to_owned(), 50);
    lowered.manifest.limits.insert(DELTA_LIMIT.to_owned(), 1);
    gate.install(lowered);
    assert!(!grant.invalidated.is_cancelled());
    assert_eq!(
        ledger.reserve(&grant, ResourceClass::Text, 60).unwrap_err(),
        exceeded(RESIDENT_LIMIT, 60, 50),
        "the reinstalled limit judges the next reservation"
    );
    assert_eq!(
        ledger.admit_deltas(&grant, 2).unwrap_err(),
        Denial::LimitExceeded {
            limit: DELTA_LIMIT.to_owned(),
            observed: 2,
            max: 1
        }
    );
    drop(ledger.reserve(&grant, ResourceClass::Text, 50).unwrap());

    gate.close();
    assert_eq!(
        ledger.reserve(&grant, ResourceClass::Text, 1).unwrap_err(),
        Refusal::Denied(Denial::NoManifest)
    );
    assert_eq!(
        ledger.admit_deltas(&grant, 0).unwrap_err(),
        Denial::NoManifest
    );

    let mut renamed = passing_evaluator(&identity, 0, &ProjectionHook::ALL);
    renamed.current.embedding_model = "another-model".to_owned();
    gate.install(renamed);
    assert_eq!(
        ledger.reserve(&grant, ResourceClass::Text, 1).unwrap_err(),
        Refusal::Denied(Denial::EvidenceIdentity),
        "an evaluator for another identity refuses the ledger's"
    );
    assert_eq!(ledger.census(), Census::default());
}

#[test]
fn a_zero_byte_static_charge_still_charges_once_and_refusals_keep_their_order() {
    let (gate, ledger) = ledger_with(RESIDENT_LIMIT, 100);
    let grant = admit(&gate);
    let model = ledger
        .reserve(&grant, ResourceClass::ModelMemory, 0)
        .unwrap();
    let text = ledger.reserve(&grant, ResourceClass::Text, 1).unwrap();
    assert_eq!(
        ledger.census().held.get(&ResourceClass::ModelMemory),
        Some(&0),
        "a zero-byte reservation still charges its class"
    );
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::ModelMemory, 0)
            .unwrap_err(),
        Refusal::AlreadyCharged {
            class: ResourceClass::ModelMemory
        }
    );

    // The ledger refuses a double charge and an overflow before it consults the gate, so a cancelled grant sees those refusals first.
    grant.invalidated.cancel();
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::ModelMemory, 1)
            .unwrap_err(),
        Refusal::AlreadyCharged {
            class: ResourceClass::ModelMemory
        }
    );
    assert_eq!(
        ledger
            .reserve(&grant, ResourceClass::Text, u64::MAX)
            .unwrap_err(),
        Refusal::Overflow {
            pool: Pool::Resident
        }
    );
    assert_eq!(
        ledger.reserve(&grant, ResourceClass::Text, 1).unwrap_err(),
        Refusal::Denied(Denial::Invalidated)
    );
    drop((model, text));
    assert_eq!(ledger.census(), Census::default());
}

const CLASSES: [ResourceClass; 9] = [
    ResourceClass::ModelMemory,
    ResourceClass::TokenizerCache,
    ResourceClass::SqliteCache,
    ResourceClass::Text,
    ResourceClass::Scratch,
    ResourceClass::RowBuffers,
    ResourceClass::LayerTables,
    ResourceClass::Staging,
    ResourceClass::CompactionScratch,
];

/// The match is exhaustive, so a class added to the enum fails to compile here until it is named, next to `CLASSES`.
fn named(class: ResourceClass) -> bool {
    match class {
        ResourceClass::ModelMemory
        | ResourceClass::TokenizerCache
        | ResourceClass::SqliteCache
        | ResourceClass::Text
        | ResourceClass::Scratch
        | ResourceClass::RowBuffers
        | ResourceClass::LayerTables
        | ResourceClass::Staging
        | ResourceClass::CompactionScratch => CLASSES.contains(&class),
    }
}

#[test]
fn every_class_reserves_into_its_own_census_slot_and_pool_and_releases_by_drop() {
    let fixture = Fixture::new();
    let mut reservations = Vec::new();
    let mut expected = Census::default();
    // Each class charges a distinct power of two, so a charge landing in another class's slot or pool changes the census.
    for (index, class) in CLASSES.into_iter().enumerate() {
        assert!(named(class), "{class:?} is listed in CLASSES");
        let bytes = 1u64 << index;
        let reservation = match class.pool() {
            Pool::Resident => fixture.ledger.reserve(&fixture.admission, class, bytes),
            Pool::Disk => {
                fixture
                    .ledger
                    .reserve_disk(&fixture.admission, class, bytes, &fixture.store)
            }
        }
        .unwrap_or_else(|refusal| panic!("{class:?} reserves: {refusal}"));
        match class.pool() {
            Pool::Resident => expected.resident += bytes,
            Pool::Disk => expected.disk += bytes,
        }
        expected.held.insert(class, bytes);
        assert_eq!(
            fixture.ledger.census(),
            expected,
            "after charging {class:?}"
        );
        reservations.push(reservation);
    }
    for reservation in reservations {
        let class = reservation.class();
        match class.pool() {
            Pool::Resident => expected.resident -= reservation.bytes(),
            Pool::Disk => expected.disk -= reservation.bytes(),
        }
        expected.held.remove(&class);
        drop(reservation);
        assert_eq!(
            fixture.ledger.census(),
            expected,
            "after releasing {class:?}"
        );
    }
    assert_eq!(fixture.ledger.census(), Census::default());
}

#[test]
fn a_prune_readback_above_the_ledgers_pinned_bytes_is_unaccounted() {
    let (_, ledger) = ledger_with(RESIDENT_LIMIT, u64::MAX);
    let none = ledger.reconcile(&PruneReport {
        retained_bytes: 0,
        ..PruneReport::default()
    });
    assert!(!none.unaccounted());
    let some = ledger.reconcile(&PruneReport {
        retained_bytes: 10,
        retained_pinned: 1,
        ..PruneReport::default()
    });
    assert_eq!((some.ledger_pinned, some.prune_retained), (0, 10));
    assert!(
        some.unaccounted(),
        "readers pin bytes the ledger was never told about"
    );
}
