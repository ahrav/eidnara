use kernel::{
    ArtifactDestination, CommitIntent, DomainSpec, EligibilityDeniedReason, KernelError,
    KernelStore, MAX_EXPIRED_USE_OUTCOMES_PER_CALL, Sensitivity, ServedCounts, SessionRole,
    USE_OUTCOME_RETENTION_CLASS, USE_OUTCOME_RETENTION_MS, UseOutcomeExpiry, UseOutcomeSpec,
    served_set_digest, use_outcome_id,
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::Digest;

const HARNESS: &str = "opencode";
const SECRET: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyzABCDEFGH12345678";

fn project() -> String {
    "a".repeat(64)
}

fn intent(key: &str) -> CommitIntent {
    CommitIntent {
        producer: "use-outcome-test".to_string(),
        operation_key: key.to_string(),
        request_digest: "d".repeat(64),
        actor: "test".to_string(),
        cause: "proof".to_string(),
    }
}

fn writer(root: &std::path::Path) -> Connection {
    let connection = Connection::open(root.join("kernel.sqlite")).unwrap();
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
}

fn count(root: &std::path::Path, sql: &str) -> i64 {
    writer(root).query_row(sql, [], |row| row.get(0)).unwrap()
}

fn store_with_objects(root: &std::path::Path, objects: &[&str]) -> KernelStore {
    let store = KernelStore::open(root).unwrap();
    store
        .commit(intent("domain"), |envelope| {
            envelope.insert_domain(DomainSpec {
                domain_id: "domain".to_string(),
                object_id: "domain-object".to_string(),
                name: "domain".to_string(),
                source_kind: "fixture".to_string(),
                source_id: "domain".to_string(),
                source_revision: 1,
                sensitivity: Sensitivity::Normal,
            })?;
            Ok(String::new())
        })
        .unwrap();
    let connection = writer(root);
    for object in objects {
        connection
            .execute(
                "INSERT INTO object_registry(
                     object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                     created_commit_seq,sensitivity_class
                 ) VALUES (?1,'decision','domain','fixture',?1,1,1,'normal')",
                [object],
            )
            .unwrap();
    }
    store
}

fn tick(store: &KernelStore, key: &str) -> i64 {
    store
        .commit(intent(key), |_| Ok(String::new()))
        .unwrap()
        .commit_seq
}

fn spec(
    session: &str,
    objects: &[&str],
    served_commit_seq: i64,
    recorded_at: i64,
) -> UseOutcomeSpec {
    let mut decision_object_ids: Vec<String> = objects.iter().map(|id| (*id).to_string()).collect();
    decision_object_ids.sort();
    UseOutcomeSpec {
        project_digest: project(),
        harness: HARNESS.to_string(),
        session_id: session.to_string(),
        served_set_digest: served_set_digest(objects.iter().copied()),
        composition_revision: u64::MAX - 1,
        served_commit_seq,
        session_role: SessionRole::Primary,
        recorded_at,
        decision_object_ids,
    }
}

fn insert(store: &KernelStore, key: &str, spec: UseOutcomeSpec) -> Result<String, KernelError> {
    let id = std::cell::RefCell::new(String::new());
    store.commit(intent(key), |envelope| {
        *id.borrow_mut() = envelope.insert_use_outcome(spec)?;
        Ok(String::new())
    })?;
    Ok(id.into_inner())
}

fn admission(
    root: &std::path::Path,
    id: &str,
    subject: &str,
    event_kind: &str,
    revision: i64,
    commit_seq: Option<i64>,
    use_outcome: Option<&str>,
) {
    admission_on(
        root,
        id,
        subject,
        event_kind,
        ("fixture", subject, revision),
        commit_seq,
        use_outcome,
    );
}

fn admission_on(
    root: &std::path::Path,
    id: &str,
    subject: &str,
    event_kind: &str,
    (source_kind, source_id, revision): (&str, &str, i64),
    commit_seq: Option<i64>,
    use_outcome: Option<&str>,
) {
    writer(root)
        .execute(
            "INSERT INTO admission_decisions(
                 admission_decision_id,subject_object_id,source_kind,source_id,source_revision,
                 source_class,taint_class,event_kind,maturity,effective_maturity,disposition,
                 visibility,outcome,sensitivity_class,policy_revision,reason,commit_seq,
                 decided_at,use_outcome_id
             ) VALUES (?1,?2,?7,?8,?3,'explicit_user','user_explicit',?4,'candidate',
                       'candidate','disputed','automatic','admit','normal',1,'fixture',?5,1,?6)",
            params![
                id,
                subject,
                revision,
                event_kind,
                commit_seq,
                use_outcome,
                source_kind,
                source_id
            ],
        )
        .unwrap();
}

/// The served-set read answers every record of the session with its ids, digest, revision, served sequence, outcome columns, and retirement state; a session with no record answers `None`, distinct from a record of an empty composition.
#[test]
fn served_set_answers_records_and_distinguishes_no_record_from_an_empty_serve() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1", "d2"]);
    let served_at = tick(&store, "tick");
    let first = insert(&store, "serve-1", spec("s1", &["d2", "d1"], served_at, 100)).unwrap();
    let empty = insert(&store, "serve-2", spec("s1", &[], served_at, 200)).unwrap();
    assert_eq!(
        first,
        use_outcome_id(&project(), HARNESS, "s1", &served_set_digest(["d1", "d2"]))
    );

    let records = store
        .served_set(&project(), HARNESS, "s1")
        .unwrap()
        .unwrap();
    assert_eq!(records.len(), 2);
    let serve = &records[0].serve;
    assert_eq!(serve.use_outcome_id, first);
    assert_eq!(records[0].decision_object_ids, ["d1", "d2"]);
    assert_eq!(serve.served_set_digest, served_set_digest(["d1", "d2"]));
    assert_eq!(serve.composition_revision, u64::MAX - 1);
    assert_eq!(serve.served_commit_seq, served_at);
    assert_eq!(serve.session_role, SessionRole::Primary);
    assert_eq!(serve.recorded_at, 100);
    assert_eq!(serve.retain_until, 100 + USE_OUTCOME_RETENTION_MS);
    assert_eq!(serve.invalidated_commit_seq, None);
    assert_eq!(serve.deleted_at, None);
    assert_eq!(serve.last_transform_at, None);
    assert_eq!(records[1].serve.use_outcome_id, empty);
    assert!(records[1].decision_object_ids.is_empty());

    assert_eq!(store.served_set(&project(), HARNESS, "s2").unwrap(), None);
    assert_eq!(store.served_set(&project(), "pi", "s1").unwrap(), None);
    assert_eq!(
        store.served_set(&"b".repeat(64), HARNESS, "s1").unwrap(),
        None
    );
    assert_eq!(
        count(
            root.path(),
            "SELECT COUNT(*) FROM use_outcomes
             WHERE retention_class='use_outcome_capture' AND sensitivity_class='sensitive'
               AND provider_egress_class='local_only'"
        ),
        2
    );
    assert_eq!(USE_OUTCOME_RETENTION_CLASS, "use_outcome_capture");
}

/// A duplicate identity is `Conflict`, an unregistered decision is `NotFound`, a malformed or secret-bearing spec is `InvalidInput`, and a refused commit leaves no row in any of the three use-outcome tables.
#[test]
fn duplicate_and_malformed_serves_are_refused_without_rows() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1", "d2"]);
    let served_at = tick(&store, "tick");
    insert(&store, "serve", spec("s1", &["d1"], served_at, 100)).unwrap();
    let rows = || {
        (
            count(root.path(), "SELECT COUNT(*) FROM use_outcomes"),
            count(root.path(), "SELECT COUNT(*) FROM use_outcome_decisions"),
            count(root.path(), "SELECT COUNT(*) FROM use_outcome_sessions"),
        )
    };
    assert_eq!(rows(), (1, 1, 1));

    assert_eq!(
        insert(&store, "again", spec("s1", &["d1"], served_at, 300)),
        Err(KernelError::Conflict),
        "one identity tuple has one record"
    );
    assert_eq!(
        insert(&store, "unknown", spec("s3", &["missing"], served_at, 100)),
        Err(KernelError::NotFound),
        "a decision row names a registry object"
    );
    writer(root.path())
        .execute(
            "INSERT INTO object_registry(
                 object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                 created_commit_seq,sensitivity_class
             ) VALUES ('o1','observation','domain','fixture','o1',1,1,'normal')",
            [],
        )
        .unwrap();
    assert_eq!(
        insert(
            &store,
            "observation",
            spec("s3", &["d1", "o1"], served_at, 100)
        ),
        Err(KernelError::InvalidInput),
        "a decision row names a decision object"
    );
    writer(root.path())
        .execute_batch(&format!(
            "INSERT INTO object_registry(
                 object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                 created_commit_seq,sensitivity_class
             ) VALUES ('d3','decision','domain','fixture','d3',1,{later},'normal');
             INSERT INTO object_registry(
                 object_id,object_kind,domain_id,source_kind,source_id,source_revision,
                 created_commit_seq,invalidated_commit_seq,sensitivity_class
             ) VALUES ('d4','decision','domain','fixture','d4',1,1,{served_at},'normal');",
            later = served_at + 1
        ))
        .unwrap();
    tick(&store, "later");
    for (context, ids) in [
        ("created after the serve", ["d1", "d3"]),
        ("retired at the serve", ["d1", "d4"]),
    ] {
        assert_eq!(
            insert(&store, context, spec("s3", &ids, served_at, 100)),
            Err(KernelError::InvalidInput),
            "{context}"
        );
    }
    let next = store.tip().unwrap() + 1;
    let with_secret_id = || {
        let mut ids = vec!["d1".to_string(), SECRET.to_string()];
        ids.sort();
        ids
    };
    type Corrupt = Box<dyn Fn(&mut UseOutcomeSpec)>;
    let malformed: Vec<(&str, Corrupt)> = vec![
        ("empty harness", Box::new(|s| s.harness.clear())),
        ("empty session", Box::new(|s| s.session_id.clear())),
        ("blank harness", Box::new(|s| s.harness = " \t".to_string())),
        (
            "secret session",
            Box::new(|s| s.session_id = SECRET.to_string()),
        ),
        (
            "secret decision id",
            Box::new(move |s| {
                s.decision_object_ids = with_secret_id();
                s.served_set_digest =
                    served_set_digest(s.decision_object_ids.iter().map(String::as_str));
            }),
        ),
        (
            "non-hex project",
            Box::new(|s| s.project_digest = "z".repeat(64)),
        ),
        (
            "uppercase digest",
            Box::new(|s| s.served_set_digest = s.served_set_digest.to_uppercase()),
        ),
        (
            "short digest",
            Box::new(|s| s.served_set_digest.truncate(63)),
        ),
        (
            "digest of another set",
            Box::new(|s| s.served_set_digest = served_set_digest(["d1"])),
        ),
        (
            "unsorted ids",
            Box::new(|s| s.decision_object_ids.reverse()),
        ),
        (
            "repeated id",
            Box::new(|s| s.decision_object_ids = vec!["d1".into(), "d1".into()]),
        ),
        (
            "negative served sequence",
            Box::new(|s| s.served_commit_seq = -1),
        ),
        (
            "zero served sequence",
            Box::new(|s| s.served_commit_seq = 0),
        ),
        (
            "future served sequence",
            Box::new(move |s| s.served_commit_seq = next),
        ),
        ("negative recorded time", Box::new(|s| s.recorded_at = -1)),
        (
            "recorded time past the clock skew",
            Box::new(|s| {
                s.recorded_at = i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis(),
                )
                .unwrap()
                    + 10 * 60 * 1_000;
            }),
        ),
        (
            "far-future recorded time",
            Box::new(|s| s.recorded_at = i64::MAX),
        ),
    ];
    for (index, (context, corrupt)) in malformed.into_iter().enumerate() {
        let mut bad = spec("s2", &["d1", "d2"], served_at, 100);
        corrupt(&mut bad);
        assert_eq!(
            insert(&store, &format!("bad-{index}"), bad),
            Err(KernelError::InvalidInput),
            "{context}"
        );
    }
    assert_eq!(rows(), (1, 1, 1));
    assert_eq!(
        count(
            root.path(),
            "SELECT COUNT(*) FROM commit_log
             WHERE operation_key GLOB 'bad-*' OR operation_key IN ('again','unknown','observation','created after the serve','retired at the serve')"
        ),
        0,
        "a refused commit appends no commit-log row"
    );
}

/// A serve creates no registry object, evidence row, change event, or outbox entry, and the egress rule denies Remote for the record's class.
#[test]
fn a_serve_creates_no_registry_or_evidence_row_and_its_class_denies_remote() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    let totals = || {
        ["object_registry", "evidence_meta", "change_event", "outbox"]
            .map(|table| count(root.path(), &format!("SELECT COUNT(*) FROM {table}")))
    };
    let before = totals();
    insert(&store, "serve", spec("s1", &["d1"], served_at, 100)).unwrap();
    assert_eq!(totals(), before);
    assert_eq!(
        Sensitivity::Sensitive.denies_destination(ArtifactDestination::Remote),
        Some(EligibilityDeniedReason::SensitiveRemote)
    );
}

/// For a serve at `known_as_of = n`, `mark_disputed` or `contradict` committed after n sets the flag and a later approval does not clear it; an event at or before n, a `mark_stale`, a null-commit-sequence row, and another source kind, source id, or revision do not; two qualifying events in one commit count once; a retired record is not answered. `served_counts_for` counts each case by the same rule.
#[test]
fn disputed_after_serve_counts_only_later_committed_disputes_on_the_same_lineage() {
    let objects = [
        "other-source-id",
        "other-source-kind",
        "re-approved",
        "later-dispute",
        "later-contradict",
        "at-serve",
        "before-serve",
        "stale",
        "uncommitted",
        "other-lineage",
        "two-in-one",
        "never",
    ];
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &objects);
    let before = tick(&store, "before");
    let served_at = tick(&store, "served");
    let id = insert(&store, "serve", spec("s1", &objects, served_at, 100)).unwrap();
    let after = tick(&store, "after");
    for (row, subject, kind, revision, commit_seq) in [
        ("a1", "later-dispute", "mark_disputed", 1, Some(after)),
        ("a2", "later-contradict", "contradict", 1, Some(after)),
        ("a3", "at-serve", "mark_disputed", 1, Some(served_at)),
        ("a4", "before-serve", "contradict", 1, Some(before)),
        ("a5", "stale", "mark_stale", 1, Some(after)),
        ("a6", "uncommitted", "mark_disputed", 1, None),
        ("a7", "other-lineage", "mark_disputed", 2, Some(after)),
        ("a8", "two-in-one", "mark_disputed", 1, Some(after)),
        ("a9", "two-in-one", "contradict", 1, Some(after)),
    ] {
        admission(root.path(), row, subject, kind, revision, commit_seq, None);
    }
    admission_on(
        root.path(),
        "b1",
        "other-source-id",
        "mark_disputed",
        ("fixture", "elsewhere", 1),
        Some(after),
        None,
    );
    admission_on(
        root.path(),
        "b2",
        "other-source-kind",
        "contradict",
        ("elsewhere", "other-source-kind", 1),
        Some(after),
        None,
    );
    admission(
        root.path(),
        "b3",
        "re-approved",
        "mark_disputed",
        1,
        Some(after),
        None,
    );
    let approved_at = tick(&store, "approved");
    admission(
        root.path(),
        "b4",
        "re-approved",
        "approve",
        1,
        Some(approved_at),
        None,
    );
    assert_eq!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(*) FROM admission_decisions
                 WHERE subject_object_id='two-in-one' AND commit_seq={after}"
            )
        ),
        2,
        "two qualifying events share one commit"
    );
    let disputed = |object: &str| {
        let sessions = store.served_sessions_for(object).unwrap();
        assert_eq!(sessions.len(), 1, "{object} has one serve");
        assert_eq!(sessions[0].serve.use_outcome_id, id);
        assert_eq!(
            store
                .served_counts_for(
                    &kernel::applicability::EvalBudget::unbounded(),
                    &project(),
                    object
                )
                .unwrap(),
            ServedCounts {
                served_sessions: 1,
                disputed_after_serve: u64::from(sessions[0].disputed_after_serve),
            },
            "{object}: the counts apply the by-decision read's rule"
        );
        sessions[0].disputed_after_serve
    };
    for object in [
        "later-dispute",
        "later-contradict",
        "two-in-one",
        "re-approved",
    ] {
        assert!(disputed(object), "{object}");
    }
    for object in [
        "other-source-id",
        "other-source-kind",
        "at-serve",
        "before-serve",
        "stale",
        "uncommitted",
        "other-lineage",
        "never",
    ] {
        assert!(!disputed(object), "{object}");
    }
    assert!(store.served_sessions_for("unserved").unwrap().is_empty());
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &project(),
                "unserved"
            )
            .unwrap(),
        ServedCounts::default()
    );

    writer(root.path())
        .execute("UPDATE use_outcomes SET invalidated_commit_seq=?1", [after])
        .unwrap();
    assert!(
        store
            .served_sessions_for("later-dispute")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &project(),
                "later-dispute"
            )
            .unwrap(),
        ServedCounts::default(),
        "a retired record is not counted"
    );
    assert_eq!(
        store
            .served_set(&project(), HARNESS, "s1")
            .unwrap()
            .unwrap()[0]
            .serve
            .invalidated_commit_seq,
        Some(after),
        "a retired record stays readable by session"
    );
}

/// An expired record is retired in a page commit under the reserved producer with its own receipt keyed by the cutoff and its id, and stays readable; a repeat at the same cutoff retires nothing; a record whose deadline equals the cutoff is kept.
#[test]
fn expiry_retires_an_expired_record_under_a_reserved_receipt() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    let expired = insert(&store, "old", spec("s1", &["d1"], served_at, 0)).unwrap();
    let due = insert(&store, "due", spec("s2", &["d1"], served_at, 1)).unwrap();
    let now = USE_OUTCOME_RETENTION_MS + 1;
    let digest = format!("{:x}", sha2::Sha256::digest(expired.as_bytes()));

    assert_eq!(
        store.expire_use_outcomes(now).unwrap(),
        UseOutcomeExpiry {
            retired: 1,
            cited: 0
        }
    );
    assert_eq!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(*) FROM operation_receipts r
                 JOIN use_outcomes u ON u.invalidated_commit_seq=r.commit_seq
                 JOIN commit_log c ON c.commit_seq=r.commit_seq
                 WHERE r.producer='eidnara-kernel/use-outcome-expiry'
                   AND r.operation_key='{now}:{expired}' AND r.request_digest='{digest}'
                   AND c.producer=r.producer
                   AND c.operation_key GLOB '{now}:page:*'
                   AND u.use_outcome_id='{expired}'"
            )
        ),
        1
    );
    let records = store
        .served_set(&project(), HARNESS, "s1")
        .unwrap()
        .unwrap();
    assert!(records[0].serve.invalidated_commit_seq.is_some());
    assert_eq!(records[0].decision_object_ids, ["d1"]);
    let kept = store
        .served_set(&project(), HARNESS, "s2")
        .unwrap()
        .unwrap();
    assert_eq!(kept[0].serve.use_outcome_id, due);
    assert_eq!(
        kept[0].serve.invalidated_commit_seq, None,
        "a deadline equal to the cutoff has not passed"
    );

    let tip = store.tip().unwrap();
    assert_eq!(
        store.expire_use_outcomes(now).unwrap(),
        UseOutcomeExpiry::default()
    );
    assert_eq!(
        store.tip().unwrap(),
        tip,
        "a repeat at the same cutoff commits nothing"
    );
}

/// N eligible records drain in ceil(N / 64) calls, oldest deadline first.
#[test]
fn an_expiry_backlog_drains_one_page_per_call() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    let total = MAX_EXPIRED_USE_OUTCOMES_PER_CALL + 6;
    for index in 0..total {
        insert(
            &store,
            &format!("serve-{index}"),
            spec(
                &format!("s{index}"),
                &["d1"],
                served_at,
                i64::try_from(index).unwrap(),
            ),
        )
        .unwrap();
    }
    let now = 2 * USE_OUTCOME_RETENTION_MS;
    assert!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(*) FROM use_outcomes
                 WHERE invalidated_commit_seq IS NULL AND retain_until<{now}"
            )
        ) > i64::try_from(MAX_EXPIRED_USE_OUTCOMES_PER_CALL).unwrap(),
        "more eligible records than one page holds"
    );
    assert_eq!(
        store.expire_use_outcomes(now).unwrap().retired,
        MAX_EXPIRED_USE_OUTCOMES_PER_CALL
    );
    assert_eq!(
        count(
            root.path(),
            "SELECT MAX(recorded_at) FROM use_outcomes WHERE invalidated_commit_seq IS NOT NULL"
        ),
        i64::try_from(MAX_EXPIRED_USE_OUTCOMES_PER_CALL).unwrap() - 1,
        "the oldest deadlines retire first"
    );
    assert_eq!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(*) FROM operation_receipts r
                 JOIN use_outcomes u ON u.invalidated_commit_seq=r.commit_seq
                   AND r.operation_key='{now}:' || u.use_outcome_id
                 WHERE r.producer='eidnara-kernel/use-outcome-expiry'"
            )
        ),
        i64::try_from(MAX_EXPIRED_USE_OUTCOMES_PER_CALL).unwrap(),
        "one page commit records one receipt per retired record"
    );
    assert_eq!(store.expire_use_outcomes(now).unwrap().retired, 6);
    assert_eq!(
        store.expire_use_outcomes(now).unwrap(),
        UseOutcomeExpiry::default()
    );
    assert_eq!(
        count(
            root.path(),
            "SELECT COUNT(*) FROM use_outcomes WHERE invalidated_commit_seq IS NULL"
        ),
        0
    );
}

/// An expired record cited by a committed admission row whose subject is live survives and is counted cited; a citation by an uncommitted row or with a retired subject keeps nothing; once the cited subject is retired the record retires.
#[test]
fn an_expired_record_cited_by_a_live_report_is_retained() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1", "d2", "gone"]);
    let served_at = tick(&store, "tick");
    let cited = insert(&store, "cited", spec("s1", &["d1"], served_at, 0)).unwrap();
    let uncommitted = insert(&store, "uncommitted", spec("s2", &["d1"], served_at, 0)).unwrap();
    let dead_subject = insert(&store, "dead", spec("s3", &["d1"], served_at, 0)).unwrap();
    let report_at = tick(&store, "report");
    admission(
        root.path(),
        "r1",
        "d1",
        "contradict",
        1,
        Some(report_at),
        Some(&cited),
    );
    admission(
        root.path(),
        "r2",
        "d2",
        "contradict",
        1,
        None,
        Some(&uncommitted),
    );
    admission(
        root.path(),
        "r3",
        "gone",
        "contradict",
        1,
        Some(report_at),
        Some(&dead_subject),
    );
    writer(root.path())
        .execute(
            "UPDATE object_registry SET invalidated_commit_seq=?1 WHERE object_id='gone'",
            [report_at],
        )
        .unwrap();
    let now = USE_OUTCOME_RETENTION_MS + 1;
    assert_eq!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(*) FROM use_outcomes u
                 JOIN admission_decisions a ON a.use_outcome_id=u.use_outcome_id
                 JOIN object_registry o ON o.object_id=a.subject_object_id
                 WHERE u.use_outcome_id='{cited}' AND u.retain_until<{now}
                   AND a.commit_seq IS NOT NULL AND o.invalidated_commit_seq IS NULL"
            )
        ),
        1,
        "an expired record with a committed citation whose subject is live"
    );

    assert_eq!(
        store.expire_use_outcomes(now).unwrap(),
        UseOutcomeExpiry {
            retired: 2,
            cited: 1
        }
    );
    let live = |session: &str| {
        store
            .served_set(&project(), HARNESS, session)
            .unwrap()
            .unwrap()[0]
            .serve
            .invalidated_commit_seq
            .is_none()
    };
    assert!(live("s1"));
    assert!(!live("s2"));
    assert!(!live("s3"));

    let retire_at = tick(&store, "retire-subject");
    writer(root.path())
        .execute(
            "UPDATE object_registry SET invalidated_commit_seq=?1 WHERE object_id='d1'",
            [retire_at],
        )
        .unwrap();
    assert_eq!(store.expire_use_outcomes(now + 1).unwrap().retired, 1);
    assert!(!live("s1"));
}

/// The caller's receipt makes an equal repeat of a serve a replay that writes nothing, and the same key under another digest a conflict.
#[test]
fn a_repeated_serve_intent_replays_and_a_rebound_key_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    let serve = |digest: &str| {
        let mut intent = intent("serve");
        intent.request_digest = digest.to_string();
        store.commit(intent, |envelope| {
            envelope.insert_use_outcome(spec("s1", &["d1"], served_at, 100))
        })
    };
    let first = serve(&"1".repeat(64)).unwrap();
    assert!(!first.replayed);
    let again = serve(&"1".repeat(64)).unwrap();
    assert!(again.replayed);
    assert_eq!(again.commit_seq, first.commit_seq);
    assert_eq!(serve(&"2".repeat(64)), Err(KernelError::Conflict));
    assert_eq!(count(root.path(), "SELECT COUNT(*) FROM use_outcomes"), 1);
}

#[test]
fn session_roles_round_trip_and_the_served_set_digest_is_order_free() {
    for role in [SessionRole::Primary, SessionRole::Subagent] {
        assert_eq!(SessionRole::from_stored(role.as_str()), Some(role));
    }
    assert_eq!(SessionRole::from_stored("producer"), None);
    assert_eq!(
        served_set_digest(["b", "a", "a"]),
        served_set_digest(["a", "b"])
    );
    assert_ne!(served_set_digest(["ab"]), served_set_digest(["a", "b"]));
    assert_ne!(served_set_digest([]), served_set_digest([""]));
}

/// The dispute comparison is per serve: a dispute between two serves of one object counts for the earlier serve only, and retiring one serve leaves the other answered. The counts read the same rows: two serves with one disputed, then one with none, and an object never served counts zero of each.
#[test]
fn disputed_after_serve_is_judged_against_each_serve() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["x", "y"]);
    let first_at = tick(&store, "first");
    let first = insert(&store, "serve-1", spec("s1", &["x"], first_at, 100)).unwrap();
    let disputed_at = tick(&store, "disputed");
    admission(
        root.path(),
        "d1",
        "x",
        "mark_disputed",
        1,
        Some(disputed_at),
        None,
    );
    let second_at = tick(&store, "second");
    let second = insert(&store, "serve-2", spec("s2", &["x"], second_at, 100)).unwrap();
    let answer = |store: &KernelStore| {
        store
            .served_sessions_for("x")
            .unwrap()
            .into_iter()
            .map(|session| (session.serve.use_outcome_id, session.disputed_after_serve))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        answer(&store),
        [(first.clone(), true), (second.clone(), false)]
    );
    let counts = |served_sessions, disputed_after_serve| ServedCounts {
        served_sessions,
        disputed_after_serve,
    };
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &project(),
                "x"
            )
            .unwrap(),
        counts(2, 1)
    );
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &project(),
                "never-served"
            )
            .unwrap(),
        counts(0, 0)
    );
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &"b".repeat(64),
                "x"
            )
            .unwrap(),
        counts(0, 0),
        "another project's serves are not counted"
    );
    writer(root.path())
        .execute(
            "UPDATE use_outcomes SET invalidated_commit_seq=?1 WHERE use_outcome_id=?2",
            params![tick(&store, "retire"), first],
        )
        .unwrap();
    assert_eq!(answer(&store), [(second, false)]);
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &project(),
                "x"
            )
            .unwrap(),
        counts(1, 0),
        "a retired record is not counted"
    );
    let third_at = tick(&store, "third");
    insert(&store, "serve-3", spec("s2", &["x", "y"], third_at, 100)).unwrap();
    assert_eq!(
        store
            .served_counts_for(
                &kernel::applicability::EvalBudget::unbounded(),
                &project(),
                "x"
            )
            .unwrap(),
        counts(2, 0),
        "one session served at two served sets holds two records"
    );
}

/// The by-decision read answers serves oldest first by creating commit even when their ids, which the decision index orders, sort otherwise.
#[test]
fn served_sessions_are_ordered_by_creating_commit() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["x"]);
    let served_at = tick(&store, "tick");
    let ids: Vec<String> = (0..6)
        .map(|serve| {
            insert(
                &store,
                &format!("serve-{serve}"),
                spec(&format!("s{serve}"), &["x"], served_at, 100),
            )
            .unwrap()
        })
        .collect();
    let mut by_id = ids.clone();
    by_id.sort();
    assert_ne!(
        by_id, ids,
        "the serves' ids differ from their creation order"
    );
    let answer: Vec<String> = store
        .served_sessions_for("x")
        .unwrap()
        .into_iter()
        .map(|session| session.serve.use_outcome_id)
        .collect();
    assert_eq!(answer, ids);
}

/// A cited record whose deadline is later than a full page of uncited records is not read by the call that fills the page, and is counted by the call that reaches it.
#[test]
fn the_cited_count_covers_the_rows_read_before_the_page_fills() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    let page = i64::try_from(MAX_EXPIRED_USE_OUTCOMES_PER_CALL).unwrap();
    for index in 0..page {
        insert(
            &store,
            &format!("serve-{index}"),
            spec(&format!("s{index}"), &["d1"], served_at, index),
        )
        .unwrap();
    }
    let cited = insert(&store, "cited", spec("late", &["d1"], served_at, page)).unwrap();
    admission(
        root.path(),
        "report",
        "d1",
        "contradict",
        1,
        Some(tick(&store, "report")),
        Some(&cited),
    );
    let now = 2 * USE_OUTCOME_RETENTION_MS;
    assert_eq!(
        store.expire_use_outcomes(now).unwrap(),
        UseOutcomeExpiry {
            retired: MAX_EXPIRED_USE_OUTCOMES_PER_CALL,
            cited: 0
        }
    );
    assert_eq!(
        store.expire_use_outcomes(now).unwrap(),
        UseOutcomeExpiry {
            retired: 0,
            cited: 1
        }
    );
}

fn lifecycle(store: &KernelStore, session: &str) -> Vec<(Option<i64>, Option<i64>)> {
    store
        .served_set(&project(), HARNESS, session)
        .unwrap()
        .unwrap_or_default()
        .iter()
        .map(|record| (record.serve.deleted_at, record.serve.last_transform_at))
        .collect()
}

/// The other bindings a lifecycle predicate must leave alone: the same session id under another harness, and under another project.
const OTHER_HARNESS: &str = "claude-code";

fn other_project() -> String {
    "b".repeat(64)
}

/// Inserts `s1` records under the other harness and the other project, and answers a reader of both records' lifecycle columns.
fn insert_other_bindings(
    store: &KernelStore,
    served_at: i64,
    now: i64,
) -> impl Fn() -> Vec<(Option<i64>, Option<i64>)> + '_ {
    let mut harness_record = spec("s1", &["d1"], served_at, now);
    harness_record.harness = OTHER_HARNESS.to_string();
    insert(store, "other-harness", harness_record).unwrap();
    let mut project_record = spec("s1", &["d1"], served_at, now);
    project_record.project_digest = other_project();
    insert(store, "other-project", project_record).unwrap();
    move || {
        [(project(), OTHER_HARNESS), (other_project(), HARNESS)]
            .iter()
            .flat_map(|(project, harness)| {
                store
                    .served_set(project, harness, "s1")
                    .unwrap()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|record| (record.serve.deleted_at, record.serve.last_transform_at))
            })
            .collect()
    }
}

fn mark_deleted(
    store: &KernelStore,
    key: &str,
    session: &str,
    at: i64,
) -> Result<usize, KernelError> {
    let marked = std::cell::Cell::new(0);
    store.commit(intent(key), |envelope| {
        marked.set(envelope.mark_use_outcomes_deleted(&project(), HARNESS, session, at)?);
        Ok(String::new())
    })?;
    Ok(marked.get())
}

fn budget(wait: std::time::Duration) -> kernel::applicability::EvalBudget {
    kernel::applicability::EvalBudget::new(
        Some(std::time::Instant::now() + wait),
        std::sync::Arc::default(),
    )
}

/// Marking a session deleted sets `deleted_at` once on each of its live records: a repeat keeps the first value, a record served after the first mark is marked by the next, a retired record and another session keep their state, and a negative time is refused with no row changed.
#[test]
fn a_session_delete_marks_each_live_record_once() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1", "d2", "d3"]);
    let served_at = tick(&store, "tick");
    let now = USE_OUTCOME_RETENTION_MS + 1;
    insert(&store, "a", spec("s1", &["d1"], served_at, now)).unwrap();
    insert(&store, "b", spec("s1", &["d2"], served_at, now)).unwrap();
    insert(&store, "c", spec("s1", &["d3"], served_at, 0)).unwrap();
    insert(&store, "other", spec("s2", &["d1"], served_at, now)).unwrap();
    let other_bindings = insert_other_bindings(&store, served_at, now);
    assert_eq!(store.expire_use_outcomes(now).unwrap().retired, 1);

    assert_eq!(
        mark_deleted(&store, "neg", "s1", -1),
        Err(KernelError::InvalidInput)
    );
    assert_eq!(
        lifecycle(&store, "s1"),
        [(None, None), (None, None), (None, None)]
    );
    assert_eq!(mark_deleted(&store, "del-1", "s1", 10), Ok(2));
    assert_eq!(mark_deleted(&store, "del-2", "s1", 20), Ok(0));
    assert_eq!(
        lifecycle(&store, "s1"),
        [(Some(10), None), (Some(10), None), (None, None)],
        "the retired record keeps its state"
    );
    assert_eq!(lifecycle(&store, "s2"), [(None, None)]);
    assert_eq!(
        other_bindings(),
        [(None, None), (None, None)],
        "the same session id under another harness or project keeps its state"
    );
    let served_again = tick(&store, "tick-2");
    insert(&store, "e", spec("s1", &["d1", "d2"], served_again, now)).unwrap();
    assert_eq!(mark_deleted(&store, "del-3", "s1", 30), Ok(1));
    assert_eq!(
        lifecycle(&store, "s1"),
        [
            (Some(10), None),
            (Some(10), None),
            (None, None),
            (Some(30), None)
        ]
    );
    let records = store
        .served_set(&project(), HARNESS, "s1")
        .unwrap()
        .unwrap();
    assert_eq!(records[0].decision_object_ids, ["d1"]);
    assert_eq!(records[3].decision_object_ids, ["d1", "d2"]);
}

/// `last_transform_at` moves forward only on the session's live records, through a write that appends no commit-log row, receipt, or outbox entry; an earlier time, a deleted record, a retired record, and another session are left alone.
#[test]
fn last_transform_at_advances_live_records_without_a_commit() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1", "d2"]);
    let served_at = tick(&store, "tick");
    let now = USE_OUTCOME_RETENTION_MS + 1;
    insert(&store, "a", spec("s1", &["d1"], served_at, now)).unwrap();
    insert(&store, "c", spec("s2", &["d1"], served_at, now)).unwrap();
    insert(&store, "d", spec("s3", &["d1"], served_at, 0)).unwrap();
    let other_bindings = insert_other_bindings(&store, served_at, now);
    assert_eq!(store.expire_use_outcomes(now).unwrap().retired, 1);
    let advance = |session: &str, at: i64| {
        store.advance_use_outcome_last_transform(
            &budget(std::time::Duration::from_secs(5)),
            &project(),
            HARNESS,
            session,
            at,
        )
    };
    let tip = store.tip().unwrap();
    let durable = |root: &std::path::Path| {
        (
            count(root, "SELECT COUNT(*) FROM commit_log"),
            count(root, "SELECT COUNT(*) FROM operation_receipts"),
            count(root, "SELECT COUNT(*) FROM change_event"),
            count(root, "SELECT COUNT(*) FROM outbox"),
        )
    };
    let before = durable(root.path());

    assert_eq!(advance("s1", -1), Err(KernelError::InvalidInput));
    assert_eq!(advance("s1", 100), Ok(1));
    assert_eq!(advance("s1", 50), Ok(0));
    assert_eq!(advance("s1", 100), Ok(0));
    assert_eq!(lifecycle(&store, "s1"), [(None, Some(100))]);
    assert_eq!(lifecycle(&store, "s2"), [(None, None)]);
    assert_eq!(
        advance("s3", 100),
        Ok(0),
        "a retired record is not advanced"
    );
    assert_eq!(lifecycle(&store, "s3"), [(None, None)]);
    assert_eq!(
        other_bindings(),
        [(None, None), (None, None)],
        "the same session id under another harness or project is not advanced"
    );
    // At 100 the advanced record is current while both other bindings' `s1` records are null, so a
    // predicate that dropped the project or the harness would answer `true` for the first binding.
    let stale = |project: &str, harness: &str| {
        store.use_outcome_last_transform_is_stale(
            &budget(std::time::Duration::from_secs(5)),
            project,
            harness,
            "s1",
            100,
        )
    };
    assert_eq!(stale(&project(), HARNESS), Ok(false));
    assert_eq!(
        (
            stale(&other_project(), HARNESS),
            stale(&project(), OTHER_HARNESS)
        ),
        (Ok(true), Ok(true)),
        "the staleness check reads the binding it was asked about"
    );
    assert_eq!(store.tip().unwrap(), tip);
    assert_eq!(durable(root.path()), before);

    insert(&store, "b", spec("s1", &["d2"], served_at, now)).unwrap();
    assert_eq!(mark_deleted(&store, "del", "s1", 150), Ok(2));
    let later = tick(&store, "tick-2");
    insert(&store, "e", spec("s1", &["d1", "d2"], later, now)).unwrap();
    let tip = store.tip().unwrap();
    assert_eq!(advance("s1", 200), Ok(1));
    assert_eq!(
        lifecycle(&store, "s1"),
        [(Some(150), Some(100)), (Some(150), None), (None, Some(200))],
        "a deleted record receives no update"
    );
    assert_eq!(store.tip().unwrap(), tip);
}

#[test]
fn a_held_writer_bounds_the_advance_and_an_idle_session_skips_it() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    insert(&store, "a", spec("s1", &["d1"], served_at, 1)).unwrap();
    let wait = std::time::Duration::from_millis(100);
    std::thread::scope(|scope| {
        let (entered, held) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let store = &store;
        let holder = scope.spawn(move || {
            store
                .commit(intent("hold"), |_| {
                    entered.send(()).unwrap();
                    released.recv().unwrap();
                    Ok(String::new())
                })
                .unwrap();
        });
        held.recv().unwrap();
        let started = std::time::Instant::now();
        let advanced =
            store.advance_use_outcome_last_transform(&budget(wait), &project(), HARNESS, "s1", 10);
        assert_eq!(advanced, Err(KernelError::Deadline));
        let waited = started.elapsed();
        assert!(
            waited >= wait && waited < wait * 10,
            "the writer wait is bounded by the budget: {waited:?}"
        );
        assert_eq!(
            store.use_outcome_last_transform_is_stale(&budget(wait), &project(), HARNESS, "s9", 10),
            Ok(false)
        );
        release.send(()).unwrap();
        holder.join().unwrap();
    });
    assert_eq!(lifecycle(&store, "s1"), [(None, None)]);
}

/// A writer outside this process holding the database file lock stops the advance at its budget, at BEGIN, before SQLite's busy timeout; the record is unchanged.
#[test]
fn an_outside_file_lock_bounds_the_advance_at_begin() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    insert(&store, "a", spec("s1", &["d1"], served_at, 1)).unwrap();
    let outside = writer(root.path());
    outside.execute_batch("BEGIN IMMEDIATE").unwrap();
    let wait = std::time::Duration::from_millis(100);
    let started = std::time::Instant::now();
    let advanced =
        store.advance_use_outcome_last_transform(&budget(wait), &project(), HARNESS, "s1", 10);
    let waited = started.elapsed();
    outside.execute_batch("ROLLBACK").unwrap();
    assert_eq!(advanced, Err(KernelError::Deadline));
    // Ten budgets of slack for a loaded host, an order of magnitude under the 5 s busy timeout the budget preempts.
    assert!(
        waited >= wait && waited < wait * 10,
        "the wait is bounded by the budget, not the busy timeout: {waited:?}"
    );
    assert_eq!(lifecycle(&store, "s1"), [(None, None)]);
}

// --- Dispute rows: reporter columns and the use-outcome link ---

use kernel::{
    AdmissionEvent, AdmissionRequest, DecisionPayload, DecisionSpec, DisputeReporter, EventKind,
    SourceClass, TaintClass,
};

/// A store holding live, admitted decisions `objects` under the route's
/// plugin classes.
fn decision_store(root: &std::path::Path, objects: &[&str]) -> KernelStore {
    let store = store_with_objects(root, &[]);
    for object in objects {
        store
            .commit(intent(&format!("decide-{object}")), |envelope| {
                envelope.insert_decision(DecisionSpec {
                    decision_id: format!("{object}-decision"),
                    object_id: (*object).to_string(),
                    domain_id: "domain".to_string(),
                    proposition_id: None,
                    scope_id: None,
                    anchor_id: None,
                    evidence_id: None,
                    decision_kind: "memory".to_string(),
                    payload: DecisionPayload {
                        summary: (*object).to_string(),
                        rationale: String::new(),
                    },
                    source_kind: "assistant".to_string(),
                    source_id: format!("{object}-lineage"),
                    source_revision: 1,
                    sensitivity: Sensitivity::Normal,
                })?;
                envelope.record_admission(request(object, EventKind::Other))?;
                Ok(String::new())
            })
            .unwrap();
    }
    store
}

fn request(subject: &str, kind: EventKind) -> AdmissionRequest {
    AdmissionRequest {
        candidate_id: None,
        subject_object_id: Some(subject.to_string()),
        source_class: Some(SourceClass::ModelInference),
        taint_class: Some(TaintClass::AssistantInference),
        event: AdmissionEvent {
            kind,
            trigger_object_id: None,
            approval_object_id: None,
            evidence_id: None,
            reason: "dispute".to_string(),
        },
    }
}

fn reporter(session: &str, taint: TaintClass, payload: Option<&str>) -> DisputeReporter {
    DisputeReporter {
        source_class: SourceClass::ModelInference,
        taint_class: taint,
        event_payload: payload.map(str::to_string),
        project_digest: project(),
        harness: HARNESS.to_string(),
        session_id: session.to_string(),
    }
}

/// Files a dispute and answers its row's `(use_outcome_id, reporter source,
/// reporter taint, payload)`.
fn dispute(
    store: &KernelStore,
    root: &std::path::Path,
    key: &str,
    subject: &str,
    reporter: &DisputeReporter,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let receipt = store
        .commit(intent(key), |envelope| {
            envelope
                .record_dispute_admission(request(subject, EventKind::MarkDisputed), reporter)?;
            Ok(String::new())
        })
        .unwrap();
    writer(root)
        .query_row(
            "SELECT use_outcome_id,reporter_source_class,reporter_taint_class,event_payload
               FROM admission_decisions WHERE commit_seq=?1 AND subject_object_id=?2",
            params![receipt.commit_seq, subject],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

/// A dispute links the reporting session's serve of its subject with the
/// greatest served sequence below the dispute's commit: not an older serve, not
/// another session's, not a serve of another decision, and none for a session
/// never served the subject. A serve recorded after the dispute leaves the
/// dispute's link as it was written.
#[test]
fn a_dispute_links_the_reporting_sessions_latest_prior_serve_of_its_subject() {
    let root = tempfile::tempdir().unwrap();
    let store = decision_store(root.path(), &["d1", "d2", "d3"]);
    let first_at = tick(&store, "first");
    let older = insert(&store, "older", spec("s1", &["d1"], first_at, 0)).unwrap();
    let second_at = tick(&store, "second");
    let newer = insert(&store, "newer", spec("s1", &["d1", "d2"], second_at, 0)).unwrap();
    let third_at = tick(&store, "third");
    let other_decision =
        insert(&store, "other-decision", spec("s1", &["d2"], third_at, 0)).unwrap();
    insert(&store, "other-session", spec("s2", &["d1"], third_at, 0)).unwrap();
    let mut other_harness = spec("s1", &["d1"], third_at, 0);
    other_harness.harness = "pi".to_string();
    insert(&store, "other-harness", other_harness).unwrap();
    let mut other_project = spec("s1", &["d1"], third_at, 0);
    other_project.project_digest = "b".repeat(64);
    insert(&store, "other-project", other_project).unwrap();
    assert_ne!(older, newer);

    let payload =
        r#"{"path":"src/a.rs","git_object_id":"0123456789abcdef0123456789abcdef01234567"}"#;
    let served = reporter("s1", TaintClass::UserInferred, Some(payload));
    let linked = dispute(&store, root.path(), "dispute-d1", "d1", &served);
    assert_eq!(
        linked,
        (
            Some(newer.clone()),
            Some("model_inference".to_string()),
            Some("user_inferred".to_string()),
            Some(payload.to_string()),
        )
    );
    assert_eq!(
        dispute(
            &store,
            root.path(),
            "dispute-d2",
            "d2",
            &reporter("s1", TaintClass::AssistantInference, None)
        )
        .0,
        Some(other_decision)
    );
    let unserved = dispute(
        &store,
        root.path(),
        "unserved",
        "d1",
        &reporter("s3", TaintClass::AssistantInference, None),
    );
    assert_eq!(
        unserved,
        (
            None,
            Some("model_inference".to_string()),
            Some("assistant_inference".to_string()),
            None
        )
    );

    let later_at = tick(&store, "later");
    insert(
        &store,
        "later-serve",
        spec("s1", &["d1", "d3"], later_at, 0),
    )
    .unwrap();
    assert_eq!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(*) FROM admission_decisions
                  WHERE subject_object_id='d1' AND use_outcome_id='{newer}'"
            )
        ),
        1,
        "the first dispute keeps the link it was written with"
    );
}

/// Only disputes take reporter columns; every other writer stores NULL, the
/// DDL accepts every class pair the Kernel vocabularies name, and it refuses a
/// class outside its vocabulary, a half-set pair, and a payload that is not
/// JSON.
#[test]
fn reporter_columns_are_dispute_only_and_checked_by_the_schema() {
    let root = tempfile::tempdir().unwrap();
    let store = decision_store(root.path(), &["d1"]);
    assert_eq!(
        count(
            root.path(),
            "SELECT COUNT(*) FROM admission_decisions WHERE reporter_source_class IS NULL
               AND reporter_taint_class IS NULL AND event_payload IS NULL AND use_outcome_id IS NULL"
        ),
        1
    );
    let refused = store.commit(intent("stale-as-dispute"), |envelope| {
        envelope.record_dispute_admission(
            request("d1", EventKind::MarkStale),
            &reporter("s1", TaintClass::UserInferred, None),
        )?;
        Ok(String::new())
    });
    assert_eq!(refused.unwrap_err(), KernelError::InvalidInput);
    for (key, request, payload) in [
        (
            "no-subject",
            AdmissionRequest {
                subject_object_id: None,
                ..request("d1", EventKind::MarkDisputed)
            },
            None::<String>,
        ),
        (
            "array-payload",
            request("d1", EventKind::MarkDisputed),
            Some("[1]".to_string()),
        ),
        // Pins the refusal; the length check runs before the parse, and the secret scan refuses the same input.
        (
            "oversized-payload",
            request("d1", EventKind::MarkDisputed),
            Some(format!(
                "{{\"path\":\"{}\"}}",
                "a".repeat(context_core::redaction::MAX_REDACTABLE_BYTES)
            )),
        ),
    ] {
        let refused = store.commit(intent(key), |envelope| {
            envelope.record_dispute_admission(
                request,
                &reporter("s1", TaintClass::UserInferred, payload.as_deref()),
            )?;
            Ok(String::new())
        });
        assert_eq!(refused.unwrap_err(), KernelError::InvalidInput, "{key}");
    }

    let insert_row = |id: &str, event: &str, source: &str, taint: &str, payload: &str| {
        writer(root.path()).execute(
            &format!(
                "INSERT INTO admission_decisions(
                     admission_decision_id,subject_object_id,source_kind,source_id,
                     source_revision,source_class,taint_class,event_kind,maturity,
                     effective_maturity,disposition,visibility,outcome,sensitivity_class,
                     policy_revision,reason,decided_at,reporter_source_class,
                     reporter_taint_class,event_payload
                 ) VALUES ('{id}','d1','fixture','d1',1,'model_inference','assistant_inference',
                           '{event}','candidate','candidate','disputed','explicit_labeled',
                           'deny','normal',1,'r',1,{source},{taint},{payload})"
            ),
            [],
        )
    };
    assert!(
        insert_row(
            "ok",
            "mark_disputed",
            "'model_inference'",
            "'user_inferred'",
            "'{}'"
        )
        .is_ok()
    );
    assert!(insert_row("ok-plain", "mark_stale", "NULL", "NULL", "NULL").is_ok());
    for source in SourceClass::ALL {
        for taint in TaintClass::ALL {
            let id = format!("every-{}-{}", source.as_str(), taint.as_str());
            let (source, taint) = (
                format!("'{}'", source.as_str()),
                format!("'{}'", taint.as_str()),
            );
            assert!(
                insert_row(&id, "contradict", &source, &taint, "NULL").is_ok(),
                "{id}"
            );
        }
    }
    for (id, event, source, taint, payload) in [
        (
            "bad-source",
            "contradict",
            "'oracle'",
            "'user_inferred'",
            "NULL",
        ),
        (
            "bad-taint",
            "contradict",
            "'model_inference'",
            "'oracle'",
            "NULL",
        ),
        (
            "half-source",
            "contradict",
            "'model_inference'",
            "NULL",
            "NULL",
        ),
        (
            "half-taint",
            "contradict",
            "NULL",
            "'user_inferred'",
            "NULL",
        ),
        (
            "not-json",
            "contradict",
            "'model_inference'",
            "'user_inferred'",
            "'{'",
        ),
        (
            "not-object",
            "contradict",
            "'model_inference'",
            "'user_inferred'",
            "'[1]'",
        ),
        (
            "not-a-dispute",
            "mark_stale",
            "'model_inference'",
            "'user_inferred'",
            "NULL",
        ),
        ("payload-alone", "contradict", "NULL", "NULL", "'{}'"),
    ] {
        assert!(
            insert_row(id, event, source, taint, payload).is_err(),
            "{id}"
        );
    }
}

/// A session's first serve records it once; every later serve of the session, whatever its served set, carries the same `session_seq`, another session, harness, or project gets its own, and each decision row copies its record's `session_seq` and `served_commit_seq`.
#[test]
fn a_session_is_recorded_once_and_its_decision_rows_copy_their_record() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1", "d2"]);
    let first_at = tick(&store, "first");
    insert(&store, "s1-first", spec("s1", &["d1"], first_at, 0)).unwrap();
    let second_at = tick(&store, "second");
    insert(&store, "s1-second", spec("s1", &["d1", "d2"], second_at, 0)).unwrap();
    insert(&store, "s2", spec("s2", &["d1"], second_at, 0)).unwrap();
    let mut other_harness = spec("s1", &["d2"], second_at, 0);
    other_harness.harness = "pi".to_string();
    insert(&store, "other-harness", other_harness).unwrap();
    let mut other_project = spec("s1", &["d2"], second_at, 0);
    other_project.project_digest = "b".repeat(64);
    insert(&store, "other-project", other_project).unwrap();

    assert_eq!(
        count(root.path(), "SELECT COUNT(*) FROM use_outcome_sessions"),
        4
    );
    assert_eq!(
        count(
            root.path(),
            &format!(
                "SELECT COUNT(DISTINCT session_seq) FROM use_outcomes
                  WHERE project_digest='{}' AND harness='{HARNESS}' AND session_id='s1'",
                project()
            )
        ),
        1
    );
    assert_eq!(
        count(
            root.path(),
            "SELECT COUNT(*) FROM use_outcome_decisions d
               JOIN use_outcomes u ON u.use_outcome_id=d.use_outcome_id
              WHERE d.session_seq IS NOT u.session_seq
                 OR d.served_commit_seq IS NOT u.served_commit_seq"
        ),
        0
    );
    assert_eq!(
        count(root.path(), "SELECT COUNT(*) FROM use_outcome_decisions"),
        6
    );
}

/// Two records of one session that serve the subject at the same sequence tie on `served_commit_seq`; the dispute links the one with the greater id.
#[test]
fn a_tie_on_the_served_sequence_links_the_greater_record_id() {
    let root = tempfile::tempdir().unwrap();
    let store = decision_store(root.path(), &["d1", "d2"]);
    let served_at = tick(&store, "served");
    let alone = insert(&store, "alone", spec("s1", &["d1"], served_at, 0)).unwrap();
    let paired = insert(&store, "paired", spec("s1", &["d1", "d2"], served_at, 0)).unwrap();
    let linked = dispute(
        &store,
        root.path(),
        "tie",
        "d1",
        &reporter("s1", TaintClass::AssistantInference, None),
    );
    assert_eq!(linked.0, Some(alone.max(paired)));
}

/// The link as #1162 defines it, read through the record join: the session's record naming the subject with the greatest `served_commit_seq` below the dispute's commit, ties to the greatest id.
const DEFINITION_SQL: &str = "SELECT u.use_outcome_id FROM use_outcome_decisions d
     JOIN use_outcomes u ON u.use_outcome_id=d.use_outcome_id
    WHERE d.object_id=?1 AND u.project_digest=?2 AND u.harness=?3 AND u.session_id=?4
      AND u.served_commit_seq<?5
    ORDER BY u.served_commit_seq DESC, u.use_outcome_id DESC LIMIT 1";

/// One generated event: kind (0 or 1 a serve, 2 a tick, 3 a dispute), session, harness, and project indexes, a served-set mask over `d1..d3` (a dispute's subject is `mask % 3`), and which tick a serve was served at.
type GeneratedEvent = (u8, usize, usize, usize, u8, usize);

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig {
        cases: 48,
        rng_algorithm: proptest::test_runner::RngAlgorithm::ChaCha,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x1162),
        ..proptest::prelude::ProptestConfig::default()
    })]

    /// Over generated histories that interleave serves, ticks, and disputes (shared and out-of-order served sequences, overlapping served sets, two harnesses, two projects, and sessions never served), every dispute stores the link the record-join definition answers when it commits.
    #[test]
    fn the_stored_link_equals_the_record_join_definition(
        events in proptest::collection::vec(
            (0u8..4, 0usize..4, 0usize..2, 0usize..2, 0u8..8, 0usize..8),
            0..24,
        ),
    ) {
        check_link_definition(&events)?;
    }
}

fn check_link_definition(
    events: &[GeneratedEvent],
) -> Result<(), proptest::test_runner::TestCaseError> {
    const OBJECTS: [&str; 3] = ["d1", "d2", "d3"];
    const SESSIONS: [&str; 4] = ["s1", "s2", "s3", "never"];
    const HARNESSES: [&str; 2] = [HARNESS, "pi"];
    let projects = [project(), "b".repeat(64)];
    let root = tempfile::tempdir().unwrap();
    let store = decision_store(root.path(), &OBJECTS);
    let mut ticks: Vec<i64> = (0..2).map(|n| tick(&store, &format!("tick-{n}"))).collect();
    let reads = writer(root.path());
    let check_dispute = |key: &str,
                         session: usize,
                         harness: usize,
                         project: usize,
                         subject: &str|
     -> Result<(), proptest::test_runner::TestCaseError> {
        let reporter = DisputeReporter {
            project_digest: projects[project].clone(),
            harness: HARNESSES[harness].to_string(),
            ..reporter(SESSIONS[session], TaintClass::AssistantInference, None)
        };
        let receipt = store
            .commit(intent(key), |envelope| {
                envelope.record_dispute_admission(
                    request(subject, EventKind::MarkDisputed),
                    &reporter,
                )?;
                Ok(String::new())
            })
            .unwrap();
        let stored: Option<String> = reads
            .query_row(
                "SELECT use_outcome_id FROM admission_decisions
                  WHERE commit_seq=?1 AND subject_object_id=?2",
                params![receipt.commit_seq, subject],
                |row| row.get(0),
            )
            .unwrap();
        let defined: Option<String> = reads
            .query_row(
                DEFINITION_SQL,
                params![
                    subject,
                    reporter.project_digest,
                    reporter.harness,
                    reporter.session_id,
                    receipt.commit_seq
                ],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        proptest::prop_assert_eq!(stored, defined, "{}", key);
        Ok(())
    };
    for (n, &(kind, session, harness, project, mask, at)) in events.iter().enumerate() {
        match kind {
            0 | 1 => {
                let objects: Vec<&str> = (0..OBJECTS.len())
                    .filter(|bit| mask & (1 << bit) != 0)
                    .map(|bit| OBJECTS[bit])
                    .collect();
                // Serves draw from the first three sessions, so `SESSIONS[3]` only ever disputes.
                let mut serve = spec(SESSIONS[session % 3], &objects, ticks[at % ticks.len()], 0);
                serve.harness = HARNESSES[harness].to_string();
                serve.project_digest = projects[project].clone();
                // A repeated identity is `Conflict` and records nothing; the history keeps its first record.
                match insert(&store, &format!("serve-{n}"), serve) {
                    Ok(_) | Err(KernelError::Conflict) => {}
                    Err(error) => proptest::prop_assert!(false, "serve {} refused: {:?}", n, error),
                }
            }
            2 => ticks.push(tick(&store, &format!("tick-event-{n}"))),
            _ => check_dispute(
                &format!("dispute-{n}"),
                session,
                harness,
                project,
                OBJECTS[usize::from(mask) % OBJECTS.len()],
            )?,
        }
    }
    for (session, _) in SESSIONS.iter().enumerate() {
        for subject in OBJECTS {
            check_dispute(
                &format!("final-{session}-{subject}"),
                session,
                0,
                0,
                subject,
            )?;
        }
    }
    Ok(())
}

#[test]
fn a_held_reader_pool_bounds_the_count_read() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    insert(&store, "a", spec("s1", &["d1"], served_at, 100)).unwrap();
    let wait = std::time::Duration::from_millis(100);
    let started = std::time::Instant::now();
    let counted = store
        .with_readers_held_for_test(|| store.served_counts_for(&budget(wait), &project(), "d1"));
    let waited = started.elapsed();
    assert_eq!(counted, Err(KernelError::Deadline));
    assert!(waited >= wait, "{waited:?}");
    assert_eq!(
        store.served_counts_for(&budget(wait), &project(), "d1"),
        Ok(ServedCounts {
            served_sessions: 1,
            disputed_after_serve: 0,
        })
    );
}

/// The staleness check answers `true` while a live record of the session holds a `last_transform_at` that is null or before the time, and writes nothing when it does; a spent budget answers `Deadline`. The live-record filter it shares with the advance is covered by `last_transform_at_advances_live_records_without_a_commit`.
#[test]
fn the_staleness_check_answers_for_null_or_earlier_values_and_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    insert(&store, "a", spec("s1", &["d1"], served_at, 1)).unwrap();
    let stale = |wait: std::time::Duration, at: i64| {
        store.use_outcome_last_transform_is_stale(&budget(wait), &project(), HARNESS, "s1", at)
    };
    let wait = std::time::Duration::from_secs(5);
    let tip = store.tip().unwrap();

    assert_eq!(stale(wait, 100), Ok(true), "a null value is stale");
    assert_eq!(
        lifecycle(&store, "s1"),
        [(None, None)],
        "the check writes nothing"
    );
    assert_eq!(
        stale(std::time::Duration::ZERO, 100),
        Err(KernelError::Deadline)
    );
    assert_eq!(
        store.advance_use_outcome_last_transform(&budget(wait), &project(), HARNESS, "s1", 100),
        Ok(1)
    );
    assert_eq!(stale(wait, 100), Ok(false));
    assert_eq!(stale(wait, 99), Ok(false));
    assert_eq!(stale(wait, 101), Ok(true));
    assert_eq!(store.tip().unwrap(), tip);
}

#[test]
fn the_advance_takes_no_pooled_reader() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["d1"]);
    let served_at = tick(&store, "tick");
    insert(&store, "a", spec("s1", &["d1"], served_at, 1)).unwrap();
    let advanced = store.with_readers_held_for_test(|| {
        store.advance_use_outcome_last_transform(
            &budget(std::time::Duration::from_millis(100)),
            &project(),
            HARNESS,
            "s1",
            10,
        )
    });
    assert_eq!(advanced, Ok(1));
    assert_eq!(lifecycle(&store, "s1"), [(None, Some(10))]);
}
