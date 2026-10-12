//! Each probe here is `#[ignore]`d and prints figures that characterize this host; its assertions check only completion and result shapes.
//! Run in release: `cargo test --release --locked -p daemon --test rp25_host_measurements -- --ignored --nocapture --test-threads=1`.

#[allow(dead_code)]
#[path = "support/alloc_recorder.rs"]
mod alloc_recorder;
mod support;

use std::fs;
use std::hint::black_box;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

use alloc_recorder::record_window;
use daemon::vector_admission::{Ledger, ResourceClass};
use daemon::vector_composition::{SelectorState, recover};
use daemon::vector_generation::{ROWS_FILE, build, stage, verify};
use daemon::vector_reader::acquire;
use host_runtime::generation::GenerationStore;
use retrieval::batch::ProjectionCheckpoint;
use retrieval::dense::export::{ExportedRow, LiveRows};
use support::vector_reads::bounds;
use support::vector_store::{Fixture, KERNEL};

#[global_allocator]
static GLOBAL: alloc_recorder::RecordingAlloc = alloc_recorder::RecordingAlloc;

const DIMENSION: u32 = 384;
const BASE: usize = 10_000;

/// At least one run, so every median has a sample.
fn runs() -> usize {
    std::env::var("EIDNARA_RP25_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(11)
        .max(1)
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn summary(samples: &[Duration]) -> String {
    format!(
        "n={} median={:?} min={:?} max={:?}",
        samples.len(),
        median(samples.to_vec()),
        samples.iter().min().unwrap(),
        samples.iter().max().unwrap()
    )
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed())
}

/// SplitMix64 rows seeded by `seed` and the row index; equal arguments produce equal rows.
fn unit(seed: u64, index: usize) -> Vec<f32> {
    let mut state = seed.wrapping_mul(0x1000_0000_01b3) ^ index as u64;
    let raw: Vec<f64> = (0..DIMENSION)
        .map(|_| {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            ((z >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        })
        .collect();
    let norm = raw.iter().map(|value| value * value).sum::<f64>().sqrt();
    raw.iter().map(|value| (value / norm) as f32).collect()
}

fn id(index: usize) -> String {
    format!("{index:064x}")
}

/// `rows` and `tombstones` must each be increasing.
fn export(
    fixture: &Fixture,
    rows: impl Iterator<Item = usize>,
    tombstones: impl Iterator<Item = usize>,
    seed: u64,
    checkpoint: i64,
) -> LiveRows {
    LiveRows {
        generation: fixture.generation.clone(),
        kernel_incarnation_id: KERNEL.to_owned(),
        checkpoint: ProjectionCheckpoint {
            snapshot_commit_seq: checkpoint - 1,
            checkpoint_commit_seq: checkpoint,
            hold_id: "hold-7".to_owned(),
        },
        rows: rows
            .map(|index| ExportedRow {
                occurrence_id: id(index),
                vector: unit(seed, index),
            })
            .collect(),
        tombstones: tombstones.map(id).collect(),
    }
}

/// Delta `d` replaces 500 base rows, adds 500 rows past the base, and tombstones 100 further base rows.
fn delta(fixture: &Fixture, d: usize, checkpoint: i64) -> LiveRows {
    export(
        fixture,
        (d * 2000..d * 2000 + 500).chain(BASE + d * 500..BASE + d * 500 + 500),
        d * 2000 + 500..d * 2000 + 600,
        100 + d as u64,
        checkpoint,
    )
}

/// Reads one `/proc/self/status` field in KiB.
fn status_kib(field: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap()
}

/// Resets `VmHWM` to the current resident set and returns that resident set in KiB.
fn reset_peak_rss() -> u64 {
    fs::write("/proc/self/clear_refs", "5").unwrap();
    status_kib("VmRSS:")
}

/// Asks the kernel to drop every clean cached page of the files under `root`.
fn evict_page_cache(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let file = fs::File::open(entry.path()).unwrap();
                file.sync_all().unwrap();
                // SAFETY: `file` owns a valid descriptor for the duration of the call, and the advice changes caching only.
                let advised = unsafe {
                    libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED)
                };
                assert_eq!(advised, 0);
            }
        }
    }
}

struct Phase {
    name: &'static str,
    times: Vec<Duration>,
    /// `VmHWM` growth over the resident set at the phase's start.
    rss_kib: Vec<u64>,
    /// Peak live heap bytes the recording allocator saw on the test thread.
    heap: Vec<u64>,
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn vector_generation_build_stage_verify_time_and_peak_memory() {
    for rows in [10_000usize, 100_000] {
        let template = Fixture::with_dimension(DIMENSION);
        let export = export(&template, 0..rows, 0..0, 1, 10);
        drop(template);
        let row_bytes = rows * DIMENSION as usize * 4;
        let mut phases = ["build", "stage", "verify"].map(|name| Phase {
            name,
            times: Vec::new(),
            rss_kib: Vec::new(),
            heap: Vec::new(),
        });
        let mut store_bytes = 0;
        for _ in 0..runs() {
            let fixture = Fixture::with_dimension(DIMENSION);
            let expected = fixture.expected();
            let work = fixture.work_dir();
            let mut measure = |phase: usize, f: &mut dyn FnMut()| {
                let before = reset_peak_rss();
                let ((), heap) = record_window(|| {
                    let ((), elapsed) = timed(&mut *f);
                    phases[phase].times.push(elapsed);
                });
                phases[phase]
                    .rss_kib
                    .push(status_kib("VmHWM:").saturating_sub(before));
                phases[phase].heap.push(heap.peak_live_bytes as u64);
            };
            let mut built = None;
            measure(0, &mut || {
                built = Some(build(&expected, &export, &work).unwrap())
            });
            let built = built.unwrap();
            let mut digest = String::new();
            measure(1, &mut || {
                digest = stage(&built, &fixture.staging()).unwrap()
            });
            let mut verified = None;
            measure(2, &mut || {
                verified = Some(verify(&fixture.store, &digest, &expected, u64::MAX).unwrap());
            });
            assert_eq!(verified.unwrap().occurrence_ids.len(), rows);
            store_bytes = Ledger::store_bytes(&fixture.store).unwrap();
        }
        for phase in phases {
            println!(
                "generation rows={rows} dim={DIMENSION} phase={} {} peak_rss_growth_kib_median={} heap_peak_bytes_median={}",
                phase.name,
                summary(&phase.times),
                median_u64(phase.rss_kib),
                median_u64(phase.heap),
            );
        }
        println!(
            "generation rows={rows} dim={DIMENSION} f32_row_bytes={row_bytes} store_bytes_after_stage={store_bytes}"
        );
    }
}

fn median_u64(mut samples: Vec<u64>) -> u64 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn composition_publish_recover_reopen_and_cold_acquire_latency() {
    let mut fixture = Fixture::with_dimension(DIMENSION);
    let mut bases = Vec::new();
    let (mut compose_times, mut publish_base) = (Vec::new(), Vec::new());
    let mut digests = Vec::new();
    for sequence in 1..=7u64 {
        let base = fixture.layer_from(&export(
            &fixture,
            0..BASE,
            0..0,
            sequence,
            10 * sequence as i64,
        ));
        let (composition, elapsed) = timed(|| fixture.compose(sequence, &base, &[]).unwrap());
        compose_times.push(elapsed);
        let (published, elapsed) = timed(|| fixture.publish(&composition).unwrap());
        publish_base.push(elapsed);
        digests.push(published);
        bases.push(base);
    }
    let delta = fixture.layer_from(&delta(&fixture, 1, 71));
    let composition = fixture
        .compose(8, bases.last().unwrap(), std::slice::from_ref(&delta))
        .unwrap();
    let (published, publish_delta) = timed(|| fixture.publish(&composition).unwrap());
    digests.push(published);
    println!("composition compose {}", summary(&compose_times));
    println!(
        "composition publish_new_base(rows={BASE}, dim={DIMENSION}) {}",
        summary(&publish_base)
    );
    println!("composition publish_delta(1000 rows + 100 tombstones) n=1 {publish_delta:?}");

    let mut intact = Vec::new();
    for _ in 0..runs() {
        let (recovered, elapsed) = timed(|| fixture.recover().unwrap());
        assert_eq!(recovered, (digests[7].clone(), SelectorState::Current));
        intact.push(elapsed);
    }
    println!(
        "recovery compositions=8 selector=current {}",
        summary(&intact)
    );

    let expected = fixture.expected();
    let mut reopen = Vec::new();
    for _ in 0..runs() {
        let ((), elapsed) = timed(|| {
            let store = GenerationStore::open(Some(fixture.root.path())).unwrap();
            let recovered = recover(
                &store,
                fixture.transaction(),
                &expected,
                bounds().max_deltas,
                u64::MAX,
                bounds().recovery_bound,
            )
            .unwrap();
            assert_eq!(recovered.composition.digest, digests[7]);
        });
        reopen.push(elapsed);
    }
    println!(
        "startup open_store+recover compositions=8 {}",
        summary(&reopen)
    );

    let ledger = Ledger::new(
        std::sync::Arc::clone(&fixture.gate),
        daemon::projection_gates::InvalidationIdentity::from(&fixture.identity),
    );
    fixture.release_transaction();
    let mut cold = Vec::new();
    for _ in 0..runs() {
        evict_page_cache(fixture.root.path());
        let (view, elapsed) = timed(|| {
            acquire(
                Some(fixture.root.path()),
                &fixture.expected(),
                bounds(),
                &ledger,
                &fixture.admission,
                &mut |_| {},
            )
            .unwrap()
        });
        assert_eq!(view.digest(), digests[7]);
        cold.push(elapsed);
    }
    fixture.reacquire_transaction();
    println!(
        "cold acquire page_cache_evicted_before_each first_on_new_ledger={:?} layers=base+1 delta {}",
        cold[0],
        summary(&cold)
    );

    // Corrupting bases 2 through 7 leaves composition 1 as the only one that verifies, so recovery examines all eight.
    for base in &bases[1..] {
        fixture.corrupt(&base.digest, ROWS_FILE);
    }
    let mut worst = Vec::new();
    for _ in 0..runs() {
        let ((digest, selector), elapsed) = timed(|| fixture.recover().unwrap());
        assert_eq!(digest, digests[0]);
        assert_eq!(selector, SelectorState::Stale(digests[7].clone()));
        worst.push(elapsed);
    }
    println!(
        "recovery compositions=8 seven_failing=yes {}",
        summary(&worst)
    );
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn reader_warm_acquire_and_pin_cost() {
    const OPS: usize = 100_000;
    let mut fixture = Fixture::with_dimension(DIMENSION);
    let base = fixture.layer_from(&export(&fixture, 0..BASE, 0..0, 1, 10));
    let deltas: Vec<_> = (1..=4)
        .map(|d| fixture.layer_from(&delta(&fixture, d, 10 + d as i64)))
        .collect();
    let composition = fixture.compose(1, &base, &deltas).unwrap();
    let digest = fixture.publish(&composition).unwrap();
    fixture.release_transaction();
    let take = |fixture: &Fixture| {
        acquire(
            Some(fixture.root.path()),
            &fixture.expected(),
            bounds(),
            &fixture.ledger,
            &fixture.admission,
            &mut |_| {},
        )
        .unwrap()
    };
    drop(take(&fixture));
    let (mut acquired, mut released) = (Vec::new(), Vec::new());
    let mut pinned_bytes = 0;
    for _ in 0..runs() {
        let (view, elapsed) = timed(|| take(&fixture));
        acquired.push(elapsed);
        assert_eq!(view.digest(), digest);
        pinned_bytes = view.pinned_bytes();
        let ((), elapsed) = timed(|| drop(view));
        released.push(elapsed);
    }
    fixture.reacquire_transaction();
    println!(
        "reader warm acquire layers=base({BASE})+4 deltas pinned_bytes={pinned_bytes} {}",
        summary(&acquired)
    );
    println!("reader view release {}", summary(&released));

    let ledger = &fixture.ledger;
    let ((), elapsed) = timed(|| {
        for _ in 0..OPS {
            drop(black_box(ledger.pin(black_box(4096))));
        }
    });
    let ((), heap) = record_window(|| {
        for _ in 0..1000 {
            drop(black_box(ledger.pin(4096)));
        }
    });
    println!(
        "ledger pin+unpin ops={OPS} ns_per_op={} allocations_per_op={:.3}",
        elapsed.as_nanos() / OPS as u128,
        heap.allocation_events as f64 / 1000.0
    );
}

#[test]
#[ignore = "host measurement; run in release with --ignored --nocapture"]
fn ledger_reserve_cost_and_allocations() {
    const OPS: usize = 100_000;
    const DISK_OPS: usize = 10_000;
    let fixture = Fixture::with_dimension(DIMENSION);
    drop(fixture.layer_from(&export(&fixture, 0..BASE, 0..0, 1, 10)));
    let (ledger, grant, store) = (&fixture.ledger, &fixture.admission, &fixture.store);
    let reserve = || {
        drop(black_box(
            ledger
                .reserve(grant, ResourceClass::Scratch, black_box(4096))
                .unwrap(),
        ));
    };
    let reserve_disk = || {
        drop(black_box(
            ledger
                .reserve_disk(grant, ResourceClass::Staging, black_box(4096), store)
                .unwrap(),
        ));
    };
    for (name, ops, op) in [
        ("reserve", OPS, &reserve as &dyn Fn()),
        ("reserve_disk", DISK_OPS, &reserve_disk as &dyn Fn()),
    ] {
        let ((), elapsed) = timed(|| (0..ops).for_each(|_| op()));
        let ((), heap) = record_window(|| (0..1000).for_each(|_| op()));
        assert!(!heap.overflow);
        println!(
            "ledger {name}+release ops={ops} ns_per_op={} allocations_per_op={:.3} requested_bytes_per_op={:.1}",
            elapsed.as_nanos() / ops as u128,
            heap.allocation_events as f64 / 1000.0,
            heap.requested_bytes as f64 / 1000.0
        );
    }
    println!(
        "ledger reserve_disk store_files_bytes={}",
        Ledger::store_bytes(store).unwrap()
    );
}
