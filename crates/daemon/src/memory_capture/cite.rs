use std::ops::Range;

use kernel::source_identity::{Occurrence, OccurrenceClass, encode_preserving_span, lower_hex};
use kernel::{
    AnchorSpec, AnchorValue, DecisionEventPayload, DecisionEventSpec, Envelope, KernelError,
    Sensitivity,
};
use memory_store::memory_capture::CaptureJob;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::FrozenHead;
use crate::source_capture::{Count, SettlementOutcome};

/// Block indexes a message's citation lookup probes, `0..MAX_CITATION_BLOCKS`.
pub(super) const MAX_CITATION_BLOCKS: usize = 256;
pub(crate) const CITATION_EVENT_KIND: &str = "memory_capture_citation";
const CITATION_EVENT_SUMMARY: &str = "cites the captured source message block";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CitationReason {
    Cited,
    SourceCaptureOff,
    SourceIdUnjoinable,
    PublishBudgetExpired,
    DescriptorNotFound,
}

impl CitationReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cited => "cited",
            Self::SourceCaptureOff => "source_capture_off",
            Self::SourceIdUnjoinable => "source_id_unjoinable",
            Self::PublishBudgetExpired => "publish_budget_expired",
            Self::DescriptorNotFound => "descriptor_not_found",
        }
    }

    fn count(self) -> Count {
        match self {
            Self::Cited => Count::DecisionsCited,
            Self::SourceCaptureOff => Count::DecisionsSourceCaptureOff,
            Self::SourceIdUnjoinable => Count::DecisionsSourceIdUnjoinable,
            Self::PublishBudgetExpired => Count::DecisionsPublishBudgetExpired,
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

/// The lineage of one block of the job's message, or `None` for an identity
/// the encoder refuses, which no descriptor can hold. A lineage omits the
/// revision, so any revision encodes it.
fn message_lineage(job: &CaptureJob, block: usize) -> Option<String> {
    let identity = crate::harness_sources::message_identity(
        &job.project,
        &job.harness,
        &job.session_id,
        &job.message_id,
        block,
    );
    let identity: Vec<(&str, &str)> = identity
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    encode_preserving_span(&Occurrence {
        class: OccurrenceClass::Messages.code(),
        identity: &identity,
        revision: "1",
        representation: crate::harness_sources::Representation::Text.as_str(),
        span: None,
    })
    .ok()
    .map(|encoded| encoded.lineage_id)
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

/// Byte ranges of the leading `blocks` in `text`, which joins the message's
/// text blocks with `\n`. One SHA-256 runs over the text and is finalized at
/// each `\n` and at the end; a segment whose digest equals the next block's
/// artifact digest locates that block. The walk stops at the first block it
/// cannot locate.
pub(super) fn locate(text: &str, blocks: &[Block]) -> Vec<Range<usize>> {
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

impl Citation {
    /// Reads the live descriptors of the job's message inside `envelope`, so
    /// the read, the anchor, the decisions, and their citation events share
    /// one view. `settlement` is what the session's source publish reached
    /// before the submit; `Off` means consent is off and nothing is read.
    pub(super) fn lookup(
        envelope: &Envelope<'_>,
        job: &CaptureJob,
        settlement: SettlementOutcome,
    ) -> Result<Self, KernelError> {
        let empty = |reason| Self {
            reason,
            blocks: Vec::new(),
            located: Vec::new(),
        };
        if settlement == SettlementOutcome::Off {
            return Ok(empty(CitationReason::SourceCaptureOff));
        }
        let mut blocks = Vec::new();
        for index in 0..MAX_CITATION_BLOCKS {
            let Some(lineage) = message_lineage(job, index) else {
                break;
            };
            if let Some(row) =
                envelope.live_source_descriptor_of_lineage(OccurrenceClass::Messages, &lineage)?
            {
                blocks.push(Block {
                    index,
                    digest: row.detail.artifact_digest,
                    evidence_id: row.detail.evidence_id,
                });
            }
        }
        if blocks.is_empty() {
            return Ok(empty(if is_hashed_source_id(&job.message_id) {
                CitationReason::SourceIdUnjoinable
            } else if settlement == SettlementOutcome::BudgetExpired {
                CitationReason::PublishBudgetExpired
            } else {
                CitationReason::DescriptorNotFound
            }));
        }
        let located = locate(&job.text, &blocks);
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
                evidence_id: format!("srcev:{index}"),
            })
            .collect()
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
        assert_eq!(message_lineage(&job, 0), None);
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
                    let citation = Citation::lookup(envelope, &job, SettlementOutcome::Settled)?;
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

    proptest! {
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
