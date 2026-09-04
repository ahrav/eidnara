//! Fixed-workload timing harness; see `DESIGN.md` for the estimand. Prints one
//! `arm\tmedian_ns_per_byte\tbytes` line per arm. `report.sh` runs this binary many times and
//! does the statistics; nothing here compares anything.
//!
//! Modes:
//!   arms                 all warm arms, `ITERS` timed calls each (env `ARMS_ITERS`, default 25)
//!   arms --quick         one warmup, one timed call per arm (cachegrind / perf stat driver)
//!   arms --cold          fresh-process cold start: ns to first `estimate_tokens("hello")`
//!   arms --arm <name>    only that arm (any number of `--arm` flags)
#![allow(missing_docs)]

use std::hint::black_box;
use std::time::Instant;

use tokenizer::{encode_ordinary, estimate_tokens};

const CORPUS: &[(&str, &str)] = &[
    ("ascii_prose", include_str!("corpus/ascii_prose.txt")),
    ("code", include_str!("corpus/code.txt")),
    ("cjk", include_str!("corpus/cjk.txt")),
    ("mixed_unicode", include_str!("corpus/mixed_unicode.txt")),
    (
        "whitespace_heavy",
        include_str!("corpus/whitespace_heavy.txt"),
    ),
    ("numeric", include_str!("corpus/numeric.txt")),
    (
        "adversarial_long_piece",
        include_str!("corpus/adversarial_long_piece.txt"),
    ),
];
const SHORT_STRINGS: &str = include_str!("corpus/short_strings.json");

/// Minimal parser for the `["...", ...]` fixture: no serde in the bench so the dependency
/// graph of the timed binary is exactly the crate's own.
fn parse_short_strings() -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = SHORT_STRINGS.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        loop {
            match chars.next().expect("unterminated string") {
                '"' => break,
                '\\' => match chars.next().expect("dangling escape") {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    'b' => s.push('\u{8}'),
                    'f' => s.push('\u{c}'),
                    '/' => s.push('/'),
                    '\\' => s.push('\\'),
                    '"' => s.push('"'),
                    'u' => {
                        let hex = |chars: &mut std::iter::Peekable<std::str::Chars>| {
                            let h: String = (0..4).map(|_| chars.next().unwrap()).collect();
                            u32::from_str_radix(&h, 16).unwrap()
                        };
                        let mut cp = hex(&mut chars);
                        if (0xD800..0xDC00).contains(&cp) {
                            assert_eq!(chars.next(), Some('\\'));
                            assert_eq!(chars.next(), Some('u'));
                            let lo = hex(&mut chars);
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        }
                        s.push(char::from_u32(cp).expect("bad code point"));
                    }
                    other => panic!("unknown escape {other}"),
                },
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Times `f` `iters` times after `warmup` untimed calls; returns median ns per call.
fn time_ns(warmup: usize, iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..warmup {
        f();
    }
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_nanos() as f64);
    }
    median(&mut samples)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--cold") {
        let t = Instant::now();
        let n = estimate_tokens("hello");
        let ns = t.elapsed().as_nanos();
        black_box(n);
        println!("cold_start\t{ns}\t1");
        return;
    }
    let quick = args.iter().any(|a| a == "--quick");
    let only: Vec<&str> = args
        .windows(2)
        .filter(|w| w[0] == "--arm")
        .map(|w| w[1].as_str())
        .collect();
    let iters = if quick {
        1
    } else {
        std::env::var("ARMS_ITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(25)
    };
    let warmup = if quick { 1 } else { 3 };
    let want = |name: &str| only.is_empty() || only.contains(&name);

    // Force vocabulary construction outside every warm arm's timing boundary.
    black_box(estimate_tokens("hello"));

    let report = |name: &str, bytes: usize, ns: f64| {
        println!("{name}\t{:.4}\t{bytes}", ns / bytes as f64);
    };

    for (name, text) in CORPUS {
        if want(name) {
            let ns = time_ns(warmup, iters, || {
                black_box(encode_ordinary(black_box(text)));
            });
            report(name, text.len(), ns);
        }
        let count_name = format!("{name}_count");
        if (*name == "ascii_prose" || *name == "code") && want(&count_name) {
            let ns = time_ns(warmup, iters, || {
                black_box(estimate_tokens(black_box(text)));
            });
            report(&count_name, text.len(), ns);
        }
    }

    if want("short_strings") {
        let strings = parse_short_strings();
        assert_eq!(strings.len(), 10_000, "short_strings fixture size");
        let bytes: usize = strings.iter().map(String::len).sum();
        let ns = time_ns(warmup, iters, || {
            for s in &strings {
                black_box(encode_ordinary(black_box(s)));
            }
        });
        report("short_strings", bytes, ns);
    }
}
