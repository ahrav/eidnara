//! Goldens for the analysis contract, authored by hand from the documented rules rather than from analyzer output.

use std::num::NonZeroUsize;

use retrieval::exact::{Intent, SelectorBounds, classify};
use retrieval::lexical::{
    ANALYSIS_CONTRACT_EPOCH, Analysis, AnalysisIdentity, DETAIL, LexicalBounds, LexicalRefusal,
    ORIGINAL_COLUMN, PARTS_COLUMN, Probe, TOKENIZER, analyze, analyze_segments, compile,
};
use rusqlite::ToSql;
use rusqlite::types::{ToSqlOutput, ValueRef};

fn bounds() -> LexicalBounds {
    LexicalBounds {
        max_input_bytes: NonZeroUsize::new(1024).unwrap(),
        max_atoms: NonZeroUsize::new(64).unwrap(),
    }
}

fn analyzed(text: &str) -> Analysis {
    analyze(text, bounds()).unwrap()
}

fn golden(text: &str, atoms: &[&str], parts: &[&str]) {
    let analysis = analyzed(text);
    assert_eq!(
        analysis.atoms().collect::<Vec<_>>(),
        atoms,
        "atoms of {text:?}"
    );
    assert_eq!(
        analysis.parts().collect::<Vec<_>>(),
        parts,
        "parts of {text:?}"
    );
}

/// The text a probe binds, read the way the engine receives it.
fn bound(probe: &Probe) -> String {
    match probe.to_sql().unwrap() {
        ToSqlOutput::Borrowed(ValueRef::Text(text)) => String::from_utf8(text.to_vec()).unwrap(),
        other => panic!("a probe binds text, not {other:?}"),
    }
}

fn probes(text: &str) -> Vec<String> {
    compile(&analyzed(text)).iter().map(bound).collect()
}

#[test]
fn acronym_case_and_digit_boundaries() {
    golden("HTTPServer", &["HTTPServer"], &["HTTP", "Server"]);
    golden(
        "getHTTPResponse2xx",
        &["getHTTPResponse2xx"],
        &["get", "HTTP", "Response", "2", "xx"],
    );
    golden("getHTTPSUrl", &["getHTTPSUrl"], &["get", "HTTPS", "Url"]);
    golden(
        "XMLHttpRequest",
        &["XMLHttpRequest"],
        &["XML", "Http", "Request"],
    );
    golden("iOS", &["iOS"], &["i", "OS"]);
    golden("X11", &["X11"], &["X", "11"]);
    golden("UTF8String", &["UTF8String"], &["UTF", "8", "String"]);
    golden("sha256", &["sha256"], &["sha", "256"]);
    golden("fooBar2Baz", &["fooBar2Baz"], &["foo", "Bar", "2", "Baz"]);
    golden("TLSv1", &["TLSv1"], &["TLSv", "1"]);
    golden("E0308", &["E0308"], &["E", "0308"]);
}

#[test]
fn uppercase_runs_with_a_one_letter_tail_stay_whole() {
    for whole in [
        "IDs", "ABCd", "URLs", "ENOENT", "ABC", "A", "ab", "Ωmega", "Über",
    ] {
        golden(whole, &[whole], &[]);
    }
}

#[test]
fn non_ascii_case_and_uncased_letters_follow_the_same_rules() {
    golden("ÜberServer", &["ÜberServer"], &["Über", "Server"]);
    golden("éA", &["éA"], &["é", "A"]);
    golden("HTTPSérver", &["HTTPSérver"], &["HTTP", "Sérver"]);
    golden("日本語2024", &["日本語2024"], &["日本語", "2024"]);
    golden("\u{E000}1", &["\u{E000}1"], &["\u{E000}", "1"]);
    golden("日本語Server", &["日本語Server"], &[]);
}

#[test]
fn combining_marks_inherit_the_class_of_their_base() {
    golden("e\u{301}Bar", &["e\u{301}Bar"], &["e\u{301}", "Bar"]);
    golden("éBar", &["éBar"], &["é", "Bar"]);
    golden(
        "HTTPSe\u{301}rver",
        &["HTTPSe\u{301}rver"],
        &["HTTP", "Se\u{301}rver"],
    );
    golden("ab\u{301}Cd", &["ab\u{301}Cd"], &["ab\u{301}", "Cd"]);
    golden("1\u{301}a", &["1\u{301}a"], &["1\u{301}", "a"]);
}

#[test]
fn separators_split_atoms_and_underscores_split_parts() {
    golden("snake_case", &["snake_case"], &["snake", "case"]);
    golden("x86_64", &["x86_64"], &["x", "86", "64"]);
    golden(
        "ERR_CONN_REFUSED",
        &["ERR_CONN_REFUSED"],
        &["ERR", "CONN", "REFUSED"],
    );
    golden("_foo_", &["_foo_"], &["foo"]);
    golden("__", &["__"], &[]);
    golden("kebab-case", &["kebab", "case"], &[]);
    golden("a.b.c", &["a", "b", "c"], &[]);
    golden("src/a-b/File.rs", &["src", "a", "b", "File", "rs"], &[]);
    golden("--dry-run", &["dry", "run"], &[]);
    golden("git rebase -i", &["git", "rebase", "i"], &[]);
    golden(
        "server.max_connections",
        &["server", "max_connections"],
        &["max", "connections"],
    );
    golden("v1.2.3", &["v1", "2", "3"], &["v", "1"]);
    golden("2024-01-02", &["2024", "01", "02"], &[]);
    golden("don't 3.14", &["don", "t", "3", "14"], &[]);
}

#[test]
fn unicode_marks_private_use_and_scripts_stay_inside_atoms() {
    golden("Über naïve", &["Über", "naïve"], &[]);
    golden("cre\u{301}me", &["cre\u{301}me"], &[]);
    golden("foo\u{E000}bar", &["foo\u{E000}bar"], &[]);
    golden("日本語テキスト", &["日本語テキスト"], &[]);
    golden("\u{301}x", &["x"], &[]);
}

#[test]
fn short_terms_are_complete_atoms_inside_broader_requests() {
    golden("a b12 x", &["a", "b12", "x"], &["b", "12"]);
    golden("a", &["a"], &[]);
    golden("ab", &["ab"], &[]);
}

#[test]
fn syntax_shaped_input_is_ordinary_atoms() {
    golden("a OR b", &["a", "OR", "b"], &[]);
    golden("a NOT b", &["a", "NOT", "b"], &[]);
    golden("NEAR(a b)", &["NEAR", "a", "b"], &[]);
    golden("{original}: \"x\" ^y z*", &["original", "x", "y", "z"], &[]);
    golden("!!! ... \"\"", &[], &[]);
    golden("", &[], &[]);
}

#[test]
fn duplicates_are_retained_in_both_columns() {
    golden("a a", &["a", "a"], &[]);
    golden(
        "snake_case snake_case",
        &["snake_case", "snake_case"],
        &["snake", "case", "snake", "case"],
    );
}

#[test]
fn column_texts_join_with_single_spaces() {
    let analysis = analyzed("HTTPServer x86_64");
    assert_eq!(analysis.original_text(), "HTTPServer x86_64");
    assert_eq!(analysis.parts_text(), "HTTP Server x 86 64");
    assert_eq!(analyzed("").original_text(), "");
}

#[test]
fn parts_never_exceed_the_scalar_values_of_their_atom() {
    let alternating: String = (0..512)
        .map(|i| if i % 2 == 0 { 'a' } else { 'B' })
        .collect();
    let analysis = analyze(
        &alternating,
        LexicalBounds {
            max_input_bytes: NonZeroUsize::new(512).unwrap(),
            max_atoms: NonZeroUsize::new(1).unwrap(),
        },
    )
    .unwrap();
    assert_eq!(analysis.atoms().collect::<Vec<_>>(), [alternating.as_str()]);
    assert_eq!(
        analysis.parts().len(),
        257,
        "`a`, 255 `Ba`, and a final `B`"
    );
    assert!(analysis.parts().len() <= alternating.chars().count());
}

#[test]
fn compilation_quotes_every_atom_and_keeps_order_and_duplicates() {
    assert_eq!(probes("a OR b a"), ["\"a\"", "\"OR\"", "\"b\"", "\"a\""]);
}

#[test]
fn compilation_probes_atoms_not_parts() {
    assert_eq!(probes("HTTPServer"), ["\"HTTPServer\""]);
}

#[test]
fn zero_atoms_compile_to_no_probe() {
    assert!(compile(&analyzed("!!! ...")).is_empty());
    assert!(compile(&analyze_segments(&[], bounds()).unwrap()).is_empty());
}

#[test]
fn atoms_never_contain_quotes() {
    let analysis = analyzed("say \"OR\" \"\"a\"\"");
    assert_eq!(analysis.atoms().collect::<Vec<_>>(), ["say", "OR", "a"]);
    assert!(analysis.atoms().all(|atom| !atom.contains('"')));
    assert_eq!(probes("say \"OR\""), ["\"say\"", "\"OR\""]);
}

#[test]
fn byte_bound_is_checked_before_analysis_at_exact_and_over_bound_sizes() {
    let exact = LexicalBounds {
        max_input_bytes: NonZeroUsize::new(5).unwrap(),
        max_atoms: NonZeroUsize::new(64).unwrap(),
    };
    assert_eq!(
        analyze("a b c", exact).unwrap().atoms().collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(
        analyze("a b cd", exact),
        Err(LexicalRefusal::InputTooLong { bytes: 6, bound: 5 })
    );
    assert_eq!(
        analyze_segments(&["abc", "def"], exact),
        Err(LexicalRefusal::InputTooLong { bytes: 6, bound: 5 }),
        "the bound sums every segment"
    );
    let one = LexicalBounds {
        max_input_bytes: NonZeroUsize::new(1).unwrap(),
        max_atoms: NonZeroUsize::new(64).unwrap(),
    };
    assert_eq!(
        analyze("é", one),
        Err(LexicalRefusal::InputTooLong { bytes: 2, bound: 1 }),
        "bytes, not scalar values"
    );
    assert_eq!(
        analyze("a\0bcde", exact),
        Err(LexicalRefusal::InputTooLong { bytes: 6, bound: 5 }),
        "the byte bound is checked before the NUL scan"
    );
}

#[test]
fn atom_bound_refuses_the_next_atom_at_exact_and_over_bound_counts() {
    let exact = LexicalBounds {
        max_input_bytes: NonZeroUsize::new(1024).unwrap(),
        max_atoms: NonZeroUsize::new(3).unwrap(),
    };
    assert_eq!(
        analyze("a b c", exact).unwrap().atoms().collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(
        analyze("a b c d", exact),
        Err(LexicalRefusal::TooManyAtoms { bound: 3 })
    );
    assert_eq!(
        analyze_segments(&["a b", "c d"], exact),
        Err(LexicalRefusal::TooManyAtoms { bound: 3 })
    );
}

#[test]
fn nul_is_refused_before_analysis() {
    assert_eq!(analyze("ab\0cd", bounds()), Err(LexicalRefusal::Nul));
    assert_eq!(
        analyze_segments(&["abc", "d\0"], bounds()),
        Err(LexicalRefusal::Nul)
    );
}

fn selector_bounds() -> SelectorBounds {
    SelectorBounds {
        max_input_bytes: NonZeroUsize::new(256).unwrap(),
        max_value_bytes: NonZeroUsize::new(128).unwrap(),
    }
}

fn segments(request: &str) -> Vec<&str> {
    let intent = classify(request, selector_bounds()).unwrap();
    intent.lexical_segments(request)
}

#[test]
fn lexical_segments_exclude_selector_mentions_wherever_they_sit() {
    assert_eq!(segments("explain symbol:foo now"), ["explain ", " now"]);
    assert_eq!(segments("id:x foo"), [" foo"]);
    assert_eq!(segments("foo id:x"), ["foo "]);
    assert_eq!(segments("id:x id:y"), [" "]);
    assert_eq!(segments("plain prose"), ["plain prose"]);
}

#[test]
fn removing_a_mention_never_glues_its_neighbours() {
    let request = "foo.id:\"x\"bar";
    let intent = classify(request, selector_bounds()).unwrap();
    assert!(matches!(&intent, Intent::Hybrid(mentions) if mentions.len() == 1));
    let segments = intent.lexical_segments(request);
    assert_eq!(segments, ["foo.", "bar"]);
    assert_eq!(
        analyze_segments(&segments, bounds())
            .unwrap()
            .atoms()
            .collect::<Vec<_>>(),
        ["foo", "bar"]
    );
    assert_eq!(
        analyze_segments(&["ab", "cd"], bounds())
            .unwrap()
            .atoms()
            .collect::<Vec<_>>(),
        ["ab", "cd"],
        "a segment boundary is an atom boundary even without a separator character"
    );
    assert_eq!(
        analyze_segments(&["", "a", ""], bounds())
            .unwrap()
            .atoms()
            .collect::<Vec<_>>(),
        ["a"]
    );
}

#[test]
fn direct_requests_have_no_lexical_text_and_length_never_routes() {
    let request = "id:ab12";
    let intent = classify(request, selector_bounds()).unwrap();
    assert!(matches!(intent, Intent::Direct(_)));
    assert!(intent.lexical_segments(request).is_empty());

    let request = "ab12";
    let intent = classify(request, selector_bounds()).unwrap();
    assert_eq!(intent, Intent::Hybrid(vec![]));
    assert_eq!(intent.lexical_segments(request), [request]);
    assert_eq!(probes(request), ["\"ab12\""]);
}

#[test]
fn identity_pins_the_manifest() {
    let preimage = AnalysisIdentity::preimage();
    let (major, minor, update) = char::UNICODE_VERSION;
    for field in [
        ANALYSIS_CONTRACT_EPOCH,
        TOKENIZER,
        DETAIL,
        ORIGINAL_COLUMN,
        PARTS_COLUMN,
        &format!("{major}.{minor}.{update}"),
        rusqlite::version(),
        rusqlite::ffi::SQLITE_SOURCE_ID.to_str().unwrap(),
    ] {
        assert!(
            preimage.contains(&format!("{}:{field}\n", field.len())),
            "{field:?} missing from preimage"
        );
    }
    assert_eq!(ANALYSIS_CONTRACT_EPOCH, "identifier-analysis.v1");
    assert_eq!(
        AnalysisIdentity::current().as_str(),
        "b624de2ef523ed1b8a78f6a30fd80cf425f7563b94201887814886cd9b62ba83",
        "a changed manifest field changes the identity; update the contract and digest together"
    );
}
