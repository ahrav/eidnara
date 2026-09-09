//! This module defines the zero-tool memory-classification producer contract.
//!
//! The host owns prompt rendering and XML parsing.
//! The fixed provider-facing role and generation budget prevent callers from using this surface for arbitrary prompts.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::time::Duration;

/// `dreamer.run_task` accepts only `CLASSIFY_TASK`.
pub const CLASSIFY_TASK: &str = "classify";
/// The host limits rendered prompts to `MAX_CLASSIFY_PROMPT_BYTES` before invoking the provider.
pub const MAX_CLASSIFY_PROMPT_BYTES: usize = 256 * 1024;
/// `MAX_CLASSIFY_MODEL_CHAIN` caps sequential provider attempts at 8 because each failed attempt advances to the next model.
pub const MAX_CLASSIFY_MODEL_CHAIN: usize = 8;
pub const CLASSIFY_TEMPERATURE: f64 = 0.1;
pub const CLASSIFY_MAX_OUTPUT_TOKENS: u32 = 32_000;
pub const CLASSIFY_AWAIT_TIMEOUT: Duration = Duration::from_secs(600);
/// The host clamps a request's `timeout_ms` to the await ceiling, so no caller can hold a producer past it.
/// With the two bounds equal, an await that times out has spent the whole request budget. commentlint: allow(JUDGE)
pub const CLASSIFY_MAX_REQUEST_TIMEOUT: Duration = CLASSIFY_AWAIT_TIMEOUT;

/// The time budget one request may spend across its whole chain.
pub fn classify_request_timeout(timeout_ms: u64) -> Duration {
    Duration::from_millis(timeout_ms).min(CLASSIFY_MAX_REQUEST_TIMEOUT)
}
/// Dispatched attempts one project may accumulate within `DREAMER_ATTEMPT_BUDGET_WINDOW` before requests are refused.
pub const DREAMER_ATTEMPT_BUDGET: u64 = 200;
pub const DREAMER_ATTEMPT_BUDGET_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// This is deliberately a zero-tool system role. The host supplies the pool and
/// retains the parser because accepting a caller-selected role would reopen the
/// producer trust boundary.
pub const CLASSIFY_SYSTEM_PROMPT: &str = r#"You are a memory classifier for the Eidnara system. You classify project memories by metadata only. You do NOT rewrite, merge, archive, verify, or create memories, and you do NOT read code — you judge each memory from its own text.

### How to score importance (1-100)
Importance decides which memories survive when the injected memory block is over budget: high scores stay in context, low scores drop first. So the score is only useful if it **discriminates** — if most memories land in the same band, you have not classified them, you have just labelled them.

Use judgment, not a formula. Blend:
- **Durability / decay-rate value:** Will this fact still matter weeks from now, across sessions?
- **Operational impact:** Would missing this fact cause wrong code, wasted time, broken workflows, or violated constraints?

Most memories are ordinary working facts — they belong in the middle, not the top. Reserve the high band for the genuinely load-bearing handful a teammate would be sunk without; push routine observations, one-off details, and now-obvious facts down. A "real, true fact" is not automatically important — truth is not importance.

Rough anchors (not quotas — spread naturally within them): transient/obvious observations 1-30, ordinary helpful project facts 40-65, load-bearing rules/architecture/constraints 70-100. A constraint that is a genuine must/never/always rule the project actively depends on floors around 60; but not every memory in a category is load-bearing — a niche, dated, or narrowly-scoped external quirk can sit lower even if it is a "constraint". Score the fact, not the label. If you assigned most of the pool to one band, re-read and differentiate.

### Scope
- `project` — only meaningful inside this repository/product (default when uncertain).
- `ecosystem` — useful to sibling projects in the same stack, harness, provider, or company ecosystem.
- `universe` — broadly true outside this codebase (protocol/platform/API facts), still written as a concise memory.

### Shareability
Shareability is about EXPOSURE, not scope: **would a teammate working on THIS SAME project benefit from seeing this memory, and is it free of anything personal, local, or sensitive?** If yes, set `shareable="true"`. This is the COMMON case — most project knowledge is exactly what you'd hand a new teammate: architecture, design rules, conventions, constraints, file locations, hard-won gotchas. Mark those shareable even though they are specific to this repo's internals.

Keep `shareable="false"` only for what is tied to the USER or their machine rather than the project: personal/absolute paths, usernames, local or private endpoints (e.g. localhost), credentials/secrets/tokens, customer data, machine-specific config, and personal working-style preferences. A fact's scope does NOT decide shareability. The host also fails closed and forces secret/credential/personal-path text to private regardless.

Output ONE XML manifest at the very end and NOTHING else — no narration, no per-memory commentary, no reasoning:
<classify>
<memory claim="mcm_..." importance="75" scope="project" shareable="true"/>
<memory claim="mcm_..." importance="20" scope="universe" shareable="false"/>
</classify>

Rules:
- Every memory in the pool below MUST appear exactly once.
- importance is an integer 1-100; scope is one of project|ecosystem|universe; shareable is true|false."#;

/// Scope values accepted by the TypeScript manifest parser.
const CLASSIFY_SCOPES: [&str; 3] = ["project", "ecosystem", "universe"];

fn memory_entry_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<memory\b([^>]*)/?>").expect("memory entry pattern"))
}

/// The pattern matches the claim syntax accepted by `parseClassifyManifest`.
/// The well-formedness check, not this regex, distinguishes a claim identity from arbitrary text.
/// A narrower pattern would classify a malformed identity as missing the claim attribute and suppress that diagnostic.
fn claim_attr_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\bclaim\s*=\s*"([^"]+)""#).expect("claim attribute pattern"))
}

fn importance_attr_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"\bimportance\s*=\s*"(\d+)""#).expect("importance attribute pattern")
    })
}

fn scope_attr_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\bscope\s*=\s*"([a-z]+)""#).expect("scope attribute pattern")
    })
}

fn shareable_attr_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\bshareable\s*=\s*"(true|false|1|0)""#)
            .expect("shareable attribute pattern")
    })
}

fn classify_root_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)<classify\b[^>]*>(.*?)</classify>").expect("classify root pattern")
    })
}

/// Returns the first complete classify root body.
///
/// Matching is case-insensitive and permits root attributes and surrounding text.
fn classify_body(text: &str) -> Option<&str> {
    classify_root_pattern()
        .captures(text)
        .and_then(|caps| caps.get(1))
        .map(|body| body.as_str())
}

/// Validates manifest shape and exact claim coverage, and rejects an unknown `scope`, but does not range-check `importance`, reject an unrecognized shareability value, or reject unknown attributes.
///
/// Identity is the opaque public claim ID in each `claim` attribute. Claim IDs
/// are validated before other entry fields, so diagnostics never echo arbitrary
/// model-controlled attribute text.
///
/// Returns an error for a missing envelope, malformed entry, duplicate or
/// malformed claim ID, unknown scope, empty classification, or coverage mismatch.
pub fn validate_classify_manifest(text: &str, expected: &BTreeSet<String>) -> Result<(), String> {
    let body = classify_body(text).ok_or("no complete classify envelope")?;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut entries = 0usize;
    for captures in memory_entry_pattern().captures_iter(body) {
        entries += 1;
        let attrs = captures.get(1).map_or("", |group| group.as_str());
        let claim = claim_attr_pattern()
            .captures(attrs)
            .map(|caps| caps[1].to_owned())
            .ok_or("manifest entry is missing a claim id")?;
        if !context_core::claim_operation::is_valid_public_claim_id(&claim) {
            return Err("manifest entry carries a malformed claim id".to_owned());
        }
        let importance = importance_attr_pattern()
            .captures(attrs)
            .and_then(|caps| caps[1].parse::<u32>().ok());
        let scope = scope_attr_pattern()
            .captures(attrs)
            .map(|caps| caps[1].to_ascii_lowercase());
        let shareable = shareable_attr_pattern().is_match(attrs);
        if let Some(scope) = &scope
            && !CLASSIFY_SCOPES.contains(&scope.as_str())
        {
            return Err(format!("manifest entry {claim} carries an unknown scope"));
        }
        if importance.is_none() && scope.is_none() && !shareable {
            return Err(format!(
                "manifest entry {claim} carries no classification fields"
            ));
        }
        if !seen.insert(claim.clone()) {
            return Err(format!("manifest repeats entry {claim}"));
        }
    }
    // Content that yields no parsed entries is an unrecognized shape, not an empty manifest.
    // classification.
    if entries == 0 && !body.trim().is_empty() {
        return Err("manifest body has no recognizable entries".to_owned());
    }
    if &seen != expected {
        return Err(format!(
            "manifest covers {} of the {} requested claims",
            seen.intersection(expected).count(),
            expected.len()
        ));
    }
    Ok(())
}

/// Reports whether trimmed text contains exact lowercase opening and closing tags.
pub fn has_manifest_envelope(text: &str) -> bool {
    let text = text.trim();
    text.contains("<classify>") && text.contains("</classify>")
}

/// The child ID is opaque so provider and session diagnostics do not expose the command ID or project path.
/// The registry determines transform exemptions; the child-ID prefix does not.
pub fn child_session_id(project: &str, command_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project.as_bytes());
    hasher.update([0]);
    hasher.update(command_id.as_bytes());
    let digest = hasher.finalize();
    format!("eidnara-dreamer:classify:{}", hex_prefix(&digest, 16))
}

/// Derives an opaque child ID from full attempt identity.
///
/// Durable ledger commands are scoped to `(ledger_session, command_id)`.
/// Including attempt index and model separates fallback attempts, while
/// `ledger_session` prevents module sessions that reuse `command_id` from
/// attaching to or purging each other's runs. The receipt generation keeps a
/// successor's sessions apart from a predecessor's, so a taken-over command
/// can never attach to or purge a run the predecessor may still hold.
pub fn attempt_child_session_id(
    project: &str,
    ledger_session: &str,
    command_id: &str,
    generation: u64,
    attempt: usize,
    model: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project.as_bytes());
    hasher.update([0]);
    hasher.update(ledger_session.as_bytes());
    hasher.update([0]);
    hasher.update(command_id.as_bytes());
    hasher.update([0]);
    hasher.update(generation.to_le_bytes());
    hasher.update([0]);
    hasher.update((attempt as u64).to_le_bytes());
    hasher.update([0]);
    hasher.update(model.as_bytes());
    let digest = hasher.finalize();
    format!("eidnara-dreamer:classify:{}", hex_prefix(&digest, 16))
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(count);
    for byte in bytes.iter().take(count.div_ceil(2)) {
        out.push(HEX[(byte >> 4) as usize] as char);
        if out.len() < count {
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out.truncate(count);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The function returns a well-formed public claim ID derived from `seed`.
    fn claim(seed: u8) -> String {
        format!("mcm_{}", format!("{seed:02x}").repeat(16))
    }

    #[test]
    fn manifest_envelope_rejects_provider_outage_text() {
        assert!(!has_manifest_envelope("All Antigravity endpoints failed"));
        assert!(has_manifest_envelope(
            "<classify><memory claim=\"mcm_test\"/></classify>"
        ));
    }

    #[test]
    fn child_ids_are_stable_but_lineage_scoped() {
        assert_eq!(
            child_session_id("project", "command"),
            child_session_id("project", "command")
        );
        assert_ne!(
            child_session_id("project", "command"),
            child_session_id("other", "command")
        );
        assert!(child_session_id("project", "command").starts_with("eidnara-dreamer:classify:"));
    }

    #[test]
    fn manifest_root_matching_mirrors_the_caller_parser() {
        let one = claim(1);
        let expected: BTreeSet<String> = [one.clone()].into_iter().collect();
        let entry = format!("<memory claim=\"{one}\" scope=\"project\"/>");
        for text in [
            format!("<classify>{entry}</classify>"),
            format!("<Classify>{entry}</Classify>"),
            format!("<classify version=\"1\">{entry}</classify>"),
            // Surrounding prose is ignored: the root is located, not anchored.
            format!("here you go:\n<classify>{entry}</classify>\ndone"),
        ] {
            assert_eq!(
                validate_classify_manifest(&text, &expected),
                Ok(()),
                "must accept {text:?}"
            );
        }
        assert_eq!(
            validate_classify_manifest("All Antigravity endpoints failed", &expected),
            Err("no complete classify envelope".to_owned())
        );
    }

    #[test]
    fn manifest_validation_accepts_exact_coverage_and_rejects_every_invalid_shape() {
        let (one, two, three) = (claim(1), claim(2), claim(3));
        let expected: BTreeSet<String> = [one.clone(), two.clone()].into_iter().collect();
        let ok = format!(
            "<classify><memory claim=\"{one}\" importance=\"80\" scope=\"project\"/>\
             <memory claim=\"{two}\" shareable=\"false\"/></classify>"
        );
        assert_eq!(validate_classify_manifest(&ok, &expected), Ok(()));

        // An empty request is satisfied only by an empty manifest.
        assert_eq!(
            validate_classify_manifest("<classify></classify>", &BTreeSet::new()),
            Ok(())
        );

        for (label, text, expected_error) in [
            (
                "no envelope",
                "All Antigravity endpoints failed".to_owned(),
                "no complete classify envelope".to_owned(),
            ),
            (
                "unterminated envelope",
                format!("<classify><memory claim=\"{one}\" scope=\"project\"/>"),
                "no complete classify envelope".to_owned(),
            ),
            (
                "missing memory",
                format!("<classify><memory claim=\"{one}\" scope=\"project\"/></classify>"),
                "manifest covers 1 of the 2 requested claims".to_owned(),
            ),
            (
                "extra memory",
                format!(
                    "<classify><memory claim=\"{one}\" scope=\"project\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/>\
                     <memory claim=\"{three}\" scope=\"project\"/></classify>"
                ),
                "manifest covers 2 of the 2 requested claims".to_owned(),
            ),
            (
                "duplicate claim",
                format!(
                    "<classify><memory claim=\"{one}\" scope=\"project\"/>\
                     <memory claim=\"{one}\" scope=\"project\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>"
                ),
                format!("manifest repeats entry {one}"),
            ),
            (
                "missing claim attribute",
                format!(
                    "<classify><memory scope=\"project\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>"
                ),
                "manifest entry is missing a claim id".to_owned(),
            ),
            (
                "numeric id instead of a claim",
                format!(
                    "<classify><memory id=\"1\" scope=\"project\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>"
                ),
                "manifest entry is missing a claim id".to_owned(),
            ),
            (
                "malformed claim id",
                format!(
                    "<classify><memory claim=\"mcm_short\" scope=\"project\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>"
                ),
                "manifest entry carries a malformed claim id".to_owned(),
            ),
            (
                "unprefixed claim id",
                format!(
                    "<classify><memory claim=\"{}\" scope=\"project\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>",
                    &one[4..]
                ),
                "manifest entry carries a malformed claim id".to_owned(),
            ),
            (
                "unknown scope",
                format!(
                    "<classify><memory claim=\"{one}\" scope=\"galaxy\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>"
                ),
                format!("manifest entry {one} carries an unknown scope"),
            ),
            (
                "no classification fields",
                format!(
                    "<classify><memory claim=\"{one}\"/>\
                     <memory claim=\"{two}\" scope=\"project\"/></classify>"
                ),
                format!("manifest entry {one} carries no classification fields"),
            ),
            (
                "unrecognized body",
                "<classify>I classified them all, trust me.</classify>".to_owned(),
                "manifest body has no recognizable entries".to_owned(),
            ),
        ] {
            assert_eq!(
                validate_classify_manifest(&text, &expected),
                Err(expected_error),
                "{label} must not be accepted as a successful attempt"
            );
        }
    }

    #[test]
    fn manifest_validation_diagnostics_never_quote_the_manifest() {
        let expected: BTreeSet<String> = [claim(7)].into_iter().collect();
        let secret = "POOL-SECRET-SENTINEL";
        for text in [
            format!("<classify>{secret}</classify>"),
            format!("<classify><memory claim=\"{secret}\" importance=\"80\"/></classify>"),
            format!(
                "<classify><memory claim=\"{secret}\" scope=\"galaxy\"/>\
                 <memory claim=\"{secret}\"/></classify>"
            ),
        ] {
            let detail = validate_classify_manifest(&text, &expected).expect_err("rejected");
            assert!(!detail.contains(secret), "manifest text leaked: {detail}");
        }
    }

    #[test]
    fn a_request_timeout_is_clamped_to_the_host_ceiling() {
        assert_eq!(classify_request_timeout(1), Duration::from_millis(1));
        assert_eq!(
            classify_request_timeout(CLASSIFY_MAX_REQUEST_TIMEOUT.as_millis() as u64),
            CLASSIFY_MAX_REQUEST_TIMEOUT
        );
        assert_eq!(
            classify_request_timeout(CLASSIFY_MAX_REQUEST_TIMEOUT.as_millis() as u64 + 1),
            CLASSIFY_MAX_REQUEST_TIMEOUT
        );
        assert_eq!(
            classify_request_timeout(u64::MAX),
            CLASSIFY_MAX_REQUEST_TIMEOUT
        );
    }

    #[test]
    fn child_ids_are_stable_per_attempt_and_distinct_across_attempt_identity() {
        assert_eq!(
            attempt_child_session_id("project", "ses", "command", 1, 0, "prov/model-a"),
            attempt_child_session_id("project", "ses", "command", 1, 0, "prov/model-a"),
            "a retry of the same attempt must reuse its session"
        );
        let base = attempt_child_session_id("project", "ses", "command", 1, 0, "prov/model-a");
        assert_ne!(
            base,
            attempt_child_session_id("project", "ses", "command", 1, 1, "prov/model-b"),
            "fallback attempts must use distinct sessions"
        );
        assert_ne!(
            base,
            attempt_child_session_id("project", "ses", "command", 1, 1, "prov/model-a"),
            "the attempt slot alone must separate sessions"
        );
        assert_ne!(
            base,
            attempt_child_session_id("project", "ses", "command", 1, 0, "prov/model-b"),
            "the model alone must separate sessions"
        );
        assert_ne!(
            base,
            attempt_child_session_id("project", "ses", "command", 2, 0, "prov/model-a"),
            "the receipt generation alone must separate sessions: a successor never \
             shares a child session with the predecessor it fenced"
        );
        assert_ne!(
            base,
            attempt_child_session_id("other", "ses", "command", 1, 0, "prov/model-a")
        );
        assert_ne!(
            base,
            attempt_child_session_id("project", "other", "command", 1, 0, "prov/model-a"),
            "the ledger session alone must separate sessions: commands are \
             scoped to (ledger_session, command_id)"
        );
        assert_ne!(
            base,
            attempt_child_session_id("project", "ses", "other", 1, 0, "prov/model-a")
        );
        assert!(base.starts_with("eidnara-dreamer:classify:"));
    }
}
