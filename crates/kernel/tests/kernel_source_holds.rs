//! Holds are compared with an independent ledger of which descriptors were live
//! at S, and every way a hold can stop protecting its bytes is exercised.

#![cfg(feature = "test-support")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::num::{NonZeroU64, NonZeroUsize};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use kernel::source_identity::Occurrence;
use kernel::{
    ArtifactDeletionIdentity, ArtifactDeletionKind, ArtifactDeletionRequest, ArtifactIngestRequest,
    CommitIntent, DomainSpec, HeldCursor, HeldDescriptor, KernelError, KernelStore,
    MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER, MAX_SOURCE_HOLD_LIFETIME_MS, ProviderEgress,
    RemediationTarget, RepositoryProvenance, Sensitivity, SourceDescriptorPolicy,
    SourceDescriptorRequest, SourceHold, SourceHoldBinding, SourceHoldBounds, SourceHoldError,
    SourceHoldInvalidity,
};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

const DOMAIN: &str = "domain";
const CONSUMER: &str = "search";
const POLICY: &str = "source-policy.v1";
const HOUR_MS: u64 = 60 * 60 * 1_000;
const DAY_MS: u64 = 24 * HOUR_MS;
const CLASSES: [&str; 5] = [
    "messages",
    "canonical_claims",
    "promoted_memory",
    "git_commits",
    "raw_tool_spans",
];

fn wall_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "kernel-source-hold-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

/// Audit fields are required by a purge and refused by a logical delete.
fn audit(kind: ArtifactDeletionKind, value: &str) -> Option<String> {
    (kind == ArtifactDeletionKind::Purge).then(|| value.to_string())
}

fn bounds(expiry_ms: u64) -> SourceHoldBounds {
    SourceHoldBounds {
        max_references: NonZeroUsize::new(1024).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        expiry_ms: NonZeroU64::new(expiry_ms).unwrap(),
    }
}

fn wide() -> SourceHoldBounds {
    bounds(HOUR_MS)
}

/// The five classes, each with an identity the descriptor encoder accepts.
fn identity(class: &str, key: &str) -> Vec<(String, String)> {
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

fn representation(class: &str) -> &'static str {
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
struct LedgerEntry {
    class: String,
    key: String,
    object_id: String,
    revision: i64,
    evidence_id: String,
    digest: String,
    text: String,
    created: i64,
    invalidated: Option<i64>,
    evidence_invalidated: Option<i64>,
}

struct Fixture {
    root: tempfile::TempDir,
    store: Arc<KernelStore>,
    ledger: BTreeMap<String, LedgerEntry>,
}

impl Fixture {
    fn open() -> Self {
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

    fn reopen(self) -> Self {
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

    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.store.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    fn retain(&self, key: &str, text: &str) -> (String, String) {
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
    fn publish(&mut self, class: &str, key: &str, revision: i64, text: &str) -> String {
        let evidence = self.retain(&format!("{class}-{key}-{revision}"), text);
        self.publish_over(class, key, revision, text, evidence)
    }

    /// Publishes a descriptor over already retained evidence and records it in
    /// the ledger, closing the previous revision's entry.
    fn publish_over(
        &mut self,
        class: &str,
        key: &str,
        revision: i64,
        text: &str,
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
                span: None,
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
        if let Some(replaced) = &outcome.replaced_object_id {
            self.ledger.get_mut(replaced).unwrap().invalidated = Some(receipt.commit_seq);
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
                created: receipt.commit_seq,
                invalidated: None,
                evidence_invalidated,
            },
        );
        outcome.object_id
    }

    fn retire(&mut self, object_id: &str) -> i64 {
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
    fn delete_evidence(&mut self, evidence_id: &str, kind: ArtifactDeletionKind, at: i64) -> i64 {
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

    fn entry_by_evidence(&self, evidence_id: &str) -> &LedgerEntry {
        self.ledger
            .values()
            .find(|entry| entry.evidence_id == evidence_id)
            .unwrap()
    }

    fn live_entry(&self, class: &str, key: &str) -> &LedgerEntry {
        self.ledger
            .values()
            .find(|entry| entry.class == class && entry.key == key && entry.invalidated.is_none())
            .unwrap()
    }

    /// The ledger's prediction of what a hold captured at `snapshot` holds:
    /// every descriptor whose descriptor and evidence were both live at S, in
    /// `(class, object_id, revision)` order, and the distinct evidence cited.
    fn expected_at(&self, snapshot: i64) -> (Vec<HeldDescriptor>, usize, u64) {
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

    /// Every held descriptor across every page of `page_size`.
    fn held_all(&self, hold: &SourceHold, page_size: usize) -> Vec<HeldDescriptor> {
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

    fn first_page(
        &self,
        binding: &SourceHoldBinding,
        hold_id: &str,
        now: i64,
    ) -> Result<usize, SourceHoldError> {
        self.store
            .held_descriptors(binding, hold_id, now, None, NonZeroUsize::new(8).unwrap())
            .map(|page| page.descriptors.len())
    }

    fn inspect(&self) -> Connection {
        Connection::open_with_flags(
            self.root.path().join("kernel.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.inspect().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn pin_refs(&self, hold_id: &str) -> Vec<String> {
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

    fn checkpoint(&self) -> i64 {
        self.inspect()
            .query_row(
                "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id=?1",
                [CONSUMER],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn domain_name(&self) -> String {
        self.inspect()
            .query_row(
                "SELECT name FROM domains WHERE object_id='domain-object'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn object_present(&self, digest: &str) -> bool {
        self.root
            .path()
            .join("artifacts/objects")
            .join(&digest[..2])
            .join(&digest[2..])
            .is_file()
    }

    fn seed_five_classes(&mut self) {
        for (index, class) in CLASSES.into_iter().enumerate() {
            self.publish(class, "a", 1, &format!("{class} text a"));
            self.publish(class, "b", 1, &format!("{class} text b {index}"));
        }
    }
}

fn assert_hold_matches_ledger(fixture: &Fixture, hold: &SourceHold, page_size: usize) {
    let (expected, references, bytes) = fixture.expected_at(hold.snapshot);
    assert_eq!(hold.references, references, "reference count at S");
    assert_eq!(hold.encoded_bytes, bytes, "encoded bytes at S");
    assert_eq!(
        fixture.held_all(hold, page_size),
        expected,
        "held inventory at S"
    );
    let mut expected_refs: Vec<String> = expected.iter().map(|d| d.evidence_id.clone()).collect();
    expected_refs.sort();
    expected_refs.dedup();
    assert_eq!(
        fixture.pin_refs(&hold.hold_id),
        expected_refs,
        "pinned evidence"
    );
}

#[test]
fn a_hold_equals_the_five_class_ledger_at_s_and_needs_a_registered_consumer() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    // Revise one, retire one, and cite one artifact from two classes so the
    // ledger has closed entries and a shared reference before S.
    let msg_a = fixture.publish("messages", "a", 2, "messages text a v2");
    let claim_b = fixture
        .live_entry("canonical_claims", "b")
        .object_id
        .clone();
    fixture.retire(&claim_b);
    let shared = fixture.retain("shared", "one artifact, two descriptors");
    fixture.publish_over(
        "promoted_memory",
        "shared",
        1,
        "one artifact, two descriptors",
        shared.clone(),
    );
    fixture.publish_over(
        "git_commits",
        "shared",
        1,
        "one artifact, two descriptors",
        shared,
    );
    let tip = fixture.store.tip().unwrap();

    let before = wall_ms();
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), bounds(HOUR_MS))
        .unwrap();
    assert_eq!(hold.snapshot, tip, "S is the tip under the writer");
    assert_eq!(hold.binding, fixture.binding());
    assert!(hold.captured_at >= before && hold.captured_at <= wall_ms());
    assert_eq!(
        hold.expires_at - hold.captured_at,
        i64::try_from(HOUR_MS).unwrap()
    );
    for page_size in [1, 3, 64] {
        assert_hold_matches_ledger(&fixture, &hold, page_size);
    }
    let held = fixture.held_all(&hold, 64);
    assert_eq!(
        hold.references + 1,
        held.len(),
        "the shared artifact is one reference"
    );
    assert!(held.iter().any(|d| d.object_id == msg_a));
    assert!(
        !held.iter().any(|d| d.object_id == claim_b),
        "retired before S"
    );
    assert_eq!(
        held.iter()
            .map(|d| d.class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        CLASSES.len()
    );
    assert_eq!(
        fixture
            .store
            .source_hold_status(&fixture.binding(), &hold.hold_id, hold.captured_at)
            .unwrap(),
        hold
    );
    let two_days = fixture
        .store
        .capture_source_hold(&fixture.binding(), bounds(2 * DAY_MS))
        .unwrap();
    assert_eq!(
        two_days.expires_at - two_days.captured_at,
        i64::try_from(2 * DAY_MS).unwrap()
    );

    // Repeated captures are bounded per owner; releasing one frees a slot.
    let mut extra = Vec::new();
    while extra.len() + 2 < MAX_ACTIVE_SOURCE_HOLDS_PER_CONSUMER {
        extra.push(
            fixture
                .store
                .capture_source_hold(&fixture.binding(), wide())
                .unwrap(),
        );
    }
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .unwrap_err(),
        SourceHoldError::HoldLimitReached
    );
    // The cap is per consumer: a fresh policy version is not a fresh budget.
    let mut other_policy_at_cap = fixture.binding();
    other_policy_at_cap.source_policy_version = "source-policy.v2".to_string();
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&other_policy_at_cap, wide())
            .unwrap_err(),
        SourceHoldError::HoldLimitReached
    );
    fixture
        .store
        .release_source_hold(&fixture.binding(), &two_days.hold_id, 1)
        .unwrap();
    let refilled = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    for stale in extra.iter().chain([&refilled]) {
        fixture
            .store
            .release_source_hold(&fixture.binding(), &stale.hold_id, 1)
            .unwrap();
    }

    // Registration precedes S; wrong consumer, incarnation, or policy fails.
    let mut wrong_consumer = fixture.binding();
    wrong_consumer.consumer_id = "nobody".to_string();
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&wrong_consumer, wide())
            .unwrap_err(),
        SourceHoldError::UnknownConsumer
    );
    let mut wrong_epoch = fixture.binding();
    wrong_epoch.lease_epoch += 1;
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&wrong_epoch, wide())
            .unwrap_err(),
        SourceHoldError::IncarnationMismatch
    );
    for policy in [
        "",
        "has space",
        &"p".repeat(129),
        "semi;colon",
        "unit\u{1f}sep",
    ] {
        let mut wrong_policy = fixture.binding();
        wrong_policy.source_policy_version = policy.to_string();
        assert_eq!(
            fixture
                .store
                .capture_source_hold(&wrong_policy, wide())
                .unwrap_err(),
            SourceHoldError::InvalidRequest,
            "{policy:?}"
        );
    }
    // A consumer id carrying the owner separator can never alias another
    // consumer's holds: it is refused before registration is consulted.
    fixture
        .store
        .commit(intent("separator-consumer"), |envelope| {
            envelope.register_outbox_consumer("search\u{1f}x", 1)?;
            Ok(String::new())
        })
        .unwrap();
    let mut separator = fixture.binding();
    separator.consumer_id = "search\u{1f}x".to_string();
    assert_eq!(
        fixture
            .store
            .capture_source_hold(&separator, wide())
            .unwrap_err(),
        SourceHoldError::InvalidRequest
    );
    // Using the hold under another binding fails before anything is read.
    let mut other_policy = fixture.binding();
    other_policy.source_policy_version = "source-policy.v2".to_string();
    assert_eq!(
        fixture
            .store
            .source_hold_status(&other_policy, &hold.hold_id, hold.captured_at)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    assert_eq!(
        fixture
            .first_page(&wrong_consumer, &hold.hold_id, hold.captured_at)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    assert_eq!(
        fixture.count(
            "SELECT COUNT(*) FROM capture_pins WHERE pin_kind='source_hold' AND released_at IS NULL"
        ),
        1,
        "only the first hold is still active"
    );
    assert_eq!(fixture.checkpoint(), 0, "capturing acknowledges nothing");
}

#[test]
fn writer_serialized_capture_leaves_no_gap_between_s_and_protection_under_concurrent_gc() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    // Two artifacts are logically deleted before the loop: reclaimable once
    // the grace period passes, unless a hold captured before the deletion
    // pins them. The sweep clock sits past the grace period the whole time.
    let long = bounds(MAX_SOURCE_HOLD_LIFETIME_MS);
    let pinned_before_delete = fixture
        .store
        .capture_source_hold(&fixture.binding(), long)
        .unwrap();
    let doomed: Vec<String> = ["messages", "raw_tool_spans"]
        .into_iter()
        .map(|class| fixture.live_entry(class, "b").evidence_id.clone())
        .collect();
    let deleted_at = wall_ms();
    for evidence_id in &doomed {
        fixture.delete_evidence(evidence_id, ArtifactDeletionKind::Delete, deleted_at);
    }
    let sweep_now = wall_ms() + i64::try_from(15 * DAY_MS).unwrap();
    let first_publish = fixture.store.tip().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let store = Arc::clone(&fixture.store);
    let binding = fixture.binding();
    let (captured, sweeps): (Vec<SourceHold>, usize) = std::thread::scope(|scope| {
        let stop_reader = Arc::clone(&stop);
        let reader_store = Arc::clone(&store);
        let reader = scope.spawn(move || {
            let mut holds = Vec::new();
            while !stop_reader.load(Ordering::Relaxed) && holds.len() < 16 {
                holds.push(reader_store.capture_source_hold(&binding, long).unwrap());
            }
            holds
        });
        let stop_gc = Arc::clone(&stop);
        let gc_store = Arc::clone(&store);
        let gc = scope.spawn(move || {
            let mut sweeps = 0;
            while !stop_gc.load(Ordering::Relaxed) {
                gc_store.run_staging_maintenance(sweep_now).unwrap();
                sweeps += 1;
            }
            sweeps
        });
        // The writer also captures between a revision and its successor, so
        // at least one S is known to sit inside the run whatever the
        // background threads' timing.
        let mut inline = Vec::new();
        for round in 0..12 {
            let key = format!("flip{round}");
            fixture.publish("messages", &key, 1, &format!("flip {round}"));
            if round % 4 == 1 {
                let binding = fixture.binding();
                inline.push(fixture.store.capture_source_hold(&binding, long).unwrap());
            }
            fixture.publish("messages", &key, 2, &format!("flip {round} v2"));
            if round % 3 == 0 {
                let current = fixture.live_entry("messages", &key).object_id.clone();
                fixture.retire(&current);
            }
        }
        stop.store(true, Ordering::Relaxed);
        let mut captured = reader.join().unwrap();
        captured.extend(inline);
        captured.sort_by_key(|hold| hold.snapshot);
        (captured, gc.join().unwrap())
    });
    let last_publish = fixture.store.tip().unwrap();
    assert!(sweeps > 0);
    assert!(captured.len() > 3);
    assert!(
        captured
            .iter()
            .any(|hold| hold.snapshot > first_publish && hold.snapshot < last_publish),
        "some S landed inside the writer's run"
    );
    // The revision leg: some hold froze a revision 1 that a later commit superseded.
    let superseded_rev1 = captured.iter().any(|hold| {
        fixture.held_all(hold, 64).iter().any(|d| {
            d.class == "messages"
                && d.revision == 1
                && d.evidence_id.contains("-flip")
                && d.invalidated_after_snapshot.is_some()
        })
    });
    assert!(superseded_rev1, "no hold witnessed a revision in flight");
    assert!(
        captured
            .windows(2)
            .all(|pair| pair[0].snapshot <= pair[1].snapshot)
    );
    for hold in &captured {
        assert_hold_matches_ledger(&fixture, hold, 7);
        assert_eq!(
            fixture
                .store
                .source_hold_status(&hold.binding, &hold.hold_id, sweep_now)
                .unwrap(),
            *hold,
            "every byte captured under a live sweep is still on disk"
        );
    }
    // Holds captured after the deletion exclude the deleted evidence; the one
    // captured before still pins it, so the sweeps could not reclaim it.
    for evidence_id in &doomed {
        let digest = fixture.entry_by_evidence(evidence_id).digest.clone();
        assert!(
            fixture.object_present(&digest),
            "{evidence_id} reclaimed while pinned"
        );
        assert!(
            fixture
                .pin_refs(&pinned_before_delete.hold_id)
                .contains(evidence_id)
        );
        for hold in &captured {
            assert!(!fixture.pin_refs(&hold.hold_id).contains(evidence_id));
        }
    }
    assert_hold_matches_ledger(&fixture, &pinned_before_delete, 5);

    // Releasing the pin is what frees the bytes: past the grace period after
    // release the deleted evidence is reclaimed and everything else stays.
    let released_at = sweep_now;
    fixture
        .store
        .release_source_hold(
            &fixture.binding(),
            &pinned_before_delete.hold_id,
            released_at,
        )
        .unwrap();
    fixture.store.run_staging_maintenance(sweep_now).unwrap();
    for evidence_id in &doomed {
        let digest = fixture.entry_by_evidence(evidence_id).digest.clone();
        assert!(
            fixture.object_present(&digest),
            "released but inside the grace period"
        );
    }
    fixture
        .store
        .run_staging_maintenance(released_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    for entry in fixture.ledger.values() {
        assert_eq!(
            fixture.object_present(&entry.digest),
            !doomed.contains(&entry.evidence_id),
            "{}",
            entry.evidence_id
        );
    }
}

#[test]
fn admission_precedes_reference_materialization_and_refusal_leaves_no_partial_hold() {
    let mut fixture = Fixture::open();
    // An empty corpus is admitted with zero references and zero bytes.
    let empty = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    assert_eq!((empty.references, empty.encoded_bytes), (0, 0));
    assert!(fixture.held_all(&empty, 4).is_empty());

    fixture.seed_five_classes();
    let (expected, references, bytes) = fixture.expected_at(fixture.store.tip().unwrap());
    assert_eq!(expected.len(), 10);
    let exact = SourceHoldBounds {
        max_references: NonZeroUsize::new(references).unwrap(),
        max_encoded_bytes: NonZeroU64::new(bytes).unwrap(),
        expiry_ms: NonZeroU64::new(HOUR_MS).unwrap(),
    };
    let refs_before = fixture.count("SELECT COUNT(*) FROM capture_pin_refs");
    let mut seen_at_admission = Vec::new();
    let hold = fixture
        .store
        .capture_source_hold_with_hook_for_test(&fixture.binding(), exact, |refs| {
            seen_at_admission.push(refs);
        })
        .unwrap();
    assert_eq!(
        seen_at_admission,
        vec![refs_before],
        "admitted before any reference row"
    );
    assert_eq!(hold.references, references);
    assert_eq!(hold.encoded_bytes, bytes);
    assert_hold_matches_ledger(&fixture, &hold, 4);

    let pins_before = fixture.count("SELECT COUNT(*) FROM capture_pins");
    let refs_before = fixture.count("SELECT COUNT(*) FROM capture_pin_refs");
    for tight in [
        SourceHoldBounds {
            max_references: NonZeroUsize::new(references - 1).unwrap(),
            ..exact
        },
        SourceHoldBounds {
            max_encoded_bytes: NonZeroU64::new(bytes - 1).unwrap(),
            ..exact
        },
    ] {
        let mut seen = Vec::new();
        let error = fixture
            .store
            .capture_source_hold_with_hook_for_test(&fixture.binding(), tight, |refs| {
                seen.push(refs);
            })
            .unwrap_err();
        assert_eq!(
            error,
            SourceHoldError::Unadmitted {
                references,
                encoded_bytes: bytes,
            }
        );
        assert_eq!(seen, vec![refs_before], "refused before any reference row");
    }
    // No pin, no reference, and no narrowed inventory came out of a refusal.
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM capture_pins"),
        pins_before
    );
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM capture_pin_refs"),
        refs_before
    );
    assert_eq!(fixture.checkpoint(), 0);
}

#[test]
fn backup_release_refuses_source_holds_without_changing_their_references() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let references = fixture.pin_refs(&hold.hold_id);
    assert_eq!(
        fixture
            .store
            .release_capture_pin(&hold.hold_id, hold.captured_at),
        Err(KernelError::NotFound)
    );
    assert_eq!(fixture.pin_refs(&hold.hold_id), references);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Ok(hold.clone())
    );
    fixture
        .store
        .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
        .unwrap();
    assert!(fixture.pin_refs(&hold.hold_id).is_empty());
}

#[test]
fn hold_status_observes_invalidation_during_object_verification() {
    for purge in [true, false] {
        let mut fixture = Fixture::open();
        fixture.seed_five_classes();
        let binding = fixture.binding();
        let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
        let digest = fixture.held_all(&hold, 64)[0].artifact_digest.clone();
        let expected = if purge {
            SourceHoldInvalidity::PurgeDegraded
        } else {
            SourceHoldInvalidity::Released
        };
        let result = fixture.store.source_hold_status_with_hook_for_test(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            || {
                if purge {
                    let error = fixture
                        .store
                        .delete_artifact_with_fault_for_test(
                            ArtifactDeletionRequest {
                                intent: intent("purge-during-verification"),
                                identity: ArtifactDeletionIdentity::Digest(digest.clone()),
                                kind: ArtifactDeletionKind::Purge,
                                operator_id: Some("operator".to_string()),
                                target_locator: Some("incident://verification".to_string()),
                                reason: Some("retired".to_string()),
                                deleted_at: hold.captured_at,
                            },
                            kernel::ArtifactDeletionFault::AfterCommit,
                        )
                        .unwrap_err();
                    assert_eq!(error.kind(), kernel::ArtifactErrorKind::PurgeUnlinkPending);
                    assert!(
                        fixture
                            .root
                            .path()
                            .join("artifacts/objects")
                            .join(&digest[..2])
                            .join(&digest[2..])
                            .is_file()
                    );
                } else {
                    fixture
                        .store
                        .release_source_hold(&binding, &hold.hold_id, hold.captured_at)
                        .unwrap();
                }
            },
        );
        assert_eq!(
            result,
            Err(SourceHoldError::Invalid(expected)),
            "purge={purge}"
        );
    }
}

#[test]
fn future_release_times_do_not_extend_hold_retention() {
    for removal in ["release", "deregister", "abandon", "reconcile"] {
        for maximum in [true, false] {
            let mut fixture = Fixture::open();
            fixture.seed_five_classes();
            let binding = fixture.binding();
            let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
            let released_at = if maximum {
                i64::MAX
            } else {
                hold.expires_at + 1
            };
            match removal {
                "release" => fixture
                    .store
                    .release_source_hold(&binding, &hold.hold_id, released_at)
                    .unwrap(),
                "deregister" => {
                    fixture
                        .store
                        .acknowledge_outbox(CONSUMER, fixture.store.tip().unwrap(), 1)
                        .unwrap();
                    fixture
                        .store
                        .commit(intent("deregister-future"), |envelope| {
                            envelope.deregister_outbox_consumer(CONSUMER, released_at)?;
                            Ok(String::new())
                        })
                        .unwrap();
                }
                "abandon" => {
                    fixture
                        .store
                        .commit(intent("abandon-future"), |envelope| {
                            envelope.abandon_outbox_consumer(
                                CONSUMER,
                                kernel::ConsumerAbandonment {
                                    operator_id: "operator".to_string(),
                                    reason: "retired".to_string(),
                                    abandoned_at: released_at,
                                    barrier_id: None,
                                },
                            )?;
                            Ok(String::new())
                        })
                        .unwrap();
                }
                "reconcile" => {
                    fixture = fixture.reopen();
                    assert_eq!(
                        fixture
                            .store
                            .reconcile_source_holds(CONSUMER, released_at)
                            .unwrap(),
                        std::slice::from_ref(&hold.hold_id)
                    );
                }
                _ => unreachable!(),
            }
            let connection = fixture.inspect();
            let stored: i64 = connection
                .query_row(
                    "SELECT released_at FROM capture_pins WHERE capture_pin_id=?1",
                    [&hold.hold_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                stored, hold.expires_at,
                "{removal}: release extended the hold lifetime"
            );
            let references: i64 = connection.query_row(
                "SELECT COUNT(*) FROM capture_pin_refs WHERE capture_pin_id=?1 AND released_at=?2",
                rusqlite::params![hold.hold_id, stored], |row| row.get(0),
            ).unwrap();
            assert_eq!(usize::try_from(references).unwrap(), hold.references);
            fixture
                .store
                .run_capture_pin_maintenance(
                    hold.expires_at + 14 * i64::try_from(DAY_MS).unwrap() + 1,
                )
                .unwrap();
            assert_eq!(fixture.count("SELECT COUNT(*) FROM capture_pin_refs"), 0);
        }
    }
}

#[test]
fn repeated_purges_preserve_the_first_hold_degradation() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let descriptors = fixture.held_all(&hold, 64);
    let first = &descriptors[0];
    let second = descriptors
        .iter()
        .find(|descriptor| descriptor.artifact_digest != first.artifact_digest)
        .unwrap();
    let degradation = |fixture: &Fixture| -> (i64, String) {
        fixture
            .inspect()
            .query_row(
                "SELECT purge_degraded_at,purge_barrier_id FROM capture_pins
                 WHERE capture_pin_id=?1",
                [&hold.hold_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    };

    let first_commit = fixture.delete_evidence(&first.evidence_id, ArtifactDeletionKind::Purge, 42);
    let first_degradation = degradation(&fixture);
    assert_eq!(first_degradation.0, 42);
    let second_commit =
        fixture.delete_evidence(&second.evidence_id, ArtifactDeletionKind::Purge, 43);
    assert!(second_commit > first_commit);
    assert_eq!(degradation(&fixture), first_degradation);
    assert_eq!(
        fixture.count("SELECT COUNT(*) FROM artifact_purge_tombstones"),
        2
    );
    assert_eq!(fixture.pin_refs(&hold.hold_id).len(), hold.references);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at),
        Err(SourceHoldError::Invalid(
            SourceHoldInvalidity::PurgeDegraded
        ))
    );

    let fresh = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_eq!(fresh.references + 2, hold.references);
    assert_hold_matches_ledger(&fixture, &fresh, 4);
}

#[test]
fn purge_expiry_missing_bytes_and_release_invalidate_the_hold_without_moving_the_consumer() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let checkpoint = fixture.store.tip().unwrap();
    fixture
        .store
        .acknowledge_outbox(CONSUMER, checkpoint, 1)
        .unwrap();
    assert!(checkpoint > 0);
    assert_eq!(fixture.checkpoint(), checkpoint);
    let status = |fixture: &Fixture, hold: &SourceHold, now: i64| {
        fixture
            .store
            .source_hold_status(&binding, &hold.hold_id, now)
    };

    // Expiry: judged against the caller's clock, and the maintenance sweep
    // releases the expired pin durably. A dead hold serves no page.
    let expiring = fixture
        .store
        .capture_source_hold(&binding, bounds(1))
        .unwrap();
    let fresh = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_eq!(
        status(&fixture, &expiring, expiring.expires_at - 1),
        Ok(expiring.clone())
    );
    assert_eq!(
        status(&fixture, &expiring, expiring.expires_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
    assert_eq!(
        fixture.first_page(&binding, &expiring.hold_id, expiring.expires_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Expired))
    );
    assert_eq!(
        status(&fixture, &fresh, expiring.expires_at),
        Ok(fresh.clone())
    );
    fixture
        .store
        .run_capture_pin_maintenance(expiring.expires_at + 1)
        .unwrap();
    assert_eq!(
        status(&fixture, &expiring, 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
    );
    assert_eq!(
        status(&fixture, &fresh, expiring.expires_at),
        Ok(fresh.clone())
    );

    // Purge degradation: purging held evidence marks the pin degraded, and a
    // hold captured afterwards does not list the purged descriptor.
    let held = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let victim = fixture.held_all(&held, 64)[0].clone();
    fixture.delete_evidence(&victim.evidence_id, ArtifactDeletionKind::Purge, 42);
    assert_eq!(
        status(&fixture, &held, held.captured_at),
        Err(SourceHoldError::Invalid(
            SourceHoldInvalidity::PurgeDegraded
        ))
    );
    assert_eq!(
        fixture.first_page(&binding, &held.hold_id, held.captured_at),
        Err(SourceHoldError::Invalid(
            SourceHoldInvalidity::PurgeDegraded
        ))
    );
    let after_purge = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    assert_hold_matches_ledger(&fixture, &after_purge, 4);
    assert!(
        !fixture
            .held_all(&after_purge, 64)
            .iter()
            .any(|d| d.evidence_id == victim.evidence_id)
    );
    assert_eq!(after_purge.references + 1, held.references);

    // Missing bytes: an object removed from disk behind a valid pin.
    assert_eq!(
        status(&fixture, &after_purge, after_purge.captured_at),
        Ok(after_purge.clone())
    );
    let gone = fixture.held_all(&after_purge, 64)[0].clone();
    let gone_path = fixture
        .root
        .path()
        .join("artifacts/objects")
        .join(&gone.artifact_digest[..2])
        .join(&gone.artifact_digest[2..]);
    // A shard that cannot be probed is an I/O failure, not `MissingBytes`: the
    // hold still protects its bytes.
    let shard = gone_path.parent().unwrap();
    fs::set_permissions(shard, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::metadata(&gone_path).is_ok() {
        // A privileged test process ignores the mode bits.
        fs::set_permissions(shard, fs::Permissions::from_mode(0o700)).unwrap();
    } else {
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Err(SourceHoldError::Kernel(KernelError::Io))
        );
        fs::set_permissions(shard, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Ok(after_purge.clone())
        );
    }
    let original = fs::read(&gone_path).unwrap();
    let mut overwritten = original.clone();
    overwritten[0] ^= 1;
    for damaged in [&original[..original.len() - 1], overwritten.as_slice()] {
        fs::write(&gone_path, damaged).unwrap();
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
        );
        fs::write(&gone_path, &original).unwrap();
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Ok(after_purge.clone())
        );
    }
    fs::set_permissions(&gone_path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&gone_path).is_err() {
        assert_eq!(
            status(&fixture, &after_purge, after_purge.captured_at),
            Err(SourceHoldError::Kernel(KernelError::Io))
        );
    }
    fs::set_permissions(&gone_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::remove_file(&gone_path).unwrap();
    assert_eq!(
        status(&fixture, &after_purge, after_purge.captured_at),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::MissingBytes))
    );

    // Release is idempotent for an existing hold and an error for a missing one.
    let released = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    fixture
        .store
        .release_source_hold(&binding, &released.hold_id, 7)
        .unwrap();
    fixture
        .store
        .release_source_hold(&binding, &released.hold_id, 8)
        .unwrap();
    assert_eq!(
        status(&fixture, &released, 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
    );
    assert_eq!(
        fixture.first_page(&binding, &released.hold_id, 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
    );
    assert!(fixture.pin_refs(&released.hold_id).is_empty());
    assert_eq!(
        fixture
            .store
            .source_hold_status(&binding, "no-such-hold", 0),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))
    );
    assert_eq!(
        fixture
            .store
            .release_source_hold(&binding, "no-such-hold", 7),
        Err(SourceHoldError::Invalid(SourceHoldInvalidity::Missing))
    );
    // Nothing above acknowledged, rewound, or abandoned the consumer.
    assert_eq!(fixture.checkpoint(), checkpoint);
    assert_eq!(fixture.count("SELECT COUNT(*) FROM outbox_consumers"), 1);
}

#[test]
fn a_new_incarnation_reconciles_old_holds_and_captures_a_new_s() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let old_binding = fixture.binding();
    let long = bounds(MAX_SOURCE_HOLD_LIFETIME_MS);
    let old = fixture
        .store
        .capture_source_hold(&old_binding, long)
        .unwrap();
    let older = fixture
        .store
        .capture_source_hold(&old_binding, long)
        .unwrap();
    let old_inventory = fixture.held_all(&old, 64);
    // Another consumer's hold in the old incarnation is not this consumer's to reconcile.
    fixture
        .store
        .commit(intent("other-consumer"), |envelope| {
            envelope.register_outbox_consumer("other", 1)?;
            Ok(String::new())
        })
        .unwrap();
    let other_binding = SourceHoldBinding {
        consumer_id: "other".to_string(),
        ..old_binding.clone()
    };
    let other = fixture
        .store
        .capture_source_hold(&other_binding, long)
        .unwrap();
    // Evidence deleted after the holds took S stays pinned by them alone,
    // until every one of them is released.
    let doomed = fixture.live_entry("git_commits", "a").evidence_id.clone();
    let deleted_at = wall_ms();
    fixture.delete_evidence(&doomed, ArtifactDeletionKind::Delete, deleted_at);

    let mut fixture = fixture.reopen();
    let new_binding = fixture.binding();
    assert_ne!(new_binding.lease_epoch, old_binding.lease_epoch);

    // The old hold cannot be used under the new incarnation.
    assert_eq!(
        fixture
            .store
            .source_hold_status(&new_binding, &old.hold_id, old.captured_at)
            .unwrap_err(),
        SourceHoldError::BindingMismatch
    );
    let released_at = wall_ms();
    let mut released = fixture
        .store
        .reconcile_source_holds(CONSUMER, released_at)
        .unwrap();
    released.sort();
    let mut expected = vec![old.hold_id.clone(), older.hold_id.clone()];
    expected.sort();
    assert_eq!(released, expected);
    assert!(
        fixture
            .store
            .reconcile_source_holds(CONSUMER, released_at)
            .unwrap()
            .is_empty()
    );
    for hold in [&old, &older] {
        assert_eq!(
            fixture
                .store
                .source_hold_status(&old_binding, &hold.hold_id, 0),
            Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released))
        );
        assert!(
            fixture.pin_refs(&hold.hold_id).is_empty(),
            "references released with the pin"
        );
    }
    // The other consumer's old hold is untouched and still pins the deleted evidence.
    assert_eq!(
        fixture
            .store
            .source_hold_status(&other_binding, &other.hold_id, other.captured_at),
        Ok(other.clone())
    );
    assert!(fixture.pin_refs(&other.hold_id).contains(&doomed));

    // A new S is captured fresh; the old cursor is not resumed, and the
    // deleted evidence is not part of it.
    fixture.publish("messages", "after-reopen", 1, "after reopen");
    let fresh = fixture
        .store
        .capture_source_hold(&new_binding, long)
        .unwrap();
    assert!(fresh.snapshot > old.snapshot);
    assert_hold_matches_ledger(&fixture, &fresh, 4);
    assert!(!fixture.pin_refs(&fresh.hold_id).contains(&doomed));
    // Immutable bytes survived the reopen and reconciliation.
    for descriptor in &old_inventory {
        let entry = fixture.entry_by_evidence(&descriptor.evidence_id);
        assert_eq!(
            fs::read(
                fixture
                    .root
                    .path()
                    .join("artifacts/objects")
                    .join(&descriptor.artifact_digest[..2])
                    .join(&descriptor.artifact_digest[2..])
            )
            .unwrap(),
            entry.text.as_bytes()
        );
    }
    // Reconciliation freed protection: once the other consumer's pin is also
    // released and the grace period passes, the deleted evidence is reclaimed
    // while everything the fresh hold cites stays.
    fixture
        .store
        .reconcile_source_holds("other", released_at)
        .unwrap();
    let doomed_digest = fixture.entry_by_evidence(&doomed).digest.clone();
    fixture
        .store
        .run_staging_maintenance(released_at + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    assert!(!fixture.object_present(&doomed_digest));
    assert_eq!(
        fixture
            .store
            .source_hold_status(&new_binding, &fresh.hold_id, fresh.captured_at),
        Ok(fresh.clone())
    );

    // A name-only remediation rewrites the domain name and changes no held input.
    let before_name = fixture.domain_name();
    let receipt = fixture
        .store
        .commit(intent("remediate"), |envelope| {
            envelope.remediate_text(
                RemediationTarget::CanonicalDomainName {
                    object_id: "domain-object".to_string(),
                },
                "operator",
                5,
            )?;
            Ok(String::new())
        })
        .unwrap();
    assert!(receipt.commit_seq > fresh.snapshot);
    assert_ne!(
        fixture.domain_name(),
        before_name,
        "the remediation changed the name"
    );
    assert_hold_matches_ledger(&fixture, &fresh, 4);
    assert_eq!(
        fixture
            .store
            .source_hold_status(&new_binding, &fresh.hold_id, fresh.captured_at),
        Ok(fresh.clone())
    );
}

#[test]
fn retention_is_capped_and_a_removed_consumer_leaves_no_hold_behind() {
    for abandon in [false, true] {
        let mut fixture = Fixture::open();
        fixture.seed_five_classes();
        let binding = fixture.binding();

        // A lifetime past the kernel ceiling is refused whole; the ceiling itself
        // is admitted.
        assert_eq!(
            fixture
                .store
                .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS + 1))
                .unwrap_err(),
            SourceHoldError::InvalidRequest
        );
        let capped = fixture
            .store
            .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
            .unwrap();
        assert_eq!(
            capped.expires_at - capped.captured_at,
            i64::try_from(MAX_SOURCE_HOLD_LIFETIME_MS).unwrap()
        );

        // The consumer holds under two policy versions; another consumer holds too.
        let mut other_policy = binding.clone();
        other_policy.source_policy_version = "source-policy.v2".to_string();
        let second = fixture
            .store
            .capture_source_hold(&other_policy, wide())
            .unwrap();
        fixture
            .store
            .commit(intent("other-consumer"), |envelope| {
                envelope.register_outbox_consumer("other", 1)?;
                Ok(String::new())
            })
            .unwrap();
        let other_binding = SourceHoldBinding {
            consumer_id: "other".to_string(),
            ..binding.clone()
        };
        let other = fixture
            .store
            .capture_source_hold(&other_binding, wide())
            .unwrap();

        let tip = fixture.store.tip().unwrap();
        fixture.store.acknowledge_outbox(CONSUMER, tip, 1).unwrap();
        fixture
            .store
            .commit(intent("remove-consumer"), |envelope| {
                if abandon {
                    envelope.abandon_outbox_consumer(
                        CONSUMER,
                        kernel::ConsumerAbandonment {
                            operator_id: "operator".to_string(),
                            reason: "retired".to_string(),
                            abandoned_at: 9,
                            barrier_id: None,
                        },
                    )?;
                } else {
                    envelope.deregister_outbox_consumer(CONSUMER, 9)?;
                }
                Ok(String::new())
            })
            .unwrap();
        for hold in [&capped, &second] {
            assert_eq!(
                fixture
                    .store
                    .source_hold_status(&hold.binding, &hold.hold_id, hold.captured_at),
                Err(SourceHoldError::Invalid(SourceHoldInvalidity::Released)),
                "abandon={abandon}, hold={}",
                hold.hold_id
            );
            assert!(fixture.pin_refs(&hold.hold_id).is_empty());
        }
        assert_eq!(
            fixture.count(
                "SELECT COUNT(*) FROM capture_pins
             WHERE pin_kind='source_hold' AND released_at IS NULL"
            ),
            1
        );
        assert_eq!(
            fixture.count(
                "SELECT COUNT(*) FROM capture_pins
             WHERE pin_kind='source_hold' AND released_at=9"
            ),
            2
        );
        assert_eq!(
            fixture
                .store
                .source_hold_status(&other_binding, &other.hold_id, other.captured_at),
            Ok(other.clone())
        );
    }
}
