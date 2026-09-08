//! Pins the durable-text cliff of a first HARD pass: the committed `meta` field grows with
//! message count and the memory store bounds one durable text field at 512 KiB.

#![cfg(feature = "bench-internals")]
#![forbid(unsafe_code)]

// The corpus module is shared with the benches, which use generators this test does not.
#[allow(dead_code)]
#[path = "../benches/support/corpus.rs"]
mod corpus;

use context_core::redaction::RedactionErrorKind;
use daemon::bench_internals::{self, CacheTtlProvenance, transform_cached};
use daemon::canonical_memory::{CanonicalMemoryRead, CanonicalMemorySnapshot};
use daemon::transform::{ProducerContext, TransformError, TransformRequest};
use memory_store::{MemoryStore, MemoryStoreError};

use corpus::{CORPUS_SEED, ContentClass};

#[test]
fn first_hard_pass_meta_respects_the_store_durable_text_bound() {
    for (count, expect_ok) in [(1_000usize, true), (1_400, false)] {
        let dir = tempfile::tempdir().expect("store dir");
        let store = MemoryStore::open(&daemon::store_descriptor_in(dir.path())).expect("store");
        let session = "meta-bound";
        let messages = corpus::messages(ContentClass::Mixed, count, 2_048, CORPUS_SEED);
        let req: TransformRequest = serde_json::from_value(serde_json::json!({
            "kind": "transform",
            "v": 2,
            "serializer_profile": "owned-llmrunner",
            "session_id": session,
            "render_config": "meta-bound-config",
            "messages": messages,
        }))
        .expect("transform request");
        let project_directory = dir.path().to_str().expect("utf8 dir");
        let ctx = ProducerContext {
            project_memory: CanonicalMemoryRead::Available(CanonicalMemorySnapshot {
                known_as_of: 0,
                truncated: false,
                rows: Vec::new(),
            }),
            project_path: "git:meta-bound",
            note_project_path: "git:meta-bound",
            project_directory,
            history_budget_tokens: 60_000.0,
            memory_budget_tokens: 8_000.0,
            user_profile_budget_tokens: 4_000.0,
            memory_enabled: true,
            inject_docs: true,
            temporal_awareness: true,
            now_ms: 1_700_000_000_000,
            execute_threshold_percentage: 65.0,
            compaction_enabled: true,
            smart_drops: false,
            cache_ttl: "5m".to_string(),
            cache_ttl_provenance: CacheTtlProvenance::Default,
            model_key: None,
            observed_last_response_at_ms: None,
            guidance_date: Some("Today's date: Thu Jan 01 2026".to_string()),
            historian_active: false,
            wrapup_active: false,
        };
        let before = store.load(session).expect("load before");
        assert_eq!(before.row_version, None, "{count}: fresh store has no row");

        let result = transform_cached(&store, &req, &ctx, &bench_internals::OutputCache::default())
            .map(|out| out.response.action);
        let after = store.load(session).expect("load after");
        if expect_ok {
            assert!(
                matches!(result.as_deref(), Ok("HARD")),
                "{count} messages must commit: {result:?}"
            );
            assert!(
                after.row_version.is_some(),
                "{count}: the HARD pass commits a row"
            );
        } else {
            assert!(
                matches!(
                    result,
                    Err(TransformError::Store(MemoryStoreError::Redaction(
                        RedactionErrorKind::InputLimit
                    )))
                ),
                "{count} messages must trip the durable-text bound: {result:?}"
            );
            assert_eq!(
                after.row_version, None,
                "{count}: a refused commit leaves no row"
            );
            assert_eq!(
                after.meta, before.meta,
                "{count}: a refused commit changes no meta"
            );
        }
    }
}
