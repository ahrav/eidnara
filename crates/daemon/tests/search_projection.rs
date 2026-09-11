use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;

use daemon::search_projection::{CACHE_KIB, SearchProjection, SearchProjectionError};
use kernel::source_identity::{Occurrence, Span, select, validate_span};
use kernel::{
    ArtifactIngestRequest, CommitIntent, DomainSpec, ExportWindow, KernelStore, ProviderEgress,
    RemediationTarget, RepositoryProvenance, Sensitivity, SourceDescriptorRequest,
    SourceHoldAdmission, SourceHoldBinding, SourceHoldBounds, SourcePageBounds, SourceRow,
};
use retrieval::{
    OccurrenceRecord, PersistBounds, ProjectionError, ProjectionIdentity, install_identity,
    persist_occurrences, read_identity, read_occurrence,
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
        buffer: text,
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

#[cfg(unix)]
#[test]
fn non_utf8_data_home_is_rejected_without_creating_either_path() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().unwrap();
    let data_home = dir.path().join(OsString::from_vec(b"data-\xff".to_vec()));
    let lossy_home = dir.path().join("data-\u{fffd}");
    let result = SearchProjection::open(&data_home).map(|_| ());

    for home in [&data_home, &lossy_home] {
        assert_eq!(
            (
                home.try_exists().unwrap(),
                home.join("search/search.sqlite").try_exists().unwrap(),
            ),
            (false, false),
            "non-UTF-8 input must not create the root or database at {home:?}"
        );
    }
    assert!(
        matches!(
            &result,
            Err(SearchProjectionError::Store(storage::StoreError::Io(error)))
                if error.kind() == std::io::ErrorKind::InvalidInput
        ),
        "expected InvalidInput, got {result:?}"
    );
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

/// Exported text is already selected, but descriptor spans refer to the
/// original fixture buffer that persistence requires.
fn persist_fixture_rows(
    projection: &SearchProjection,
    rows: &[SourceRow],
    source_buffer: &str,
) -> Vec<(String, bool)> {
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
                .map(|(row, identity)| {
                    let span = row.detail.span.map(|(start, end)| Span { start, end });
                    validate_span(span, source_buffer).unwrap();
                    assert_eq!(
                        select(span, source_buffer),
                        row.text.as_deref().unwrap().as_bytes(),
                        "exported bytes match the original fixture selection"
                    );
                    OccurrenceRecord {
                        occurrence: Occurrence {
                            class: &row.detail.class,
                            identity,
                            revision: &row.detail.revision,
                            representation: &row.detail.representation,
                            span,
                        },
                        buffer: source_buffer,
                        domain_id: "domain",
                        sensitivity: Sensitivity::Normal,
                        source_object_id: &row.object_id,
                        source_evidence_id: &row.detail.evidence_id,
                        source_artifact_digest: &row.detail.artifact_digest,
                        created_commit_seq: row.created_commit_seq,
                    }
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
    let selections = [
        (None, text),
        (Some(Span { start: 0, end: 3 }), "the"),
        (Some(Span { start: 4, end: 11 }), "message"),
        (Some(Span { start: 4, end: 4 }), ""),
    ];
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
            for (span, _) in selections {
                envelope
                    .publish_source_descriptor(&SourceDescriptorRequest {
                        occurrence: Occurrence {
                            class: "messages",
                            identity: MSG_A,
                            revision: "1",
                            representation: "text",
                            span,
                        },
                        source_policy: kernel::SourceDescriptorPolicy::Native,
                        domain_id: "domain",
                        scope_id: None,
                        evidence_id: &handle.evidence_id,
                        artifact_digest: &handle.digest,
                        buffer: text,
                        sensitivity: Sensitivity::Normal,
                        observed_at: 1,
                    })
                    .unwrap();
            }
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
    assert_eq!(before.len(), selections.len());
    for (span, selected) in selections {
        let row = before
            .iter()
            .find(|row| row.detail.span == span.map(|span| (span.start, span.end)))
            .unwrap();
        assert_eq!(row.text.as_deref(), Some(selected));
    }

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
    let first = persist_fixture_rows(&projection, &before, text);
    assert_eq!(
        first,
        before
            .iter()
            .map(|row| (row.detail.occurrence_id.clone(), true))
            .collect::<Vec<_>>(),
        "first persistence inserts every exported occurrence under its own identity"
    );

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
    let second = persist_fixture_rows(&projection, &after, text);
    assert_eq!(
        second,
        first
            .iter()
            .map(|(id, _)| (id.clone(), false))
            .collect::<Vec<_>>()
    );
    projection
        .read(|conn| {
            for row in &after {
                let stored = read_occurrence(conn, &row.detail.occurrence_id)?.unwrap();
                assert_eq!(stored.occurrence_id, row.detail.occurrence_id);
                assert_eq!(stored.lineage_id, row.detail.lineage_id);
                assert_eq!(stored.payload_id, row.detail.payload_id);
                assert_eq!(stored.span, row.detail.span);
                assert_eq!(stored.bytes, row.text.as_deref().unwrap().as_bytes());
                assert_eq!(stored.domain_id, "domain");
                assert_ne!(
                    stored.domain_id, "Original Name",
                    "an identifier, never the name"
                );
            }
            Ok(())
        })
        .unwrap();
}
