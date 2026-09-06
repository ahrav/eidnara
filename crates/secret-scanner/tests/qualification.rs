//! Qualification fixture provides tooling evidence.

mod common;

use std::collections::BTreeSet;

use common::digest_hex;
use secret_scanner::{ScanProfile, Scanner};
use serde::Deserialize;

// `deny_unknown_fields` makes a renamed or added manifest field fail here instead of being silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    status: String,
    authority_qualified: bool,
    fixture: String,
    fixture_sha256: String,
    fixture_cases: usize,
    planned_scan_quota: usize,
    operational_review_complete: bool,
    production_divergence_classes_reproduced: bool,
    cells: Vec<Cell>,
    note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cell {
    cell_id: String,
    size_class: String,
    finding_density: String,
    feasibility: String,
    quota: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    case_id: String,
    cell_id: String,
    input: String,
    expected_rule_ids: Vec<String>,
    expected_divergence: String,
    consent: String,
}

#[test]
fn minimal_fixture_is_truthful_and_executable() {
    let manifest: Manifest =
        serde_json::from_str(include_str!("fixtures/qualification-manifest-v1.json")).unwrap();
    assert_eq!(
        manifest.schema,
        "eidnara.secret-scanner-qualification-manifest/v1"
    );
    assert_eq!(manifest.status, "tooling_only");
    assert!(!manifest.authority_qualified);
    assert_eq!(manifest.planned_scan_quota, 0);
    assert!(!manifest.operational_review_complete);
    assert!(!manifest.production_divergence_classes_reproduced);
    assert!(manifest.note.contains("Minimal synthetic fixture only"));
    assert_eq!(manifest.cells.len(), 16);
    for cell in &manifest.cells {
        assert_eq!(
            cell.cell_id,
            format!("size_{}__density_{}", cell.size_class, cell.finding_density)
        );
        assert_eq!(cell.feasibility, "unassessed");
        assert_eq!(cell.quota, 0);
    }

    let fixture_bytes = include_bytes!("fixtures/qualification-v1.jsonl");
    assert_eq!(manifest.fixture, "qualification-v1.jsonl");
    assert_eq!(
        manifest.fixture_sha256,
        digest_hex(fixture_bytes),
        "fixture bytes changed without updating fixture_sha256"
    );

    let fixtures: Vec<Fixture> = include_str!("fixtures/qualification-v1.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(fixtures.len(), manifest.fixture_cases);
    let scanner = Scanner::new(ScanProfile::Comprehensive).unwrap();
    let mut case_ids = BTreeSet::new();
    for fixture in fixtures {
        assert!(
            case_ids.insert(fixture.case_id.clone()),
            "duplicate case_id"
        );
        assert_eq!(fixture.consent, "synthetic");
        assert_eq!(fixture.expected_divergence, "match");
        assert!(
            manifest
                .cells
                .iter()
                .any(|cell| cell.cell_id == fixture.cell_id)
        );
        let observed_rule_ids = scanner
            .scan(&fixture.input)
            .unwrap()
            .findings
            .into_iter()
            .map(|finding| finding.rule_id)
            .collect::<BTreeSet<_>>();
        let expected_rule_ids = fixture
            .expected_rule_ids
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(observed_rule_ids, expected_rule_ids);
    }
}
