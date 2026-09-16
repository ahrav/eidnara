//! The grouping algorithm reads only selected bytes, so bytes outside
//! selected spans never enter a group.

use std::collections::HashMap;
use std::fmt;

use kernel::source_identity::Span;

use super::{Grouping, GroupingKey, SelectedOccurrence};
use crate::fusion::OccurrenceId;

/// `Debug` reports the byte length, never the selected content.
#[derive(Clone, Copy)]
pub struct Selected<'a> {
    pub row: &'a SelectedOccurrence,
    pub bytes: &'a [u8],
}

impl fmt::Debug for Selected<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Selected")
            .field("row", &self.row)
            .field("byte_length", &self.bytes.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ungrouped {
    /// Only an explicit span can be empty; an empty whole-object row is its
    /// own group.
    EmptySpan,
    SpanOverflow,
    /// Every span of the disagreeing run is refused.
    OverlapDisagreement,
    Utf8Boundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GroupIdentity {
    Grouped(GroupingKey),
    Single(OccurrenceId),
}

impl GroupIdentity {
    pub fn of(row: &SelectedOccurrence) -> Self {
        match &row.grouping {
            Grouping::Grouped(key) => Self::Grouped(key.clone()),
            Grouping::NonGrouping(_) => Self::Single(row.occurrence),
        }
    }
}

/// One maximal run of overlapping or adjacent spans, in canonical offset
/// order within its group. `Debug` reports the byte length, never the
/// selected content.
#[derive(Clone, PartialEq, Eq)]
pub struct MergedRange {
    pub span: Span,
    pub bytes: Vec<u8>,
    pub members: Vec<OccurrenceId>,
}

impl fmt::Debug for MergedRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MergedRange")
            .field("span", &self.span)
            .field("byte_length", &self.bytes.len())
            .field("members", &self.members)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub identity: GroupIdentity,
    /// The position, in the grouped input, of the earliest member that landed
    /// in a range; refused spans do not advance a group.
    pub first_fused: usize,
    pub ranges: Vec<MergedRange>,
}

impl Group {
    pub fn members(&self) -> impl Iterator<Item = OccurrenceId> + '_ {
        self.ranges
            .iter()
            .flat_map(|range| range.members.iter().copied())
    }
}

/// Every selected identity appears exactly once: in one group or in `refused`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub groups: Vec<Group>,
    pub refused: Vec<(OccurrenceId, Ungrouped)>,
}

struct Member<'a> {
    fused: usize,
    occurrence: OccurrenceId,
    start: u64,
    end: u64,
    whole_buffer: bool,
    bytes: &'a [u8],
}

/// Two spans of one key are adjacent when the first ends where the second
/// starts; groups keep the fused order of their first constituent.
pub fn group(selected: &[Selected<'_>]) -> Partition {
    let mut order: Vec<GroupIdentity> = Vec::new();
    let mut buckets: Vec<Vec<Member<'_>>> = Vec::new();
    let mut index: HashMap<GroupIdentity, usize> = HashMap::new();
    let mut refused = Vec::new();
    for (fused, item) in selected.iter().enumerate() {
        let identity = GroupIdentity::of(item.row);
        let (start, end) = match item.row.span {
            None => (0, item.row.payload.byte_length),
            Some(span) => (span.start, span.end),
        };
        let slot = *index.entry(identity.clone()).or_insert_with(|| {
            order.push(identity);
            buckets.push(Vec::new());
            order.len() - 1
        });
        buckets[slot].push(Member {
            fused,
            occurrence: item.row.occurrence,
            start,
            end,
            whole_buffer: item.row.span.is_none(),
            bytes: item.bytes,
        });
    }
    let mut groups = Vec::with_capacity(order.len());
    for (identity, members) in order.into_iter().zip(buckets) {
        let mut fused: HashMap<OccurrenceId, usize> = HashMap::new();
        for member in &members {
            fused.entry(member.occurrence).or_insert(member.fused);
        }
        let ranges = merge(members, &mut refused);
        let first_fused = ranges
            .iter()
            .flat_map(|range| &range.members)
            .map(|member| fused[member])
            .min();
        if let Some(first_fused) = first_fused {
            groups.push(Group {
                identity,
                first_fused,
                ranges,
            });
        }
    }
    groups.sort_by_key(|group| group.first_fused);
    Partition { groups, refused }
}

fn merge(
    mut members: Vec<Member<'_>>,
    refused: &mut Vec<(OccurrenceId, Ungrouped)>,
) -> Vec<MergedRange> {
    members.retain(|member| {
        let reason = if member.end < member.start {
            Some(Ungrouped::SpanOverflow)
        } else if member.end == member.start && !member.whole_buffer {
            Some(Ungrouped::EmptySpan)
        } else if member.end - member.start != member.bytes.len() as u64 {
            Some(Ungrouped::SpanOverflow)
        } else {
            None
        };
        if let Some(reason) = reason {
            refused.push((member.occurrence, reason));
        }
        reason.is_none()
    });
    let whole_length = members
        .iter()
        .find(|member| member.whole_buffer)
        .map(|member| member.end);
    if let Some(length) = whole_length {
        members.retain(|member| {
            let beyond = member.end > length;
            if beyond {
                refused.push((member.occurrence, Ungrouped::SpanOverflow));
            }
            !beyond
        });
    }
    members.sort_by_key(|member| (member.start, member.end, member.occurrence));

    let mut runs: Vec<(MergedRange, bool)> = Vec::new();
    for member in members {
        match runs
            .last_mut()
            .filter(|(current, _)| member.start <= current.span.end)
        {
            Some((current, disagreed)) => {
                let overlap = (current.span.end - member.start) as usize;
                let (shared, tail) = member.bytes.split_at(overlap.min(member.bytes.len()));
                let offset = (member.start - current.span.start) as usize;
                if current.bytes[offset..offset + shared.len()] != *shared {
                    *disagreed = true;
                }
                current.bytes.extend_from_slice(tail);
                current.span.end = current.span.end.max(member.end);
                current.members.push(member.occurrence);
            }
            None => runs.push((
                MergedRange {
                    span: Span {
                        start: member.start,
                        end: member.end,
                    },
                    bytes: member.bytes.to_vec(),
                    members: vec![member.occurrence],
                },
                false,
            )),
        }
    }
    let mut ranges = Vec::with_capacity(runs.len());
    for run in runs {
        finish(run, &mut ranges, refused);
    }
    ranges
}

fn finish(
    (range, disagreed): (MergedRange, bool),
    ranges: &mut Vec<MergedRange>,
    refused: &mut Vec<(OccurrenceId, Ungrouped)>,
) {
    let reason = if disagreed {
        Some(Ungrouped::OverlapDisagreement)
    } else if std::str::from_utf8(&range.bytes).is_err() {
        Some(Ungrouped::Utf8Boundary)
    } else {
        None
    };
    match reason {
        Some(reason) => refused.extend(range.members.iter().map(|member| (*member, reason))),
        None => ranges.push(range),
    }
}
