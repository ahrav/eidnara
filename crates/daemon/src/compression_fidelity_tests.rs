//! The compression fidelity corpus (`testdata/compression-fidelity.json`) is the hand-reviewed
//! oracle for the compression fidelity contract. This module owns its source, span, and revision
//! validation: every case and scenario ID is well formed and unique, every evidence span resolves
//! to the exact bytes of its native OpenCode block at the named revision, a successor revision
//! keeps its predecessor's identity and byte length, every case covers P1 through P4 and a
//! pressure or omission scenario, and no answer key or evaluator label appears in a field that can
//! reach provider input. The compiled bytes must hash to [`CORPUS_SHA256`], the same pin the
//! TypeScript reader in `packages/e2e-tests` enforces, so a changed corpus fails until both pins
//! move together.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The corpus bytes compiled into this test binary.
pub(super) const CORPUS_BYTES: &[u8] = include_bytes!("../testdata/compression-fidelity.json");

/// SHA-256 of the complete committed corpus file. It moves only when the corpus is deliberately
/// re-authored and re-reviewed; `packages/e2e-tests/src/compression-fidelity/corpus.ts` pins the
/// same value.
pub(super) const CORPUS_SHA256: &str =
    "915b74d141b63a2c7d3a7f4b0f12b00d83675fb7b1254a0d251b2cb0616d2eb2";

const SCHEMA: &str = "eidnara.compression-fidelity-corpus/v1";

/// Wording that labels evaluation, not conversation. None of it may reach provider input.
const EVALUATOR_VOCABULARY: [&str; 7] = [
    "false-authoritative",
    "materiality",
    "nonmaterial",
    "disposition",
    "abstention",
    "forbidden conclusion",
    "answer key",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Corpus {
    pub schema: String,
    pub note: String,
    pub harness: String,
    pub project_id: String,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Case {
    pub id: String,
    pub title: String,
    pub provenance: Provenance,
    pub sources: Vec<Source>,
    pub follow_ups: Vec<FollowUp>,
    #[serde(default)]
    pub memory_examples: Vec<MemoryExample>,
    pub obligations: Vec<Obligation>,
    pub forbidden_conclusions: Vec<ForbiddenConclusion>,
    pub allowed_losses: Vec<AllowedLoss>,
    pub scenarios: Vec<Scenario>,
}

/// A case is synthetic unless it cites a documented incident.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Provenance {
    Synthetic,
    Incident { reference: String },
}

/// One native OpenCode session: its records, any later revisions of those records, and the
/// human-approved producer output for it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub id: String,
    pub session_id: String,
    pub messages: Vec<NativeMessage>,
    #[serde(default)]
    pub successors: Vec<NativeMessage>,
    pub approved_example: String,
}

/// An OpenCode `MessageV2` record restricted to the fields the corpus uses, so an evaluator label
/// cannot ride along in an extra field.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeMessage {
    pub info: Info,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Info {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub role: Role,
    pub time: Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Time {
    pub created: u64,
    #[serde(default)]
    pub completed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Part {
    Text {
        id: String,
        text: String,
    },
    Tool {
        id: String,
        #[serde(rename = "callID")]
        call_id: String,
        tool: String,
        state: ToolState,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolState {
    pub status: ToolStatus,
    pub input: Value,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub time: ToolTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolStatus {
    Completed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolTime {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FollowUp {
    pub id: String,
    pub source: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryExample {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub text: String,
    pub eligibility: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Obligation {
    pub id: String,
    pub kind: ObligationKind,
    pub materiality: Materiality,
    pub statement: String,
    #[serde(default)]
    pub evidence: Vec<Span>,
    #[serde(default)]
    pub memory: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ObligationKind {
    Decision,
    Constraint,
    Status,
    Uncertainty,
    Rationale,
    Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Materiality {
    Material,
    Nonmaterial,
}

/// A byte range of one native block at one revision. Offsets are UTF-8 bytes, end-exclusive,
/// and `text` repeats the bytes so a reviewer reads the span without counting.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Span {
    pub source: String,
    pub message_id: String,
    pub block_index: usize,
    pub revision: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ForbiddenConclusion {
    pub id: String,
    pub class: FailureClass,
    pub statement: String,
}

/// The historical failure vocabulary: a false state transition or authority is
/// `false-authoritative`; lost material meaning is `recall`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FailureClass {
    FalseAuthoritative,
    Recall,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AllowedLoss {
    pub id: String,
    pub materiality: Materiality,
    pub statement: String,
    pub evidence: Vec<Span>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scenario {
    pub id: String,
    pub source: String,
    pub follow_up: String,
    pub serving: Serving,
    pub expectations: Vec<Expectation>,
    pub abstention: Abstention,
    pub forbidden: Vec<String>,
    pub description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Serving {
    pub path: ServingPath,
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub stage: Option<Stage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ServingPath {
    Natural,
    Pressure,
    Omission,
    HintTruncated,
    MemoryAdmitted,
    MemoryExcluded,
    ExactRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Tier {
    P1,
    P2,
    P3,
    P4,
    P5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    M1,
    M0,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Expectation {
    pub obligation: String,
    pub accepted: Vec<Disposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Disposition {
    Visible,
    Discoverable,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Abstention {
    Permitted,
    Forbidden,
}

/// Why the corpus file's bytes are not the reviewed corpus.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IdentityError {
    Mismatch { actual: String },
}

/// Accepts exactly the pinned bytes. No normalization: whitespace and same-length edits change
/// the digest and are rejected like any other edit.
pub(super) fn verify_corpus_identity(bytes: &[u8]) -> Result<(), IdentityError> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == CORPUS_SHA256 {
        Ok(())
    } else {
        Err(IdentityError::Mismatch { actual })
    }
}

/// The compiled corpus after its identity and validation pass. Later fidelity witnesses read the
/// corpus only through this function.
pub(super) fn corpus() -> Corpus {
    verify_corpus_identity(CORPUS_BYTES).expect("compiled corpus bytes match the pinned digest");
    let corpus = decode(CORPUS_BYTES).expect("compiled corpus decodes");
    let violations = validate(&corpus);
    assert!(violations.is_empty(), "corpus violations: {violations:#?}");
    corpus
}

fn decode(bytes: &[u8]) -> serde_json::Result<Corpus> {
    serde_json::from_slice(bytes)
}

/// One way the corpus fails its own contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Violation {
    Header(&'static str),
    MalformedId(String),
    DuplicateId(String),
    UnknownReference {
        from: String,
        to: String,
    },
    Message {
        source: String,
        message_id: String,
        problem: &'static str,
    },
    Successor {
        source: String,
        message_id: String,
        problem: &'static str,
    },
    SpanUnresolved {
        owner: String,
        problem: &'static str,
    },
    RevisionMismatch {
        owner: String,
        message_id: String,
        revision: String,
    },
    Obligation {
        id: String,
        problem: &'static str,
    },
    Scenario {
        id: String,
        problem: &'static str,
    },
    MissingTier {
        case: String,
        tier: Tier,
    },
    MissingPressureOrOmission(String),
    MissingObligationKind(ObligationKind),
    MissingFalseTransition(String),
    UnexercisedObligation(String),
    AnswerKeyInProviderInput {
        field: String,
        label: String,
    },
}

/// Every violation the corpus holds, in a deterministic order. Empty means valid.
pub(super) fn validate(corpus: &Corpus) -> Vec<Violation> {
    let mut out = Vec::new();
    if corpus.schema != SCHEMA {
        out.push(Violation::Header("schema"));
    }
    if corpus.note.trim().is_empty() {
        out.push(Violation::Header("note"));
    }
    if corpus.harness != "opencode" {
        out.push(Violation::Header("harness"));
    }
    if corpus.project_id.len() != 64
        || !corpus
            .project_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        out.push(Violation::Header("project_id"));
    }
    let mut ids = BTreeSet::new();
    for case in &corpus.cases {
        if !is_case_id(&case.id) {
            out.push(Violation::MalformedId(case.id.clone()));
        }
        claim(&mut ids, &case.id, &mut out);
        validate_case(case, &mut ids, &mut out);
    }
    // R5: the corpus covers decisions, constraints, status, and exact-source evidence.
    let kinds: BTreeSet<ObligationKind> = corpus
        .cases
        .iter()
        .flat_map(|c| c.obligations.iter().map(|o| o.kind))
        .collect();
    for kind in [
        ObligationKind::Decision,
        ObligationKind::Constraint,
        ObligationKind::Status,
        ObligationKind::Evidence,
    ] {
        if !kinds.contains(&kind) {
            out.push(Violation::MissingObligationKind(kind));
        }
    }
    answer_key_exclusion(corpus, &mut out);
    out
}

fn is_case_id(id: &str) -> bool {
    id.strip_prefix('C').is_some_and(|n| {
        !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
    })
}

/// A child ID is `<case>.<kind letter><positive integer>`, for example `C1.S3`.
fn is_child_id(case: &str, kind: char, id: &str) -> bool {
    id.strip_prefix(case)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_prefix(kind))
        .is_some_and(|n| {
            !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
        })
}

fn claim(ids: &mut BTreeSet<String>, id: &str, out: &mut Vec<Violation>) {
    if !ids.insert(id.to_owned()) {
        out.push(Violation::DuplicateId(id.to_owned()));
    }
}

fn child(case: &Case, kind: char, id: &str, ids: &mut BTreeSet<String>, out: &mut Vec<Violation>) {
    if !is_child_id(&case.id, kind, id) {
        out.push(Violation::MalformedId(id.to_owned()));
    }
    claim(ids, id, out);
}

fn reference(from: &str, to: &str, known: &BTreeSet<&str>, out: &mut Vec<Violation>) {
    if !known.contains(to) {
        out.push(Violation::UnknownReference {
            from: from.to_owned(),
            to: to.to_owned(),
        });
    }
}

fn validate_case(case: &Case, ids: &mut BTreeSet<String>, out: &mut Vec<Violation>) {
    if let Provenance::Incident { reference } = &case.provenance
        && reference.trim().is_empty()
    {
        out.push(Violation::Header("incident provenance needs a reference"));
    }
    if case.title.trim().is_empty() {
        out.push(Violation::Header("a case has a title"));
    }
    if !case
        .forbidden_conclusions
        .iter()
        .any(|f| f.class == FailureClass::FalseAuthoritative)
    {
        out.push(Violation::MissingFalseTransition(case.id.clone()));
    }
    for source in &case.sources {
        child(case, 'V', &source.id, ids, out);
        validate_source(source, out);
    }
    for follow_up in &case.follow_ups {
        child(case, 'F', &follow_up.id, ids, out);
    }
    for memory in &case.memory_examples {
        child(case, 'M', &memory.id, ids, out);
    }
    for obligation in &case.obligations {
        child(case, 'O', &obligation.id, ids, out);
    }
    for forbidden in &case.forbidden_conclusions {
        child(case, 'X', &forbidden.id, ids, out);
    }
    for loss in &case.allowed_losses {
        child(case, 'L', &loss.id, ids, out);
    }
    for scenario in &case.scenarios {
        child(case, 'S', &scenario.id, ids, out);
    }

    let sources: BTreeSet<&str> = case.sources.iter().map(|s| s.id.as_str()).collect();
    let follow_ups: BTreeMap<&str, &FollowUp> =
        case.follow_ups.iter().map(|f| (f.id.as_str(), f)).collect();
    let memories: BTreeSet<&str> = case.memory_examples.iter().map(|m| m.id.as_str()).collect();
    let obligations: BTreeSet<&str> = case.obligations.iter().map(|o| o.id.as_str()).collect();
    let forbidden: BTreeSet<&str> = case
        .forbidden_conclusions
        .iter()
        .map(|f| f.id.as_str())
        .collect();

    for follow_up in &case.follow_ups {
        reference(&follow_up.id, &follow_up.source, &sources, out);
    }
    for memory in &case.memory_examples {
        if memory.scope != "project"
            || memory.text.trim().is_empty()
            || !promotable_categories().contains(&memory.category)
        {
            out.push(Violation::Obligation {
                id: memory.id.clone(),
                problem: "a memory example is nonempty project memory in a promotable category",
            });
        }
    }
    for obligation in &case.obligations {
        if obligation.materiality != Materiality::Material {
            out.push(Violation::Obligation {
                id: obligation.id.clone(),
                problem: "an obligation is material; nonmaterial detail is an allowed loss",
            });
        }
        match (&obligation.memory, obligation.evidence.is_empty()) {
            (None, true) => out.push(Violation::Obligation {
                id: obligation.id.clone(),
                problem: "an obligation needs native evidence or a memory example",
            }),
            (Some(_), false) => out.push(Violation::Obligation {
                id: obligation.id.clone(),
                problem: "an obligation cites native evidence or a memory example, not both",
            }),
            (Some(memory), true) => reference(&obligation.id, memory, &memories, out),
            (None, false) => {}
        }
        for span in &obligation.evidence {
            validate_span(case, &obligation.id, span, &sources, out);
        }
    }
    for loss in &case.allowed_losses {
        if loss.materiality != Materiality::Nonmaterial {
            out.push(Violation::Obligation {
                id: loss.id.clone(),
                problem: "an allowed loss is marked nonmaterial",
            });
        }
        if loss.evidence.is_empty() {
            out.push(Violation::Obligation {
                id: loss.id.clone(),
                problem: "an allowed loss names the native detail it permits losing",
            });
        }
        for span in &loss.evidence {
            validate_span(case, &loss.id, span, &sources, out);
        }
    }

    let mut exercised = BTreeSet::new();
    for scenario in &case.scenarios {
        reference(&scenario.id, &scenario.source, &sources, out);
        match follow_ups.get(scenario.follow_up.as_str()) {
            None => out.push(Violation::UnknownReference {
                from: scenario.id.clone(),
                to: scenario.follow_up.clone(),
            }),
            Some(follow_up) if follow_up.source != scenario.source => {
                out.push(Violation::Scenario {
                    id: scenario.id.clone(),
                    problem: "the follow-up belongs to another source",
                })
            }
            Some(_) => {}
        }
        for id in &scenario.forbidden {
            reference(&scenario.id, id, &forbidden, out);
        }
        validate_serving(scenario, out);
        let mut seen = BTreeSet::new();
        for expectation in &scenario.expectations {
            reference(&scenario.id, &expectation.obligation, &obligations, out);
            exercised.insert(expectation.obligation.as_str());
            if !seen.insert(expectation.obligation.as_str()) {
                out.push(Violation::Scenario {
                    id: scenario.id.clone(),
                    problem: "an obligation has one expectation per scenario",
                });
            }
            let accepted: BTreeSet<_> = expectation.accepted.iter().collect();
            if accepted.is_empty() || accepted.len() != expectation.accepted.len() {
                out.push(Violation::Scenario {
                    id: scenario.id.clone(),
                    problem: "accepted dispositions are nonempty and distinct",
                });
            }
            if accepted.contains(&Disposition::Unavailable)
                && scenario.abstention == Abstention::Forbidden
            {
                out.push(Violation::Scenario {
                    id: scenario.id.clone(),
                    problem: "unavailable evidence is acceptable only where abstention is permitted",
                });
            }
        }
        if scenario.expectations.is_empty() {
            out.push(Violation::Scenario {
                id: scenario.id.clone(),
                problem: "a scenario names the obligations it checks",
            });
        }
    }
    for obligation in &case.obligations {
        if !exercised.contains(obligation.id.as_str()) {
            out.push(Violation::UnexercisedObligation(obligation.id.clone()));
        }
    }

    for tier in [Tier::P1, Tier::P2, Tier::P3, Tier::P4] {
        let covered = case.scenarios.iter().any(|s| {
            s.serving.path == ServingPath::Natural
                && s.serving.tier == Some(tier)
                && (tier != Tier::P1 || s.serving.stage == Some(Stage::M1))
        });
        if !covered {
            out.push(Violation::MissingTier {
                case: case.id.clone(),
                tier,
            });
        }
    }
    if !case.scenarios.iter().any(|s| {
        matches!(
            s.serving.path,
            ServingPath::Pressure | ServingPath::Omission
        )
    }) {
        out.push(Violation::MissingPressureOrOmission(case.id.clone()));
    }
}

/// The categories the memory classifier may promote, from the daemon's frozen vocabulary.
fn promotable_categories() -> Vec<String> {
    let vocabulary: Value =
        serde_json::from_str(include_str!("../testdata/memory-category-vocabulary.json"))
            .expect("memory category vocabulary decodes");
    serde_json::from_value(vocabulary["PROMOTABLE_CATEGORIES"].clone())
        .expect("promotable categories are strings")
}

fn validate_serving(scenario: &Scenario, out: &mut Vec<Violation>) {
    let serving = &scenario.serving;
    let problem = match (serving.path, serving.tier, serving.stage) {
        (ServingPath::ExactRead, None, None) => None,
        (ServingPath::ExactRead, _, _) => {
            Some("an exact read is internal and names no served tier or stage")
        }
        (_, None, _) | (_, _, None) => Some("a served scenario names its tier and stage"),
        (ServingPath::Natural, Some(Tier::P5), _) => {
            Some("natural serving shows segment text, so its tier is P1 to P4")
        }
        (ServingPath::Omission, Some(tier), _) if tier != Tier::P5 => {
            Some("an omission scenario serves the segment at P5")
        }
        _ => None,
    };
    if let Some(problem) = problem {
        out.push(Violation::Scenario {
            id: scenario.id.clone(),
            problem,
        });
    }
}

/// The canonical revision of a block: a tool's settle time, or the message's completion time,
/// or its creation time when it never completes. These are the revisions the OpenCode source
/// adapter publishes.
fn block_revision(message: &NativeMessage, part: &Part) -> String {
    match part {
        Part::Tool { state, .. } => state.time.end.to_string(),
        Part::Text { .. } => message
            .info
            .time
            .completed
            .unwrap_or(message.info.time.created)
            .to_string(),
    }
}

/// The exact native bytes a block contributes: a text part's text or a settled tool's one
/// output or error string.
fn block_text(part: &Part) -> Option<&str> {
    match part {
        Part::Text { text, .. } => Some(text),
        Part::Tool { state, .. } => match state.status {
            ToolStatus::Completed => state.output.as_deref(),
            ToolStatus::Error => state.error.as_deref(),
        },
    }
}

fn part_id(part: &Part) -> &str {
    match part {
        Part::Text { id, .. } | Part::Tool { id, .. } => id,
    }
}

fn validate_message(source: &Source, message: &NativeMessage, out: &mut Vec<Violation>) {
    let mut problem = |problem| {
        out.push(Violation::Message {
            source: source.id.clone(),
            message_id: message.info.id.clone(),
            problem,
        })
    };
    if message.info.session_id != source.session_id {
        problem("the record names another session");
    }
    if message.info.role == Role::Assistant && message.info.time.completed.is_none() {
        problem("an assistant record is completed, so its text has a revision");
    }
    if message.parts.is_empty() {
        problem("a record has parts");
    }
    for part in &message.parts {
        if let Part::Tool { state, call_id, .. } = part {
            let settled = match state.status {
                ToolStatus::Completed => state.output.is_some() && state.error.is_none(),
                ToolStatus::Error => state.error.is_some() && state.output.is_none(),
            };
            if !settled || call_id.is_empty() || state.time.start > state.time.end {
                problem("a tool part is settled with one output or error string");
            }
        }
    }
}

fn validate_source(source: &Source, out: &mut Vec<Violation>) {
    let mut message_ids = BTreeSet::new();
    let mut part_ids = BTreeSet::new();
    for message in &source.messages {
        validate_message(source, message, out);
        if !message_ids.insert(message.info.id.as_str()) {
            out.push(Violation::DuplicateId(message.info.id.clone()));
        }
        for part in &message.parts {
            if !part_ids.insert(part_id(part)) {
                out.push(Violation::DuplicateId(part_id(part).to_owned()));
            }
        }
    }
    for successor in &source.successors {
        validate_message(source, successor, out);
        let mut problem = |problem| {
            out.push(Violation::Successor {
                source: source.id.clone(),
                message_id: successor.info.id.clone(),
                problem,
            })
        };
        let Some(original) = source
            .messages
            .iter()
            .find(|m| m.info.id == successor.info.id)
        else {
            problem("a successor revises a record of its source");
            continue;
        };
        if successor.info.role != original.info.role
            || successor.parts.len() != original.parts.len()
        {
            problem("a successor keeps the record's role and parts");
            continue;
        }
        let mut changed = false;
        for (before, after) in original.parts.iter().zip(&successor.parts) {
            match (before, after) {
                (Part::Text { id: a, text: x }, Part::Text { id: b, text: y }) if a == b => {
                    if x.len() != y.len() {
                        problem("a successor text keeps its byte length");
                    }
                    changed |= x != y;
                }
                (Part::Tool { .. }, Part::Tool { .. }) if before == after => {}
                _ => problem("a successor keeps each part's identity and every tool part"),
            }
        }
        if !changed {
            problem("a successor changes some text");
        }
        let later = original
            .parts
            .iter()
            .zip(&successor.parts)
            .filter(|(_, part)| matches!(part, Part::Text { .. }))
            .all(|(before, after)| {
                block_revision(successor, after).parse::<u64>().ok()
                    > block_revision(original, before).parse::<u64>().ok()
            });
        if !later {
            problem("a successor's revision is later than the original's");
        }
    }
}

fn validate_span(
    case: &Case,
    owner: &str,
    span: &Span,
    sources: &BTreeSet<&str>,
    out: &mut Vec<Violation>,
) {
    let unresolved = |problem| Violation::SpanUnresolved {
        owner: owner.to_owned(),
        problem,
    };
    if !sources.contains(span.source.as_str()) {
        out.push(Violation::UnknownReference {
            from: owner.to_owned(),
            to: span.source.clone(),
        });
        return;
    }
    let source = case.sources.iter().find(|s| s.id == span.source).unwrap();
    let records: Vec<&NativeMessage> = source
        .messages
        .iter()
        .chain(&source.successors)
        .filter(|m| m.info.id == span.message_id)
        .collect();
    if records.is_empty() {
        out.push(unresolved("the message is not in the source"));
        return;
    }
    let mut block_exists = false;
    let Some(text) = records.iter().find_map(|message| {
        let part = message.parts.get(span.block_index)?;
        block_exists = true;
        (block_revision(message, part) == span.revision).then(|| block_text(part))
    }) else {
        out.push(if block_exists {
            Violation::RevisionMismatch {
                owner: owner.to_owned(),
                message_id: span.message_id.clone(),
                revision: span.revision.clone(),
            }
        } else {
            unresolved("the block index is not in the message")
        });
        return;
    };
    let Some(text) = text else {
        out.push(unresolved("the block has no text"));
        return;
    };
    let problem = if span.start >= span.end {
        Some("the span is empty or reversed")
    } else if span.end > text.len() {
        Some("the span ends past the block")
    } else if !text.is_char_boundary(span.start) || !text.is_char_boundary(span.end) {
        Some("the span splits a UTF-8 character")
    } else if text[span.start..span.end] != span.text {
        Some("the span bytes differ from its quoted text")
    } else {
        None
    };
    if let Some(problem) = problem {
        out.push(unresolved(problem));
    }
}

/// Collects every string that can reach a provider: native text and tool fields, follow-up
/// prompts, memory example text, and the approved producer output that is later served.
fn provider_input(corpus: &Corpus) -> Vec<(String, String)> {
    fn strings(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::String(s) => into.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|v| strings(v, into)),
            Value::Object(map) => map.iter().for_each(|(k, v)| {
                into.push(k.clone());
                strings(v, into);
            }),
            _ => {}
        }
    }
    let mut fields = Vec::new();
    for case in &corpus.cases {
        for source in &case.sources {
            fields.push((
                format!("{}.approved_example", source.id),
                source.approved_example.clone(),
            ));
            for message in source.messages.iter().chain(&source.successors) {
                for (index, part) in message.parts.iter().enumerate() {
                    let field = format!("{}#{index}", message.info.id);
                    let mut texts = Vec::new();
                    match part {
                        Part::Text { text, .. } => texts.push(text.clone()),
                        Part::Tool { tool, state, .. } => {
                            texts.push(tool.clone());
                            strings(&state.input, &mut texts);
                            texts.extend(state.output.clone());
                            texts.extend(state.error.clone());
                        }
                    }
                    fields.extend(texts.into_iter().map(|text| (field.clone(), text)));
                }
            }
        }
        for follow_up in &case.follow_ups {
            fields.push((follow_up.id.clone(), follow_up.prompt.clone()));
        }
        for memory in &case.memory_examples {
            fields.push((memory.id.clone(), memory.text.clone()));
        }
    }
    fields
}

/// Every answer key and evaluator label: corpus IDs, obligation and loss statements, forbidden
/// conclusions, scenario descriptions, memory eligibility notes, and evaluation vocabulary.
fn evaluator_labels(corpus: &Corpus) -> Vec<String> {
    let mut labels: Vec<String> = EVALUATOR_VOCABULARY
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    for case in &corpus.cases {
        let ids = case
            .sources
            .iter()
            .map(|s| &s.id)
            .chain(case.follow_ups.iter().map(|f| &f.id))
            .chain(case.memory_examples.iter().map(|m| &m.id))
            .chain(case.obligations.iter().map(|o| &o.id))
            .chain(case.forbidden_conclusions.iter().map(|f| &f.id))
            .chain(case.allowed_losses.iter().map(|l| &l.id))
            .chain(case.scenarios.iter().map(|s| &s.id));
        labels.extend(ids.cloned());
        labels.extend(case.obligations.iter().map(|o| o.statement.clone()));
        labels.extend(case.allowed_losses.iter().map(|l| l.statement.clone()));
        labels.extend(
            case.forbidden_conclusions
                .iter()
                .map(|f| f.statement.clone()),
        );
        labels.extend(case.scenarios.iter().map(|s| s.description.clone()));
        labels.extend(case.memory_examples.iter().map(|m| m.eligibility.clone()));
    }
    labels
}

fn answer_key_exclusion(corpus: &Corpus, out: &mut Vec<Violation>) {
    let labels: Vec<(String, String)> = evaluator_labels(corpus)
        .into_iter()
        .map(|label| (label.to_lowercase(), label))
        .collect();
    for (field, text) in provider_input(corpus) {
        let haystack = text.to_lowercase();
        for (needle, label) in &labels {
            if haystack.contains(needle.as_str()) {
                out.push(Violation::AnswerKeyInProviderInput {
                    field: field.clone(),
                    label: label.clone(),
                });
            }
        }
    }
}

fn corpus_value() -> Value {
    serde_json::from_slice(CORPUS_BYTES).unwrap()
}

fn validate_value(value: Value) -> Vec<Violation> {
    validate(&serde_json::from_value(value).expect("mutated corpus still decodes"))
}

fn case_mut(value: &mut Value, case: usize) -> &mut Value {
    &mut value["cases"][case]
}

#[test]
fn the_committed_corpus_matches_its_pin_and_validates() {
    let corpus = corpus();
    assert_eq!(
        corpus
            .cases
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["C1", "C2", "C3", "C4", "C5", "C6"]
    );
    assert!(
        corpus
            .cases
            .iter()
            .all(|c| matches!(c.provenance, Provenance::Synthetic)),
        "no case cites a documented incident, so every case is synthetic"
    );
}

#[test]
fn the_digest_pin_accepts_only_the_exact_committed_bytes() {
    assert_eq!(verify_corpus_identity(CORPUS_BYTES), Ok(()));
    let text = std::str::from_utf8(CORPUS_BYTES).unwrap();
    let edits = [
        ("missing", String::new()),
        (
            "stale prebuilt copy",
            serde_json::to_string_pretty(&{
                let mut older = corpus_value();
                older["cases"].as_array_mut().unwrap().pop();
                older
            })
            .unwrap(),
        ),
        ("whitespace only", text.replacen("\n  ", "\n   ", 1)),
        ("same length", text.replacen("quick win", "quick wit", 1)),
        (
            "scenario only",
            text.replacen(
                r#""abstention": "forbidden""#,
                r#""abstention": "permitted""#,
                1,
            ),
        ),
    ];
    for (label, edited) in edits {
        assert_ne!(edited.as_bytes(), CORPUS_BYTES, "{label} edits the bytes");
        assert!(
            matches!(
                verify_corpus_identity(edited.as_bytes()),
                Err(IdentityError::Mismatch { .. })
            ),
            "{label} must be rejected"
        );
    }
    // The content-preserving edits still validate, so the pin and not validation rejects them.
    let whitespace = text.replacen("\n  ", "\n   ", 1);
    assert!(validate(&decode(whitespace.as_bytes()).unwrap()).is_empty());
    let same_length = text.replacen("quick win", "quick wit", 1);
    assert_eq!(same_length.len(), text.len());
    assert!(validate(&decode(same_length.as_bytes()).unwrap()).is_empty());
}

#[test]
fn duplicate_and_malformed_ids_are_rejected() {
    let mut value = corpus_value();
    case_mut(&mut value, 1)["id"] = "C1".into();
    let violations = validate_value(value);
    assert!(violations.contains(&Violation::DuplicateId("C1".into())));

    let mut value = corpus_value();
    let duplicate = case_mut(&mut value, 0)["scenarios"][0]["id"].clone();
    case_mut(&mut value, 0)["scenarios"][1]["id"] = duplicate;
    assert!(validate_value(value).contains(&Violation::DuplicateId("C1.S1".into())));

    for malformed in ["C1-S2", "C1.S0", "C2.S2", "C1.O2"] {
        let mut value = corpus_value();
        case_mut(&mut value, 0)["scenarios"][1]["id"] = malformed.into();
        assert!(
            validate_value(value).contains(&Violation::MalformedId(malformed.into())),
            "{malformed}"
        );
    }
    let mut value = corpus_value();
    case_mut(&mut value, 0)["id"] = "C01".into();
    assert!(validate_value(value).contains(&Violation::MalformedId("C01".into())));
}

#[test]
fn spans_that_do_not_resolve_into_their_native_block_are_rejected() {
    let evidence = |value: &mut Value| -> Value {
        case_mut(value, 0)["obligations"][0]["evidence"][0].clone()
    };
    let unresolved = |problem| Violation::SpanUnresolved {
        owner: "C1.O1".into(),
        problem,
    };
    let mutations: [(&str, fn(&mut Value), Violation); 7] = [
        (
            "quoted text differs",
            |span| span["text"] = "Let's do pooling first.".into(),
            unresolved("the span bytes differ from its quoted text"),
        ),
        (
            "shifted offsets",
            |span| {
                span["start"] = (span["start"].as_u64().unwrap() + 1).into();
                span["end"] = (span["end"].as_u64().unwrap() + 1).into();
            },
            unresolved("the span bytes differ from its quoted text"),
        ),
        (
            "past the block",
            |span| span["end"] = 100_000.into(),
            unresolved("the span ends past the block"),
        ),
        (
            "reversed",
            |span| span["end"] = span["start"].clone(),
            unresolved("the span is empty or reversed"),
        ),
        (
            "unknown message",
            |span| span["message_id"] = "msg_cf_c1_99".into(),
            unresolved("the message is not in the source"),
        ),
        (
            "unknown block",
            |span| span["block_index"] = 7.into(),
            unresolved("the block index is not in the message"),
        ),
        (
            "wrong revision",
            |span| {
                let revision: u64 = span["revision"].as_str().unwrap().parse().unwrap();
                span["revision"] = (revision + 1).to_string().into();
            },
            Violation::RevisionMismatch {
                owner: "C1.O1".into(),
                message_id: "msg_cf_c1_03".into(),
                revision: String::new(),
            },
        ),
    ];
    for (label, mutate, expected) in mutations {
        let mut value = corpus_value();
        let mut span = evidence(&mut value);
        mutate(&mut span);
        case_mut(&mut value, 0)["obligations"][0]["evidence"][0] = span.clone();
        let violations = validate_value(value);
        let expected = match expected {
            Violation::RevisionMismatch {
                owner, message_id, ..
            } => Violation::RevisionMismatch {
                owner,
                message_id,
                revision: span["revision"].as_str().unwrap().to_owned(),
            },
            other => other,
        };
        assert_eq!(violations, [expected], "{label}");
    }

    // A span that splits a multibyte character in the C6 block cannot resolve.
    let mut value = corpus_value();
    let span = &mut case_mut(&mut value, 5)["obligations"][1]["evidence"][0];
    let text = span["text"].as_str().unwrap().to_owned();
    let inside = text.find('ï').unwrap() + 1;
    span["end"] = inside.into();
    span["text"] = "x".into();
    assert_eq!(
        validate_value(value),
        [Violation::SpanUnresolved {
            owner: "C6.O2".into(),
            problem: "the span splits a UTF-8 character",
        }]
    );
}

#[test]
fn successor_revisions_keep_identity_and_byte_length() {
    let successor_problem = |problem| Violation::Successor {
        source: "C6.V1".into(),
        message_id: "msg_cf_c6_02".into(),
        problem,
    };
    let mut value = corpus_value();
    let text = &mut case_mut(&mut value, 5)["sources"][0]["successors"][0]["parts"][1]["text"];
    *text = format!("{} ", text.as_str().unwrap()).into();
    assert_eq!(
        validate_value(value),
        [successor_problem("a successor text keeps its byte length")]
    );

    let mut value = corpus_value();
    let original = case_mut(&mut value, 5)["sources"][0]["messages"][1]["parts"][1]["text"].clone();
    case_mut(&mut value, 5)["sources"][0]["successors"][0]["parts"][1]["text"] = original;
    assert_eq!(
        validate_value(value),
        [successor_problem("a successor changes some text")]
    );

    let mut value = corpus_value();
    let created =
        case_mut(&mut value, 5)["sources"][0]["messages"][1]["info"]["time"]["completed"].clone();
    case_mut(&mut value, 5)["sources"][0]["successors"][0]["info"]["time"]["completed"] = created;
    let violations = validate_value(value);
    assert!(
        violations.contains(&successor_problem(
            "a successor's revision is later than the original's"
        )),
        "{violations:?}"
    );

    // The original and successor revisions each resolve their own bytes.
    let corpus = corpus();
    let source = &corpus.cases[5].sources[0];
    let (original, successor) = (&source.messages[1], &source.successors[0]);
    assert_ne!(
        block_revision(original, &original.parts[1]),
        block_revision(successor, &successor.parts[1])
    );
    assert_ne!(
        block_text(&original.parts[1]),
        block_text(&successor.parts[1])
    );
    assert_eq!(
        block_text(&original.parts[1]).map(str::len),
        block_text(&successor.parts[1]).map(str::len)
    );
}

#[test]
fn every_case_requires_p1_to_p4_and_a_pressure_or_omission_scenario() {
    for (tier, index) in [(Tier::P1, 0), (Tier::P2, 1), (Tier::P3, 2), (Tier::P4, 3)] {
        let mut value = corpus_value();
        case_mut(&mut value, 0)["scenarios"]
            .as_array_mut()
            .unwrap()
            .remove(index);
        assert_eq!(
            validate_value(value),
            [Violation::MissingTier {
                case: "C1".into(),
                tier
            }],
            "{tier:?}"
        );
    }
    // P1 must be witnessed in the m1 window, not only after decay.
    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["serving"]["stage"] = "m0".into();
    assert_eq!(
        validate_value(value),
        [Violation::MissingTier {
            case: "C1".into(),
            tier: Tier::P1
        }]
    );
    let mut value = corpus_value();
    case_mut(&mut value, 4)["scenarios"]
        .as_array_mut()
        .unwrap()
        .remove(4);
    assert_eq!(
        validate_value(value),
        [Violation::MissingPressureOrOmission("C5".into())]
    );
}

#[test]
fn answer_keys_and_evaluator_labels_stay_out_of_provider_input() {
    let leaked = |field: &str, label: &str| Violation::AnswerKeyInProviderInput {
        field: field.into(),
        label: label.into(),
    };
    let statement = corpus().cases[0].obligations[0].statement.clone();
    let mut value = corpus_value();
    let prompt = &mut case_mut(&mut value, 0)["follow_ups"][0]["prompt"];
    *prompt = format!("{} {statement}", prompt.as_str().unwrap()).into();
    assert_eq!(validate_value(value), [leaked("C1.F1", &statement)]);

    let mut value = corpus_value();
    let text = &mut case_mut(&mut value, 0)["sources"][0]["messages"][2]["parts"][0]["text"];
    *text = format!("{} (see C1.O1)", text.as_str().unwrap()).into();
    assert_eq!(validate_value(value), [leaked("msg_cf_c1_03#0", "C1.O1")]);

    let mut value = corpus_value();
    let output = &mut case_mut(&mut value, 1)["sources"][0]["approved_example"];
    *output = output
        .as_str()
        .unwrap()
        .replacen("<p3>", "<p3>Disposition: visible. ", 1)
        .into();
    assert_eq!(
        validate_value(value),
        [leaked("C2.V1.approved_example", "disposition")]
    );

    // An evaluator label in a field of its own never decodes as a native record.
    let mut value = corpus_value();
    case_mut(&mut value, 0)["sources"][0]["messages"][2]["parts"][0]["expected"] =
        "pooling-first rejected".into();
    let error = serde_json::from_value::<Corpus>(value).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
    let mut value = corpus_value();
    case_mut(&mut value, 0)["follow_ups"][0]["answer"] = "no".into();
    assert!(serde_json::from_value::<Corpus>(value).is_err());
}

#[test]
fn scenario_expectations_are_consistent() {
    let mut value = corpus_value();
    case_mut(&mut value, 0)["scenarios"][0]["expectations"][0]["accepted"] =
        serde_json::json!(["unavailable"]);
    assert_eq!(
        validate_value(value),
        [Violation::Scenario {
            id: "C1.S1".into(),
            problem: "unavailable evidence is acceptable only where abstention is permitted",
        }]
    );

    let mut value = corpus_value();
    for scenario in case_mut(&mut value, 0)["scenarios"].as_array_mut().unwrap() {
        scenario["expectations"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["obligation"] != "C1.O3");
    }
    assert_eq!(
        validate_value(value),
        [Violation::UnexercisedObligation("C1.O3".into())]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 1)["scenarios"][5]["follow_up"] = "C2.F1".into();
    assert_eq!(
        validate_value(value),
        [Violation::Scenario {
            id: "C2.S6".into(),
            problem: "the follow-up belongs to another source",
        }]
    );

    let mut value = corpus_value();
    for forbidden in case_mut(&mut value, 3)["forbidden_conclusions"]
        .as_array_mut()
        .unwrap()
    {
        forbidden["class"] = "recall".into();
    }
    assert_eq!(
        validate_value(value),
        [Violation::MissingFalseTransition("C4".into())]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 2)["memory_examples"][0]["category"] = "SESSION_NOTES".into();
    assert_eq!(
        validate_value(value),
        [Violation::Obligation {
            id: "C3.M1".into(),
            problem: "a memory example is nonempty project memory in a promotable category",
        }]
    );

    let mut value = corpus_value();
    for case in value["cases"].as_array_mut().unwrap() {
        for obligation in case["obligations"].as_array_mut().unwrap() {
            if obligation["kind"] == "constraint" {
                obligation["kind"] = "decision".into();
            }
        }
    }
    assert_eq!(
        validate_value(value),
        [Violation::MissingObligationKind(ObligationKind::Constraint)]
    );

    let mut value = corpus_value();
    case_mut(&mut value, 0)["obligations"][0]["materiality"] = "nonmaterial".into();
    assert_eq!(
        validate_value(value),
        [Violation::Obligation {
            id: "C1.O1".into(),
            problem: "an obligation is material; nonmaterial detail is an allowed loss",
        }]
    );
}
