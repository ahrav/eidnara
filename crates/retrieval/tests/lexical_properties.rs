//! Property tests cover input-wide laws; `lexical_analysis.rs` goldens cover exact examples.
//! The seed is fixed so a failing case reproduces.

use std::num::NonZeroUsize;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use retrieval::lexical::{LexicalBounds, LexicalRefusal, analyze, analyze_segments, compile};
use rusqlite::ToSql;
use rusqlite::types::{ToSqlOutput, ValueRef};

const SEED: [u8; 32] = *b"lexical-analysis-laws-seed-0001!";

fn runner() -> TestRunner {
    TestRunner::new_with_rng(
        Config {
            cases: 256,
            rng_algorithm: RngAlgorithm::ChaCha,
            ..Config::default()
        },
        TestRng::from_seed(RngAlgorithm::ChaCha, &SEED),
    )
}

fn unbounded() -> LexicalBounds {
    LexicalBounds {
        max_input_bytes: NonZeroUsize::new(usize::MAX).unwrap(),
        max_atoms: NonZeroUsize::new(usize::MAX).unwrap(),
    }
}

fn atom_char() -> impl Strategy<Value = char> {
    prop::sample::select(vec![
        'a', 'b', 'z', 'A', 'B', 'Z', '0', '1', '9', '_', 'é', 'Ü', 'Ω', '日', '\u{301}',
        '\u{E000}',
    ])
}

fn atom() -> impl Strategy<Value = String> {
    prop::collection::vec(atom_char(), 1..12).prop_map(|chars| chars.into_iter().collect())
}

fn text() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            atom_char(),
            prop::sample::select(vec![' ', '.', '/', '-', '"', '\'', ':', '{', '*', '!']),
        ],
        0..40,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

fn bound_text(probe: &impl ToSql) -> String {
    match probe.to_sql().unwrap() {
        ToSqlOutput::Borrowed(ValueRef::Text(text)) => String::from_utf8(text.to_vec()).unwrap(),
        other => panic!("{other:?}"),
    }
}

/// A mark takes the class of the character before it, so a mark right after `_` is dropped with it.
fn without_separators(atom: &str) -> String {
    let mut kept = String::new();
    let mut after_separator = false;
    for c in atom.chars() {
        if c == '_' || (after_separator && c == '\u{301}') {
            after_separator = true;
        } else {
            after_separator = false;
            kept.push(c);
        }
    }
    kept
}

#[test]
fn parts_conserve_their_atom_and_are_additive() {
    runner()
        .run(&atom(), |atom| {
            let atom = atom.trim_start_matches('\u{301}').to_string();
            prop_assume!(!atom.is_empty());
            let analysis = analyze(&atom, unbounded()).unwrap();
            prop_assert_eq!(analysis.atoms().collect::<Vec<_>>(), vec![atom.as_str()]);
            let parts: Vec<&str> = analysis.parts().collect();
            prop_assert!(
                parts
                    .iter()
                    .all(|part| !part.is_empty() && !part.contains('_'))
            );
            prop_assert!(parts.len() <= atom.chars().count());
            if !parts.is_empty() {
                prop_assert_eq!(parts.concat(), without_separators(&atom));
                prop_assert_ne!(parts.as_slice(), [atom.as_str()]);
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn combining_marks_are_transparent_to_part_boundaries() {
    runner()
        .run(&atom(), |atom| {
            let atom = atom.trim_start_matches('\u{301}').to_string();
            prop_assume!(!atom.is_empty());
            let stripped = atom.replace('\u{301}', "");
            prop_assume!(!stripped.is_empty());
            let marked = analyze(&atom, unbounded()).unwrap();
            let plain = analyze(&stripped, unbounded()).unwrap();
            prop_assert!(
                marked.parts().all(|part| !part.starts_with('\u{301}')),
                "a part starts with a mark: {:?}",
                marked.parts().collect::<Vec<_>>()
            );
            let marked_parts: Vec<String> = marked
                .parts()
                .map(|part| part.replace('\u{301}', ""))
                .collect();
            prop_assert_eq!(marked_parts, plain.parts().collect::<Vec<_>>());
            Ok(())
        })
        .unwrap();
}

#[test]
fn atoms_are_in_order_substrings_and_reanalyze_to_themselves() {
    runner()
        .run(&text(), |text| {
            let analysis = analyze(&text, unbounded()).unwrap();
            let mut cursor = 0;
            for atom in analysis.atoms() {
                prop_assert!(!atom.is_empty());
                prop_assert!(!atom.chars().any(|c| c.is_whitespace() || c == '"'));
                let at = text[cursor..].find(atom).expect("atoms appear in order");
                cursor += at + atom.len();
            }
            let again = analyze(&analysis.original_text(), unbounded()).unwrap();
            prop_assert_eq!(&again, &analysis);
            let probes = compile(&analysis);
            prop_assert_eq!(probes.len(), analysis.atoms().len());
            for (probe, atom) in probes.iter().zip(analysis.atoms()) {
                prop_assert_eq!(bound_text(probe), format!("\"{atom}\""));
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn segments_analyze_independently() {
    runner()
        .run(&(text(), text()), |(left, right)| {
            let joined = analyze_segments(&[&left, &right], unbounded()).unwrap();
            let separate = [
                analyze(&left, unbounded()).unwrap(),
                analyze(&right, unbounded()).unwrap(),
            ];
            let atoms: Vec<&str> = separate.iter().flat_map(|a| a.atoms()).collect();
            let parts: Vec<&str> = separate.iter().flat_map(|a| a.parts()).collect();
            prop_assert_eq!(joined.atoms().collect::<Vec<_>>(), atoms);
            prop_assert_eq!(joined.parts().collect::<Vec<_>>(), parts);
            Ok(())
        })
        .unwrap();
}

#[test]
fn bounds_refuse_in_declared_order_and_nul_only_separates() {
    runner()
        .run(
            &(text(), 1usize..=8, 1usize..=32, any::<bool>()),
            |(text, max_atoms, max_bytes, nul)| {
                let text = if nul { format!("{text}\0") } else { text };
                let bounds = LexicalBounds {
                    max_input_bytes: NonZeroUsize::new(max_bytes).unwrap(),
                    max_atoms: NonZeroUsize::new(max_atoms).unwrap(),
                };
                let clean = text.replace('\0', " ");
                let unbounded_atoms = analyze(&clean, unbounded()).unwrap().atoms().len();
                let outcome = analyze(&text, bounds);
                if text.len() > max_bytes {
                    let too_long = matches!(outcome, Err(LexicalRefusal::InputTooLong { .. }));
                    prop_assert!(too_long, "{:?}", outcome);
                } else if unbounded_atoms > max_atoms {
                    prop_assert_eq!(
                        outcome,
                        Err(LexicalRefusal::TooManyAtoms { bound: max_atoms })
                    );
                } else {
                    let analysis = outcome.unwrap();
                    prop_assert_eq!(analysis.atoms().len(), unbounded_atoms);
                    prop_assert!(analysis.atoms().all(|atom| !atom.contains('\0')));
                    prop_assert_eq!(&analysis, &analyze(&clean, bounds).unwrap());
                }
                Ok(())
            },
        )
        .unwrap();
}
