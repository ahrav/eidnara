//! This test lowers the process's soft descriptor limit, so it runs in a
//! dedicated binary to isolate the process-wide limit.

#![cfg(all(feature = "test-support", target_os = "linux"))]

#[path = "support/eligibility_fixture.rs"]
mod eligibility_fixture;

use std::fs;

use kernel::{KernelStore, Sensitivity};
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

const SHARDS: u64 = 128;
const OBJECTS_PER_SHARD: u64 = 150;
const HEADROOM: u64 = 8;

fn open_descriptors() -> u64 {
    u64::try_from(fs::read_dir("/proc/self/fd").unwrap().count()).unwrap()
}

/// The soft descriptor limit as it stood before the test lowered it,
/// restored on drop so a panic leaves the process as it found it.
struct SoftLimit(Rlimit);

impl SoftLimit {
    fn lower_to(headroom: u64) -> Self {
        let before = getrlimit(Resource::Nofile);
        let lowered = Rlimit {
            current: Some(open_descriptors() + headroom),
            maximum: before.maximum,
        };
        setrlimit(Resource::Nofile, lowered).unwrap();
        Self(before)
    }
}

impl Drop for SoftLimit {
    fn drop(&mut self) {
        setrlimit(Resource::Nofile, self.0).unwrap();
    }
}

#[test]
fn an_admission_walk_short_of_descriptors_counts_the_tree_one_shard_at_a_time() {
    let fixture = eligibility_fixture::fixture();
    let root = fixture._root.path().to_path_buf();
    let mut expected = fixture.store.artifact_budget_facts().unwrap().usage_bytes;
    drop(fixture.store);

    let objects = root.join("artifacts/objects");
    for shard in 0..SHARDS {
        let shard_dir = objects.join(format!("{shard:02x}"));
        fs::create_dir(&shard_dir).unwrap();
        for object in 0..OBJECTS_PER_SHARD {
            fs::write(shard_dir.join(format!("{shard:02x}{object:060x}")), b"xyz").unwrap();
            expected += 3;
        }
    }

    let store = KernelStore::open(&root).unwrap();
    assert_eq!(
        store.artifact_budget_facts().unwrap().usage_bytes,
        expected,
        "the open census counts the tree under the ordinary limit"
    );
    store.forget_artifact_usage_for_test();

    let handle = {
        let _limit = SoftLimit::lower_to(HEADROOM);
        store.ingest_artifact(eligibility_fixture::artifact(
            "probe",
            b"probe",
            Sensitivity::Normal,
        ))
    };
    handle.expect("admission walks the tree within the descriptor limit");
    assert_eq!(
        store.artifact_budget_facts().unwrap().usage_bytes,
        expected + 5,
        "the walk that admitted the probe became the count"
    );
}
