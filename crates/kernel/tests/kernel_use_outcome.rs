use kernel::{
    ArtifactDestination, CommitIntent, DomainSpec, EligibilityDeniedReason, KernelError,
    KernelStore, MAX_EXPIRED_USE_OUTCOMES_PER_CALL, Sensitivity, SessionRole,
    USE_OUTCOME_RETENTION_CLASS, USE_OUTCOME_RETENTION_MS, UseOutcomeExpiry, UseOutcomeSpec,
    served_set_digest, use_outcome_id,
};
use rusqlite::{Connection, params};
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

/// A duplicate identity is `Conflict`, an unregistered decision is `NotFound`, a malformed or secret-bearing spec is `InvalidInput`, and a refused commit leaves no row in either table.
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
        )
    };
    assert_eq!(rows(), (1, 1));

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
    assert_eq!(rows(), (1, 1));
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

/// For a serve at `known_as_of = n`, `mark_disputed` or `contradict` committed after n sets the flag and a later approval does not clear it; an event at or before n, a `mark_stale`, a null-commit-sequence row, and another source kind, source id, or revision do not; two qualifying events in one commit count once; a retired record is not answered.
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

/// The dispute comparison is per serve: a dispute between two serves of one object counts for the earlier serve only, and retiring one serve leaves the other answered.
#[test]
fn disputed_after_serve_is_judged_against_each_serve() {
    let root = tempfile::tempdir().unwrap();
    let store = store_with_objects(root.path(), &["x"]);
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
    writer(root.path())
        .execute(
            "UPDATE use_outcomes SET invalidated_commit_seq=?1 WHERE use_outcome_id=?2",
            params![tick(&store, "retire"), first],
        )
        .unwrap();
    assert_eq!(answer(&store), [(second, false)]);
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
    assert_eq!(
        store.use_outcome_last_transform_is_stale(
            &budget(std::time::Duration::from_secs(5)),
            &other_project(),
            HARNESS,
            "s1",
            200
        ),
        Ok(true),
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
        assert!(started.elapsed() >= wait);
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
