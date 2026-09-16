//! These tests require distinct occurrences to remain separate, each lane to vote once per occurrence, and digests to preserve identity tuples without scope.
//! The seed is fixed so a failing case reproduces.

use std::collections::BTreeSet;

use kernel::source_identity::{
    EncodedOccurrence, MAX_IDENTITY_VALUE_BYTES, OCCURRENCE_ENCODING_VERSION, Occurrence,
    OccurrenceClass, Span, encode, payload_id,
};
use kernel::{Dimension, ProjectScope, ScopeTermSpec};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::batch::VectorGeneration;
use retrieval::fusion::{
    ContextRepresentation, ContextRevision, DeclaredLanes, GenerationId, IdentityRefusal,
    InvocationId, Lane, LaneHit, LaneRanking, OccurrenceId, ParentGroupKey, ParentId,
    PreparationDigest, PreparationInputs, RawScore, SelectedSpan, SelectionDigest,
};

const SEED: [u8; 32] = *b"fusion-identity-laws-seed-000001";
const VERSION: u8 = OCCURRENCE_ENCODING_VERSION;

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

fn hit(occurrence: OccurrenceId, raw_score: RawScore) -> LaneHit {
    LaneHit {
        occurrence,
        raw_score,
    }
}

fn exact(hits: impl IntoIterator<Item = OccurrenceId>) -> LaneRanking {
    LaneRanking::consolidate(
        Lane::Exact,
        VERSION,
        hits.into_iter().map(|id| hit(id, RawScore::Exact)),
    )
    .unwrap()
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
    let mut ids = BTreeSet::from([base.id()]);
    for variant in &variants {
        assert!(ids.insert(variant.id()), "{variant:?} collapsed");
        let ranking = exact([base.id(), variant.id()]);
        let ranked: BTreeSet<OccurrenceId> =
            ranking.entries().iter().map(|e| *e.occurrence()).collect();
        assert_eq!(ranked, BTreeSet::from([base.id(), variant.id()]));
    }
    let payload = OccurrenceId::parse(&payload_id(BUFFER.as_bytes())).unwrap();
    assert!(!ids.contains(&payload));
}

#[test]
fn a_lane_admits_one_entry_per_occurrence_with_the_lane_own_best_score() {
    let (a, b, c) = (synthetic(0xaa), synthetic(0xbb), synthetic(0xcc));

    let lexical = LaneRanking::consolidate(
        Lane::Lexical,
        VERSION,
        [
            hit(a, RawScore::Lexical(-3.0)),
            hit(b, RawScore::Lexical(-9.0)),
            hit(a, RawScore::Lexical(-7.0)),
            hit(a, RawScore::Lexical(-7.0)),
            hit(c, RawScore::Lexical(-7.0)),
        ],
    )
    .unwrap();
    assert_eq!(positions(&lexical), vec![(b, 1), (a, 2), (c, 3)]);
    assert_eq!(lexical.entries()[1].raw_score(), RawScore::Lexical(-7.0));
    assert_eq!(lexical.lane(), Lane::Lexical);
    assert_eq!(lexical.encoding_version(), VERSION);

    let dense = LaneRanking::consolidate(
        Lane::Dense,
        VERSION,
        [
            hit(a, RawScore::Dense(0.2)),
            hit(a, RawScore::Dense(0.9)),
            hit(b, RawScore::Dense(0.9)),
        ],
    )
    .unwrap();
    assert_eq!(positions(&dense), vec![(a, 1), (b, 2)]);
    assert_eq!(dense.entries()[0].raw_score(), RawScore::Dense(0.9));

    assert_eq!(
        positions(&exact([c, a, c, b])),
        vec![(a, 1), (b, 2), (c, 3)],
        "an exact set ranks in identifier order"
    );
}

#[test]
fn a_lane_refuses_foreign_scores_non_finite_scores_and_other_encoding_versions() {
    let a = synthetic(0x01);
    assert_eq!(
        LaneRanking::consolidate(Lane::Lexical, VERSION, [hit(a, RawScore::Dense(1.0))])
            .unwrap_err(),
        IdentityRefusal::ScoreLaneMismatch {
            declared: Lane::Lexical,
            found: Lane::Dense,
        }
    );
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            LaneRanking::consolidate(Lane::Dense, VERSION, [hit(a, RawScore::Dense(value))])
                .unwrap_err(),
            IdentityRefusal::NonFiniteScore
        );
    }
    for found in [0, VERSION - 1, VERSION + 1, u8::MAX] {
        assert_eq!(
            LaneRanking::consolidate(Lane::Exact, found, [hit(a, RawScore::Exact)]).unwrap_err(),
            IdentityRefusal::EncodingVersion {
                lane: Lane::Exact,
                expected: VERSION,
                found,
            }
        );
    }
}

fn digest_hex() -> impl Strategy<Value = String> {
    prop::collection::vec(0u8..16, 64)
        .prop_map(|nibbles| nibbles.iter().map(|n| format!("{n:x}")).collect())
}

fn synthetic_id() -> impl Strategy<Value = OccurrenceId> {
    digest_hex().prop_map(|hex| OccurrenceId::parse(&hex).unwrap())
}

fn lexical_hits() -> impl Strategy<Value = Vec<LaneHit>> {
    prop::collection::vec(
        (synthetic_id(), -50i16..0)
            .prop_map(|(occurrence, rank)| hit(occurrence, RawScore::Lexical(f64::from(rank)))),
        0..24,
    )
}

#[test]
fn consolidation_ignores_probe_order_and_duplication() {
    runner()
        .run(
            &lexical_hits().prop_flat_map(|hits| {
                let doubled: Vec<LaneHit> = hits.iter().chain(&hits).copied().collect();
                (Just(hits), Just(doubled).prop_shuffle())
            }),
            |(hits, shuffled)| {
                let reference =
                    LaneRanking::consolidate(Lane::Lexical, VERSION, hits.clone()).unwrap();
                let permuted = LaneRanking::consolidate(Lane::Lexical, VERSION, shuffled).unwrap();
                prop_assert_eq!(&permuted, &reference);
                let distinct: BTreeSet<OccurrenceId> =
                    hits.iter().map(|hit| hit.occurrence).collect();
                prop_assert_eq!(reference.entries().len(), distinct.len());
                for (entry, expected) in reference.entries().iter().zip(1usize..) {
                    prop_assert_eq!(entry.position().get(), expected);
                    let best = hits
                        .iter()
                        .filter(|hit| hit.occurrence == *entry.occurrence())
                        .map(|hit| match hit.raw_score {
                            RawScore::Lexical(rank) => rank,
                            _ => unreachable!(),
                        })
                        .fold(f64::INFINITY, f64::min);
                    prop_assert_eq!(entry.raw_score(), RawScore::Lexical(best));
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn declared_lanes_hold_one_ranking_per_lane_in_fixed_order() {
    let admitted = DeclaredLanes::admit([
        LaneRanking::consolidate(Lane::Dense, VERSION, []).unwrap(),
        exact([]),
    ])
    .unwrap();
    let lanes: Vec<Lane> = admitted.rankings().map(LaneRanking::lane).collect();
    assert_eq!(lanes, vec![Lane::Exact, Lane::Dense]);
    assert!(admitted.lane(Lane::Lexical).is_none());
    assert_eq!(admitted.lane(Lane::Dense).unwrap().lane(), Lane::Dense);
    assert_eq!(DeclaredLanes::admit([]).unwrap().rankings().count(), 0);

    assert_eq!(
        DeclaredLanes::admit([exact([]), exact([synthetic(1)])]).unwrap_err(),
        IdentityRefusal::DuplicateLane(Lane::Exact)
    );
    assert_eq!(Lane::ORDER.map(Lane::code), ["exact", "lexical", "dense"]);
    assert!(Lane::ORDER.is_sorted());
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
    for token in ["", "a\u{0}b", &"x".repeat(MAX_IDENTITY_VALUE_BYTES + 1)] {
        assert_eq!(
            InvocationId::parse(token).unwrap_err(),
            IdentityRefusal::MalformedToken
        );
    }
    let generation = VectorGeneration {
        generation_id: "gen-1".to_string(),
        embedding_model: "m".to_string(),
        tokenizer_fingerprint: "t".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    };
    assert_eq!(
        GenerationId::try_from(&generation).unwrap().as_str(),
        "gen-1"
    );
    let malformed = VectorGeneration {
        generation_id: String::new(),
        ..generation
    };
    assert_eq!(
        GenerationId::try_from(&malformed).unwrap_err(),
        IdentityRefusal::MalformedToken
    );
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
fn selected_spans_normalize_the_whole_buffer_and_refuse_malformed_ranges() {
    let a = synthetic(0x0a);
    let whole = SelectedSpan::new(a, None, 10).unwrap();
    assert_eq!(SelectedSpan::new(a, range(0, 10), 10).unwrap(), whole);
    assert_eq!(whole.span(), None);
    assert_eq!(
        SelectedSpan::new(a, range(0, 9), 10).unwrap().span(),
        range(0, 9)
    );
    for malformed in [range(5, 4), range(0, 11), range(11, 11)] {
        assert_eq!(
            SelectedSpan::new(a, malformed, 10).unwrap_err(),
            IdentityRefusal::MalformedSpan
        );
    }
}

#[test]
fn preparation_digest_tracks_every_component_and_never_merges_component_splits() {
    let (a, b) = (synthetic(0x0a), synthetic(0x0b));
    let selection = SelectionDigest::derive(&[a, b]);
    let selected = |occurrence, span| SelectedSpan::new(occurrence, span, 20).unwrap();
    let context = ContextRevision::parse("rev-7").unwrap();
    let text = ContextRepresentation::parse("message-entry").unwrap();
    let inputs = PreparationInputs {
        context: &context,
        representation: &text,
        spans: &[selected(a, None), selected(b, range(3, 9))],
        selection: &selection,
    };
    let base = PreparationDigest::derive(inputs);
    assert_eq!(base, PreparationDigest::derive(inputs));

    let other_context = ContextRevision::parse("rev-8").unwrap();
    let other_surface = ContextRepresentation::parse("system-prompt").unwrap();
    let other_selection = SelectionDigest::derive(&[b, a]);
    let changed = [
        PreparationInputs {
            context: &other_context,
            ..inputs
        },
        PreparationInputs {
            representation: &other_surface,
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

    let split_left = (
        ContextRevision::parse("ab").unwrap(),
        ContextRepresentation::parse("c").unwrap(),
    );
    let split_right = (
        ContextRevision::parse("a").unwrap(),
        ContextRepresentation::parse("bc").unwrap(),
    );
    assert_ne!(
        PreparationDigest::derive(PreparationInputs {
            context: &split_left.0,
            representation: &split_left.1,
            spans: &[],
            selection: &selection,
        }),
        PreparationDigest::derive(PreparationInputs {
            context: &split_right.0,
            representation: &split_right.1,
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
    assert_eq!(whole.group().revision(), 3);
    assert_eq!(
        whole.group().parent().to_string(),
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
    assert_eq!(later.group().parent(), whole.group().parent());

    let sources = [&whole, &head, &tail, &later, &other_representation];
    let occurrences: BTreeSet<OccurrenceId> = sources.iter().map(|s| s.id()).collect();
    assert_eq!(occurrences.len(), 5);
    let parents: BTreeSet<ParentId> = sources.iter().map(|s| s.group().parent()).collect();
    for parent in parents {
        assert!(!occurrences.contains(&OccurrenceId::parse(&parent.to_string()).unwrap()));
    }
    assert_eq!(exact(sources.iter().map(|s| s.id())).entries().len(), 5);

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
    for index in 0..encoded.tuple.len() {
        let mut damaged = encoded.tuple.clone();
        damaged[index] ^= 1;
        let derived = ParentGroupKey::derive(
            &damaged,
            encoded.class,
            encoded.revision,
            "decision_summary",
            encoded.span,
        );
        assert_ne!(derived, Ok(head.group()), "byte {index}");
    }
}

#[test]
fn identities_add_no_authority_beyond_byte_equality_with_the_route_binding() {
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
    let laundered = OccurrenceId::parse(&bound).unwrap();
    assert_eq!(laundered.to_string(), bound);

    runner()
        .run(
            &(
                digest_hex(),
                prop::collection::vec(any::<u8>(), 0..80),
                prop::collection::vec(synthetic_id(), 0..6),
            ),
            |(hex, bytes, selection)| {
                let text = String::from_utf8_lossy(&bytes);
                let id = OccurrenceId::parse(&hex).unwrap();
                prop_assert_eq!(id.to_string(), hex.clone());
                prop_assert_eq!(id.as_bytes().len(), 32);
                let values = [
                    Ok(hex),
                    InvocationId::parse(&text).map(|t| t.to_string()),
                    ContextRevision::parse(&text).map(|t| t.to_string()),
                    ContextRepresentation::parse(&text).map(|t| t.to_string()),
                    GenerationId::parse(&text).map(|t| t.to_string()),
                    Ok(SelectionDigest::derive(&selection).to_string()),
                ];
                for value in values.into_iter().flatten() {
                    prop_assert_eq!(names(&value), value == bound, "{}", value);
                }
                Ok(())
            },
        )
        .unwrap();
}
