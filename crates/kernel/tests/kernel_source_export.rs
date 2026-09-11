//! Exported pages are compared with the independent ledger of what was
//! published, and every bound is proved to act before any byte is read.

#![cfg(feature = "test-support")]

mod source_fixture;

use std::collections::BTreeSet;
use std::fs;
use std::num::{NonZeroU64, NonZeroUsize};

use kernel::{
    ArtifactDeletionKind, ArtifactErrorKind, ExportWindow, KernelError,
    MAX_SOURCE_HOLD_LIFETIME_MS, ObservationPayload, PageBound, SourceCursor,
    SourceDescriptorDetail, SourceExportError, SourceHold, SourceHoldError, SourceHoldInvalidity,
    SourcePage, SourcePageBounds, SourceRow,
};
use source_fixture::*;

#[test]
fn continuations_are_bound_to_their_hold_and_window() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let held = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let unextended = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let other = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    fixture.publish("messages", "late", 1, "late message");
    fixture.publish("raw_tool_spans", "late", 1, "late tool");
    let through = fixture.store.tip().unwrap();
    for hold in [&held, &other] {
        fixture
            .store
            .extend_source_hold(&binding, &hold.hold_id, through, wide_admission())
            .unwrap();
    }
    let window = ExportWindow::CatchUp { through };
    let first = fixture
        .store
        .export_source_page(
            &binding,
            &held.hold_id,
            held.captured_at,
            window,
            None,
            page_bounds(1, 1024),
        )
        .unwrap();
    assert_eq!(first.rows[0].detail.class, "messages");
    let cursor = first.next.unwrap();
    let snapshot_cursor = fixture
        .store
        .export_source_page(
            &binding,
            &held.hold_id,
            held.captured_at,
            ExportWindow::Snapshot,
            None,
            page_bounds(1, 1024),
        )
        .unwrap()
        .next
        .unwrap();

    let before_cursor = fixture.publish("canonical_claims", "late", 1, "earlier key");
    let later = fixture.store.tip().unwrap();
    fixture
        .store
        .extend_source_hold(&binding, &held.hold_id, later, wide_admission())
        .unwrap();
    let complete = fixture
        .store
        .export_source_page(
            &binding,
            &held.hold_id,
            held.captured_at,
            ExportWindow::CatchUp { through: later },
            None,
            roomy(),
        )
        .unwrap();
    assert_eq!(complete.rows[0].object_id, before_cursor);
    let valid = fixture
        .store
        .export_source_page(
            &binding,
            &held.hold_id,
            held.captured_at,
            window,
            Some(&cursor),
            roomy(),
        )
        .unwrap();
    assert_eq!(valid.rows.len(), 1);
    assert_eq!(valid.rows[0].detail.class, "raw_tool_spans");
    assert!(valid.next.is_none());

    let mut accepted = Vec::new();
    for (case, hold, requested, cursor) in [
        (
            "changed through",
            &held,
            ExportWindow::CatchUp { through: later },
            &cursor,
        ),
        ("swapped extended hold", &other, window, &cursor),
        ("swapped unextended hold", &unextended, window, &cursor),
        (
            "catch-up to snapshot",
            &held,
            ExportWindow::Snapshot,
            &cursor,
        ),
        ("snapshot to catch-up", &held, window, &snapshot_cursor),
    ] {
        let result = fixture.store.export_source_page(
            &binding,
            &hold.hold_id,
            held.captured_at,
            requested,
            Some(cursor),
            roomy(),
        );
        if result != Err(SourceExportError::Hold(SourceHoldError::InvalidRequest)) {
            accepted.push(case);
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted mismatched continuations: {accepted:?}"
    );
}

#[test]
fn export_revalidates_descriptor_identity_and_selected_payload() {
    let mut accepted = Vec::new();
    for invalidation_only in [false, true] {
        let mut fixture = Fixture::open();
        let text = "alpha β gamma";
        let evidence = fixture.retain("identity-check", text);
        let object_id = fixture.publish_span(
            "raw_tool_spans",
            "identity-check",
            1,
            text,
            Some((6, 8)),
            evidence,
        );
        let hold = fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .unwrap();
        let window = if invalidation_only {
            let through = fixture.retire(&object_id);
            fs::remove_file(fixture.object_path(&fixture.ledger[&object_id].digest)).unwrap();
            ExportWindow::CatchUp { through }
        } else {
            ExportWindow::Snapshot
        };
        let original_payload: Vec<u8> = fixture
            .inspect()
            .query_row(
                "SELECT observation_payload FROM observations WHERE object_id=?1",
                [&object_id],
                |row| row.get(0),
            )
            .unwrap();
        let original: ObservationPayload = serde_json::from_slice(&original_payload).unwrap();
        let detail: SourceDescriptorDetail =
            serde_json::from_str(original.detail.as_deref().unwrap()).unwrap();
        let baseline = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                window,
                None,
                roomy(),
            )
            .unwrap();
        assert_eq!(
            baseline.rows[0].text.as_deref(),
            (!invalidation_only).then_some("β")
        );
        for field in [
            "identity",
            "representation",
            "occurrence_id",
            "payload_id",
            "occurrence_tuple",
            "source_policy",
            "span",
        ] {
            let mut changed = detail.clone();
            match field {
                "identity" => changed.identity[0].1 = "different-project".to_string(),
                "representation" => changed.representation = "tool_error".to_string(),
                "occurrence_id" => changed.occurrence_id = "0".repeat(64),
                "payload_id" => changed.payload_id = "0".repeat(64),
                "occurrence_tuple" => changed.occurrence_tuple[0] ^= 1,
                "source_policy" => {
                    changed.source_policy = kernel::SourceDescriptorPolicy::Git {
                        version: POLICY.to_string(),
                    }
                }
                "span" => changed.span = Some((0, text.len() as u64)),
                _ => unreachable!(),
            }
            let mut payload = original.clone();
            payload.detail = Some(serde_json::to_string(&changed).unwrap());
            fixture.tamper_observation_payload(&object_id, &serde_json::to_vec(&payload).unwrap());
            let result = fixture.store.export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                window,
                None,
                roomy(),
            );
            if invalidation_only && field == "payload_id" {
                let page = result.unwrap();
                assert!(
                    page.rows[0].text.is_none(),
                    "absent text has no payload verification"
                );
            } else if result
                != Err(SourceExportError::MalformedRow {
                    object_id: object_id.clone(),
                })
            {
                accepted.push((invalidation_only, field));
            }
        }
        fixture.tamper_observation_payload(&object_id, &original_payload);
        assert_eq!(
            fixture
                .store
                .export_source_page(
                    &hold.binding,
                    &hold.hold_id,
                    hold.captured_at,
                    window,
                    None,
                    roomy(),
                )
                .unwrap(),
            baseline
        );
    }
    assert!(
        accepted.is_empty(),
        "accepted corrupt descriptor fields (invalidation-only, field): {accepted:?}"
    );
}

#[test]
fn byte_admission_stops_raw_candidate_reads_at_the_deferred_row() {
    let mut fixture = Fixture::open();
    for index in 0..20 {
        fixture.publish("messages", &format!("row-{index}"), 1, "four");
    }
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    for bounds in [page_bounds(64, 4), page_bounds(1, 1024)] {
        kernel::KernelStore::take_source_export_row_reads_for_test();
        let page = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                None,
                bounds,
            )
            .unwrap();
        let raw_reads = kernel::KernelStore::take_source_export_row_reads_for_test();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.charge.decoded_bytes, 4);
        assert!(page.next.is_some());
        assert_eq!(
            raw_reads, 2,
            "only the admitted row and first deferred raw row may be fetched"
        );
    }
}

#[test]
fn snapshot_pages_hide_invalidations_after_s() {
    let mut fixture = Fixture::open();
    let first = fixture.publish("canonical_claims", "first", 1, "first");
    let corrected = fixture.publish("messages", "corrected", 1, "original message");
    let retired = fixture.publish("raw_tool_spans", "retired", 1, "original tool");
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let bounds = page_bounds(1, 1024);
    let before = walk(&mut fixture, &hold, ExportWindow::Snapshot, bounds);
    assert_eq!(before.len(), 3);
    assert_eq!(before[0].rows[0].object_id, first);
    let after = all_pages(
        &mut fixture,
        &hold,
        ExportWindow::Snapshot,
        bounds,
        |fixture, page| {
            if page.rows[0].object_id == first {
                fixture.publish("messages", "corrected", 2, "replacement message");
                fixture.retire(&retired);
            }
        },
    );
    for object_id in [&corrected, &retired] {
        assert!(fixture.ledger[object_id].invalidated.unwrap() > hold.snapshot);
    }
    let invalidations: Vec<_> = after
        .iter()
        .flat_map(|page| page.rows.iter().map(|row| row.invalidated_commit_seq))
        .collect();
    assert_eq!(
        invalidations,
        vec![None; 3],
        "snapshot metadata cannot include invalidations after S"
    );
    assert_eq!(
        after, before,
        "snapshot pages are independent of read timing"
    );
}

#[test]
fn catch_up_pages_bound_invalidation_metadata_by_through() {
    let mut fixture = Fixture::open();
    let baseline = fixture.publish("canonical_claims", "baseline", 1, "baseline");
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let baseline_retirement = fixture.retire(&baseline);
    let corrected = fixture.publish("messages", "corrected", 1, "original message");
    let retired = fixture.publish("raw_tool_spans", "retired", 1, "original tool");
    let within = fixture.publish("promoted_memory", "within", 1, "within window");
    let through = fixture.retire(&within);
    fixture
        .store
        .extend_source_hold(&hold.binding, &hold.hold_id, through, wide_admission())
        .unwrap();
    let window = ExportWindow::CatchUp { through };
    let bounds = page_bounds(1, 1024);
    let before = walk(&mut fixture, &hold, window, bounds);
    assert_eq!(before.len(), 4);
    assert_eq!(before[0].rows[0].object_id, baseline);
    for (object_id, text, invalidated) in [
        (&baseline, None, Some(baseline_retirement)),
        (&corrected, Some("original message"), None),
        (&within, Some("within window"), Some(through)),
        (&retired, Some("original tool"), None),
    ] {
        let row = before
            .iter()
            .flat_map(|page| &page.rows)
            .find(|row| &row.object_id == object_id)
            .unwrap();
        assert_eq!(row.text.as_deref(), text);
        assert_eq!(row.invalidated_commit_seq, invalidated);
    }
    assert!(fixture.ledger[&within].created > hold.snapshot);
    let after = all_pages(&mut fixture, &hold, window, bounds, |fixture, page| {
        if page.rows[0].object_id == baseline {
            fixture.publish("messages", "corrected", 2, "replacement message");
            fixture.retire(&retired);
        }
    });
    for object_id in [&corrected, &retired] {
        assert!(fixture.ledger[object_id].invalidated.unwrap() > through);
    }
    let invalidations: Vec<_> = after
        .iter()
        .flat_map(|page| page.rows.iter().map(|row| row.invalidated_commit_seq))
        .collect();
    assert_eq!(
        invalidations,
        vec![Some(baseline_retirement), None, Some(through), None],
        "catch-up metadata preserves in-window invalidations and masks future ones"
    );
    assert_eq!(
        after, before,
        "catch-up pages are independent of read timing"
    );
}

#[test]
fn delta_pages_carry_exactly_one_step_of_the_catch_up_window() {
    let mut fixture = Fixture::open();
    let baseline = fixture.publish("canonical_claims", "baseline", 1, "baseline");
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let first = fixture.publish("messages", "first", 1, "first message");
    let middle = fixture.retire(&baseline);
    let second = fixture.publish("messages", "second", 1, "second message");
    let through = fixture.retire(&first);
    fixture
        .store
        .extend_source_hold(&hold.binding, &hold.hold_id, through, wide_admission())
        .unwrap();
    let bounds = page_bounds(8, 4096);

    // The first step creates `first` and retires `baseline`.
    let step_one = concatenated(&walk(
        &mut fixture,
        &hold,
        ExportWindow::Delta {
            after: hold.snapshot,
            through: middle,
        },
        bounds,
    ));
    assert_eq!(
        step_one
            .iter()
            .map(|row| row.object_id.as_str())
            .collect::<Vec<_>>(),
        vec![baseline.as_str(), first.as_str()]
    );
    // The second step creates `second` and retires `first`, which was live at
    // `middle`; `baseline` was already gone at `middle` and is not repeated.
    let step_two = walk(
        &mut fixture,
        &hold,
        ExportWindow::Delta {
            after: middle,
            through,
        },
        bounds,
    );
    let rows: Vec<_> = step_two.iter().flat_map(|page| &page.rows).collect();
    assert_eq!(rows.len(), 2);
    let retired_first = rows.iter().find(|row| row.object_id == first).unwrap();
    assert_eq!(retired_first.text, None, "created before the step, no text");
    assert_eq!(retired_first.invalidated_commit_seq, Some(through));
    let created_second = rows.iter().find(|row| row.object_id == second).unwrap();
    assert_eq!(created_second.text.as_deref(), Some("second message"));
    assert_eq!(created_second.invalidated_commit_seq, None);

    // Both steps together name every object the whole catch-up window names.
    let whole = concatenated(&walk(
        &mut fixture,
        &hold,
        ExportWindow::CatchUp { through },
        bounds,
    ));
    let mut stepped: Vec<String> = step_one
        .iter()
        .map(|row| row.object_id.clone())
        .chain(rows.iter().map(|row| row.object_id.clone()))
        .collect();
    stepped.sort();
    stepped.dedup();
    let mut named: Vec<String> = whole.iter().map(|row| row.object_id.clone()).collect();
    named.sort();
    assert_eq!(stepped, named);

    // A step must start at or after S and end at or before `through`.
    for after in [hold.snapshot - 1, through + 1] {
        let error = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Delta { after, through },
                None,
                bounds,
            )
            .unwrap_err();
        assert_eq!(
            error,
            SourceExportError::Hold(SourceHoldError::InvalidRequest),
            "after {after}"
        );
    }
}

#[test]
fn export_rejects_lifecycle_drift_even_when_observation_filters_would_hide_it() {
    let mut accepted = Vec::new();
    for mode in ["snapshot", "created", "invalidation"] {
        let mut fixture = Fixture::open();
        if mode != "created" {
            fixture.publish("messages", "lifecycle", 1, "lifecycle bytes");
        }
        fixture
            .store
            .commit(intent("before-lifecycle-hold"), |_| Ok(String::new()))
            .unwrap();
        let hold = fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .unwrap();
        if mode == "created" {
            fixture.publish("messages", "lifecycle", 1, "lifecycle bytes");
        }
        let object_id = fixture
            .live_entry("messages", "lifecycle")
            .object_id
            .clone();
        if mode == "invalidation" {
            fixture.retire(&object_id);
        }
        fixture
            .store
            .commit(intent("lifecycle-window-end"), |_| Ok(String::new()))
            .unwrap();
        let through = fixture.store.tip().unwrap();
        let window = if mode == "snapshot" {
            ExportWindow::Snapshot
        } else {
            fixture
                .store
                .extend_source_hold(&hold.binding, &hold.hold_id, through, wide_admission())
                .unwrap();
            ExportWindow::CatchUp { through }
        };
        let future = fixture
            .store
            .commit(intent("advance-lifecycle-tip"), |envelope| {
                envelope.register_outbox_consumer("other", 1)?;
                Ok(String::new())
            })
            .unwrap()
            .commit_seq;
        assert!(future > through);
        let original = fixture.ledger[&object_id].clone();
        let baseline = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                window,
                None,
                roomy(),
            )
            .unwrap();
        assert_eq!(baseline.rows.len(), 1);
        assert_eq!(baseline.rows[0].text.is_none(), mode == "invalidation");
        for field in ["creation", "invalidation", "future invalidation"] {
            let (created, invalidated) = match field {
                "creation" => (
                    if mode == "snapshot" {
                        future
                    } else {
                        hold.snapshot
                    },
                    original.invalidated,
                ),
                "invalidation" => (
                    original.created,
                    if mode == "invalidation" {
                        None
                    } else if mode == "snapshot" {
                        Some(hold.snapshot)
                    } else {
                        Some(through)
                    },
                ),
                "future invalidation" => (original.created, Some(future)),
                _ => unreachable!(),
            };
            fixture.tamper(
                "UPDATE observations SET created_commit_seq=?1,invalidated_commit_seq=?2 WHERE object_id=?3",
                rusqlite::params![created, invalidated, object_id],
            );
            let result = fixture.store.export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                window,
                None,
                roomy(),
            );
            if result
                != Err(SourceExportError::MalformedRow {
                    object_id: object_id.clone(),
                })
            {
                accepted.push((
                    mode,
                    field,
                    result.as_ref().map(|page| page.rows.len()).ok(),
                ));
            }
            fixture.tamper(
                "UPDATE observations SET created_commit_seq=?1,invalidated_commit_seq=?2 WHERE object_id=?3",
                rusqlite::params![original.created, original.invalidated, object_id],
            );
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted lifecycle drift (mode, field, returned rows): {accepted:?}"
    );
}

#[test]
fn cached_artifact_length_is_checked_for_each_evidence_row() {
    let mut fixture = Fixture::open();
    let first = fixture.retain("length-first", "four");
    let second = fixture.retain("length-second", "four");
    assert_ne!(first.0, second.0);
    assert_eq!(first.1, second.1);
    fixture.publish_span("canonical_claims", "short", 1, "four", Some((0, 1)), first);
    let evidence_id = second.0.clone();
    let object_id = fixture.publish_over("messages", "whole", 1, "four", second);
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    fixture.tamper(
        "UPDATE evidence_meta SET byte_length=2 WHERE evidence_id=?1",
        [&evidence_id],
    );
    let tight = SourcePageBounds {
        max_row_bytes: NonZeroU64::new(2).unwrap(),
        max_encoded_bytes: NonZeroU64::new(4).unwrap(),
        ..page_bounds(2, 3)
    };
    assert_eq!(
        fixture.store.export_source_page(
            &hold.binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::Snapshot,
            None,
            tight,
        ),
        Err(SourceExportError::BytesUnavailable {
            object_id,
            kind: ArtifactErrorKind::CorruptObject,
        }),
        "the cached second row must not export four bytes after admitting only two"
    );
}

#[test]
fn terminal_page_rechecks_hold_after_the_read_transaction() {
    let mut accepted = Vec::new();
    for invalidity in [
        SourceHoldInvalidity::Released,
        SourceHoldInvalidity::Expired,
        SourceHoldInvalidity::PurgeDegraded,
    ] {
        let mut fixture = Fixture::open();
        let object_id = fixture.publish("messages", "terminal", 1, "readable bytes");
        let hold = fixture
            .store
            .capture_source_hold(&fixture.binding(), wide())
            .unwrap();
        let baseline = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                None,
                roomy(),
            )
            .unwrap();
        assert_eq!(baseline.rows.len(), 1);
        assert!(baseline.next.is_none());
        let now = if invalidity == SourceHoldInvalidity::Expired {
            hold.expires_at - 1
        } else {
            hold.captured_at
        };
        let store = std::sync::Arc::clone(&fixture.store);
        let hook_store = std::sync::Arc::clone(&fixture.store);
        let digest = fixture.ledger[&object_id].digest.clone();
        let object_path = fixture.object_path(&digest);
        let held = hold.clone();
        let (ran_tx, ran_rx) = std::sync::mpsc::channel();
        let result = kernel::KernelStore::with_source_export_after_snapshot_hook_for_test(
            move || {
                match invalidity {
                    SourceHoldInvalidity::Released => hook_store
                        .release_source_hold(&held.binding, &held.hold_id, held.captured_at)
                        .unwrap(),
                    SourceHoldInvalidity::Expired => {
                        let remaining = u64::try_from(held.expires_at - now).unwrap();
                        let started = std::time::Instant::now();
                        std::thread::sleep(std::time::Duration::from_millis(remaining + 1));
                        assert!(started.elapsed().as_millis() >= u128::from(remaining));
                    }
                    SourceHoldInvalidity::PurgeDegraded => {
                        let error = hook_store
                            .delete_artifact_with_fault_for_test(
                                kernel::ArtifactDeletionRequest {
                                    intent: intent("purge-during-export"),
                                    identity: kernel::ArtifactDeletionIdentity::Digest(
                                        digest.clone(),
                                    ),
                                    kind: ArtifactDeletionKind::Purge,
                                    operator_id: Some("operator".to_string()),
                                    target_locator: Some("incident://export".to_string()),
                                    reason: Some("retired".to_string()),
                                    deleted_at: held.captured_at,
                                },
                                kernel::ArtifactDeletionFault::AfterCommit,
                            )
                            .unwrap_err();
                        assert_eq!(error.kind(), ArtifactErrorKind::PurgeUnlinkPending);
                    }
                    _ => unreachable!(),
                }
                assert_eq!(fs::read(&object_path).unwrap(), b"readable bytes");
                ran_tx.send(()).unwrap();
            },
            || {
                store.export_source_page(
                    &hold.binding,
                    &hold.hold_id,
                    now,
                    ExportWindow::Snapshot,
                    None,
                    roomy(),
                )
            },
        );
        ran_rx.try_recv().unwrap();
        if result
            != Err(SourceExportError::Hold(SourceHoldError::Invalid(
                invalidity,
            )))
        {
            accepted.push((
                invalidity,
                result.as_ref().map(|page| page.next.is_none()).ok(),
            ));
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted invalid holds (invalidity, terminal page): {accepted:?}"
    );
}

fn page_bounds(max_rows: usize, max_decoded_bytes: u64) -> SourcePageBounds {
    SourcePageBounds {
        max_rows: NonZeroUsize::new(max_rows).unwrap(),
        max_encoded_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_decoded_bytes: NonZeroU64::new(max_decoded_bytes).unwrap(),
        max_row_bytes: NonZeroU64::new(1 << 16).unwrap(),
    }
}

/// Rewrites an object file the way the store keeps it: owner-only, so the
/// store's exclusive-ownership check still admits the file.
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn roomy() -> SourcePageBounds {
    page_bounds(64, 1 << 20)
}

/// What the ledger predicts a row exports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Expected {
    class: String,
    object_id: String,
    revision: i64,
    created: i64,
    evidence_id: String,
    digest: String,
    identity: Vec<(String, String)>,
    representation: &'static str,
    span: Option<(u64, u64)>,
    text: Option<String>,
    invalidated: Option<i64>,
}

fn expected_row(entry: &LedgerEntry, with_text: bool, end: i64) -> Expected {
    Expected {
        class: entry.class.clone(),
        object_id: entry.object_id.clone(),
        revision: entry.revision,
        created: entry.created,
        evidence_id: entry.evidence_id.clone(),
        digest: entry.digest.clone(),
        identity: identity(&entry.class, &entry.key),
        representation: representation(&entry.class),
        span: entry.span,
        text: with_text.then(|| entry.selected_text().to_string()),
        invalidated: entry.invalidated.filter(|at| *at <= end),
    }
}

fn observed_row(row: &SourceRow) -> Expected {
    Expected {
        class: row.detail.class.clone(),
        object_id: row.object_id.clone(),
        revision: row.revision,
        created: row.created_commit_seq,
        evidence_id: row.detail.evidence_id.clone(),
        digest: row.detail.artifact_digest.clone(),
        identity: row.detail.identity.clone(),
        representation: match row.detail.representation.as_str() {
            "text" => "text",
            "decision_summary" => "decision_summary",
            "summary" => "summary",
            "commit_message" => "commit_message",
            "tool_output" => "tool_output",
            other => panic!("unknown representation {other}"),
        },
        span: row.detail.span,
        text: row.text.clone(),
        invalidated: row.invalidated_commit_seq,
    }
}

fn sorted(mut rows: Vec<Expected>) -> Vec<Expected> {
    rows.sort_by(|a, b| {
        (&a.class, &a.object_id, a.revision).cmp(&(&b.class, &b.object_id, b.revision))
    });
    rows
}

/// The ledger's fixed-S inventory: descriptors and evidence both live at S,
/// every row with text.
fn expected_snapshot(fixture: &Fixture, snapshot: i64) -> Vec<Expected> {
    let live = |at: Option<i64>| at.is_none_or(|at| at > snapshot);
    sorted(
        fixture
            .ledger
            .values()
            .filter(|e| {
                e.created <= snapshot && live(e.invalidated) && live(e.evidence_invalidated)
            })
            .map(|e| expected_row(e, true, snapshot))
            .collect(),
    )
}

/// The ledger's catch-up inventory for `(snapshot, through]`.
fn expected_catch_up(fixture: &Fixture, snapshot: i64, through: i64) -> Vec<Expected> {
    sorted(
        fixture
            .ledger
            .values()
            .filter_map(|e| {
                let created_in_window = e.created > snapshot
                    && e.created <= through
                    && e.evidence_invalidated.is_none_or(|at| at > through);
                let invalidated_in_window = e.created <= snapshot
                    && e.invalidated
                        .is_some_and(|at| at > snapshot && at <= through)
                    && e.evidence_invalidated.is_none_or(|at| at > snapshot);
                if created_in_window {
                    Some(expected_row(e, true, through))
                } else if invalidated_in_window {
                    Some(expected_row(e, false, through))
                } else {
                    None
                }
            })
            .collect(),
    )
}

/// Every page of `window` at `bounds`, with `between` run after each page so
/// the test can mutate sources while paging.
fn all_pages(
    fixture: &mut Fixture,
    hold: &SourceHold,
    window: ExportWindow,
    bounds: SourcePageBounds,
    mut between: impl FnMut(&mut Fixture, &SourcePage),
) -> Vec<SourcePage> {
    let mut cursor = None;
    let mut pages = Vec::new();
    loop {
        let page = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                window,
                cursor.as_ref(),
                bounds,
            )
            .unwrap();
        assert!(page.rows.len() <= bounds.max_rows.get());
        assert!(page.charge.decoded_bytes <= bounds.max_decoded_bytes.get());
        assert!(page.charge.encoded_bytes <= bounds.max_encoded_bytes.get());
        assert_eq!(page.charge.rows, page.rows.len());
        assert_eq!(
            page.charge.decoded_bytes,
            page.rows
                .iter()
                .map(|r| r.text.as_ref().map_or(0, |t| t.len() as u64))
                .sum::<u64>()
        );
        between(fixture, &page);
        let next = page.next.clone();
        pages.push(page);
        match next {
            Some(next) => cursor = Some(next),
            None => break pages,
        }
    }
}

fn walk(
    fixture: &mut Fixture,
    hold: &SourceHold,
    window: ExportWindow,
    bounds: SourcePageBounds,
) -> Vec<SourcePage> {
    all_pages(fixture, hold, window, bounds, |_, _| {})
}

fn concatenated(pages: &[SourcePage]) -> Vec<Expected> {
    pages
        .iter()
        .flat_map(|page| page.rows.iter().map(observed_row))
        .collect()
}

/// Sources exercising whitespace, Unicode, an empty selection, two spans in
/// one buffer, and two distinct sources with identical bytes.
fn seed_native_shapes(fixture: &mut Fixture) {
    fixture.publish("messages", "ws", 1, "  leading\ttab\r\n trailing  \n");
    fixture.publish("messages", "uni", 1, "héllo → 世界 🎉 \u{200b}zero-width");
    let buffer = "alpha β gamma";
    let shared = fixture.retain("spans", buffer);
    fixture.publish_span(
        "raw_tool_spans",
        "first",
        1,
        buffer,
        Some((0, 5)),
        shared.clone(),
    );
    fixture.publish_span(
        "raw_tool_spans",
        "second",
        1,
        buffer,
        Some((6, 8)),
        shared.clone(),
    );
    fixture.publish_span("raw_tool_spans", "empty", 1, buffer, Some((6, 6)), shared);
    fixture.publish("promoted_memory", "twin-a", 1, "identical bytes");
    fixture.publish("promoted_memory", "twin-b", 1, "identical bytes");
}

#[test]
fn concatenated_pages_equal_the_five_class_ledger_at_s_while_sources_mutate() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    seed_native_shapes(&mut fixture);
    fixture.publish("messages", "a", 2, "messages text a v2");
    let retired = fixture
        .live_entry("canonical_claims", "b")
        .object_id
        .clone();
    fixture.retire(&retired);
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let expected = expected_snapshot(&fixture, hold.snapshot);
    assert!(expected.len() >= 16);
    assert_eq!(
        expected
            .iter()
            .map(|e| e.class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        CLASSES.len()
    );

    let mut mid_walk = 0;
    let walked = all_pages(
        &mut fixture,
        &hold,
        ExportWindow::Snapshot,
        page_bounds(1, 1 << 20),
        |fixture, page| {
            mid_walk += 1;
            let read = &page.rows[0];
            if mid_walk == 1 {
                assert_eq!(
                    read.detail.class, "canonical_claims",
                    "first page is the first class"
                );
                fixture.publish("messages", "mid", 1, "published mid-walk");
                fixture.publish("canonical_claims", "a", 2, "revised after being read");
                fixture.publish("messages", "uni", 2, "revised before being read");
            }
            if mid_walk == 2 {
                let ahead = fixture
                    .live_entry("promoted_memory", "twin-b")
                    .object_id
                    .clone();
                fixture.retire(&ahead);
            }
        },
    );
    assert_eq!(concatenated(&walked), expected);
    for bounds in [page_bounds(1, 1 << 20), page_bounds(3, 40), roomy()] {
        let pages = walk(&mut fixture, &hold, ExportWindow::Snapshot, bounds);
        assert_eq!(concatenated(&pages), expected, "{bounds:?}");
        assert!(pages.last().unwrap().next.is_none());
        assert!(
            !pages.last().unwrap().rows.is_empty(),
            "no empty final page"
        );
        let resume = pages
            .iter()
            .rev()
            .nth(1)
            .and_then(|page| page.next.as_ref());
        let last_again = fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                resume,
                bounds,
            )
            .unwrap();
        assert_eq!(&last_again, pages.last().unwrap());
    }
    let observed = expected.clone();
    // Every class boundary is crossed inside the walk.
    assert_eq!(
        observed
            .iter()
            .map(|e| e.class.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        CLASSES.len()
    );

    // Native shapes are exact: whitespace, Unicode, spans, an empty selection,
    // and two distinct sources with identical bytes.
    let text_of = |key: &str, class: &str| {
        let entry = fixture
            .ledger
            .values()
            .find(|e| {
                e.class == class
                    && e.key == key
                    && e.created <= hold.snapshot
                    && e.invalidated.is_none_or(|at| at > hold.snapshot)
            })
            .unwrap();
        observed
            .iter()
            .find(|r| r.object_id == entry.object_id)
            .unwrap()
            .text
            .clone()
            .unwrap()
    };
    assert_eq!(
        text_of("uni", "messages"),
        "héllo → 世界 🎉 \u{200b}zero-width"
    );
    assert_eq!(text_of("first", "raw_tool_spans"), "alpha");
    assert_eq!(text_of("empty", "raw_tool_spans"), "");
    assert_eq!(text_of("twin-a", "promoted_memory"), "identical bytes");
    let ws = fixture
        .ledger
        .values()
        .find(|e| e.key == "ws" && e.revision == 1)
        .unwrap();
    assert_eq!(
        observed
            .iter()
            .find(|r| r.object_id == ws.object_id)
            .unwrap()
            .text
            .as_deref(),
        Some("  leading\ttab\r\n trailing  \n")
    );
    let twins: Vec<&Expected> = observed
        .iter()
        .filter(|r| r.text.as_deref() == Some("identical bytes"))
        .collect();
    assert_eq!(twins.len(), 2);
    assert_ne!(twins[0].object_id, twins[1].object_id);
    assert_ne!(twins[0].identity, twins[1].identity);
}

#[test]
fn logical_admission_precedes_materialization_and_decode() {
    let mut fixture = Fixture::open();
    // Many small rows over one shared buffer and over their own buffers.
    let buffer: String = (0..40).map(|i| format!("{i:02}")).collect();
    let shared = fixture.retain("many", &buffer);
    for i in 0..20u64 {
        fixture.publish_span(
            "raw_tool_spans",
            &format!("piece{i:02}"),
            1,
            &buffer,
            Some((i * 4, i * 4 + 4)),
            shared.clone(),
        );
    }
    for i in 0..6 {
        fixture.publish(
            "messages",
            &format!("own{i}"),
            1,
            &format!("own message {i}"),
        );
    }
    let hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let expected = expected_snapshot(&fixture, hold.snapshot);

    // Decoded-byte bounds defer whole rows; encoded bytes charge a shared
    // buffer once per page; every row appears exactly once.
    let bounds = page_bounds(64, 13);
    let pages = walk(&mut fixture, &hold, ExportWindow::Snapshot, bounds);
    assert!(pages.len() > 4);
    for page in &pages {
        let distinct: BTreeSet<&str> = page
            .rows
            .iter()
            .map(|r| r.detail.artifact_digest.as_str())
            .collect();
        let encoded: u64 = distinct
            .iter()
            .map(|digest| {
                fixture
                    .ledger
                    .values()
                    .find(|e| e.digest == *digest)
                    .unwrap()
                    .text
                    .len() as u64
            })
            .sum();
        assert_eq!(
            page.charge.encoded_bytes, encoded,
            "shared buffer charged once per page"
        );
    }
    assert_eq!(concatenated(&pages), expected);

    // Encoded bytes bound a page too: with room for exactly the shared
    // buffer, the six own rows fill one page and the first shared-buffer row
    // is deferred whole, so the second page charges the shared buffer once.
    let encoded_pages = walk(
        &mut fixture,
        &hold,
        ExportWindow::Snapshot,
        SourcePageBounds {
            max_encoded_bytes: NonZeroU64::new(80).unwrap(),
            ..roomy()
        },
    );
    assert_eq!(encoded_pages.len(), 2);
    assert_eq!(
        (
            encoded_pages[0].rows.len(),
            encoded_pages[0].charge.encoded_bytes
        ),
        (6, 78)
    );
    assert_eq!(
        (
            encoded_pages[1].rows.len(),
            encoded_pages[1].charge.encoded_bytes
        ),
        (20, 80)
    );
    assert_eq!(concatenated(&encoded_pages), expected);

    // Admission before decode: the third own row is deferred by the decoded
    // bound after it was preflighted. Its object is removed before the page
    // is exported, and the page still succeeds because a deferred row is
    // never read; the next page then fails at exactly that row.
    let two_rows = page_bounds(64, 26);
    let first = fixture
        .store
        .export_source_page(
            &hold.binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::Snapshot,
            None,
            two_rows,
        )
        .unwrap();
    assert_eq!(first.rows.len(), 2);
    let victim = expected[2].clone();
    assert_eq!(victim.class, "messages");
    let victim_path = fixture.object_path(&victim.digest);
    let victim_bytes = fs::read(&victim_path).unwrap();
    fs::remove_file(&victim_path).unwrap();
    let again = fixture
        .store
        .export_source_page(
            &hold.binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::Snapshot,
            None,
            two_rows,
        )
        .unwrap();
    assert_eq!(again, first, "the deferred row was never read");
    assert_eq!(
        fixture
            .store
            .export_source_page(
                &hold.binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                first.next.as_ref(),
                two_rows,
            )
            .unwrap_err(),
        SourceExportError::BytesUnavailable {
            object_id: victim.object_id.clone(),
            kind: ArtifactErrorKind::MissingObject,
        }
    );
    write_owner_only(&victim_path, &victim_bytes);

    // An oversized row fails before decode: its bytes are gone, yet the
    // refusal is about size, not about the missing object, and nothing was
    // skipped to make progress.
    let big = "x".repeat(300);
    fixture.publish("messages", "big", 1, &big);
    let big_hold = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    let big_entry = fixture.live_entry("messages", "big").clone();
    fs::remove_file(fixture.object_path(&big_entry.digest)).unwrap();
    let tight = SourcePageBounds {
        max_row_bytes: NonZeroU64::new(100).unwrap(),
        ..roomy()
    };
    let mut cursor = None;
    let error = loop {
        match fixture.store.export_source_page(
            &big_hold.binding,
            &big_hold.hold_id,
            big_hold.captured_at,
            ExportWindow::Snapshot,
            cursor.as_ref(),
            tight,
        ) {
            Ok(page) => cursor = Some(page.next.expect("the oversized row is still ahead")),
            Err(error) => break error,
        }
    };
    assert_eq!(
        error,
        SourceExportError::OversizedRow {
            object_id: big_entry.object_id.clone(),
            bound: PageBound::Row,
            bytes: 300,
        }
    );
    // The same row is oversized against the page's decoded and encoded bounds.
    for (bounds, bound) in [
        (
            SourcePageBounds {
                max_decoded_bytes: NonZeroU64::new(100).unwrap(),
                ..roomy()
            },
            PageBound::Decoded,
        ),
        (
            SourcePageBounds {
                max_encoded_bytes: NonZeroU64::new(100).unwrap(),
                ..roomy()
            },
            PageBound::Encoded,
        ),
    ] {
        let mut cursor = None;
        let error = loop {
            match fixture.store.export_source_page(
                &big_hold.binding,
                &big_hold.hold_id,
                big_hold.captured_at,
                ExportWindow::Snapshot,
                cursor.as_ref(),
                bounds,
            ) {
                Ok(page) => cursor = Some(page.next.expect("the oversized row is still ahead")),
                Err(error) => break error,
            }
        };
        assert_eq!(
            error,
            SourceExportError::OversizedRow {
                object_id: big_entry.object_id.clone(),
                bound,
                bytes: 300,
            }
        );
    }
    assert_eq!(fixture.checkpoint(), 0, "no export moved the consumer");
}

#[test]
fn dead_holds_and_lost_history_prevent_completion_and_a_reopen_needs_a_new_s() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let export = |fixture: &Fixture, hold: &SourceHold, now: i64| {
        fixture.store.export_source_page(
            &binding,
            &hold.hold_id,
            now,
            ExportWindow::Snapshot,
            None,
            roomy(),
        )
    };
    let invalid = |kind| Err(SourceExportError::Hold(SourceHoldError::Invalid(kind)));

    let expiring = fixture
        .store
        .capture_source_hold(&binding, bounds(HOUR_MS))
        .unwrap();
    assert!(export(&fixture, &expiring, expiring.captured_at).is_ok());
    assert_eq!(
        export(&fixture, &expiring, expiring.expires_at),
        invalid(SourceHoldInvalidity::Expired)
    );

    let released = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    fixture
        .store
        .release_source_hold(&binding, &released.hold_id, 1)
        .unwrap();
    assert_eq!(
        export(&fixture, &released, released.captured_at),
        invalid(SourceHoldInvalidity::Released)
    );

    let degraded = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let purged = fixture.held_all(&degraded, 64)[0].evidence_id.clone();
    fixture.delete_evidence(&purged, ArtifactDeletionKind::Purge, 42);
    assert_eq!(
        export(&fixture, &degraded, degraded.captured_at),
        invalid(SourceHoldInvalidity::PurgeDegraded)
    );

    // Hold-phase and page-phase kernel errors share one variant.
    let unreadable = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    fixture.tamper_hold_expiry(&unreadable.hold_id);
    assert_eq!(
        export(&fixture, &unreadable, unreadable.captured_at),
        Err(SourceExportError::Kernel(KernelError::CorruptCanonicalRow))
    );

    // Lost history: missing bytes and bytes that no longer hash to the digest
    // both refuse the page; nothing partial is handed back.
    let hold = fixture.store.capture_source_hold(&binding, wide()).unwrap();
    let full = export(&fixture, &hold, hold.captured_at).unwrap();
    assert!(full.rows.len() >= 9);
    let gone = full.rows[3].clone();
    let path = fixture.object_path(&gone.detail.artifact_digest);
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(
        export(&fixture, &hold, hold.captured_at),
        Err(SourceExportError::BytesUnavailable {
            object_id: gone.object_id.clone(),
            kind: ArtifactErrorKind::MissingObject,
        })
    );
    let mut forged = original.clone();
    forged[0] ^= 0x01;
    write_owner_only(&path, &forged);
    assert_eq!(
        export(&fixture, &hold, hold.captured_at),
        Err(SourceExportError::BytesUnavailable {
            object_id: gone.object_id.clone(),
            kind: ArtifactErrorKind::CorruptObject,
        })
    );
    write_owner_only(&path, &original);
    assert_eq!(export(&fixture, &hold, hold.captured_at).unwrap(), full);

    // Lost history in the canonical row itself: a stored detail whose span
    // runs past the retained bytes, or that no longer parses, refuses the
    // page as malformed rather than exporting a guess.
    let tampered = full.rows[5].clone();
    let original_payload: Vec<u8> = fixture
        .inspect()
        .query_row(
            "SELECT observation_payload FROM observations WHERE object_id=?1",
            [&tampered.object_id],
            |row| row.get(0),
        )
        .unwrap();
    let mut payload: ObservationPayload = serde_json::from_slice(&original_payload).unwrap();
    let mut detail: SourceDescriptorDetail =
        serde_json::from_str(payload.detail.as_deref().unwrap()).unwrap();
    assert_eq!(detail.span, None);
    detail.span = Some((0, 4096));
    payload.detail = Some(serde_json::to_string(&detail).unwrap());
    fixture.tamper_observation_payload(&tampered.object_id, &serde_json::to_vec(&payload).unwrap());
    assert_eq!(
        export(&fixture, &hold, hold.captured_at),
        Err(SourceExportError::MalformedRow {
            object_id: tampered.object_id.clone(),
        })
    );
    fixture.tamper_observation_payload(&tampered.object_id, b"not json");
    assert_eq!(
        export(&fixture, &hold, hold.captured_at),
        Err(SourceExportError::MalformedRow {
            object_id: tampered.object_id.clone(),
        })
    );
    // Export rejects a detail whose revision or lineage id differs from the
    // stored row.
    let genuine: ObservationPayload = serde_json::from_slice(&original_payload).unwrap();
    let genuine_detail: SourceDescriptorDetail =
        serde_json::from_str(genuine.detail.as_deref().unwrap()).unwrap();
    let wrong_revision = SourceDescriptorDetail {
        revision: "99".to_string(),
        ..genuine_detail.clone()
    };
    let wrong_lineage = SourceDescriptorDetail {
        lineage_id: "forged".to_string(),
        ..genuine_detail
    };
    for (name, detail) in [("revision", wrong_revision), ("lineage", wrong_lineage)] {
        let mut payload = genuine.clone();
        payload.detail = Some(serde_json::to_string(&detail).unwrap());
        fixture.tamper_observation_payload(
            &tampered.object_id,
            &serde_json::to_vec(&payload).unwrap(),
        );
        assert_eq!(
            export(&fixture, &hold, hold.captured_at),
            Err(SourceExportError::MalformedRow {
                object_id: tampered.object_id.clone(),
            }),
            "tampered {name}"
        );
    }
    fixture.tamper_observation_payload(&tampered.object_id, &original_payload);
    assert_eq!(export(&fixture, &hold, hold.captured_at).unwrap(), full);

    // A reopened store refuses the old binding and the old cursor; the old
    // hold is refused under the new binding too, and a new S is captured.
    let old_binding = binding.clone();
    let cursor = fixture
        .store
        .export_source_page(
            &old_binding,
            &hold.hold_id,
            hold.captured_at,
            ExportWindow::Snapshot,
            None,
            page_bounds(1, 1 << 20),
        )
        .unwrap()
        .next;
    assert!(cursor.is_some());
    let mut fixture = fixture.reopen();
    assert_eq!(
        fixture
            .store
            .export_source_page(
                &old_binding,
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                cursor.as_ref(),
                roomy(),
            )
            .unwrap_err(),
        SourceExportError::Hold(SourceHoldError::IncarnationMismatch)
    );
    fixture.store.reconcile_source_holds(CONSUMER, 5).unwrap();
    assert_eq!(
        fixture
            .store
            .export_source_page(
                &fixture.binding(),
                &hold.hold_id,
                hold.captured_at,
                ExportWindow::Snapshot,
                cursor.as_ref(),
                roomy(),
            )
            .unwrap_err(),
        SourceExportError::Hold(SourceHoldError::BindingMismatch)
    );
    fixture.publish("messages", "after-reopen", 1, "after reopen");
    let fresh = fixture
        .store
        .capture_source_hold(&fixture.binding(), wide())
        .unwrap();
    assert!(fresh.snapshot > hold.snapshot);
    let pages = walk(&mut fixture, &fresh, ExportWindow::Snapshot, roomy());
    assert_eq!(
        concatenated(&pages),
        expected_snapshot(&fixture, fresh.snapshot)
    );
    assert_eq!(fixture.checkpoint(), 0);
}

#[test]
fn catch_up_pages_cover_the_window_through_t_and_bytes_outlive_acknowledgement() {
    let mut fixture = Fixture::open();
    fixture.seed_five_classes();
    let binding = fixture.binding();
    let hold = fixture
        .store
        .capture_source_hold(&binding, bounds(MAX_SOURCE_HOLD_LIFETIME_MS))
        .unwrap();
    // The window holds published commits, a control commit with no descriptor,
    // a revision of an S row, a retirement of an S row, and a row created and
    // superseded inside the window.
    fixture.publish("messages", "late", 1, "late one");
    fixture
        .store
        .commit(intent("control"), |envelope| {
            envelope.register_outbox_consumer("control-consumer", 1)?;
            Ok(String::new())
        })
        .unwrap();
    fixture.publish("messages", "a", 2, "messages text a v2");
    let retired = fixture
        .live_entry("canonical_claims", "b")
        .object_id
        .clone();
    fixture.retire(&retired);
    fixture.publish("git_commits", "flip", 1, "flip one");
    fixture.publish("git_commits", "flip", 2, "flip two");
    // Evidence invalidated inside the window: a row created in the window
    // whose bytes are deleted before T is no candidate; an S row retired in
    // the window keeps its invalidation fact whatever happens to its bytes
    // afterwards.
    fixture.publish(
        "promoted_memory",
        "gone",
        1,
        "created then deleted in window",
    );
    let gone = fixture
        .live_entry("promoted_memory", "gone")
        .evidence_id
        .clone();
    fixture.delete_evidence(&gone, ArtifactDeletionKind::Delete, wall_ms());
    let retired_then_deleted = fixture.live_entry("raw_tool_spans", "a").clone();
    fixture.retire(&retired_then_deleted.object_id);
    fixture.delete_evidence(
        &retired_then_deleted.evidence_id,
        ArtifactDeletionKind::Delete,
        wall_ms(),
    );
    let t = fixture.store.tip().unwrap();
    fixture.publish("messages", "after-t", 1, "after T");
    let after_t = fixture.live_entry("messages", "after-t").object_id.clone();
    let tip = fixture.store.tip().unwrap();

    // The window must be within [S, tip] and covered by the hold.
    let catch_up = |through| ExportWindow::CatchUp { through };
    let page = |fixture: &Fixture, window, cursor: Option<&SourceCursor>| {
        fixture.store.export_source_page(
            &binding,
            &hold.hold_id,
            hold.captured_at,
            window,
            cursor,
            roomy(),
        )
    };
    assert_eq!(
        page(&fixture, catch_up(tip + 1), None).unwrap_err(),
        SourceExportError::Hold(SourceHoldError::InvalidRequest)
    );
    let (refs_through_t, _) = fixture.expected_refs_through(hold.snapshot, t);
    let uncovered = refs_through_t.len() - hold.references;
    assert!(uncovered >= 4);
    assert_eq!(
        page(&fixture, catch_up(t), None).unwrap_err(),
        SourceExportError::Hold(SourceHoldError::ExtensionIncomplete { uncovered })
    );
    fixture
        .store
        .extend_source_hold(&binding, &hold.hold_id, t, wide_admission())
        .unwrap();

    let expected = expected_catch_up(&fixture, hold.snapshot, t);
    assert!(
        expected.iter().any(|e| e.text.is_none()),
        "invalidation facts without text"
    );
    assert!(
        expected
            .iter()
            .any(|e| e.text.is_some() && e.invalidated.is_some()),
        "created and superseded inside the window"
    );
    assert!(
        !expected.iter().any(|e| e.evidence_id == gone),
        "evidence deleted inside the window is no candidate"
    );
    assert!(
        expected
            .iter()
            .any(|e| e.object_id == retired_then_deleted.object_id && e.text.is_none()),
        "the invalidation fact outlives the bytes"
    );
    for bounds in [page_bounds(1, 1 << 20), page_bounds(2, 20), roomy()] {
        let pages = walk(&mut fixture, &hold, catch_up(t), bounds);
        let observed = concatenated(&pages);
        assert_eq!(observed, expected, "{bounds:?}");
        assert!(!observed.iter().any(|r| r.object_id == after_t));
        assert!(pages.last().unwrap().next.is_none());
    }
    // Rows without text charge no decoded bytes.
    let pages = walk(&mut fixture, &hold, catch_up(t), roomy());
    let facts_only: Vec<&SourceRow> = pages
        .iter()
        .flat_map(|p| p.rows.iter())
        .filter(|r| r.text.is_none())
        .collect();
    assert!(!facts_only.is_empty());
    assert!(facts_only.iter().all(|r| {
        r.created_commit_seq <= hold.snapshot
            && r.invalidated_commit_seq
                .is_some_and(|at| at > hold.snapshot && at <= t)
    }));
    // An empty window exports nothing.
    let none = page(&fixture, catch_up(hold.snapshot), None).unwrap();
    assert!(none.rows.is_empty() && none.next.is_none());

    // Acknowledging through T does not move the bytes out from under the
    // pages: the same pages export after acknowledgement, publication, and a
    // sweep past the grace period, even when the evidence is deleted.
    fixture
        .store
        .acknowledge_through_source_hold(&binding, &hold.hold_id, t, 1)
        .unwrap();
    assert_eq!(fixture.checkpoint(), t);
    let doomed = fixture.live_entry("messages", "late").evidence_id.clone();
    fixture.delete_evidence(&doomed, ArtifactDeletionKind::Delete, wall_ms());
    // The control: evidence the hold never cited, deleted and acknowledged,
    // is reclaimed by the same sweep that leaves the held bytes alone.
    let control = fixture.live_entry("messages", "after-t").clone();
    fixture.delete_evidence(
        &control.evidence_id,
        ArtifactDeletionKind::Delete,
        wall_ms(),
    );
    for consumer in ["control-consumer", CONSUMER] {
        fixture
            .store
            .acknowledge_outbox(consumer, fixture.store.tip().unwrap(), 1)
            .unwrap();
    }
    fixture.publish_and_prune();
    let swept = fixture
        .store
        .run_staging_maintenance(wall_ms() + i64::try_from(15 * DAY_MS).unwrap())
        .unwrap();
    assert!(swept.artifact_gc.reclaimed_objects >= 1);
    assert!(
        !fixture.object_present(&control.digest),
        "unheld evidence collected"
    );
    let doomed_digest = fixture.entry_by_evidence(&doomed).digest.clone();
    assert!(fixture.object_present(&doomed_digest), "held evidence kept");
    let after = walk(&mut fixture, &hold, catch_up(t), roomy());
    assert_eq!(
        concatenated(&after),
        expected,
        "bytes retained across acknowledgement"
    );
    let snapshot_pages = walk(&mut fixture, &hold, ExportWindow::Snapshot, roomy());
    assert_eq!(
        concatenated(&snapshot_pages),
        expected_snapshot(&fixture, hold.snapshot)
    );
}
