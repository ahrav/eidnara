//! Staging cut by failures the kernel reports, not by the stage observer. The observer here only records events and, for the exhaustion cells, consumes free space or inodes through ordinary file operations before the step runs; the error each cell asserts comes from the step's own syscall.
//!
//! Two mechanisms run:
//! - A file-size limit (`RLIMIT_FSIZE`) in a re-executed child with `SIGXFSZ` ignored. The write that crosses the limit returns a short count and the next one `EFBIG`, so the copy loop's short-write handling and the write-failure path run on real syscall results. This runs everywhere.
//! - A size- and inode-limited `tmpfs` mounted in an unprivileged user and mount namespace. Data and inode exhaustion return a real `ENOSPC` from `write`, `openat`, or `mkdirat`. The test is ignored because it needs `unshare -Urm`; CI runs it in its own step.
//!
//! Neither mechanism exercises a failed `fsync`, write-back error, quota (`EDQUOT`), or power loss: `tmpfs` syncs nothing and this kernel lacks `tmpfs` quotas. Those cells stay unmet here.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

use daemon::vector_admission::Census;
use daemon::vector_generation::{
    BuiltVectors, CODES_FILE, ROW_IDS_FILE, ROWS_FILE, SCALES_FILE, SIDECAR_FILE, TOMBSTONES_FILE,
    VectorRefusal, build, stage, stage_observed, verify,
};
use host_runtime::generation::{CurrentProfile, GENERATIONS_DIR_NAME, GenerationStore, StageEvent};
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};

use support::vector_store::*;

const MANIFEST_FILE: &str = "manifest.json";
const CHILD_CELL: &str = "EIDNARA_KERNEL_FAULT_CELL";
const CHILD_WITNESS: &str = "EIDNARA_KERNEL_FAULT_WITNESS";

/// Dimension and row count of the wide layer, so `rows.f32` spans several pages and a write can be cut inside it.
const WIDE: u32 = 1024;
const WIDE_ROWS: u8 = 8;

/// Every file the vector stager writes, in the order it writes them: the six payload files in path order, then the manifest.
fn staged_files() -> Vec<&'static str> {
    let mut files = vec![
        CODES_FILE,
        ROW_IDS_FILE,
        ROWS_FILE,
        SCALES_FILE,
        TOMBSTONES_FILE,
        SIDECAR_FILE,
    ];
    files.sort_unstable();
    files.push(MANIFEST_FILE);
    files
}

/// The event that precedes writing `file`.
fn before_write(file: &str) -> StageEvent {
    if file == MANIFEST_FILE {
        StageEvent::BeforeManifestWrite
    } else {
        StageEvent::BeforeCopy(file.to_owned())
    }
}

/// The event that follows writing `file`.
fn after_write(file: &str) -> StageEvent {
    if file == MANIFEST_FILE {
        StageEvent::AfterManifestWrite
    } else {
        StageEvent::AfterCopy(file.to_owned())
    }
}

fn wide_export(fixture: &Fixture) -> LiveRows {
    let mut state = 0x2545_f491u32;
    let rows = (0..WIDE_ROWS)
        .map(|index| {
            let raw: Vec<f64> = (0..WIDE)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    f64::from(state >> 8) / f64::from(1u32 << 24) - 0.5
                })
                .collect();
            let norm = raw.iter().map(|value| value * value).sum::<f64>().sqrt();
            ExportedRow {
                occurrence_id: format!("{:02x}", index + 1).repeat(32),
                vector: raw.iter().map(|value| (value / norm) as f32).collect(),
            }
        })
        .collect();
    LiveRows {
        generation: fixture.generation.clone(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: 9,
            checkpoint_commit_seq: 10,
            hold_id: "hold-7".to_owned(),
        },
        rows,
        tombstones: Vec::new(),
    }
}

fn wide_fixture() -> (Fixture, BuiltVectors) {
    let fixture = Fixture::with_dimension(WIDE);
    let built = build(
        &fixture.expected(),
        &wide_export(&fixture),
        &fixture.work_dir(),
    )
    .unwrap();
    (fixture, built)
}

/// The size of each file the build stages, the manifest included.
fn sizes(built: &BuiltVectors) -> Vec<(&'static str, u64)> {
    let manifest = built.sidecar.stage_manifest();
    staged_files()
        .into_iter()
        .map(|name| {
            let size = if name == MANIFEST_FILE {
                manifest.canonical_bytes().len() as u64
            } else {
                manifest
                    .files
                    .iter()
                    .find(|file| file.path == name)
                    .unwrap()
                    .size
            };
            (name, size)
        })
        .collect()
}

/// Checks the store after a kernel-reported failure cut the staging of `built`: the last event the observer saw is `cut`, two reopenings agree that nothing is selected and nothing is named, the ledger holds nothing, `prune` finds no residue, and a complete staging of the same build then succeeds, verifies, and selects.
fn assert_recovers(
    fixture: &Fixture,
    built: &BuiltVectors,
    trace: &[StageEvent],
    cut: &StageEvent,
) {
    assert_eq!(
        trace.last(),
        Some(cut),
        "the failure came from the cut step"
    );
    let root = fixture.root.path();
    let reopen = || {
        let store = GenerationStore::open(Some(root)).unwrap();
        let entries: BTreeSet<String> = fs::read_dir(store.root().join(GENERATIONS_DIR_NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        (
            store.read_search_current().unwrap(),
            store.read_vector_current().unwrap(),
            entries,
        )
    };
    let first = reopen();
    assert_eq!(first, reopen(), "{cut:?}: reopening is stable");
    assert_eq!(
        first,
        (
            CurrentProfile::Absent,
            CurrentProfile::Absent,
            BTreeSet::new()
        ),
        "{cut:?}: the failed attempt removed its temp and published nothing"
    );
    assert_eq!(fixture.ledger.census(), Census::default(), "{cut:?}");
    let report = fixture.store.prune(&BTreeSet::new()).unwrap();
    assert_eq!(
        (report.removed_temps, report.removed_generations),
        (0, 0),
        "{cut:?}"
    );
    let digest = stage(built, &fixture.staging()).unwrap();
    assert_eq!(digest, built.digest(), "{cut:?}");
    assert!(verify(&fixture.store, &digest, &fixture.expected(), u64::MAX).is_ok());
    fixture
        .store
        .select_vector(&digest, fixture.transaction(), &mut |_| Ok(()))
        .expect_err("a bare layer is not a composition");
    fixture
        .store
        .select_search(&digest, fixture.transaction(), &mut |_| Ok(()))
        .unwrap();
}

/// One cell of the file-size-limit matrix: the file whose write crosses the limit, and the limit.
struct SizeCell {
    file: &'static str,
    limit: u64,
}

/// The frozen file-size-limit matrix over the wide layer. A limit cuts the first file larger than it, so a cell exists only for a file larger than every file written before it; each cell's limit is half that file, raised to the largest earlier file. The cell list is fixed before any cell runs, and each precondition is asserted.
fn size_cells(sizes: &[(&'static str, u64)]) -> Vec<SizeCell> {
    let cells: Vec<SizeCell> = [CODES_FILE, ROWS_FILE]
        .into_iter()
        .map(|file| {
            let index = sizes.iter().position(|(name, _)| *name == file).unwrap();
            let earlier = sizes[..index]
                .iter()
                .map(|(_, size)| *size)
                .max()
                .unwrap_or(0);
            let size = sizes[index].1;
            let limit = earlier.max(size / 2);
            assert!(
                limit < size,
                "{file}: a limit of {limit} cannot cut a {size}-byte write after {earlier} bytes"
            );
            SizeCell { file, limit }
        })
        .collect();
    assert_eq!(cells.len(), 2, "the size matrix is frozen");
    cells
}

/// Runs one size cell in this process: lowers `RLIMIT_FSIZE`, stages under an observer that records every event, restores the limit, and checks the outcome and recovery. The caller ignores `SIGXFSZ`, so the crossing write returns a short count and the next one `EFBIG`.
fn run_size_cell(index: usize, witness: &Path) {
    let (fixture, built) = wide_fixture();
    let cell = &size_cells(&sizes(&built))[index];
    let mut trace = Vec::new();
    let unlimited = rustix::process::getrlimit(rustix::process::Resource::Fsize);
    rustix::process::setrlimit(
        rustix::process::Resource::Fsize,
        rustix::process::Rlimit {
            current: Some(cell.limit),
            maximum: unlimited.maximum,
        },
    )
    .unwrap();
    let staged = stage_observed(&built, &fixture.staging(), &mut |event| {
        trace.push(event);
        Ok(())
    });
    rustix::process::setrlimit(rustix::process::Resource::Fsize, unlimited).unwrap();
    assert_eq!(
        staged,
        Err(VectorRefusal::Store(
            "native payload invalid: staging output write failed".to_owned()
        )),
        "{}: EFBIG is a write fault, not exhaustion",
        cell.file
    );
    assert!(!trace.contains(&after_write(cell.file)), "{}", cell.file);
    assert_recovers(&fixture, &built, &trace, &before_write(cell.file));
    fs::write(witness, format!("{:?}", before_write(cell.file))).unwrap();
}

/// A file-size limit cuts the copy of a file mid-write. Each cell runs in a child that ignores `SIGXFSZ`, so the kernel's short write and `EFBIG` reach the copy loop instead of killing the process.
#[test]
fn a_file_size_limit_cuts_a_copy_with_a_short_write_and_efbig_and_the_store_recovers() {
    if let Some(index) = std::env::var_os(CHILD_CELL) {
        let witness = PathBuf::from(std::env::var_os(CHILD_WITNESS).unwrap());
        run_size_cell(index.to_str().unwrap().parse().unwrap(), &witness);
        return;
    }
    let (_, built) = wide_fixture();
    let cells = size_cells(&sizes(&built));
    let scratch = tempfile::tempdir().unwrap();
    for (index, cell) in cells.iter().enumerate() {
        let witness = scratch.path().join(format!("witness-{index}"));
        let output = std::process::Command::new("sh")
            .args([
                "-c",
                "trap '' XFSZ; exec \"$@\"",
                "sh",
                std::env::current_exe().unwrap().to_str().unwrap(),
                "--exact",
                "a_file_size_limit_cuts_a_copy_with_a_short_write_and_efbig_and_the_store_recovers",
                "--nocapture",
            ])
            .env(CHILD_CELL, index.to_string())
            .env(CHILD_WITNESS, &witness)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {:?}\n{}\n{}",
            cell.file,
            output.status.signal(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&witness).unwrap(),
            format!("{:?}", before_write(cell.file)),
            "the child ran the declared cell"
        );
    }
}

/// What the observer exhausts before a step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exhaust {
    /// Every free data block, so the step's first write fails.
    Blocks,
    /// All but enough blocks for half of the file, so the write is short before it fails.
    BlocksPartway,
    /// Every free inode, so the step's file creation fails.
    Inodes,
}

/// One cell of the exhaustion matrix: what is exhausted, and before which step. `None` exhausts inodes before staging starts, so the staging temp's `mkdirat` fails.
#[derive(Debug, Clone, Copy)]
struct SpaceCell {
    exhaust: Exhaust,
    file: Option<&'static str>,
}

/// The frozen exhaustion matrix: data blocks and inodes before every staged file and the manifest, a write cut partway through `rows.f32`, and inodes before the staging temp is created.
fn space_cells() -> Vec<SpaceCell> {
    let mut cells = Vec::new();
    for file in staged_files() {
        for exhaust in [Exhaust::Blocks, Exhaust::Inodes] {
            cells.push(SpaceCell {
                exhaust,
                file: Some(file),
            });
        }
    }
    cells.push(SpaceCell {
        exhaust: Exhaust::BlocksPartway,
        file: Some(ROWS_FILE),
    });
    cells.push(SpaceCell {
        exhaust: Exhaust::Inodes,
        file: None,
    });
    assert_eq!(cells.len(), 16, "the exhaustion matrix is frozen");
    cells
}

/// Fills the filesystem holding `dir` as `exhaust` says, leaving `keep` free blocks, inside a directory the caller removes to release the space.
fn exhaust(dir: &Path, exhaust: Exhaust, keep: u64) -> tempfile::TempDir {
    let fill = tempfile::Builder::new()
        .prefix("fill-")
        .tempdir_in(dir)
        .unwrap();
    match exhaust {
        Exhaust::Blocks | Exhaust::BlocksPartway => {
            let stat = rustix::fs::statvfs(dir).unwrap();
            let free = stat.f_bavail.saturating_sub(keep) * stat.f_frsize;
            let file = fs::File::create(fill.path().join("blocks")).unwrap();
            rustix::fs::fallocate(&file, rustix::fs::FallocateFlags::empty(), 0, free).unwrap();
            assert_eq!(rustix::fs::statvfs(dir).unwrap().f_bavail, keep);
        }
        Exhaust::Inodes => {
            let mut count = 0usize;
            loop {
                match fs::File::create(fill.path().join(format!("inode-{count}"))) {
                    Ok(_) => count += 1,
                    Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => break,
                    Err(error) => panic!("filling inodes: {error}"),
                }
            }
            assert_eq!(rustix::fs::statvfs(dir).unwrap().f_ffree, 0);
        }
    }
    fill
}

/// Runs every exhaustion cell inside the namespace's `tmpfs`, where `TMPDIR` points.
fn run_space_cells() {
    for cell in space_cells() {
        let (fixture, built) = wide_fixture();
        let sizes = sizes(&built);
        let root = fixture.root.path().to_owned();
        let page = rustix::fs::statvfs(&root).unwrap().f_frsize;
        let mut trace = Vec::new();
        let mut held = None;
        if cell.file.is_none() {
            held = Some(exhaust(&root, cell.exhaust, 0));
        }
        let cut = cell.file.map(before_write);
        let staged = stage_observed(&built, &fixture.staging(), &mut |event| {
            if Some(&event) == cut.as_ref() {
                let keep = match cell.exhaust {
                    Exhaust::BlocksPartway => {
                        let size = sizes
                            .iter()
                            .find(|(name, _)| Some(*name) == cell.file)
                            .unwrap()
                            .1;
                        assert!(size >= 4 * page, "{cell:?}: the file spans several pages");
                        size / 2 / page
                    }
                    Exhaust::Blocks | Exhaust::Inodes => 0,
                };
                held = Some(exhaust(&root, cell.exhaust, keep));
            }
            trace.push(event);
            Ok(())
        });
        drop(held.expect("the cell fired"));
        assert_eq!(
            staged,
            Err(VectorRefusal::InsufficientStorage),
            "{cell:?}: the kernel's ENOSPC is storage exhaustion"
        );
        match cell.file {
            Some(file) => {
                assert!(!trace.contains(&after_write(file)), "{cell:?}");
                assert_recovers(&fixture, &built, &trace, &before_write(file));
            }
            None => {
                assert!(trace.is_empty(), "{cell:?}: the temp was never created");
                let store = GenerationStore::open(Some(&root)).unwrap();
                assert_eq!(
                    fs::read_dir(store.root().join(GENERATIONS_DIR_NAME))
                        .unwrap()
                        .count(),
                    0
                );
                assert_eq!(stage(&built, &fixture.staging()).unwrap(), built.digest());
            }
        }
    }
}

const SPACE_CHILD: &str = "EIDNARA_KERNEL_SPACE_CHILD";

/// Each exhaustion cell consumes free data blocks or inodes of a `tmpfs` just before one staging step, so that step's `write`, `openat`, or `mkdirat` gets the kernel's `ENOSPC`. The `tmpfs` is larger than the store's lifecycle reserve, so the store's own free-space check passes before the cell consumes the space.
#[test]
#[ignore = "needs an unprivileged user and mount namespace (`unshare -Urm`)"]
fn kernel_storage_exhaustion_at_every_staged_write_reports_insufficient_storage_and_recovers() {
    if std::env::var_os(SPACE_CHILD).is_some() {
        run_space_cells();
        return;
    }
    let mount = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    // The store refuses ancestors it does not own, and every host directory belongs to an unmapped root inside the namespace, so the child runs chrooted into the `tmpfs` with the host's system directories and the test binary's directory bound in.
    let script = r#"
        root=$0; exe_dir=$1; exe_name=$2; shift 2
        mount -t tmpfs -o size=320m,nr_inodes=4096,mode=0700 eidnara-faults "$root" || exit 101
        for dir in /usr /lib /lib64 /bin /sbin /etc /dev; do
            if [ -L "$dir" ]; then ln -s "$(readlink "$dir")" "$root$dir"
            elif [ -d "$dir" ]; then mkdir -p "$root$dir" && mount --rbind "$dir" "$root$dir" || exit 102
            fi
        done
        mkdir "$root/exe" "$root/work" && mount --bind "$exe_dir" "$root/exe" || exit 103
        TMPDIR=/work exec chroot "$root" "/exe/$exe_name" "$@"
    "#;
    let output = std::process::Command::new("unshare")
        .args([
            "-Urm",
            "sh",
            "-c",
            script,
            mount.path().to_str().unwrap(),
            exe.parent().unwrap().to_str().unwrap(),
            exe.file_name().unwrap().to_str().unwrap(),
            "--exact",
            "kernel_storage_exhaustion_at_every_staged_write_reports_insufficient_storage_and_recovers",
            "--ignored",
            "--nocapture",
        ])
        .env(SPACE_CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1 passed"),
        "the child ran the exhaustion cells: {stdout}"
    );
}
