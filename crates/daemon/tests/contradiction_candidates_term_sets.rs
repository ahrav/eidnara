use std::collections::BTreeSet;

use daemon::contradiction_candidates::scorer::{
    DESCRIPTOR_BOUNDS, MIN_TERM_SCALARS, STOPWORDS, SUMMARY_BOUNDS, term_set,
};
use retrieval::lexical::{LexicalBounds, LexicalRefusal, analyze_segments};

fn analyzer_terms(text: &str, bounds: LexicalBounds) -> Result<BTreeSet<String>, LexicalRefusal> {
    let analysis = analyze_segments(&[text], bounds)?;
    Ok(analysis
        .atoms()
        .chain(analysis.parts())
        .map(str::to_lowercase)
        .filter(|term| {
            term.chars().count() >= MIN_TERM_SCALARS
                && term.chars().any(char::is_alphanumeric)
                && !term.chars().all(char::is_numeric)
                && !STOPWORDS.contains(&term.as_str())
        })
        .collect())
}

struct Xorshift(u64);

impl Xorshift {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound as u64) as usize
    }
}

const PIECES: [&str; 48] = [
    "a",
    "b",
    "z",
    "A",
    "B",
    "Z",
    "0",
    "7",
    "_",
    " ",
    " ",
    "-",
    ".",
    ",",
    "\n",
    "\t",
    "é",
    "É",
    "\u{301}",
    "İ",
    "ß",
    "日本",
    "٣",
    "\u{E000}",
    "🙂",
    "—",
    "\u{a0}",
    "Σ",
    "ǅ",
    "Ⅻ",
    "ﬁ",
    "Ａ",
    "HTTP",
    "Server",
    "IDs",
    "utf8",
    "Decoder",
    "config_loader",
    "THE",
    "and",
    "with",
    "cache",
    "kernel",
    "writer",
    "2024",
    "v2",
    "abcDEF",
    "ABCdef",
];

#[test]
fn term_sets_equal_the_analyzer_rules_on_generated_text() {
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    for case in 0..20_000 {
        let pieces = 1 + rng.below(if case % 100 == 0 { 3_000 } else { 40 });
        let text: String = (0..pieces)
            .map(|_| PIECES[rng.below(PIECES.len())])
            .collect();
        for bounds in [SUMMARY_BOUNDS, DESCRIPTOR_BOUNDS] {
            assert_eq!(
                term_set(&text, bounds),
                analyzer_terms(&text, bounds),
                "case {case}: {text:?}"
            );
        }
    }
}

#[test]
fn term_sets_equal_the_analyzer_rules_at_the_byte_and_atom_bounds() {
    let tight = LexicalBounds {
        max_input_bytes: std::num::NonZeroUsize::new(64).unwrap(),
        max_atoms: std::num::NonZeroUsize::new(3).unwrap(),
    };
    for text in [
        "alpha beta gamma",
        "alpha beta gamma delta",
        "alpha é beta",
        "alpha é beta gamma",
        "x".repeat(64).as_str(),
        "x".repeat(65).as_str(),
    ] {
        assert_eq!(
            term_set(text, tight),
            analyzer_terms(text, tight),
            "{text:?}"
        );
    }
    for bounds in [SUMMARY_BOUNDS, DESCRIPTOR_BOUNDS] {
        let max = bounds.max_input_bytes.get();
        for len in [max - 1, max, max + 1] {
            let text = "ab é".repeat(len / 5 + 1);
            let mut cut = len.min(text.len());
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            let text = format!("{}{}", &text[..cut], "x".repeat(len - cut));
            assert_eq!(text.len(), len);
            assert_eq!(term_set(&text, bounds), analyzer_terms(&text, bounds));
        }
    }
}
