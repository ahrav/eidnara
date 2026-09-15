use std::hint::black_box;
use std::num::NonZeroUsize;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group};
use retrieval::lexical::{LexicalBounds, analyze, compile};

const CORPUS: [&str; 12] = [
    "HTTPServer",
    "snake_case_identifier",
    "src/retrieval/lexical/analysis.rs",
    "ERR_CONN_REFUSED E0308 ENOENT",
    "server.max_connections=1024",
    "git rebase -i HEAD~3",
    "sha256:9f8d958f0a1b2c3d",
    "getHTTPResponse2xx XMLHttpRequest fooBar2Baz",
    "crème brûlée naïve Über 日本語テキスト",
    "why does the a b c test fail on x86_64 with v1.2.3",
    "a OR b NEAR(c d) {original}: \"e\" ^f g*",
    "!!! ... \"\"",
];

fn bounds() -> LexicalBounds {
    LexicalBounds {
        max_input_bytes: NonZeroUsize::new(4096).unwrap(),
        max_atoms: NonZeroUsize::new(256).unwrap(),
    }
}

fn analysis_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("lexical");
    for (index, request) in CORPUS.iter().enumerate() {
        group.bench_with_input(
            BenchmarkId::new("analyze_and_compile", index),
            request,
            |b, request| {
                b.iter(|| {
                    let analysis = analyze(black_box(request), bounds()).unwrap();
                    black_box(compile(&analysis))
                })
            },
        );
    }
    let long: String = (0..256).map(|i| format!("atom{i} ")).collect();
    group.bench_function("analyze_256_atoms", |b| {
        b.iter(|| black_box(analyze(black_box(&long), bounds()).unwrap()))
    });
    group.finish();
}

fn configure() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
}

criterion_group! {
    name = benches;
    config = configure();
    targets = analysis_benches
}

fn main() {
    if !std::env::args().any(|arg| arg == "--bench") {
        eprintln!("lexical_analysis: measurements run only under `cargo bench`");
        return;
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
