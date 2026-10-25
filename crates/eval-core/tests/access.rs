use std::collections::BTreeMap;

use context_core::canonical_json::canonical_json_encode;
use eval_core::{
    AccessError, AccessRecord, ArmKind, Cut, HistoryPolicy, MemoryRead, MemoryWrite, SampleError,
    SampleLedger, SampleRecord, Terminal, WriteKey, excludes,
};
use serde_json::{Value, json};

const RUN: &str = "aa00000000000000000000000000000000000000000000000000000000000000";
const PRIOR: &str = "bb00000000000000000000000000000000000000000000000000000000000000";
const OTHER: &str = "cc00000000000000000000000000000000000000000000000000000000000000";

fn sample(id: &str, task: &str, arm: ArmKind, lineage: &[&str]) -> SampleRecord {
    SampleRecord {
        id: id.to_string(),
        task: task.to_string(),
        arm,
        policy: HistoryPolicy::Raw,
        cut: Cut::EndOfRun,
        lineage: lineage.iter().map(|run| run.to_string()).collect(),
        terminal: Terminal::Pass,
    }
}

fn ledger() -> SampleLedger {
    let samples = [
        sample("s1", "t1", ArmKind::Fresh, &[PRIOR]),
        sample("s2", "t1", ArmKind::Aged, &[]),
        sample("s3", "t2", ArmKind::Aged, &[OTHER]),
    ];
    SampleLedger {
        epoch: 1,
        order: samples.iter().map(|s| s.id.clone()).collect(),
        samples: samples.into_iter().map(|s| (s.id.clone(), s)).collect(),
    }
}

fn key(task: &str, arm: ArmKind) -> WriteKey {
    WriteKey {
        task: task.to_string(),
        arm,
    }
}

fn write(id: &str, writer: &str, key: WriteKey, run: &str) -> MemoryWrite {
    MemoryWrite {
        write_id: id.to_string(),
        pool: "p".to_string(),
        key,
        writer: writer.to_string(),
        eval_run_id: run.to_string(),
        sequence: 5,
    }
}

fn read(reader: &str, write_id: &str) -> MemoryRead {
    MemoryRead {
        reader: reader.to_string(),
        pool: "p".to_string(),
        write_id: write_id.to_string(),
        sequence: 9,
    }
}

fn writes() -> Vec<MemoryWrite> {
    vec![
        write("w3", "s3", key("t2", ArmKind::Aged), RUN),
        write("w7", "s2", key("t1", ArmKind::Aged), RUN),
    ]
}

fn record(writes: Vec<MemoryWrite>, reads: Vec<MemoryRead>) -> AccessRecord {
    let mut read_counts: BTreeMap<String, u32> =
        ["s1", "s2", "s3"].map(|s| (s.to_string(), 0)).into();
    for read in &reads {
        *read_counts.get_mut(&read.reader).unwrap() += 1;
    }
    AccessRecord {
        writes,
        reads,
        read_counts,
    }
}

#[test]
fn the_write_key_is_task_and_arm_and_retries_share_it() {
    let aged = sample("a", "t1", ArmKind::Aged, &[]);
    let fresh = sample("f", "t1", ArmKind::Fresh, &[]);
    assert_eq!(WriteKey::from(&aged), key("t1", ArmKind::Aged));
    assert_ne!(WriteKey::from(&aged), WriteKey::from(&fresh));
    let retry = SampleRecord {
        id: "a-retry".to_string(),
        policy: HistoryPolicy::Structured,
        cut: Cut::AtQuiescence,
        lineage: vec![PRIOR.to_string()],
        terminal: Terminal::Fail,
        ..aged.clone()
    };
    assert_eq!(WriteKey::from(&retry), WriteKey::from(&aged));
    assert_ne!(
        WriteKey::from(&sample("a", "t2", ArmKind::Aged, &[])),
        WriteKey::from(&aged)
    );
}

#[test]
fn excludes_names_the_own_task_under_every_arm_and_nothing_else() {
    let target = sample("s", "t1", ArmKind::Aged, &[]);
    for task in ["t1", "t2"] {
        for arm in [ArmKind::Aged, ArmKind::Fresh, ArmKind::FreshMinimal] {
            let key = key(task, arm);
            assert_eq!(excludes(&target, &key), task == "t1", "{key:?}");
        }
    }
}

#[test]
fn a_clean_or_write_only_record_proves_the_pools_disjoint() {
    let ledger = ledger();
    record(writes(), vec![read("s1", "w3")])
        .check_disjoint(&ledger, RUN)
        .unwrap();
    record(writes(), vec![])
        .check_disjoint(&ledger, RUN)
        .unwrap();
    record(vec![], vec![]).check_disjoint(&ledger, RUN).unwrap();
}

#[test]
fn a_read_of_an_own_task_write_is_refused_naming_sample_read_and_key() {
    let ledger = ledger();
    let own = Err(AccessError::OwnTaskRead {
        sample: "s1".to_string(),
        read: Box::new(read("s1", "w7")),
        key: key("t1", ArmKind::Aged),
    });
    assert_eq!(
        record(writes(), vec![read("s1", "w3"), read("s1", "w7")]).check_disjoint(&ledger, RUN),
        own
    );
    let prior = vec![write("w7", "s2", key("t1", ArmKind::Aged), PRIOR)];
    assert_eq!(
        record(prior, vec![read("s1", "w7")]).check_disjoint(&ledger, RUN),
        own
    );
    // `OTHER` belongs to `s3`'s lineage, so its writes come from an attempt
    // separate from `s1`.
    let other = vec![write("w7", "s2", key("t1", ArmKind::Aged), OTHER)];
    record(other, vec![read("s1", "w7")])
        .check_disjoint(&ledger, RUN)
        .unwrap();
    assert!(matches!(
        record(writes(), vec![read("s2", "w7")]).check_disjoint(&ledger, RUN),
        Err(AccessError::OwnTaskRead { sample, .. }) if sample == "s2"
    ));
    // Order-free: a read sequenced before the write is refused the same way.
    let mut early = read("s1", "w7");
    early.sequence = 0;
    assert_eq!(
        record(writes(), vec![early.clone()]).check_disjoint(&ledger, RUN),
        Err(AccessError::OwnTaskRead {
            sample: "s1".to_string(),
            read: Box::new(early),
            key: key("t1", ArmKind::Aged),
        })
    );
}

#[test]
fn dangling_references_and_attestation_gaps_are_refused_by_name() {
    let ledger = ledger();
    let refused = |record: AccessRecord| record.check_disjoint(&ledger, RUN).err();
    let mut stranger = writes();
    stranger[0].writer = "s9".to_string();
    assert_eq!(
        refused(record(stranger, vec![])),
        Some(AccessError::UnknownWriter {
            write_id: "w3".to_string(),
            writer: "s9".to_string(),
        })
    );
    let mut unrun = writes();
    unrun[1].eval_run_id = "dd".repeat(32);
    assert_eq!(
        refused(record(unrun, vec![])),
        Some(AccessError::UnknownRun {
            write_id: "w7".to_string(),
            eval_run_id: "dd".repeat(32),
        })
    );
    let mut repeated = writes();
    repeated[1].write_id = "w3".to_string();
    assert_eq!(
        refused(record(repeated, vec![])),
        Some(AccessError::RepeatedWrite {
            write_id: "w3".to_string()
        })
    );
    let mut relabeled = writes();
    relabeled[1].key = key("t2", ArmKind::Aged);
    assert_eq!(
        refused(record(relabeled, vec![read("s1", "w7")])),
        Some(AccessError::KeyDisagreesWithWriter {
            write_id: "w7".to_string(),
            key: key("t2", ArmKind::Aged),
            writer_key: key("t1", ArmKind::Aged),
        })
    );
    let mut unknown_reader = record(writes(), vec![]);
    unknown_reader.reads.push(read("s9", "w3"));
    assert_eq!(
        refused(unknown_reader),
        Some(AccessError::UnknownReader {
            reader: "s9".to_string()
        })
    );
    assert_eq!(
        refused(record(writes(), vec![read("s1", "w9")])),
        Some(AccessError::UnknownWrite {
            reader: "s1".to_string(),
            write_id: "w9".to_string(),
        })
    );
    let mut elsewhere = read("s1", "w3");
    elsewhere.pool = "q".to_string();
    assert_eq!(
        refused(record(writes(), vec![elsewhere])),
        Some(AccessError::CrossPoolRead {
            reader: "s1".to_string(),
            write_id: "w3".to_string(),
            read_pool: "q".to_string(),
            write_pool: "p".to_string(),
        })
    );
    let mut off_by_one = record(writes(), vec![read("s1", "w3")]);
    off_by_one.read_counts.insert("s1".to_string(), 2);
    assert_eq!(
        refused(off_by_one),
        Some(AccessError::ReadCountMismatch {
            sample: "s1".to_string(),
            attested: Some(2),
            recorded: 1,
        })
    );
    let mut unattested = record(writes(), vec![]);
    unattested.read_counts.remove("s3");
    assert_eq!(
        refused(unattested),
        Some(AccessError::ReadCountMismatch {
            sample: "s3".to_string(),
            attested: None,
            recorded: 0,
        })
    );
    let mut counted_stranger = record(writes(), vec![]);
    counted_stranger.read_counts.insert("s9".to_string(), 0);
    assert_eq!(
        refused(counted_stranger),
        Some(AccessError::UnknownAttestedSample {
            sample: "s9".to_string()
        })
    );
    let mut nameless = writes();
    nameless[0].write_id = " ".to_string();
    assert_eq!(
        refused(record(nameless, vec![])),
        Some(AccessError::Blank {
            write_id: " ".to_string(),
            field: "write_id",
        })
    );
    let mut poolless = writes();
    poolless[1].pool = String::new();
    assert_eq!(
        refused(record(poolless, vec![])),
        Some(AccessError::Blank {
            write_id: "w7".to_string(),
            field: "pool",
        })
    );
    let mut unpooled = read("s1", "w3");
    unpooled.pool = String::new();
    assert_eq!(
        refused(record(writes(), vec![unpooled])),
        Some(AccessError::Blank {
            write_id: "w3".to_string(),
            field: "pool",
        })
    );
    for run in ["", "AA".repeat(32).as_str(), "aa"] {
        assert_eq!(
            record(writes(), vec![]).check_disjoint(&ledger, run),
            Err(AccessError::MalformedRun {
                eval_run_id: run.to_string()
            })
        );
    }
}

#[test]
fn an_invalid_ledger_is_refused_before_the_record_is_read() {
    let mut ledger = ledger();
    ledger
        .samples
        .get_mut("s1")
        .unwrap()
        .lineage
        .push(PRIOR.to_string());
    let expected = ledger.validate().unwrap_err();
    assert_eq!(
        expected,
        SampleError::RepeatedLineage {
            sample: "s1".to_string(),
            entry: PRIOR.to_string(),
        }
    );
    let mut dangling = record(writes(), vec![]);
    dangling.reads.push(read("s9", "w9"));
    assert_eq!(
        dangling.check_disjoint(&ledger, RUN),
        Err(AccessError::Ledger(expected))
    );
}

#[test]
fn the_access_record_round_trips_canonically_and_refuses_unknown_fields() {
    let record = record(writes(), vec![read("s1", "w3")]);
    let value = serde_json::to_value(&record).unwrap();
    let encoded = canonical_json_encode(&value).unwrap();
    let decoded: AccessRecord = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, record);
    assert_eq!(value.pointer("/writes/0/sequence"), Some(&json!(5)));
    assert_eq!(value.pointer("/reads/0/sequence"), Some(&json!(9)));
    assert_eq!(
        canonical_json_encode(&serde_json::to_value(&decoded).unwrap()).unwrap(),
        encoded
    );
    let with_note = |pointer: &str| {
        let mut value = value.clone();
        value.pointer_mut(pointer).unwrap()["note"] = json!("x");
        value
    };
    for pointer in ["", "/writes/0", "/writes/0/key", "/reads/0"] {
        assert!(
            serde_json::from_value::<AccessRecord>(with_note(pointer)).is_err(),
            "{pointer}"
        );
    }
    let mut missing: Value = value.clone();
    missing.as_object_mut().unwrap().remove("read_counts");
    assert!(serde_json::from_value::<AccessRecord>(missing).is_err());
}
