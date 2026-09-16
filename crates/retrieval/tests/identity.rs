//! These tests require distinct occurrences to remain separate, each lane to vote once per occurrence, and digests to preserve identity tuples without scope.
//! The seed is fixed so a failing case reproduces.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use kernel::source_identity::{
    EncodedOccurrence, OCCURRENCE_ENCODING_VERSION, Occurrence, OccurrenceClass, Span, encode,
    payload_id,
};
use kernel::{Dimension, ProjectScope, ScopeTermSpec};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::identity::{
    ContextRevision, DeclaredLanes, GenerationId, HitOrigin, IdentityRefusal, InvocationId, Lane,
    LaneHit, LaneRanking, OccurrenceId, ParentGroupKey, ParentId, PreparationDigest,
    PreparationInputs, ProbeOrdinal, RawScore, SelectedSpan, SelectionDigest,
};

const SEED: [u8; 32] = *b"fusion-identity-laws-seed-000001";

fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases: 512,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    )
}

const BUFFER: &str = "shared payload bytes that two occurrences carry";

#[derive(Debug)]
struct Source {
    object: &'static str,
    revision: &'static str,
    representation: &'static str,
    span: Option<Span>,
}

impl Source {
    fn encoded(&self) -> EncodedOccurrence {
        encode(
            &Occurrence {
                class: OccurrenceClass::CanonicalClaims.code(),
                identity: &[("object_id", self.object)],
                revision: self.revision,
                representation: self.representation,
                span: self.span,
            },
            BUFFER,
        )
        .unwrap()
    }

    fn id(&self) -> OccurrenceId {
        OccurrenceId::parse(&self.encoded().occurrence_id).unwrap()
    }

    fn group(&self) -> ParentGroupKey {
        let encoded = self.encoded();
        ParentGroupKey::derive(
            &encoded.tuple,
            encoded.class,
            encoded.revision,
            self.representation,
            encoded.span,
        )
        .unwrap()
    }
}

const fn source(
    object: &'static str,
    revision: &'static str,
    representation: &'static str,
    span: Option<Span>,
) -> Source {
    Source {
        object,
        revision,
        representation,
        span,
    }
}

const fn range(start: u64, end: u64) -> Option<Span> {
    Some(Span { start, end })
}

fn synthetic(byte: u8) -> OccurrenceId {
    OccurrenceId::parse(&format!("{byte:02x}").repeat(32)).unwrap()
}

fn probe(ordinal: u32) -> HitOrigin {
    HitOrigin::Probe(ProbeOrdinal(ordinal))
}

fn generation(name: &str) -> HitOrigin {
    HitOrigin::Generation(GenerationId::parse(name).unwrap())
}

fn hit(occurrence: OccurrenceId, raw_score: RawScore, origin: HitOrigin) -> LaneHit {
    LaneHit {
        occurrence,
        raw_score,
        origin,
    }
}

fn positions(ranking: &LaneRanking) -> Vec<(OccurrenceId, usize)> {
    ranking
        .entries()
        .iter()
        .map(|entry| (*entry.occurrence(), entry.position().get()))
        .collect()
}

#[test]
fn equal_payload_bytes_at_different_identities_stay_distinct_ranking_units() {
    let base = source("obj:a", "1", "decision_summary", None);
    let variants = [
        source("obj:b", "1", "decision_summary", None),
        source("obj:a", "2", "decision_summary", None),
        source("obj:a", "1", "rationale", None),
        source("obj:a", "1", "decision_summary", range(0, 6)),
    ];
    let payload = payload_id(BUFFER.as_bytes());
    let mut ids = BTreeSet::from([base.id()]);
    for variant in &variants {
        assert_eq!(payload_id(BUFFER.as_bytes()), payload);
        assert!(ids.insert(variant.id()), "{variant:?} collapsed");
        let ranking = LaneRanking::consolidate(
            Lane::Exact,
            OCCURRENCE_ENCODING_VERSION,
            [
                hit(base.id(), RawScore::Exact, probe(0)),
                hit(variant.id(), RawScore::Exact, probe(0)),
            ],
        )
        .unwrap();
        assert_eq!(ranking.entries().len(), 2);
    }
    assert!(OccurrenceId::parse(&payload).is_ok_and(|id| !ids.contains(&id)));
}

#[test]
fn a_lane_admits_one_entry_per_occurrence_with_the_lane_own_best_score() {
    let (a, b, c) = (synthetic(0xaa), synthetic(0xbb), synthetic(0xcc));

    let lexical = LaneRanking::consolidate(
        Lane::Lexical,
        OCCURRENCE_ENCODING_VERSION,
        [
            hit(a, RawScore::Lexical(-3.0), probe(2)),
            hit(b, RawScore::Lexical(-9.0), probe(0)),
            hit(a, RawScore::Lexical(-7.0), probe(1)),
            hit(a, RawScore::Lexical(-7.0), probe(0)),
            hit(c, RawScore::Lexical(-7.0), probe(3)),
        ],
    )
    .unwrap();
    assert_eq!(positions(&lexical), vec![(b, 1), (a, 2), (c, 3)]);
    let retained = &lexical.entries()[1];
    assert_eq!(retained.raw_score(), RawScore::Lexical(-7.0));
    assert_eq!(*retained.origin(), probe(0));

    let dense = LaneRanking::consolidate(
        Lane::Dense,
        OCCURRENCE_ENCODING_VERSION,
        [
            hit(a, RawScore::Dense(0.2), generation("gen-1")),
            hit(a, RawScore::Dense(0.9), generation("gen-2")),
            hit(b, RawScore::Dense(0.9), generation("gen-1")),
        ],
    )
    .unwrap();
    assert_eq!(positions(&dense), vec![(a, 1), (b, 2)]);
    assert_eq!(dense.entries()[0].raw_score(), RawScore::Dense(0.9));
    assert_eq!(*dense.entries()[0].origin(), generation("gen-2"));

    let exact = LaneRanking::consolidate(
        Lane::Exact,
        OCCURRENCE_ENCODING_VERSION,
        [
            hit(c, RawScore::Exact, probe(0)),
            hit(a, RawScore::Exact, probe(1)),
            hit(c, RawScore::Exact, probe(1)),
            hit(b, RawScore::Exact, probe(0)),
        ],
    )
    .unwrap();
    assert_eq!(positions(&exact), vec![(a, 1), (b, 2), (c, 3)]);
    assert_eq!(*exact.entries()[2].origin(), probe(0));
}

#[test]
fn a_lane_refuses_foreign_or_non_finite_scores() {
    let a = synthetic(0x01);
    assert_eq!(
        LaneRanking::consolidate(
            Lane::Lexical,
            OCCURRENCE_ENCODING_VERSION,
            [hit(a, RawScore::Dense(1.0), probe(0))],
        )
        .unwrap_err(),
        IdentityRefusal::ScoreLaneMismatch {
            declared: Lane::Lexical,
            found: Lane::Dense,
        }
    );
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            LaneRanking::consolidate(
                Lane::Dense,
                OCCURRENCE_ENCODING_VERSION,
                [hit(a, RawScore::Dense(value), generation("g"))],
            )
            .unwrap_err(),
            IdentityRefusal::NonFiniteScore
        );
    }
}

fn synthetic_id() -> impl Strategy<Value = OccurrenceId> {
    any::<u8>().prop_map(synthetic)
}

fn lexical_hit() -> impl Strategy<Value = LaneHit> {
    (synthetic_id(), -50i16..0, 0u32..4).prop_map(|(occurrence, rank, ordinal)| {
        hit(
            occurrence,
            RawScore::Lexical(f64::from(rank)),
            probe(ordinal),
        )
    })
}

#[test]
fn consolidation_ignores_probe_order_and_duplication() {
    runner()
        .run(
            &(prop::collection::vec(lexical_hit(), 0..24), any::<u64>()),
            |(hits, seed)| {
                let reference = LaneRanking::consolidate(
                    Lane::Lexical,
                    OCCURRENCE_ENCODING_VERSION,
                    hits.clone(),
                )
                .unwrap();
                let mut shuffled = hits.clone();
                let mut state = seed;
                for index in (1..shuffled.len()).rev() {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    shuffled.swap(index, (state >> 33) as usize % (index + 1));
                }
                shuffled.extend(hits.iter().take(hits.len() / 2).cloned());
                let permuted =
                    LaneRanking::consolidate(Lane::Lexical, OCCURRENCE_ENCODING_VERSION, shuffled)
                        .unwrap();
                prop_assert_eq!(&permuted, &reference);
                let occurrences: BTreeSet<_> = reference
                    .entries()
                    .iter()
                    .map(|entry| *entry.occurrence())
                    .collect();
                prop_assert_eq!(occurrences.len(), reference.entries().len());
                for (entry, expected) in reference.entries().iter().zip(1usize..) {
                    prop_assert_eq!(entry.position().get(), expected);
                }
                let mut sorted: Vec<_> = reference.entries().to_vec();
                sorted.sort_by(|left, right| {
                    let (RawScore::Lexical(l), RawScore::Lexical(r)) =
                        (left.raw_score(), right.raw_score())
                    else {
                        unreachable!()
                    };
                    l.total_cmp(&r)
                        .then_with(|| left.occurrence().cmp(right.occurrence()))
                });
                prop_assert_eq!(sorted, reference.entries().to_vec());
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn declared_lanes_hold_one_ranking_per_lane_in_fixed_order_under_one_encoding_version() {
    let empty = |lane, version| LaneRanking::consolidate(lane, version, []).unwrap();
    let admitted = DeclaredLanes::admit(vec![
        empty(Lane::Dense, OCCURRENCE_ENCODING_VERSION),
        empty(Lane::Exact, OCCURRENCE_ENCODING_VERSION),
    ])
    .unwrap();
    let lanes: Vec<Lane> = admitted.rankings().iter().map(LaneRanking::lane).collect();
    assert_eq!(lanes, vec![Lane::Exact, Lane::Dense]);
    assert!(admitted.lane(Lane::Lexical).is_none());
    assert_eq!(
        admitted.encoding_version(),
        Some(OCCURRENCE_ENCODING_VERSION)
    );
    assert_eq!(
        DeclaredLanes::admit(vec![]).unwrap().encoding_version(),
        None
    );

    assert_eq!(
        DeclaredLanes::admit(vec![
            empty(Lane::Lexical, OCCURRENCE_ENCODING_VERSION),
            empty(Lane::Lexical, OCCURRENCE_ENCODING_VERSION),
        ])
        .unwrap_err(),
        IdentityRefusal::DuplicateLane(Lane::Lexical)
    );
    assert_eq!(
        DeclaredLanes::admit(vec![
            empty(Lane::Exact, OCCURRENCE_ENCODING_VERSION),
            empty(Lane::Lexical, OCCURRENCE_ENCODING_VERSION + 1),
        ])
        .unwrap_err(),
        IdentityRefusal::MixedEncodingVersion {
            lane: Lane::Lexical,
            expected: OCCURRENCE_ENCODING_VERSION,
            found: OCCURRENCE_ENCODING_VERSION + 1,
        }
    );
    assert_eq!(Lane::ORDER.map(Lane::code), ["exact", "lexical", "dense"]);
    for lane in Lane::ORDER {
        assert_eq!(Lane::from_code(lane.code()), Some(lane));
    }
    assert_eq!(Lane::from_code("Exact"), None);
}

#[test]
fn only_the_lowercase_hex_spelling_of_an_identifier_is_admitted() {
    let canonical = source("obj:a", "1", "decision_summary", None)
        .encoded()
        .occurrence_id;
    let parsed = OccurrenceId::parse(&canonical).unwrap();
    assert_eq!(parsed.to_string(), canonical);
    for spelling in [
        canonical.to_uppercase(),
        format!("occurrence:{canonical}"),
        canonical[1..].to_string(),
        format!("{canonical}0"),
        canonical.replace('a', "g"),
    ] {
        assert_eq!(
            OccurrenceId::parse(&spelling).unwrap_err(),
            IdentityRefusal::MalformedDigest,
            "{spelling}"
        );
    }
    for token in ["", "a\u{0}b", &"x".repeat(513)] {
        assert_eq!(
            InvocationId::parse(token).unwrap_err(),
            IdentityRefusal::MalformedToken
        );
    }
    assert_eq!(GenerationId::parse("gen-1").unwrap().as_str(), "gen-1");
}

#[test]
fn selection_digest_tracks_order_and_membership() {
    let (a, b, c) = (synthetic(0x0a), synthetic(0x0b), synthetic(0x0c));
    let base = SelectionDigest::derive(&[a, b]);
    assert_eq!(base, SelectionDigest::derive(&[a, b]));
    assert_ne!(base, SelectionDigest::derive(&[b, a]));
    assert_ne!(base, SelectionDigest::derive(&[a, b, c]));
    assert_ne!(base, SelectionDigest::derive(&[a]));
    assert_ne!(SelectionDigest::derive(&[]), SelectionDigest::derive(&[a]));
}

#[test]
fn preparation_digest_tracks_every_component_and_never_merges_component_splits() {
    let (a, b) = (synthetic(0x0a), synthetic(0x0b));
    let selection = SelectionDigest::derive(&[a, b]);
    let selected = |occurrence, span| SelectedSpan { occurrence, span };
    let context = ContextRevision::parse("rev-7").unwrap();
    let inputs = PreparationInputs {
        context: &context,
        representation: "text",
        spans: &[selected(a, None), selected(b, range(3, 9))],
        selection: &selection,
    };
    let base = PreparationDigest::derive(inputs);
    assert_eq!(base, PreparationDigest::derive(inputs));

    let other_context = ContextRevision::parse("rev-8").unwrap();
    let other_selection = SelectionDigest::derive(&[b, a]);
    let changed = [
        PreparationInputs {
            context: &other_context,
            ..inputs
        },
        PreparationInputs {
            representation: "rationale",
            ..inputs
        },
        PreparationInputs {
            spans: &[selected(a, None), selected(b, range(3, 10))],
            ..inputs
        },
        PreparationInputs {
            spans: &[selected(a, None), selected(b, None)],
            ..inputs
        },
        PreparationInputs {
            spans: &[selected(b, range(3, 9)), selected(a, None)],
            ..inputs
        },
        PreparationInputs {
            spans: &[selected(a, None)],
            ..inputs
        },
        PreparationInputs {
            selection: &other_selection,
            ..inputs
        },
    ];
    let mut seen = BTreeSet::from([base]);
    for variant in changed {
        assert!(
            seen.insert(PreparationDigest::derive(variant)),
            "{variant:?}"
        );
    }

    let split_left = ContextRevision::parse("ab").unwrap();
    let split_right = ContextRevision::parse("a").unwrap();
    assert_ne!(
        PreparationDigest::derive(PreparationInputs {
            context: &split_left,
            representation: "c",
            spans: &[],
            selection: &selection,
        }),
        PreparationDigest::derive(PreparationInputs {
            context: &split_right,
            representation: "bc",
            spans: &[],
            selection: &selection,
        })
    );
}

#[test]
fn parent_groups_share_a_parent_across_spans_and_never_replace_occurrences() {
    let whole = source("obj:a", "3", "decision_summary", None);
    let head = source("obj:a", "3", "decision_summary", range(0, 6));
    let tail = source("obj:a", "3", "decision_summary", range(7, 14));
    let later = source("obj:a", "4", "decision_summary", range(0, 6));
    let other_representation = source("obj:a", "3", "rationale", range(0, 6));
    let other_object = source("obj:b", "3", "decision_summary", None);

    assert_eq!(whole.group(), head.group());
    assert_eq!(head.group(), tail.group());
    assert_eq!(whole.group().revision, 3);
    assert_eq!(
        whole.group().parent.to_string(),
        whole.encoded().lineage_id,
        "a whole-buffer occurrence is its own parent"
    );
    let distinct: BTreeSet<ParentGroupKey> = [
        later.group(),
        other_representation.group(),
        other_object.group(),
    ]
    .into_iter()
    .collect();
    assert_eq!(distinct.len(), 3);
    assert!(!distinct.contains(&whole.group()));
    assert_eq!(later.group().parent, whole.group().parent);

    let sources = [&whole, &head, &tail, &later, &other_representation];
    let occurrences: BTreeSet<OccurrenceId> = sources.iter().map(|s| s.id()).collect();
    assert_eq!(occurrences.len(), 5);
    let parents: BTreeSet<ParentId> = sources.iter().map(|s| s.group().parent).collect();
    for parent in parents {
        assert!(
            OccurrenceId::parse(&parent.to_string()).is_ok_and(|id| !occurrences.contains(&id))
        );
    }

    let ranking = LaneRanking::consolidate(
        Lane::Exact,
        OCCURRENCE_ENCODING_VERSION,
        [&whole, &head, &tail]
            .into_iter()
            .map(|s| hit(s.id(), RawScore::Exact, probe(0))),
    )
    .unwrap();
    assert_eq!(ranking.entries().len(), 3);

    let encoded = head.encoded();
    for (revision, representation, span) in [
        (encoded.revision + 1, "decision_summary", encoded.span),
        (encoded.revision, "rationale", encoded.span),
        (encoded.revision, "decision_summary", None),
    ] {
        assert_eq!(
            ParentGroupKey::derive(
                &encoded.tuple,
                encoded.class,
                revision,
                representation,
                span
            )
            .unwrap_err(),
            IdentityRefusal::TupleMismatch
        );
    }
}

#[test]
fn no_identity_parsed_from_arbitrary_bytes_names_the_bound_project() {
    let bound = payload_id(b"/bound/project/root");
    let scope = ProjectScope::new(&bound).unwrap();
    let names = |value: &str| {
        scope.names_project(Some(&[ScopeTermSpec {
            dimension: Dimension::Project.as_str().to_string(),
            operator: "exact".to_string(),
            exact_value: Some(value.to_string()),
            ..ScopeTermSpec::default()
        }]))
    };
    assert!(names(&bound), "the oracle accepts the bound project");

    runner()
        .run(
            &(
                prop::collection::vec(any::<u8>(), 0..80),
                prop::collection::vec(synthetic_id(), 0..6),
            ),
            |(bytes, selection)| {
                let text = String::from_utf8_lossy(&bytes);
                let parsed = [
                    OccurrenceId::parse(&text).map(|id| id.to_string()),
                    InvocationId::parse(&text).map(|t| t.to_string()),
                    ContextRevision::parse(&text).map(|t| t.to_string()),
                    GenerationId::parse(&text).map(|t| t.to_string()),
                    Ok(SelectionDigest::derive(&selection).to_string()),
                ];
                for value in parsed.into_iter().flatten() {
                    prop_assert!(!names(&value), "{value}");
                }
                prop_assert!(
                    ParentGroupKey::derive(&bytes, OccurrenceClass::Messages, 1, "text", None)
                        .is_err()
                );
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn positions_are_one_based() {
    let ranking = LaneRanking::consolidate(
        Lane::Exact,
        OCCURRENCE_ENCODING_VERSION,
        [hit(synthetic(1), RawScore::Exact, probe(0))],
    )
    .unwrap();
    assert_eq!(ranking.entries()[0].position(), NonZeroUsize::MIN);
}
