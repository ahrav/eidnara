//! Embedded secret-rule loading, validation, preselection, and semantic hashing.
//!
//! Construction verifies source digests before parsing. Rules are ordered by
//! source and name, then indexed by a fixed 256-bit mask. Preselection is
//! allocation-free after construction and preserves rule-vector order.

use std::collections::{BTreeMap, BTreeSet};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, AhoCorasickKind, MatchKind};
use regex::bytes::{Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    ConstructionError, MAX_LOCAL_CONTEXT_BYTES, MAX_RULE_RADIUS, RuleSource, ScanLimits,
    ScanProfile,
};

/// Expected SHA-256 digest of the embedded rule document.
///
/// `NOTICE` records the Gossip-rs corpus digest this document is adapted from;
/// the `facebook-page-access-token` character class is spelled `EAA[CM]` here,
/// the same two-letter set as upstream in the other order.
pub const UPSTREAM_CORPUS_SHA256: &str =
    "5249f06114ae7f48b7f049c1735da373bfbda50e96a95db45488778e45bbfc50";
/// Expected SHA-256 digest of the embedded conservative overlay document.
pub const CONSERVATIVE_OVERLAY_SHA256: &str =
    "973181a0af049fb4c0ae06160cd022b1beae3660b87ac9fa4d498864912b3487";

const UPSTREAM_BYTES: &[u8] = include_bytes!("../default_rules.yaml");
const OVERLAY_BYTES: &[u8] = include_bytes!("../conservative_overlay.yaml");

/// Suppresses an upstream-parity candidate whose surroundings show the value is
/// not a live credential.
///
/// Every pattern describes the value's own syntax or immediate delimiters. A
/// pattern matching only the mood of the surrounding prose belongs in neither
/// list: the window spans 256 bytes, so it would clear a genuine credential that
/// merely sits near documentation.
const CONTEXT_SAFELIST: &[&str] = &[
    r"(?i)\b(?:placeholder|dummy|fake|sample|example|test)[-_ ]{0,3}(?:key|token|secret|password)\b|\b(?:key|token|secret|password)[-_ ]{0,3}(?:placeholder|dummy|fake|sample|example|test)\b",
    r"\bAKIA[0-9A-Z]{9}EXAMPLE\b",
    r"\*{3,}",
    r"[:=]\s*(?:\$\{[A-Za-z_][A-Za-z0-9_]*\}|\$[A-Za-z_][A-Za-z0-9_]*)",
    r"(?i)\$\((?:openssl|uuidgen)\b[^)]*\)",
    r"(?i)[:=]\s*(?:null|changeme|todo|fixme)\b",
    r"(?i)\bhunter2\b",
    r"(?i)\b(?:0123456789|abcdefghij)\b",
    r#"(?i)\b(?:classpath:[^\s"'`]+|xsi:schemaLocation\b|xmlns(?::[A-Za-z0-9_-]+)?=)"#,
    r"(?:\$\{[A-Za-z_][A-Za-z0-9_]*\}|\{\{[A-Za-z_][A-Za-z0-9_]*\}\})",
    r#"(?i)\b(?:https?|ssh)://(?:localhost|(?:[A-Za-z0-9-]+\.)*example(?:\.[A-Za-z]{2,})?)(?::\d+)?(?:/[^\s"']*)?"#,
    r"(?i)(?:<\s*/?\s*(?:secret|token|password)\s*>|(?:secretmanager|vault)://|secret(?:manager)?[:=])",
    r"(?i)\b(?:INSERT[_\s-]?YOUR|REPLACE[_\s-]?WITH)[A-Z0-9_\s-]*\b",
    r"(?:ZXhhbXBsZQ==|c2FtcGxl={0,2}|dGVzdA==)",
    r"(?m)^(?:<{7}|={7}|>{7})(?: .*)?$",
    r"(?i)\b(?:sha(?:1|224|256|384|512)|md5)\s*[:=]\s*[A-Fa-f0-9]{8,}\b",
];

const VALUE_SAFELIST: &[&str] = &[
    r"(?i)^(?:placeholder|dummy|fake|sample|example|test)[-_ ]{0,3}(?:key|token|secret|password)$|^(?:key|token|secret|password)[-_ ]{0,3}(?:placeholder|dummy|fake|sample|example|test)$",
    r"^AKIA[0-9A-Z]{9}EXAMPLE$",
    r"\*{3,}",
    r"(?i)^(?:null|changeme|todo|fixme)$",
    r"(?i)^hunter2$",
    r"(?i)^(?:0123456789|abcdefghij)$",
    r"(?:\$\{[A-Za-z_][A-Za-z0-9_]*\}|\{\{[A-Za-z_][A-Za-z0-9_]*\}\})",
    r"(?i)^(?:INSERT[_\s-]?YOUR|REPLACE[_\s-]?WITH)[A-Z0-9_\s-]*$",
    r"(?:ZXhhbXBsZQ==|c2FtcGxl={0,2}|dGVzdA==)",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuleDocument {
    rules: Vec<RuleDeclaration>,
}

/// Parsed rule policy retained verbatim for semantic hashing.
///
/// Unknown document fields are rejected during deserialization.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleDeclaration {
    pub name: String,
    pub regex: String,
    pub anchors: Vec<String>,
    pub radius: usize,
    #[serde(default)]
    pub must_contain: Option<String>,
    #[serde(default)]
    pub keywords_any: Option<Vec<String>>,
    #[serde(default)]
    pub value_suppressors_any: Option<Vec<String>>,
    #[serde(default)]
    pub entropy: Option<EntropySpec>,
    #[serde(default)]
    pub char_class: Option<CharClassSpec>,
    #[serde(default)]
    pub two_phase: Option<TwoPhaseSpec>,
    #[serde(default)]
    pub local_context: Option<LocalContextSpec>,
    #[serde(default)]
    pub offline_validation: Option<OfflineValidationSpec>,
    #[serde(default)]
    pub secret_group: Option<u16>,
    #[serde(default)]
    pub uuid_format_secret: bool,
    #[serde(default)]
    pub min_confidence: Option<i8>,
    #[serde(default)]
    pub key_group: Option<String>,
    #[serde(default)]
    pub value_group: Option<String>,
    #[serde(default)]
    pub reject_scalars: bool,
    /// Runs the engine's context and value safelists for overlay rules that mirror corpus rules, producing one verdict when both match a credential.
    #[serde(default)]
    pub upstream_parity: bool,
    /// Compiles this rule with Unicode mode on, so `\s` and negated classes span the
    /// Unicode whitespace set rather than the ASCII subset the byte default matches.
    ///
    /// The corpus rules stay byte-oriented, which is how their upstream shapes were
    /// adapted. A rule that separates a key from a value needs the wider set on both
    /// sides of the separator: treating a code point as a separator while the value
    /// class treats the same code point as content makes the value run past it.
    #[serde(default)]
    pub unicode: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntropySpec {
    pub min_bits_per_byte: f32,
    pub min_len: usize,
    pub max_len: usize,
    #[serde(default)]
    pub min_entropy_bits_per_byte: Option<f32>,
    #[serde(default)]
    pub digit_penalty: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CharClassSpec {
    pub max_lower_pct: u8,
    pub min_window_len: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TwoPhaseSpec {
    pub seed_radius: usize,
    pub full_radius: usize,
    pub confirm_any: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocalContextSpec {
    pub lookbehind: usize,
    pub lookahead: usize,
    #[serde(default)]
    pub require_same_line_assignment: bool,
    #[serde(default)]
    pub require_quoted: bool,
    #[serde(default)]
    pub key_names_any: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OfflineValidationSpec {
    #[serde(rename = "type")]
    pub kind: OfflineValidationKind,
    #[serde(default)]
    pub prefix_skip: Option<u8>,
    #[serde(default)]
    pub payload_len: Option<u8>,
    #[serde(default)]
    pub checksum_len: Option<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OfflineValidationKind {
    Crc32Base62,
    GithubFineGrainedPat,
    GrafanaServiceAccount,
    AwsAccessKey,
    SentryOrgToken,
    PypiToken,
    SlackToken,
}

/// Compiled declaration and its prepared candidate filters.
pub(crate) struct Rule {
    pub source: RuleSource,
    pub declaration: RuleDeclaration,
    pub regex: Regex,
    pub keyword_matcher: Option<AhoCorasick>,
    pub suppressor_matcher: Option<AhoCorasick>,
    /// Indices of the unnamed capture groups of `regex`, in group order.
    pub unnamed_captures: Vec<usize>,
    /// A byte every match of `regex` contains; input without it skips the rule.
    pub required_byte: Option<u8>,
    /// The unnamed capture whose pattern starts and ends with the same literal quote byte. Findings report the enclosed bytes; value gates read the whole capture.
    pub quoted_capture: Option<usize>,
    /// Added to each candidate's confidence before the minimum is applied.
    pub confidence_bonus: i8,
    /// Which source-code value shapes the evaluator rejects for this rule.
    pub code_reference_gate: Option<CodeReferenceGate>,
    pub encoded_declaration: Vec<u8>,
}

/// Keyed rules whose unquoted value can be source code naming a secret, as in
/// `key = event.id`, rather than the secret itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodeReferenceGate {
    /// Rejects qualified identifier paths and `{name}` templates.
    Assignment,
    /// Also rejects the upstream rule's digit-free values and `name=value`
    /// expressions whose right side is a scalar or a code reference.
    GenericApiKey,
}

/// Verified rule collection with anchor and safelist indexes.
pub(crate) struct RuleSet {
    rules: Vec<Rule>,
    anchors: AhoCorasick,
    /// Rules declaring the anchor at the same index in `anchors`.
    anchor_owners: Vec<RuleMask>,
    overlay: RuleMask,
    context_safelist: RegexSet,
    value_safelist: RegexSet,
}

/// Inline bit set over rule indices, so preselection allocates nothing and
/// iteration visits only the selected rules.
#[derive(Clone, Copy, Default)]
pub(crate) struct RuleMask {
    words: [u64; Self::WORDS],
}

impl RuleMask {
    const WORDS: usize = 4;
    pub const CAPACITY: usize = Self::WORDS * 64;

    fn insert(&mut self, index: usize) {
        self.words[index / 64] |= 1u64 << (index % 64);
    }

    fn union(self, other: Self) -> Self {
        let mut words = [0u64; Self::WORDS];
        for (slot, (left, right)) in words
            .iter_mut()
            .zip(self.words.into_iter().zip(other.words))
        {
            *slot = left | right;
        }
        Self { words }
    }

    fn intersect(self, other: Self) -> Self {
        let mut words = [0u64; Self::WORDS];
        for (slot, (left, right)) in words
            .iter_mut()
            .zip(self.words.into_iter().zip(other.words))
        {
            *slot = left & right;
        }
        Self { words }
    }

    /// Yields selected indices in ascending order, which is the order the
    /// rule vector defines.
    fn iter(self) -> impl Iterator<Item = usize> {
        self.words
            .into_iter()
            .enumerate()
            .flat_map(|(offset, mut word)| {
                std::iter::from_fn(move || {
                    (word != 0).then(|| {
                        let bit = word.trailing_zeros() as usize;
                        word &= word - 1;
                        offset * 64 + bit
                    })
                })
            })
    }
}

impl RuleSet {
    /// Loads embedded upstream and overlay documents after digest verification.
    ///
    /// # Errors
    ///
    /// Returns [`ConstructionError`] for digest mismatch, malformed YAML,
    /// duplicate or invalid policy, excess rule count, or regex construction
    /// failure.
    pub fn from_embedded() -> Result<Self, ConstructionError> {
        Self::from_sources(
            UPSTREAM_BYTES,
            UPSTREAM_CORPUS_SHA256,
            OVERLAY_BYTES,
            CONSERVATIVE_OVERLAY_SHA256,
        )
    }

    fn from_sources(
        upstream: &[u8],
        upstream_digest: &str,
        overlay: &[u8],
        overlay_digest: &str,
    ) -> Result<Self, ConstructionError> {
        verify_digest(
            upstream,
            upstream_digest,
            ConstructionError::CorpusDigestMismatch,
        )?;
        verify_digest(
            overlay,
            overlay_digest,
            ConstructionError::OverlayDigestMismatch,
        )?;

        let mut rules = parse_document(upstream, RuleSource::Upstream)?;
        rules.extend(parse_document(overlay, RuleSource::ConservativeOverlay)?);
        let mut identities = BTreeSet::new();
        for rule in &rules {
            if !identities.insert(rule.declaration.name.clone()) {
                return Err(ConstructionError::InvalidRuleIdentity);
            }
        }
        rules.sort_by(|left, right| {
            (left.source, left.declaration.name.as_str())
                .cmp(&(right.source, right.declaration.name.as_str()))
        });
        if rules.len() > RuleMask::CAPACITY {
            return Err(ConstructionError::InvalidRulePolicy);
        }
        let (anchors, anchor_owners) = build_anchor_index(&rules)?;
        let mut overlay = RuleMask::default();
        for (index, rule) in rules.iter().enumerate() {
            if rule.source == RuleSource::ConservativeOverlay {
                overlay.insert(index);
            }
        }
        let context_safelist = build_regex_set(CONTEXT_SAFELIST)?;
        let value_safelist = build_regex_set(VALUE_SAFELIST)?;
        Ok(Self {
            rules,
            anchors,
            anchor_owners,
            overlay,
            context_safelist,
            value_safelist,
        })
    }

    /// Iterates active rules in canonical source-and-name order.
    ///
    /// Comprehensive scans include both sources. Conservative scans include only
    /// overlay rules.
    pub fn active(&self, profile: ScanProfile) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(move |rule| {
            profile == ScanProfile::Comprehensive || rule.source == RuleSource::ConservativeOverlay
        })
    }

    /// Returns the active rules whose declared anchors occur in `bytes`.
    ///
    /// A rule runs only after at least one declared anchor matches, compared
    /// ASCII-case-insensitively. Anchors may overlap. `anchor_proof` checks
    /// that skipping a rule cannot drop one of its findings.
    pub fn preselect(&self, profile: ScanProfile, bytes: &[u8]) -> impl Iterator<Item = &Rule> {
        let mut anchored = RuleMask::default();
        for hit in self.anchors.find_overlapping_iter(bytes) {
            if let Some(owners) = self.anchor_owners.get(hit.pattern().as_usize()) {
                anchored = anchored.union(*owners);
            }
        }
        let selected = match profile {
            ScanProfile::Comprehensive => anchored,
            ScanProfile::Conservative => anchored.intersect(self.overlay),
        };
        selected.iter().map(|index| &self.rules[index])
    }

    /// Tests candidate context bytes against context suppressors.
    pub fn context_is_safelisted(&self, bytes: &[u8]) -> bool {
        self.context_safelist.is_match(bytes)
    }

    /// Tests candidate value bytes against value-only suppressors.
    pub fn value_is_safelisted(&self, bytes: &[u8]) -> bool {
        self.value_safelist.is_match(bytes)
    }

    /// Binds rules, profile, and limits into one digest. Evaluator constants are not hashed: `REVISION.semantic_digest_version` binds them and must change with them, and `evaluator::tests::evaluator_constants_are_pinned` trips when one of the tables changes.
    ///
    /// Integer limits use little-endian 64-bit encoding. Rules are sorted before
    /// encoding, so storage order does not affect the digest.
    pub fn semantic_digest(
        &self,
        profile: ScanProfile,
        limits: ScanLimits,
    ) -> Result<[u8; 32], ConstructionError> {
        self.semantic_digest_with_version(
            profile,
            limits,
            crate::api::REVISION.semantic_digest_version,
        )
    }

    fn semantic_digest_with_version(
        &self,
        profile: ScanProfile,
        limits: ScanLimits,
        evaluator_version: u8,
    ) -> Result<[u8; 32], ConstructionError> {
        let mut hash = Sha256::new();
        hash.update(b"eidnara.secret-scanner.semantics\0");
        hash.update(b"direct-evaluator\0");
        hash.update([evaluator_version]);
        hash.update(UPSTREAM_CORPUS_SHA256.as_bytes());
        hash.update(CONSERVATIVE_OVERLAY_SHA256.as_bytes());
        hash.update([profile.tag()]);
        hash.update((limits.max_input_bytes as u64).to_le_bytes());
        hash.update((limits.max_candidates as u64).to_le_bytes());
        hash.update((limits.max_work_bytes as u64).to_le_bytes());
        hash_safelists(&mut hash, CONTEXT_SAFELIST, VALUE_SAFELIST);
        let mut active: Vec<_> = self.active(profile).collect();
        active.sort_by(|left, right| {
            (left.source, left.declaration.name.as_str())
                .cmp(&(right.source, right.declaration.name.as_str()))
        });
        for rule in active {
            encode_rule(&mut hash, rule);
        }
        Ok(hash.finalize().into())
    }
}

// Tags distinguish context and value lists in the digest, so moving a pattern between them changes it even though the concatenated bytes do not.
fn hash_safelists(hash: &mut Sha256, context: &[&str], value: &[&str]) {
    for (tag, patterns) in [(b'c', context), (b'v', value)] {
        hash.update([tag]);
        hash.update((patterns.len() as u64).to_le_bytes());
        for pattern in patterns {
            hash.update((pattern.len() as u64).to_le_bytes());
            hash.update(pattern.as_bytes());
        }
    }
}

/// Anchors are matched ASCII-case-insensitively, so spellings that differ only in
/// case, and the same anchor declared by several rules, compile to one pattern
/// whose owner mask names every declaring rule. An overlapping search reports
/// each pattern separately at every position it ends, so on anchor-dense text
/// the duplicate patterns cost more than the automaton walk itself.
fn build_anchor_index(rules: &[Rule]) -> Result<(AhoCorasick, Vec<RuleMask>), ConstructionError> {
    let mut patterns: Vec<Vec<u8>> = Vec::new();
    let mut owners: Vec<RuleMask> = Vec::new();
    let mut slots: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (index, rule) in rules.iter().enumerate() {
        for anchor in &rule.declaration.anchors {
            let folded = anchor.to_ascii_lowercase().into_bytes();
            let slot = *slots.entry(folded).or_insert_with(|| {
                patterns.push(anchor.as_bytes().to_vec());
                owners.push(RuleMask::default());
                patterns.len() - 1
            });
            owners[slot].insert(index);
        }
    }
    let automaton = AhoCorasickBuilder::new()
        .match_kind(MatchKind::Standard)
        .ascii_case_insensitive(true)
        // Overlapping search cannot use a prefilter, so the automaton walks
        // every input byte; a DFA resolves each byte with one table lookup.
        .kind(Some(AhoCorasickKind::DFA))
        .build(&patterns)
        .map_err(|_| ConstructionError::InvalidRulePolicy)?;
    Ok((automaton, owners))
}

fn build_regex_set(patterns: &[&str]) -> Result<RegexSet, ConstructionError> {
    let mut builder = RegexSetBuilder::new(patterns);
    builder.unicode(false).size_limit(16 * 1024 * 1024);
    builder
        .build()
        .map_err(|_| ConstructionError::InvalidRulePattern)
}

fn parse_document(bytes: &[u8], source: RuleSource) -> Result<Vec<Rule>, ConstructionError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConstructionError::InvalidRuleDocument)?;
    let document: RuleDocument =
        serde_norway::from_str(text).map_err(|_| ConstructionError::InvalidRuleDocument)?;
    if document.rules.is_empty() {
        return Err(ConstructionError::InvalidRuleDocument);
    }
    compile_rules(source, document.rules)
}

fn compile_rules(
    source: RuleSource,
    declarations: Vec<RuleDeclaration>,
) -> Result<Vec<Rule>, ConstructionError> {
    const MAX_COMPILE_THREADS: usize = 8;
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAX_COMPILE_THREADS);
    compile_rules_on(source, declarations, workers, spawn_compile_worker)
}

type WorkerSpawn = for<'scope, 'env> fn(
    &'scope std::thread::Scope<'scope, 'env>,
    &'env (dyn Fn() + Sync),
) -> std::io::Result<()>;

fn spawn_compile_worker<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    work: &'env (dyn Fn() + Sync),
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .spawn_scoped(scope, work)
        .map(drop)
}

fn compile_rules_on(
    source: RuleSource,
    declarations: Vec<RuleDeclaration>,
    workers: usize,
    spawn: WorkerSpawn,
) -> Result<Vec<Rule>, ConstructionError> {
    let workers = workers.min(declarations.len());
    if workers <= 1 {
        return declarations
            .into_iter()
            .map(|declaration| compile_rule(source, declaration))
            .collect();
    }
    let pending: Vec<std::sync::Mutex<Option<RuleDeclaration>>> = declarations
        .into_iter()
        .map(|declaration| std::sync::Mutex::new(Some(declaration)))
        .collect();
    let compiled: Vec<std::sync::Mutex<Option<Result<Rule, ConstructionError>>>> = pending
        .iter()
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let work = || {
        loop {
            let index = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(slot) = pending.get(index) else {
                return;
            };
            let declaration = slot
                .lock()
                .expect("no worker panics while holding a slot")
                .take()
                .expect("the cursor hands out each index once");
            let result = compile_rule(source, declaration);
            *compiled[index]
                .lock()
                .expect("no worker panics while holding a slot") = Some(result);
        }
    };
    std::thread::scope(|scope| {
        for _ in 1..workers {
            if spawn(scope, &work).is_err() {
                break;
            }
        }
        work();
    });
    compiled
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("no worker panics while holding a slot")
                .expect("the calling thread drains every declaration")
        })
        .collect()
}

pub(crate) fn compile_rule(
    source: RuleSource,
    declaration: RuleDeclaration,
) -> Result<Rule, ConstructionError> {
    validate_policy(&declaration)?;
    let mut builder = RegexBuilder::new(&declaration.regex);
    builder
        .unicode(declaration.unicode)
        .size_limit(128 * 1024 * 1024);
    let regex = builder
        .build()
        .map_err(|_| ConstructionError::InvalidRulePattern)?;
    if declaration
        .secret_group
        .is_some_and(|group| usize::from(group) >= regex.captures_len())
    {
        return Err(ConstructionError::InvalidRulePattern);
    }
    let capture_names: BTreeSet<_> = regex.capture_names().flatten().collect();
    if declaration
        .key_group
        .as_deref()
        .is_some_and(|name| !capture_names.contains(name))
        || declaration
            .value_group
            .as_deref()
            .is_some_and(|name| !capture_names.contains(name))
    {
        return Err(ConstructionError::InvalidRulePattern);
    }
    let keyword_matcher = declaration
        .keywords_any
        .as_deref()
        .map(build_case_insensitive_matcher)
        .transpose()?;
    let suppressor_matcher = declaration
        .value_suppressors_any
        .as_deref()
        .map(build_case_insensitive_matcher)
        .transpose()?;
    let unnamed_captures = regex
        .capture_names()
        .enumerate()
        .skip(1)
        .filter(|(_, name)| name.is_none())
        .map(|(index, _)| index)
        .collect();
    // The value group `("[a-z0-9=_\-]{8,20}")` makes a double quote a necessary byte of every match.
    let required_byte = (declaration.name == "hashicorp-tf-password").then_some(b'"');
    let quoted_capture = (declaration.name == "hashicorp-tf-password").then_some(1);
    let confidence_bonus = if declaration.name == "generic-api-key" {
        2
    } else {
        0
    };
    let code_reference_gate = match declaration.name.as_str() {
        "magic-keyed-assignment" => Some(CodeReferenceGate::Assignment),
        "generic-api-key" => Some(CodeReferenceGate::GenericApiKey),
        _ => None,
    };
    let encoded_declaration =
        serde_json::to_vec(&declaration).map_err(|_| ConstructionError::InvalidRulePolicy)?;
    Ok(Rule {
        source,
        declaration,
        regex,
        keyword_matcher,
        suppressor_matcher,
        unnamed_captures,
        required_byte,
        quoted_capture,
        confidence_bonus,
        code_reference_gate,
        encoded_declaration,
    })
}

fn build_case_insensitive_matcher(patterns: &[String]) -> Result<AhoCorasick, ConstructionError> {
    AhoCorasickBuilder::new()
        // The evaluator probes these matchers with `find_overlapping_iter`, which panics for every match kind except `Standard`.
        .match_kind(MatchKind::Standard)
        .ascii_case_insensitive(true)
        .build(patterns)
        .map_err(|_| ConstructionError::InvalidRulePattern)
}

fn validate_policy(rule: &RuleDeclaration) -> Result<(), ConstructionError> {
    let non_empty_list =
        |values: &Vec<String>| !values.is_empty() && !values.iter().any(String::is_empty);
    let bits_per_byte = |value: f32| value.is_finite() && (0.0..=8.0).contains(&value);

    let identity = !rule.name.is_empty()
        && non_empty_list(&rule.anchors)
        && rule
            .must_contain
            .as_ref()
            .is_none_or(|needle| !needle.is_empty())
        && rule.keywords_any.as_ref().is_none_or(non_empty_list)
        && rule
            .value_suppressors_any
            .as_ref()
            .is_none_or(non_empty_list);
    let entropy = rule.entropy.as_ref().is_none_or(|spec| {
        bits_per_byte(spec.min_bits_per_byte)
            && spec.min_len > 0
            && spec.min_len <= spec.max_len
            && spec.min_entropy_bits_per_byte.is_none_or(bits_per_byte)
    });
    let windows = rule
        .char_class
        .as_ref()
        .is_none_or(|spec| spec.max_lower_pct <= 100 && spec.min_window_len >= 16)
        && rule.radius <= MAX_RULE_RADIUS
        && rule.two_phase.as_ref().is_none_or(|spec| {
            spec.seed_radius <= spec.full_radius
                && spec.full_radius <= MAX_RULE_RADIUS
                && non_empty_list(&spec.confirm_any)
        })
        && rule.local_context.as_ref().is_none_or(|spec| {
            spec.lookbehind <= MAX_LOCAL_CONTEXT_BYTES
                && spec.lookahead <= MAX_LOCAL_CONTEXT_BYTES
                && spec.key_names_any.as_ref().is_none_or(non_empty_list)
        })
        && rule
            .min_confidence
            .is_none_or(|value| (0..=10).contains(&value));
    let offline = rule
        .offline_validation
        .as_ref()
        .is_none_or(|spec| match spec.kind {
            OfflineValidationKind::Crc32Base62 => {
                spec.prefix_skip.is_some()
                    && spec.payload_len.is_some_and(|value| value > 0)
                    && spec
                        .checksum_len
                        .is_some_and(|value| (1..=6).contains(&value))
            }
            _ => {
                spec.prefix_skip.is_none()
                    && spec.payload_len.is_none()
                    && spec.checksum_len.is_none()
            }
        });

    if identity && entropy && windows && offline {
        Ok(())
    } else {
        Err(ConstructionError::InvalidRulePolicy)
    }
}

pub(crate) fn digest_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn verify_digest(
    bytes: &[u8],
    expected: &str,
    error: ConstructionError,
) -> Result<(), ConstructionError> {
    if digest_hex(bytes) == expected {
        Ok(())
    } else {
        Err(error)
    }
}

fn encode_rule(hash: &mut Sha256, rule: &Rule) {
    hash.update([match rule.source {
        RuleSource::Upstream => 1,
        RuleSource::ConservativeOverlay => 2,
    }]);
    hash.update((rule.encoded_declaration.len() as u64).to_le_bytes());
    hash.update(&rule.encoded_declaration);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pattern count above which `AhoCorasickKind::Auto` stops building a DFA and falls back to a contiguous NFA.
    const AUTO_DFA_PATTERN_LIMIT: usize = 100;

    /// A matching rule must be preselected because unselected rules are not
    /// evaluated.
    #[test]
    fn preselection_never_drops_a_rule_whose_pattern_matches() {
        let rules = RuleSet::from_embedded().unwrap();
        let corpus = [
            "password=hunter2",
            "AKIAIOSFODNN7EXAMPLE",
            "A3-0A1B2C-3D4E5F6G7H8-9I0J1-2K3L4-5M6N7",
            "AGE-SECRET-KEY-1QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7LQPZRY9X8GF2TVDW0S3JN54KHCE",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----",
            "-----begin rsa private key-----\nMIIEowIBAAKCAQEA\n-----end rsa private key-----",
            "Netlify_Api_Key: Ab3fGh1jKlMnOpQrStUvWxYz79PqRs24Tv68Wt-Q",
            "glsa_AbCdEfGhIjKlMnOpQrStUvWxYz012345_0a1b2c3d",
            "github_pat_11ABCDEFG0abcdefghijkl_MnOpQrStUvWxYz0123456789AbCdEfGhIjKlMnOpQrStUvWx",
            "pypi-AgEIcHlwaS5vcmcCJDAwMDAwMDAw",
            "sntrys_eyJpYXQiOjE2OTk5OTk5OTl9_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab",
            "zxlk ZXlKclpYbGZiM0J6SWpwY0lqRXlNelExTmpjNE9UQXhNak0wTlRZM09EazBNVEl6TkRVMg",
            "'api_token': 'xy'",
            r#"{"clientSecret":"hunter-two"}"#,
            "é password=hunter2 é",
            "the quick brown fox jumps over the lazy dog",
        ];
        for profile in [ScanProfile::Conservative, ScanProfile::Comprehensive] {
            for input in corpus {
                let bytes = input.as_bytes();
                let selected: BTreeSet<&str> = rules
                    .preselect(profile, bytes)
                    .map(|rule| rule.declaration.name.as_str())
                    .collect();
                for rule in rules.active(profile) {
                    if rule.regex.is_match(bytes) {
                        assert!(
                            selected.contains(rule.declaration.name.as_str()),
                            "{} matches {input:?} but was not preselected",
                            rule.declaration.name
                        );
                    }
                }
            }
        }
    }

    // The evaluator probes keyword and suppressor matchers with `find_overlapping_iter`, which panics for every match kind except `Standard`.
    // `AhoCorasickKind::Auto` builds a DFA for an unanchored automaton holding at most 100 patterns, so the kind assertion below holds without pinning the kind and forfeiting the contiguous-NFA fallback that keeps a larger list from failing construction.
    #[test]
    fn every_prepared_matcher_supports_overlapping_search() {
        let rules = RuleSet::from_embedded().unwrap();
        let mut prepared = 0;
        for rule in rules.active(ScanProfile::Comprehensive) {
            for (patterns, matcher) in [
                (&rule.declaration.keywords_any, &rule.keyword_matcher),
                (
                    &rule.declaration.value_suppressors_any,
                    &rule.suppressor_matcher,
                ),
            ] {
                let (Some(patterns), Some(matcher)) = (patterns, matcher) else {
                    continue;
                };
                assert!(
                    matches!(matcher.match_kind(), MatchKind::Standard),
                    "{} builds a matcher whose match kind rejects overlapping search",
                    rule.declaration.name
                );
                assert!(
                    patterns.len() <= AUTO_DFA_PATTERN_LIMIT,
                    "{} declares {} patterns, past the limit that keeps `Auto` on a DFA",
                    rule.declaration.name,
                    patterns.len()
                );
                assert!(
                    matches!(matcher.kind(), AhoCorasickKind::DFA),
                    "{} resolves overlapping search with {:?} rather than a DFA",
                    rule.declaration.name,
                    matcher.kind()
                );
                let _ = matcher
                    .find_overlapping_iter(b"placeholder example")
                    .count();
                prepared += 1;
            }
        }
        assert!(prepared > 0, "no rule compiled a prepared matcher");
    }

    /// A document that does not deserialize is a document error; a document
    /// that deserializes with an empty name is a policy error, because the
    /// identity loop assumes every parsed name is non-empty.
    #[test]
    fn malformed_rule_documents_return_their_typed_errors() {
        for (document, expected) in [
            (
                b"rules:\n- name: bad\n".as_slice(),
                ConstructionError::InvalidRuleDocument,
            ),
            (
                b"rules:\n- name: ''\n  regex: 'x'\n  anchors: ['x']\n  radius: 16\n".as_slice(),
                ConstructionError::InvalidRulePolicy,
            ),
        ] {
            assert_eq!(
                parse_document(document, RuleSource::Upstream).err(),
                Some(expected),
                "{}",
                String::from_utf8_lossy(document)
            );
        }
    }

    fn refuse_spawn<'scope, 'env>(
        _scope: &'scope std::thread::Scope<'scope, 'env>,
        _work: &'env (dyn Fn() + Sync),
    ) -> std::io::Result<()> {
        Err(std::io::ErrorKind::WouldBlock.into())
    }

    thread_local! {
        static SPAWNED_ONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn spawn_one_then_refuse<'scope, 'env>(
        scope: &'scope std::thread::Scope<'scope, 'env>,
        work: &'env (dyn Fn() + Sync),
    ) -> std::io::Result<()> {
        if SPAWNED_ONE.with(|spawned| spawned.replace(true)) {
            refuse_spawn(scope, work)
        } else {
            spawn_compile_worker(scope, work)
        }
    }

    #[test]
    fn parallel_compilation_keeps_declaration_order_and_sequential_error_selection() {
        let declaration = |name: &str, regex: &str| {
            format!("- name: '{name}'\n  regex: '{regex}'\n  anchors: ['x']\n  radius: 16\n")
        };
        let names: Vec<String> = (0..37).map(|index| format!("rule-{index:02}")).collect();
        let valid = names
            .iter()
            .fold(String::from("rules:\n"), |mut document, name| {
                document.push_str(&declaration(name, "x[0-9]+"));
                document
            });
        let mut two_errors = String::from("rules:\n");
        for (index, name) in names.iter().enumerate() {
            match index {
                9 => two_errors.push_str(&declaration(name, "x(")),
                20 => two_errors.push_str(&declaration("", "x")),
                _ => two_errors.push_str(&declaration(name, "x[0-9]+")),
            }
        }
        let policy_error = two_errors.replacen("regex: 'x('", "regex: 'x'", 1);

        let rules = parse_document(valid.as_bytes(), RuleSource::Upstream).unwrap();
        let compiled: Vec<&str> = rules
            .iter()
            .map(|rule| rule.declaration.name.as_str())
            .collect();
        assert_eq!(compiled, names);
        assert_eq!(
            parse_document(two_errors.as_bytes(), RuleSource::Upstream).err(),
            Some(ConstructionError::InvalidRulePattern),
            "the earlier declaration's error wins"
        );
        assert_eq!(
            parse_document(policy_error.as_bytes(), RuleSource::Upstream).err(),
            Some(ConstructionError::InvalidRulePolicy)
        );

        let spawners: [(&str, WorkerSpawn); 3] = [
            ("spawned helpers", spawn_compile_worker),
            ("every spawn refused", refuse_spawn),
            ("one helper, then refused", spawn_one_then_refuse),
        ];
        let declarations = |document: &str| {
            serde_norway::from_str::<RuleDocument>(document)
                .unwrap()
                .rules
        };
        for (label, spawn) in spawners {
            let compile = |document: &str| {
                SPAWNED_ONE.with(|spawned| spawned.set(false));
                compile_rules_on(RuleSource::Upstream, declarations(document), 4, spawn)
            };
            let rules = compile(&valid).unwrap();
            let compiled: Vec<&str> = rules
                .iter()
                .map(|rule| rule.declaration.name.as_str())
                .collect();
            assert_eq!(compiled, names, "{label}");
            assert_eq!(
                compile(&two_errors).err(),
                Some(ConstructionError::InvalidRulePattern),
                "{label}: the earlier declaration's error wins"
            );
            assert_eq!(
                compile(&policy_error).err(),
                Some(ConstructionError::InvalidRulePolicy),
                "{label}"
            );
        }
    }

    #[test]
    fn construction_wiring_rejects_source_tampering() {
        assert_eq!(
            RuleSet::from_sources(
                b"changed corpus",
                UPSTREAM_CORPUS_SHA256,
                OVERLAY_BYTES,
                CONSERVATIVE_OVERLAY_SHA256,
            )
            .err(),
            Some(ConstructionError::CorpusDigestMismatch)
        );
        assert_eq!(
            RuleSet::from_sources(
                UPSTREAM_BYTES,
                UPSTREAM_CORPUS_SHA256,
                b"changed overlay",
                CONSERVATIVE_OVERLAY_SHA256,
            )
            .err(),
            Some(ConstructionError::OverlayDigestMismatch)
        );
    }

    // A pattern matching only the mood of the surrounding prose clears any candidate
    // inside a 256-byte window, including a live credential quoted beside
    // documentation, so no context pattern may match prose alone.
    #[test]
    fn no_context_pattern_matches_prose_without_a_credential_shaped_neighbour() {
        let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";
        let rules = RuleSet::from_embedded().unwrap();
        for prose in [
            "for example",
            "sample config",
            "example config",
            "fixtures",
            "__tests__",
            "mocks",
        ] {
            let window = format!("{prose}: {secret}");
            assert!(
                !rules.context_is_safelisted(window.as_bytes()),
                "{prose:?} safelists a window holding a credential"
            );
        }
    }

    /// Detects changes to the domain string, digest version, embedded
    /// documents, safelists, default limits, or encoding. The recorded values
    /// are hex-encoded semantic digests.
    #[test]
    fn default_semantic_digests_match_recorded_values() {
        let rules = RuleSet::from_embedded().unwrap();
        for (profile, expected) in [
            (
                ScanProfile::Conservative,
                "69235bd55d45ad0e6e80caf378cce8a3ee27087a13190c26776864264503bf22",
            ),
            (
                ScanProfile::Comprehensive,
                "c19adcf9d5dc7aa972f008fdb12a0cf8ceeb520b1ed4a73b5211d0c0a629dadb",
            ),
        ] {
            let digest = rules
                .semantic_digest(profile, ScanLimits::default())
                .unwrap();
            assert_eq!(hex(&digest), expected, "{profile:?}");
        }
    }

    /// The digest depends on rule semantics, scan limits, and evaluator
    /// version, not on corpus order.
    #[test]
    fn semantic_digest_ignores_corpus_order_and_tracks_every_finding_affecting_input() {
        let mut rules = RuleSet::from_embedded().unwrap();
        let base = ScanLimits::default();
        let expected = rules
            .semantic_digest(ScanProfile::Comprehensive, base)
            .unwrap();
        for limits in [
            ScanLimits {
                max_input_bytes: base.max_input_bytes - 1,
                ..base
            },
            ScanLimits {
                max_candidates: base.max_candidates - 1,
                ..base
            },
            ScanLimits {
                max_work_bytes: base.max_work_bytes - 1,
                ..base
            },
        ] {
            assert_ne!(
                rules
                    .semantic_digest(ScanProfile::Comprehensive, limits)
                    .unwrap(),
                expected
            );
        }
        assert_ne!(
            rules
                .semantic_digest_with_version(
                    ScanProfile::Comprehensive,
                    base,
                    crate::api::REVISION.semantic_digest_version + 1,
                )
                .unwrap(),
            expected
        );
        rules.rules.reverse();
        assert_eq!(
            rules
                .semantic_digest(ScanProfile::Comprehensive, base)
                .unwrap(),
            expected
        );
        let mut widened = rules.rules[0].declaration.clone();
        widened.radius += 1;
        rules.rules[0] = compile_rule(rules.rules[0].source, widened).unwrap();
        assert_ne!(
            rules
                .semantic_digest(ScanProfile::Comprehensive, base)
                .unwrap(),
            expected
        );
    }

    fn safelist_digest(context: &[&str], value: &[&str]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash_safelists(&mut hash, context, value);
        hash.finalize().into()
    }

    /// Each pair shares its pattern bytes but differs in list membership or
    /// pattern boundaries.
    #[test]
    fn safelist_digest_separates_list_membership_and_pattern_boundaries() {
        let moved_context_tail: Vec<&str> = {
            let mut moved = vec![CONTEXT_SAFELIST[CONTEXT_SAFELIST.len() - 1]];
            moved.extend_from_slice(VALUE_SAFELIST);
            moved
        };
        for (left, right) in [
            (
                safelist_digest(&["a", "b"], &["c"]),
                safelist_digest(&["a"], &["b", "c"]),
            ),
            (
                safelist_digest(CONTEXT_SAFELIST, VALUE_SAFELIST),
                safelist_digest(
                    &CONTEXT_SAFELIST[..CONTEXT_SAFELIST.len() - 1],
                    &moved_context_tail,
                ),
            ),
            (
                safelist_digest(&["ab", "c"], VALUE_SAFELIST),
                safelist_digest(&["a", "bc"], VALUE_SAFELIST),
            ),
        ] {
            assert_ne!(left, right);
        }
    }

    #[test]
    fn every_safelist_pattern_reaches_the_safelist_digest() {
        for index in 0..CONTEXT_SAFELIST.len() {
            let mut shortened = CONTEXT_SAFELIST.to_vec();
            shortened.remove(index);
            assert_ne!(
                safelist_digest(CONTEXT_SAFELIST, VALUE_SAFELIST),
                safelist_digest(&shortened, VALUE_SAFELIST),
                "dropping context pattern {index} left the digest unchanged"
            );
        }
        for index in 0..VALUE_SAFELIST.len() {
            let mut shortened = VALUE_SAFELIST.to_vec();
            shortened.remove(index);
            assert_ne!(
                safelist_digest(CONTEXT_SAFELIST, VALUE_SAFELIST),
                safelist_digest(CONTEXT_SAFELIST, &shortened),
                "dropping value pattern {index} left the digest unchanged"
            );
        }
    }

    // A mixed rule would let a named structural group decide the reported secret span.
    #[test]
    fn no_rule_mixes_named_and_unnamed_undeclared_capture_groups() {
        let rules = RuleSet::from_embedded().unwrap();
        for rule in rules.active(ScanProfile::Comprehensive) {
            if rule.declaration.secret_group.is_some() || rule.declaration.value_group.is_some() {
                continue;
            }
            let named = rule.regex.capture_names().flatten().count();
            let unnamed = rule.regex.captures_len() - 1 - named;
            assert!(
                named == 0 || unnamed == 0,
                "{} declares {named} named and {unnamed} unnamed groups without a secret_group",
                rule.declaration.name
            );
        }
    }

    // `evaluate_candidate` awards `generic-api-key` a confidence bonus by name;
    // dropping its declared floor would leave that bonus against the generic
    // fallback minimum.
    #[test]
    fn the_rule_the_evaluator_names_declares_its_own_confidence_floor() {
        let rules = RuleSet::from_embedded().unwrap();
        let rule = rules
            .active(ScanProfile::Comprehensive)
            .find(|rule| rule.declaration.name == "generic-api-key")
            .expect("generic-api-key is missing from the corpus");
        assert_eq!(rule.declaration.min_confidence, Some(5));
    }

    fn opens_and_closes_with_one_quote(hir: &regex_syntax::hir::Hir) -> bool {
        use regex_syntax::hir::HirKind;
        let edge = |part: Option<&regex_syntax::hir::Hir>, last: bool| match part?.kind() {
            HirKind::Literal(literal) if last => literal.0.last().copied(),
            HirKind::Literal(literal) => literal.0.first().copied(),
            _ => None,
        };
        let ends = match hir.kind() {
            HirKind::Literal(literal) if literal.0.len() >= 2 => {
                (literal.0.first().copied(), literal.0.last().copied())
            }
            HirKind::Concat(parts) => (edge(parts.first(), false), edge(parts.last(), true)),
            _ => (None, None),
        };
        matches!(ends, (Some(first), Some(last)) if first == last && matches!(first, b'"' | b'\'' | b'`'))
    }

    fn quote_enclosed_captures(hir: &regex_syntax::hir::Hir, found: &mut Vec<usize>) {
        use regex_syntax::hir::HirKind;
        match hir.kind() {
            HirKind::Capture(capture) => {
                if capture.name.is_none() && opens_and_closes_with_one_quote(&capture.sub) {
                    found.push(capture.index as usize);
                }
                quote_enclosed_captures(&capture.sub, found);
            }
            HirKind::Concat(parts) | HirKind::Alternation(parts) => {
                for part in parts {
                    quote_enclosed_captures(part, found);
                }
            }
            HirKind::Repetition(repetition) => quote_enclosed_captures(&repetition.sub, found),
            _ => {}
        }
    }

    #[test]
    fn quoted_capture_matches_pattern_syntax() {
        let rules = RuleSet::from_embedded().unwrap();
        let mut assigned = 0;
        for rule in rules.active(ScanProfile::Comprehensive) {
            let name = rule.declaration.name.as_str();
            let hir = regex_syntax::ParserBuilder::new()
                .unicode(rule.declaration.unicode)
                .utf8(false)
                .build()
                .parse(&rule.declaration.regex)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            let mut found = Vec::new();
            if rule.declaration.value_group.is_none() && rule.declaration.secret_group.is_none() {
                quote_enclosed_captures(&hir, &mut found);
            }
            assert!(
                found.len() <= 1,
                "{name}: quote-enclosed captures {found:?}"
            );
            assert_eq!(rule.quoted_capture, found.first().copied(), "{name}");
            assigned += usize::from(rule.quoted_capture.is_some());
        }
        assert_eq!(
            assigned, 1,
            "the corpus rule set carrying a `quoted_capture` changed; update this oracle"
        );
    }
}
