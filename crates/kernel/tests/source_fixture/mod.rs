//! A real-store fixture shared by the source-hold and source-export tests: a
//! domain, a registered consumer, exact-retained artifacts, published source
//! descriptors, and an independent ledger of what the test wrote.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use kernel::source_identity::{Occurrence, Span};
use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactIngestRequest,
    CommitIntent, DomainSpec, HeldCursor, HeldDescriptor, KernelStore, ProviderEgress,
    RepositoryProvenance, Sensitivity, SourceDescriptorPolicy, SourceDescriptorRequest, SourceHold,
    SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds, SourceHoldError,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

pub const DOMAIN: &str = "domain";
pub const CONSUMER: &str = "search";
pub const POLICY: &str = "source-policy.v1";
pub const HOUR_MS: u64 = 60 * 60 * 1_000;
pub const DAY_MS: u64 = 24 * HOUR_MS;
pub const CLASSES: [&str; 5] = [
    "messages",
    "canonical_claims",
    "promoted_memory",
    "git_commits",
    "raw_tool_spans",
];

pub fn wall_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

pub fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-source-hold-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

/// Audit fields are required by a purge and refused by a logical delete.
pub fn audit(kind: ArtifactDeletionKind, value: &str) -> Option<String> {
    (kind == ArtifactDeletionKind::Purge).then(|| value.to_string())
}

pub fn admission(max_references: usize, max_encoded_bytes: u64) -> SourceHoldAdmission {
    SourceHoldAdmission {
        max_references: NonZeroUsize::new(max_references).unwrap(),
        max_encoded_bytes: NonZeroU64::new(max_encoded_bytes).unwrap(),
    }
}

pub fn wide_admission() -> SourceHoldAdmission {
    admission(1024, 1 << 20)
}

pub fn bounds(expiry_ms: u64) -> SourceHoldBounds {
    SourceHoldBounds {
        admission: wide_admission(),
        max_descriptor_rows: NonZeroUsize::new(1024).unwrap(),
        expiry_ms: NonZeroU64::new(expiry_ms).unwrap(),
    }
}

pub fn wide() -> SourceHoldBounds {
    bounds(HOUR_MS)
}

/// The five classes, each with an identity the descriptor encoder accepts.
pub fn identity(class: &str, key: &str) -> Vec<(String, String)> {
    let pairs: Vec<(&str, String)> = match class {
        "messages" => vec![
            ("project_id", "proj-a".into()),
            ("harness", "opencode".into()),
            ("session_id", "sess-01".into()),
            ("message_id", format!("msg-{key}")),
            ("block_index", "0".into()),
        ],
        "canonical_claims" => vec![("object_id", format!("obj-{key}"))],
        "promoted_memory" => vec![("decision_object_id", format!("obj-{key}"))],
        "git_commits" => vec![
            ("repository_id", "repo-1".into()),
            ("object_format", "sha1".into()),
            (
                "oid",
                format!(
                    "{:040x}",
                    key.bytes().fold(0u128, |acc, b| acc
                        .wrapping_mul(31)
                        .wrapping_add(u128::from(b)))
                ),
            ),
        ],
        "raw_tool_spans" => vec![
            ("project_id", "proj-a".into()),
            ("harness", "pi".into()),
            ("session_id", "sess-01".into()),
            ("parent_message_id", "msg-2".into()),
            ("tool_call_id", format!("call-{key}")),
            ("result_revision", "1".into()),
            ("block_index", "0".into()),
        ],
        other => panic!("unknown class {other}"),
    };
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

pub fn representation(class: &str) -> &'static str {
    match class {
        "messages" => "text",
        "canonical_claims" => "decision_summary",
        "promoted_memory" => "summary",
        "git_commits" => "commit_message",
        "raw_tool_spans" => "tool_output",
        other => panic!("unknown class {other}"),
    }
}

/// What the test wrote: one entry per published descriptor, keyed by the
/// descriptor object id, with the commits that created and invalidated it and
/// the commit that invalidated its evidence. Built from the writes, never
/// from a hold.
#[derive(Debug, Clone)]
pub struct LedgerEntry {
    pub class: String,
    pub key: String,
    pub object_id: String,
    pub revision: i64,
    pub evidence_id: String,
    pub digest: String,
    /// The whole retained buffer.
    pub text: String,
    /// The half-open byte span the descriptor selects, `None` for the whole buffer.
    pub span: Option<(u64, u64)>,
    pub created: i64,
    pub invalidated: Option<i64>,
    pub evidence_invalidated: Option<i64>,
}

impl LedgerEntry {
    /// The exact text the descriptor selects.
    pub fn selected_text(&self) -> &str {
        match self.span {
            None => &self.text,
            Some((start, end)) => &self.text[start as usize..end as usize],
        }
    }
}

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub store: Arc<KernelStore>,
    pub ledger: BTreeMap<String, LedgerEntry>,
}

impl Fixture {
    pub fn open() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(KernelStore::open(root.path()).unwrap());
        store
            .commit(intent("seed"), |envelope| {
                envelope.insert_domain(DomainSpec {
                    domain_id: DOMAIN.to_string(),
                    object_id: "domain-object".to_string(),
                    name: "fixture".to_string(),
                    source_kind: "fixture".to_string(),
                    source_id: DOMAIN.to_string(),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.register_outbox_consumer(CONSUMER, 1)?;
                Ok(String::new())
            })
            .unwrap();
        Self {
            root,
            store,
            ledger: BTreeMap::new(),
        }
    }

    pub fn reopen(self) -> Self {
        let Fixture {
            root,
            store,
            ledger,
        } = self;
        drop(store);
        let store = Arc::new(KernelStore::open(root.path()).unwrap());
        Self {
            root,
            store,
            ledger,
        }
    }

    pub fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.store.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    pub fn retain(&self, key: &str, text: &str) -> (String, String) {
        let handle = self
            .store
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: DOMAIN.to_string(),
                source_kind: "tool_output".to_string(),
                source_id: format!("native/{key}"),
                source_revision: 1,
                media_type: "text/plain".to_string(),
                retention_class: "canonical".to_string(),
                retain_until: None,
                asserted_sensitivity: Sensitivity::Normal,
                provider_egress: ProviderEgress::RemoteAllowed,
                provenance: Some(RepositoryProvenance {
                    repository_id: "repo".to_string(),
                    revision: "abc123".to_string(),
                }),
            })
            .unwrap();
        (handle.evidence_id, handle.digest)
    }

    /// Publishes revision `revision` of `class`/`key` over fresh evidence.
    pub fn publish(&mut self, class: &str, key: &str, revision: i64, text: &str) -> String {
        let evidence = self.retain(&format!("{class}-{key}-{revision}"), text);
        self.publish_over(class, key, revision, text, evidence)
    }

    /// Publishes a descriptor over already retained evidence and records it in
    /// the ledger, closing the previous revision's entry.
    pub fn publish_over(
        &mut self,
        class: &str,
        key: &str,
        revision: i64,
        text: &str,
        evidence: (String, String),
    ) -> String {
        self.publish_span(class, key, revision, text, None, evidence)
    }

    /// Publishes a descriptor selecting `span` from the retained buffer `text`.
    pub fn publish_span(
        &mut self,
        class: &str,
        key: &str,
        revision: i64,
        text: &str,
        span: Option<(u64, u64)>,
        (evidence_id, digest): (String, String),
    ) -> String {
        let identity = identity(class, key);
        let identity: Vec<(&str, &str)> = identity
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let revision_text = revision.to_string();
        let request = SourceDescriptorRequest {
            occurrence: Occurrence {
                class,
                identity: &identity,
                revision: &revision_text,
                representation: representation(class),
                span: span.map(|(start, end)| Span { start, end }),
            },
            source_policy: if class == "git_commits" {
                SourceDescriptorPolicy::Git {
                    version: POLICY.to_string(),
                }
            } else {
                SourceDescriptorPolicy::Native
            },
            domain_id: DOMAIN,
            scope_id: None,
            evidence_id: &evidence_id,
            artifact_digest: &digest,
            buffer: text,
            sensitivity: Sensitivity::Normal,
            observed_at: 1,
        };
        let mut outcome = None;
        let receipt = self
            .store
            .commit(
                intent(&format!("publish-{class}-{key}-{revision}")),
                |envelope| {
                    let published = envelope
                        .publish_source_descriptor(&request)
                        .unwrap_or_else(|error| panic!("{error}"));
                    outcome = Some(published);
                    Ok(String::new())
                },
            )
            .unwrap();
        let outcome = outcome.unwrap();
        // Supersession is the ledger's own fact: the one live entry for this
        // (class, key) closes, and the kernel must have reported exactly it.
        let superseded: Vec<String> = self
            .ledger
            .values()
            .filter(|entry| entry.class == class && entry.key == key && entry.invalidated.is_none())
            .map(|entry| entry.object_id.clone())
            .collect();
        assert!(
            superseded.len() <= 1,
            "{class}/{key} had two live revisions"
        );
        assert_eq!(outcome.replaced_object_id, superseded.first().cloned());
        for object_id in superseded {
            self.ledger.get_mut(&object_id).unwrap().invalidated = Some(receipt.commit_seq);
        }
        let evidence_invalidated = self
            .ledger
            .values()
            .find(|entry| entry.evidence_id == evidence_id)
            .and_then(|entry| entry.evidence_invalidated);
        self.ledger.insert(
            outcome.object_id.clone(),
            LedgerEntry {
                class: class.to_string(),
                key: key.to_string(),
                object_id: outcome.object_id.clone(),
                revision,
                evidence_id,
                digest,
                text: text.to_string(),
                span: span.filter(|&(start, end)| !(start == 0 && end as usize == text.len())),
                created: receipt.commit_seq,
                invalidated: None,
                evidence_invalidated,
            },
        );
        outcome.object_id
    }

    pub fn retire(&mut self, object_id: &str) -> i64 {
        let receipt = self
            .store
            .commit(intent(&format!("retire-{object_id}")), |envelope| {
                envelope.retire_observation(object_id)?;
                Ok(String::new())
            })
            .unwrap();
        self.ledger.get_mut(object_id).unwrap().invalidated = Some(receipt.commit_seq);
        receipt.commit_seq
    }

    /// Deletes or purges the artifact behind `evidence_id` and records the
    /// evidence invalidation in the ledger.
    pub fn delete_evidence(
        &mut self,
        evidence_id: &str,
        kind: ArtifactDeletionKind,
        at: i64,
    ) -> i64 {
        let digest = self.entry_by_evidence(evidence_id).digest.clone();
        let result = self
            .store
            .delete_artifact(ArtifactDeletionRequest {
                intent: intent(&format!("{kind:?}-{evidence_id}-{at}")),
                identity: ArtifactDeletionIdentity::Digest(digest),
                kind,
                operator_id: audit(kind, "operator-1"),
                target_locator: audit(kind, "incident://1"),
                reason: audit(kind, "secret"),
                deleted_at: at,
            })
            .unwrap();
        for entry in self.ledger.values_mut() {
            if entry.evidence_id == evidence_id {
                entry.evidence_invalidated = Some(result.commit_seq);
            }
        }
        result.commit_seq
    }

    pub fn entry_by_evidence(&self, evidence_id: &str) -> &LedgerEntry {
        self.ledger
            .values()
            .find(|entry| entry.evidence_id == evidence_id)
            .unwrap()
    }

    pub fn live_entry(&self, class: &str, key: &str) -> &LedgerEntry {
        let mut live = self.ledger.values().filter(|entry| {
            entry.class == class && entry.key == key && entry.invalidated.is_none()
        });
        let entry = live.next().unwrap();
        assert!(
            live.next().is_none(),
            "{class}/{key} has two live revisions"
        );
        entry
    }

    /// The ledger's prediction of what a hold captured at `snapshot` holds:
    /// every descriptor whose descriptor and evidence were both live at S, in
    /// `(class, object_id, revision)` order, and the distinct evidence cited.
    pub fn expected_at(&self, snapshot: i64) -> (Vec<HeldDescriptor>, usize, u64) {
        let live_at = |at: Option<i64>| at.is_none_or(|at| at > snapshot);
        let mut live: Vec<&LedgerEntry> = self
            .ledger
            .values()
            .filter(|entry| {
                entry.created <= snapshot
                    && live_at(entry.invalidated)
                    && live_at(entry.evidence_invalidated)
            })
            .collect();
        live.sort_by(|a, b| {
            (&a.class, &a.object_id, a.revision).cmp(&(&b.class, &b.object_id, b.revision))
        });
        let mut evidence: BTreeMap<&str, u64> = BTreeMap::new();
        for entry in &live {
            evidence.insert(&entry.evidence_id, entry.text.len() as u64);
        }
        let descriptors = live
            .iter()
            .map(|entry| HeldDescriptor {
                class: entry.class.clone(),
                object_id: entry.object_id.clone(),
                revision: entry.revision,
                evidence_id: entry.evidence_id.clone(),
                artifact_digest: entry.digest.clone(),
                byte_length: entry.text.len() as u64,
                invalidated_after_snapshot: entry.invalidated.filter(|at| *at > snapshot),
            })
            .collect();
        (descriptors, evidence.len(), evidence.values().sum())
    }

    /// The ledger's prediction of what a hold captured at `snapshot` and
    /// extended through `through` references: the evidence live at S plus the
    /// evidence of every descriptor created in `(S, through]` whose evidence
    /// was not invalidated at or before `through`.
    pub fn expected_refs_through(&self, snapshot: i64, through: i64) -> (Vec<String>, u64) {
        let (at_s, _, _) = self.expected_at(snapshot);
        let mut refs: BTreeMap<String, u64> = at_s
            .into_iter()
            .map(|d| (d.evidence_id, d.byte_length))
            .collect();
        for entry in self.ledger.values() {
            if entry.created > snapshot
                && entry.created <= through
                && entry.evidence_invalidated.is_none_or(|at| at > through)
            {
                refs.insert(entry.evidence_id.clone(), entry.text.len() as u64);
            }
        }
        let bytes = refs.values().sum();
        (refs.into_keys().collect(), bytes)
    }

    /// Marks every outbox row published and prunes what every consumer has
    /// acknowledged, the publication path a hold must not depend on.
    pub fn publish_and_prune(&self) {
        let pending = self.store.pending_outbox(usize::MAX).unwrap();
        let last = pending.last().expect("unpublished rows to publish");
        self.store
            .mark_outbox_published_through(last.outbox_position, 1)
            .unwrap();
        assert!(self.store.pending_outbox(usize::MAX).unwrap().is_empty());
        let pruned = self.store.prune_outbox().unwrap();
        assert!(pruned.deleted > 0, "pruning removed acknowledged rows");
    }

    /// Every held descriptor across every page of `page_size`.
    pub fn held_all(&self, hold: &SourceHold, page_size: usize) -> Vec<HeldDescriptor> {
        let mut cursor: Option<HeldCursor> = None;
        let mut all = Vec::new();
        loop {
            let page = self
                .store
                .held_descriptors(
                    &hold.binding,
                    &hold.hold_id,
                    hold.captured_at,
                    cursor.as_ref(),
                    NonZeroUsize::new(page_size).unwrap(),
                )
                .unwrap();
            assert!(page.descriptors.len() <= page_size);
            all.extend(page.descriptors);
            match page.next {
                Some(next) => cursor = Some(next),
                None => break all,
            }
        }
    }

    pub fn first_page(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
    ) -> Result<usize, SourceHoldError> {
        self.store
            .held_descriptors(binding, hold_id, now, None, NonZeroUsize::new(8).unwrap())
            .map(|page| page.descriptors.len())
    }

    pub fn inspect(&self) -> Connection {
        Connection::open_with_flags(
            self.root.path().join("kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    /// Rewrites one observation's stored payload in place, standing in for
    /// history the store can no longer honour.
    pub fn tamper_observation_payload(&self, object_id: &str, payload: &[u8]) {
        self.tamper(
            "UPDATE observations SET observation_payload=?1 WHERE object_id=?2",
            rusqlite::params![payload, object_id],
        );
    }

    pub fn tamper_hold_expiry(&self, hold_id: &str) {
        self.tamper(
            "UPDATE capture_pins SET expires_at=NULL WHERE capture_pin_id=?1",
            [hold_id],
        );
    }

    pub fn tamper(&self, sql: &str, params: impl rusqlite::Params) {
        let connection = Connection::open(self.root.path().join("kernel.sqlite")).unwrap();
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let changed = connection.execute(sql, params).unwrap();
        assert_eq!(changed, 1);
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.inspect().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    pub fn pin_refs(&self, hold_id: &str) -> Vec<String> {
        let connection = self.inspect();
        let mut statement = connection
            .prepare(
                "SELECT evidence_id FROM capture_pin_refs
                 WHERE capture_pin_id=?1 AND released_at IS NULL ORDER BY evidence_id",
            )
            .unwrap();
        statement
            .query_map([hold_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    pub fn pin_released(&self, hold_id: &str) -> bool {
        self.inspect()
            .query_row(
                "SELECT released_at IS NOT NULL FROM capture_pins WHERE capture_pin_id=?1",
                [hold_id],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn checkpoint(&self) -> i64 {
        self.inspect()
            .query_row(
                "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                [CONSUMER],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn domain_name(&self) -> String {
        self.inspect()
            .query_row(
                "SELECT name FROM domains WHERE object_id='domain-object'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn object_path(&self, digest: &str) -> std::path::PathBuf {
        self.root
            .path()
            .join("artifacts/objects")
            .join(&digest[..2])
            .join(&digest[2..])
    }

    pub fn object_present(&self, digest: &str) -> bool {
        self.object_path(digest).is_file()
    }

    pub fn seed_five_classes(&mut self) {
        for (index, class) in CLASSES.into_iter().enumerate() {
            self.publish(class, "a", 1, &format!("{class} text a"));
            self.publish(class, "b", 1, &format!("{class} text b {index}"));
        }
    }
}
