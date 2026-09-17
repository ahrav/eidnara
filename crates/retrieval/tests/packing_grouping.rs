//! These tests verify grouping laws against an oracle parent that the
//! production packer never reads.

mod support;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;

use kernel::Sensitivity;
use kernel::source_identity::{Occurrence, OccurrenceClass, Span, encode};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestCaseError, TestRng, TestRunner};
use retrieval::fusion::OccurrenceId;
use retrieval::packing::{
    Grouping, GroupingKey, OptionalBound, OptionalBounds, PayloadRef, Provenance, Selected,
    SelectedOccurrence, Ungrouped, admit_fused_candidates, admit_optional_set, group,
    skip_and_continue,
};
use serde_json::Value;
use support::frozen_packer::{self, RefReason, RefSpan};

const SEED: [u8; 32] = *b"packing-grouping-laws-seed-00001";

fn runner(cases: u32) -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    )
}

fn tool_span(
    call: &str,
    revision: &str,
    representation: &str,
    parent: &str,
    range: Option<(u64, u64)>,
) -> (SelectedOccurrence, Vec<u8>) {
    let identity = [
        ("project_id", "proj-a"),
        ("harness", "opencode"),
        ("session_id", "sess-01"),
        ("parent_message_id", "msg-002"),
        ("tool_call_id", call),
        ("result_revision", "1"),
        ("block_index", "0"),
    ];
    row(
        OccurrenceClass::RawToolSpans,
        &identity,
        call,
        revision,
        representation,
        parent,
        range,
    )
}

fn promoted_memory(decision: &str, payload: &str) -> (SelectedOccurrence, Vec<u8>) {
    let identity = [("decision_object_id", decision)];
    row(
        OccurrenceClass::PromotedMemory,
        &identity,
        decision,
        "1",
        "summary",
        payload,
        None,
    )
}

fn row(
    class: OccurrenceClass,
    identity: &[(&str, &str)],
    source_object_id: &str,
    revision: &str,
    representation: &str,
    parent: &str,
    range: Option<(u64, u64)>,
) -> (SelectedOccurrence, Vec<u8>) {
    let span = range.map(|(start, end)| Span { start, end });
    let encoded = encode(
        &Occurrence {
            class: class.code(),
            identity,
            revision,
            representation,
            span,
        },
        parent,
    )
    .unwrap();
    let bytes = match encoded.span {
        Some(span) => parent.as_bytes()[span.start as usize..span.end as usize].to_vec(),
        None => parent.as_bytes().to_vec(),
    };
    let row = SelectedOccurrence {
        occurrence: OccurrenceId::parse(&encoded.occurrence_id).unwrap(),
        class,
        revision: encoded.revision,
        representation: representation.to_owned(),
        span: encoded.span,
        payload: PayloadRef {
            payload_id: kernel::source_identity::payload_id(&bytes),
            byte_length: bytes.len() as u64,
        },
        sensitivity: Sensitivity::Normal,
        provenance: Provenance {
            domain_id: "d".to_owned(),
            source_object_id: source_object_id.to_owned(),
            source_evidence_id: "e".to_owned(),
            source_artifact_digest: "0".repeat(64),
            created_commit_seq: 1,
        },
        tombstone: None,
        grouping: Grouping::derive(
            &encoded.tuple,
            class,
            encoded.revision,
            representation,
            encoded.span,
        )
        .unwrap(),
    };
    (row, bytes)
}

/// `seed_end` picks a distinct well-formed span to mint the identifier from.
fn damaged_span(
    call: &str,
    parent: &str,
    seed_end: u64,
    start: u64,
    end: u64,
    bytes: Vec<u8>,
) -> (SelectedOccurrence, Vec<u8>) {
    let (mut row, _) = tool_span(call, "1", "tool_output", parent, Some((0, seed_end)));
    row.span = Some(Span { start, end });
    row.payload.byte_length = bytes.len() as u64;
    (row, bytes)
}

fn selected<'a>(rows: &'a [(SelectedOccurrence, Vec<u8>)]) -> Vec<Selected<'a>> {
    rows.iter()
        .map(|(row, bytes)| Selected { row, bytes })
        .collect()
}

/// The no-gap oracle: the bytes a grouping carries must equal the selected
/// byte union, so a merger that reads the parent across a gap fails it.
fn no_gap_filled(carried_bytes: usize, selected_union: usize) -> Result<(), String> {
    if carried_bytes == selected_union {
        Ok(())
    } else {
        Err(format!(
            "carried {carried_bytes} bytes for {selected_union} selected"
        ))
    }
}

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packing/groups.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn oracle_parent_groups_match_the_expected_tables_without_a_parent_read() {
    let fixture = fixture();
    assert_eq!(fixture["reference_version"], frozen_packer::VERSION);
    let parent = fixture["parent"].as_str().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let rows: Vec<_> = case["spans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|span| {
                let start = span["start"].as_u64().unwrap();
                let end = span["end"].as_u64().unwrap();
                tool_span("call-1", "1", "tool_output", parent, Some((start, end)))
            })
            .collect();
        let ids: BTreeMap<&str, OccurrenceId> = case["spans"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&rows)
            .map(|(span, (row, _))| (span["id"].as_str().unwrap(), row.occurrence))
            .collect();
        let partition = group(&selected(&rows));
        let expected_refused = case["expected_refused"].as_array().unwrap();
        assert_eq!(partition.refused.len(), expected_refused.len(), "{name}");
        let expected = case["expected_groups"].as_array().unwrap();
        assert_eq!(partition.groups.len(), expected.len(), "{name}");
        for (group, expected) in partition.groups.iter().zip(expected) {
            let ranges = expected["ranges"].as_array().unwrap();
            assert_eq!(group.ranges.len(), ranges.len(), "{name}");
            for (range, expected) in group.ranges.iter().zip(ranges) {
                let start = expected["start"].as_u64().unwrap();
                let end = expected["end"].as_u64().unwrap();
                assert_eq!((range.span.start, range.span.end), (start, end), "{name}");
                assert_eq!(
                    range.bytes,
                    parent.as_bytes()[start as usize..end as usize],
                    "{name}: merged bytes equal the parent bytes for the merged range"
                );
                let members: Vec<OccurrenceId> = expected["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| ids[id.as_str().unwrap()])
                    .collect();
                assert_eq!(range.members, members, "{name}");
                assert!(std::str::from_utf8(&range.bytes).is_ok(), "{name}");
            }
        }
        let grouped: usize = partition
            .groups
            .iter()
            .flat_map(|group| &group.ranges)
            .map(|range| range.bytes.len())
            .sum();
        let selected_union: BTreeSet<u64> = rows
            .iter()
            .flat_map(|(row, _)| {
                let span = row.span.unwrap();
                span.start..span.end
            })
            .collect();
        assert_eq!(
            no_gap_filled(grouped, selected_union.len()),
            Ok(()),
            "{name}"
        );
        if case["gap_contains_multibyte"].as_bool() == Some(true) {
            let first = partition.groups[0].ranges[0].span;
            let last = partition.groups[0].ranges[1].span;
            let gap_filled = frozen_packer::gap_filling_merge(parent, first.start, last.end);
            assert!(
                no_gap_filled(gap_filled.len(), selected_union.len()).is_err(),
                "{name}: the gap-filling merger must fail the no-gap oracle"
            );
            assert!(
                parent[first.end as usize..last.start as usize]
                    .chars()
                    .any(|c| c.len_utf8() > 1)
            );
        }
    }
}

#[test]
fn spans_of_another_revision_or_representation_never_merge_and_groups_follow_fused_order() {
    let parent = "0123456789abcdefghij";
    let rows = vec![
        tool_span("call-2", "1", "tool_output", parent, Some((0, 5))),
        tool_span("call-1", "1", "tool_output", parent, Some((0, 5))),
        tool_span("call-1", "2", "tool_output", parent, Some((3, 8))),
        tool_span("call-1", "1", "tool_error", parent, Some((3, 8))),
        tool_span("call-1", "1", "tool_output", parent, Some((3, 8))),
    ];
    let partition = group(&selected(&rows));
    assert!(partition.refused.is_empty());
    let first_fused: Vec<usize> = partition.groups.iter().map(|g| g.first_fused).collect();
    assert_eq!(first_fused, vec![0, 1, 2, 3]);
    let sizes: Vec<usize> = partition
        .groups
        .iter()
        .map(|group| group.members().count())
        .collect();
    assert_eq!(
        sizes,
        vec![1, 2, 1, 1],
        "only same revision and representation merge"
    );
    assert_eq!(partition.groups[1].ranges.len(), 1);
    assert_eq!(partition.groups[1].ranges[0].bytes, b"01234567");
}

#[test]
fn every_selected_span_is_grouped_or_carries_a_typed_reason() {
    let parent = "héllo wörld";
    let empty = damaged_span("call-1", parent, 1, 3, 3, Vec::new());
    let overflow = damaged_span("call-1", parent, 3, 0, 4, parent.as_bytes()[0..3].to_vec());
    let whole = tool_span("call-1", "1", "tool_output", parent, None);
    let beyond = damaged_span(
        "call-1",
        parent,
        4,
        parent.len() as u64,
        parent.len() as u64 + 3,
        b"abc".to_vec(),
    );
    let agree_a = tool_span("call-3", "1", "tool_output", parent, Some((0, 6)));
    let disagree_b = damaged_span("call-3", parent, 1, 3, 9, b"XXXrld".to_vec());
    let split_a = damaged_span("call-4", parent, 1, 0, 2, parent.as_bytes()[0..2].to_vec());
    let split_b = damaged_span("call-5", parent, 3, 2, 4, parent.as_bytes()[2..4].to_vec());
    let rejoined_a = damaged_span("call-6", parent, 1, 0, 2, parent.as_bytes()[0..2].to_vec());
    let rejoined_b = damaged_span("call-6", parent, 3, 2, 4, parent.as_bytes()[2..4].to_vec());
    let reversed = damaged_span("call-1", parent, 5, 4, 2, Vec::new());
    assert!(std::str::from_utf8(&split_a.1).is_err());

    let rows = vec![
        empty, overflow, whole, beyond, agree_a, disagree_b, split_a, split_b, rejoined_a,
        rejoined_b, reversed,
    ];
    let partition = group(&selected(&rows));
    let mut seen: BTreeMap<OccurrenceId, Option<Ungrouped>> = BTreeMap::new();
    for group in &partition.groups {
        for member in group.members() {
            assert!(seen.insert(member, None).is_none(), "member in two groups");
        }
    }
    for (occurrence, reason) in &partition.refused {
        assert!(
            seen.insert(*occurrence, Some(*reason)).is_none(),
            "refused twice"
        );
    }
    let all: BTreeSet<OccurrenceId> = rows.iter().map(|(row, _)| row.occurrence).collect();
    assert_eq!(seen.keys().copied().collect::<BTreeSet<_>>(), all);
    let expected = [
        Some(Ungrouped::EmptySpan),
        Some(Ungrouped::SpanOverflow),
        None,
        Some(Ungrouped::SpanOverflow),
        Some(Ungrouped::OverlapDisagreement),
        Some(Ungrouped::OverlapDisagreement),
        Some(Ungrouped::Utf8Boundary),
        Some(Ungrouped::Utf8Boundary),
        None,
        None,
        Some(Ungrouped::SpanOverflow),
    ];
    for ((row, _), expected) in rows.iter().zip(expected) {
        assert_eq!(seen[&row.occurrence], expected, "{:?}", row.span);
    }
    let rejoined = partition
        .groups
        .iter()
        .find(|group| group.first_fused == 8)
        .unwrap();
    assert_eq!(rejoined.ranges.len(), 1);
    assert_eq!(rejoined.ranges[0].bytes, parent.as_bytes()[0..4]);
}

#[test]
fn the_scan_skips_and_continues_and_the_prefix_packer_does_not() {
    let costs = [11u64, 4, 6];
    let scan = skip_and_continue(10u64, costs.len(), |_, i| costs[i]);
    assert_eq!(scan.admitted, vec![1, 2]);
    assert_eq!(scan.skipped, vec![0]);
    assert_eq!(scan.remaining, 0);
    assert_eq!(
        frozen_packer::scan(&costs, 10),
        (vec![1, 2], vec![0], 0),
        "the frozen reference agrees"
    );
    let (prefix, left) = frozen_packer::prefix_scan(&costs, 10);
    assert_eq!(prefix, Vec::<usize>::new());
    assert_eq!(left, 10);
    assert_ne!(
        prefix, scan.admitted,
        "the prefix-packer negative control diverges"
    );

    let unused = skip_and_continue(10u64, 2, |_, i| [3u64, 3][i]);
    assert_eq!(unused.remaining, 4, "unused budget is success");
    let mut visits = 0;
    skip_and_continue(10u64, 5, |_, _| {
        visits += 1;
        1u64
    });
    assert_eq!(visits, 5, "each item is visited once");
}

#[test]
fn optional_bounds_saturate_at_their_limit_and_refuse_at_limit_plus_one() {
    let parent = "0123456789abcdefghij";
    let rows: Vec<_> = (0..4)
        .map(|i| {
            let call = format!("call-{}", i / 2);
            tool_span(&call, "1", "tool_output", parent, Some((i * 5, i * 5 + 5)))
        })
        .collect();
    let refs: Vec<&SelectedOccurrence> = rows.iter().map(|(row, _)| row).collect();
    let wide = OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(4).unwrap(),
        max_parents: NonZeroUsize::new(2).unwrap(),
        max_spans_per_parent: NonZeroUsize::new(2).unwrap(),
        max_payload_loads: NonZeroUsize::new(4).unwrap(),
        max_payload_bytes: NonZeroU64::new(20).unwrap(),
        max_item_bytes: NonZeroU64::new(5).unwrap(),
    };
    assert_eq!(admit_optional_set(refs.iter().copied(), &wide), Ok(()));
    assert_eq!(admit_fused_candidates(4, &wide), Ok(()));
    let three = OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(3).unwrap(),
        ..wide
    };
    let exceeded = admit_fused_candidates(4, &three).unwrap_err();
    assert_eq!(
        (exceeded.bound, exceeded.at),
        (OptionalBound::FusedCandidates, 3)
    );
    let cases = [
        (
            OptionalBounds {
                max_parents: NonZeroUsize::MIN,
                ..wide
            },
            OptionalBound::Parents,
            2,
        ),
        (
            OptionalBounds {
                max_spans_per_parent: NonZeroUsize::MIN,
                ..wide
            },
            OptionalBound::SpansPerParent,
            1,
        ),
        (
            OptionalBounds {
                max_payload_loads: NonZeroUsize::new(3).unwrap(),
                ..wide
            },
            OptionalBound::PayloadLoads,
            3,
        ),
        (
            OptionalBounds {
                max_payload_bytes: NonZeroU64::new(19).unwrap(),
                ..wide
            },
            OptionalBound::PayloadBytes,
            3,
        ),
        (
            OptionalBounds {
                max_item_bytes: NonZeroU64::new(4).unwrap(),
                ..wide
            },
            OptionalBound::ItemBytes,
            0,
        ),
    ];
    for (bounds, bound, at) in cases {
        let exceeded = admit_optional_set(refs.iter().copied(), &bounds).unwrap_err();
        assert_eq!((exceeded.bound, exceeded.at), (bound, at), "{bound:?}");
    }
}

/// `id` is the sorted rank of `row.occurrence`.
fn ref_spans(
    rows: &[(SelectedOccurrence, Vec<u8>)],
) -> (Vec<RefSpan>, BTreeMap<u64, OccurrenceId>) {
    #[derive(PartialEq, Eq, Hash)]
    enum RefKey {
        Grouped(GroupingKey),
        Single(OccurrenceId),
    }
    let mut ordered: Vec<OccurrenceId> = rows.iter().map(|(row, _)| row.occurrence).collect();
    ordered.sort();
    ordered.dedup();
    let ids: BTreeMap<u64, OccurrenceId> = ordered
        .iter()
        .enumerate()
        .map(|(rank, occurrence)| (rank as u64, *occurrence))
        .collect();
    let mut keys: HashMap<RefKey, u64> = HashMap::new();
    let spans = rows
        .iter()
        .enumerate()
        .map(|(fused, (row, bytes))| {
            let ref_key = match &row.grouping {
                Grouping::Grouped(grouping) => RefKey::Grouped(grouping.clone()),
                Grouping::NonGrouping(_) => RefKey::Single(row.occurrence),
            };
            let next = keys.len() as u64;
            let key = *keys.entry(ref_key).or_insert(next);
            let id = ordered.binary_search(&row.occurrence).unwrap() as u64;
            let (start, end) = match row.span {
                Some(span) => (span.start, span.end),
                None => (0, row.payload.byte_length),
            };
            RefSpan {
                id,
                key,
                fused,
                start,
                end,
                bytes: bytes.clone(),
                whole_buffer: row.span.is_none(),
            }
        })
        .collect();
    (spans, ids)
}

#[test]
fn production_grouping_and_scan_never_diverge_from_the_frozen_reference() {
    let parent = "The quick brown fox jumps över the lazy dög, twice.";
    let boundaries: Vec<u64> = parent
        .char_indices()
        .map(|(i, _)| i as u64)
        .chain([parent.len() as u64])
        .collect();
    let span = {
        let boundaries = boundaries.clone();
        (0..boundaries.len(), 0..boundaries.len()).prop_map(move |(a, b)| {
            let (a, b) = (a.min(b), a.max(b));
            (boundaries[a], boundaries[b])
        })
    };
    let set = proptest::collection::vec(
        (
            0u8..3,
            0u8..2,
            span,
            proptest::bool::weighted(0.15),
            0u8..8,
            proptest::bool::weighted(0.1),
        ),
        0..8,
    );
    let costs = proptest::collection::vec(0u64..12, 0..8);
    runner(512)
        .run(&(set, costs, 0u64..20), |(set, costs, budget)| {
            let rows: Vec<_> = set
                .iter()
                .enumerate()
                .map(
                    |(index, (call, revision, (start, end), whole, damage, object))| {
                        let call = format!("call-{call}");
                        let revision = if *revision == 0 { "1" } else { "2" };
                        let seed_end = boundaries[1 + index % (boundaries.len() - 1)];
                        let slice =
                            |end: u64| parent.as_bytes()[*start as usize..end as usize].to_vec();
                        if *object {
                            promoted_memory(&format!("decision-{call}"), &parent[..*end as usize])
                        } else if *whole {
                            tool_span(&call, revision, "tool_output", parent, None)
                        } else if start == end {
                            damaged_span(&call, parent, seed_end, *start, *end, Vec::new())
                        } else if *damage == 4 {
                            damaged_span(&call, parent, seed_end, *end, *start, Vec::new())
                        } else if *damage == 5 {
                            let mut bytes = slice(*end);
                            bytes[0] ^= 0x01;
                            damaged_span(&call, parent, seed_end, *start, *end, bytes)
                        } else if *damage == 6 {
                            damaged_span(&call, parent, seed_end, *start, *end + 1, slice(*end))
                        } else if *damage == 7 {
                            let end = (*end + 1).min(parent.len() as u64);
                            damaged_span(&call, parent, seed_end, *start, end, slice(end))
                        } else {
                            tool_span(&call, revision, "tool_output", parent, Some((*start, *end)))
                        }
                    },
                )
                .collect();
            grouping_agrees_with_reference(&rows)?;

            let scan = skip_and_continue(budget, costs.len(), |_, i| costs[i]);
            let (admitted, skipped, remaining) = frozen_packer::scan(&costs, budget);
            prop_assert_eq!(
                (scan.admitted, scan.skipped, scan.remaining),
                (admitted, skipped, remaining)
            );
            Ok(())
        })
        .unwrap();
}

fn grouping_agrees_with_reference(
    rows: &[(SelectedOccurrence, Vec<u8>)],
) -> Result<(), TestCaseError> {
    let production = group(&selected(rows));
    let (ref_spans, ids) = ref_spans(rows);
    let (reference_groups, reference_refused) = frozen_packer::group(&ref_spans);
    prop_assert_eq!(
        production.groups.len(),
        reference_groups.len(),
        "production {:?} reference {:?} refused {:?}",
        production,
        reference_groups,
        reference_refused
    );
    for (group, reference) in production.groups.iter().zip(&reference_groups) {
        prop_assert_eq!(group.first_fused, reference.first_fused);
        prop_assert_eq!(group.ranges.len(), reference.ranges.len());
        for (range, reference) in group.ranges.iter().zip(&reference.ranges) {
            prop_assert_eq!(
                (range.span.start, range.span.end),
                (reference.start, reference.end)
            );
            prop_assert_eq!(&range.bytes, &reference.bytes);
            let members: Vec<OccurrenceId> = reference.members.iter().map(|id| ids[id]).collect();
            prop_assert_eq!(&range.members, &members);
        }
    }
    let mut refused = production.refused.clone();
    refused.sort();
    let mut reference_refused: Vec<(OccurrenceId, Ungrouped)> = reference_refused
        .iter()
        .map(|(id, reason)| {
            (
                ids[id],
                match reason {
                    RefReason::Empty => Ungrouped::EmptySpan,
                    RefReason::Overflow => Ungrouped::SpanOverflow,
                    RefReason::Disagreement => Ungrouped::OverlapDisagreement,
                    RefReason::Utf8 => Ungrouped::Utf8Boundary,
                },
            )
        })
        .collect();
    reference_refused.sort();
    prop_assert_eq!(refused, reference_refused);
    Ok(())
}

#[test]
fn tied_offsets_of_distinct_occurrences_order_by_identifier_in_both_implementations() {
    let parent = "0123456789abcdefghij";
    let bytes = parent.as_bytes()[4..9].to_vec();
    let first = damaged_span("call-1", parent, 1, 4, 9, bytes.clone());
    let second = damaged_span("call-1", parent, 2, 4, 9, bytes.clone());
    assert_ne!(first.0.occurrence, second.0.occurrence);
    let rows = if first.0.occurrence > second.0.occurrence {
        vec![first, second]
    } else {
        vec![second, first]
    };
    let partition = group(&selected(&rows));
    assert_eq!(partition.groups.len(), 1);
    assert_eq!(partition.groups[0].ranges.len(), 1);
    let members = &partition.groups[0].ranges[0].members;
    assert_eq!(members, &vec![rows[1].0.occurrence, rows[0].0.occurrence]);
    grouping_agrees_with_reference(&rows).unwrap();
}

#[test]
fn whole_objects_of_one_class_are_their_own_group_in_both_implementations() {
    let rows = vec![
        promoted_memory("decision-a", "alpha"),
        promoted_memory("decision-b", "alpha"),
        promoted_memory("decision-c", "beta"),
        promoted_memory("decision-d", ""),
    ];
    let partition = group(&selected(&rows));
    assert_eq!(partition.groups.len(), 4);
    assert!(partition.refused.is_empty());
    let empty = &partition.groups[3];
    assert_eq!(empty.ranges.len(), 1);
    assert_eq!(empty.ranges[0].span, Span { start: 0, end: 0 });
    assert!(empty.ranges[0].bytes.is_empty());
    assert_eq!(empty.ranges[0].members, vec![rows[3].0.occurrence]);
    grouping_agrees_with_reference(&rows).unwrap();
}

/// The crate promises that payloads are never logged; a group carries the
/// selected bytes, so its `Debug` names their length, not their content.
#[test]
fn selected_bytes_are_never_printed_by_grouping_types() {
    let secret = "the selected payload content";
    let rows = [tool_span("call-a", "1", "tool_output", secret, None)];
    let items = selected(&rows);
    let partition = group(&items);
    assert_eq!(partition.groups.len(), 1, "{partition:?}");
    let range = &partition.groups[0].ranges[0];
    for rendered in [
        format!("{:?}", items[0]),
        format!("{range:?}"),
        format!("{:?}", partition.groups[0]),
        format!("{partition:?}"),
    ] {
        assert!(
            !rendered.contains("payload content") && !rendered.contains("116, 104, 101"),
            "{rendered}"
        );
        assert!(rendered.contains(&secret.len().to_string()), "{rendered}");
    }
}
