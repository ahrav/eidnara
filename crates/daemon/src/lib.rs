//! Eidnara host component: the harness-agnostic cache-stability transform.
//!
//! This file declares the modules that have landed so far and the root items
//! they import; the full handler, dispatcher, and route family replace it once
//! every module is present. Private modules and items whose consumer has not
//! landed carry `#[allow(dead_code)]`; the replacement file carries none.
//! `pub` items inside `pub(crate)` modules are the surface the full handler
//! exposes, so item visibility is left as-is. commentlint: allow(JUDGE)

#![forbid(unsafe_code)]

pub mod caveman;
#[allow(dead_code)]
pub(crate) mod chunk_text;
#[allow(dead_code)]
pub(crate) mod classify;
#[allow(dead_code)]
pub(crate) mod compartment_coverage;
#[allow(dead_code)]
pub(crate) mod config;
#[allow(dead_code)]
pub(crate) mod divergence;
#[allow(dead_code)]
pub(crate) mod healing;
#[allow(dead_code)]
pub(crate) mod historian_producer;
#[allow(dead_code)]
pub(crate) mod project_docs;
#[allow(dead_code)]
pub(crate) mod prompt_surface;
#[allow(dead_code)]
mod retained_size;
#[allow(dead_code)]
pub(crate) mod session_resolver;
#[allow(dead_code)]
pub(crate) mod smart_note_evaluation;
#[allow(dead_code)]
mod token_cache;
pub mod wire;

use serde_json::{Value, json};

#[allow(dead_code)]
fn ctx_memory_description() -> String {
    "Read and maintain durable project-memory claims. Use public claim IDs and exact revision-bound mutation tokens; never use local row IDs. Create standalone facts, revise changed claims, archive or restore lifecycle state, and merge duplicate claims through the host commit path.".to_string()
}

#[allow(dead_code)]
fn ctx_search_description() -> String {
    "Keyword-search saved project memories, session notes, and summarized conversation history. This is literal word or phrase search, not semantic search; use it to find remembered facts or prior discussion snippets before answering.".to_string()
}

#[allow(dead_code)]
fn ctx_expand_description() -> String {
    "Recover compacted conversation ranges. The default view serves persisted historian chunk transcripts; verbose=true separately previews each cached raw message part, including tool-output sizes, so an ordinal can be recovered in full while that bounded snapshot is available.".to_string()
}

#[allow(dead_code)]
fn ctx_note_description() -> String {
    "Save or inspect durable session notes for future follow-ups. surface_condition is accepted and recorded, but condition evaluation arrives later on this leg.".to_string()
}

#[allow(dead_code)]
fn ctx_memory_schema() -> Value {
    let mutation_token = json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "tokenVersion", "publicClaimId", "revision", "contentDigest",
            "lifecycleSeq", "applicabilityHeadsDigest", "policyHeadsDigest"
        ],
        "properties": {
            "tokenVersion": { "type": "integer", "minimum": 1 },
            "publicClaimId": { "type": "string", "pattern": "^mcm_[0-9a-f]{32}$" },
            "revision": { "type": "integer", "minimum": 1 },
            "contentDigest": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "lifecycleSeq": { "type": "integer", "minimum": 1 },
            "applicabilityHeadsDigest": { "type": "string", "pattern": "^[0-9a-f]{64}$" },
            "policyHeadsDigest": { "type": "string", "pattern": "^[0-9a-f]{64}$" }
        }
    });
    let positive_categories = json!([
        "PROJECT_RULES",
        "ARCHITECTURE",
        "CONSTRAINTS",
        "CONFIG_VALUES",
        "NAMING"
    ]);
    let all_categories = {
        let mut categories = positive_categories
            .as_array()
            .expect("positive categories are a json array")
            .clone();
        categories.push(json!("REJECTED_APPROACH"));
        categories
    };
    let anti_memory = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["trigger", "rejectedStrategy", "rejectionReason"],
        "properties": {
            "trigger": { "type": "string", "minLength": 1 },
            "rejectedStrategy": { "type": "string", "minLength": 1 },
            "rejectionReason": { "type": "string", "minLength": 1 },
            "saferAlternative": { "type": ["string", "null"] },
            "preconditions": { "type": ["string", "null"] },
            "attemptedApproach": { "type": ["string", "null"] },
            "observedFailure": { "type": ["string", "null"] },
            "rootCause": { "type": ["string", "null"] },
            "recovery": { "type": ["string", "null"] },
            "nonApplicableWhen": { "type": ["string", "null"] }
        }
    });
    json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "action": {
                "type": "string",
                "enum": ["create", "get", "list", "revise", "archive", "restore", "merge"]
            },
            "category": {
                "type": "string",
                "enum": all_categories
            },
            "content": { "type": "string", "maxLength": 65536 },
            "antiMemory": anti_memory,
            "publicClaimId": { "type": "string", "pattern": "^mcm_[0-9a-f]{32}$" },
            "publicClaimIds": {
                "type": "array",
                "maxItems": 20,
                "items": { "type": "string", "pattern": "^mcm_[0-9a-f]{32}$" }
            },
            "mutationToken": mutation_token,
            "mutationTokens": {
                "type": "array",
                "maxItems": 100,
                "items": mutation_token
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
            "reason": { "type": "string", "maxLength": 4096 },
            "memory_project": { "type": "string" }
        },
        "oneOf": [
            {
                "required": ["action", "category", "content"],
                "properties": {
                    "action": { "const": "create" },
                    "category": { "enum": positive_categories }
                },
                "not": { "required": ["antiMemory"] }
            },
            {
                "required": ["action", "category", "antiMemory"],
                "properties": {
                    "action": { "const": "create" },
                    "category": { "const": "REJECTED_APPROACH" }
                },
                "not": { "required": ["content"] }
            },
            {
                "required": ["action"],
                "properties": {
                    "action": { "const": "revise" },
                    "category": { "enum": positive_categories }
                },
                "anyOf": [{ "required": ["content"] }, { "required": ["category"] }],
                "not": { "required": ["antiMemory"] }
            },
            {
                "required": ["action", "category", "antiMemory"],
                "properties": {
                    "action": { "const": "revise" },
                    "category": { "const": "REJECTED_APPROACH" }
                },
                "not": { "required": ["content"] }
            },
            {
                "required": ["action"],
                "properties": {
                    "action": { "enum": ["get", "list", "archive", "restore", "merge"] }
                }
            },
            {
                "required": ["reduced", "summary"],
                "properties": {
                    "reduced": { "const": true },
                    "summary": { "type": "string" }
                },
                "not": { "required": ["action"] }
            }
        ]
    })
}

#[allow(dead_code)]
fn ctx_search_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "query": {
                "type": "string",
                "maxLength": 1024,
                "description": "Literal keyword or phrase to find in memories and summarized history."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 25,
                "default": 8,
                "description": "Maximum number of matches to return."
            },
        }
    })
}

#[allow(dead_code)]
fn ctx_expand_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "start": { "type": "integer", "minimum": 0, "description": "First message ordinal to expand." },
            "end": { "type": "integer", "minimum": 0, "description": "Last message ordinal to expand, inclusive." },
            "verbose": { "type": "boolean", "description": "With start/end: list each message separately with its ordinal [N] and per-part preview, including each tool call's output size, so one message can be recovered by ordinal." },
            "message": { "type": "integer", "minimum": 0, "description": "Recover one message by ordinal in full from the cached raw request when available, otherwise its persisted historian chunk transcript." },
        }
    })
}

#[allow(dead_code)]
fn ctx_note_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "action": { "type": "string", "enum": ["write", "read", "update", "dismiss"], "description": "Operation to perform. Defaults to write when content is provided, otherwise read." },
            "content": { "type": "string", "maxLength": 65536, "description": "Note text for write/update, or optional dismissal resolution when action is dismiss." },
            "note_id": { "type": "integer", "minimum": 1, "description": "Note id for update or dismiss." },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 25, "description": "Maximum active notes to return." },
            "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Skip this many newest notes in each section." },
            "filter": { "type": "string", "enum": ["all", "active", "pending", "ready", "dismissed"], "description": "Optional read filter. Defaults to active session notes plus ready smart notes." },
            "surface_condition": { "type": "string", "maxLength": 4096, "description": "Optional externally checkable condition to record with the note. Evaluation arrives later." },
            "memory_project": { "type": "string", "description": "Resolved memory project identity supplied by the host transport." },
        }
    })
}

#[cfg(test)]
mod interim_schema_tests {
    use serde_json::Value;

    use super::*;

    fn collect_claim_id_patterns(value: &Value, patterns: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(pattern) = map.get("pattern").and_then(Value::as_str)
                    && pattern.contains("mcm_")
                {
                    patterns.push(pattern.to_owned());
                }
                for nested in map.values() {
                    collect_claim_id_patterns(nested, patterns);
                }
            }
            Value::Array(items) => {
                for nested in items {
                    collect_claim_id_patterns(nested, patterns);
                }
            }
            _ => {}
        }
    }

    /// The tool schemas restate the public-claim-ID grammar as a regex while
    /// `context_core::claim_operation::is_valid_public_claim_id` is the
    /// authoritative rule; the two must not drift apart, or the schema
    /// boundary would reject IDs the runtime validator accepts.
    /// commentlint: allow(JUDGE)
    #[test]
    fn schema_claim_id_patterns_agree_with_the_context_core_validator() {
        let mut patterns = Vec::new();
        for schema in [
            ctx_memory_schema(),
            ctx_search_schema(),
            ctx_expand_schema(),
            ctx_note_schema(),
        ] {
            collect_claim_id_patterns(&schema, &mut patterns);
        }
        assert!(
            !patterns.is_empty(),
            "the interim schemas are expected to constrain public claim IDs"
        );

        let vectors = [
            (format!("mcm_{}", "0".repeat(32)), true),
            (format!("mcm_{}", "a1b2c3d4".repeat(4)), true),
            (format!("mcm_{}", "f".repeat(32)), true),
            (format!("mcm_{}", "0".repeat(31)), false),
            (format!("mcm_{}", "0".repeat(33)), false),
            (format!("mcm_{}", "A".repeat(32)), false),
            (format!("mcm_{}", "g".repeat(32)), false),
            (format!("mcm{}", "0".repeat(33)), false),
            (format!("xcm_{}", "0".repeat(32)), false),
            ("0".repeat(36), false),
            (String::new(), false),
        ];
        for (candidate, accepted) in &vectors {
            assert_eq!(
                context_core::claim_operation::is_valid_public_claim_id(candidate),
                *accepted,
                "validator disagrees with the vector for {candidate:?}"
            );
        }
        for pattern in patterns {
            let compiled = regex::Regex::new(&pattern).expect("schema pattern compiles");
            for (candidate, accepted) in &vectors {
                assert_eq!(
                    compiled.is_match(candidate),
                    *accepted,
                    "pattern {pattern:?} and the validator disagree on {candidate:?}"
                );
            }
        }
    }
}
