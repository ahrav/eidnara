//! Cuts inside the lifecycle store's staging path, driven through its [`StageEvent`] observer.
//!
//! Every cell of the frozen table is cut three ways: a hook-simulated `EIO`, hook-simulated storage exhaustion (`ENOSPC`, `EDQUOT`), and a process kill in a re-executed child that writes and syncs a witness file and then aborts. The hook cells exercise the store's error classification and cleanup at each step; they are not kernel `fsync` evidence. The kill cells show what a crash at each step leaves on disk for the next process, not what survives power loss.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

use daemon::vector_composition::publish;
use daemon::vector_generation::{
    BuiltVectors, CODES_FILE, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE, SIDECAR_FILE, Staging,
    TOMBSTONES_FILE, VectorRefusal, build, stage, stage_observed, verify,
};
use host_runtime::generation::{
    CURRENT_PROFILE_NAME, CurrentProfile, GENERATIONS_DIR_NAME, GenerationError,
    GenerationManifest, GenerationStore, ManifestFile, SourceSpec, StageEvent, StageMeta,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use rustix::io::Errno;
use sha2::Digest;

use support::vector_store::*;

/// The workload whose staging a cell cuts. Vector layers are flat, so the nested-directory sync is cut on a store-level generation with one nested file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workload {
    Vector,
    Nested,
}

const NESTED_DIR: &str = "nested";
const NESTED_FILE: &str = "nested/payload.bin";

/// The frozen cell count: six vector files cut before and after their copy, the manifest cut before and after its write, the one sync group of the six files and the manifest cut before and after its syncs, six root, rename, and generations-directory events, and two nested-directory sync events.
const CELL_COUNT: usize = 24;

/// Every vector event in the order staging reports it; `the_cell_table_is_the_complete_event_trace` holds the store to exactly this sequence.
fn vector_cells() -> Vec<StageEvent> {
    let mut files = [
        CODES_FILE,
        ROW_IDS_FILE,
        ROWS_FILE,
        SCALES_FILE,
        TOMBSTONES_FILE,
        SIDECAR_FILE,
    ];
    files.sort_unstable();
    let mut cells = Vec::new();
    for file in files {
        cells.push(StageEvent::BeforeCopy(file.to_owned()));
        cells.push(StageEvent::AfterCopy(file.to_owned()));
    }
    cells.extend([
        StageEvent::BeforeManifestWrite,
        StageEvent::AfterManifestWrite,
        StageEvent::BeforeFileSyncs(0),
        StageEvent::AfterFileSyncs(0),
        StageEvent::BeforeRootSync,
        StageEvent::AfterRootSync,
        StageEvent::BeforeRename,
        StageEvent::AfterRename,
        StageEvent::BeforeGenerationsSync,
        StageEvent::AfterGenerationsSync,
    ]);
    cells
}

fn cells() -> Vec<(Workload, StageEvent)> {
    let mut cells: Vec<_> = vector_cells()
        .into_iter()
        .map(|event| (Workload::Vector, event))
        .collect();
    cells.push((
        Workload::Nested,
        StageEvent::BeforeDirectorySync(NESTED_DIR.to_owned()),
    ));
    cells.push((
        Workload::Nested,
        StageEvent::AfterDirectorySync(NESTED_DIR.to_owned()),
    ));
    assert_eq!(cells.len(), CELL_COUNT, "the cell table is frozen");
    cells
}

/// A cut at or before the rename leaves no digest-named directory; after it, every file and directory was synced before the rename, so the published directory is complete.
fn precedes_rename(event: &StageEvent) -> bool {
    !matches!(
        event,
        StageEvent::AfterRename
            | StageEvent::BeforeGenerationsSync
            | StageEvent::AfterGenerationsSync
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Staged(String),
    Exhausted,
    Fault,
}

type Observer<'a> = &'a mut dyn FnMut(StageEvent) -> Result<(), Errno>;

fn nested_meta() -> StageMeta {
    StageMeta {
        target: "nested-fixture".to_owned(),
        release_contract_sha256: "a".repeat(64),
        inputs_lock_sha256: "b".repeat(64),
        source_payload_manifest_sha256: "c".repeat(64),
    }
}

const NESTED_BYTES: &[u8] = b"nested payload bytes";

fn nested_digest() -> String {
    GenerationManifest::from_files(
        &nested_meta(),
        vec![ManifestFile {
            path: NESTED_FILE.to_owned(),
            mode: 0o600,
            size: NESTED_BYTES.len() as u64,
            sha256: format!("{:x}", sha2::Sha256::digest(NESTED_BYTES)),
        }],
    )
    .digest()
}

fn vector_build(fixture: &Fixture) -> BuiltVectors {
    build(&fixture.expected(), &export(1, 10), &fixture.work_dir()).unwrap()
}

/// Stages `workload` into `staging`'s store under `observer` and returns the digest it stages to with the outcome.
fn run(
    fixture: &Fixture,
    staging: &Staging<'_>,
    workload: Workload,
    observer: Observer<'_>,
) -> (String, Outcome) {
    match workload {
        Workload::Vector => {
            let built = vector_build(fixture);
            let outcome = match stage_observed(&built, staging, observer) {
                Ok(digest) => Outcome::Staged(digest),
                Err(VectorRefusal::InsufficientStorage) => Outcome::Exhausted,
                Err(VectorRefusal::Store(_)) => Outcome::Fault,
                Err(other) => panic!("unexpected refusal: {other:?}"),
            };
            (built.digest(), outcome)
        }
        Workload::Nested => {
            let dir = fixture.work_dir();
            let source = dir.join("payload.bin");
            fs::write(&source, NESTED_BYTES).unwrap();
            let staged = staging.store.stage_observed(
                &[SourceSpec {
                    rel_path: NESTED_FILE.to_owned(),
                    source,
                    executable: false,
                    expected_size: None,
                    expected_sha256: None,
                }],
                &nested_meta(),
                &BTreeSet::new(),
                observer,
            );
            let outcome = match staged {
                Ok(digest) => Outcome::Staged(digest),
                Err(GenerationError::InsufficientStorage) => Outcome::Exhausted,
                Err(GenerationError::NativePayloadInvalid { .. }) => Outcome::Fault,
                Err(other) => panic!("unexpected store error: {other:?}"),
            };
            (nested_digest(), outcome)
        }
    }
}

/// What a fresh handle on the store reads back; two reopenings must agree.
#[derive(Debug, PartialEq, Eq)]
struct Reopened {
    deployment: CurrentProfile,
    search: CurrentProfile,
    vector: CurrentProfile,
    entries: BTreeSet<String>,
    valid: bool,
}

fn reopen(root: &Path, digest: &str) -> Reopened {
    let store = GenerationStore::open(Some(root)).unwrap();
    let entries = fs::read_dir(store.root().join(GENERATIONS_DIR_NAME))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    Reopened {
        deployment: store.read_current().unwrap(),
        search: store.read_search_current().unwrap(),
        vector: store.read_vector_current().unwrap(),
        entries,
        valid: store.validate(digest).is_ok(),
    }
}

fn is_temp(name: &str) -> bool {
    name.starts_with("tmp-")
}

/// Checks the store at `root` after a cut at `cut` left `temps` staging temps: two reopenings agree, nothing is selected, an unpublished digest is refused by validation, verification, and both selectors, `prune` reclaims all residue, and a complete restaging then succeeds and selects.
fn assert_cut_recovers(
    fixture: &Fixture,
    root: &Path,
    workload: Workload,
    cut: &StageEvent,
    digest: &str,
    temps: usize,
) {
    let first = reopen(root, digest);
    assert_eq!(first, reopen(root, digest), "{cut:?}: reopening is stable");
    for selector in [&first.deployment, &first.search, &first.vector] {
        assert_eq!(
            *selector,
            CurrentProfile::Absent,
            "{cut:?}: nothing selected"
        );
    }
    let published = !precedes_rename(cut);
    let expected_entries: BTreeSet<String> =
        published.then(|| digest.to_owned()).into_iter().collect();
    assert_eq!(
        first.entries.iter().filter(|name| is_temp(name)).count(),
        temps,
        "{cut:?}: {:?}",
        first.entries
    );
    assert_eq!(
        first
            .entries
            .iter()
            .filter(|name| !is_temp(name))
            .cloned()
            .collect::<BTreeSet<_>>(),
        expected_entries,
        "{cut:?}"
    );
    assert_eq!(first.valid, published, "{cut:?}");

    let store = GenerationStore::open(Some(root)).unwrap();
    let tx = LifecycleTransactionLock::acquire_exclusive(Some(root)).unwrap();
    let verified = verify(&store, digest, &fixture.expected(), u64::MAX).is_ok();
    assert_eq!(
        verified,
        published && workload == Workload::Vector,
        "{cut:?}"
    );
    if !published {
        assert!(
            store.select_search(digest, &tx, &mut |_| Ok(())).is_err(),
            "{cut:?}"
        );
        assert!(
            store.select_vector(digest, &tx, &mut |_| Ok(())).is_err(),
            "{cut:?}"
        );
    }
    let report = store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(report.removed_temps, temps, "{cut:?}");
    assert_eq!(
        report.removed_generations,
        usize::from(published),
        "{cut:?}"
    );
    assert!(reopen(root, digest).entries.is_empty(), "{cut:?}");

    // The complete control on the same store stages the same digest and selects it.
    let protected = BTreeSet::new();
    let staging = Staging {
        store: &store,
        transaction: &tx,
        gate: &fixture.gate,
        admission: &fixture.admission,
        identity: &fixture.identity,
        ledger: &fixture.ledger,
        protected: &protected,
    };
    let (_, outcome) = run(fixture, &staging, workload, &mut |_| Ok(()));
    assert_eq!(outcome, Outcome::Staged(digest.to_owned()), "{cut:?}");
    if workload == Workload::Vector {
        assert!(verify(&store, digest, &fixture.expected(), u64::MAX).is_ok());
    }
    store.select_search(digest, &tx, &mut |_| Ok(())).unwrap();
    assert_eq!(
        GenerationStore::open(Some(root))
            .unwrap()
            .read_search_current()
            .unwrap(),
        CurrentProfile::Current(digest.to_owned())
    );
}

/// Cuts `cut` with `errno` in-process and checks the reported class, the cleanup, and recovery.
fn hook_cut(workload: Workload, cut: &StageEvent, errno: Errno) {
    let mut fixture = Fixture::new();
    let mut fired = false;
    let (digest, outcome) = run(&fixture, &fixture.staging(), workload, &mut |event| {
        if event == *cut {
            fired = true;
            Err(errno)
        } else {
            Ok(())
        }
    });
    assert!(fired, "declared cell {cut:?} never fired");
    let expected = if matches!(errno, Errno::NOSPC | Errno::DQUOT) {
        Outcome::Exhausted
    } else {
        Outcome::Fault
    };
    assert_eq!(outcome, expected, "{cut:?} {errno:?}");
    fixture.release_transaction();
    let root = fixture.root.path().to_owned();
    // A hook error before the rename removes this attempt's temp, so only a crash leaves one.
    assert_cut_recovers(&fixture, &root, workload, cut, &digest, 0);
}

#[test]
fn the_cell_table_is_the_complete_event_trace() {
    let fixture = Fixture::new();
    let mut vector = Vec::new();
    let (digest, outcome) = run(
        &fixture,
        &fixture.staging(),
        Workload::Vector,
        &mut |event| {
            vector.push(event);
            Ok(())
        },
    );
    assert_eq!(outcome, Outcome::Staged(digest));
    assert_eq!(
        vector,
        vector_cells(),
        "every vector event is a declared cell, in order"
    );

    let mut nested = Vec::new();
    let (digest, outcome) = run(
        &fixture,
        &fixture.staging(),
        Workload::Nested,
        &mut |event| {
            nested.push(event);
            Ok(())
        },
    );
    assert_eq!(outcome, Outcome::Staged(digest));
    let declared: Vec<StageEvent> = cells()
        .into_iter()
        .filter(|(workload, _)| *workload == Workload::Nested)
        .map(|(_, event)| event)
        .collect();
    let dir_events: Vec<StageEvent> = nested
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                StageEvent::BeforeDirectorySync(_) | StageEvent::AfterDirectorySync(_)
            )
        })
        .collect();
    assert_eq!(dir_events, declared);
}

#[test]
fn a_hook_error_at_every_stage_event_selects_nothing_and_prune_reclaims_the_residue() {
    for (workload, cut) in cells() {
        hook_cut(workload, &cut, Errno::IO);
    }
}

/// Hook-simulated: the observer returns the errno a full disk or an exhausted quota reports, at each write and sync site; real-filesystem exhaustion is not exercised here.
#[test]
fn hook_simulated_storage_exhaustion_at_every_stage_event_reports_insufficient_storage() {
    for (workload, cut) in cells() {
        for errno in [Errno::NOSPC, Errno::DQUOT] {
            hook_cut(workload, &cut, errno);
        }
    }
}

/// Hook-simulated: `EIO` at each `After*` sync event models a delayed write or sync failure reported once the step returned.
#[test]
fn hook_simulated_delayed_sync_failure_after_every_sync_is_a_payload_fault() {
    let delayed: Vec<_> = cells()
        .into_iter()
        .filter(|(_, event)| {
            matches!(
                event,
                StageEvent::AfterFileSyncs(_)
                    | StageEvent::AfterDirectorySync(_)
                    | StageEvent::AfterRootSync
                    | StageEvent::AfterGenerationsSync
            )
        })
        .collect();
    assert_eq!(
        delayed.len(),
        4,
        "the file group, nested, root, generations"
    );
    for (workload, cut) in delayed {
        hook_cut(workload, &cut, Errno::IO);
    }
}

const KILL_CELL: &str = "EIDNARA_STAGE_KILL_CELL";
const KILL_WITNESS: &str = "EIDNARA_STAGE_KILL_WITNESS";
/// The signal `std::process::abort` raises.
const SIGABRT: i32 = 6;

/// Writes the witness file, syncs it, and aborts, so the parent knows the child reached the cut before it died.
fn witness_and_abort(witness: &Path, root: &Path, cut: &StageEvent) -> ! {
    let mut file = fs::File::create(witness).unwrap();
    writeln!(file, "{}", root.display()).unwrap();
    write!(file, "{cut:?}").unwrap();
    file.sync_all().unwrap();
    std::process::abort();
}

/// Each child stages one workload and aborts at its cell after syncing a witness; the parent reads the witness, reopens the abandoned store twice, and checks refusal, reclamation, and a complete control.
#[test]
fn a_process_killed_at_every_stage_event_leaves_residue_the_next_process_refuses_and_reclaims() {
    if let Some(index) = std::env::var_os(KILL_CELL) {
        let index: usize = index.to_str().unwrap().parse().unwrap();
        let (workload, cut) = cells().swap_remove(index);
        let witness = PathBuf::from(std::env::var_os(KILL_WITNESS).unwrap());
        let fixture = Fixture::new();
        let root = fixture.root.path().to_owned();
        run(&fixture, &fixture.staging(), workload, &mut |event| {
            if event == cut {
                witness_and_abort(&witness, &root, &cut);
            }
            Ok(())
        });
        // Reaching here means the cell never fired; the parent reports the clean exit.
        return;
    }
    let parent = Fixture::new();
    let scratch = tempfile::tempdir_in(parent.root.path()).unwrap();
    for (index, (workload, cut)) in cells().into_iter().enumerate() {
        let witness = scratch.path().join(format!("witness-{index}"));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "a_process_killed_at_every_stage_event_leaves_residue_the_next_process_refuses_and_reclaims",
                "--nocapture",
            ])
            .env(KILL_CELL, index.to_string())
            .env(KILL_WITNESS, &witness)
            .env("TMPDIR", scratch.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.signal(),
            Some(SIGABRT),
            "declared cell {cut:?} did not abort the child: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let witnessed = fs::read_to_string(&witness).unwrap();
        let (root, event) = witnessed.split_once('\n').unwrap();
        assert_eq!(event, format!("{cut:?}"), "the witness names the cell");
        let root = PathBuf::from(root);
        // A crash before the rename abandons the staging temp; prune reclaims it.
        let temps = usize::from(precedes_rename(&cut));
        let digest = match workload {
            Workload::Vector => vector_build(&parent).digest(),
            Workload::Nested => nested_digest(),
        };
        assert_cut_recovers(&parent, &root, workload, &cut, &digest, temps);
    }
}

/// Stages a deployment generation and makes it the deployment selection through `current-profile.json`.
fn select_deployment(fixture: &Fixture) -> (String, Vec<u8>) {
    let dir = fixture.work_dir();
    let path = dir.join("payload.bin");
    fs::write(&path, b"deployment payload").unwrap();
    let digest = fixture
        .store
        .stage_and_promote(
            &[SourceSpec {
                rel_path: "payload.bin".to_owned(),
                source: path,
                executable: false,
                expected_size: None,
                expected_sha256: None,
            }],
            &nested_meta(),
            &BTreeSet::new(),
        )
        .unwrap();
    (
        digest,
        fixture.selector_bytes(CURRENT_PROFILE_NAME).unwrap(),
    )
}

#[test]
fn vector_stage_select_prune_discard_and_exchange_repair_leave_the_deployment_selector_bytes_unchanged()
 {
    let fixture = Fixture::new();
    let (selected, deployment) = select_deployment(&fixture);
    let unchanged = |step: &str| {
        assert_eq!(
            fixture.selector_bytes(CURRENT_PROFILE_NAME).unwrap(),
            deployment,
            "{step} rewrote the deployment selector"
        );
    };

    let base = fixture.layer(1, 10);
    unchanged("stage");
    let composition = fixture.compose(1, &base, &[]).unwrap();
    publish(
        &composition,
        &fixture.staging(),
        &fixture.work_dir(),
        &mut |_| Ok(()),
    )
    .unwrap();
    unchanged("select");

    let stray = fixture.layer(7, 20).digest;
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        report.removed_generations, 1,
        "the unselected stray is reclaimed"
    );
    assert!(!fixture.generations().contains(&stray));
    unchanged("prune");

    let discarded = fixture.layer(8, 30);
    let manifest = fixture.store.manifest(&discarded.digest).unwrap();
    drop(discarded);
    fixture
        .store
        .discard_unselected(&manifest, fixture.transaction(), &BTreeSet::new())
        .unwrap();
    assert!(!fixture.generations().contains(&manifest.digest()));
    unchanged("discard");

    let repaired = fixture.layer(9, 40);
    let digest = repaired.digest.clone();
    drop(repaired);
    fixture.corrupt(&digest, CODES_FILE);
    assert!(fixture.store.validate(&digest).is_err());
    let rebuilt = build(&fixture.expected(), &export(9, 40), &fixture.work_dir()).unwrap();
    assert_eq!(stage(&rebuilt, &fixture.staging()).unwrap(), digest);
    assert!(
        fixture.store.validate(&digest).is_ok(),
        "the corrupt target was exchange-repaired"
    );
    unchanged("exchange repair");

    // A cut staging leaves the deployment selector alone as well.
    let cut = stage_observed(&vector_build(&fixture), &fixture.staging(), &mut |event| {
        if event == StageEvent::BeforeRename {
            Err(Errno::IO)
        } else {
            Ok(())
        }
    });
    assert!(cut.is_err());
    unchanged("a cut staging");
    assert_eq!(
        fixture.store.read_current().unwrap(),
        CurrentProfile::Current(selected)
    );
}
