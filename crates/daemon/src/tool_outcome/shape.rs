//! The shape of a tool result's output, an instrument for corpus measurement. Tests and the
//! source-corpus report example are its only readers; no digest, summary, or selection rule
//! reads it.

use super::OutputText;

/// One of the six output shapes [`classify`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputShape {
    /// No nonblank line.
    Empty,
    /// A content list holding at least one block that is not text.
    NonTextBlocks,
    /// The wire's JSON output variant, whatever its value.
    StructuredValue,
    /// Exactly one nonblank line.
    SingleLine,
    /// A line starting `diff --git `, or a line starting `--- ` directly followed by one starting `+++ `.
    UnifiedDiff,
    /// Any other text.
    MultiLine,
}

impl OutputShape {
    pub const ALL: [Self; 6] = [
        Self::Empty,
        Self::NonTextBlocks,
        Self::StructuredValue,
        Self::SingleLine,
        Self::UnifiedDiff,
        Self::MultiLine,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NonTextBlocks => "non_text_blocks",
            Self::StructuredValue => "structured_value",
            Self::SingleLine => "single_line",
            Self::UnifiedDiff => "unified_diff",
            Self::MultiLine => "multi_line",
        }
    }
}

/// Classifies `text` from its bytes alone, deciding in a fixed order so overlapping inputs have one
/// answer: a non-text block, then the JSON variant (never parsed text), then no nonblank line, then
/// a diff marker, then one nonblank line. The lines are those of the joined view, so a content list
/// of text blocks and its joined text classify alike.
pub fn classify(text: &OutputText<'_>) -> OutputShape {
    match text {
        OutputText::Blocks(blocks)
            if blocks
                .iter()
                .any(|block| !matches!(block.kind, memory_store::ResultBlockKind::Text { .. })) =>
        {
            return OutputShape::NonTextBlocks;
        }
        OutputText::Json(_) => return OutputShape::StructuredValue,
        OutputText::Text(_) | OutputText::Blocks(_) => {}
    }
    let mut nonblank = 0_usize;
    let mut diff = false;
    let mut after_minus = false;
    for line in text.pieces().flat_map(|piece| piece.split('\n')) {
        diff |= line.starts_with("diff --git ") || (after_minus && line.starts_with("+++ "));
        after_minus = line.starts_with("--- ");
        nonblank += usize::from(!line.trim().is_empty());
    }
    match (nonblank, diff) {
        (0, _) => OutputShape::Empty,
        (_, true) => OutputShape::UnifiedDiff,
        (1, false) => OutputShape::SingleLine,
        _ => OutputShape::MultiLine,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn text_block(text: &str) -> memory_store::ResultBlock {
        memory_store::ResultBlock {
            kind: memory_store::ResultBlockKind::Text {
                text: text.to_owned(),
            },
            provider_extras: memory_store::ProviderExtras::new(),
        }
    }

    fn media_block() -> memory_store::ResultBlock {
        memory_store::ResultBlock {
            kind: memory_store::ResultBlockKind::Media {
                media: memory_store::MediaBlock {
                    kind: memory_store::MediaKind::Image,
                    media_type: "image/png".to_owned(),
                    filename: None,
                    source: serde_json::json!({}),
                },
            },
            provider_extras: memory_store::ProviderExtras::new(),
        }
    }

    /// The six classes, each from its example, and the overlaps the fixed order decides.
    #[test]
    fn each_shape_has_its_example_and_overlaps_have_one_answer() {
        let media = [text_block("ok"), media_block()];
        for (text, shape) in [
            (
                OutputText::Json("{\"a\":1}".to_owned()),
                OutputShape::StructuredValue,
            ),
            (
                OutputText::Text("diff --git a/x b/x\nindex 1..2\n"),
                OutputShape::UnifiedDiff,
            ),
            (OutputText::Text("\n \n"), OutputShape::Empty),
            (OutputText::Blocks(&media), OutputShape::NonTextBlocks),
            (OutputText::Text("ok"), OutputShape::SingleLine),
            (
                OutputText::Text("first line\nsecond line"),
                OutputShape::MultiLine,
            ),
            (
                OutputText::Json("{}".to_owned()),
                OutputShape::StructuredValue,
            ),
            (OutputText::Text("--- a\n+++ b\n"), OutputShape::UnifiedDiff),
            (OutputText::Text("{\"a\":1}"), OutputShape::SingleLine),
            (
                OutputText::Text("--- a\nmiddle\n+++ b"),
                OutputShape::MultiLine,
            ),
            (OutputText::Blocks(&[]), OutputShape::Empty),
            (
                OutputText::Blocks(&[media_block()]),
                OutputShape::NonTextBlocks,
            ),
            (OutputText::Text(""), OutputShape::Empty),
        ] {
            assert_eq!(
                classify(&text),
                shape,
                "{:?}",
                text.pieces().collect::<Vec<_>>()
            );
        }
        let long = "x".repeat(1 << 20);
        assert_eq!(classify(&OutputText::Text(&long)), OutputShape::SingleLine);
        assert_eq!(
            OutputShape::ALL.map(OutputShape::name),
            [
                "empty",
                "non_text_blocks",
                "structured_value",
                "single_line",
                "unified_diff",
                "multi_line"
            ]
        );
    }

    /// A diff split across two text blocks, two empty blocks, and one block classify as their joined text.
    #[test]
    fn text_blocks_classify_as_their_joined_text() {
        for (pieces, shape) in [
            (vec!["--- a", "+++ b"], OutputShape::UnifiedDiff),
            (vec!["", ""], OutputShape::Empty),
            (vec!["ok"], OutputShape::SingleLine),
        ] {
            let blocks: Vec<memory_store::ResultBlock> =
                pieces.iter().map(|piece| text_block(piece)).collect();
            let joined = pieces.join("\n");
            assert_eq!(classify(&OutputText::Blocks(&blocks)), shape, "{pieces:?}");
            assert_eq!(classify(&OutputText::Text(&joined)), shape, "{pieces:?}");
        }
    }

    /// Whether `source` names the classifier or its shape through any path, alias, or import form.
    /// Grouped imports can reach `shape` through an aliased parent: `use outcome::{shape as classifier};`.
    fn names_the_classifier(source: &str) -> bool {
        fn is_ident(found: char) -> bool {
            found.is_alphanumeric() || found == '_'
        }
        fn starts_use_item(line: &str) -> bool {
            let mut rest = line.trim_start();
            if let Some(after) = rest.strip_prefix("pub") {
                rest = after.trim_start();
                if let Some(after) = rest.strip_prefix('(') {
                    rest = after.split_once(')').map_or("", |(_, after)| after);
                }
            }
            rest.trim_start()
                .strip_prefix("use")
                .is_some_and(|after| after.starts_with(char::is_whitespace))
        }
        let mut offset = 0;
        let mut use_items = source.split_inclusive('\n').filter_map(|line| {
            let start = offset;
            offset += line.len();
            starts_use_item(line).then(|| source[start..].split(';').next().unwrap_or_default())
        });
        source.contains("OutputShape")
            || source.match_indices("shape").any(|(at, found)| {
                let (before, after) = (&source[..at], &source[at + found.len()..]);
                !before.chars().next_back().is_some_and(is_ident)
                    && !after.chars().next().is_some_and(is_ident)
                    && (before.ends_with("::") || after.starts_with("::"))
            })
            || use_items.any(|item| {
                item.split(|found: char| !is_ident(found))
                    .any(|word| word == "shape")
            })
    }

    /// The scan flags every path and import form that reaches the classifier, and passes the
    /// module declaration and imports of the module's other items.
    #[test]
    fn the_production_scan_flags_each_way_to_reach_the_classifier() {
        for reaching in [
            "use crate::tool_outcome::shape::classify;",
            "use crate::tool_outcome::{ToolBlock, shape};\nlet _ = shape::classify(&text);",
            "use crate::tool_outcome::{\n    ToolBlock,\n    shape::{OutputShape, classify},\n};",
            "let shape = crate::tool_outcome::shape::classify(&text);",
            "use self::shape::*;",
            "use super::shape as s;",
            "pub use self::shape as output_shape;",
            "use crate::tool_outcome::{a::{b}, shape};",
            "if matches!(found, OutputShape::UnifiedDiff) {}",
            "use crate::tool_outcome as outcome;\nuse outcome::{shape as classifier};\nclassifier::classify(&text);",
            "use crate::tool_outcome::*;\nuse shape as classifier;",
            "pub(crate) use crate::tool_outcome::{\n    shape,\n};",
        ] {
            assert!(names_the_classifier(reaching), "{reaching}");
        }
        for clear in [
            "pub mod shape;",
            "use crate::tool_outcome::{ToolBlock, ToolPairs};",
            "let shape = classify(&node);",
            "use crate::reshape::classify;",
            "/// The shape the summarizer reads.",
        ] {
            assert!(!names_the_classifier(clear), "{clear}");
        }
    }

    /// Every daemon source file outside this module, tests included, is scanned whole, so no
    /// production path, the digest, the formatter, and selection among them, reaches the classifier.
    #[test]
    fn no_production_path_reads_the_output_shape() {
        fn visit(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, files);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    files.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let here = root.join("tool_outcome").join("shape.rs");
        let mut files = Vec::new();
        visit(&root, &mut files);
        assert!(files.contains(&here) && files.contains(&root.join("lib.rs")));
        assert!(files.len() > 50);
        for file in files.into_iter().filter(|file| *file != here) {
            let source = std::fs::read_to_string(&file).unwrap();
            assert!(
                !names_the_classifier(&source),
                "{} names the classifier",
                file.display()
            );
        }
    }

    /// The positive control of the question set: the digest of compression-fidelity case C5.V1's
    /// wave-1 `bash` result answers "did it fail" and "which line names the cause". The answers were
    /// authored from the full output: its second line reports the failure and names the cause.
    #[test]
    fn the_positive_control_answers_its_first_two_questions_from_its_digest() {
        let cause = "billing-worker: error: connection refused (10.4.2.17:5433)";
        let fidelity: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("testdata/compression-fidelity.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let output = fidelity["cases"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|case| case["sources"].as_array().into_iter().flatten())
            .filter(|source| source["id"] == "C5.V1")
            .flat_map(|source| source["messages"].as_array().into_iter().flatten())
            .flat_map(|message| message["parts"].as_array().into_iter().flatten())
            .find(|part| part["callID"] == "call_cf_504")
            .and_then(|part| part["state"]["output"].as_str())
            .unwrap();
        assert_eq!(output.lines().nth(1), Some(cause));
        let output = memory_store::ToolOutput::bare(memory_store::OutputKind::Text {
            text: output.to_owned(),
        });
        let digest = super::super::output_outcome(&output);
        assert!(digest.contains(&format!("\"{cause}\"")), "{digest}");
        assert_eq!(
            classify(&super::super::from_output(&output).1),
            OutputShape::MultiLine
        );
    }

    fn output_text() -> impl Strategy<Value = (Vec<String>, bool)> {
        (
            proptest::collection::vec("[a-z+\\- \\n]{0,12}", 0..6),
            any::<bool>(),
        )
    }

    proptest! {
        #[test]
        fn classify_is_total_deterministic_and_agrees_on_joined_text(
            (pieces, with_media) in output_text(),
            text in any::<String>(),
            media_at in any::<prop::sample::Index>(),
        ) {
            for value in [OutputText::Text(&text), OutputText::Json(text.clone())] {
                prop_assert_eq!(classify(&value), classify(&value));
            }
            prop_assert_eq!(classify(&OutputText::Json(text.clone())), OutputShape::StructuredValue);
            let mut blocks: Vec<memory_store::ResultBlock> =
                pieces.iter().map(|piece| text_block(piece)).collect();
            let joined = pieces.join("\n");
            prop_assert_eq!(
                classify(&OutputText::Blocks(&blocks)),
                classify(&OutputText::Text(&joined))
            );
            if with_media {
                blocks.insert(media_at.index(blocks.len() + 1), media_block());
                prop_assert_eq!(classify(&OutputText::Blocks(&blocks)), OutputShape::NonTextBlocks);
            }
        }
    }
}
