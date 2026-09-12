//! The retrieval eligibility adapter, the daemon route, and the kernel judge the same claim occurrences identically at one snapshot; retirement and correction after a grant refuse it with the kernel's exact verdict; a foreign project sees wrong scope.

mod support;

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;

use daemon::claim_sources::{ClaimMaterializer, MaterializationEnd};
use daemon::search_projection::{SearchProjection, SearchProjectionError};
use kernel::source_identity::OccurrenceClass;
use kernel::{
    ArtifactDestination, CommitIntent, CommitPageBounds, EligibilityCandidate, EligibilityVerdict,
    ExportWindow, KernelError, KernelStore, MAX_ELIGIBILITY_CANDIDATES, ProjectScope,
    ProviderEgress, SourceHold, SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds,
    SourcePageBounds, SourceRow,
};
use retrieval::batch::{BatchBounds, MutationIdentity, batch_from_rows, row_identities};
use retrieval::eligibility::{
    Disposition, EligibilityReport, OccurrenceCandidate, judge_occurrences, live_candidates,
};
use retrieval::{PersistBounds, ProjectionError, ProjectionIdentity, install_identity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::kernel_daemon::{KernelDaemon, verdicts};

const CONSUMER: &str = "search";
const POLICY: &str = "source-policy.v1";
const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MEMORY: &str = "memory";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MODEL: &str = "tiny-test-model";
const FINGERPRINT: &str = "a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234a2b4c6d8e0f01234";
const NOW: i64 = 1_000;
const CONTRACT: &str = "Keep the public contract.";

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-claim-sources-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn bounds() -> CommitPageBounds {
    CommitPageBounds {
        max_commits: NonZeroUsize::new(8).unwrap(),
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

fn batch_bounds() -> BatchBounds {
    BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    }
}

fn page_bounds() -> SourcePageBounds {
    SourcePageBounds {
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

fn hold_admission() -> SourceHoldAdmission {
    SourceHoldAdmission {
        max_references: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
    }
}

fn projection_identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: POLICY.to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: MODEL.to_string(),
        tokenizer_fingerprint: FINGERPRINT.to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

/// A kernel and a hold to export it under.
struct Corpus {
    kernel: Arc<KernelStore>,
}

impl Corpus {
    fn binding(&self) -> SourceHoldBinding {
        SourceHoldBinding {
            consumer_id: CONSUMER.to_string(),
            lease_epoch: self.kernel.lease_epoch(),
            source_policy_version: POLICY.to_string(),
        }
    }

    fn capture(&self) -> SourceHold {
        self.kernel
            .capture_source_hold(
                &self.binding(),
                SourceHoldBounds {
                    max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
                    admission: hold_admission(),
                    expiry_ms: NonZeroU64::new((20 * DAY_MS) as u64).unwrap(),
                },
            )
            .unwrap()
    }

    /// Every page of `window` under `hold`.
    fn export_window(&self, hold: &SourceHold, window: ExportWindow) -> Vec<SourceRow> {
        let binding = self.binding();
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .kernel
                .export_source_page(
                    &binding,
                    &hold.hold_id,
                    hold.captured_at,
                    window,
                    cursor.as_ref(),
                    page_bounds(),
                )
                .unwrap();
            rows.extend(page.rows);
            match page.next {
                Some(next) => cursor = Some(next),
                None => return rows,
            }
        }
    }

    /// Every descriptor live at a fresh S, with its text.
    fn export(&self) -> Vec<SourceRow> {
        let hold = self.capture();
        let rows = self.export_window(&hold, ExportWindow::Snapshot);
        self.kernel
            .release_source_hold(&self.binding(), &hold.hold_id, hold.captured_at)
            .unwrap();
        rows
    }
}

/// The wire spelling of a kernel verdict, as the daemon route serializes it.
fn wire_verdict(disposition: Disposition) -> String {
    match disposition {
        Disposition::Eligible => "ok".to_string(),
        Disposition::PolicyExcluded(verdict) => {
            format!("{verdict:?}")
                .chars()
                .fold(String::new(), |mut out, c| {
                    if c.is_uppercase() && !out.is_empty() {
                        out.push('_');
                    }
                    out.push(c.to_ascii_lowercase());
                    out
                })
        }
    }
}

/// AC4: on one snapshot the retrieval adapter, the daemon route, and the kernel agree on every claim occurrence; a grant taken before a retirement or a correction is refused afterward with the exact verdict per occurrence; a foreign project sees wrong scope; the adapter's exclusions name the kernel's verdicts per class; an over-bound batch is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retrieval_adapter_agrees_with_daemon_and_kernel_on_one_snapshot() {
    let daemon = KernelDaemon::start().await;
    let store = daemon.store();
    // Register the consumer before committing decisions so its checkpoint does not skip them.
    ClaimMaterializer::register(&store, NOW).unwrap();
    let spec = |object: &str, lineage: &str, revision: i64, summary: &str| {
        json!({
            "decision_id": format!("{object}-decision"),
            "object_id": object,
            "domain_id": MEMORY,
            "decision_kind": "PROJECT_RULES",
            "payload": {"summary": summary, "rationale": format!("because {object}")},
            "source_id": format!("{lineage}-lineage"),
            "source_revision": revision,
        })
    };
    let created = daemon
        .commit(
            "create",
            vec![
                json!({"op": "insert_decision", "spec": spec("rule", "rule", 1, CONTRACT)}),
                json!({"op": "insert_decision", "spec": spec("other", "other", 1, "Name things.")}),
            ],
        )
        .await;
    assert_eq!(created["state"]["kind"], "available", "{created}");
    let scope_id = daemon.read("explicit_search", None, None).await["rows"][0]["scope_id"]
        .as_str()
        .unwrap()
        .to_string();
    let project = ProjectScope::new(scope_id.strip_prefix("project:").unwrap()).unwrap();
    store
        .commit(intent("search"), |envelope| {
            envelope.register_outbox_consumer(CONSUMER, 1)?;
            Ok(String::new())
        })
        .unwrap();
    let mut materializer = ClaimMaterializer::new(&store, ProviderEgress::LocalOnly);
    let report = materializer.run_episode(bounds(), NOW).unwrap();
    assert!(
        matches!(report.end, MaterializationEnd::ReachedTarget),
        "{report:?}"
    );
    assert_eq!(report.published, 6, "{report:?}");

    let dir = tempfile::tempdir().unwrap();
    let corpus = Corpus {
        kernel: Arc::clone(&store),
    };
    let hold = corpus.capture();
    let rows = corpus.export_window(&hold, ExportWindow::Snapshot);
    let projection = SearchProjection::open(dir.path()).unwrap();
    projection
        .write(|conn| install_identity(conn, &projection_identity("route-kernel"), 1).map(|_| ()))
        .unwrap();
    let identities = row_identities(&rows);
    let batch = batch_from_rows(
        &rows,
        &identities,
        MutationIdentity {
            kernel_incarnation_id: "route-kernel".to_string(),
            hold_id: hold.hold_id.clone(),
            snapshot_commit_seq: hold.snapshot,
            through_commit_seq: hold.snapshot,
        },
        None,
    )
    .unwrap();
    projection.apply_batch(&batch, batch_bounds(), 2).unwrap();

    let candidates = projection
        .read(|conn| live_candidates(conn, None, NonZeroUsize::new(64).unwrap()))
        .unwrap();
    assert_eq!(candidates.len(), 6);
    let promoted = projection
        .read(|conn| {
            live_candidates(
                conn,
                Some(OccurrenceClass::PromotedMemory),
                NonZeroUsize::new(64).unwrap(),
            )
        })
        .unwrap();
    assert_eq!(promoted.len(), 2);
    assert!(
        promoted
            .iter()
            .all(|c| c.class == OccurrenceClass::PromotedMemory)
    );
    assert!(matches!(
        projection.read(|conn| live_candidates(conn, None, NonZeroUsize::new(5).unwrap())),
        Err(SearchProjectionError::Projection(
            ProjectionError::TooManyRecords { count: 6 }
        ))
    ));
    let wire: Vec<Value> = candidates
        .iter()
        .map(|c| {
            json!({
                "object_id": c.candidate.object_id,
                "source_revision": c.candidate.source_revision,
                "artifact_digest": c.candidate.artifact_digest,
            })
        })
        .collect();
    let kernel_candidates: Vec<_> = candidates.iter().map(|c| c.candidate.clone()).collect();
    // The decision each occurrence derives from, read from its descriptor's identity.
    let decision_of = |occurrence_id: &str| -> String {
        rows.iter()
            .find(|row| row.detail.occurrence_id == occurrence_id)
            .unwrap()
            .detail
            .identity[0]
            .1
            .clone()
    };
    let compare = |adapter: &EligibilityReport,
                   route: &Value,
                   expected: &dyn Fn(&str) -> EligibilityVerdict| {
        let route_verdicts: BTreeMap<String, String> = verdicts(route).into_iter().collect();
        let kernel_batch = store
            .judge_eligibility(&project, ArtifactDestination::Local, &kernel_candidates)
            .unwrap();
        assert_eq!(adapter.snapshot.tip, kernel_batch.snapshot.tip);
        assert_eq!(route["known_as_of"], adapter.snapshot.tip);
        for ((judged, candidate), kernel_verdict) in adapter
            .occurrences
            .iter()
            .zip(&candidates)
            .zip(kernel_batch.verdicts)
        {
            let want = Disposition::from(expected(&decision_of(&judged.occurrence_id)));
            assert_eq!(judged.disposition, want, "{judged:?}");
            assert_eq!(judged.disposition, Disposition::from(kernel_verdict));
            assert_eq!(
                route_verdicts[&candidate.candidate.object_id],
                wire_verdict(judged.disposition),
                "{judged:?}"
            );
        }
    };

    let adapter =
        judge_occurrences(&store, &project, ArtifactDestination::Local, &candidates).unwrap();
    let route = daemon.eligibility("local", wire.clone()).await;
    compare(&adapter, &route, &|_| EligibilityVerdict::Ok);
    assert!(adapter.is_reusable());
    assert!(adapter.exclusions().is_empty());

    // A retirement after the grant: the route's cached verdicts do not survive the tip, and the adapter's exclusions name the kernel's verdict per class.
    let retired = daemon
        .commit(
            "retire",
            vec![json!({"op": "retire_decision", "object_id": "other"})],
        )
        .await;
    assert_eq!(retired["state"]["kind"], "available", "{retired}");
    let report = materializer.run_episode(bounds(), NOW).unwrap();
    assert_eq!(report.retired, 3, "{report:?}");
    let adapter =
        judge_occurrences(&store, &project, ArtifactDestination::Local, &candidates).unwrap();
    let route = daemon.eligibility("local", wire.clone()).await;
    compare(&adapter, &route, &|decision| match decision {
        "other" => EligibilityVerdict::Retracted,
        _ => EligibilityVerdict::Ok,
    });
    assert_eq!(
        adapter
            .exclusions()
            .iter()
            .map(|e| (e.class.code(), e.verdict, e.count))
            .collect::<Vec<_>>(),
        vec![
            ("canonical_claims", EligibilityVerdict::Retracted, 2),
            ("promoted_memory", EligibilityVerdict::Retracted, 1),
        ]
    );

    // A correction after the grant: the old lineage's descriptors are retired, so the grant taken before it is refused for every representation.
    let corrected = daemon
        .commit(
            "correct",
            vec![json!({
                "op": "supersede_decision",
                "replaced_object_id": "rule",
                "spec": spec("rule-v2", "rule", 2, CONTRACT),
            })],
        )
        .await;
    assert_eq!(corrected["state"]["kind"], "available", "{corrected}");
    let report = materializer.run_episode(bounds(), NOW).unwrap();
    assert_eq!((report.retired, report.published), (3, 3), "{report:?}");
    let adapter =
        judge_occurrences(&store, &project, ArtifactDestination::Local, &candidates).unwrap();
    let route = daemon.eligibility("local", wire.clone()).await;
    compare(&adapter, &route, &|_| EligibilityVerdict::Retracted);

    let over: Vec<_> =
        std::iter::repeat_n(candidates[0].clone(), MAX_ELIGIBILITY_CANDIDATES + 1).collect();
    assert_eq!(
        judge_occurrences(&store, &project, ArtifactDestination::Local, &over).unwrap_err(),
        KernelError::InvalidInput
    );
    let fresh = projection
        .read(|conn| live_candidates(conn, None, NonZeroUsize::new(64).unwrap()))
        .unwrap();
    let foreign = ProjectScope::new(PROJECT).unwrap();
    let current: Vec<_> = {
        let rows = corpus.export();
        fresh
            .iter()
            .filter(|c| {
                rows.iter().any(|row| {
                    row.object_id == c.candidate.object_id && row.invalidated_commit_seq.is_none()
                })
            })
            .cloned()
            .collect()
    };
    assert!(
        current.is_empty(),
        "the snapshot projection holds only retired lineages"
    );
    let live_now = corpus.export();
    let live_candidates_now: Vec<_> = live_now
        .iter()
        .map(|row| OccurrenceCandidate {
            occurrence_id: row.detail.occurrence_id.clone(),
            class: OccurrenceClass::from_code(&row.detail.class).unwrap(),
            candidate: EligibilityCandidate {
                object_id: row.object_id.clone(),
                source_revision: row.revision,
                artifact_digest: Some(row.detail.artifact_digest.clone()),
            },
        })
        .collect();
    assert_eq!(live_candidates_now.len(), 3);
    let adapter = judge_occurrences(
        &store,
        &foreign,
        ArtifactDestination::Local,
        &live_candidates_now,
    )
    .unwrap();
    let dispositions = |report: &EligibilityReport| -> Vec<Disposition> {
        report.occurrences.iter().map(|o| o.disposition).collect()
    };
    assert_eq!(
        dispositions(&adapter),
        vec![
            Disposition::PolicyExcluded(EligibilityVerdict::WrongScope);
            live_candidates_now.len()
        ],
        "{adapter:?}"
    );
    let adapter = judge_occurrences(
        &store,
        &project,
        ArtifactDestination::Local,
        &live_candidates_now,
    )
    .unwrap();
    assert_eq!(
        dispositions(&adapter),
        vec![Disposition::Eligible; live_candidates_now.len()],
        "{adapter:?}"
    );
    daemon.shutdown().await;
}
