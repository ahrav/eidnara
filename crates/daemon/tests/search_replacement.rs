#[path = "../../kernel/tests/source_fixture/mod.rs"]
mod source_fixture;
mod support;

#[path = "search_replacement/selection.rs"]
mod selection;
use selection::selection_child;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use daemon::projection_gates::HookGate;
use daemon::projection_lifecycle::{
    Cause, ConsumerBinding, ControlState, LifecycleRequest, ProjectionLifecycle, RecoveryTarget,
    Transition,
};
use daemon::search_catchup::{EpisodeBounds, EpisodeEvent};
use daemon::search_replacement::{BuildError, BuildEvent, ReplacementBuilder, ReplacementSpec};
use daemon::search_seed::SeedBounds;
use host_runtime::generation::{CurrentProfile, GenerationStore};
use kernel::{
    CommitPageBounds, ExportWindow, SourceHoldAdmission, SourceHoldBounds, SourcePageBounds,
};
use rusqlite::Connection;
use support::embedding_fixtures::{
    self as fixtures, Corpus, batch_bounds, budget, generation, identity, intent,
    kernel_incarnation_id,
};
use support::projection_gate::open_gate;

const CONSUMER: &str = "replacement";
const CLASSES: [&str; 5] = [
    "messages",
    "canonical_claims",
    "promoted_memory",
    "git_commits",
    "raw_tool_spans",
];

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn spec(root: &Path) -> ReplacementSpec {
    spec_with_identity(identity(&kernel_incarnation_id(root)))
}

fn spec_with_identity(identity: retrieval::ProjectionIdentity) -> ReplacementSpec {
    let admission = SourceHoldAdmission {
        max_references: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
    };
    ReplacementSpec {
        identity,
        generation: generation(),
        capture: SourceHoldBounds {
            admission,
            max_descriptor_rows: NonZeroUsize::new(256).unwrap(),
            expiry_ms: NonZeroU64::new(60_000).unwrap(),
        },
        episode: EpisodeBounds {
            commits: CommitPageBounds {
                max_commits: NonZeroUsize::MIN,
                max_rows: NonZeroUsize::new(64).unwrap(),
                max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
            },
            hold_admission: admission,
            source_page: SourcePageBounds {
                max_rows: NonZeroUsize::new(2).unwrap(),
                max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
            },
            max_source_pages: NonZeroUsize::new(64).unwrap(),
            max_source_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
            batch: batch_bounds(),
        },
        seed: SeedBounds {
            checkpoint_attempts: NonZeroU32::new(2).unwrap(),
            attempt_wait: Duration::from_millis(20),
            max_bytes: 64 << 20,
        },
    }
}

fn record(
    root: &Path,
    gate: &HookGate,
    target: Option<i64>,
    identity: &retrieval::ProjectionIdentity,
) {
    ProjectionLifecycle::open(root)
        .unwrap()
        .record(gate, &request(target, identity), now())
        .unwrap();
}

fn request(target: Option<i64>, identity: &retrieval::ProjectionIdentity) -> LifecycleRequest {
    LifecycleRequest {
        transition: Transition::Rebuilding,
        selected_generation: "old-selection".to_owned(),
        kernel_incarnation_id: identity.kernel_incarnation_id.clone(),
        consumer: ConsumerBinding {
            consumer_id: CONSUMER.to_owned(),
            generation_id: fixtures::GENERATION.to_owned(),
        },
        cause: Cause::DeletedAfterPruning,
        attempt_id: "replacement-attempt".to_owned(),
        recovery_target: target.map(|commit_seq| RecoveryTarget { commit_seq }),
        allowance: 3,
        deadline: now() + 60_000,
        authorization_ref: None,
    }
}

#[test]
fn authorized_recovery_requires_both_transition_and_construction_grants() {
    use daemon::projection_gates::{EntryPoint, ProjectionHook};
    for denied in [
        None,
        Some(ProjectionHook::EmbeddingBootstrap),
        Some(ProjectionHook::EmbeddingBackfill),
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        let config = spec(root.path());
        let request = LifecycleRequest {
            transition: Transition::AuthorizedRecovery,
            cause: Cause::DisabledRecovery,
            authorization_ref: Some("operator:recovery".to_owned()),
            ..request(None, &config.identity)
        };
        ProjectionLifecycle::open(root.path())
            .unwrap()
            .record(&gate, &request, now())
            .unwrap();
        let mut evaluator =
            support::projection_gate::passing_evaluator(&config.identity, 0, &ProjectionHook::ALL);
        if let Some(hook) = denied {
            evaluator.manifest.enabled.insert(hook, false);
        }
        gate.install(evaluator);
        let builder = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config);
        if denied.is_some() {
            assert!(builder.is_err());
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                None
            );
        } else {
            let candidate = builder
                .unwrap()
                .build(&budget(Duration::from_secs(30)), &mut |_| {})
                .unwrap();
            candidate.revalidate().unwrap();
            for hook in [
                ProjectionHook::EmbeddingBootstrap,
                ProjectionHook::EmbeddingBackfill,
            ] {
                assert!(gate.ledger().iter().any(|entry| entry.hook == hook
                    && entry.entry == EntryPoint::Reload
                    && entry.verdict.is_ok()));
            }
        }
    }
}

fn workspace(root: &Path) -> std::path::PathBuf {
    root.join("search-lifecycle/replacement")
}

fn database(root: &Path) -> std::path::PathBuf {
    workspace(root).join("search/search.sqlite")
}

fn control(root: &Path) -> daemon::projection_lifecycle::LifecycleIntent {
    match ProjectionLifecycle::open(root).unwrap().read() {
        ControlState::Intent(intent) => intent,
        other => panic!("{other:?}"),
    }
}

type Rows = BTreeMap<String, (String, Vec<u8>, Option<i64>)>;

fn rows(path: &Path) -> Rows {
    let conn = Connection::open(path).unwrap();
    conn.prepare("SELECT o.source_object_id,o.class,p.bytes,t.invalidated_commit_seq FROM occurrences o JOIN payloads p USING(payload_id) LEFT JOIN occurrence_tombstones t USING(occurrence_id) ORDER BY o.source_object_id")
        .unwrap().query_map([], |row| Ok((row.get(0)?, (row.get(1)?,row.get(2)?,row.get(3)?))))
        .unwrap().map(Result::unwrap).collect()
}

#[test]
fn construction_exports_five_classes_at_s_and_stages_exactly_t_without_selection() {
    let root = tempfile::tempdir().unwrap();
    let mut source = source_fixture::Fixture::open();
    let corpus = Corpus {
        kernel: source.store.clone(),
    };
    let incarnation: String = Connection::open(source.root.path().join("kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut config = spec_with_identity(identity(&incarnation));
    config.capture.expiry_ms = NonZeroU64::new(kernel::MAX_SOURCE_HOLD_LIFETIME_MS).unwrap();
    for class in CLASSES {
        let text = format!("  {class}\r\n\tλ\0exact ");
        if class == "raw_tool_spans" {
            let evidence = source.retain("raw", &text);
            source.publish_span(
                class,
                class,
                1,
                &text,
                Some((2, text.len() as u64 - 1)),
                evidence,
            );
        } else {
            source.publish(class, class, 1, &text);
        }
    }
    let gate = open_gate();
    record(root.path(), &gate, None, &config.identity);
    let mut changed = false;
    let mut target = None;
    let mut baseline_pages = 0;
    let mut verification_pages = 0;
    let mut verifying = false;
    let mut commits = Vec::new();
    let candidate = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| match event {
            BuildEvent::Exported {
                window: ExportWindow::Snapshot,
                ..
            } => {
                if verifying {
                    verification_pages += 1;
                } else {
                    baseline_pages += 1;
                }
                if !changed {
                    changed = true;
                    let object = source
                        .live_entry("canonical_claims", "canonical_claims")
                        .object_id
                        .clone();
                    source.retire(&object);
                    source.publish("messages", "messages", 2, "revised message");
                    for class in CLASSES {
                        let text = format!("late {class}\r\n");
                        source.publish(class, &format!("late-{class}"), 1, &text);
                    }
                    corpus
                        .kernel
                        .commit(intent("empty"), |_| Ok(String::new()))
                        .unwrap();
                    corpus
                        .kernel
                        .commit(intent("control"), |envelope| {
                            envelope.register_outbox_consumer("audit", now())?;
                            Ok(String::new())
                        })
                        .unwrap();
                }
            }
            BuildEvent::TargetFixed(t) => {
                target = Some(t);
                source.publish("messages", "after-target", 1, "must not appear");
                let retire_after_t = source
                    .live_entry("git_commits", "git_commits")
                    .object_id
                    .clone();
                source.retire(&retire_after_t);
            }
            BuildEvent::Verifying => verifying = true,
            BuildEvent::CatchUp(EpisodeEvent::Acknowledged { through }) => commits.push(through),
            _ => {}
        })
        .unwrap();
    candidate.revalidate().unwrap();
    let ledger: Rows = source
        .ledger
        .iter()
        .filter(|(_, row)| row.created <= target.unwrap())
        .map(|(object, row)| {
            (
                object.clone(),
                (
                    row.class.clone(),
                    row.selected_text().as_bytes().to_vec(),
                    row.invalidated.filter(|at| *at <= target.unwrap()),
                ),
            )
        })
        .collect();
    assert!(baseline_pages >= 3);
    assert!(verification_pages >= 3);
    assert_eq!(rows(candidate.path()), ledger);
    assert_eq!(
        candidate.staged().verification.checkpoint_commit_seq,
        target.unwrap()
    );
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        target
    );
    assert_eq!(commits.last().copied(), target);
    assert!(commits.windows(2).all(|pair| pair[1] == pair[0] + 1));
    assert_eq!(
        candidate.staged().verification.pending_jobs,
        ledger
            .values()
            .filter(|(class, _, dead)| class != "raw_tool_spans" && dead.is_none())
            .count() as u64
    );
    assert_eq!(candidate.staged().verification.generation_state, "building");
    assert_eq!(
        GenerationStore::open(Some(root.path()))
            .unwrap()
            .read_current()
            .unwrap(),
        CurrentProfile::Absent
    );
    let intent = control(root.path());
    assert_eq!(
        intent.staged_seed_digest.as_deref(),
        Some(candidate.staged().digest.as_str())
    );
    assert_eq!(
        intent.replacement_capture.as_ref().unwrap().snapshot,
        candidate.staged().verification.snapshot_commit_seq
    );
    assert!(intent.replacement_capture.unwrap().stage.is_some());

    let conn = Connection::open(candidate.path()).unwrap();
    for row in source
        .ledger
        .values()
        .filter(|row| row.created <= target.unwrap())
    {
        let stored: (i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT revision,span_start,span_end FROM occurrences WHERE source_object_id=?1",
                [&row.object_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            stored,
            (
                row.revision,
                row.span.map(|s| s.0 as i64),
                row.span.map(|s| s.1 as i64)
            )
        );
    }
    drop(conn);
    let baseline = source
        .live_entry("promoted_memory", "promoted_memory")
        .clone();
    source.delete_evidence(
        &baseline.evidence_id,
        kernel::ArtifactDeletionKind::Delete,
        now(),
    );
    let held = source.live_entry("messages", "late-messages").clone();
    source.delete_evidence(
        &held.evidence_id,
        kernel::ArtifactDeletionKind::Delete,
        now(),
    );
    let control = source.live_entry("messages", "after-target").clone();
    source.delete_evidence(
        &control.evidence_id,
        kernel::ArtifactDeletionKind::Delete,
        now(),
    );
    for consumer in [CONSUMER, source_fixture::CONSUMER, "audit"] {
        corpus
            .kernel
            .acknowledge_outbox(consumer, corpus.tip(), now())
            .unwrap();
    }
    source.publish_and_prune();
    let swept = corpus
        .kernel
        .run_staging_maintenance(now() + 15 * fixtures::DAY_MS)
        .unwrap();
    assert!(swept.artifact_gc.reclaimed_objects > 0);
    assert!(source.object_present(&baseline.digest));
    assert!(source.object_present(&held.digest));
    assert!(!source.object_present(&control.digest));
    candidate.revalidate().unwrap();
}

#[test]
fn expired_degraded_and_missing_source_holds_abort_owned_construction() {
    for fault in ["none", "expiry", "purge", "missing", "history", "oversized"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        record(root.path(), &gate, None, &spec(root.path()).identity);
        let mut config = spec(root.path());
        config.episode.commits.max_commits = NonZeroUsize::new(64).unwrap();
        if fault == "oversized" {
            config.episode.commits.max_rows = NonZeroUsize::MIN;
        }
        let mut baseline_ack = None;
        let mut refusal_ack = None;
        let mut baseline = None;
        let mut baseline_pending = None;
        let mut checked_refusal = false;
        let mut fired = None;
        let result = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
            .unwrap()
            .build(&budget(Duration::from_secs(30)), &mut |event| match event {
                BuildEvent::BaselineReleased => {
                    baseline = Some(rows(&database(root.path())));
                    baseline_pending = Some(pending(&database(root.path())));
                    corpus.publish("late", "late bytes");
                    if fault == "history" {
                        fired = Some("baseline-released");
                        let conn =
                            Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
                        conn.execute(
                            "DELETE FROM outbox WHERE commit_seq=?1 AND ordinal=0",
                            [corpus.tip()],
                        )
                        .unwrap();
                    }
                    if fault == "oversized" {
                        fired = Some("baseline-released");
                    }
                    baseline_ack =
                        Some(corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap());
                }
                BuildEvent::Captured { hold_id, .. } if fault == "expiry" => {
                    fired = Some("captured");
                    let conn = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
                    conn.execute(
                        "UPDATE capture_pins SET expires_at=0 WHERE capture_pin_id=?1",
                        [hold_id],
                    )
                    .unwrap();
                }
                BuildEvent::Captured { .. } if fault == "purge" => {
                    fired = Some("captured");
                    corpus
                        .kernel
                        .delete_artifact(kernel::ArtifactDeletionRequest {
                            intent: intent("purge-base"),
                            identity: kernel::ArtifactDeletionIdentity::EvidenceId(
                                "evidence-base".to_owned(),
                            ),
                            kind: kernel::ArtifactDeletionKind::Purge,
                            operator_id: Some("operator".to_owned()),
                            target_locator: Some("fixture".to_owned()),
                            reason: Some("fixture".to_owned()),
                            deleted_at: now(),
                        })
                        .unwrap();
                }
                BuildEvent::Captured { .. } if fault == "missing" => {
                    fired = Some("captured");
                    let digest = format!(
                        "{:x}",
                        <sha2::Sha256 as sha2::Digest>::digest(b"base bytes")
                    );
                    std::fs::remove_file(
                        root.path()
                            .join("kernel/artifacts/objects")
                            .join(&digest[..2])
                            .join(&digest[2..]),
                    )
                    .unwrap();
                }
                BuildEvent::CatchUp(EpisodeEvent::Acknowledged { through }) => {
                    refusal_ack = Some(through)
                }
                BuildEvent::Aborting if matches!(fault, "history" | "oversized") => {
                    checked_refusal = true;
                    assert_eq!(rows(&database(root.path())), *baseline.as_ref().unwrap());
                    assert_eq!(
                        pending(&database(root.path())),
                        *baseline_pending.as_ref().unwrap()
                    );
                    let expected = if fault == "history" {
                        control(root.path()).replacement_capture.unwrap().snapshot
                    } else {
                        corpus.tip() - 1
                    };
                    let checkpoint: i64 = Connection::open(database(root.path()))
                        .unwrap()
                        .query_row(
                            "SELECT checkpoint_commit_seq FROM projection_checkpoint",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(checkpoint, expected);
                    assert_eq!(
                        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                        Some(expected)
                    );
                }
                _ => {}
            });
        if fault == "none" {
            let candidate = result.unwrap();
            candidate.revalidate().unwrap();
            assert_eq!(candidate.staged().verification.occurrences, 2);
            assert_eq!(fired, None);
            continue;
        }
        let failure = result.err().expect("fault refuses construction");
        match (fault, &failure.error) {
            (
                "expiry",
                BuildError::Export(kernel::SourceExportError::Hold(
                    kernel::SourceHoldError::Invalid(kernel::SourceHoldInvalidity::Expired),
                )),
            )
            | (
                "purge",
                BuildError::Export(kernel::SourceExportError::Hold(
                    kernel::SourceHoldError::Invalid(kernel::SourceHoldInvalidity::PurgeDegraded),
                )),
            )
            | ("missing", BuildError::Export(kernel::SourceExportError::BytesUnavailable { .. })) =>
            {
                assert_eq!(fired, Some("captured"))
            }
            (
                "history",
                BuildError::Blocked(daemon::search_catchup::Blocked::Read(
                    kernel::CommitReadError::MissingHistory { .. },
                )),
            )
            | (
                "oversized",
                BuildError::Blocked(daemon::search_catchup::Blocked::OversizedCommit { .. }),
            ) => assert_eq!(fired, Some("baseline-released")),
            _ => panic!("wrong refusal for {fault}: {failure:?}"),
        }
        assert!(failure.cleanup_error.is_none(), "{fault}: {failure:?}");
        assert!(!database(root.path()).exists());
        assert!(control(root.path()).replacement_capture.is_none());
        assert!(control(root.path()).staged_seed_digest.is_none());
        if matches!(fault, "history" | "oversized") {
            assert!(checked_refusal);
            assert!(baseline_ack.is_some());
            assert!(refusal_ack.is_some());
            assert!(refusal_ack.unwrap() < corpus.tip());
        }
    }
}

#[test]
fn cleanup_refusal_keeps_the_hold_and_exclusive_family_until_retried() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    let config = spec(root.path());
    record(root.path(), &gate, None, &config.identity);
    let mut failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config.clone())
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| {
            if event == BuildEvent::BaselineReleased {
                gate.close();
            }
        })
        .err()
        .unwrap();
    assert!(failure.cleanup_error.is_some());
    assert!(database(root.path()).exists());
    assert!(control(root.path()).replacement_capture.is_some());
    assert!(daemon::search_projection::SearchProjection::open(&workspace(root.path())).is_err());
    gate.install(support::projection_gate::passing_evaluator(
        &config.identity,
        0,
        &daemon::projection_gates::ProjectionHook::ALL,
    ));
    failure.cleanup(&budget(Duration::from_secs(30))).unwrap();
    assert!(!database(root.path()).exists());
    assert!(control(root.path()).replacement_capture.is_none());
}

#[test]
fn stage_reply_reconciliation_reuses_closed_bytes_and_the_original_hold() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    let config = spec(root.path());
    record(root.path(), &gate, None, &config.identity);
    let failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config.clone())
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| {
            if event == BuildEvent::Staged {
                gate.close();
            }
        })
        .err()
        .unwrap();
    assert!(matches!(
        failure.cleanup_error,
        Some(BuildError::Intent(
            daemon::projection_lifecycle::IntentRefusal::Denied(_)
        ))
    ));
    let interrupted = control(root.path());
    assert!(
        interrupted
            .replacement_capture
            .as_ref()
            .unwrap()
            .stage
            .is_some()
    );
    assert!(interrupted.staged_seed_digest.is_none());
    assert!(daemon::search_projection::SearchProjection::open(&workspace(root.path())).is_err());
    gate.install(support::projection_gate::passing_evaluator(
        &config.identity,
        0,
        &daemon::projection_gates::ProjectionHook::ALL,
    ));
    let closed_bytes = std::fs::read(database(root.path())).unwrap();
    let failure = corpus
        .kernel
        .with_readers_held_for_test(|| {
            failure.retry(&budget(Duration::from_millis(40)), &mut |event| {
                assert_eq!(event, BuildEvent::Aborting);
            })
        })
        .err()
        .unwrap();
    assert!(matches!(
        failure.error,
        BuildError::Kernel(kernel::KernelError::Deadline)
    ));
    assert!(matches!(failure.cleanup_error, Some(BuildError::Expired)));
    assert_eq!(control(root.path()), interrupted);
    assert_eq!(std::fs::read(database(root.path())).unwrap(), closed_bytes);
    let candidate = failure
        .retry(&budget(Duration::from_secs(30)), &mut |_| {})
        .unwrap();
    candidate.revalidate().unwrap();
    let completed = control(root.path());
    assert_eq!(
        completed.replacement_capture,
        interrupted.replacement_capture
    );
    assert_eq!(completed.recovery_target, interrupted.recovery_target);
    assert_eq!(completed.episodes.deadline, interrupted.episodes.deadline);
    assert_eq!(completed.episodes.consumed, 2);
    drop(candidate);
    assert!(ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config).is_err());
}

#[test]
fn a_changed_closed_file_is_removed_without_pinning_or_releasing_a_live_family() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| {
            if event == BuildEvent::Closed {
                Connection::open(database(root.path()))
                    .unwrap()
                    .execute("UPDATE payloads SET bytes=zeroblob(byte_length)", [])
                    .unwrap();
            }
        })
        .err()
        .unwrap();
    assert!(failure.cleanup_error.is_none(), "{failure:?}");
    assert!(!database(root.path()).exists());
    assert!(control(root.path()).replacement_capture.is_none());
    assert!(control(root.path()).staged_seed_digest.is_none());
}

#[test]
fn a_registration_receipt_does_not_recreate_a_deregistered_consumer() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let first = budget(Duration::from_secs(30));
    let failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
        .unwrap()
        .build(&first, &mut |event| {
            if matches!(event, BuildEvent::Captured { .. }) {
                first.cancel();
            }
        })
        .err()
        .unwrap();
    corpus
        .kernel
        .acknowledge_outbox(CONSUMER, corpus.tip(), now())
        .unwrap();
    corpus
        .kernel
        .commit(intent("deregister"), |envelope| {
            envelope.deregister_outbox_consumer(CONSUMER, now())?;
            Ok(String::new())
        })
        .unwrap();
    let failure = failure
        .retry(&budget(Duration::from_secs(30)), &mut |_| {})
        .err()
        .unwrap();
    assert!(
        matches!(
            failure.error,
            BuildError::Hold(kernel::SourceHoldError::UnknownConsumer)
        ),
        "{failure:?}"
    );
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        None
    );
    assert!(control(root.path()).replacement_capture.is_none());
}

#[test]
fn changed_bytes_same_count_replacement_and_noncurrent_jobs_fail_construction() {
    for mutation in [
        "none",
        "bytes",
        "omission",
        "jobs",
        "raw_jobs",
        "extra_generation",
        "vector",
        "future_checkpoint",
        "extra_tombstone",
        "metadata",
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("one", "first payload");
        corpus.publish("two", "other payload");
        corpus.publish_class("tool", "tool payload", "raw_tool_spans");
        let gate = open_gate();
        record(root.path(), &gate, None, &spec(root.path()).identity);
        let mut fired = false;
        let result = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path())).unwrap()
            .build(&budget(Duration::from_secs(30)), &mut |event| {
                if event == BuildEvent::Verifying && mutation != "none" {
                    fired = true;
                    let conn = Connection::open(database(root.path())).unwrap();
                    match mutation {
                        "bytes" => { conn.execute("UPDATE payloads SET bytes=zeroblob(byte_length)", []).unwrap(); }
                        "omission" => {
                            use sha2::{Digest, Sha256};
                            let fields = source_fixture::identity("messages", "unexpected");
                            let fields: Vec<_> = fields.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect();
                            let encoded = kernel::source_identity::encode_preserving_span(&kernel::source_identity::Occurrence {
                                class: "messages", identity: &fields, revision: "1", representation: "text", span: None,
                            }).unwrap();
                            let old: String = conn.query_row("SELECT occurrence_id FROM occurrences WHERE class='messages' ORDER BY occurrence_id LIMIT 1", [], |row| row.get(0)).unwrap();
                            let mut hash = Sha256::new();
                            hash.update(encoded.occurrence_id.as_bytes());
                            hash.update([0x1f]);
                            hash.update(fixtures::GENERATION.as_bytes());
                            conn.execute_batch("BEGIN; PRAGMA defer_foreign_keys=ON;").unwrap();
                            conn.execute("UPDATE occurrences SET occurrence_id=?1,tuple=?2,lineage_id=?3,source_object_id='unexpected' WHERE occurrence_id=?4",
                                rusqlite::params![encoded.occurrence_id, encoded.tuple, encoded.lineage_id, old]).unwrap();
                            conn.execute("UPDATE embedding_jobs SET occurrence_id=?1,job_id=?2 WHERE occurrence_id=?3",
                                rusqlite::params![encoded.occurrence_id, format!("{:x}", hash.finalize()), old]).unwrap();
                            conn.execute_batch("COMMIT;").unwrap();
                            let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
                            assert_eq!(integrity, "ok");
                            assert!(conn.prepare("PRAGMA foreign_key_check").unwrap().query([]).unwrap().next().unwrap().is_none());
                            assert_eq!(rows(&database(root.path())).len(), 3);
                        }
                        "jobs" => { conn.execute("UPDATE embedding_jobs SET state='obsolete'", []).unwrap(); }
                        "raw_jobs" => { conn.execute("INSERT INTO embedding_jobs(job_id,occurrence_id,generation_id,state,created_at,updated_at) SELECT 'raw-job',occurrence_id,?1,'pending',1,1 FROM occurrences WHERE class='raw_tool_spans'", [fixtures::GENERATION]).unwrap(); }
                        "extra_generation" => { conn.execute("INSERT INTO vector_generations SELECT 'extra',embedding_model,tokenizer_fingerprint,vector_dimension,generation_epoch,state,created_at,updated_at FROM vector_generations", []).unwrap(); }
                        "vector" => { conn.execute("INSERT INTO occurrence_vectors SELECT occurrence_id,?1,zeroblob(32),8,0,0,1 FROM occurrences WHERE class='messages' LIMIT 1", [fixtures::GENERATION]).unwrap(); }
                        "future_checkpoint" => { conn.execute("UPDATE projection_checkpoint SET checkpoint_commit_seq=checkpoint_commit_seq+1", []).unwrap(); }
                        "extra_tombstone" => {
                            conn.execute("INSERT INTO occurrence_tombstones SELECT occurrence_id,(SELECT checkpoint_commit_seq+1 FROM projection_checkpoint),'retired',1 FROM occurrences WHERE class='messages' LIMIT 1", []).unwrap();
                            conn.execute("UPDATE embedding_jobs SET state='obsolete' WHERE occurrence_id IN (SELECT occurrence_id FROM occurrence_tombstones)", []).unwrap();
                        }
                        "metadata" => { conn.execute("UPDATE occurrences SET domain_id='different-domain'", []).unwrap(); }
                        _ => unreachable!(),
                    }
                }
            });
        if mutation == "none" {
            let candidate = result.unwrap();
            candidate.revalidate().unwrap();
            assert!(!fired);
            continue;
        }
        let failure = result.err().expect("corrupt construction refused");
        assert!(fired);
        match (&failure.error, mutation) {
            (
                BuildError::Projection(
                    daemon::search_projection::SearchProjectionError::Projection(
                        retrieval::ProjectionError::OccurrenceCollision { .. },
                    ),
                ),
                "metadata",
            ) => {}
            (
                BuildError::Projection(
                    daemon::search_projection::SearchProjectionError::Projection(
                        retrieval::ProjectionError::CorruptRow,
                    ),
                ),
                _,
            ) => {}
            _ => panic!("{mutation}: {failure:?}"),
        }
        assert!(failure.cleanup_error.is_none(), "{failure:?}");
        assert!(!database(root.path()).exists());
        assert!(control(root.path()).replacement_capture.is_none());
        assert!(control(root.path()).staged_seed_digest.is_none());
    }
}

#[test]
fn interrupted_export_restarts_at_fresh_s_without_renewing_allowance() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    for key in ["one", "two", "three", "four", "five"] {
        corpus.publish(key, key);
    }
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let first_budget = budget(Duration::from_secs(30));
    let mut failure =
        ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
            .unwrap()
            .build(&first_budget, &mut |event| {
                if matches!(event, BuildEvent::Exported { .. }) {
                    first_budget.cancel();
                }
            })
            .err()
            .unwrap();
    assert!(matches!(failure.cleanup_error, Some(BuildError::Expired)));
    failure.cleanup(&budget(Duration::from_secs(30))).unwrap();
    assert_eq!(control(root.path()).episodes.consumed, 1);
    assert!(control(root.path()).recovery_target.is_none());
    corpus.publish("new", "fresh S includes this");
    let expected_s = corpus.tip();
    let candidate = failure
        .retry(&budget(Duration::from_secs(30)), &mut |_| {})
        .unwrap();
    assert_eq!(
        candidate.staged().verification.snapshot_commit_seq,
        expected_s
    );
    assert_eq!(control(root.path()).episodes.consumed, 2);
    assert_eq!(candidate.staged().verification.occurrences, 6);
}

#[test]
fn a_retry_with_s_beyond_the_recorded_target_blocks_instead_of_moving_t() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let first = budget(Duration::from_secs(30));
    let failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
        .unwrap()
        .build(&first, &mut |event| {
            if matches!(event, BuildEvent::TargetFixed(_)) {
                first.cancel();
            }
        })
        .err()
        .unwrap();
    let original = control(root.path());
    let acknowledged = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
    corpus.publish("later", "beyond fixed target");
    let failure = failure
        .retry(&budget(Duration::from_secs(30)), &mut |_| {})
        .err()
        .unwrap();
    assert!(
        matches!(failure.error, BuildError::SnapshotBeyondTarget { snapshot, target } if snapshot > target)
    );
    assert!(failure.cleanup_error.is_none());
    let refused = control(root.path());
    assert_eq!(refused.recovery_target, original.recovery_target);
    assert_eq!(refused.episodes.deadline, original.episodes.deadline);
    assert_eq!(refused.episodes.consumed, 2);
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        acknowledged
    );
}

#[test]
fn exact_inventory_capacity_handles_empty_catchup_and_invalidation_overlays() {
    for case in ["exact", "overlay", "zero-charge", "too-small", "oversized"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        let object = corpus.publish("base", "12345678");
        let gate = open_gate();
        record(root.path(), &gate, None, &spec(root.path()).identity);
        let mut config = spec(root.path());
        config.episode.batch.persist.max_records =
            NonZeroUsize::new(if case == "zero-charge" { 2 } else { 1 }).unwrap();
        config.episode.source_page.max_rows = NonZeroUsize::MIN;
        config.episode.batch.max_source_bytes =
            NonZeroUsize::new(if case == "too-small" { 7 } else { 8 }).unwrap();
        if case == "oversized" {
            config.episode.source_page.max_decoded_bytes = NonZeroU64::new(7).unwrap();
        }
        let mut retired = false;
        let mut catchup_exported = false;
        let result = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
            .unwrap()
            .build(&budget(Duration::from_secs(30)), &mut |event| {
                if event == BuildEvent::BaselineReleased
                    && matches!(case, "overlay" | "zero-charge")
                {
                    if case == "zero-charge" {
                        corpus.publish("empty", "");
                    }
                    corpus.retire(&object);
                    retired = true;
                }
                if matches!(
                    event,
                    BuildEvent::Exported {
                        window: ExportWindow::CatchUp { .. },
                        ..
                    }
                ) {
                    catchup_exported = true;
                }
            });
        if case == "too-small" {
            assert!(matches!(
                result.err().unwrap().error,
                BuildError::InventoryBound
            ));
        } else if case == "oversized" {
            let failure = result.err().unwrap();
            assert!(matches!(
                failure.error,
                BuildError::Export(kernel::SourceExportError::OversizedRow {
                    bound: kernel::PageBound::Decoded,
                    bytes: 8,
                    ..
                })
            ));
        } else {
            let candidate = result.unwrap();
            candidate.revalidate().unwrap();
            assert_eq!(
                candidate.staged().verification.occurrences,
                if case == "zero-charge" { 2 } else { 1 }
            );
            assert_eq!(catchup_exported, retired);
            assert_eq!(
                candidate.staged().verification.tombstones,
                u64::from(retired)
            );
            if case == "exact" {
                assert_eq!(
                    candidate.staged().verification.snapshot_commit_seq,
                    candidate.staged().verification.checkpoint_commit_seq
                );
            }
        }
    }
}

#[test]
fn invalid_attempt_budgets_do_not_consume_the_lifecycle_allowance() {
    for invalid in [
        kernel::applicability::EvalBudget::unbounded(),
        budget(Duration::from_secs(120)),
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        let gate = open_gate();
        record(root.path(), &gate, None, &spec(root.path()).identity);
        let before = control(root.path());
        let tip = corpus.tip();
        let failure =
            ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
                .unwrap()
                .build(&invalid, &mut |event| {
                    assert_eq!(event, BuildEvent::Aborting)
                })
                .err()
                .unwrap();
        assert!(matches!(
            failure.error,
            BuildError::Invalid("budget must end within the original episode deadline")
        ));
        assert_eq!(control(root.path()), before);
        assert_eq!(corpus.tip(), tip);
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            None
        );
    }
}

#[test]
fn cancellation_after_pinning_preserves_the_candidate_but_refuses_selection_eligibility() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let operation = budget(Duration::from_secs(30));
    let mut pinned = false;
    let candidate = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
        .unwrap()
        .build(&operation, &mut |event| {
            if event == BuildEvent::Pinned {
                pinned = true;
                operation.cancel();
            }
        })
        .unwrap();
    assert!(pinned);
    assert_eq!(
        control(root.path()).staged_seed_digest.as_deref(),
        Some(candidate.staged().digest.as_str())
    );
    assert_eq!(control(root.path()).episodes.consumed, 1);
    assert!(matches!(candidate.revalidate(), Err(BuildError::Expired)));
}

#[test]
fn one_complete_commit_can_span_multiple_source_pages() {
    let root = tempfile::tempdir().unwrap();
    let mut source = source_fixture::Fixture::open();
    source.publish("messages", "base", 1, "base");
    let kernel = Arc::clone(&source.store);
    let incarnation: String = source
        .inspect()
        .query_row(
            "SELECT database_incarnation_id FROM kernel_format_marker",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let gate = open_gate();
    let mut config = spec_with_identity(identity(&incarnation));
    config.episode.source_page.max_rows = NonZeroUsize::MIN;
    config.episode.commits.max_commits = NonZeroUsize::new(64).unwrap();
    record(root.path(), &gate, None, &config.identity);
    let mut ledger = source_rows(&source);
    let mut applied = Vec::new();
    let candidate = ReplacementBuilder::open(root.path(), &kernel, &gate, config)
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| match event {
            BuildEvent::BaselineReleased => {
                let entries: Vec<_> = ["one", "two", "three"]
                    .into_iter()
                    .map(|key| (key, source.retain(key, key)))
                    .collect();
                kernel
                    .commit(intent("multi-source-commit"), |envelope| {
                        for (key, (evidence, digest)) in &entries {
                            let fields = source_fixture::identity("messages", key);
                            let fields: Vec<_> = fields
                                .iter()
                                .map(|(key, value)| (key.as_str(), value.as_str()))
                                .collect();
                            let row = envelope
                                .publish_source_descriptor(&kernel::SourceDescriptorRequest {
                                    source_policy: kernel::SourceDescriptorPolicy::Native,
                                    occurrence: kernel::source_identity::Occurrence {
                                        class: "messages",
                                        identity: &fields,
                                        revision: "1",
                                        representation: "text",
                                        span: None,
                                    },
                                    domain_id: "domain",
                                    scope_id: None,
                                    evidence_id: evidence,
                                    artifact_digest: digest,
                                    buffer: key,
                                    sensitivity: kernel::Sensitivity::Normal,
                                    observed_at: now(),
                                })
                                .unwrap();
                            ledger.insert(
                                row.object_id,
                                ("messages".to_owned(), key.as_bytes().to_vec(), None),
                            );
                        }
                        Ok(String::new())
                    })
                    .unwrap();
            }
            BuildEvent::CatchUp(EpisodeEvent::LocalStaged { through }) => applied.push(through),
            _ => {}
        })
        .unwrap();
    assert_eq!(
        applied,
        [candidate.staged().verification.checkpoint_commit_seq]
    );
    assert_eq!(rows(candidate.path()), ledger);
    candidate.revalidate().unwrap();
}

#[test]
fn cancellation_inside_catchup_rolls_back_without_quarantine_or_acknowledgement() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    let mut config = spec(root.path());
    config.episode.commits.max_commits = NonZeroUsize::new(64).unwrap();
    record(root.path(), &gate, None, &config.identity);
    let allowance = budget(Duration::from_secs(30));
    let mut fired = false;
    let mut baseline = None;
    let mut checked = false;
    let mut failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
        .unwrap()
        .build(&allowance, &mut |event| match event {
            BuildEvent::BaselineReleased => {
                baseline = Some(rows(&database(root.path())));
                corpus.publish("late", "late bytes");
            }
            BuildEvent::CatchUp(EpisodeEvent::LocalStaged { .. }) => {
                fired = true;
                allowance.cancel();
            }
            BuildEvent::Aborting => {
                checked = true;
                let snapshot = control(root.path()).replacement_capture.unwrap().snapshot;
                assert_eq!(rows(&database(root.path())), *baseline.as_ref().unwrap());
                assert_eq!(
                    corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                    Some(snapshot)
                );
            }
            _ => {}
        })
        .err()
        .unwrap();
    assert!(fired && checked);
    assert!(matches!(
        failure.error,
        BuildError::Blocked(daemon::search_catchup::Blocked::Cancelled)
    ));
    assert!(matches!(failure.cleanup_error, Some(BuildError::Expired)));
    assert!(database(root.path()).exists());
    failure.cleanup(&budget(Duration::from_secs(30))).unwrap();
    assert!(!database(root.path()).exists());
}

#[test]
fn certificates_reject_cancelled_budgets_and_replaced_grants() {
    for change in ["cancel", "same-identity", "different-identity", "closed"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        let config = spec(root.path());
        record(root.path(), &gate, None, &config.identity);
        let allowance = budget(Duration::from_secs(30));
        let candidate =
            ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config.clone())
                .unwrap()
                .build(&allowance, &mut |_| {})
                .unwrap();
        candidate.revalidate().unwrap();
        match change {
            "cancel" => allowance.cancel(),
            "closed" => gate.close(),
            _ => {
                let mut identity = config.identity;
                if change == "different-identity" {
                    identity.generation_epoch += 1;
                }
                gate.install(support::projection_gate::passing_evaluator(
                    &identity,
                    0,
                    &daemon::projection_gates::ProjectionHook::ALL,
                ));
            }
        }
        let error = candidate.revalidate().unwrap_err();
        assert!(
            matches!(
                error,
                BuildError::Expired
                    | BuildError::Intent(daemon::projection_lifecycle::IntentRefusal::Denied(_))
            ),
            "{change}: {error:?}"
        );
    }
}

#[test]
fn revalidation_does_not_renew_the_successful_attempt_deadline() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    corpus.publish("base", "base bytes");
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let allowance = budget(Duration::from_secs(5));
    let candidate = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
        .unwrap()
        .build(&allowance, &mut |_| {})
        .unwrap();
    candidate.revalidate().unwrap();
    std::thread::sleep(
        allowance
            .deadline()
            .unwrap()
            .saturating_duration_since(std::time::Instant::now()),
    );
    assert!(matches!(candidate.revalidate(), Err(BuildError::Expired)));
}

#[test]
fn construction_caps_are_checked_against_manifest_limits_before_registration() {
    for limit in [
        "export_page_rows",
        "local_transaction_rows",
        "local_transaction_bytes",
        "pending_bytes",
        "capture_disk_bytes",
        "capture_hold_expiry_ms",
        "B_recovery_ms",
        "retry_attempts",
        "missing",
    ] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        let gate = open_gate();
        let config = spec(root.path());
        record(root.path(), &gate, None, &config.identity);
        let mut evaluator = support::projection_gate::passing_evaluator(
            &config.identity,
            0,
            &daemon::projection_gates::ProjectionHook::ALL,
        );
        if limit == "missing" {
            evaluator.manifest.limits.remove("export_page_rows");
        } else {
            evaluator.manifest.limits.insert(limit.to_owned(), 1);
        }
        gate.install(evaluator);
        let error = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
            .err()
            .expect("unapproved cap refused");
        match error {
            BuildError::Denied(daemon::projection_gates::Denial::LimitExceeded {
                limit: found,
                observed,
                max: 1,
            }) => {
                assert_eq!(found, limit);
                assert!(observed > 1);
            }
            BuildError::Denied(daemon::projection_gates::Denial::Failed(
                daemon::projection_gates::Gate::Resource,
                message,
            )) if limit == "missing" => assert!(message.contains("export_page_rows")),
            other => panic!("{limit}: {other:?}"),
        }
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            None
        );
        assert!(!database(root.path()).exists());
    }
}

#[test]
fn encoded_window_admission_is_distinct_from_page_capacity() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let gate = open_gate();
    let mut config = spec(root.path());
    config.episode.source_page.max_encoded_bytes = NonZeroU64::new(16).unwrap();
    config.episode.commits.max_payload_bytes = NonZeroU64::new(128).unwrap();
    config.episode.max_source_encoded_bytes = NonZeroU64::new(129).unwrap();
    record(root.path(), &gate, None, &config.identity);
    let mut evaluator = support::projection_gate::passing_evaluator(
        &config.identity,
        0,
        &daemon::projection_gates::ProjectionHook::ALL,
    );
    evaluator
        .manifest
        .limits
        .insert("catchup_batch_encoded_bytes".to_owned(), 128);
    gate.install(evaluator);
    let error = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config.clone())
        .err()
        .expect("window limit rejected");
    assert!(
        matches!(error, BuildError::Denied(daemon::projection_gates::Denial::LimitExceeded { limit, observed: 129, max: 128 }) if limit == "catchup_batch_encoded_bytes")
    );
    config.episode.max_source_encoded_bytes = NonZeroU64::new(128).unwrap();
    let builder = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config).unwrap();
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        None
    );
    drop(builder);
}

#[test]
fn a_restore_cannot_rebind_the_target_even_when_it_reuses_the_sequence() {
    use std::os::unix::fs::PermissionsExt;
    for before_target in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        std::fs::set_permissions(backup.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        record(root.path(), &gate, None, &spec(root.path()).identity);
        let mut backup_path = None;
        let mut restored = false;
        let mut acknowledged = None;
        let failure =
            ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
                .unwrap()
                .build(&budget(Duration::from_secs(30)), &mut |event| match event {
                    BuildEvent::BaselineReleased => {
                        let saved = corpus
                            .kernel
                            .backup(kernel::BackupRequest {
                                destination_directory: backup.path().to_path_buf(),
                                deadline: std::time::Instant::now() + Duration::from_secs(10),
                                capture_pin_expires_at: None,
                            })
                            .unwrap();
                        backup_path = Some(saved.destination_path);
                        acknowledged = corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap();
                        corpus
                            .kernel
                            .commit(intent("displaced"), |_| Ok(String::new()))
                            .unwrap();
                        if before_target {
                            corpus
                                .kernel
                                .restore(backup_path.as_ref().unwrap())
                                .unwrap();
                            restored = true;
                        }
                    }
                    BuildEvent::TargetFixed(target) => {
                        assert!(!before_target, "S and T must belong to the same history");
                        corpus
                            .kernel
                            .restore(backup_path.as_ref().unwrap())
                            .unwrap();
                        corpus
                            .kernel
                            .commit(intent("replacement-history"), |_| Ok(String::new()))
                            .unwrap();
                        assert_eq!(corpus.tip(), target);
                        restored = true;
                    }
                    _ => {}
                })
                .err()
                .unwrap();
        assert!(restored);
        assert!(matches!(
            failure.error,
            BuildError::Blocked(daemon::search_catchup::Blocked::Read(
                kernel::CommitReadError::IncarnationMismatch
            ))
        ));
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            acknowledged
        );
        assert!(failure.cleanup_error.is_none(), "{failure:?}");
    }
}

#[test]
fn exhausted_budget_refuses_before_kernel_readers_and_defers_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let gate = open_gate();
    record(root.path(), &gate, None, &spec(root.path()).identity);
    let builder =
        ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path())).unwrap();
    let allowance = budget(Duration::from_secs(10));
    allowance.cancel();
    let held = std::sync::Barrier::new(2);
    let mut failure = std::thread::scope(|scope| {
        let readers = scope.spawn(|| {
            corpus
                .kernel
                .hold_readers_for_test(&held, Duration::from_secs(2))
        });
        held.wait();
        let failure = builder.build(&allowance, &mut |_| {}).err().unwrap();
        assert!(!readers.is_finished(), "refusal does not wait for readers");
        failure
    });
    assert!(matches!(failure.error, BuildError::Expired));
    assert!(matches!(failure.cleanup_error, Some(BuildError::Expired)));
    failure.cleanup(&budget(Duration::from_secs(30))).unwrap();
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        None
    );
}

#[test]
fn kernel_writer_wait_ends_at_the_deadline_and_a_released_holder_allows_construction() {
    for release_early in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("base", "base bytes");
        let gate = open_gate();
        record(root.path(), &gate, None, &spec(root.path()).identity);
        let builder =
            ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
                .unwrap();
        let allowance = budget(Duration::from_secs(5));
        let deadline = allowance.deadline().unwrap();
        let (request_tx, request_rx) = mpsc::channel();
        let (held_tx, held_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let kernel = &corpus.kernel;
        let result = std::thread::scope(|scope| {
            let writer = scope.spawn(move || {
                request_rx.recv_timeout(Duration::from_secs(30)).unwrap();
                kernel
                    .commit(intent("held-writer"), |_| {
                        held_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(30)).unwrap();
                        Ok(String::new())
                    })
                    .unwrap();
            });
            let worker = scope.spawn(move || {
                let result = builder.build(&allowance, &mut |event| {
                    if matches!(
                        event,
                        BuildEvent::CatchUp(EpisodeEvent::HoldExtensionRequested { .. })
                    ) {
                        request_tx.send(()).unwrap();
                        held_rx.recv_timeout(Duration::from_secs(30)).unwrap();
                        ready_tx.send(()).unwrap();
                    }
                });
                result_tx
                    .send(result)
                    .unwrap_or_else(|_| panic!("result receiver disappeared"));
            });
            ready_rx.recv_timeout(Duration::from_secs(30)).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "writer is held before deadline"
            );
            if release_early {
                release_tx.send(()).unwrap();
            }
            let result = result_rx.recv_timeout(
                deadline.saturating_duration_since(std::time::Instant::now())
                    + Duration::from_secs(1),
            );
            let holder_still_owned = !writer.is_finished();
            if !release_early {
                release_tx.send(()).unwrap();
            }
            writer.join().unwrap();
            worker.join().unwrap();
            if !release_early {
                assert!(holder_still_owned);
            }
            result.expect("construction returns by its deadline while the writer remains held")
        });
        if release_early {
            result.unwrap().revalidate().unwrap();
            continue;
        }
        let mut failure = result.err().unwrap();
        assert!(matches!(
            failure.error,
            BuildError::Blocked(daemon::search_catchup::Blocked::Cancelled)
                | BuildError::CatchUp(daemon::search_catchup::CatchUpError::Kernel(
                    kernel::KernelError::Deadline
                ))
        ));
        assert!(matches!(failure.cleanup_error, Some(BuildError::Expired)));
        failure.cleanup(&budget(Duration::from_secs(30))).unwrap();
    }
}

#[test]
fn admission_bounds_and_fixed_target_refuse_without_a_candidate() {
    for case in ["closed", "bytes", "pages", "target", "future-target"] {
        let root = tempfile::tempdir().unwrap();
        let corpus = Corpus::open(root.path());
        corpus.seed();
        corpus.publish("one", "12345678");
        corpus.publish("two", "abcdefgh");
        let gate = open_gate();
        record(
            root.path(),
            &gate,
            match case {
                "target" => Some(corpus.tip()),
                "future-target" => Some(corpus.tip() + 1),
                _ => None,
            },
            &spec(root.path()).identity,
        );
        let mut config = spec(root.path());
        if case == "bytes" {
            config.episode.batch.max_source_bytes = NonZeroUsize::new(10).unwrap();
        }
        if case == "pages" {
            config.episode.source_page.max_rows = NonZeroUsize::MIN;
            config.episode.max_source_pages = NonZeroUsize::MIN;
        }
        if case == "closed" {
            gate.close();
            assert!(ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config).is_err());
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                None
            );
            continue;
        }
        let failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, config)
            .unwrap()
            .build(&budget(Duration::from_secs(30)), &mut |_| {})
            .err()
            .unwrap();
        if case == "target" {
            assert!(matches!(
                failure.error,
                BuildError::SnapshotBeyondTarget { .. }
            ));
        }
        if case == "future-target" {
            assert!(matches!(
                failure.error,
                BuildError::Blocked(daemon::search_catchup::Blocked::Read(
                    kernel::CommitReadError::TargetBeyondTip
                ))
            ));
            assert_eq!(
                corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                None
            );
        }
        assert!(failure.cleanup_error.is_none(), "{failure:?}");
        assert!(control(root.path()).staged_seed_digest.is_none());
        assert!(!database(root.path()).exists());
    }
}

fn pending(path: &Path) -> Vec<String> {
    Connection::open(path).unwrap().prepare(
        "SELECT o.source_object_id FROM embedding_jobs j JOIN occurrences o USING(occurrence_id) WHERE j.state='pending' ORDER BY o.source_object_id",
    ).unwrap().query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
}

fn source_rows(source: &source_fixture::Fixture) -> Rows {
    source
        .ledger
        .iter()
        .map(|(id, row)| {
            (
                id.clone(),
                (
                    row.class.clone(),
                    row.selected_text().as_bytes().to_vec(),
                    row.invalidated,
                ),
            )
        })
        .collect()
}

fn expected_pending(rows: &Rows) -> Vec<String> {
    rows.iter()
        .filter(|(_, (class, _, dead))| class != "raw_tool_spans" && dead.is_none())
        .map(|(id, _)| id.clone())
        .collect()
}

#[test]
#[ignore = "parent launches and kills this child at a named boundary"]
fn replacement_child() {
    let root = std::path::PathBuf::from(std::env::var("REPLACEMENT_CHILD_ROOT").unwrap());
    let cut = std::env::var("REPLACEMENT_CHILD_CUT").unwrap();
    if cut.starts_with("select-") || cut.starts_with("active-") {
        selection_child(&root, &cut);
        return;
    }
    let kernel_root = tempfile::Builder::new()
        .prefix("kernel")
        .rand_bytes(0)
        .tempdir_in(&root)
        .unwrap();
    let corpus = Corpus::open(&root);
    corpus.seed();
    let mut source = source_fixture::Fixture {
        root: kernel_root,
        store: Arc::clone(&corpus.kernel),
        ledger: BTreeMap::new(),
    };
    for class in CLASSES {
        source.publish(class, class, 1, &format!("{class}\r\nλ"));
    }
    let before = source_rows(&source);
    let gate = open_gate();
    let mut config = spec(&root);
    config.episode.commits.max_commits = NonZeroUsize::new(64).unwrap();
    config.episode.source_page.max_rows = NonZeroUsize::new(64).unwrap();
    record(&root, &gate, None, &config.identity);
    let mut snapshot = 0;
    let mut target = 0;
    let _ = ReplacementBuilder::open(&root, &corpus.kernel, &gate, config).unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| {
            match event {
                BuildEvent::CaptureHeld { snapshot: s, ref hold_id } => {
                    println!("CAPTURE {}", serde_json::json!({"s": s, "hold": hold_id, "epoch": corpus.kernel.lease_epoch()}));
                }
                BuildEvent::Captured { snapshot: s, .. } => snapshot = s,
                BuildEvent::BaselineReleased => {
                    if cut == "down-restore" {
                        use std::os::unix::fs::PermissionsExt;
                        let directory = root.join("backup");
                        std::fs::create_dir(&directory).unwrap();
                        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
                        let backup = corpus.kernel.backup(kernel::BackupRequest {
                            destination_directory: directory, deadline: std::time::Instant::now() + Duration::from_secs(10), capture_pin_expires_at: None,
                        }).unwrap();
                        println!("BACKUP {}", serde_json::to_string(&backup.destination_path).unwrap());
                    }
                    source.publish("messages", "messages", 2, "revised\r\n");
                    let retired = source.live_entry("canonical_claims", "canonical_claims").object_id.clone();
                    source.retire(&retired);
                    source.publish("promoted_memory", "late", 1, "created after S");
                    let late = source.live_entry("promoted_memory", "late").object_id.clone();
                    source.retire(&late);
                    corpus.kernel.commit(intent("empty-cut"), |_| Ok(String::new())).unwrap();
                    let pending = corpus.kernel.pending_outbox(1000).unwrap();
                    corpus.kernel.mark_outbox_published_through(pending.last().unwrap().outbox_position, now()).unwrap();
                }
                BuildEvent::TargetFixed(t) => {
                    target = t;
                    let after = source_rows(&source);
                    println!("WITNESS {}", serde_json::json!({
                        "s": snapshot, "t": target, "before": before, "after": after,
                        "pending_before": expected_pending(&before), "pending_after": expected_pending(&after),
                    }));
                }
                _ => {}
            }
            let fired = match (&*cut, event) {
                ("before-commit", BuildEvent::CatchUp(EpisodeEvent::LocalStaged { through }))
                | ("after-commit", BuildEvent::CatchUp(EpisodeEvent::LocalReleased { through }))
                | ("before-ack", BuildEvent::CatchUp(EpisodeEvent::AcknowledgementRequested { through }))
                | ("after-ack", BuildEvent::CatchUp(EpisodeEvent::Acknowledged { through })) => through == target && target > 0,
                ("capture-before-record", BuildEvent::CaptureHeld { .. })
                | ("before-stage", BuildEvent::StagePrepared)
                | ("after-stage", BuildEvent::Staged)
                | ("down-restore", BuildEvent::Staged) => true,
                _ => false,
            };
            if fired {
                println!("BARRIER {cut}");
                std::io::stdout().flush().unwrap();
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                panic!("parent must kill child before resuming it");
            }
        });
    panic!("construction did not fire {cut}");
}

struct ChildOwner(Child);

impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn process_cuts_preserve_the_ledger_and_rebuild_through_the_original_target() {
    for cut in ["before-commit", "after-commit", "before-ack", "after-ack"] {
        let root = tempfile::tempdir().unwrap();
        let witness = kill_child_at(root.path(), cut)["WITNESS"].clone();
        let committed = cut != "before-commit";
        let checkpoint = witness[if committed { "t" } else { "s" }].as_i64().unwrap();
        let ack = witness[if cut == "after-ack" { "t" } else { "s" }]
            .as_i64()
            .unwrap();
        for _ in 0..2 {
            let kernel = kernel::KernelStore::open(root.path().join("kernel")).unwrap();
            let projection =
                daemon::search_projection::SearchProjection::open(&workspace(root.path())).unwrap();
            assert_eq!(
                serde_json::to_value(rows(projection.path())).unwrap(),
                witness[if committed { "after" } else { "before" }],
                "{cut}"
            );
            assert_eq!(
                serde_json::to_value(pending(projection.path())).unwrap(),
                witness[if committed {
                    "pending_after"
                } else {
                    "pending_before"
                }],
                "{cut}"
            );
            let conn = Connection::open(projection.path()).unwrap();
            let actual: (i64, i64, String) = conn.query_row("SELECT snapshot_commit_seq,checkpoint_commit_seq,hold_id FROM projection_checkpoint", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
            assert_eq!(
                (actual.0, actual.1),
                (witness["s"].as_i64().unwrap(), checkpoint)
            );
            assert_eq!(
                actual.2,
                control(root.path()).replacement_capture.unwrap().hold_id
            );
            assert_eq!(
                kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
                Some(ack)
            );
        }
        let original = control(root.path());
        let old_capture = original.replacement_capture.as_ref().unwrap();
        let corpus = Corpus::open(root.path());
        let gate = open_gate();
        let candidate =
            ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
                .unwrap()
                .build(&budget(Duration::from_secs(30)), &mut |_| {})
                .unwrap();
        candidate.revalidate().unwrap();
        let expected: Rows = serde_json::from_value::<Rows>(witness["after"].clone())
            .unwrap()
            .into_iter()
            .filter(|(_, (_, _, invalidated))| invalidated.is_none())
            .collect();
        assert_eq!(rows(candidate.path()), expected);
        assert_eq!(pending(candidate.path()), expected_pending(&expected));
        let current = control(root.path());
        assert_eq!(current.recovery_target, original.recovery_target);
        assert_eq!(current.episodes.deadline, original.episodes.deadline);
        assert_eq!(current.episodes.consumed, original.episodes.consumed + 1);
        assert_ne!(
            current.replacement_capture.unwrap().lease_epoch,
            old_capture.lease_epoch
        );
        assert_eq!(
            candidate.staged().verification.snapshot_commit_seq,
            witness["t"].as_i64().unwrap()
        );
        assert_eq!(
            candidate.staged().verification.checkpoint_commit_seq,
            witness["t"].as_i64().unwrap()
        );
        assert_eq!(
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
            Some(witness["t"].as_i64().unwrap())
        );
        assert_hold_released(root.path(), &old_capture.hold_id);
    }
}

fn kill_child_at(root: &Path, cut: &str) -> BTreeMap<String, serde_json::Value> {
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "replacement_child", "--ignored", "--nocapture"])
            .env("REPLACEMENT_CHILD_ROOT", root)
            .env("REPLACEMENT_CHILD_CUT", cut)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let output = child.0.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let mut observations = BTreeMap::new();
    loop {
        let line = receive
            .recv_timeout(Duration::from_secs(40))
            .expect("child boundary");
        if let Some((kind, json)) = line.split_once(' ')
            && matches!(kind, "WITNESS" | "CAPTURE" | "BACKUP")
        {
            observations.insert(kind.to_owned(), serde_json::from_str(json).unwrap());
        }
        if line == format!("BARRIER {cut}") {
            break;
        }
    }
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    reader.join().unwrap();
    observations
}

fn assert_hold_released(root: &Path, hold: &str) {
    let released: bool = Connection::open(root.join("kernel/kernel.sqlite"))
        .unwrap()
        .query_row(
            "SELECT released_at IS NOT NULL FROM capture_pins WHERE capture_pin_id=?1",
            [hold],
            |row| row.get(0),
        )
        .unwrap();
    assert!(released, "old hold {hold} must be released");
}

#[test]
fn restarted_capture_and_staging_cuts_rebuild_at_a_fresh_fence_without_moving_t() {
    for cut in ["capture-before-record", "before-stage", "after-stage"] {
        let root = tempfile::tempdir().unwrap();
        let observations = kill_child_at(root.path(), cut);
        let captured = &observations["CAPTURE"];
        let old = control(root.path());
        let stage = old
            .replacement_capture
            .as_ref()
            .and_then(|capture| capture.stage.as_ref());
        let digest = stage.map(|stage| stage.stage_manifest().digest());
        if cut == "capture-before-record" {
            assert!(old.replacement_capture.is_none());
        } else {
            assert!(stage.is_some());
        }
        let store = GenerationStore::open(Some(root.path())).unwrap();
        if cut == "after-stage" {
            store.validate(digest.as_ref().unwrap()).unwrap();
            std::fs::remove_file(database(root.path())).unwrap();
        }
        for _ in 0..2 {
            let kernel = kernel::KernelStore::open(root.path().join("kernel")).unwrap();
            assert_ne!(kernel.lease_epoch(), captured["epoch"].as_u64().unwrap());
        }
        let corpus = Corpus::open(root.path());
        if cut == "capture-before-record" {
            corpus
                .kernel
                .commit(intent("advance-after-capture"), |_| Ok(String::new()))
                .unwrap();
        }
        let gate = open_gate();
        let candidate =
            ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
                .unwrap()
                .build(&budget(Duration::from_secs(30)), &mut |_| {})
                .unwrap();
        candidate.revalidate().unwrap();
        if let Some(target) = old.recovery_target {
            assert_eq!(
                candidate.staged().verification.snapshot_commit_seq,
                target.commit_seq
            );
            assert_eq!(
                candidate.staged().verification.checkpoint_commit_seq,
                target.commit_seq
            );
            let expected: Rows =
                serde_json::from_value::<Rows>(observations["WITNESS"]["after"].clone())
                    .unwrap()
                    .into_iter()
                    .filter(|(_, (_, _, invalidated))| invalidated.is_none())
                    .collect();
            assert_eq!(rows(candidate.path()), expected);
        }
        assert_eq!(
            control(root.path()).episodes.deadline,
            old.episodes.deadline
        );
        assert_ne!(
            control(root.path())
                .replacement_capture
                .unwrap()
                .lease_epoch,
            captured["epoch"].as_u64().unwrap()
        );
        assert_hold_released(root.path(), captured["hold"].as_str().unwrap());
        if let Some(digest) = digest {
            assert!(
                store.validate(&digest).is_err(),
                "owned abandoned stage removed even without local file"
            );
        }
        assert_eq!(control(root.path()).episodes.consumed, 2);
    }
}

#[test]
fn restore_while_down_rebuilds_current_authority_at_the_unchanged_objective() {
    let root = tempfile::tempdir().unwrap();
    let observed = kill_child_at(root.path(), "down-restore");
    let original = control(root.path());
    let target = original.recovery_target.unwrap().commit_seq;
    let mut expected: Rows = serde_json::from_value(observed["WITNESS"]["before"].clone()).unwrap();
    {
        let corpus = Corpus::open(root.path());
        let backup: std::path::PathBuf =
            serde_json::from_value(observed["BACKUP"].clone()).unwrap();
        corpus.kernel.restore(&backup).unwrap();
        assert_eq!(
            kernel_incarnation_id(root.path()),
            original.kernel_incarnation_id
        );
        let object = corpus.publish("restored-only", "current restored authority");
        expected.insert(
            object,
            (
                "messages".to_owned(),
                b"current restored authority".to_vec(),
                None,
            ),
        );
        let retired = expected
            .iter()
            .find(|(_, (class, _, _))| class == "canonical_claims")
            .unwrap()
            .0
            .clone();
        corpus.retire(&retired);
        expected.remove(&retired);
        while corpus.tip() < target {
            corpus
                .kernel
                .commit(intent(&format!("restore-pad-{}", corpus.tip())), |_| {
                    Ok(String::new())
                })
                .unwrap();
        }
        assert_eq!(corpus.tip(), target);
    }
    let corpus = Corpus::open(root.path());
    let gate = open_gate();
    let candidate = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, spec(root.path()))
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |_| {})
        .unwrap();
    candidate.revalidate().unwrap();
    assert_eq!(rows(candidate.path()), expected);
    assert_ne!(
        serde_json::to_value(&expected).unwrap(),
        observed["WITNESS"]["after"]
    );
    assert_eq!(pending(candidate.path()), expected_pending(&expected));
    assert_eq!(candidate.staged().verification.snapshot_commit_seq, target);
    assert_eq!(
        candidate.staged().verification.checkpoint_commit_seq,
        target
    );
    let current = control(root.path());
    assert_eq!(current.episodes.deadline, original.episodes.deadline);
    assert_eq!(current.episodes.consumed, original.episodes.consumed + 1);
    assert_eq!(current.recovery_target, original.recovery_target);
    assert_hold_released(root.path(), observed["CAPTURE"]["hold"].as_str().unwrap());
}

#[test]
fn a_foreign_restored_lineage_refuses_the_recorded_intent_before_acknowledgement() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let backup = tempfile::tempdir().unwrap();
    std::fs::set_permissions(backup.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let corpus = Corpus::open(root.path());
    corpus.seed();
    let gate = open_gate();
    let stale = spec(root.path());
    record(root.path(), &gate, None, &stale.identity);
    let original = control(root.path());
    let foreign = Corpus::open(other.path());
    foreign.seed();
    let saved = foreign
        .kernel
        .backup(kernel::BackupRequest {
            destination_directory: backup.path().to_path_buf(),
            deadline: std::time::Instant::now() + Duration::from_secs(10),
            capture_pin_expires_at: None,
        })
        .unwrap();
    corpus.kernel.restore(saved.destination_path).unwrap();
    drop(corpus);
    let corpus = Corpus::open(root.path());
    let current = spec(root.path());
    assert_ne!(
        current.identity.kernel_incarnation_id,
        original.kernel_incarnation_id
    );
    assert!(matches!(
        ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, current),
        Err(BuildError::Invalid(_))
    ));
    let kernel_state = || {
        let connection = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
        let captures: i64 = connection
            .query_row("SELECT count(*) FROM capture_pins", [], |row| row.get(0))
            .unwrap();
        (
            corpus.tip(),
            captures,
            corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        )
    };
    let before = kernel_state();
    let observed_kernel = Connection::open(root.path().join("kernel/kernel.sqlite")).unwrap();
    let data_version: i64 = observed_kernel
        .pragma_query_value(None, "data_version", |row| row.get(0))
        .unwrap();
    let record_path = root.path().join("search-lifecycle/intent.json");
    let bytes = std::fs::read(&record_path).unwrap();
    let failure = ReplacementBuilder::open(root.path(), &corpus.kernel, &gate, stale)
        .unwrap()
        .build(&budget(Duration::from_secs(30)), &mut |event| {
            assert_eq!(event, BuildEvent::Aborting)
        })
        .err()
        .expect("stale caller identity cannot authorize the foreign kernel");
    assert!(matches!(
        failure.error,
        BuildError::Mutation(retrieval::ProjectionError::IdentityMismatch)
    ));
    assert!(matches!(
        failure.cleanup_error,
        Some(BuildError::Mutation(
            retrieval::ProjectionError::IdentityMismatch
        ))
    ));
    assert_eq!(
        kernel_state(),
        before,
        "no commit, capture, registration, or acknowledgement"
    );
    assert_eq!(
        observed_kernel
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        data_version
    );
    assert_eq!(std::fs::read(record_path).unwrap(), bytes);
    assert!(!database(root.path()).exists());
    assert_eq!(
        corpus.kernel.outbox_consumer_checkpoint(CONSUMER).unwrap(),
        None
    );
    assert_eq!(control(root.path()), original);
}
