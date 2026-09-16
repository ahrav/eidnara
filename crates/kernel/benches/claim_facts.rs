//! Local development probes, not a production workload or a regression gate.
//! Fixture construction and canaries are untimed; returned-value destruction
//! is timed. Repeated reads warm caches; no concurrent writers are modeled.

#[path = "../tests/claim_fixture/mod.rs"]
mod claim_fixture;

use std::hint::black_box;

use claim_fixture::{Fixture, bounds, direct, request};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use kernel::{CausalClass, ClaimFactsError, ServedStanding};

fn claim_facts(c: &mut Criterion) {
    for (shape, representations) in [
        ("no_descriptors", &[][..]),
        (
            "all_descriptors",
            &[
                ("canonical_claims", "decision_summary"),
                ("canonical_claims", "rationale"),
                ("promoted_memory", "summary"),
            ][..],
        ),
    ] {
        let fixture = Fixture::open();
        let (evidence_id, digest) = fixture.retain("acquisition", "observed text");
        let ids: Vec<_> = (0..64).map(|i| format!("decision-object-{i}")).collect();
        for (index, object_id) in ids.iter().enumerate() {
            let index = i64::try_from(index).unwrap();
            fixture.admit_decision(index, 1);
            fixture
                .record(
                    &format!("direct-{index}"),
                    request(object_id, 1, direct(&evidence_id, &digest)),
                )
                .unwrap();
            for &(class, representation) in representations {
                fixture.publish(class, representation, index, 1, "claim text");
            }
        }
        let tip = fixture.tip();
        let bounds = bounds();
        let mut group = c.benchmark_group(format!("claim_facts/{shape}"));
        for count in [1, 8, 64] {
            let ids = &ids[..count];
            let snapshot = fixture.store.claim_facts_as_of(ids, tip, bounds).unwrap();
            assert_eq!(snapshot.known_as_of, tip);
            assert_eq!(snapshot.tip, tip);
            assert!(snapshot.missing.is_empty());
            assert_eq!(snapshot.claims.len(), count);
            for (claim, id) in snapshot.claims.iter().zip(ids) {
                assert_eq!(&claim.object.object_id, id);
                assert!(matches!(claim.served, ServedStanding::Served(_)));
                assert!(claim.own_admission.is_some());
                assert!(matches!(
                    claim.causality,
                    CausalClass::DirectObservation { .. }
                ));
                assert_eq!(claim.occurrences.len(), representations.len());
                assert_eq!(
                    claim.excluded_representations.len(),
                    3 - representations.len()
                );
                for (occurrence, &(class, representation)) in
                    claim.occurrences.iter().zip(representations)
                {
                    assert_eq!(occurrence.class.code(), class);
                    assert_eq!(occurrence.representation, representation);
                }
            }
            drop(snapshot);
            group.bench_function(BenchmarkId::new("read", count), |b| {
                b.iter(|| {
                    black_box(
                        fixture
                            .store
                            .claim_facts_as_of(black_box(ids), black_box(tip), bounds)
                            .unwrap(),
                    );
                });
            });

            let mut rejected = ids.to_vec();
            rejected[0] = "domain-object".to_string();
            assert_eq!(
                fixture.store.claim_facts_as_of(&rejected, tip, bounds),
                Err(ClaimFactsError::NotADecision)
            );
            group.bench_function(BenchmarkId::new("reject_first", count), |b| {
                b.iter(|| {
                    black_box(
                        fixture
                            .store
                            .claim_facts_as_of(black_box(&rejected), black_box(tip), bounds)
                            .unwrap_err(),
                    );
                });
            });
        }
        group.finish();
    }
}

criterion_group!(benches, claim_facts);
criterion_main!(benches);
