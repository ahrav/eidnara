//! The compression fidelity corpus (`testdata/compression-fidelity.json`) is the oracle for the
//! compression fidelity contract. This module owns its schema, its byte identity, and its source,
//! span, and revision validation: every case and scenario ID is well formed and unique, every
//! evidence span resolves to the exact bytes of its native OpenCode block at the named revision,
//! a successor revision keeps its predecessor's identity and byte length, every case covers P1
//! through P4 and a pressure or omission scenario, and no answer key or evaluator label appears in
//! a field that can reach provider input. The compiled bytes must hash to [`CORPUS_SHA256`], the
//! same pin the TypeScript reader in `packages/e2e-tests` enforces, so a changed corpus fails
//! until both pins move together.
//!
//! Integration tests can include this module by path, so its imports stay external crates and
//! its embedded files stay in `testdata/`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use aho_corasick::{AhoCorasickBuilder, AhoCorasickKind};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The corpus bytes compiled into this test binary.
pub(crate) const CORPUS_BYTES: &[u8] = include_bytes!("../testdata/compression-fidelity.json");

/// SHA-256 of the complete committed corpus file. It moves only when the corpus is deliberately
/// re-authored and re-reviewed; `packages/e2e-tests/src/compression-fidelity/corpus.ts` pins the
/// same value.
pub(crate) const CORPUS_SHA256: &str =
    "46980759b02b7696ea3be9d9b44e43603eed199a404620c3c6d952afbc153800";

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

/// The corpus schema. Every string field is either provider input, collected by
/// `provider_input`, or an evaluator label, collected by `evaluator_labels`, unless it is an ID
/// or enum; a new string field joins one of the two.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Corpus {
    pub schema: String,
    pub note: String,
    pub harness: String,
    pub project_id: String,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
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
pub(crate) enum Provenance {
    Synthetic,
    Incident { reference: String },
}

/// One native OpenCode session: its records, any later revisions of those records, and the
/// approved producer output for it, which scripted runs return.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    pub id: String,
    pub session_id: String,
    pub messages: Vec<NativeMessage>,
    #[serde(default)]
    pub successors: Vec<NativeMessage>,
    pub approved_example: String,
}

/// An OpenCode `MessageV2` record restricted to the fields the corpus uses, so an evaluator label
/// cannot ride along in an extra field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeMessage {
    pub info: Info,
    pub parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Info {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub role: Role,
    pub time: Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Time {
    pub created: u64,
    #[serde(default)]
    pub completed: Option<u64>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Part {
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

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolState {
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
pub(crate) enum ToolStatus {
    Completed,
    Error,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolTime {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowUp {
    pub id: String,
    pub source: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemoryExample {
    pub id: String,
    pub scope: String,
    pub category: String,
    pub text: String,
    pub eligibility: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Obligation {
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
pub(crate) enum ObligationKind {
    Decision,
    Constraint,
    Status,
    Uncertainty,
    Rationale,
    Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Materiality {
    Material,
    Nonmaterial,
}

/// A byte range of one native block at one revision. `block_index` is the block's part position
/// in its record; for a tool part the source adapter's own identity is the call ID and output
/// block 0. Offsets are UTF-8 bytes, end-exclusive, and `text` repeats the bytes so a reviewer
/// reads the span without counting.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Span {
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
pub(crate) struct ForbiddenConclusion {
    pub id: String,
    pub class: FailureClass,
    pub statement: String,
}

/// The historical failure vocabulary: a false state transition or authority is
/// `false-authoritative`; lost material meaning is `recall`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FailureClass {
    FalseAuthoritative,
    Recall,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AllowedLoss {
    pub id: String,
    pub materiality: Materiality,
    pub statement: String,
    pub evidence: Vec<Span>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scenario {
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
pub(crate) struct Serving {
    pub path: ServingPath,
    #[serde(default)]
    pub tier: Option<Tier>,
    #[serde(default)]
    pub stage: Option<Stage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServingPath {
    Natural,
    Pressure,
    Omission,
    HintTruncated,
    MemoryAdmitted,
    MemoryExcluded,
    ExactRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier {
    P1,
    P2,
    P3,
    P4,
    P5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    M1,
    M0,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expectation {
    pub obligation: String,
    pub accepted: Vec<Disposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Disposition {
    Visible,
    Discoverable,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Abstention {
    Permitted,
    Forbidden,
}

/// Accepts exactly the pinned bytes and returns the actual digest otherwise. No normalization
/// applies: whitespace and same-length edits change the digest like any other edit.
pub(crate) fn verify_corpus_identity(bytes: &[u8]) -> Result<(), String> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == CORPUS_SHA256 {
        Ok(())
    } else {
        Err(actual)
    }
}

/// Decodes and validates `bytes` after their identity check.
pub(crate) fn corpus_from(bytes: &[u8]) -> Result<Corpus, String> {
    verify_corpus_identity(bytes).map_err(|actual| format!("corpus digest {actual}"))?;
    let corpus: Corpus = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let violations = validate(&corpus);
    if violations.is_empty() {
        Ok(corpus)
    } else {
        Err(format!("corpus violations: {violations:#?}"))
    }
}

/// The compiled corpus, checked once per test binary. Every fidelity witness reads the oracle
/// through this function.
pub(crate) fn corpus() -> &'static Corpus {
    static CORPUS: LazyLock<Corpus> =
        LazyLock::new(|| corpus_from(CORPUS_BYTES).unwrap_or_else(|error| panic!("{error}")));
    &CORPUS
}

impl Corpus {
    pub(crate) fn case(&self, id: &str) -> Option<&Case> {
        self.cases.iter().find(|case| case.id == id)
    }
}

impl Case {
    pub(crate) fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|source| source.id == id)
    }
}

/// How a span fails to resolve to a native block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unresolved {
    Message,
    Block,
    Revision,
    NoText,
}

impl Source {
    /// The exact native bytes of the block a span names: the original or successor record of
    /// `message_id` whose block at `block_index` carries `revision`. A miss reports the closest
    /// match: a revision mismatch before a missing block before a missing message.
    pub(crate) fn block_bytes(
        &self,
        message_id: &str,
        block_index: usize,
        revision: &str,
    ) -> Result<&str, Unresolved> {
        let mut found = Unresolved::Message;
        for message in self.messages.iter().chain(&self.successors) {
            if message.info.id != message_id {
                continue;
            }
            let Some(part) = message.parts.get(block_index) else {
                if found == Unresolved::Message {
                    found = Unresolved::Block;
                }
                continue;
            };
            if block_revision(message, part).to_string() == revision {
                return block_text(part).ok_or(Unresolved::NoText);
            }
            found = Unresolved::Revision;
        }
        Err(found)
    }
}

/// The canonical revision of a block: a tool's settle time, or the message's completion time,
/// or its creation time when it never completes. These are the revisions the OpenCode source
/// adapter publishes, compared as their canonical decimal strings.
pub(crate) fn block_revision(message: &NativeMessage, part: &Part) -> u64 {
    match part {
        Part::Tool { state, .. } => state.time.end,
        Part::Text { .. } => message
            .info
            .time
            .completed
            .unwrap_or(message.info.time.created),
    }
}

/// The exact native bytes a block contributes: a text part's text or a settled tool's one
/// output or error string.
pub(crate) fn block_text(part: &Part) -> Option<&str> {
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

/// One way the corpus fails its own contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Violation {
    Header(&'static str),
    Case {
        case: String,
        problem: &'static str,
    },
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
    /// An obligation, allowed loss, or memory example breaks its entry rule.
    Entry {
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
pub(crate) fn validate(corpus: &Corpus) -> Vec<Violation> {
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
    id.strip_prefix('C').is_some_and(is_positive_decimal)
}

fn is_positive_decimal(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
}

/// A child ID is `<case>.<kind letter><positive integer>`, for example `C1.S3`.
fn is_child_id(case: &str, kind: char, id: &str) -> bool {
    id.strip_prefix(case)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_prefix(kind))
        .is_some_and(is_positive_decimal)
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

fn entry(id: &str, problem: &'static str) -> Violation {
    Violation::Entry {
        id: id.to_owned(),
        problem,
    }
}

fn validate_case(case: &Case, ids: &mut BTreeSet<String>, out: &mut Vec<Violation>) {
    let case_problem = |problem| Violation::Case {
        case: case.id.clone(),
        problem,
    };
    if let Provenance::Incident { reference } = &case.provenance
        && reference.trim().is_empty()
    {
        out.push(case_problem("incident provenance needs a reference"));
    }
    if case.title.trim().is_empty() {
        out.push(case_problem("a case has a title"));
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
    for follow_up in &case.follow_ups {
        reference(&follow_up.id, &follow_up.source, &sources, out);
    }
    validate_memory_examples(case, out);
    validate_obligations(case, out);
    validate_losses(case, out);
    let exercised = validate_scenarios(case, &sources, out);
    for obligation in &case.obligations {
        if !exercised.contains(obligation.id.as_str()) {
            out.push(Violation::UnexercisedObligation(obligation.id.clone()));
        }
    }
    validate_coverage(case, out);
}

fn validate_memory_examples(case: &Case, out: &mut Vec<Violation>) {
    for memory in &case.memory_examples {
        if memory.scope != "project"
            || memory.text.trim().is_empty()
            || !promotable_categories().contains(&memory.category)
        {
            out.push(entry(
                &memory.id,
                "a memory example is nonempty project memory in a promotable category",
            ));
        }
    }
}

fn validate_obligations(case: &Case, out: &mut Vec<Violation>) {
    let memories: BTreeSet<&str> = case.memory_examples.iter().map(|m| m.id.as_str()).collect();
    for obligation in &case.obligations {
        if obligation.materiality != Materiality::Material {
            out.push(entry(
                &obligation.id,
                "an obligation is material; nonmaterial detail is an allowed loss",
            ));
        }
        match (&obligation.memory, obligation.evidence.is_empty()) {
            (None, true) => out.push(entry(
                &obligation.id,
                "an obligation needs native evidence or a memory example",
            )),
            (Some(_), false) => out.push(entry(
                &obligation.id,
                "an obligation cites native evidence or a memory example, not both",
            )),
            (Some(memory), true) => reference(&obligation.id, memory, &memories, out),
            (None, false) => {}
        }
        for span in &obligation.evidence {
            validate_span(case, &obligation.id, span, out);
        }
    }
}

fn validate_losses(case: &Case, out: &mut Vec<Violation>) {
    for loss in &case.allowed_losses {
        if loss.materiality != Materiality::Nonmaterial {
            out.push(entry(&loss.id, "an allowed loss is marked nonmaterial"));
        }
        if loss.evidence.is_empty() {
            out.push(entry(
                &loss.id,
                "an allowed loss names the native detail it permits losing",
            ));
        }
        for span in &loss.evidence {
            validate_span(case, &loss.id, span, out);
        }
    }
}

/// Checks every scenario's references and expectations and returns the obligations they name.
fn validate_scenarios<'c>(
    case: &'c Case,
    sources: &BTreeSet<&str>,
    out: &mut Vec<Violation>,
) -> BTreeSet<&'c str> {
    let follow_ups: BTreeMap<&str, &FollowUp> =
        case.follow_ups.iter().map(|f| (f.id.as_str(), f)).collect();
    let obligations: BTreeSet<&str> = case.obligations.iter().map(|o| o.id.as_str()).collect();
    let forbidden: BTreeSet<&str> = case
        .forbidden_conclusions
        .iter()
        .map(|f| f.id.as_str())
        .collect();
    let mut exercised = BTreeSet::new();
    for scenario in &case.scenarios {
        let problem = |problem| Violation::Scenario {
            id: scenario.id.clone(),
            problem,
        };
        reference(&scenario.id, &scenario.source, sources, out);
        match follow_ups.get(scenario.follow_up.as_str()) {
            None => out.push(Violation::UnknownReference {
                from: scenario.id.clone(),
                to: scenario.follow_up.clone(),
            }),
            Some(follow_up) if follow_up.source != scenario.source => {
                out.push(problem("the follow-up belongs to another source"))
            }
            Some(_) => {}
        }
        for id in &scenario.forbidden {
            reference(&scenario.id, id, &forbidden, out);
        }
        validate_serving(scenario, out);
        if scenario.expectations.is_empty() {
            out.push(problem("a scenario names the obligations it checks"));
        }
        let mut seen = BTreeSet::new();
        for expectation in &scenario.expectations {
            reference(&scenario.id, &expectation.obligation, &obligations, out);
            exercised.insert(expectation.obligation.as_str());
            if !seen.insert(expectation.obligation.as_str()) {
                out.push(problem("an obligation has one expectation per scenario"));
            }
            let accepted: BTreeSet<_> = expectation.accepted.iter().collect();
            if accepted.is_empty() || accepted.len() != expectation.accepted.len() {
                out.push(problem("accepted dispositions are nonempty and distinct"));
            }
            if accepted.contains(&Disposition::Unavailable)
                && scenario.abstention == Abstention::Forbidden
            {
                out.push(problem(
                    "unavailable evidence is acceptable only where abstention is permitted",
                ));
            }
        }
    }
    exercised
}

/// Every case serves P1 at m1, P2 to P4 naturally, and at least one pressure or omission path.
fn validate_coverage(case: &Case, out: &mut Vec<Violation>) {
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
fn promotable_categories() -> &'static [String] {
    static CATEGORIES: LazyLock<Vec<String>> = LazyLock::new(|| {
        let vocabulary: Value =
            serde_json::from_str(include_str!("../testdata/memory-category-vocabulary.json"))
                .expect("memory category vocabulary decodes");
        serde_json::from_value(vocabulary["PROMOTABLE_CATEGORIES"].clone())
            .expect("promotable categories are strings")
    });
    &CATEGORIES
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
        validate_successor(source, successor, out);
    }
}

/// A successor revises one record of its source: the same role, parts, and tool parts, at least
/// one text changed without changing its byte length, and a later revision.
fn validate_successor(source: &Source, successor: &NativeMessage, out: &mut Vec<Violation>) {
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
        return problem("a successor revises a record of its source");
    };
    if successor.info.role != original.info.role || successor.parts.len() != original.parts.len() {
        return problem("a successor keeps the record's role and parts");
    }
    let mut changed = false;
    let mut later = true;
    for (before, after) in original.parts.iter().zip(&successor.parts) {
        match (before, after) {
            (Part::Text { id: a, text: x }, Part::Text { id: b, text: y }) if a == b => {
                if x.len() != y.len() {
                    problem("a successor text keeps its byte length");
                }
                changed |= x != y;
                later &= block_revision(successor, after) > block_revision(original, before);
            }
            (Part::Tool { .. }, Part::Tool { .. }) if before == after => {}
            _ => problem("a successor keeps each part's identity and every tool part"),
        }
    }
    if !changed {
        problem("a successor changes some text");
    }
    if !later {
        problem("a successor's revision is later than the original's");
    }
}

fn validate_span(case: &Case, owner: &str, span: &Span, out: &mut Vec<Violation>) {
    let unresolved = |problem| Violation::SpanUnresolved {
        owner: owner.to_owned(),
        problem,
    };
    let Some(source) = case.source(&span.source) else {
        out.push(Violation::UnknownReference {
            from: owner.to_owned(),
            to: span.source.clone(),
        });
        return;
    };
    let text = match source.block_bytes(&span.message_id, span.block_index, &span.revision) {
        Ok(text) => text,
        Err(Unresolved::Revision) => {
            out.push(Violation::RevisionMismatch {
                owner: owner.to_owned(),
                message_id: span.message_id.clone(),
                revision: span.revision.clone(),
            });
            return;
        }
        Err(Unresolved::Message) => {
            return out.push(unresolved("the message is not in the source"));
        }
        Err(Unresolved::Block) => {
            return out.push(unresolved("the block index is not in the message"));
        }
        Err(Unresolved::NoText) => return out.push(unresolved("the block has no text")),
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

/// The provider-reachable strings of one part: a text part's text, or a tool's name, every key
/// and string of its input, and its output or error.
fn part_texts(part: &Part) -> Vec<String> {
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
    match part {
        Part::Text { text, .. } => vec![text.clone()],
        Part::Tool { tool, state, .. } => {
            let mut texts = vec![tool.clone()];
            strings(&state.input, &mut texts);
            texts.extend(state.output.clone());
            texts.extend(state.error.clone());
            texts
        }
    }
}

/// Collects every string that can reach a provider: native text and tool fields, follow-up
/// prompts, memory example text, and the approved producer output that is later served.
fn provider_input(corpus: &Corpus) -> Vec<(String, String)> {
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
                    fields.extend(
                        part_texts(part)
                            .into_iter()
                            .map(|text| (field.clone(), text)),
                    );
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
    let mut labels = EVALUATOR_VOCABULARY.map(String::from).to_vec();
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

/// Short label seeds keep the automaton small; each seed hit is confirmed against its whole label.
const LABEL_SEED_BYTES: usize = 8;

/// Overlapping matching reports every seed occurrence, including a seed inside another label, so
/// every label a field contains is confirmed at one of its seed hits.
fn answer_key_exclusion(corpus: &Corpus, out: &mut Vec<Violation>) {
    let labels = evaluator_labels(corpus);
    let needles: Vec<String> = labels.iter().map(|label| label.to_lowercase()).collect();
    let seeds = needles
        .iter()
        .map(|needle| &needle.as_bytes()[..needle.len().min(LABEL_SEED_BYTES)]);
    let matcher = AhoCorasickBuilder::new()
        .kind(Some(AhoCorasickKind::NoncontiguousNFA))
        .build(seeds)
        .expect("evaluator label seeds build one matcher");
    for (field, text) in provider_input(corpus) {
        let haystack = text.to_lowercase();
        let found: BTreeSet<usize> = matcher
            .find_overlapping_iter(&haystack)
            .map(|seed| (seed.start(), seed.pattern().as_usize()))
            .filter(|&(start, index)| {
                haystack.as_bytes()[start..].starts_with(needles[index].as_bytes())
            })
            .map(|(_, index)| index)
            .collect();
        for index in found {
            out.push(Violation::AnswerKeyInProviderInput {
                field: field.clone(),
                label: labels[index].clone(),
            });
        }
    }
}
