//! Selector grammar for exact lookup and lexical query compilation.
//! Byte equality is the contract.

use std::num::NonZeroUsize;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    Id,
    Sha,
    Path,
    Symbol,
    Command,
    Config,
    Error,
}

impl Family {
    pub const ALL: [Family; 7] = [
        Self::Id,
        Self::Sha,
        Self::Path,
        Self::Symbol,
        Self::Command,
        Self::Config,
        Self::Error,
    ];

    pub fn keyword(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Sha => "sha",
            Self::Path => "path",
            Self::Symbol => "symbol",
            Self::Command => "command",
            Self::Config => "config",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectorBounds {
    pub max_input_bytes: NonZeroUsize,
    /// The limit applies to the encoded token; JSON and percent decoding never
    /// lengthen a value, so the decoded bytes stay within it.
    pub max_value_bytes: NonZeroUsize,
}

pub const MAX_SHA_HEX: usize = 64;

/// Whether a prefix is a full object id depends on the repository's object
/// format, which the parser does not know; see [`crate::exact::ShaPrefixQuery`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HexPrefix(String);

impl HexPrefix {
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty()
            || text.len() > MAX_SHA_HEX
            || !text.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self(text.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn digits(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorValue {
    Text(String),
    Path(Vec<u8>),
    Sha(HexPrefix),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub family: Family,
    pub value: SelectorValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub selector: Selector,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Exactly one selector occupies the whole trimmed request.
    Direct(Selector),
    /// Mentions are evidence for hybrid retrieval, never routing.
    Hybrid(Vec<Mention>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRefusal {
    Absolute,
    EmptyComponent,
    DotComponent,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectorRefusal {
    #[error("the request carries {bytes} bytes, over the {bound} byte bound")]
    InputTooLong { bytes: usize, bound: usize },
    #[error("{} selector at {offset} has an empty value", family.keyword())]
    EmptyValue { family: Family, offset: usize },
    #[error(
        "{} selector at {offset} carries {bytes} bytes, over the {bound} byte bound",
        family.keyword()
    )]
    ValueTooLong {
        family: Family,
        offset: usize,
        bytes: usize,
        bound: usize,
    },
    #[error("{} selector at {offset} is not one complete JSON string", family.keyword())]
    MalformedQuote { family: Family, offset: usize },
    #[error("{} selector at {offset} contains NUL", family.keyword())]
    Nul { family: Family, offset: usize },
    #[error("sha selector at {offset} is not one to {MAX_SHA_HEX} hex digits")]
    NotHex { offset: usize },
    #[error("path selector at {offset} has a `%` not followed by two hex digits")]
    MalformedPercentEscape { offset: usize },
    #[error("path selector at {offset} is not repository-relative: {reason:?}")]
    InvalidPath { offset: usize, reason: PathRefusal },
}

/// Trimming removes only ASCII space, tab, CR, and LF at both ends.
///
/// # Errors
///
/// The classifier refuses requests over `bounds.max_input_bytes`.
/// A malformed whole-request selector is refused, not treated as prose.
pub fn classify(request: &str, bounds: SelectorBounds) -> Result<Intent, SelectorRefusal> {
    if request.len() > bounds.max_input_bytes.get() {
        return Err(SelectorRefusal::InputTooLong {
            bytes: request.len(),
            bound: bounds.max_input_bytes.get(),
        });
    }
    let start = request.len() - request.trim_start_matches(ASCII_TRIM).len();
    let trimmed = request.trim_matches(ASCII_TRIM);
    if let Some((family, value_at)) = selector_head(trimmed, 0) {
        let token = token_extent(trimmed, value_at, &mut BareRun::default()).ok_or(
            SelectorRefusal::MalformedQuote {
                family,
                offset: start + value_at,
            },
        )?;
        if token.end == trimmed.len() {
            let selector = decode_value(
                family,
                &trimmed[value_at..token.end],
                token.quoted,
                start + value_at,
                bounds,
            )?;
            return Ok(Intent::Direct(selector));
        }
    }
    Ok(Intent::Hybrid(mentions(trimmed, start, bounds)))
}

const ASCII_TRIM: [char; 4] = [' ', '\t', '\r', '\n'];

fn mentions(trimmed: &str, start: usize, bounds: SelectorBounds) -> Vec<Mention> {
    let mut found = Vec::new();
    let mut at = 0;
    let mut run = BareRun::default();
    while at < trimmed.len() {
        if !trimmed.is_char_boundary(at) {
            at += 1;
            continue;
        }
        let Some((family, value_at)) = selector_head(trimmed, at) else {
            at += 1;
            continue;
        };
        let Some(token) = token_extent(trimmed, value_at, &mut run) else {
            at += 1;
            continue;
        };
        match decode_value(
            family,
            &trimmed[value_at..token.end],
            token.quoted,
            start + value_at,
            bounds,
        ) {
            Ok(selector) => {
                found.push(Mention {
                    selector,
                    span: start + at..start + token.end,
                });
                at = token.end;
            }
            Err(_) => at += 1,
        }
    }
    found
}

/// A selector head starts the text or follows whitespace or ASCII punctuation
/// other than `_`; any other character continues an identifier.
fn selector_head(text: &str, at: usize) -> Option<(Family, usize)> {
    if text[..at].chars().next_back().is_some_and(|before| {
        !(before.is_whitespace() || (before.is_ascii_punctuation() && before != '_'))
    }) {
        return None;
    }
    let rest = &text.as_bytes()[at..];
    Family::ALL.into_iter().find_map(|family| {
        let keyword = family.keyword().as_bytes();
        (rest.starts_with(keyword) && rest.get(keyword.len()) == Some(&b':'))
            .then(|| (family, at + keyword.len() + 1))
    })
}

struct Token {
    end: usize,
    quoted: bool,
}

/// End of the most recent bare run scanned. Heads in the same bare run reuse
/// its scanned end: heads are visited in increasing order, and a start at or
/// before `end` sees only bare characters up to `end`.
#[derive(Default)]
struct BareRun {
    end: usize,
}

fn token_extent(text: &str, value_at: usize, run: &mut BareRun) -> Option<Token> {
    let rest = &text[value_at..];
    if rest.starts_with('"') {
        let end = json_string_end(rest)?;
        return Some(Token {
            end: value_at + end,
            quoted: true,
        });
    }
    if run.end < value_at {
        run.end = value_at
            + rest
                .char_indices()
                .find(|(_, c)| !bare_char(*c))
                .map_or(rest.len(), |(index, _)| index);
    }
    Some(Token {
        end: run.end,
        quoted: false,
    })
}

#[cfg(test)]
thread_local! {
    /// Characters examined by bare-token scans on this thread; tests bound it
    /// to prove each request byte is scanned a constant number of times.
    static BARE_SCAN_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn bare_char(c: char) -> bool {
    #[cfg(test)]
    BARE_SCAN_STEPS.with(|steps| steps.set(steps.get() + 1));
    !(c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '\\' | ',' | ';'))
}

fn json_string_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some(index + 1),
            b'\\' => index += 2,
            _ => index += 1,
        }
    }
    None
}

fn decode_json_string(text: &str) -> Option<String> {
    serde_json::from_str(text).ok()
}

fn decode_value(
    family: Family,
    token: &str,
    quoted: bool,
    offset: usize,
    bounds: SelectorBounds,
) -> Result<Selector, SelectorRefusal> {
    let bound = bounds.max_value_bytes.get();
    if token.len() > bound {
        return Err(SelectorRefusal::ValueTooLong {
            family,
            offset,
            bytes: token.len(),
            bound,
        });
    }
    let text = if quoted {
        decode_json_string(token).ok_or(SelectorRefusal::MalformedQuote { family, offset })?
    } else {
        token.to_string()
    };
    if text.is_empty() {
        return Err(SelectorRefusal::EmptyValue { family, offset });
    }
    let value = match family {
        Family::Sha => {
            SelectorValue::Sha(HexPrefix::parse(&text).ok_or(SelectorRefusal::NotHex { offset })?)
        }
        Family::Path => {
            let bytes = if quoted {
                text.into_bytes()
            } else {
                percent_decode(text.as_bytes())
                    .ok_or(SelectorRefusal::MalformedPercentEscape { offset })?
            };
            if bytes.contains(&0) {
                return Err(SelectorRefusal::Nul { family, offset });
            }
            validate_path(&bytes)
                .map_err(|reason| SelectorRefusal::InvalidPath { offset, reason })?;
            SelectorValue::Path(bytes)
        }
        Family::Id | Family::Symbol | Family::Command | Family::Config | Family::Error => {
            if text.contains('\0') {
                return Err(SelectorRefusal::Nul { family, offset });
            }
            SelectorValue::Text(text)
        }
    };
    Ok(Selector { family, value })
}

fn percent_decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
            out.push((digit(high)? << 4) | digit(low)?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(out)
}

/// Backslash is filename data; only `/` separates components.
fn validate_path(bytes: &[u8]) -> Result<(), PathRefusal> {
    if bytes.first() == Some(&b'/') {
        return Err(PathRefusal::Absolute);
    }
    for component in bytes.split(|b| *b == b'/') {
        match component {
            [] => return Err(PathRefusal::EmptyComponent),
            b"." | b".." => return Err(PathRefusal::DotComponent),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> SelectorBounds {
        SelectorBounds {
            max_input_bytes: NonZeroUsize::new(256).unwrap(),
            max_value_bytes: NonZeroUsize::new(128).unwrap(),
        }
    }

    fn direct(request: &str) -> Selector {
        match classify(request, bounds()).unwrap() {
            Intent::Direct(selector) => selector,
            other => panic!("{request:?}: {other:?}"),
        }
    }

    fn hybrid(request: &str) -> Vec<Mention> {
        match classify(request, bounds()).unwrap() {
            Intent::Hybrid(mentions) => mentions,
            other => panic!("{request:?}: {other:?}"),
        }
    }

    fn refused(request: &str) -> SelectorRefusal {
        classify(request, bounds()).unwrap_err()
    }

    #[test]
    fn every_family_parses_bare_and_quoted_values_losslessly() {
        for family in Family::ALL {
            let value = if family == Family::Sha {
                "AbC1"
            } else {
                "Foo_Bar.9"
            };
            let bare = direct(&format!("{}:{value}", family.keyword()));
            let quoted = direct(&format!("{}:\"{value}\"", family.keyword()));
            assert_eq!(bare.family, family);
            assert_eq!(quoted.family, family);
            assert_eq!(bare.value, quoted.value);
            match bare.value {
                SelectorValue::Sha(hex) => assert_eq!(hex.as_str(), "abc1"),
                SelectorValue::Path(bytes) => assert_eq!(bytes, b"Foo_Bar.9"),
                SelectorValue::Text(text) => assert_eq!(text, "Foo_Bar.9"),
            }
        }
    }

    #[test]
    fn trimming_removes_only_ascii_space_tab_cr_lf() {
        assert_eq!(
            direct(" \t\r\nid:x\n").value,
            SelectorValue::Text("x".into())
        );
        let mentions = hybrid("\u{a0}id:x");
        assert_eq!(mentions.len(), 1, "a no-break space is content");
        assert_eq!(mentions[0].span, 2..6);
    }

    #[test]
    fn whitespace_around_the_colon_or_an_empty_value_is_not_a_selector() {
        assert_eq!(hybrid("id :x"), vec![]);
        assert_eq!(hybrid("id: x"), vec![]);
        assert_eq!(hybrid("error: the build failed"), vec![]);
        assert_eq!(hybrid("see id: x"), vec![]);
        assert!(matches!(refused("id:"), SelectorRefusal::EmptyValue { .. }));
        assert!(matches!(
            refused("id:\"\""),
            SelectorRefusal::EmptyValue { .. }
        ));
        assert_eq!(hybrid("ID:x"), vec![], "families are lowercase only");
        assert_eq!(hybrid("identifier:x"), vec![], "no aliases or prefixes");
    }

    #[test]
    fn prose_and_multiple_selectors_stay_hybrid_with_spans() {
        let mentions = hybrid("explain symbol:foo");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].span, 8..18);
        assert_eq!(
            mentions[0].selector.value,
            SelectorValue::Text("foo".into())
        );

        let mentions = hybrid("id:a,sha:b");
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[0].span, 0..4);
        assert_eq!(mentions[1].span, 5..10);
        assert_eq!(
            direct("symbol:\"a,sha:b\"").value,
            SelectorValue::Text("a,sha:b".into()),
            "quoted contents are data"
        );

        let mentions = hybrid("command:git status");
        assert_eq!(mentions.len(), 1);
        assert_eq!(
            mentions[0].selector.value,
            SelectorValue::Text("git".into())
        );
        assert_eq!(
            direct("command:\"git status\"").value,
            SelectorValue::Text("git status".into())
        );
        assert_eq!(hybrid(""), vec![]);
        assert_eq!(hybrid("   "), vec![]);
        assert_eq!(
            hybrid("oid:abc"),
            vec![],
            "a keyword inside an identifier is not a head"
        );
        for glued in [
            "caf\u{e9}id:abc",
            "cafe\u{301}id:abc",
            "\u{2192}id:abc",
            "_id:abc",
        ] {
            assert_eq!(
                hybrid(glued),
                vec![],
                "{glued:?}: only whitespace or ASCII punctuation may precede a head"
            );
        }
        for opened in ["(id:abc", "\u{2003}id:abc", "x;id:abc"] {
            assert_eq!(hybrid(opened).len(), 1, "{opened:?}");
        }
    }

    #[test]
    fn bare_tokens_stop_at_unicode_whitespace_controls_quotes_and_separators() {
        for stop in ["\u{2003}", "\u{1}", "'", "\"", "\\", ",", ";"] {
            let request = format!("id:a{stop}b");
            let mentions = hybrid(&request);
            assert_eq!(mentions.len(), 1, "{request:?}");
            assert_eq!(
                mentions[0].selector.value,
                SelectorValue::Text("a".into()),
                "{request:?}"
            );
        }
        assert_eq!(hybrid("id:'x'"), vec![], "a quote stops the bare token");
    }

    #[test]
    fn json_strings_decode_once_and_malformed_ones_are_refused() {
        assert_eq!(
            direct(r#"id:"a\"b\\c\/d\u00e9\ud83d\ude00""#).value,
            SelectorValue::Text("a\"b\\c/d\u{e9}\u{1f600}".into())
        );
        for malformed in [
            r#"id:"abc"#,
            r#"id:"a\x""#,
            r#"id:"a\ud83d""#,
            r#"id:"a\ude00""#,
            r#"id:"a\u12""#,
            "id:\"a\tb\"",
        ] {
            assert!(
                matches!(refused(malformed), SelectorRefusal::MalformedQuote { .. }),
                "{malformed:?}"
            );
        }
        assert!(matches!(
            refused(r#"id:"a\u0000b""#),
            SelectorRefusal::Nul { .. }
        ));
        let mentions = hybrid("id:\"ab\"c");
        assert_eq!(
            mentions.len(),
            1,
            "a closed string followed by prose is a mention"
        );
        assert_eq!(mentions[0].span, 0..7);
    }

    #[test]
    fn text_values_keep_bytes_without_normalization() {
        assert_eq!(
            direct("symbol:Foo").value,
            SelectorValue::Text("Foo".into())
        );
        assert_ne!(direct("symbol:Foo").value, direct("symbol:foo").value);
        assert_eq!(
            direct("error:E0308.").value,
            SelectorValue::Text("E0308.".into())
        );
        assert_eq!(
            direct("config:ﬁle").value,
            SelectorValue::Text("ﬁle".into())
        );
        assert_ne!(direct("config:ﬁle").value, direct("config:file").value);
    }

    #[test]
    fn bare_paths_percent_decode_once_and_quoted_paths_do_not() {
        assert_eq!(
            direct("path:src/%FF.rs").value,
            SelectorValue::Path(b"src/\xff.rs".to_vec())
        );
        assert_eq!(
            direct("path:\"src/%FF.rs\"").value,
            SelectorValue::Path(b"src/%FF.rs".to_vec())
        );
        assert_ne!(
            direct("path:src/%FF.rs").value,
            direct("path:\"src/%FF.rs\"").value
        );
        assert_eq!(
            direct("path:a%25b").value,
            SelectorValue::Path(b"a%b".to_vec())
        );
        assert_eq!(
            direct("path:a%2525b").value,
            SelectorValue::Path(b"a%25b".to_vec())
        );
        assert_ne!(
            direct("path:%FE").value,
            direct("path:%FF").value,
            "distinct non-UTF-8 paths stay distinct"
        );
        assert_eq!(
            direct("path:a%5Cb").value,
            SelectorValue::Path(b"a\\b".to_vec())
        );
        assert_eq!(
            direct("path:\"a\\\\b\"").value,
            SelectorValue::Path(b"a\\b".to_vec())
        );
        for malformed in ["path:a%", "path:a%2", "path:a%zz"] {
            assert!(
                matches!(
                    refused(malformed),
                    SelectorRefusal::MalformedPercentEscape { .. }
                ),
                "{malformed:?}"
            );
        }
    }

    #[test]
    fn paths_reject_absolute_empty_and_dot_components_after_decoding() {
        let invalid = [
            ("path:/etc/passwd", PathRefusal::Absolute),
            ("path:%2Fetc", PathRefusal::Absolute),
            ("path:a//b", PathRefusal::EmptyComponent),
            ("path:a/", PathRefusal::EmptyComponent),
            ("path:a/./b", PathRefusal::DotComponent),
            ("path:a/../b", PathRefusal::DotComponent),
            ("path:%2E%2E/b", PathRefusal::DotComponent),
            ("path:..", PathRefusal::DotComponent),
            ("path:\"../b\"", PathRefusal::DotComponent),
        ];
        for (request, expected) in invalid {
            match refused(request) {
                SelectorRefusal::InvalidPath { reason, .. } => {
                    assert_eq!(reason, expected, "{request:?}")
                }
                other => panic!("{request:?}: {other:?}"),
            }
        }
        assert_eq!(
            direct("path:a/.hidden/b..c").value,
            SelectorValue::Path(b"a/.hidden/b..c".to_vec())
        );
        assert!(matches!(
            refused("path:a%00b"),
            SelectorRefusal::Nul {
                family: Family::Path,
                ..
            }
        ));
        assert!(matches!(
            refused("path:\"a\\u0000b\""),
            SelectorRefusal::Nul {
                family: Family::Path,
                ..
            }
        ));
    }

    #[test]
    fn refusal_offsets_point_at_the_value_in_the_untrimmed_request() {
        assert_eq!(
            refused(" \n path:a%"),
            SelectorRefusal::MalformedPercentEscape { offset: 8 }
        );
        assert_eq!(refused("sha:\"zz\""), SelectorRefusal::NotHex { offset: 4 });
    }

    #[test]
    fn bare_punctuation_is_data_except_the_declared_stops() {
        let cases: [(&str, SelectorValue); 6] = [
            (
                "symbol:Foo::bar<T>",
                SelectorValue::Text("Foo::bar<T>".into()),
            ),
            ("id:a/b@c#d+e", SelectorValue::Text("a/b@c#d+e".into())),
            ("path:a:b", SelectorValue::Path(b"a:b".to_vec())),
            ("config:[a].b=c", SelectorValue::Text("[a].b=c".into())),
            (
                "command:git-log|less",
                SelectorValue::Text("git-log|less".into()),
            ),
            ("error:E0308(x)", SelectorValue::Text("E0308(x)".into())),
        ];
        for (request, expected) in cases {
            assert_eq!(direct(request).value, expected, "{request:?}");
        }
        let mentions = hybrid("error:E0308, then more");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].span, 0..11);
        assert_eq!(
            mentions[0].selector.value,
            SelectorValue::Text("E0308".into())
        );
    }

    #[test]
    fn unicode_whitespace_is_data_inside_quotes_and_content_outside_them() {
        assert_eq!(
            direct("id:\"a\u{2003}b\"").value,
            SelectorValue::Text("a\u{2003}b".into())
        );
        let mentions = hybrid("id:x\u{a0}");
        assert_eq!(mentions.len(), 1, "a trailing no-break space is prose");
        assert_eq!(mentions[0].span, 0..4);
    }

    #[test]
    fn json_decoding_agrees_with_serde_json_on_generated_strings() {
        for text in [
            "plain",
            "quote\"inside",
            "back\\slash",
            "tab\tnew\nline",
            "\u{e9}\u{1f600}\u{2003}",
            "control\u{1}",
        ] {
            let encoded = serde_json::to_string(text).unwrap();
            assert_eq!(decode_json_string(&encoded).as_deref(), Some(text));
            assert_eq!(json_string_end(&encoded), Some(encoded.len()));
        }
        for malformed in [r#""\ud83d""#, r#""\ude00""#, "\"a\tb\"", r#""\x""#, r#""a"#] {
            assert_eq!(decode_json_string(malformed), None, "{malformed:?}");
        }
    }

    #[test]
    fn sha_values_normalize_case_only_and_refuse_nonhex_or_overlong_input() {
        assert_eq!(
            direct("sha:ABCDEF").value,
            SelectorValue::Sha(HexPrefix::parse("abcdef").unwrap())
        );
        assert_eq!(direct("sha:\"ABCDEF\"").value, direct("sha:abcdef").value);
        assert_eq!(
            direct("sha:a").value,
            SelectorValue::Sha(HexPrefix::parse("a").unwrap())
        );
        let full = "f".repeat(64);
        assert_eq!(
            direct(&format!("sha:{full}")).value,
            SelectorValue::Sha(HexPrefix::parse(&full).unwrap())
        );
        for malformed in ["sha:\"zz\"", "sha:xyz", "sha:abc-def", "sha:\"abc def\""] {
            assert!(
                matches!(refused(malformed), SelectorRefusal::NotHex { .. }),
                "{malformed:?}"
            );
        }
        assert!(matches!(
            refused(&format!("sha:{}", "a".repeat(65))),
            SelectorRefusal::NotHex { .. }
        ));
    }

    #[test]
    fn bounds_apply_to_input_encoded_and_decoded_bytes() {
        let tight = SelectorBounds {
            max_input_bytes: NonZeroUsize::new(10).unwrap(),
            max_value_bytes: NonZeroUsize::new(4).unwrap(),
        };
        assert!(matches!(
            classify("id:abcdefghij", tight),
            Err(SelectorRefusal::InputTooLong {
                bytes: 13,
                bound: 10
            })
        ));
        assert!(matches!(
            classify("id:abcde", tight),
            Err(SelectorRefusal::ValueTooLong {
                bytes: 5,
                bound: 4,
                ..
            })
        ));
        let loose = SelectorBounds {
            max_input_bytes: NonZeroUsize::new(64).unwrap(),
            max_value_bytes: NonZeroUsize::new(8).unwrap(),
        };
        assert!(
            matches!(
                classify("path:%41%41%41%41%41%41%41%41%41", loose),
                Err(SelectorRefusal::ValueTooLong {
                    bytes: 27,
                    bound: 8,
                    ..
                })
            ),
            "the encoded token is refused before decoding"
        );
        assert!(matches!(
            classify("path:\"abcdefghi\"", loose),
            Err(SelectorRefusal::ValueTooLong {
                bytes: 11,
                bound: 8,
                ..
            })
        ));
        assert_eq!(
            classify("see id:abcdefghijk now", loose).unwrap(),
            Intent::Hybrid(vec![]),
            "an over-bound token inside prose is prose"
        );
    }

    #[test]
    fn hybrid_scanning_is_linear_in_the_request_however_many_heads_are_refused() {
        // Every `sha:` head owns a bare token that runs to the end of the
        // request, so each one is refused and the next head starts four bytes
        // later; the scan must not restart from every refused head.
        let bounds = SelectorBounds {
            max_input_bytes: NonZeroUsize::new(1 << 20).unwrap(),
            max_value_bytes: NonZeroUsize::new(64).unwrap(),
        };
        let mut steps = Vec::new();
        for heads in [1_024usize, 4_096] {
            let request = format!("x {}", "sha:".repeat(heads));
            BARE_SCAN_STEPS.with(|s| s.set(0));
            assert_eq!(classify(&request, bounds).unwrap(), Intent::Hybrid(vec![]));
            steps.push(BARE_SCAN_STEPS.with(std::cell::Cell::get));
        }
        assert!(
            steps[1] <= steps[0] * 8,
            "a 4x longer request must not cost more than ~4x the scan: {steps:?}"
        );
        // A refused head does not hide a later selector inside its token.
        let mentions = hybrid("see sha:id:abc");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].span, 8..14);
        assert_eq!(
            mentions[0].selector.value,
            SelectorValue::Text("abc".into())
        );
    }
}
