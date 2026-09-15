//! An atom is a maximal run of token characters, where a token character is anything Rust classifies as alphanumeric, `_`, a private-use code point, or a combining diacritical mark (`U+0300..=U+036F`) that continues a run.
//! The engine tokenizes every atom itself, so its output is the effective term; the analyzer only fixes which characters stay adjacent, and the same analyzer runs on both the indexed text and the probe.
//!
//! Parts split one atom at `_`, at a lower-to-upper case change, at a letter-to-digit or digit-to-letter change, and before the last uppercase letter of an uppercase run that two or more lowercase letters follow (`HTTPServer` yields `HTTP` and `Server`; `IDs` stays whole).
//! A combining mark takes the class of the character before it, so `e\u{301}Bar` and `éBar` split identically.
//! Parts keep their original bytes; the engine folds case and diacritics.
//! Parts are additive: an atom whose parts are exactly itself contributes nothing to the parts column, so a term is never counted twice for one atom.

use std::num::NonZeroUsize;
use std::ops::RangeInclusive;

/// Bounds checked before any atom is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexicalBounds {
    /// The byte bound applies across all segments, not per segment.
    pub max_input_bytes: NonZeroUsize,
    /// The atom after the last permitted one is refused before it is retained.
    pub max_atoms: NonZeroUsize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LexicalRefusal {
    #[error("the input carries {bytes} bytes, over the {bound} byte bound")]
    InputTooLong { bytes: usize, bound: usize },
    /// The engine reads bound MATCH text as a C string, so a NUL would silently truncate the probe.
    #[error("the input contains NUL")]
    Nul,
    #[error("the input analyzes to more than {bound} atoms")]
    TooManyAtoms { bound: usize },
}

/// Atoms and parts in input order, duplicates retained.
/// Parts are a flat sequence with no per-atom alignment: an atom contributes zero, one, or several parts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    atoms: Vec<String>,
    parts: Vec<String>,
}

impl Analysis {
    pub fn atoms(&self) -> impl ExactSizeIterator<Item = &str> {
        self.atoms.iter().map(String::as_str)
    }

    pub fn parts(&self) -> impl ExactSizeIterator<Item = &str> {
        self.parts.iter().map(String::as_str)
    }

    /// A successful empty analysis compiles to no probe; the caller issues no MATCH for it.
    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// Atoms never contain whitespace, so a single space separates them without changing what the engine tokenizes.
    pub fn original_text(&self) -> String {
        self.atoms.join(" ")
    }

    /// Parts never contain whitespace, so a single space separates them without changing what the engine tokenizes.
    pub fn parts_text(&self) -> String {
        self.parts.join(" ")
    }
}

/// The index-side entry point; a query goes through `Intent::lexical_segments` first so selector mentions are not analyzed as prose.
///
/// # Errors
///
/// See [`analyze_segments`].
pub fn analyze(text: &str, bounds: LexicalBounds) -> Result<Analysis, LexicalRefusal> {
    analyze_segments(&[text], bounds)
}

/// A segment boundary always ends an atom, so removing a selector mention never glues its neighbours together.
///
/// # Errors
///
/// The total byte length is checked against `bounds.max_input_bytes` and every segment is checked for NUL before any atom is scanned.
/// The atom after `bounds.max_atoms` is refused before it is retained.
pub fn analyze_segments(
    segments: &[&str],
    bounds: LexicalBounds,
) -> Result<Analysis, LexicalRefusal> {
    let bytes: usize = segments.iter().map(|segment| segment.len()).sum();
    if bytes > bounds.max_input_bytes.get() {
        return Err(LexicalRefusal::InputTooLong {
            bytes,
            bound: bounds.max_input_bytes.get(),
        });
    }
    if segments.iter().any(|segment| segment.contains('\0')) {
        return Err(LexicalRefusal::Nul);
    }
    let mut analysis = Analysis::default();
    for segment in segments {
        for atom in atoms(segment) {
            if analysis.atoms.len() == bounds.max_atoms.get() {
                return Err(LexicalRefusal::TooManyAtoms {
                    bound: bounds.max_atoms.get(),
                });
            }
            let parts = parts(atom);
            if parts.len() != 1 || parts[0] != atom {
                analysis.parts.extend(parts);
            }
            analysis.atoms.push(atom.to_string());
        }
    }
    Ok(analysis)
}

const COMBINING_MARKS: RangeInclusive<char> = '\u{0300}'..='\u{036F}';

fn is_private_use(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}')
}

fn starts_atom(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || is_private_use(c)
}

fn continues_atom(c: char) -> bool {
    starts_atom(c) || COMBINING_MARKS.contains(&c)
}

/// A leading mark cannot start an atom, so it is trimmed off the run it would otherwise begin.
fn atoms(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !continues_atom(c))
        .map(|run| run.trim_start_matches(|c| COMBINING_MARKS.contains(&c)))
        .filter(|run| !run.is_empty())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Separator,
    Upper,
    Lower,
    Digit,
    Uncased,
}

fn class(c: char) -> Class {
    if c == '_' {
        Class::Separator
    } else if c.is_numeric() {
        Class::Digit
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_lowercase() {
        Class::Lower
    } else {
        Class::Uncased
    }
}

fn parts(atom: &str) -> Vec<String> {
    let mut classes: Vec<Class> = Vec::with_capacity(atom.len());
    for c in atom.chars() {
        let inherited = classes
            .last()
            .copied()
            .filter(|_| COMBINING_MARKS.contains(&c));
        classes.push(inherited.unwrap_or_else(|| class(c)));
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut previous = None;
    for (index, (c, class)) in atom.chars().zip(&classes).enumerate() {
        if *class == Class::Separator {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            previous = None;
            continue;
        }
        if let Some(previous) = previous
            && splits(previous, *class, &classes[index + 1..])
        {
            parts.push(std::mem::take(&mut current));
        }
        current.push(c);
        previous = Some(*class);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn splits(previous: Class, current: Class, rest: &[Class]) -> bool {
    let letter = |class: Class| matches!(class, Class::Upper | Class::Lower | Class::Uncased);
    match (previous, current) {
        (Class::Digit, class) if letter(class) => true,
        (class, Class::Digit) if letter(class) => true,
        (Class::Lower, Class::Upper) => true,
        (Class::Upper, Class::Upper) => {
            rest.first() == Some(&Class::Lower) && rest.get(1) == Some(&Class::Lower)
        }
        _ => false,
    }
}
