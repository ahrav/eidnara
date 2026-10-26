use std::ops::Range;

use kernel::source_identity::{
    Occurrence, OccurrenceClass, lineage_ids_varying_last_field, lower_hex,
};
use kernel::{
    AnchorSpec, AnchorValue, DecisionEventPayload, DecisionEventSpec, Envelope, KernelError,
    Sensitivity,
};
use memory_store::memory_capture::CaptureJob;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::FrozenHead;
use crate::source_capture::Count;

/// Block indexes a message's citation lookup probes, `0..MAX_CITATION_BLOCKS`.
pub(super) const MAX_CITATION_BLOCKS: usize = 256;
pub(crate) const CITATION_EVENT_KIND: &str = "memory_capture_citation";
const CITATION_EVENT_SUMMARY: &str = "cites the captured source message block";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CitationReason {
    Cited,
    SourceCaptureOff,
    SourceIdUnjoinable,
    DescriptorNotFound,
}

impl CitationReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cited => "cited",
            Self::SourceCaptureOff => "source_capture_off",
            Self::SourceIdUnjoinable => "source_id_unjoinable",
            Self::DescriptorNotFound => "descriptor_not_found",
        }
    }

    fn count(self) -> Count {
        match self {
            Self::Cited => Count::DecisionsCited,
            Self::SourceCaptureOff => Count::DecisionsSourceCaptureOff,
            Self::SourceIdUnjoinable => Count::DecisionsSourceIdUnjoinable,
            Self::DescriptorNotFound => Count::DecisionsDescriptorNotFound,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnchorReason {
    Anchored,
    HeadUnavailable,
}

impl AnchorReason {
    pub(super) fn of(anchor: Option<&AnchorSpec>) -> Self {
        if anchor.is_some() {
            Self::Anchored
        } else {
            Self::HeadUnavailable
        }
    }

    /// Records the reason in a decision's rationale and its count.
    pub(super) fn record(self, rationale: &mut Map<String, Value>, reasons: &mut Vec<Count>) {
        let (name, count) = match self {
            Self::Anchored => ("anchored", Count::DecisionsAnchored),
            Self::HeadUnavailable => ("head_unavailable", Count::DecisionsHeadUnavailable),
        };
        rationale.insert("anchor".into(), json!(name));
        reasons.push(count);
    }
}

/// One live `messages` descriptor of the job's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Block {
    pub(super) index: usize,
    pub(super) digest: String,
    /// The byte length of the block's artifact, its item text.
    pub(super) length: u64,
    pub(super) evidence_id: String,
}

/// The rule that picked a memory's cited blocks, as the rationale records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockRule {
    /// The quote lies inside one located block.
    Block,
    /// The quote crosses adjacent located blocks; a separator it touches
    /// belongs to the blocks on both sides.
    Span,
    /// No located block holds the quote; the lowest live block is cited.
    First,
}

impl BlockRule {
    fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Span => "span",
            Self::First => "first",
        }
    }
}

/// What a job's citation lookup found: its reason, and for a cited message
/// the live blocks and the byte ranges the leading ones occupy in the job text.
pub(super) struct Citation {
    reason: CitationReason,
    blocks: Vec<Block>,
    located: Vec<Range<usize>>,
}

/// The fragment id a capture checkpoint writes for a message it cannot pass
/// through: a SHA-256 hex digest, `:`, and a canonical decimal offset.
pub(super) fn is_hashed_source_id(id: &str) -> bool {
    let Some((digest, offset)) = id.split_once(':') else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !offset.is_empty()
        && offset.bytes().all(|byte| byte.is_ascii_digit())
        && (offset == "0" || !offset.starts_with('0'))
}

/// The lineages of the job's message blocks `blocks`, ending before the first
/// block whose identity the encoder refuses, which no descriptor can hold. A
/// lineage omits the revision, so any revision encodes it.
fn message_lineages(job: &CaptureJob, blocks: Range<usize>) -> Vec<String> {
    let indexes: Vec<String> = blocks.clone().map(|block| block.to_string()).collect();
    let identity = crate::harness_sources::message_identity(
        &job.project,
        &job.harness,
        &job.session_id,
        &job.message_id,
        blocks.start,
    );
    let identity: Vec<(&str, &str)> = identity
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    lineage_ids_varying_last_field(
        &Occurrence {
            class: OccurrenceClass::Messages.code(),
            identity: &identity,
            revision: "1",
            representation: crate::harness_sources::Representation::Text.as_str(),
            span: None,
        },
        indexes.iter().map(String::as_str),
    )
}

/// The 32 bytes a lowercase hex SHA-256 digest names, or `None` for any other
/// string.
fn digest_bytes(hex: &str) -> Option<[u8; 32]> {
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let hex = hex.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    let (pairs, _) = hex.as_chunks::<2>();
    for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = (digit(*high)? << 4) | digit(*low)?;
    }
    Some(bytes)
}

#[cfg(test)]
pub(super) fn locate(text: &str, blocks: &[Block]) -> Vec<Range<usize>> {
    let mut located = Vec::new();
    for block in blocks {
        if !extend(text, &mut located, block) {
            break;
        }
    }
    located
}

/// `extend` appends the range `block` occupies in `text`, which joins the
/// message's text blocks with `\n`, after the `located` prefix, and returns
/// whether it was located. The block starts after the last located block's
/// separator and spans its artifact's byte length; it is located when that
/// range ends at a `\n` or at the end of the text and its SHA-256 equals the
/// block's artifact digest, so each text byte is hashed at most once.
fn extend(text: &str, located: &mut Vec<Range<usize>>, block: &Block) -> bool {
    let bytes = text.as_bytes();
    let start = located.last().map_or(0, |range| range.end + 1);
    let range = digest_bytes(&block.digest).and_then(|digest| {
        let end = start.checked_add(usize::try_from(block.length).ok()?)?;
        let segment = bytes.get(start..end)?;
        if bytes.get(end).is_some_and(|byte| *byte != b'\n') {
            return None;
        }
        (Sha256::digest(segment).as_slice() == digest).then_some(start..end)
    });
    range.map(|range| located.push(range)).is_some()
}

/// The blocks a memory cites: the located block holding the first occurrence
/// of `quote`, the located run it spans, or the lowest live block.
pub(super) fn target<'b>(
    text: &str,
    quote: &str,
    blocks: &'b [Block],
    located: &[Range<usize>],
) -> (BlockRule, &'b [Block]) {
    let span = text
        .find(quote)
        .map(|start| start..start + quote.len())
        .filter(|span| !span.is_empty());
    // The separator after a located block opens a quote in that block, and
    // the separator before one closes a quote in it.
    let opening = |offset: usize| {
        located
            .iter()
            .position(|range| range.start <= offset && offset <= range.end)
    };
    let closing = |offset: usize| {
        located
            .iter()
            .position(|range| range.start.saturating_sub(1) <= offset && offset < range.end)
    };
    match span.and_then(|span| Some((opening(span.start)?, closing(span.end - 1)?))) {
        Some((first, last)) if first == last => (BlockRule::Block, &blocks[first..=first]),
        Some((first, last)) if first < last => (BlockRule::Span, &blocks[first..=last]),
        _ => (BlockRule::First, &blocks[..1]),
    }
}

/// Ends of the block-index chunks a lookup reads, one statement each: 4, 12,
/// 48, and 192 indexes, so a message of a few blocks costs one read and a
/// message with no descriptor costs four.
const CHUNK_ENDS: [usize; 4] = [4, 16, 64, MAX_CITATION_BLOCKS];

/// The live blocks of indexes `0..MAX_CITATION_BLOCKS` in ascending order,
/// each located on arrival, and their located ranges. `read` returns one slot
/// per index of the chunk it is given, `None` for an index with no live
/// descriptor, and ends early at an index whose identity has no lineage.
/// Reading stops there, at a block the walk cannot locate, and at a block
/// that ends the text, after which the walk locates no block; [`target`]
/// reads only the lowest live block and the located ones.
fn read_blocks(
    text: &str,
    mut read: impl FnMut(Range<usize>) -> Result<Vec<Option<Block>>, KernelError>,
) -> Result<(Vec<Block>, Vec<Range<usize>>), KernelError> {
    let mut blocks = Vec::new();
    let mut located = Vec::new();
    let mut start = 0;
    for end in CHUNK_ENDS {
        let chunk = read(start..end)?;
        let short = chunk.len() < end - start;
        for block in chunk.into_iter().flatten() {
            let extended = extend(text, &mut located, &block);
            blocks.push(block);
            if !extended || located.last().is_some_and(|range| range.end == text.len()) {
                return Ok((blocks, located));
            }
        }
        if short {
            break;
        }
        start = end;
    }
    Ok((blocks, located))
}

impl Citation {
    /// Reads the live descriptors of the job's message inside `envelope`, so
    /// the read, the anchor, the decisions, and their citation events share
    /// one view. `consent` is the binding's source and memory consent.
    pub(super) fn lookup(
        envelope: &Envelope<'_>,
        job: &CaptureJob,
        consent: bool,
    ) -> Result<Self, KernelError> {
        let empty = |reason| Self {
            reason,
            blocks: Vec::new(),
            located: Vec::new(),
        };
        if !consent {
            return Ok(empty(CitationReason::SourceCaptureOff));
        }
        let (blocks, located) = read_blocks(&job.text, |indexes| {
            let lineages = message_lineages(job, indexes.clone());
            Ok(envelope
                .live_source_descriptors_of_lineages(OccurrenceClass::Messages, &lineages)?
                .into_iter()
                .zip(indexes)
                .map(|(row, index)| {
                    row.map(|row| Block {
                        index,
                        digest: row.detail.artifact_digest,
                        length: row.byte_length,
                        evidence_id: row.detail.evidence_id,
                    })
                })
                .collect())
        })?;
        if blocks.is_empty() {
            return Ok(empty(if is_hashed_source_id(&job.message_id) {
                CitationReason::SourceIdUnjoinable
            } else {
                CitationReason::DescriptorNotFound
            }));
        }
        Ok(Self {
            reason: CitationReason::Cited,
            blocks,
            located,
        })
    }

    /// Records one memory's citation reason and its reason count, and for a
    /// cited message the rule, the cited block indexes, and their evidence
    /// ids, which it returns in ascending block order.
    pub(super) fn cite(
        &self,
        text: &str,
        quote: &str,
        rationale: &mut Map<String, Value>,
        reasons: &mut Vec<Count>,
    ) -> Vec<String> {
        rationale.insert("citation".into(), json!(self.reason.as_str()));
        reasons.push(self.reason.count());
        if self.reason != CitationReason::Cited {
            return Vec::new();
        }
        let (rule, cited) = target(text, quote, &self.blocks, &self.located);
        let evidence: Vec<String> = cited
            .iter()
            .map(|block| block.evidence_id.clone())
            .collect();
        rationale.insert("citation_block".into(), json!(rule.as_str()));
        rationale.insert(
            "citation_blocks".into(),
            json!(cited.iter().map(|block| block.index).collect::<Vec<_>>()),
        );
        rationale.insert("citation_evidence".into(), json!(evidence));
        evidence
    }
}

/// Appends one citation event per evidence id to the decision, in order. The
/// events carry the citation; the decision row's own `evidence_id` stays
/// empty, so the decision keeps its own sensitivity class.
pub(super) fn append_citations(
    envelope: &mut Envelope<'_>,
    decision_id: &str,
    evidence: &[String],
    recorded_at: i64,
) -> Result<(), KernelError> {
    for evidence_id in evidence {
        envelope.append_decision_event(
            decision_id,
            DecisionEventSpec {
                event_kind: CITATION_EVENT_KIND.into(),
                payload: DecisionEventPayload {
                    summary: CITATION_EVENT_SUMMARY.into(),
                },
                evidence_id: Some(evidence_id.clone()),
                recorded_at,
            },
        )?;
    }
    Ok(())
}

/// The job's `reachable_from` anchor at its frozen HEAD. Its ids derive from
/// the job id and the oid, so a retry rebuilds the same row.
pub(super) fn anchor_spec(job: &CaptureJob, head: &FrozenHead) -> Option<AnchorSpec> {
    let FrozenHead::Commit(oid) = head else {
        return None;
    };
    let digest = lower_hex(&Sha256::digest(format!("{}:{oid}", job.job_id).as_bytes()));
    Some(AnchorSpec {
        anchor_id: format!("anchor_{}", &digest[..32]),
        object_id: format!("anchorobj_{}", &digest[..32]),
        domain_id: "memory".into(),
        source_kind: "assistant".into(),
        source_id: "memory_capture".into(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        value: AnchorValue::ReachableFrom {
            oid: oid.clone(),
            captures: Vec::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn blocks(texts: &[&str]) -> Vec<Block> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| Block {
                index: index * 2,
                digest: lower_hex(&Sha256::digest(text.as_bytes())),
                length: text.len() as u64,
                evidence_id: format!("srcev:{index}"),
            })
            .collect()
    }

    /// The reference location rule `locate` must reproduce.
    fn newline_walk(text: &str, blocks: &[Block]) -> Vec<Range<usize>> {
        let bytes = text.as_bytes();
        let mut located = Vec::new();
        let mut targets = blocks.iter().map(|block| digest_bytes(&block.digest));
        let Some(Some(mut target)) = targets.next() else {
            return located;
        };
        let mut hasher = Sha256::new();
        let mut start = 0;
        let mut fed = 0;
        let ends = memchr::memchr_iter(b'\n', bytes).chain(std::iter::once(bytes.len()));
        for end in ends {
            hasher.update(&bytes[fed..end]);
            fed = end;
            if hasher.clone().finalize().as_slice() == target {
                located.push(start..end);
                start = end + 1;
                fed = start.min(bytes.len());
                hasher = Sha256::new();
                match targets.next() {
                    Some(Some(next)) => target = next,
                    _ => break,
                }
            }
        }
        located
    }

    #[test]
    fn blocks_with_inner_newlines_are_located_in_order_and_the_walk_stops_at_a_miss() {
        let texts = ["first line\nsecond line", "", "third\n\nblock"];
        let text = texts.join("\n");
        assert_eq!(locate(&text, &blocks(&texts)), vec![0..22, 23..23, 24..36]);
        let mut changed = blocks(&texts);
        changed[1].digest = lower_hex(&Sha256::digest(b"other"));
        assert_eq!(locate(&text, &changed), vec![0..22]);
        assert!(locate("redacted\nthird", &blocks(&["first", "third"])).is_empty());
    }

    #[test]
    fn an_empty_block_a_block_past_the_text_and_a_malformed_digest_match_the_newline_walk() {
        let mut malformed = blocks(&["first"]);
        malformed[0].digest = "zz".repeat(32);
        for (text, blocks) in [
            ("", blocks(&[""])),
            ("first", blocks(&["first\nsecond"])),
            ("first", malformed),
        ] {
            assert_eq!(
                locate(text, &blocks),
                newline_walk(text, &blocks),
                "{text:?}"
            );
        }
        assert_eq!(locate("", &blocks(&[""])), vec![0..0]);
        assert!(locate("first", &blocks(&["first\nsecond"])).is_empty());
    }

    #[test]
    fn a_quote_cites_its_block_the_run_it_spans_or_the_lowest_block() {
        let texts = ["alpha one", "beta two", "gamma three", "delta four"];
        let text = texts.join("\n");
        let blocks = blocks(&texts);
        let located = locate(&text, &blocks);
        let cited = |quote: &str| {
            let (rule, cited) = target(&text, quote, &blocks, &located);
            (
                rule,
                cited.iter().map(|block| block.index).collect::<Vec<_>>(),
            )
        };
        assert_eq!(cited("gamma"), (BlockRule::Block, vec![4]));
        assert_eq!(cited("two\ngamma"), (BlockRule::Span, vec![2, 4]));
        assert_eq!(
            cited("one\nbeta two\ngamma"),
            (BlockRule::Span, vec![0, 2, 4])
        );
        assert_eq!(cited("\ngamma"), (BlockRule::Span, vec![2, 4]));
        assert_eq!(cited("two\n"), (BlockRule::Span, vec![2, 4]));
        assert_eq!(cited("absent"), (BlockRule::First, vec![0]));
        let partial = &located[..2];
        let (rule, cited) = target(&text, "gamma", &blocks, partial);
        assert_eq!((rule, cited[0].index), (BlockRule::First, 0));
    }

    #[test]
    fn only_a_digest_colon_offset_id_has_the_hashed_shape() {
        let digest = "a".repeat(64);
        assert!(is_hashed_source_id(&format!("{digest}:0")));
        assert!(is_hashed_source_id(&format!("{digest}:16384")));
        for id in [
            format!("{digest}:016384"),
            format!("{digest}:"),
            format!("{}:0", "A".repeat(64)),
            format!("{}:0", "a".repeat(63)),
            "a1b2c3d4".to_string(),
            "native-user-1".to_string(),
            digest.clone(),
        ] {
            assert!(!is_hashed_source_id(&id), "{id}");
        }
    }

    /// An identity the source-identity encoder refuses can hold no
    /// descriptor, so the lookup reads nothing and the memory reports a miss
    /// instead of failing its publish.
    #[test]
    fn an_identity_the_encoder_refuses_reads_no_descriptor_and_reports_a_miss() {
        let job = CaptureJob {
            job_id: "job".into(),
            project: "a".repeat(64),
            harness: "cli".into(),
            session_id: "session".into(),
            message_id: "message".into(),
            role: "user".into(),
            text: "Use port 4321.".into(),
            prepared: None,
            attempts: 0,
            failures: 0,
        };
        assert!(message_lineages(&job, 0..MAX_CITATION_BLOCKS).is_empty());
        let dir = tempfile::tempdir().unwrap();
        let kernel = kernel::KernelStore::open(dir.path()).unwrap();
        kernel
            .commit(
                kernel::CommitIntent {
                    producer: "cite-test".into(),
                    operation_key: "lookup".into(),
                    request_digest: "0".repeat(64),
                    actor: "test".into(),
                    cause: "lookup".into(),
                },
                |envelope| {
                    let citation = Citation::lookup(envelope, &job, true)?;
                    let mut rationale = Map::new();
                    let mut reasons = Vec::new();
                    assert!(
                        citation
                            .cite(&job.text, "Use port", &mut rationale, &mut reasons)
                            .is_empty()
                    );
                    assert_eq!(rationale["citation"], "descriptor_not_found");
                    assert_eq!(reasons, vec![Count::DecisionsDescriptorNotFound]);
                    Ok(String::new())
                },
            )
            .unwrap();
    }

    fn intent(key: &str) -> kernel::CommitIntent {
        kernel::CommitIntent {
            producer: "cite-test".into(),
            operation_key: key.into(),
            request_digest: lower_hex(&Sha256::digest(key.as_bytes())),
            actor: "test".into(),
            cause: "lookup".into(),
        }
    }

    fn publish_blocks(kernel: &kernel::KernelStore, job: &CaptureJob, blocks: &[(usize, String)]) {
        kernel
            .commit(intent("domain"), |envelope| {
                envelope.insert_domain(kernel::DomainSpec {
                    domain_id: "source".into(),
                    object_id: "source-domain".into(),
                    name: "source".into(),
                    source_kind: "test".into(),
                    source_id: "source".into(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                Ok(String::new())
            })
            .unwrap();
        let requests = blocks
            .iter()
            .map(|(index, text)| kernel::ArtifactIngestRequest {
                intent: intent(&format!("artifact-{index}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("srcev:{index}"),
                object_id: format!("evidence-{index}"),
                object_kind: "evidence".into(),
                domain_id: "source".into(),
                source_kind: "test".into(),
                source_id: format!("block/{index}"),
                source_revision: 1,
                media_type: "text/plain".into(),
                retention_class: "canonical".into(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: kernel::ProviderEgress::RemoteAllowed,
                provenance: None,
            })
            .collect();
        kernel
            .ingest_exact_artifacts_with(
                intent("descriptors"),
                requests,
                None,
                |envelope, handles| {
                    for ((index, text), handle) in blocks.iter().zip(handles) {
                        let identity = crate::harness_sources::message_identity(
                            &job.project,
                            &job.harness,
                            &job.session_id,
                            &job.message_id,
                            *index,
                        );
                        let identity: Vec<(&str, &str)> = identity
                            .iter()
                            .map(|(name, value)| (*name, value.as_str()))
                            .collect();
                        envelope
                            .publish_source_descriptor(&kernel::SourceDescriptorRequest {
                                occurrence: Occurrence {
                                    class: OccurrenceClass::Messages.code(),
                                    identity: &identity,
                                    revision: "1",
                                    representation: "text",
                                    span: None,
                                },
                                source_policy: kernel::SourceDescriptorPolicy::Native,
                                domain_id: "source",
                                scope_id: None,
                                evidence_id: &handle.evidence_id,
                                artifact_digest: &handle.digest,
                                buffer: text,
                                sensitivity: Sensitivity::Normal,
                                observed_at: 1,
                                expected_head: None,
                                outcome: None,
                            })
                            .map_err(|_| KernelError::InvalidInput)?;
                    }
                    Ok(String::new())
                },
            )
            .unwrap();
    }

    #[test]
    fn a_chunked_lookup_cites_what_reading_every_index_one_by_one_cites() {
        // Descriptor lookup misses at indexes 4 and 16, the starts of two chunks.
        let published: Vec<(usize, String)> = (0..=70)
            .filter(|index| ![4, 16].contains(index))
            .map(|index| (index, format!("block {index} text")))
            .collect();
        let joined = |edited: Option<usize>| {
            published
                .iter()
                .map(|(index, text)| match edited {
                    Some(edited) if edited == *index => "edited".to_owned(),
                    _ => text.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut job = CaptureJob {
            job_id: "job".into(),
            project: "/project".into(),
            harness: "pi".into(),
            session_id: "session".into(),
            message_id: "message".into(),
            role: "user".into(),
            text: joined(None),
            prepared: None,
            attempts: 0,
            failures: 0,
        };
        let dir = tempfile::tempdir().unwrap();
        let kernel = kernel::KernelStore::open(dir.path()).unwrap();
        publish_blocks(&kernel, &job, &published);
        let quotes = [
            "block 3 text\nblock 5",
            "block 15 text\nblock 17",
            "block 17 text",
            "block 63 text\nblock 64",
            "block 66 text",
            "block 70 text",
            "absent",
        ];
        for edited in [None, Some(20)] {
            job.text = joined(edited);
            kernel
                .commit(intent(&format!("lookup-{edited:?}")), |envelope| {
                    let mut every = Vec::new();
                    for index in 0..MAX_CITATION_BLOCKS {
                        let identity = crate::harness_sources::message_identity(
                            &job.project,
                            &job.harness,
                            &job.session_id,
                            &job.message_id,
                            index,
                        );
                        let identity: Vec<(&str, &str)> = identity
                            .iter()
                            .map(|(name, value)| (*name, value.as_str()))
                            .collect();
                        let lineage =
                            kernel::source_identity::encode_preserving_span(&Occurrence {
                                class: OccurrenceClass::Messages.code(),
                                identity: &identity,
                                revision: "1",
                                representation: "text",
                                span: None,
                            })
                            .unwrap()
                            .lineage_id;
                        if let Some(row) = envelope.live_source_descriptor_of_lineage(
                            OccurrenceClass::Messages,
                            &lineage,
                        )? {
                            every.push(Block {
                                index,
                                digest: row.detail.artifact_digest,
                                length: row.byte_length,
                                evidence_id: row.detail.evidence_id,
                            });
                        }
                    }
                    assert_eq!(every.len(), published.len());
                    let every_located = locate(&job.text, &every);
                    let citation = Citation::lookup(envelope, &job, true)?;
                    assert_eq!(citation.located, every_located, "{edited:?}");
                    assert_eq!(
                        citation.located.len(),
                        if edited.is_some() {
                            18
                        } else {
                            published.len()
                        }
                    );
                    assert_eq!(citation.blocks[..], every[..citation.blocks.len()]);
                    for quote in quotes {
                        let (rule, cited) =
                            target(&job.text, quote, &citation.blocks, &citation.located);
                        let (every_rule, every_cited) =
                            target(&job.text, quote, &every, &every_located);
                        assert_eq!(
                            (rule, cited),
                            (every_rule, every_cited),
                            "{quote:?} {edited:?}"
                        );
                    }
                    Ok(String::new())
                })
                .unwrap();
        }
    }

    proptest! {
        #[test]
        fn stopping_at_the_walk_cites_what_reading_every_index_cites(
            items in proptest::collection::vec("[ab\n]{0,4}", 1..6),
            offset in proptest::sample::select(vec![0usize, 2, 3, 14, 15, 62, 63]),
            published in proptest::collection::vec(0u8..4, 1..12),
            refused in proptest::option::of(0usize..80),
            quotes in proptest::collection::vec((any::<proptest::sample::Index>(), 0usize..8), 1..4),
        ) {
            let refs: Vec<&str> = items.iter().map(String::as_str).collect();
            let text = refs.join("\n");
            let live = |index: usize| -> Option<Block> {
                let position = index.checked_sub(offset)?;
                let item = refs[position % refs.len()];
                let artifact = match published.get(position)? {
                    1 => item.to_owned(),
                    2 => format!("{item}b"),
                    _ => return None,
                };
                Some(Block {
                    index,
                    digest: lower_hex(&Sha256::digest(artifact.as_bytes())),
                    length: artifact.len() as u64,
                    evidence_id: format!("srcev:{index}"),
                })
            };
            let readable = |index: &usize| refused.is_none_or(|refused| *index < refused);
            let every: Vec<Block> = (0..MAX_CITATION_BLOCKS)
                .take_while(readable)
                .filter_map(live)
                .collect();
            let every_located = locate(&text, &every);
            let mut requested = Vec::new();
            let (blocks, located) = read_blocks(&text, |indexes| {
                requested.push(indexes.clone());
                Ok(indexes.take_while(readable).map(live).collect())
            })
            .unwrap();
            prop_assert_eq!(&located, &every_located);
            prop_assert_eq!(blocks.is_empty(), every.is_empty());
            prop_assert_eq!(blocks.first(), every.first());
            let stopped = blocks.len() > located.len()
                || located.last().is_some_and(|range| range.end == text.len());
            let mut expected = Vec::new();
            let mut start = 0;
            for end in CHUNK_ENDS {
                expected.push(start..end);
                let holds = |index: usize| (start..end).contains(&index);
                if (stopped && blocks.last().is_some_and(|block| holds(block.index)))
                    || refused.is_some_and(holds)
                {
                    break;
                }
                start = end;
            }
            prop_assert_eq!(requested, expected);
            for (from, length) in quotes {
                if text.is_empty() || blocks.is_empty() {
                    break;
                }
                let start = from.index(text.len());
                let quote = &text[start..(start + length).min(text.len())];
                let (rule, cited) = target(&text, quote, &blocks, &located);
                let (every_rule, every_cited) = target(&text, quote, &every, &every_located);
                prop_assert_eq!(rule, every_rule);
                prop_assert_eq!(cited, every_cited);
            }
        }

        #[test]
        fn locating_by_length_matches_the_newline_walk(
            items in proptest::collection::vec("[ab\n]{0,5}", 1..6),
            artifacts in proptest::collection::vec(proptest::option::of("[ab\n]{0,5}"), 1..6),
            cut in proptest::option::of(0usize..32),
        ) {
            let refs: Vec<&str> = items.iter().map(String::as_str).collect();
            let mut text = refs.join("\n");
            if let Some(cut) = cut {
                text.truncate(cut.min(text.len()));
            }
            // An artifact other than its item stands for a redacted or
            // edited item.
            let published: Vec<&str> = refs
                .iter()
                .zip(artifacts.iter().chain(std::iter::repeat(&None)))
                .map(|(item, artifact)| artifact.as_deref().unwrap_or(item))
                .collect();
            let blocks = blocks(&published);
            prop_assert_eq!(locate(&text, &blocks), newline_walk(&text, &blocks));
        }

        #[test]
        fn every_block_of_a_joined_message_is_located_at_its_own_bytes(
            texts in proptest::collection::vec("[a-c\n]{0,6}", 1..6),
        ) {
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let text = refs.join("\n");
            let blocks = blocks(&refs);
            let located = locate(&text, &blocks);
            prop_assert_eq!(located.len(), blocks.len());
            for (range, block) in located.iter().zip(&texts) {
                prop_assert_eq!(&text[range.clone()], block.as_str());
            }
        }

        #[test]
        fn a_quote_of_a_fully_located_message_cites_blocks_whose_join_holds_it(
            texts in proptest::collection::vec("[a-c\n]{0,6}", 1..6),
            from in any::<proptest::sample::Index>(),
            length in 1usize..12,
        ) {
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            let text = refs.join("\n");
            prop_assume!(!text.is_empty());
            let start = from.index(text.len());
            let quote = &text[start..(start + length).min(text.len())];
            let blocks = blocks(&refs);
            let located = locate(&text, &blocks);
            let (rule, cited) = target(&text, quote, &blocks, &located);
            prop_assert_ne!(rule, BlockRule::First);
            let joined: Vec<&str> = cited.iter().map(|block| refs[block.index / 2]).collect();
            prop_assert!(joined.join("\n").contains(quote));
            prop_assert_eq!(rule == BlockRule::Block, cited.len() == 1);
        }
    }
}
