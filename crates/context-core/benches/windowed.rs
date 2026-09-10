//! Windowed redaction throughput over fixed, seeded corpora.
//!
//! Cell names are `windowed/<entry>/<corpus>/<MiB>`.
//!
//! `redact` passes `usize::MAX` as the detection cap, so it renders every finding in the corpus.
//!
//! `detect_bytes` returns at the first finding, so it is measured only over
//! clean corpora; on a secret-bearing corpus it would scan one window and be
//! credited the whole input's bytes. Setup asserts each corpus is what its
//! `clean` flag claims.
//!
//! `redact/keyed` uses one window of back-to-back keyed assignments, one finding every 20 bytes.
//!
//! The Criterion filter argument selects cells by name.

use std::hint::black_box;
use std::time::Duration;

use context_core::redaction::{MAX_REDACTABLE_BYTES, Redactor};
use criterion::{BenchmarkId, Criterion, SamplingMode, Throughput, criterion_group};

#[path = "support/windowed_corpus.rs"]
mod corpus;

use corpus::{MIB, TEXT_CORPORA, invalid_utf8_bytes, seed_for};

const SIZES: [usize; 2] = [MIB, 8 * MIB];

fn keyed(bytes: usize) -> String {
    let mut keyed = "password=hunter-two ".repeat(bytes / 20 + 1);
    keyed.truncate(bytes);
    keyed
}

fn windowed(c: &mut Criterion) {
    let redactor = Redactor::new().unwrap();
    let mut group = c.benchmark_group("windowed");
    group.sampling_mode(SamplingMode::Flat);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));
    group.sample_size(10);

    for corpus in TEXT_CORPORA {
        let name = corpus.name;
        for size in SIZES {
            let input = (corpus.generate)(size, seed_for(name, size));
            let mib = size / MIB;
            let found = redactor.detect_windowed(&input).unwrap();
            assert_eq!(
                found, !corpus.clean,
                "{name}/{mib}: corpus classification does not match the scanner"
            );
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("redact/{name}"), mib),
                &input,
                |b, input| {
                    b.iter(|| {
                        redactor
                            .redact_windowed(black_box(input), usize::MAX)
                            .unwrap()
                    })
                },
            );
            if corpus.clean {
                group.bench_with_input(
                    BenchmarkId::new(format!("detect_bytes/{name}"), mib),
                    &input,
                    |b, input| {
                        b.iter(|| {
                            redactor
                                .detect_windowed_bytes(black_box(input.as_bytes()))
                                .unwrap()
                        })
                    },
                );
            }
        }
    }

    for size in SIZES {
        let input = invalid_utf8_bytes(size, seed_for("invalid_utf8_bytes", size));
        let mib = size / MIB;
        assert!(
            !redactor.detect_windowed_bytes(&input).unwrap(),
            "invalid_utf8_bytes/{mib}: corpus is not clean"
        );
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("detect_bytes/invalid_utf8_bytes", mib),
            &input,
            |b, input| {
                b.iter(|| {
                    redactor
                        .detect_windowed_bytes(black_box(input.as_slice()))
                        .unwrap()
                })
            },
        );
    }

    let input = keyed(MAX_REDACTABLE_BYTES);
    assert!(
        redactor.detect_windowed(&input).unwrap(),
        "keyed: corpus carries no finding"
    );
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("redact/keyed", MAX_REDACTABLE_BYTES),
        &input,
        |b, input| {
            b.iter(|| {
                redactor
                    .redact_windowed(black_box(input), usize::MAX)
                    .unwrap()
            })
        },
    );
    group.finish();
}

criterion_group!(benches, windowed);

fn main() {
    // `cargo test --all-targets` omits `--bench`; without this guard Criterion's test mode walks every cell over 64 MiB corpora in a debug build, which took seven minutes per toolchain in CI.
    // `cargo bench -- --test` keeps `--bench`, so Criterion still runs every cell once.
    if !std::env::args().any(|arg| arg == "--bench") {
        eprintln!("windowed: corpus setup runs only under `cargo bench`");
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
