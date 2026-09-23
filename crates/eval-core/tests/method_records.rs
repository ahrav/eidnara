//! The evaluator property catalog is read at runtime and held to the METHOD
//! contract: field order, check semantics, cited tests that exist, evidence
//! files that exist, and an index that equals the record set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const FIELDS: [&str; 12] = [
    "Type:",
    "Reachability:",
    "Status:",
    "Exercised:",
    "Guarantee:",
    "Check:",
    "Fault/timing angle:",
    "Required faults and enabling state:",
    "Confidence:",
    "Existing check:",
    "Impact:",
    "Open questions:",
];
const SEMANTICS: [&str; 5] = [
    "always",
    "always-or-unreached",
    "sometimes",
    "reachable",
    "unreachable",
];
const EVIDENCE_HEADINGS: [&str; 6] = [
    "## Discovery trigger",
    "## Evidence trail",
    "## Failure scenario",
    "## Timing windows and dependencies",
    "## What a test must construct",
    "## Investigation log",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/eval-core sits two levels below the workspace root")
        .to_path_buf()
}

struct Record {
    slug: String,
    lines: Vec<String>,
}

fn records(catalog: &str) -> Vec<Record> {
    let body = &catalog[catalog.find("## Records").expect("a records section")..];
    let body = &body[..body
        .find("## Relationship map")
        .expect("a relationship map")];
    body.split("\n### ")
        .skip(1)
        .map(|block| {
            let (slug, rest) = block.split_once('\n').unwrap();
            Record {
                slug: slug.trim().to_string(),
                lines: rest.lines().map(str::to_string).collect(),
            }
        })
        .collect()
}

/// A backticked `path::test_name` citation resolves when the file exists
/// under the workspace root and declares `fn test_name`.
fn cited_tests(text: &str) -> Vec<(String, String)> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|token| token.split_once("::"))
        .filter(|(path, _)| path.ends_with(".rs"))
        .map(|(path, test)| (path.to_string(), test.to_string()))
        .collect()
}

#[test]
fn every_evaluator_record_is_method_ordered_and_cites_an_executed_check() {
    let root = workspace_root();
    let part = root.join("docs/properties/evaluator");
    let catalog = std::fs::read_to_string(part.join("catalog.md")).unwrap();
    let records = records(&catalog);
    assert!(!records.is_empty());

    let index: BTreeSet<String> = catalog
        .lines()
        .filter(|line| line.starts_with("| [`"))
        .map(|line| line["| [`".len()..].split('`').next().unwrap().to_string())
        .collect();
    let slugs: BTreeSet<String> = records.iter().map(|r| r.slug.clone()).collect();
    assert_eq!(index, slugs, "index rows equal the record set");
    assert_eq!(slugs.len(), records.len(), "slugs are unique");

    for record in &records {
        let slug = &record.slug;
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{slug}: kebab-case"
        );
        let heads: Vec<&str> = record
            .lines
            .iter()
            .filter_map(|line| FIELDS.iter().copied().find(|f| line.starts_with(f)))
            .collect();
        assert_eq!(
            heads,
            FIELDS.to_vec(),
            "{slug}: the twelve fields in METHOD order"
        );
        let field = |head: &str| {
            record
                .lines
                .iter()
                .find(|line| line.starts_with(head))
                .map(|line| line[head.len()..].trim().to_string())
                .unwrap()
        };
        let check = field("Check:");
        let semantics = check.split('`').nth(1).unwrap_or_default();
        assert!(
            SEMANTICS.contains(&semantics),
            "{slug}: check semantics {semantics:?} is outside the closed set"
        );
        assert!(
            ["safety", "liveness", "reachability"].contains(&field("Type:").as_str()),
            "{slug}: type"
        );
        assert!(
            ["default-production", "explicit-config-only", "test-only"]
                .contains(&field("Reachability:").as_str()),
            "{slug}: reachability"
        );
        assert!(
            ["active", "invalidated"].contains(&field("Status:").as_str()),
            "{slug}: status"
        );
        let exercised = field("Exercised:");
        let (state, _) = exercised
            .split_once(" - ")
            .unwrap_or((exercised.as_str(), ""));
        assert!(
            ["not yet", "partial", "yes"].contains(&state),
            "{slug}: exercised state {state:?}"
        );
        if state != "not yet" {
            let cited = cited_tests(&exercised);
            assert!(
                !cited.is_empty(),
                "{slug}: an exercised record cites a test"
            );
            for (path, test) in cited {
                let source = std::fs::read_to_string(root.join(&path))
                    .unwrap_or_else(|e| panic!("{slug}: cited {path} is unreadable: {e}"));
                assert!(
                    source.contains(&format!("fn {test}(")),
                    "{slug}: {path} declares no test named {test}"
                );
            }
        }
        let confidence = field("Confidence:");
        let evidence_link = format!("[evidence](evidence/{slug}.md)");
        assert!(
            confidence.contains(&evidence_link),
            "{slug}: confidence links its evidence file"
        );
        let evidence = std::fs::read_to_string(part.join("evidence").join(format!("{slug}.md")))
            .unwrap_or_else(|e| panic!("{slug}: evidence file: {e}"));
        assert!(
            evidence.starts_with(&format!("# {slug}\n")),
            "{slug}: evidence title"
        );
        for heading in EVIDENCE_HEADINGS {
            assert!(
                evidence.contains(heading),
                "{slug}: evidence lacks {heading:?}"
            );
        }
        let open = field("Open questions:");
        if open.is_empty() {
            assert!(
                record.lines.iter().any(|line| line.starts_with("- ")),
                "{slug}: open questions list at least one question or says None."
            );
        } else {
            assert_eq!(open, "None.", "{slug}: open questions");
        }
    }
    let evidence_files: BTreeSet<String> = std::fs::read_dir(part.join("evidence"))
        .unwrap()
        .map(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".md")
                .to_string()
        })
        .collect();
    assert_eq!(
        evidence_files, slugs,
        "one evidence file per record and no strays"
    );
}
