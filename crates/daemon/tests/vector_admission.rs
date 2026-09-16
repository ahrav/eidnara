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
use daemon::vector_generation::{ROWS_FILE, VectorRefusal, build, stage};
use host_runtime::generation::PruneReport;
use support::projection_gate::{identity, passing_evaluator};
use support::vector_store::{Fixture, export};

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
    // Restaging a manifest the store already holds still copies the inventory into a staging temp before the store finds its occupant, so the copy needs room in the pool even though the store publishes nothing twice and its total is unchanged afterwards.
    let settled = Ledger::store_bytes(&fixture.store).unwrap();
    fixture.set_limit(DISK_LIMIT, settled + staged_bytes - 1);
    assert_eq!(
        stage(&built, &fixture.staging()).unwrap_err(),
        VectorRefusal::Reservation(exceeded(
            DISK_LIMIT,
            settled + staged_bytes,
            settled + staged_bytes - 1
        )),
        "the retry's copy is charged like a first staging"
    );
    fixture.set_limit(DISK_LIMIT, settled + staged_bytes);
    assert_eq!(stage(&built, &fixture.staging()).unwrap(), digest);
    assert_eq!(Ledger::store_bytes(&fixture.store).unwrap(), settled);
    assert_eq!(held(&fixture.ledger, ResourceClass::Staging), 0);

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
        &fixture.staging(),
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
