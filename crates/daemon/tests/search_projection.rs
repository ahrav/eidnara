//! The daemon's search.sqlite connection: what it verifies at open, what a
//! write commits or rolls back, and that canonical inputs exported from the
//! kernel persist with the same identity before and after a name-only
//! remediation.

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;

use daemon::search_projection::{CACHE_KIB, SearchProjection, SearchProjectionError};
use kernel::source_identity::{Occurrence, Span};
use kernel::{
    ArtifactIngestRequest, CommitIntent, DomainSpec, ExportWindow, KernelStore, ProviderEgress,
    RemediationTarget, RepositoryProvenance, Sensitivity, SourceDescriptorRequest,
    SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds, SourcePageBounds, SourceRow,
};
use retrieval::{
    OccurrenceRecord, Payload, PersistBounds, ProjectionError, ProjectionIdentity,
    install_identity, persist_occurrences, read_identity, read_occurrence,
};
use sha2::{Digest, Sha256};

fn bounds() -> PersistBounds {
    PersistBounds {
        max_records: NonZeroUsize::new(64).unwrap(),
        max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
    }
}

fn identity(kernel_incarnation_id: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        schema_version: retrieval::SCHEMA_VERSION,
        kernel_incarnation_id: kernel_incarnation_id.to_string(),
        projection_policy_version: "source-policy.v1".to_string(),
        identity_contract_version: "search-projection-identity-v2".to_string(),
        limit_manifest_protocol_version: "limits.v1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    }
}

fn message<'a>(
    key: &'a str,
    identity: &'a [(&'a str, &'a str)],
    text: &'a str,
) -> OccurrenceRecord<'a> {
    OccurrenceRecord {
        occurrence: Occurrence {
            class: "messages",
            identity,
            revision: "1",
            representation: "text",
            span: None,
        },
        payload: Payload::Whole(text),
        domain_id: "domain-stable-id",
        sensitivity: Sensitivity::Normal,
        source_object_id: key,
        source_evidence_id: key,
        source_artifact_digest: "0000000000000000000000000000000000000000000000000000000000000000",
        created_commit_seq: 3,
    }
}

const MSG_A: &[(&str, &str)] = &[
    ("project_id", "proj-a"),
    ("harness", "opencode"),
    ("session_id", "sess-01"),
    ("message_id", "msg-a"),
    ("block_index", "0"),
];
const MSG_C: &[(&str, &str)] = &[
    ("project_id", "proj-a"),
    ("harness", "opencode"),
    ("session_id", "sess-01"),
    ("message_id", "msg-c"),
    ("block_index", "0"),
];
const MSG_B: &[(&str, &str)] = &[
    ("project_id", "proj-a"),
    ("harness", "opencode"),
    ("session_id", "sess-01"),
    ("message_id", "msg-b"),
    ("block_index", "0"),
];

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_connection_is_verified_owner_only_and_rows_survive_close_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (ids, path) = {
        let projection = SearchProjection::open(dir.path()).unwrap();
        let facts = projection.verify_connection().unwrap();
        assert_eq!(facts.journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(facts.synchronous, 2, "FULL");
        assert!(facts.foreign_keys);
        assert_eq!(facts.cache_size, -i64::from(CACHE_KIB));
        assert_eq!(facts.temp_store, 2, "MEMORY");
        let path = projection.path().to_path_buf();
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&path), 0o600);
        let ids = projection
            .write(|conn| {
                install_identity(conn, &identity("kernel-1"), 1)?;
                let outcomes = persist_occurrences(
                    conn,
                    &[
                        message("a", MSG_A, "  exact\ttext\r\n"),
                        message("b", MSG_B, "naïve 日本語 🎉"),
                    ],
                    bounds(),
                    1,
                )?;
                Ok(outcomes
                    .into_iter()
                    .map(|o| o.occurrence_id)
                    .collect::<Vec<_>>())
            })
            .unwrap();
        // The write-ahead log the write created is owner-only too.
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = std::path::PathBuf::from(wal);
        if wal.exists() {
            assert_eq!(mode(&wal), 0o600);
        }
        // A refused write commits nothing.
        let refused = projection.write(|conn| {
            persist_occurrences(conn, &[message("c", MSG_C, "would land")], bounds(), 2)?;
            persist_occurrences(
                conn,
                &[OccurrenceRecord {
                    occurrence: Occurrence {
                        class: "messages",
                        identity: &MSG_A[..4],
                        revision: "1",
                        representation: "text",
                        span: None,
                    },
                    ..message("d", MSG_A, "refused")
                }],
                bounds(),
                2,
            )?;
            Ok(())
        });
        assert!(matches!(
            refused,
            Err(SearchProjectionError::Projection(
                ProjectionError::Occurrence(_)
            ))
        ));
        // A read cannot write: the store's query-only scope refuses a statement
        // no constraint would, and the table is unchanged afterwards.
        let denied = projection.read(|conn| {
            conn.execute(
                "INSERT INTO vector_generations(generation_id,embedding_model,tokenizer_fingerprint,
                     vector_dimension,generation_epoch,state,created_at,updated_at)
                 VALUES ('g','m','f',8,1,'building',1,1)",
                [],
            )
            .map_err(ProjectionError::from)?;
            Ok(())
        });
        match denied {
            Err(SearchProjectionError::Projection(ProjectionError::Sqlite(text))) => {
                assert!(
                    text.contains("not authorized") || text.contains("readonly"),
                    "{text}"
                )
            }
            other => panic!("{other:?}"),
        }
        let generations: i64 = projection
            .read(|conn| {
                conn.query_row("SELECT COUNT(*) FROM vector_generations", [], |r| r.get(0))
                    .map_err(ProjectionError::from)
            })
            .unwrap();
        assert_eq!(generations, 0);
        (ids, path)
    };
    // Reopen: same file, same identity, same bytes.
    let projection = SearchProjection::open(dir.path()).unwrap();
    assert_eq!(projection.path(), path);
    projection
        .read(|conn| {
            assert_eq!(read_identity(conn)?, Some(identity("kernel-1")));
            assert_eq!(
                {
                    let mut statement = conn
                        .prepare("SELECT occurrence_id FROM occurrences")
                        .map_err(ProjectionError::from)?;
                    statement
                        .query_map([], |row| row.get::<_, String>(0))
                        .map_err(ProjectionError::from)?
                        .collect::<rusqlite::Result<BTreeSet<_>>>()
                        .map_err(ProjectionError::from)?
                },
                ids.iter().cloned().collect::<BTreeSet<_>>(),
                "the refused batch left no row"
            );
            let a = read_occurrence(conn, &ids[0])?.unwrap();
            assert_eq!(a.bytes, b"  exact\ttext\r\n");
            let b = read_occurrence(conn, &ids[1])?.unwrap();
            assert_eq!(std::str::from_utf8(&b.bytes).unwrap(), "naïve 日本語 🎉");
            Ok(())
        })
        .unwrap();
}

#[test]
fn retrieval_depends_on_no_product_crate_but_the_kernel() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let workspace: toml_members::Members =
        toml_members::parse(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap());
    assert!(workspace.members.contains(&"crates/retrieval".to_string()));
    let infrastructure = ["storage", "storage-types", "lease"];
    let product: BTreeSet<String> = workspace
        .members
        .iter()
        .filter_map(|m| m.strip_prefix("crates/"))
        .filter(|name| !infrastructure.contains(name))
        .map(str::to_string)
        .collect();
    assert!(product.contains("kernel") && product.contains("daemon"));
    let manifest = std::fs::read_to_string(root.join("crates/retrieval/Cargo.toml")).unwrap();
    let dependencies = toml_members::section(&manifest, "[dependencies]");
    let product_deps: BTreeSet<&str> = dependencies
        .iter()
        .filter(|name| product.contains(**name))
        .copied()
        .collect();
    assert_eq!(product_deps, BTreeSet::from(["kernel"]));
    assert!(!dependencies.contains(&"daemon"));
    // The daemon owns the connection; the retrieval crate opens nothing.
    let lib = std::fs::read_to_string(root.join("crates/retrieval/src/lib.rs")).unwrap();
    assert!(!lib.contains("open_sqlite(") && !lib.contains("Connection::open"));
}

/// A minimal reader for the two manifest shapes the test needs.
mod toml_members {
    pub struct Members {
        pub members: Vec<String>,
    }

    pub fn parse(text: &str) -> Members {
        let start = text.find("members = [").unwrap() + "members = [".len();
        let end = start + text[start..].find(']').unwrap();
        let members = text[start..end]
            .split(',')
            .map(|item| item.trim().trim_matches('"').to_string())
            .filter(|item| !item.is_empty())
            .collect();
        Members { members }
    }

    /// Dependency names under `section`, up to the next section header.
    pub fn section<'a>(manifest: &'a str, section: &str) -> Vec<&'a str> {
        let start = manifest.find(section).unwrap() + section.len();
        manifest[start..]
            .lines()
            .take_while(|line| !line.starts_with('['))
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                line.split_once('=').map(|(name, _)| name.trim())
            })
            .collect()
    }
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "daemon-search-projection-test".to_string(),
        operation_key: key.to_string(),
        request_digest: format!("{:x}", Sha256::digest(key.as_bytes())),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

/// Persists exported kernel rows and returns whether each was newly inserted.
fn persist_rows(projection: &SearchProjection, rows: &[SourceRow]) -> Vec<(String, bool)> {
    projection
        .write(|conn| {
            let identities: Vec<Vec<(&str, &str)>> = rows
                .iter()
                .map(|row| {
                    row.detail
                        .identity
                        .iter()
                        .map(|(n, v)| (n.as_str(), v.as_str()))
                        .collect()
                })
                .collect();
            let records: Vec<OccurrenceRecord<'_>> = rows
                .iter()
                .zip(&identities)
                .map(|(row, identity)| OccurrenceRecord {
                    occurrence: Occurrence {
                        class: &row.detail.class,
                        identity,
                        revision: &row.detail.revision,
                        representation: &row.detail.representation,
                        span: row.detail.span.map(|(start, end)| Span { start, end }),
                    },
                    payload: Payload::Selected(row.text.as_deref().unwrap()),
                    domain_id: "domain",
                    sensitivity: Sensitivity::Normal,
                    source_object_id: &row.object_id,
                    source_evidence_id: &row.detail.evidence_id,
                    source_artifact_digest: &row.detail.artifact_digest,
                    created_commit_seq: row.created_commit_seq,
                })
                .collect();
            Ok(persist_occurrences(conn, &records, bounds(), 1)?
                .into_iter()
                .map(|o| (o.occurrence_id, o.inserted))
                .collect())
        })
        .unwrap()
}

#[test]
fn a_name_only_remediation_changes_no_persisted_input() {
    let dir = tempfile::tempdir().unwrap();
    let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
    kernel
        .commit(intent("seed"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain".to_string(),
                object_id: "domain-object".to_string(),
                name: "Original Name".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.register_outbox_consumer("search", 1)?;
            Ok(String::new())
        })
        .unwrap();
    let text = "the message text stays the same";
    let handle = kernel
        .ingest_exact_artifact(ArtifactIngestRequest {
            intent: intent("artifact"),
            payload: text.as_bytes().to_vec(),
            evidence_id: "evidence-1".to_string(),
            object_id: "evidence-object-1".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "tool_output".to_string(),
            source_id: "native/1".to_string(),
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
    kernel
        .commit(intent("publish"), |envelope| {
            envelope
                .publish_source_descriptor(&SourceDescriptorRequest {
                    occurrence: Occurrence {
                        class: "messages",
                        identity: MSG_A,
                        revision: "1",
                        representation: "text",
                        span: None,
                    },
                    domain_id: "domain",
                    scope_id: None,
                    evidence_id: &handle.evidence_id,
                    artifact_digest: &handle.digest,
                    buffer: text,
                    sensitivity: Sensitivity::Normal,
                    observed_at: 1,
                })
                .unwrap();
            Ok(String::new())
        })
        .unwrap();
    let binding = SourceHoldBinding {
        consumer_id: "search".to_string(),
        lease_epoch: kernel.lease_epoch(),
        source_policy_version: "source-policy.v1".to_string(),
    };
    let hold = kernel
        .capture_source_hold(
            &binding,
            SourceHoldBounds {
                admission: SourceHoldAdmission {
                    max_references: NonZeroUsize::new(64).unwrap(),
                    max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                },
                expiry_ms: NonZeroU64::new(60 * 60 * 1000).unwrap(),
            },
        )
        .unwrap();
    let page_bounds = SourcePageBounds {
        max_rows: NonZeroUsize::new(64).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
    };
    let export = |kernel: &KernelStore| {
        kernel
            .export_source_page(
                &binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                None,
                page_bounds,
            )
            .unwrap()
            .rows
    };
    let before = export(&kernel);
    assert_eq!(before.len(), 1);

    let projection = SearchProjection::open(dir.path()).unwrap();
    projection
        .write(|conn| install_identity(conn, &identity("kernel-1"), 1))
        .unwrap();
    // Every tunable refusal arm of the connection check fires on the state it
    // names, and the pinned state passes again once restored.
    for (pragma, wrong, pinned) in [
        ("cache_size", -16, -i64::from(CACHE_KIB)),
        ("temp_store", 0, 2),
    ] {
        projection.set_pragma_for_test(pragma, wrong);
        match projection.verify_connection() {
            Err(SearchProjectionError::Connection(text)) => {
                assert!(text.contains(pragma), "{text}")
            }
            other => panic!("{pragma}: {other:?}"),
        }
        projection.set_pragma_for_test(pragma, pinned);
        projection.verify_connection().unwrap();
    }
    let first = persist_rows(&projection, &before);
    assert!(first[0].1, "first persistence inserts");

    // Rename the domain. The name is not an input: the exported rows are byte
    // for byte the same, and persisting them again inserts nothing.
    kernel
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
    let after = export(&kernel);
    assert_eq!(
        after, before,
        "a name-only remediation changes no exported input"
    );
    let second = persist_rows(&projection, &after);
    assert_eq!(second, vec![(first[0].0.clone(), false)]);
    projection
        .read(|conn| {
            let stored = read_occurrence(conn, &first[0].0)?.unwrap();
            assert_eq!(stored.bytes, text.as_bytes());
            assert_eq!(stored.domain_id, "domain");
            assert_ne!(
                stored.domain_id, "Original Name",
                "an identifier, never the name"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn a_kernel_export_applies_as_one_batch_with_pending_only_for_dense_inputs_and_no_acknowledgement()
{
    use kernel::source_identity::OccurrenceClass;
    use retrieval::batch::{
        BatchBounds, BatchStatus, MutationIdentity, VectorGeneration, batch_from_rows,
        pending_jobs, register_generation, row_identities,
    };
    let dir = tempfile::tempdir().unwrap();
    let kernel = KernelStore::open(dir.path().join("kernel")).unwrap();
    kernel
        .commit(intent("seed"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain".to_string(),
                object_id: "domain-object".to_string(),
                name: "Name".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            envelope.register_outbox_consumer("search", 1)?;
            Ok(String::new())
        })
        .unwrap();
    // One dense-eligible message and one raw tool span, each over its own bytes.
    let publish = |class: &str,
                   identity: &[(&str, &str)],
                   representation: &str,
                   revision: &str,
                   key: &str,
                   text: &str| {
        let handle = kernel
            .ingest_exact_artifact(ArtifactIngestRequest {
                intent: intent(&format!("artifact-{key}")),
                payload: text.as_bytes().to_vec(),
                evidence_id: format!("evidence-{key}"),
                object_id: format!("evidence-object-{key}"),
                object_kind: "evidence".to_string(),
                domain_id: "domain".to_string(),
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
        kernel
            .commit(intent(&format!("publish-{key}")), |envelope| {
                envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class,
                            identity,
                            revision,
                            representation,
                            span: None,
                        },
                        domain_id: "domain",
                        scope_id: None,
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap();
                Ok(String::new())
            })
            .unwrap();
    };
    publish("messages", MSG_A, "text", "1", "m", "dense message");
    const TOOL: &[(&str, &str)] = &[
        ("project_id", "proj-a"),
        ("harness", "pi"),
        ("session_id", "sess-01"),
        ("parent_message_id", "msg-2"),
        ("tool_call_id", "call-1"),
        ("result_revision", "1"),
        ("block_index", "0"),
    ];
    publish(
        "raw_tool_spans",
        TOOL,
        "tool_output",
        "1",
        "t",
        "lexical only tool output",
    );
    let binding = SourceHoldBinding {
        consumer_id: "search".to_string(),
        lease_epoch: kernel.lease_epoch(),
        source_policy_version: "source-policy.v1".to_string(),
    };
    let hold = kernel
        .capture_source_hold(
            &binding,
            SourceHoldBounds {
                admission: SourceHoldAdmission {
                    max_references: NonZeroUsize::new(64).unwrap(),
                    max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                },
                expiry_ms: NonZeroU64::new(60 * 60 * 1000).unwrap(),
            },
        )
        .unwrap();
    let page = kernel
        .export_source_page(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::Snapshot,
            None,
            SourcePageBounds {
                max_rows: NonZeroUsize::new(64).unwrap(),
                max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(page.rows.len(), 2);
    let kernel_checkpoint = || -> i64 {
        rusqlite::Connection::open_with_flags(
            dir.path().join("kernel").join("kernel.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT checkpoint_commit_seq FROM outbox_consumers WHERE consumer_id='search'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    let checkpoint_before = kernel_checkpoint();

    let projection = SearchProjection::open(dir.path()).unwrap();
    let generation = VectorGeneration {
        generation_id: "gen-1".to_string(),
        embedding_model: "model-a".to_string(),
        tokenizer_fingerprint: "fp-a".to_string(),
        vector_dimension: 8,
        generation_epoch: 1,
    };
    projection
        .write(|conn| {
            install_identity(conn, &identity("kernel-1"), 1)?;
            register_generation(conn, &generation, 1)?;
            Ok(())
        })
        .unwrap();
    let identities = row_identities(&page.rows);
    let mutation = MutationIdentity {
        kernel_incarnation_id: "kernel-1".to_string(),
        hold_id: hold.hold_id.clone(),
        snapshot_commit_seq: hold.snapshot,
        through_commit_seq: hold.snapshot,
    };
    let batch = batch_from_rows(&page.rows, &identities, mutation.clone(), Some("gen-1")).unwrap();
    assert_eq!(batch.records.len(), 2);
    assert!(batch.invalidations.is_empty());
    let bounds = BatchBounds {
        persist: PersistBounds {
            max_records: NonZeroUsize::new(64).unwrap(),
            max_payload_bytes: NonZeroUsize::new(1 << 16).unwrap(),
            max_tuple_bytes: NonZeroUsize::new(2048).unwrap(),
        },
        max_source_bytes: NonZeroUsize::new(1 << 16).unwrap(),
        max_local_mutations: NonZeroUsize::new(64).unwrap(),
        max_pending: NonZeroUsize::new(64).unwrap(),
    };
    assert_eq!(
        projection.batch_status(&mutation).unwrap(),
        BatchStatus::NotApplied
    );
    let hold_before = kernel
        .source_hold_status(&binding, &hold.hold_id, hold.captured_at)
        .unwrap();
    let outcome = projection.apply_batch(&batch, bounds, 2).unwrap();
    assert_eq!(
        kernel
            .source_hold_status(&binding, &hold.hold_id, hold.captured_at)
            .unwrap(),
        hold_before,
        "the hold is neither released nor extended by the primitive"
    );
    assert_eq!((outcome.rows_inserted, outcome.pending_created), (2, 1));
    assert_eq!(outcome.checkpoint_commit_seq, hold.snapshot);
    assert_eq!(
        projection.batch_status(&mutation).unwrap(),
        BatchStatus::Applied
    );
    let message_row = page
        .rows
        .iter()
        .find(|row| row.detail.class == OccurrenceClass::Messages.code())
        .unwrap();
    projection
        .read(|conn| {
            let pending = pending_jobs(conn)?;
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].1, message_row.detail.occurrence_id);
            let tool = page
                .rows
                .iter()
                .find(|row| row.detail.class == "raw_tool_spans")
                .unwrap();
            let stored = read_occurrence(conn, &tool.detail.occurrence_id)?.unwrap();
            assert_eq!(stored.bytes, b"lexical only tool output");
            assert_eq!(stored.domain_id, "domain");
            Ok(())
        })
        .unwrap();
    // The primitive acknowledged nothing to the kernel.
    assert_eq!(kernel_checkpoint(), checkpoint_before);
    // Replaying the same export changes nothing.
    let replay = projection.apply_batch(&batch, bounds, 3).unwrap();
    assert_eq!(
        (
            replay.rows_inserted,
            replay.rows_replayed,
            replay.pending_created
        ),
        (0, 2, 0)
    );
    // A catch-up window: the message is superseded, the tool span retired, and
    // a tool row over a sub-span of a new buffer is created. The exported
    // selection persists under the kernel's own occurrence identity, and each
    // invalidation carries its real reason.
    publish("messages", MSG_A, "text", "2", "m2", "dense message v2");
    let tool_row = page
        .rows
        .iter()
        .find(|row| row.detail.class == "raw_tool_spans")
        .unwrap()
        .clone();
    kernel
        .commit(intent("retire-tool"), |envelope| {
            envelope.retire_observation(&tool_row.object_id)?;
            Ok(String::new())
        })
        .unwrap();
    let spanned_buffer = "prefix|selected part|suffix";
    let handle = kernel
        .ingest_exact_artifact(ArtifactIngestRequest {
            intent: intent("artifact-span"),
            payload: spanned_buffer.as_bytes().to_vec(),
            evidence_id: "evidence-span".to_string(),
            object_id: "evidence-object-span".to_string(),
            object_kind: "evidence".to_string(),
            domain_id: "domain".to_string(),
            source_kind: "tool_output".to_string(),
            source_id: "native/span".to_string(),
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
    const TOOL_SPAN: &[(&str, &str)] = &[
        ("project_id", "proj-a"),
        ("harness", "pi"),
        ("session_id", "sess-01"),
        ("parent_message_id", "msg-2"),
        ("tool_call_id", "call-span"),
        ("result_revision", "1"),
        ("block_index", "0"),
    ];
    kernel
        .commit(intent("publish-span"), |envelope| {
            envelope
                .publish_source_descriptor(&SourceDescriptorRequest {
                    occurrence: Occurrence {
                        class: "raw_tool_spans",
                        identity: TOOL_SPAN,
                        revision: "1",
                        representation: "tool_output",
                        span: Some(Span { start: 7, end: 20 }),
                    },
                    domain_id: "domain",
                    scope_id: None,
                    evidence_id: &handle.evidence_id,
                    artifact_digest: &handle.digest,
                    buffer: spanned_buffer,
                    sensitivity: Sensitivity::Normal,
                    observed_at: 1,
                })
                .unwrap();
            Ok(String::new())
        })
        .unwrap();
    let through = kernel.tip().unwrap();
    kernel
        .extend_source_hold(
            &binding,
            &hold.hold_id,
            through,
            SourceHoldAdmission {
                max_references: NonZeroUsize::new(64).unwrap(),
                max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
            },
        )
        .unwrap();
    let catch_up = kernel
        .export_source_page(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::CatchUp { through },
            None,
            SourcePageBounds {
                max_rows: NonZeroUsize::new(64).unwrap(),
                max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                max_decoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
                max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
            },
        )
        .unwrap();
    let identities = row_identities(&catch_up.rows);
    let mutation = MutationIdentity {
        through_commit_seq: through,
        ..mutation
    };
    let batch =
        batch_from_rows(&catch_up.rows, &identities, mutation.clone(), Some("gen-1")).unwrap();
    assert_eq!(
        batch.records.len(),
        2,
        "the new message revision and the spanned tool row"
    );
    assert_eq!(
        batch.invalidations.len(),
        2,
        "the old message and the retired tool row"
    );
    let outcome = projection.apply_batch(&batch, bounds, 4).unwrap();
    assert_eq!(
        (
            outcome.rows_inserted,
            outcome.tombstones_recorded,
            outcome.pending_created,
            outcome.pending_obsoleted,
            outcome.checkpoint_commit_seq
        ),
        (2, 2, 1, 1, through)
    );
    let spanned = catch_up
        .rows
        .iter()
        .find(|row| row.detail.span.is_some())
        .unwrap();
    projection
        .read(|conn| {
            let stored = read_occurrence(conn, &spanned.detail.occurrence_id)?
                .expect("the spanned row persists under the kernel's occurrence id");
            assert_eq!(stored.bytes, b"selected part");
            assert_eq!(stored.span, Some((7, 20)));
            let old_message = read_occurrence(conn, &message_row.detail.occurrence_id)?.unwrap();
            assert_eq!(
                old_message.tombstone.map(|t| t.reason),
                Some(retrieval::TombstoneReason::Superseded)
            );
            let old_tool = read_occurrence(conn, &tool_row.detail.occurrence_id)?.unwrap();
            assert_eq!(
                old_tool.tombstone.map(|t| t.reason),
                Some(retrieval::TombstoneReason::Retired)
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        kernel_checkpoint(),
        checkpoint_before,
        "still no acknowledgement"
    );
}
