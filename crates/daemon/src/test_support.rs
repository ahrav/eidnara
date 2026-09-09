//! Deterministic fixtures for in-process module tests.
//!
//! Builders retain storage ownership and emit wire-compatible JSON without
//! starting provider or host processes.

use serde_json::{Value, json};

/// `StoreFixture` keeps backing directory alive for `store`.
pub struct StoreFixture {
    pub dir: tempfile::TempDir,
    pub store: memory_store::MemoryStore,
}

/// Builds a module-isolated SQLite descriptor rooted at `path`.
pub fn descriptor(path: &std::path::Path) -> storage::StorageDescriptor {
    storage::StorageDescriptor {
        module_id: "eidnara-test".to_string(),
        storage_namespace: "memory".to_string(),
        isolation: storage::Isolation::Module,
        backend: storage::StorageBackend::Sqlite {
            path: path.join("store.db").to_string_lossy().into_owned(),
        },
    }
}

use crate::decay_render::DecayRenderCompartment;
use crate::injection::build_synthetic_todo_pair;
use crate::wire::{BlockKind, HarnessMeta, IngressMessage, ProviderExtras, WireBlock, WireMessage};

/// Complete input set for one in-process transform fixture.
#[derive(Debug, Clone)]
pub struct InProcessFixture {
    pub session_id: String,
    pub messages: Vec<IngressMessage>,
    pub compartments: Vec<DecayRenderCompartment>,
    pub native_messages: Vec<Value>,
    pub reductions: Vec<Value>,
}

impl InProcessFixture {
    /// Renders a version-2 owned-runner transform request.
    pub fn handle_transform(&self) -> Value {
        json!({
            "kind": "transform",
            "v": 2,
            "serializer_profile": "owned-llmrunner",
            "session_id": self.session_id,
            "render_config": "fixture-config",
            "messages": self.messages,
            "reductions": self.reductions,
        })
    }

    pub fn call_transform(&self) -> Value {
        self.handle_transform()
    }
}

pub struct FixtureBuilder;

impl FixtureBuilder {
    /// `StoreFixture` retains `dir` so the isolated database remains available to `store`.
    pub fn store() -> StoreFixture {
        let dir = tempfile::tempdir().expect("fixture store directory");
        let store =
            memory_store::MemoryStore::open(&descriptor(dir.path())).expect("fixture store");
        StoreFixture { dir, store }
    }

    /// Builds a two-message session with one compartment boundary.
    pub fn session_with_boundary() -> InProcessFixture {
        let session_id = "fixture-boundary".to_string();
        let messages = vec![
            text_message("boundary-1", 1, "before boundary", false),
            text_message("boundary-2", 2, "after boundary", false),
        ];
        InProcessFixture {
            session_id,
            messages,
            compartments: vec![compartment(1, 1, "Boundary", "boundary summary")],
            native_messages: vec![],
            reductions: vec![],
        }
    }

    /// Builds a boundary session with summary and ordinal metadata.
    pub fn tagged_session() -> InProcessFixture {
        let mut fixture = Self::session_with_boundary();
        fixture.session_id = "fixture-tagged".to_string();
        fixture.messages[0].ck.meta.summary = true;
        fixture.messages[1].ck.meta.ordinal = Some(2);
        fixture
    }

    /// Builds a boundary session with one frozen text reduction.
    pub fn frozen_reductions() -> InProcessFixture {
        let mut fixture = Self::session_with_boundary();
        fixture.session_id = "fixture-frozen-reductions".to_string();
        fixture.reductions = vec![json!({
            "target_id": "boundary-1#0",
            "kind": "text",
            "payload": "[dropped 2]"
        })];
        fixture
    }

    /// Builds a session containing a synthetic active-task tool pair.
    ///
    /// Panics if fixed task JSON does not produce a valid pair.
    pub fn synthetic_todo_armed() -> InProcessFixture {
        let mut fixture = Self::session_with_boundary();
        fixture.session_id = "fixture-todo".to_string();
        let todo = build_synthetic_todo_pair(
            r#"[{"content":"Ship it","status":"in_progress","priority":"high"}]"#,
        )
        .expect("active todo fixture");
        fixture.messages = vec![
            IngressMessage {
                mid: "todo-assistant".into(),
                ordinal: 1,
                ck: todo.assistant_msg.clone(),
            },
            IngressMessage {
                mid: "todo-tool".into(),
                ordinal: 2,
                ck: todo.tool_msg.clone(),
            },
        ];
        fixture.native_messages = vec![
            serde_json::to_value(todo.assistant_msg).unwrap(),
            serde_json::to_value(todo.tool_msg).unwrap(),
        ];
        fixture
    }
}

fn text_message(mid: &str, ordinal: u64, text: &str, synthetic: bool) -> IngressMessage {
    IngressMessage {
        mid: mid.to_string(),
        ordinal,
        ck: WireMessage::from_parts(
            "user",
            vec![WireBlock::bare(BlockKind::Text { text: text.into() })],
            None,
            ProviderExtras::new(),
            HarnessMeta {
                harness_id: Some(mid.to_string()),
                synthetic,
                ..Default::default()
            },
        ),
    }
}

fn compartment(start: i64, end: i64, title: &str, content: &str) -> DecayRenderCompartment {
    DecayRenderCompartment {
        start_message: start,
        end_message: end,
        title: title.to_string(),
        content: content.to_string(),
        p1: Some(content.to_string()),
        importance: Some(50),
        ..Default::default()
    }
}

/// A name belongs to an absent subsystem when one of its segments is one of `words`. Segments
/// split on the punctuation wire names and JSON pointers use and are compared in lowercase, so
/// `git_commit_indexing` and `Mural.render` match while `github` and `digital_clock` do not.
pub fn names_absent_subsystem(name: &str, words: &[&str]) -> bool {
    name.split(['.', '_', '-', '/', ':'])
        .any(|segment| words.contains(&segment.to_ascii_lowercase().as_str()))
}
