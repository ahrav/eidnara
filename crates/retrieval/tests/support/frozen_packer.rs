//! Reference packer independently encodes `RULE_TEXT` so differential tests
//! detect production-packer drift.

use std::collections::BTreeMap;

pub const VERSION: &str = "packing-reference-v1";

pub const RULE_TEXT: &str = "\
Grouping: partition selected spans by grouping key; whole-object classes are \
their own group. Within a key, refuse an empty span, a span whose length is not \
its payload length or that reaches past a whole-buffer sibling that itself \
passed those checks. Sort by (start, end); two spans of one key never tie. Two \
spans merge when the second starts at or before the first ends; merged bytes are \
the first's bytes followed by the second's bytes after the overlap. A run whose \
overlapping bytes disagree is refused whole, as is a run whose bytes are not \
UTF-8. Ranges keep offset order; groups keep the fused order of their earliest \
member that landed in a range.\n\
Scan: visit groups once in fused order; admit a group when its cost is at most \
the remaining budget and subtract it; otherwise skip it and continue; the \
remaining budget after the last group is success.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefSpan {
    pub id: u64,
    pub key: u64,
    pub fused: usize,
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub whole_buffer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRange {
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub members: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefGroup {
    pub key: u64,
    pub first_fused: usize,
    pub ranges: Vec<RefRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefReason {
    Empty,
    Overflow,
    Disagreement,
    Utf8,
}

/// Builds a byte-coverage map per key instead of a running merge: every valid
/// span writes its bytes at its offsets; contiguous covered offsets form one
/// range; a range whose offsets were written with two different bytes is
/// refused.
pub fn group(spans: &[RefSpan]) -> (Vec<RefGroup>, Vec<(u64, RefReason)>) {
    let mut by_key: BTreeMap<u64, Vec<&RefSpan>> = BTreeMap::new();
    for span in spans {
        by_key.entry(span.key).or_default().push(span);
    }
    let mut refused = Vec::new();
    let mut groups = Vec::new();
    for (key, members) in by_key {
        let mut valid = Vec::new();
        for span in members {
            if span.end <= span.start {
                refused.push((span.id, RefReason::Empty));
            } else if span.end - span.start != span.bytes.len() as u64 {
                refused.push((span.id, RefReason::Overflow));
            } else {
                valid.push(span);
            }
        }
        if let Some(length) = valid
            .iter()
            .find(|span| span.whole_buffer)
            .map(|span| span.end)
        {
            let (kept, beyond): (Vec<_>, Vec<_>) =
                valid.into_iter().partition(|span| span.end <= length);
            refused.extend(beyond.iter().map(|span| (span.id, RefReason::Overflow)));
            valid = kept;
        }
        let mut coverage: BTreeMap<u64, (u8, bool)> = BTreeMap::new();
        for span in &valid {
            for (offset, byte) in (span.start..span.end).zip(&span.bytes) {
                let cell = coverage.entry(offset).or_insert((*byte, false));
                if cell.0 != *byte {
                    cell.1 = true;
                }
            }
        }
        let mut ranges: Vec<(RefRange, bool)> = Vec::new();
        for (offset, (byte, conflict)) in coverage {
            match ranges.last_mut() {
                Some((range, disagreed)) if range.end == offset => {
                    range.end += 1;
                    range.bytes.push(byte);
                    *disagreed |= conflict;
                }
                _ => ranges.push((
                    RefRange {
                        start: offset,
                        end: offset + 1,
                        bytes: vec![byte],
                        members: Vec::new(),
                    },
                    conflict,
                )),
            }
        }
        valid.sort_by_key(|span| (span.start, span.end, span.id));
        for span in &valid {
            let (range, _) = ranges
                .iter_mut()
                .find(|(range, _)| range.start <= span.start && span.start < range.end)
                .unwrap();
            range.members.push(span.id);
        }
        let mut kept = Vec::new();
        for (range, disagreed) in ranges {
            let reason = if disagreed {
                Some(RefReason::Disagreement)
            } else if std::str::from_utf8(&range.bytes).is_err() {
                Some(RefReason::Utf8)
            } else {
                None
            };
            match reason {
                Some(reason) => refused.extend(range.members.iter().map(|id| (*id, reason))),
                None => kept.push(range),
            }
        }
        let first_fused = kept
            .iter()
            .flat_map(|range| &range.members)
            .map(|id| spans.iter().find(|span| span.id == *id).unwrap().fused)
            .min();
        if let Some(first_fused) = first_fused {
            groups.push(RefGroup {
                key,
                first_fused,
                ranges: kept,
            });
        }
    }
    groups.sort_by_key(|group| group.first_fused);
    (groups, refused)
}

pub fn scan(costs: &[u64], budget: u64) -> (Vec<usize>, Vec<usize>, u64) {
    let mut remaining = budget;
    let mut admitted = Vec::new();
    let mut skipped = Vec::new();
    for (index, cost) in costs.iter().enumerate() {
        if *cost <= remaining {
            remaining -= cost;
            admitted.push(index);
        } else {
            skipped.push(index);
        }
    }
    (admitted, skipped, remaining)
}

pub fn prefix_scan(costs: &[u64], budget: u64) -> (Vec<usize>, u64) {
    let mut remaining = budget;
    let mut admitted = Vec::new();
    for (index, cost) in costs.iter().enumerate() {
        if *cost > remaining {
            break;
        }
        remaining -= cost;
        admitted.push(index);
    }
    (admitted, remaining)
}

/// The gap-filling merger: one range over the parent from the first start to
/// the last end.
pub fn gap_filling_merge(parent: &str, start: u64, end: u64) -> Vec<u8> {
    parent.as_bytes()[start as usize..end as usize].to_vec()
}
