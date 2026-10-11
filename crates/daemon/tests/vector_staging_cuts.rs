//! Cuts inside the lifecycle store's staging path, driven through its [`StageEvent`] observer.
//!
//! Every cell of the frozen table is cut three ways: a hook-simulated `EIO`, hook-simulated storage exhaustion (`ENOSPC`, `EDQUOT`), and a process kill in a re-executed child that writes and syncs a witness file and then sends itself `SIGKILL`. The hook cells exercise the store's error classification and cleanup at each step; they are not kernel `fsync` evidence. The kill cells show what a crash at each step leaves on disk for the next process, not what survives power loss.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
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
    StageObserver,
};
use host_runtime::lifecycle::LifecycleTransactionLock;
use rustix::io::Errno;
use sha2::Digest;

use support::vector_store::*;

/// The workload whose staging a cell cuts. Vector layers are flat, so the nested-directory sync is cut on a store-level generation with one nested file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workload {
    /// A fresh vector layer: the rename publishes the digest name.
    Vector,
    Nested,
    /// The vector layer restaged over a corrupt occupant of its digest name: the rename finds the name taken and the store exchanges the occupant out.
    Repair,
    /// The vector layer restaged over a valid occupant of its digest name: the store keeps the occupant and drops the temp.
    Retry,
}

const NESTED_DIR: &str = "nested";
const NESTED_FILE: &str = "nested/payload.bin";

/// The frozen cell count: the fresh vector trace (six files cut before and after their copy, the manifest before and after its write, the one sync group of the six files and the manifest before and after its syncs, two root, two rename, and two generations-directory events) is 22, the repair and retry traces omit `AfterRename` and are 21 each, and the nested workload adds two directory-sync events.
const CELL_COUNT: usize = 66;

/// Every event a vector `workload` fires, in the order staging reports it; `the_cell_table_is_the_complete_event_trace` holds the store to exactly this sequence.
fn vector_cells(workload: Workload) -> Vec<StageEvent> {
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
    ]);
    if workload == Workload::Vector {
        cells.push(StageEvent::AfterRename);
    }
    cells.extend([
        StageEvent::BeforeGenerationsSync,
        StageEvent::AfterGenerationsSync,
    ]);
    cells
}

fn cells() -> Vec<(Workload, StageEvent)> {
    let mut cells: Vec<_> = [Workload::Vector, Workload::Repair, Workload::Retry]
        .into_iter()
        .flat_map(|workload| {
            vector_cells(workload)
                .into_iter()
                .map(move |event| (workload, event))
        })
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
    !matches!(event, StageEvent::AfterRename) && !syncs_generations(event)
}

/// On the repair and retry paths the generations-directory sync follows the exchange or the temp's removal.
fn syncs_generations(event: &StageEvent) -> bool {
    matches!(
        event,
        StageEvent::BeforeGenerationsSync | StageEvent::AfterGenerationsSync
    )
}

/// What a cut at one cell leaves on disk.
#[derive(Debug, PartialEq, Eq)]
struct Expected {
    /// Staging temps left in the generations directory.
    temps: usize,
    /// Whether a directory has the digest name.
    present: bool,
    /// Whether that directory validates.
    valid: bool,
    /// For repair and retry, whether the digest name still holds the seeded occupant's inode.
    same_occupant: Option<bool>,
}

/// The residue a hook error (`killed == false`) or a process kill at `cut` leaves. A hook error removes this attempt's temp; a crash abandons it. A repair cut after the exchange leaves the repaired generation under the digest name and the corrupt occupant in the temp a crash abandons.
fn residue(workload: Workload, cut: &StageEvent, killed: bool) -> Expected {
    let exchanged = syncs_generations(cut);
    match workload {
        Workload::Vector | Workload::Nested => {
            let published = !precedes_rename(cut);
            Expected {
                temps: usize::from(killed && !published),
                present: published,
                valid: published,
                same_occupant: None,
            }
        }
        Workload::Retry => Expected {
            temps: usize::from(killed && !exchanged),
            present: true,
            valid: true,
            same_occupant: Some(true),
        },
        Workload::Repair => Expected {
            temps: usize::from(killed),
            present: true,
            valid: exchanged,
            same_occupant: Some(!exchanged),
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Staged(String),
    Exhausted,
    Fault,
}

type Observer<'a> = &'a mut StageObserver<'a>;

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

/// Stages the occupant a repair or retry workload restages over, corrupting it for repair, and returns its directory inode.
fn seed(fixture: &Fixture, workload: Workload) -> Option<u64> {
    if !matches!(workload, Workload::Repair | Workload::Retry) {
        return None;
    }
    let digest = stage(&vector_build(fixture), &fixture.staging()).unwrap();
    if workload == Workload::Repair {
        fixture.corrupt(&digest, CODES_FILE);
        assert!(fixture.store.validate(&digest).is_err());
    }
    Some(occupant_inode(&fixture.generation_dir(&digest)))
}

fn occupant_inode(dir: &Path) -> u64 {
    fs::metadata(dir).unwrap().ino()
}

/// Stages `workload` into `staging`'s store under `observer` and returns the digest it stages to with the outcome.
fn run(
    fixture: &Fixture,
    staging: &Staging<'_>,
    workload: Workload,
    observer: Observer<'_>,
) -> (String, Outcome) {
    match workload {
        Workload::Vector | Workload::Repair | Workload::Retry => {
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

/// Checks the store at `root` after a cut at `cut` left the `expected` residue: two reopenings agree, nothing is selected, a repair or retry occupant is replaced only by the exchange, a digest that does not validate is refused by verification and both selectors, `prune` reclaims all residue, and a complete restaging then succeeds and selects.
fn assert_cut_recovers(
    fixture: &Fixture,
    root: &Path,
    (workload, cut): (Workload, &StageEvent),
    digest: &str,
    expected: &Expected,
    occupant: Option<u64>,
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
    let expected_entries: BTreeSet<String> = expected
        .present
        .then(|| digest.to_owned())
        .into_iter()
        .collect();
    assert_eq!(
        first.entries.iter().filter(|name| is_temp(name)).count(),
        expected.temps,
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
    assert_eq!(first.valid, expected.valid, "{cut:?}");
    let store = GenerationStore::open(Some(root)).unwrap();
    let dir = store.root().join(GENERATIONS_DIR_NAME).join(digest);
    assert_eq!(
        occupant.map(|inode| inode == occupant_inode(&dir)),
        expected.same_occupant,
        "{cut:?}: the occupant is replaced only by the exchange"
    );

    let tx = LifecycleTransactionLock::acquire_exclusive(Some(root)).unwrap();
    let verified = verify(&store, digest, &fixture.expected(), u64::MAX).is_ok();
    assert_eq!(
        verified,
        expected.valid && workload != Workload::Nested,
        "{cut:?}"
    );
    if !expected.valid {
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
    assert_eq!(report.removed_temps, expected.temps, "{cut:?}");
    assert_eq!(
        report.removed_generations,
        usize::from(expected.present),
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
    if workload != Workload::Nested {
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
    let occupant = seed(&fixture, workload);
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
    let expected = residue(workload, cut, false);
    assert_cut_recovers(
        &fixture,
        &root,
        (workload, cut),
        &digest,
        &expected,
        occupant,
    );
}

#[test]
fn the_cell_table_is_the_complete_event_trace() {
    for workload in [Workload::Vector, Workload::Repair, Workload::Retry] {
        let fixture = Fixture::new();
        seed(&fixture, workload);
        let mut trace = Vec::new();
        let (digest, outcome) = run(&fixture, &fixture.staging(), workload, &mut |event| {
            trace.push(event);
            Ok(())
        });
        assert_eq!(outcome, Outcome::Staged(digest), "{workload:?}");
        assert_eq!(
            trace,
            vector_cells(workload),
            "every {workload:?} event is a declared cell, in order"
        );
    }

    let fixture = Fixture::new();
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

/// The `After*` sync cells double as the delayed write and sync failure cells: the observer reports `EIO` once the step returned.
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

const KILL_CELL: &str = "EIDNARA_STAGE_KILL_CELL";
const KILL_WITNESS: &str = "EIDNARA_STAGE_KILL_WITNESS";
/// Writes the witness file naming the store, the cell, and any seeded occupant's inode, syncs it, and kills the process with `SIGKILL`, so the parent knows the child reached the cut before it died and no core dump is written.
fn witness_and_kill(witness: &Path, root: &Path, cut: &StageEvent, occupant: Option<u64>) -> ! {
    let mut file = fs::File::create(witness).unwrap();
    writeln!(file, "{}", root.display()).unwrap();
    writeln!(file, "{cut:?}").unwrap();
    write!(file, "{occupant:?}").unwrap();
    file.sync_all().unwrap();
    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL)
        .unwrap();
    unreachable!("SIGKILL ends the process");
}

/// Each child seeds any occupant, stages one workload, and kills itself at its cell after syncing a witness; the parent reads the witness, reopens the abandoned store twice, and checks refusal, reclamation, and a complete control.
#[test]
fn a_process_killed_at_every_stage_event_leaves_residue_the_next_process_refuses_and_reclaims() {
    if let Some(index) = std::env::var_os(KILL_CELL) {
        let index: usize = index.to_str().unwrap().parse().unwrap();
        let (workload, cut) = cells().swap_remove(index);
        let witness = PathBuf::from(std::env::var_os(KILL_WITNESS).unwrap());
        let fixture = Fixture::new();
        let root = fixture.root.path().to_owned();
        let occupant = seed(&fixture, workload);
        run(&fixture, &fixture.staging(), workload, &mut |event| {
            if event == cut {
                witness_and_kill(&witness, &root, &cut, occupant);
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
            Some(libc::SIGKILL),
            "declared cell {cut:?} did not kill the child: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let witnessed = fs::read_to_string(&witness).unwrap();
        let [root, event, occupant] = witnessed.lines().collect::<Vec<_>>()[..] else {
            panic!("malformed witness {witnessed:?}");
        };
        assert_eq!(event, format!("{cut:?}"), "the witness names the cell");
        let root = PathBuf::from(root);
        let occupant = match occupant {
            "None" => None,
            inode => Some(
                inode
                    .strip_prefix("Some(")
                    .and_then(|inode| inode.strip_suffix(')'))
                    .unwrap()
                    .parse()
                    .unwrap(),
            ),
        };
        let digest = match workload {
            Workload::Nested => nested_digest(),
            Workload::Vector | Workload::Repair | Workload::Retry => vector_build(&parent).digest(),
        };
        let expected = residue(workload, &cut, true);
        assert_cut_recovers(
            &parent,
            &root,
            (workload, &cut),
            &digest,
            &expected,
            occupant,
        );
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
        fixture.predecessor().as_deref(),
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
