use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

const PART_SECTIONS: &[(&str, &[&str])] = &[
    ("export-recovery", &["## Independent situation markers"]),
    (
        "projection-coverage",
        &["## Per-record precondition markers"],
    ),
    (
        "embedding",
        &[
            "## Per-property map",
            "## Additional discriminating preconditions",
        ],
    ),
];

const ACCEPTANCE_CRITERIA: [&str; 12] = [
    "AC1", "AC2", "AC3", "AC4", "AC5", "AC6", "AC7", "AC8", "AC9", "AC10", "AC11", "AC12",
];
const SEAMS: [&str; 9] = ["T1", "T2", "T3", "T4", "T5", "T6", "T7", "T8", "T9"];
const HARNESSES: [&str; 2] = ["opencode", "pi"];
const ENTRY_POINTS: [&str; 4] = ["startup", "reload", "dispatch", "explicit"];
const SOURCE_CLASSES: [(&str, bool); 5] = [
    ("messages", true),
    ("canonical_claims", true),
    ("promoted_memory", true),
    ("git_commits", true),
    ("raw_tool_spans", false),
];
const HOOKS: [&str; 12] = [
    "search_projection.message_cleanup",
    "search_projection.embedding.bootstrap",
    "search_projection.embedding.routing",
    "search_projection.embedding.registry",
    "search_projection.embedding.backfill",
    "search_projection.embedding.identity_gc",
    "search_projection.promoted_memory.embeddings",
    "search_projection.git.ingest",
    "search_projection.git.durable_rows",
    "search_projection.git.jobs",
    "search_projection.git.sweeps",
    "search_projection.git.leases",
];
const GATES: [&str; 5] = [
    "class_coverage",
    "freshness",
    "resource",
    "capability",
    "both_harness",
];
const INVALIDATION_IDENTITY: [&str; 8] = [
    "schema_version",
    "tokenizer_fingerprint",
    "embedding_model",
    "projection_policy_version",
    "identity_contract_version",
    "limit_manifest_protocol_version",
    "vector_dimension",
    "generation_epoch",
];
const MANIFEST_FIELDS: [&str; 9] = [
    "protocol_version",
    "workload",
    "hardware",
    "corpus_snapshot",
    "metric",
    "sampling",
    "limits",
    "approvers",
    "approval_digest",
];
const LIMITS: [&str; 27] = [
    "export_page_rows",
    "export_page_encoded_bytes",
    "export_row_standalone_encoded_bytes",
    "export_live_decoded_bytes",
    "catchup_batch_commits",
    "catchup_batch_encoded_bytes",
    "catchup_batch_source_bytes",
    "local_transaction_rows",
    "local_transaction_bytes",
    "pending_count",
    "pending_bytes",
    "embedding_input_bytes",
    "embedding_input_tokens",
    "supervisor_slice_ms",
    "retry_attempts",
    "lease_duration_ms",
    "capture_reference_count",
    "capture_hold_expiry_ms",
    "capture_disk_bytes",
    "catchup_lag_commits",
    "B_catchup_ms",
    "B_recovery_ms",
    "B_authorized_recovery_ms",
    "embedding_recovery_attempts",
    "query_service_opportunities",
    "physical_drain_ms",
    "decoded_heap_high_water_bytes",
];
const ADMISSION_DIMENSIONS: [&str; 13] = [
    "row_count",
    "encoded_bytes",
    "source_bytes",
    "payload_size",
    "identity_and_range_bounds",
    "logical_decoded_memory_charge",
    "local_mutation_charge",
    "pending_count",
    "pending_bytes",
    "embedding_input_bytes",
    "embedding_input_tokens",
    "capture_reference_count",
    "capture_work",
];
const CAPABILITY_DISPOSITIONS: [(&str, &str); 9] = [
    ("message_text_block_capture", "required"),
    ("durable_session_identity", "required"),
    ("tool_result_native_string_capture", "required"),
    ("tool_result_revision_identity", "required"),
    ("multipart_error_result_capture", "required"),
    ("git_repository_scope_declaration", "required"),
    ("projection_hook_activation", "required"),
    ("dense_raw_tool_embedding", "optional_disabled"),
    ("host_mural_rendering", "not_applicable"),
];
const TICKETS: std::ops::RangeInclusive<u64> = 352..=388;
const RESULT_MARKER: &str = "search_projection_acceptance_situations_witnessed";
const CONDITIONAL_CELLS: [&str; 1] = ["search_projection_remediation_without_revision_change"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/search-projection")
        .join(name);
    serde_json::from_str(&read(&path)).unwrap_or_else(|err| panic!("parse {name}: {err}"))
}

fn str_list(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item.as_str().expect("string"))
        .collect()
}

fn marker_tokens(text: &str) -> Vec<String> {
    assert_eq!(
        text.matches('`').count() % 2,
        0,
        "unbalanced backticks change which token a row defines"
    );
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|segment| {
            segment.starts_with("search_projection_")
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        .map(str::to_owned)
        .collect()
}

fn defined_markers(path: &Path, sections: &[&str]) -> Vec<String> {
    let text = read(path);
    let mut defined = Vec::new();
    for section in sections {
        let heading = format!("{section}\n");
        assert_eq!(
            text.matches(&heading).count(),
            1,
            "{} must contain {section} exactly once",
            path.display()
        );
        let start = text.find(&heading).expect("counted above");
        let body = &text[start + section.len()..];
        let end = body.find("\n## ").unwrap_or(body.len());
        for line in body[..end].lines() {
            if !line.starts_with("| ") || line.starts_with("| ---") {
                continue;
            }
            match marker_tokens(line).as_slice() {
                [] => continue,
                [one] => defined.push(one.clone()),
                many => panic!("{} row defines several markers: {many:?}", path.display()),
            }
        }
    }
    defined
}

fn all_definitions(matrix: &Value) -> Vec<(&'static str, Vec<String>)> {
    let sites = matrix["marker_definition_sites"]
        .as_object()
        .expect("definition sites");
    assert_eq!(sites.len(), PART_SECTIONS.len());
    PART_SECTIONS
        .iter()
        .map(|(part, sections)| {
            let path = repo_root().join(sites[*part].as_str().expect("path"));
            (*part, defined_markers(&path, sections))
        })
        .collect()
}

fn cells(matrix: &Value) -> &Vec<Value> {
    matrix["cells"].as_array().expect("cells")
}

fn required_cells(matrix: &Value) -> Vec<&Value> {
    cells(matrix)
        .iter()
        .filter(|cell| cell["cell_kind"] != "result")
        .collect()
}

fn cell_for<'a>(matrix: &'a Value, marker: &str) -> &'a Value {
    cells(matrix)
        .iter()
        .find(|cell| cell["marker"] == marker)
        .unwrap_or_else(|| panic!("{marker} has no matrix cell"))
}

fn marker(cell: &Value) -> &str {
    cell["marker"].as_str().expect("marker")
}

#[test]
fn fault_maps_define_each_marker_once_and_mention_no_undefined_marker() {
    let matrix = fixture("witness-matrix.json");
    let expected = matrix["expected_marker_counts"]
        .as_object()
        .expect("counts");
    let mut union = BTreeSet::new();
    let mut total = 0;
    for (part, defined) in all_definitions(&matrix) {
        let unique: BTreeSet<_> = defined.iter().cloned().collect();
        assert_eq!(unique.len(), defined.len(), "{part} defines a marker twice");
        assert_eq!(
            defined.len() as u64,
            expected[part].as_u64().expect("count"),
            "{part} definition count",
        );
        assert!(
            union.is_disjoint(&unique),
            "{part} redefines a marker owned by another map",
        );
        total += defined.len();
        union.extend(unique);
    }
    assert_eq!(total, 65);
    assert_eq!(union.len(), 65);

    let sites = matrix["marker_definition_sites"]
        .as_object()
        .expect("definition sites");
    for (part, path) in sites {
        let text = read(&repo_root().join(path.as_str().expect("path")));
        for token in marker_tokens(&text) {
            assert!(
                union.contains(&token),
                "{part} mentions undefined marker {token}"
            );
        }
    }
}

#[test]
fn matrix_cells_are_the_defined_markers_in_definition_order() {
    let matrix = fixture("witness-matrix.json");
    let defined: Vec<String> = all_definitions(&matrix)
        .into_iter()
        .flat_map(|(_, markers)| markers)
        .collect();
    let listed: Vec<String> = cells(&matrix)
        .iter()
        .map(|cell| marker(cell).to_owned())
        .collect();
    assert_eq!(listed, defined, "cells differ from the definition order");
    for (part, markers) in all_definitions(&matrix) {
        for name in markers {
            assert_eq!(
                cell_for(&matrix, &name)["part"],
                part,
                "{name} is filed under the wrong map"
            );
        }
    }
}

#[test]
fn result_marker_is_recorded_once_and_is_not_a_required_cell() {
    let matrix = fixture("witness-matrix.json");
    assert_eq!(matrix["result_marker"], RESULT_MARKER);
    let mut result_cells = 0;
    for cell in cells(&matrix) {
        let name = marker(cell);
        match cell["cell_kind"].as_str().expect("cell_kind") {
            "required" => {
                assert_ne!(name, RESULT_MARKER);
                assert!(cell.get("applicability_decision").is_none());
            }
            "conditional-applicability" => {
                assert!(
                    CONDITIONAL_CELLS.contains(&name),
                    "{name} is not an allowed conditional cell"
                );
                let decision = cell["applicability_decision"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{name} records no applicability decision"));
                assert!(decision.contains("domains.name"));
            }
            "result" => {
                assert_eq!(name, RESULT_MARKER);
                result_cells += 1;
            }
            other => panic!("{name}: unknown cell kind {other}"),
        }
    }
    assert_eq!(result_cells, 1);
    for name in CONDITIONAL_CELLS {
        assert_eq!(
            cell_for(&matrix, name)["cell_kind"],
            "conditional-applicability"
        );
    }
}

#[test]
fn every_acceptance_criterion_and_seam_has_a_required_cell() {
    let matrix = fixture("witness-matrix.json");
    assert_eq!(
        str_list(&matrix["acceptance_criteria"]),
        ACCEPTANCE_CRITERIA
    );
    assert_eq!(str_list(&matrix["seams"]), SEAMS);
    let required = required_cells(&matrix);
    for (field, universe) in [
        ("acceptance_criteria", &ACCEPTANCE_CRITERIA[..]),
        ("seams", &SEAMS[..]),
    ] {
        for item in universe {
            assert!(
                required
                    .iter()
                    .any(|cell| str_list(&cell[field]).contains(item)),
                "{item} has no required cell",
            );
        }
        for cell in cells(&matrix) {
            let traced = str_list(&cell[field]);
            assert!(!traced.is_empty(), "{} traces no {field}", marker(cell));
            for item in traced {
                assert!(
                    universe.contains(&item),
                    "{} traces unknown {item}",
                    marker(cell)
                );
            }
        }
    }
}

#[test]
fn every_cell_names_scenarios_oracle_control_integration_and_owner() {
    let matrix = fixture("witness-matrix.json");
    for cell in cells(&matrix) {
        let name = marker(cell);
        let scenarios = str_list(&cell["scenario_ids"]);
        assert!(!scenarios.is_empty(), "{name} declares no scenario");
        let unique: BTreeSet<_> = scenarios.iter().collect();
        assert_eq!(unique.len(), scenarios.len(), "{name} repeats a scenario");
        for field in [
            "independent_oracle",
            "negative_control",
            "integration_observation",
        ] {
            let text = cell[field]
                .as_str()
                .unwrap_or_else(|| panic!("{name} lacks {field}"));
            assert!(
                text.split_whitespace().count() >= 4,
                "{name} has a placeholder {field}: {text:?}"
            );
        }
        assert!(
            cell["negative_control"]
                .as_str()
                .expect("control")
                .contains("must fail"),
            "{name}: a negative control names the wrong implementation that must fail"
        );
        let owners = cell["constructing_tickets"].as_array().expect("tickets");
        assert!(!owners.is_empty(), "{name} names no constructing ticket");
        for owner in owners {
            let number = owner.as_u64().expect("ticket number");
            assert!(
                TICKETS.contains(&number),
                "{name} names ticket {number} outside the projection ticket range",
            );
        }
    }
}

#[test]
fn gate_cells_enumerate_every_hook_entry_point_and_evidence_state() {
    let matrix = fixture("witness-matrix.json");
    assert_eq!(str_list(&matrix["hooks"]), HOOKS);
    assert_eq!(str_list(&matrix["entry_points"]), ENTRY_POINTS);
    assert_eq!(str_list(&matrix["harnesses"]), HARNESSES);
    let expect_product = |name: &str, states: &[&str]| {
        let mut expected = BTreeSet::new();
        for hook in HOOKS {
            for entry in ENTRY_POINTS {
                for state in states {
                    expected.insert(match *state {
                        "" => format!("{hook}@{entry}"),
                        state => format!("{hook}@{entry}@{state}"),
                    });
                }
            }
        }
        let actual: BTreeSet<String> = str_list(&cell_for(&matrix, name)["scenario_ids"])
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(actual, expected, "{name} scenario product");
    };
    expect_product("search_projection_missing_gate_evidence", &[""]);
    expect_product("search_projection_failed_gate_evidence", &[""]);
    expect_product(
        "search_projection_unsupported_or_inapplicable_evidence",
        &["unsupported", "inapplicable"],
    );
    let control = matrix["gate_controls"]["valid_supported_activation"]
        .as_str()
        .expect("valid activation control");
    assert!(control.contains("every hook at every entry point"));
}

fn scenario_column(cell: &Value) -> String {
    let scenarios = str_list(&cell["scenario_ids"]);
    match marker(cell) {
        "search_projection_missing_gate_evidence" | "search_projection_failed_gate_evidence" => {
            format!(
                "every hook × every entry point ({} cells: `<hook>@<entry>`)",
                scenarios.len()
            )
        }
        "search_projection_unsupported_or_inapplicable_evidence" => format!(
            "every hook × every entry point × {{`unsupported`, `inapplicable`}} ({} cells: `<hook>@<entry>@<state>`)",
            scenarios.len()
        ),
        _ => scenarios
            .iter()
            .map(|scenario| format!("`{scenario}`"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn required_row(cell: &Value) -> String {
    let kind = if cell["cell_kind"] == "conditional-applicability" {
        " (conditional applicability)"
    } else {
        ""
    };
    let part = cell["part"].as_str().expect("part");
    let property = cell["property"].as_str().expect("property");
    let tickets = cell["constructing_tickets"]
        .as_array()
        .expect("tickets")
        .iter()
        .map(|ticket| format!("#{ticket}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "| `{}`{kind} | [{property}]({part}/catalog.md#{property}) | {} | {} | {} | {} | {} | {} | {tickets} |",
        marker(cell),
        str_list(&cell["acceptance_criteria"]).join(", "),
        str_list(&cell["seams"]).join(", "),
        scenario_column(cell),
        cell["independent_oracle"].as_str().expect("oracle"),
        cell["negative_control"].as_str().expect("control"),
        cell["integration_observation"]
            .as_str()
            .expect("observation"),
    )
}

fn coverage_rows<'a>(matrix: &'a Value, field: &str, universe: &[&'a str]) -> Vec<String> {
    universe
        .iter()
        .map(|item| {
            let markers: Vec<String> = required_cells(matrix)
                .iter()
                .filter(|cell| str_list(&cell[field]).contains(item))
                .map(|cell| format!("`{}`", marker(cell)))
                .collect();
            format!("| {item} | {} | {} |", markers.len(), markers.join(", "))
        })
        .collect()
}

fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("document lacks {heading}"));
    let body = &text[start + heading.len()..];
    let end = body.find("\n## ").unwrap_or(body.len());
    &body[..end]
}

/// Data rows of every table in `body`. A header row is the row immediately
/// followed by the `| --- |` separator, so no header text is hard-coded.
fn table_rows(body: &str) -> Vec<String> {
    let lines: Vec<&str> = body.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("| ") && !line.starts_with("| ---"))
        .filter(|(index, _)| {
            !lines
                .get(index + 1)
                .is_some_and(|next| next.starts_with("| ---"))
        })
        .map(|(_, line)| (*line).to_owned())
        .collect()
}

/// `path=value` citations in cell prose. A citation names a
/// construction-contracts.json record by dotted key path; `value` is the JSON
/// literal the record must hold, or the bare word when it is not a literal.
fn contract_citations(text: &str) -> Vec<(String, Value)> {
    text.split_whitespace()
        .filter_map(|word| {
            let word = word.trim_end_matches(['.', ',', ';', ':', ')']);
            let (path, value) = word.split_once('=')?;
            let is_path = !path.is_empty()
                && path.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'
                        || byte == b'.'
                });
            is_path.then(|| {
                let value =
                    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
                (path.to_owned(), value)
            })
        })
        .collect()
}

/// Seam identifiers in a prose column: `T4` names one seam and `T1-T9` names
/// the inclusive range, so "All T1-T9" expands to every seam.
fn seam_tokens(text: &str) -> BTreeSet<&'static str> {
    let index = |token: &str| SEAMS.iter().position(|seam| *seam == token);
    let mut seams = BTreeSet::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches(['.', ',', ';', ':', '(', ')']);
        match word.split_once('-') {
            Some((low, high)) => {
                if let (Some(low), Some(high)) = (index(low), index(high)) {
                    seams.extend(&SEAMS[low..=high]);
                }
            }
            None => {
                if let Some(position) = index(word) {
                    seams.insert(SEAMS[position]);
                }
            }
        }
    }
    seams
}

#[test]
fn markdown_matrix_restates_the_fixture_exactly() {
    let matrix = fixture("witness-matrix.json");
    let text = read(&repo_root().join("docs/properties/search-projection/witness-matrix.md"));

    let expected_rows: Vec<String> = required_cells(&matrix)
        .iter()
        .map(|cell| required_row(cell))
        .collect();
    assert_eq!(
        table_rows(section(&text, "## Required cells")),
        expected_rows,
        "required-cell rows differ from the fixture"
    );

    let result = section(&text, "## Result marker");
    assert!(result.contains(&format!("`{RESULT_MARKER}`")));
    assert!(!result.contains("| `search_projection_"));

    assert_eq!(
        table_rows(section(&text, "## Coverage by acceptance criterion")),
        coverage_rows(&matrix, "acceptance_criteria", &ACCEPTANCE_CRITERIA),
    );
    assert_eq!(
        table_rows(section(&text, "## Coverage by testing seam")),
        coverage_rows(&matrix, "seams", &SEAMS),
    );

    let fields: Vec<String> = section(&text, "## Later evidence every witness record carries")
        .lines()
        .filter(|line| line.starts_with("- `"))
        .map(|line| {
            line.trim_start_matches("- `")
                .trim_end_matches('`')
                .to_owned()
        })
        .collect();
    assert_eq!(fields, str_list(&matrix["witness_record_fields"]));
}

#[test]
fn cell_prose_cites_only_contract_records_that_exist() {
    let matrix = fixture("witness-matrix.json");
    let contracts = fixture("construction-contracts.json");
    let mut citations = 0;
    for cell in cells(&matrix) {
        let name = marker(cell);
        for field in [
            "independent_oracle",
            "negative_control",
            "integration_observation",
        ] {
            for (path, value) in contract_citations(cell[field].as_str().expect(field)) {
                let record = path
                    .split('.')
                    .try_fold(&contracts, |node, key| node.get(key))
                    .unwrap_or_else(|| {
                        panic!(
                            "{name} {field} cites {path}, which construction-contracts.json lacks"
                        )
                    });
                assert_eq!(record, &value, "{name} {field} misstates {path}");
                citations += 1;
            }
        }
    }
    assert!(citations >= 1, "no cell cites a contract record");
}

#[test]
fn traceability_map_restates_matrix_coverage_per_property() {
    let matrix = fixture("witness-matrix.json");
    let text = read(&repo_root().join("docs/properties/search-projection/spec-traceability.md"));
    let mut expected: BTreeMap<&str, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for cell in cells(&matrix) {
        let entry = expected
            .entry(cell["property"].as_str().expect("property"))
            .or_default();
        entry.0.extend(str_list(&cell["acceptance_criteria"]));
        entry.1.extend(str_list(&cell["seams"]));
    }

    let mut listed = BTreeSet::new();
    for row in table_rows(section(&text, "## Property-to-specification map")) {
        let columns: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
        let [property, obligation, seam] = columns.as_slice() else {
            panic!("property row has {} columns: {row}", columns.len());
        };
        let slug = property
            .strip_prefix('[')
            .and_then(|link| link.split_once(']'))
            .map(|(slug, _)| slug)
            .unwrap_or_else(|| panic!("property column is not a catalog link: {property}"));
        let (criteria, seams) = expected
            .get(slug)
            .unwrap_or_else(|| panic!("{slug} has no witness-matrix cell"));
        let cited: BTreeSet<&str> = obligation
            .split(|c: char| c == ';' || c == '/' || c.is_whitespace())
            .filter(|token| ACCEPTANCE_CRITERIA.contains(token))
            .collect();
        assert_eq!(
            &cited, criteria,
            "{slug} acceptance criteria differ from its matrix cells"
        );
        assert_eq!(
            seam_tokens(seam),
            seams.iter().copied().collect(),
            "{slug} seams differ from its matrix cells"
        );
        assert!(listed.insert(slug.to_owned()), "{slug} is listed twice");
    }
    assert_eq!(
        listed,
        expected.keys().map(|slug| (*slug).to_owned()).collect(),
        "the traceability map lists a different property set than the matrix"
    );
}

#[test]
fn contracts_freeze_the_five_classes() {
    let contracts = fixture("construction-contracts.json");
    assert_eq!(
        contracts["identity_contract_version"],
        "search-projection-identity-v2"
    );
    let classes = contracts["classes"].as_object().expect("classes");
    assert_eq!(
        classes.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        SOURCE_CLASSES.map(|(name, _)| name).into()
    );
    for (name, dense) in SOURCE_CLASSES {
        let class = &classes[name];
        assert!(
            !str_list(&class["identity_fields"]).is_empty(),
            "{name} has no identity fields"
        );
        assert!(
            !str_list(&class["representations"]).is_empty(),
            "{name} has no representation"
        );
        assert_eq!(class["span_default"], "whole", "{name} span default");
        assert_eq!(class["dense_default"], dense, "{name} dense default");
    }
    assert_eq!(
        str_list(&contracts["tuple_encoding"]["field_order"]),
        [
            "class",
            "namespaced_identity",
            "revision",
            "representation",
            "span"
        ]
    );
    assert_eq!(contracts["tuple_encoding"]["version_byte"], 2);
    assert_eq!(contracts["tuple_encoding"]["role_bytes"]["occurrence"], 0);
    assert_eq!(contracts["tuple_encoding"]["role_bytes"]["lineage"], 1);
    assert_eq!(
        str_list(&contracts["tuple_encoding"]["lineage_field_order"]),
        ["class", "namespaced_identity", "representation", "span"]
    );
    assert_eq!(contracts["tuple_encoding"]["max_identity_value_bytes"], 512);
    assert_eq!(
        str_list(&contracts["admission_dimensions"]),
        ADMISSION_DIMENSIONS
    );
    assert_eq!(
        contracts["remediation_applicability"]["field"],
        "domains.name"
    );
    assert_eq!(
        contracts["remediation_applicability"]["mapping_consumes_field"],
        false
    );
    assert!(str_list(&contracts["excluded_inputs"]).contains(&"domains.name"));
}

#[test]
fn contracts_freeze_capability_dispositions_for_both_harnesses() {
    let contracts = fixture("construction-contracts.json");
    let dispositions = &contracts["capability_dispositions"];
    assert_eq!(
        str_list(&dispositions["disposition_values"]),
        ["required", "optional_disabled", "not_applicable"]
    );
    assert_eq!(dispositions["evidence_status"], "unwitnessed");
    let capabilities = dispositions["capabilities"]
        .as_array()
        .expect("capabilities");
    let listed: Vec<(&str, &str, &str)> = capabilities
        .iter()
        .map(|capability| {
            (
                capability["capability"].as_str().expect("name"),
                capability["opencode"].as_str().expect("opencode"),
                capability["pi"].as_str().expect("pi"),
            )
        })
        .collect();
    assert_eq!(
        listed,
        CAPABILITY_DISPOSITIONS.map(|(name, disposition)| (name, disposition, disposition))
    );
    let classes = contracts["classes"].as_object().expect("classes");
    let mut covered = BTreeSet::new();
    for capability in capabilities {
        for class in str_list(&capability["classes"]) {
            assert!(classes.contains_key(class), "unknown class {class}");
            if HARNESSES
                .iter()
                .all(|harness| capability[*harness] == "required")
            {
                covered.insert(class);
            }
        }
    }
    assert_eq!(
        covered,
        classes.keys().map(String::as_str).collect(),
        "every class needs a required capability on both harnesses",
    );
}

#[test]
fn contracts_freeze_the_hook_gate_map() {
    let contracts = fixture("construction-contracts.json");
    let defaults = &contracts["hook_defaults"];
    assert_eq!(str_list(&defaults["gates"]), GATES);
    assert_eq!(str_list(&defaults["both_harness_evidence"]), HARNESSES);
    assert_eq!(
        str_list(&defaults["invalidation_identity"]),
        INVALIDATION_IDENTITY
    );
    assert_eq!(str_list(&defaults["entry_points"]), ENTRY_POINTS);
    assert_eq!(defaults["default_state"], "disabled");
    let hooks = contracts["hooks"].as_object().expect("hooks");
    assert_eq!(
        hooks.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        HOOKS.into()
    );
    let classes = contracts["classes"].as_object().expect("classes");
    for (name, hook) in hooks {
        let hook_classes = str_list(&hook["classes"]);
        assert!(!hook_classes.is_empty(), "{name} covers no class");
        for class in hook_classes {
            assert!(
                classes.contains_key(class),
                "{name} names unknown class {class}"
            );
        }
        let ticket = hook["constructing_ticket"].as_u64().expect("ticket");
        assert!(TICKETS.contains(&ticket), "{name} ticket {ticket}");
        assert_eq!(
            hook.as_object().expect("hook").len(),
            2,
            "{name} overrides a hook default"
        );
    }
}

#[test]
fn limit_manifest_interface_names_every_limit_and_approves_no_number() {
    let contracts = fixture("construction-contracts.json");
    let manifest = &contracts["limit_manifest_interface"];
    assert!(manifest["protocol_version"].is_null());
    assert_eq!(str_list(&manifest["required_fields"]), MANIFEST_FIELDS);
    let limits = manifest["limits"].as_object().expect("limits");
    assert_eq!(
        limits.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        LIMITS.into()
    );
    for (name, value) in limits {
        assert!(
            value.is_null(),
            "{name} carries a value; the interface approves no number"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    UnknownClass,
    MissingIdentityField,
    UnknownIdentityField,
    MalformedIdentityValue,
    UnknownHarness,
    MalformedOid,
    MissingRevision,
    MalformedRevision,
    UnknownRepresentation,
    PayloadNotString,
    MalformedSpan,
    SpanReversed,
    SpanOutOfRange,
    SpanNotUtf8Aligned,
}

const MAX_IDENTITY_VALUE_BYTES: usize = 512;

const REFUSALS: [(Refusal, &str); 14] = [
    (Refusal::UnknownClass, "unknown_class"),
    (Refusal::MissingIdentityField, "missing_identity_field"),
    (Refusal::UnknownIdentityField, "unknown_identity_field"),
    (Refusal::MalformedIdentityValue, "malformed_identity_value"),
    (Refusal::UnknownHarness, "unknown_harness"),
    (Refusal::MalformedOid, "malformed_oid"),
    (Refusal::MissingRevision, "missing_revision"),
    (Refusal::MalformedRevision, "malformed_revision"),
    (Refusal::UnknownRepresentation, "unknown_representation"),
    (Refusal::PayloadNotString, "payload_not_string"),
    (Refusal::MalformedSpan, "malformed_span"),
    (Refusal::SpanReversed, "span_reversed"),
    (Refusal::SpanOutOfRange, "span_out_of_range"),
    (Refusal::SpanNotUtf8Aligned, "span_not_utf8_aligned"),
];

impl Refusal {
    fn name(self) -> &'static str {
        REFUSALS
            .iter()
            .find(|(refusal, _)| *refusal == self)
            .map(|(_, name)| *name)
            .expect("every refusal is named")
    }
}

struct Validated<'a> {
    class: &'a str,
    fields: Vec<&'a str>,
    identity: &'a serde_json::Map<String, Value>,
    revision: &'a str,
    representation: &'a str,
    payload: &'a str,
    span: Option<(usize, usize)>,
}

fn validate<'a>(contracts: &'a Value, record: &'a Value) -> Result<Validated<'a>, Refusal> {
    let class = record["class"].as_str().ok_or(Refusal::UnknownClass)?;
    let spec = contracts["classes"]
        .get(class)
        .ok_or(Refusal::UnknownClass)?;
    let fields = str_list(&spec["identity_fields"]);
    let identity = record["identity"]
        .as_object()
        .ok_or(Refusal::MissingIdentityField)?;
    for field in &fields {
        if !identity.get(*field).is_some_and(Value::is_string) {
            return Err(Refusal::MissingIdentityField);
        }
    }
    if identity.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(Refusal::UnknownIdentityField);
    }
    for field in &fields {
        let value = identity[*field].as_str().expect("checked above");
        if value.is_empty()
            || value.len() > MAX_IDENTITY_VALUE_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(Refusal::MalformedIdentityValue);
        }
    }
    if let Some(harness) = identity.get("harness").and_then(Value::as_str)
        && !HARNESSES.contains(&harness)
    {
        return Err(Refusal::UnknownHarness);
    }
    if class == "git_commits" {
        let oid = identity["oid"].as_str().expect("checked above");
        let expected_len = match identity["object_format"].as_str() {
            Some("sha1") => 40,
            Some("sha256") => 64,
            _ => return Err(Refusal::MalformedOid),
        };
        let lowercase_hex = oid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if oid.len() != expected_len || !lowercase_hex {
            return Err(Refusal::MalformedOid);
        }
    }
    let revision = record["revision"]
        .as_str()
        .filter(|revision| !revision.is_empty())
        .ok_or(Refusal::MissingRevision)?;
    // One number has one spelling: the shortest nonnegative decimal that fits i64.
    let canonical = revision
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0 && value.to_string() == revision);
    if canonical.is_none() {
        return Err(Refusal::MalformedRevision);
    }
    let representation = record["representation"]
        .as_str()
        .ok_or(Refusal::UnknownRepresentation)?;
    if !str_list(&spec["representations"]).contains(&representation) {
        return Err(Refusal::UnknownRepresentation);
    }
    let payload = record["payload"]
        .as_str()
        .ok_or(Refusal::PayloadNotString)?;
    let span = match record.get("span") {
        None | Some(Value::Null) => None,
        Some(span) => {
            let bounds = span.as_array().ok_or(Refusal::MalformedSpan)?;
            let [start, end] = bounds.as_slice() else {
                return Err(Refusal::MalformedSpan);
            };
            let start = start
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(Refusal::MalformedSpan)?;
            let end = end
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(Refusal::MalformedSpan)?;
            if start > end {
                return Err(Refusal::SpanReversed);
            }
            if end > payload.len() {
                return Err(Refusal::SpanOutOfRange);
            }
            if !payload.is_char_boundary(start) || !payload.is_char_boundary(end) {
                return Err(Refusal::SpanNotUtf8Aligned);
            }
            // A span selecting every byte is the whole-block selection.
            (start != 0 || end != payload.len()).then_some((start, end))
        }
    };
    Ok(Validated {
        class,
        fields,
        identity,
        revision,
        representation,
        payload,
        span,
    })
}

fn push_str(out: &mut Vec<u8>, text: &str) {
    let len = u32::try_from(text.len()).expect("fixture strings fit u32");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(text.as_bytes());
}

struct Encoded {
    tuple: Vec<u8>,
    lineage: Vec<u8>,
    selected: Vec<u8>,
    /// The span as encoded: `None` for the whole block, however the record spelled it.
    span: Option<(usize, usize)>,
}

/// Version byte, role byte, class, identity pairs, then the role's tail and
/// the span discriminator. The occurrence role carries the revision; the
/// lineage role does not.
fn finish(prefix: &[u8], role: u8, tail: &[&str], span: Option<(usize, usize)>) -> Vec<u8> {
    let mut out = vec![2u8, role];
    out.extend_from_slice(prefix);
    for text in tail {
        push_str(&mut out, text);
    }
    match span {
        None => out.push(0),
        Some((start, end)) => {
            out.push(1);
            out.extend_from_slice(&(start as u64).to_be_bytes());
            out.extend_from_slice(&(end as u64).to_be_bytes());
        }
    }
    out
}

fn encode(validated: &Validated<'_>) -> Encoded {
    let mut prefix = Vec::new();
    push_str(&mut prefix, validated.class);
    prefix.extend_from_slice(
        &u32::try_from(validated.fields.len())
            .expect("small")
            .to_be_bytes(),
    );
    for field in &validated.fields {
        push_str(&mut prefix, field);
        push_str(
            &mut prefix,
            validated.identity[*field].as_str().expect("validated"),
        );
    }
    let tuple = finish(
        &prefix,
        0,
        &[validated.revision, validated.representation],
        validated.span,
    );
    let lineage = finish(&prefix, 1, &[validated.representation], validated.span);
    let payload = validated.payload.as_bytes();
    let selected = match validated.span {
        None => payload.to_vec(),
        Some((start, end)) => payload[start..end].to_vec(),
    };
    Encoded {
        tuple,
        lineage,
        selected,
        span: validated.span,
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Records {
    fixtures: Value,
    encoded: BTreeMap<String, Encoded>,
}

fn encoded_records() -> Records {
    let contracts = fixture("construction-contracts.json");
    let fixtures = fixture("source-identity-fixtures.json");
    assert_eq!(
        fixtures["identity_contract_version"],
        contracts["identity_contract_version"]
    );
    let mut encoded = BTreeMap::new();
    for record in fixtures["records"].as_array().expect("records") {
        let id = record["id"].as_str().expect("id").to_owned();
        let validated = validate(&contracts, record)
            .unwrap_or_else(|refusal| panic!("{id} refused: {refusal:?}"));
        assert!(
            encoded.insert(id.clone(), encode(&validated)).is_none(),
            "{id} appears twice"
        );
    }
    Records { fixtures, encoded }
}

fn record<'a>(records: &'a Records, id: &str) -> &'a Value {
    records.fixtures["records"]
        .as_array()
        .expect("records")
        .iter()
        .find(|record| record["id"] == id)
        .unwrap_or_else(|| panic!("no record {id}"))
}

fn pairs(value: &Value) -> Vec<(&str, &str)> {
    value
        .as_array()
        .expect("pairs")
        .iter()
        .map(|pair| match str_list(pair).as_slice() {
            [a, b] => (*a, *b),
            other => panic!("expectation pair has {} elements", other.len()),
        })
        .collect()
}

#[test]
fn fixture_records_match_their_golden_identities() {
    let records = encoded_records();
    for (id, encoded) in &records.encoded {
        let expected = record(&records, id);
        assert_eq!(
            hex_digest(&encoded.tuple),
            expected["expected_occurrence_id"],
            "{id} occurrence identity"
        );
        assert_eq!(
            hex_digest(&encoded.selected),
            expected["expected_payload_id"],
            "{id} payload identity"
        );
        assert_eq!(
            hex_digest(&encoded.lineage),
            expected["expected_lineage_id"],
            "{id} lineage identity"
        );
    }
    // A lineage id never equals any occurrence id: the role byte separates them.
    let occurrences: BTreeSet<String> = records
        .encoded
        .values()
        .map(|encoded| hex_digest(&encoded.tuple))
        .collect();
    for encoded in records.encoded.values() {
        assert!(!occurrences.contains(&hex_digest(&encoded.lineage)));
    }
    assert!(
        records.fixtures["expectations"]["lineage_never_equals_occurrence"]
            .as_bool()
            .unwrap()
    );
}

#[test]
fn fixture_records_share_a_lineage_only_across_revisions() {
    let records = encoded_records();
    let encoded = &records.encoded;
    let expectations = &records.fixtures["expectations"];
    for (a, b) in pairs(&expectations["equal_lineages"]) {
        assert_eq!(
            encoded[a].lineage, encoded[b].lineage,
            "{a}/{b} share a lineage"
        );
    }
    for (a, b) in pairs(&expectations["distinct_lineages"]) {
        assert_ne!(
            encoded[a].lineage, encoded[b].lineage,
            "{a}/{b} lineages differ"
        );
    }
    // Every equal-lineage pair differs only in revision or in how it spells
    // the whole-block selection.
    for (a, b) in pairs(&expectations["equal_lineages"]) {
        let (ra, rb) = (record(&records, a), record(&records, b));
        assert_eq!(ra["class"], rb["class"]);
        assert_eq!(ra["identity"], rb["identity"]);
        assert_eq!(ra["representation"], rb["representation"]);
        assert_eq!(encoded[a].span, encoded[b].span);
    }
}

#[test]
fn fixture_records_keep_distinct_occurrences_over_shared_payloads() {
    let records = encoded_records();
    let encoded = &records.encoded;
    let expectations = &records.fixtures["expectations"];
    let distinct = pairs(&expectations["distinct_occurrences"]);
    for (a, b) in &distinct {
        assert_ne!(
            encoded[*a].tuple, encoded[*b].tuple,
            "{a} and {b} must be distinct occurrences"
        );
    }
    for (a, b) in pairs(&expectations["equal_occurrences"]) {
        assert_eq!(
            encoded[a].tuple, encoded[b].tuple,
            "{a} and {b} must be one occurrence"
        );
    }
    for (a, b) in pairs(&expectations["equal_payloads"]) {
        assert_eq!(
            encoded[a].selected, encoded[b].selected,
            "{a}/{b} share payload bytes"
        );
        assert!(
            distinct.contains(&(a, b)),
            "{a}/{b}: shared payload bytes must sit under distinct occurrences"
        );
    }
    for (a, b) in pairs(&expectations["distinct_payloads"]) {
        assert_ne!(
            encoded[a].selected, encoded[b].selected,
            "{a}/{b} payloads differ"
        );
    }
}

#[test]
fn fixture_payloads_and_spans_round_trip_exactly() {
    let records = encoded_records();
    let encoded = &records.encoded;
    let expectations = &records.fixtures["expectations"];
    let exact = expectations["exact_round_trip"]
        .as_object()
        .expect("exact round trip map");
    assert!(exact.len() >= 6);
    for (id, expected) in exact {
        let expected = expected.as_str().expect("expected bytes");
        assert_eq!(
            encoded[id].selected,
            expected.as_bytes(),
            "{id} bytes changed"
        );
        assert!(
            record(&records, id).get("span").is_none(),
            "{id} is a whole-block record"
        );
    }
    for (id, expected) in expectations["span_payloads"]
        .as_object()
        .expect("span payloads")
    {
        let expected = expected.as_str().expect("string");
        assert_eq!(
            encoded[id].selected,
            expected.as_bytes(),
            "{id} span selection"
        );
        let whole = record(&records, id)["payload"].as_str().expect("payload");
        assert_ne!(encoded[id].selected, whole.as_bytes());
    }
    assert!(encoded["t_empty"].selected.is_empty());
}

#[test]
fn invalid_fixture_records_are_refused_in_contract_order() {
    let contracts = fixture("construction-contracts.json");
    let fixtures = fixture("source-identity-fixtures.json");
    assert_eq!(
        str_list(&fixtures["refusal_precedence"]),
        REFUSALS.map(|(_, name)| name)
    );
    let invalid = fixtures["invalid_records"]
        .as_array()
        .expect("invalid records");
    let mut reasons = BTreeSet::new();
    let mut precedence_cases = 0;
    for record in invalid {
        let id = record["id"].as_str().expect("id");
        let expected = record["expected_refusal"]
            .as_str()
            .expect("expected refusal");
        match validate(&contracts, record) {
            Ok(_) => panic!("{id} was accepted; expected {expected}"),
            Err(refusal) => {
                assert_eq!(refusal.name(), expected, "{id} refused for another reason");
                reasons.insert(refusal.name());
            }
        }
        if id.starts_with("precedence_") {
            precedence_cases += 1;
        }
    }
    for (_, name) in REFUSALS {
        assert!(reasons.contains(name), "no invalid record covers {name}");
    }
    assert!(
        precedence_cases >= 4,
        "multi-fault records pin the refusal order"
    );
}
