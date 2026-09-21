mod support;

use std::collections::{BTreeMap, BTreeSet};

use context_core::canonical_json::{ContractError, canonical_json_encode};
use eval_core::{
    ArmRates, Attestation, BinaryDigest, CLOCK_FIELD_KEEP_ALLOWLIST, ClaimBoundary, DROPPED_FIELDS,
    IdentityError, MANIFEST_DIGEST_PROTOCOL, MANIFEST_SCHEMA, Manifest, ManifestError,
    ObservationSchema, REQUIRED_FIELDS, RUN_ID_PROTOCOL, ResidueEntry, ResidueError, Rule,
    RunIdentity, SemanticTrace, eval_run_id, is_canonical_decimal, is_clock_named, is_never_kept,
    parse_manifest, zero_bytes_sha256,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{OBSERVATION_TYPE, build, identity, manifest, observation, observation_schema};

/// Frozen so a field-set or encoding change forces a reviewed schema bump.
const FIXTURE_RUN_ID: &str = "e9f412ed2ad627c5801959c2c459bbb764bf45443a7774d02ac74a97f41832c9";
const FIXTURE_MANIFEST_DIGEST: &str =
    "03e2111f6eab3a9766d42e2f68fa3d0ad68b48a492f096c555e4f2a1117ea144";

#[test]
fn required_fields_are_sorted_and_equal_the_struct_field_set() {
    let mut sorted = REQUIRED_FIELDS.to_vec();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        REQUIRED_FIELDS.to_vec(),
        "REQUIRED_FIELDS is sorted"
    );
    let struct_fields: BTreeSet<String> = manifest()
        .to_value()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let pinned: BTreeSet<String> = REQUIRED_FIELDS.iter().map(|f| f.to_string()).collect();
    assert_eq!(struct_fields, pinned);
    assert!(DROPPED_FIELDS.iter().all(|f| REQUIRED_FIELDS.contains(f)));
    assert_eq!(
        Manifest::field_schema()
            .rules()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        pinned
    );
}

#[test]
fn fixture_digests_are_frozen() {
    assert_eq!(eval_run_id(&identity()).unwrap(), FIXTURE_RUN_ID);
    assert_eq!(manifest().digest().unwrap(), FIXTURE_MANIFEST_DIGEST);
}

/// `docs/evaluator.md` is a test input: its schema literal, digest protocol,
/// field count, and version history must match the manifest constants.
#[test]
fn evaluator_document_agrees_with_the_manifest_constants() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evaluator.md");
    let doc = std::fs::read_to_string(&path).expect("read docs/evaluator.md");
    let row = |needle: &str| {
        assert!(doc.contains(needle), "evaluator document lacks `{needle}`");
    };
    row(&format!("## Manifest `{MANIFEST_SCHEMA}`"));
    row(&format!("| `schema` | `{MANIFEST_SCHEMA}`. |"));
    row(&format!(
        "hashes with protocol `{MANIFEST_DIGEST_PROTOCOL}`"
    ));
    row(&format!(
        "The {} required fields, sorted:",
        REQUIRED_FIELDS.len()
    ));
    let version = MANIFEST_SCHEMA
        .rsplit_once("/v")
        .map(|(_, version)| version)
        .expect("schema literal ends in a version");
    assert_eq!(
        MANIFEST_DIGEST_PROTOCOL.rsplit_once("/v").map(|(_, v)| v),
        Some(version),
        "schema and digest protocol share one version"
    );
    // Every `eval-manifest*` literal in the document names the current version.
    for (offset, _) in doc.match_indices("`eval-manifest") {
        let literal = doc[offset + 1..]
            .split('`')
            .next()
            .expect("a backtick opens a literal");
        let stated = literal
            .rsplit_once("/v")
            .map(|(_, version)| version)
            .unwrap_or_else(|| panic!("`{literal}` names no version"));
        assert_eq!(stated, version, "stale manifest literal `{literal}`");
    }
    row(&format!("version {version} added `recency_baseline`"));
}

#[test]
fn a_valid_manifest_parses_and_round_trips() {
    let manifest = manifest();
    let value = manifest.to_value();
    let parsed = parse_manifest(&value).unwrap();
    assert_eq!(parsed, manifest);
    assert_eq!(parsed.to_value(), value);
    assert_eq!(value["attestation"], json!({"kind": "none"}));
}

#[test]
fn every_missing_field_is_refused_by_name_before_digesting() {
    let valid = manifest().to_value();
    for field in REQUIRED_FIELDS {
        let mut mutated = valid.clone();
        mutated.as_object_mut().unwrap().remove(field);
        assert_eq!(
            parse_manifest(&mutated),
            Err(ManifestError::MissingField(field.to_string()))
        );
    }
}

#[test]
fn unknown_field_wrong_schema_and_non_object_are_refused() {
    let valid = manifest().to_value();
    let mut extra = valid.clone();
    extra["extra"] = json!(1);
    assert_eq!(
        parse_manifest(&extra),
        Err(ManifestError::UnknownField("extra".to_string()))
    );
    let mut v7 = valid.clone();
    v7["schema"] = json!("eval-manifest/v8");
    assert_eq!(
        parse_manifest(&v7),
        Err(ManifestError::SchemaMismatch {
            found: "eval-manifest/v8".to_string()
        })
    );
    assert_eq!(parse_manifest(&json!([])), Err(ManifestError::NotAnObject));
    assert_eq!(MANIFEST_SCHEMA, "eval-manifest/v7");
}

#[test]
fn every_kept_field_enters_the_digest_and_every_dropped_field_leaves_it() {
    let base = manifest();
    let mut restamped = base.clone();
    restamped.start_ms += 86_400_000;
    restamped.end_ms += 86_400_000;
    restamped.envelope_peaks.elapsed_ms *= 3;
    restamped.envelope_peaks.processes += 1;
    assert_eq!(base.digest().unwrap(), restamped.digest().unwrap());

    type Mutation = Box<dyn Fn(&mut Manifest)>;
    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "arm_rates",
            Box::new(|m| {
                m.arm_rates.get_mut("fresh").unwrap().miss_rate = "0.5".to_string();
            }),
        ),
        (
            "attestation",
            Box::new(|m| {
                m.attestation = Attestation::Signed {
                    signer: "s".to_string(),
                    signature_digest: "9a".repeat(32),
                };
            }),
        ),
        (
            "component_versions",
            Box::new(|m| m.component_versions.judge = "judge-2".to_string()),
        ),
        (
            "construction",
            Box::new(|m| m.construction = eval_core::Construction::Replay),
        ),
        ("cut_receipts", Box::new(|m| m.cut_receipts.clear())),
        (
            "envelope_bounds",
            Box::new(|m| m.envelope_bounds.processes += 1),
        ),
        ("error", Box::new(|m| m.error = Some("typed".to_string()))),
        (
            "eval_run_id",
            Box::new(|m| {
                m.run_identity.root_seed += 1;
                m.eval_run_id = eval_run_id(&m.run_identity).unwrap();
            }),
        ),
        (
            "execution_mode",
            Box::new(|m| m.execution_mode = eval_core::ExecutionMode::Enumerate),
        ),
        (
            "ingestion",
            Box::new(|m| m.ingestion = eval_core::Ingestion::DirectDatabaseNonAged),
        ),
        (
            "memory_reviewer_model_calls",
            Box::new(|m| {
                m.memory_reviewer_model_calls = eval_core::MemoryReviewerModelCalls::Cassette
            }),
        ),
        (
            "analysis_family_digest",
            Box::new(|m| m.analysis_family_digest = Some("ab".repeat(32))),
        ),
        (
            "recency_baseline",
            Box::new(|m| {
                m.recency_baseline = Some(eval_core::RecencyBaseline {
                    version: eval_core::RECENCY_BASELINE_VERSION.to_string(),
                    bounds: BTreeMap::from([(eval_core::EvaluatedSurface::Surface1, 100)]),
                })
            }),
        ),
        (
            "reachability",
            Box::new(|m| m.reachability = eval_core::Reachability::TestOnly),
        ),
        (
            "residue",
            Box::new(|m| {
                m.residue.insert(ResidueEntry {
                    type_name: "extra".to_string(),
                    field: "field".to_string(),
                    rule: Rule::Drop,
                });
            }),
        ),
        (
            "result_digest",
            Box::new(|m| m.result_digest = "13".repeat(32)),
        ),
        (
            "retry_lineage",
            Box::new(|m| m.retry_lineage.push("00".repeat(32))),
        ),
        (
            "run_identity",
            Box::new(|m| {
                m.run_identity.simulator_version = "sim-2".to_string();
                m.eval_run_id = eval_run_id(&m.run_identity).unwrap();
            }),
        ),
        ("sample_epoch", Box::new(|m| m.sample_epoch += 1)),
        (
            "sample_ids",
            Box::new(|m| {
                m.sample_ids.push("pair-3".to_string());
                m.sample_order.push("pair-3".to_string());
            }),
        ),
        ("sample_order", Box::new(|m| m.sample_order.swap(0, 1))),
        (
            "status",
            Box::new(|m| m.status = eval_core::RunStatus::Refused),
        ),
        (
            "tokenizer_profile",
            Box::new(|m| m.tokenizer_profile.revision = "r2".to_string()),
        ),
        (
            "witness_digest",
            Box::new(|m| m.witness_digest = "35".repeat(32)),
        ),
    ];
    let mut digests = BTreeSet::from([base.digest().unwrap()]);
    let mut covered = BTreeSet::from(["schema", "claim_boundary", "failure_class_table_digest"]);
    for (field, mutate) in mutations {
        let mut mutated = base.clone();
        mutate(&mut mutated);
        assert!(
            digests.insert(mutated.digest().unwrap()),
            "{field} left the digest unchanged"
        );
        covered.insert(field);
    }
    let kept: BTreeSet<&str> = REQUIRED_FIELDS
        .into_iter()
        .filter(|field| !DROPPED_FIELDS.contains(field))
        .collect();
    assert_eq!(covered, kept, "every kept field has a digest witness");
    assert!(
        DROPPED_FIELDS
            .iter()
            .all(|f| is_clock_named(f) || *f == "envelope_peaks")
    );
}

#[test]
fn fractions_travel_as_canonical_decimal_strings() {
    for accepted in ["0", "12", "0.25", "1.5", "100"] {
        assert!(is_canonical_decimal(accepted), "{accepted}");
    }
    for refused in [
        "", ".5", "5.", "1/2", "-0.5", "1e3", "0.5.1", "0,5", "007", "0.250", "1.000",
    ] {
        assert!(!is_canonical_decimal(refused), "{refused}");
    }
    let mut manifest = manifest();
    manifest.arm_rates.insert(
        "aged".to_string(),
        ArmRates {
            miss_rate: "0.250".to_string(),
            refusal_rate: "0".to_string(),
        },
    );
    assert_eq!(
        parse_manifest(&manifest.to_value()),
        Err(ManifestError::MalformedDecimal {
            field: "arm_rates[aged].miss_rate".to_string(),
            value: "0.250".to_string(),
        })
    );
    let mut fractional = identity();
    fractional.config = json!({"temperature": 0.7});
    assert!(matches!(
        eval_run_id(&fractional),
        Err(IdentityError::NotCanonical(ContractError::NotCanonical(_)))
    ));
    let mut seed = serde_json::to_value(identity()).unwrap();
    seed["root_seed"] = json!("007");
    assert!(serde_json::from_value::<RunIdentity>(seed).is_err());
}

#[test]
fn arm_rates_stay_within_the_unit_interval() {
    for (rate, ok) in [
        ("0", true),
        ("1", true),
        ("0.25", true),
        ("2", false),
        ("1.5", false),
    ] {
        let mut manifest = manifest();
        manifest.arm_rates.insert(
            "aged".to_string(),
            ArmRates {
                miss_rate: rate.to_string(),
                refusal_rate: "0".to_string(),
            },
        );
        let expected = if ok {
            Ok(())
        } else {
            Err(ManifestError::RateOutOfRange {
                field: "arm_rates[aged].miss_rate".to_string(),
                value: rate.to_string(),
            })
        };
        assert_eq!(
            parse_manifest(&manifest.to_value()).map(drop),
            expected,
            "{rate}"
        );
    }
}

#[test]
fn provenance_strings_are_non_empty() {
    for (group, field) in [
        ("component_versions", "event_schema"),
        ("component_versions", "reducer"),
        ("component_versions", "oracles"),
        ("component_versions", "execution_image"),
        ("component_versions", "task_corpus"),
        ("component_versions", "judge"),
        ("tokenizer_profile", "name"),
        ("tokenizer_profile", "revision"),
    ] {
        let mut value = manifest().to_value();
        value[group][field] = json!("");
        assert_eq!(
            parse_manifest(&value).map(drop),
            Err(ManifestError::EmptyComponent {
                field: format!("{group}.{field}")
            })
        );
    }
}

#[test]
fn run_id_is_the_protocol_digest_of_the_full_tuple() {
    let identity = identity();
    let mut tuple = serde_json::to_value(&identity).unwrap();
    tuple["build"] = Value::String(identity.build.digest().unwrap());
    let mut hasher = Sha256::new();
    hasher.update(RUN_ID_PROTOCOL.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical_json_encode(&tuple).unwrap().as_bytes());
    assert_eq!(
        eval_run_id(&identity).unwrap(),
        format!("{:x}", hasher.finalize())
    );
    assert_eq!(RUN_ID_PROTOCOL, "eval-run-id/v1");
    assert_eq!(tuple.as_object().unwrap().len(), 9);
}

#[test]
fn changing_any_identity_or_build_component_changes_the_run_id() {
    let base = eval_run_id(&identity()).unwrap();
    type Mutation = Box<dyn Fn(&mut RunIdentity)>;
    let mutations: Vec<(&str, Mutation)> = vec![
        (
            "build.code_sha",
            Box::new(|i| i.build.code_sha = "4f".repeat(20)),
        ),
        ("build.dirty", Box::new(|i| i.build.dirty = true)),
        (
            "build.lockfile_digest",
            Box::new(|i| i.build.lockfile_digest = "ac".repeat(32)),
        ),
        (
            "build.rustc_version",
            Box::new(|i| i.build.rustc_version = "rustc 1.99.0".to_string()),
        ),
        ("build.features", Box::new(|i| i.build.features.clear())),
        (
            "build.target_triple",
            Box::new(|i| i.build.target_triple = "aarch64-unknown-linux-gnu".to_string()),
        ),
        (
            "build.binary_digest",
            Box::new(|i| {
                i.build.binary_digest = BinaryDigest::Absent {
                    reason: "composed in cargo test".to_string(),
                }
            }),
        ),
        (
            "simulator_version",
            Box::new(|i| i.simulator_version = "sim-2".to_string()),
        ),
        (
            "config",
            Box::new(|i| i.config = json!({"auto_search": false})),
        ),
        (
            "scenario",
            Box::new(|i| i.scenario = json!({"world": "other"})),
        ),
        ("root_seed", Box::new(|i| i.root_seed += 1)),
        (
            "random_schema_version",
            Box::new(|i| i.random_schema_version = "rng-2".to_string()),
        ),
        (
            "generator_version",
            Box::new(|i| i.generator_version = "gen-2".to_string()),
        ),
        (
            "eligibility_spec_digest",
            Box::new(|i| i.eligibility_spec_digest = "ee".repeat(32)),
        ),
        (
            "linearization_rule_version",
            Box::new(|i| i.linearization_rule_version = "lin-2".to_string()),
        ),
    ];
    let mut seen = BTreeSet::from([base.clone()]);
    for (name, mutate) in mutations {
        let mut mutated = identity();
        mutate(&mut mutated);
        let id = eval_run_id(&mutated).unwrap();
        assert!(seen.insert(id), "{name} did not change the run id");
    }
}

#[test]
fn identity_validate_refuses_what_the_run_id_refuses() {
    let mut fractional = identity();
    fractional.config = json!({"threshold": 0.7});
    assert!(matches!(
        eval_run_id(&fractional),
        Err(IdentityError::NotCanonical(_))
    ));
    assert!(matches!(
        fractional.validate(),
        Err(IdentityError::NotCanonical(_))
    ));
    let mut unexplained = build();
    unexplained.binary_digest = BinaryDigest::Absent {
        reason: String::new(),
    };
    assert_eq!(
        unexplained.validate(),
        Err(IdentityError::EmptyComponent {
            field: "binary_digest.reason"
        })
    );
    let mut dirty_unidentified = build();
    dirty_unidentified.dirty = true;
    dirty_unidentified.binary_digest = BinaryDigest::Absent {
        reason: "composed in cargo test".to_string(),
    };
    assert_eq!(
        dirty_unidentified.validate(),
        Err(IdentityError::DirtyBuildWithoutBinaryDigest)
    );
    dirty_unidentified.binary_digest = build().binary_digest;
    assert!(dirty_unidentified.validate().is_ok());
}

#[test]
fn malformed_or_empty_identity_components_are_refused() {
    let mut build = build();
    build.binary_digest = BinaryDigest::Present {
        sha256: zero_bytes_sha256(),
    };
    assert_eq!(build.digest(), Err(IdentityError::ZeroBytesBinaryDigest));
    let mut identity_with_zero = identity();
    identity_with_zero.build = build;
    assert_eq!(
        eval_run_id(&identity_with_zero),
        Err(IdentityError::ZeroBytesBinaryDigest)
    );
    let mut malformed = support::build();
    malformed.binary_digest = BinaryDigest::Present {
        sha256: "CD".repeat(32),
    };
    assert_eq!(
        malformed.digest(),
        Err(IdentityError::MalformedDigest {
            field: "binary_digest"
        })
    );
    let mut short_sha = support::build();
    short_sha.code_sha = "abc".to_string();
    assert_eq!(
        short_sha.digest(),
        Err(IdentityError::MalformedDigest { field: "code_sha" })
    );
    let mut spec = identity();
    spec.eligibility_spec_digest = "not-hex".to_string();
    assert_eq!(
        eval_run_id(&spec),
        Err(IdentityError::MalformedDigest {
            field: "eligibility_spec_digest"
        })
    );
    let mut empty = identity();
    empty.generator_version.clear();
    assert_eq!(
        eval_run_id(&empty),
        Err(IdentityError::EmptyComponent {
            field: "generator_version"
        })
    );
}

#[test]
fn manifest_consistency_refusals_name_their_cause() {
    let mut aged_direct = manifest();
    aged_direct.ingestion = eval_core::Ingestion::DirectDatabaseNonAged;
    aged_direct.construction = eval_core::Construction::Replay;
    assert_eq!(
        aged_direct.validate(),
        Err(ManifestError::DirectDatabaseAged)
    );
    assert_eq!(
        manifest().to_value()["ingestion"],
        json!("adapter-ingested, production caller: none")
    );
    let mut wrong_id = manifest();
    wrong_id.eval_run_id = "00".repeat(32);
    assert!(matches!(
        parse_manifest(&wrong_id.to_value()),
        Err(ManifestError::RunIdMismatch { .. })
    ));
    let mut generator = manifest();
    generator.component_versions.generator = "gen-9".to_string();
    assert_eq!(
        parse_manifest(&generator.to_value()),
        Err(ManifestError::GeneratorVersionMismatch)
    );
    let mut boundary = manifest();
    boundary.claim_boundary = ClaimBoundary {
        schema: "claim-boundary/v1".to_string(),
        exclusions: vec!["live-model quality".to_string()],
    };
    assert_eq!(
        parse_manifest(&boundary.to_value()),
        Err(ManifestError::ClaimBoundaryMismatch)
    );
    let mut residue = manifest();
    residue.residue.retain(|entry| entry.field != "start_ms");
    assert_eq!(
        parse_manifest(&residue.to_value()),
        Err(ManifestError::ResidueIncomplete {
            field: "start_ms".to_string()
        })
    );
    let mut order = manifest();
    order.sample_order.push("pair-1".to_string());
    assert_eq!(
        parse_manifest(&order.to_value()),
        Err(ManifestError::SampleOrderNotAPermutation)
    );
    let mut digest = manifest();
    digest.result_digest = "xyz".to_string();
    assert_eq!(
        parse_manifest(&digest.to_value()),
        Err(ManifestError::MalformedDigest {
            field: "result_digest".to_string()
        })
    );
    let mut lineage = manifest();
    lineage.retry_lineage = vec!["77".repeat(32), "not-a-run-id".to_string()];
    assert_eq!(
        parse_manifest(&lineage.to_value()),
        Err(ManifestError::MalformedDigest {
            field: "retry_lineage[1]".to_string()
        })
    );
    lineage.retry_lineage.pop();
    assert!(parse_manifest(&lineage.to_value()).is_ok());
    assert!(
        manifest().digest().is_ok() && wrong_id.digest().is_err(),
        "digest re-parses before hashing"
    );
}

#[test]
fn residue_declarations_are_non_keep_and_one_rule_per_field() {
    let kept = ResidueEntry {
        type_name: "attempt".to_string(),
        field: "pid".to_string(),
        rule: Rule::Keep,
    };
    let mut keep = manifest();
    keep.residue.insert(kept.clone());
    assert_eq!(
        parse_manifest(&keep.to_value()),
        Err(ManifestError::ResidueContradiction {
            type_name: "attempt".to_string(),
            field: "pid".to_string(),
        })
    );
    let mut twice = manifest();
    twice.residue.insert(ResidueEntry {
        type_name: OBSERVATION_TYPE.to_string(),
        field: "run_id".to_string(),
        rule: Rule::Presence,
    });
    assert_eq!(
        parse_manifest(&twice.to_value()),
        Err(ManifestError::ResidueContradiction {
            type_name: OBSERVATION_TYPE.to_string(),
            field: "run_id".to_string(),
        })
    );
}

#[test]
fn validate_refuses_what_parse_and_digest_refuse() {
    let mut schema = manifest();
    schema.schema = "eval-manifest/v8".to_string();
    assert_eq!(
        schema.validate(),
        Err(ManifestError::SchemaMismatch {
            found: "eval-manifest/v8".to_string()
        })
    );
    let mut table = manifest();
    table.failure_class_table_digest = "00".repeat(32);
    assert_eq!(
        table.validate(),
        Err(ManifestError::FailureClassTableMismatch {
            found: "00".repeat(32)
        })
    );
    let mut epoch = manifest();
    epoch.sample_epoch = 1 << 53;
    assert!(matches!(
        parse_manifest(&epoch.to_value()),
        Err(ManifestError::NotCanonical(_))
    ));
    assert!(matches!(
        epoch.validate(),
        Err(ManifestError::NotCanonical(_))
    ));
    let mut stamp = manifest();
    stamp.start_ms = i64::MAX;
    assert!(matches!(
        parse_manifest(&stamp.to_value()),
        Err(ManifestError::NotCanonical(_))
    ));
}

#[test]
fn a_recorded_recency_baseline_must_be_the_one_the_compiler_enforces() {
    use eval_core::{EvaluatedSurface, RECENCY_BASELINE_VERSION, RecencyBaseline};
    let record = |version: &str, bounds: &[(EvaluatedSurface, u32)]| {
        let mut m = manifest();
        m.recency_baseline = Some(RecencyBaseline {
            version: version.to_string(),
            bounds: bounds.iter().copied().collect(),
        });
        m
    };
    let good = record(
        RECENCY_BASELINE_VERSION,
        &[
            (EvaluatedSurface::Surface1, 100),
            (EvaluatedSurface::QueryRoute, 3),
        ],
    );
    good.validate().unwrap();
    parse_manifest(&good.to_value()).unwrap();
    for (name, bad, field) in [
        (
            "an empty version",
            record("", &[(EvaluatedSurface::Surface1, 100)]),
            "version",
        ),
        (
            "another version",
            record(
                "eval-recency-baseline/v0",
                &[(EvaluatedSurface::Surface1, 100)],
            ),
            "version",
        ),
        ("no bounds", record(RECENCY_BASELINE_VERSION, &[]), "bounds"),
        (
            "surface 1 off its pin",
            record(RECENCY_BASELINE_VERSION, &[(EvaluatedSurface::Surface1, 7)]),
            "bounds",
        ),
        (
            "a zero bound",
            record(RECENCY_BASELINE_VERSION, &[(EvaluatedSurface::Surface2, 0)]),
            "bounds",
        ),
    ] {
        assert_eq!(
            bad.validate(),
            Err(ManifestError::RecencyBaselineMismatch { field }),
            "{name}"
        );
        assert_eq!(
            parse_manifest(&bad.to_value()),
            Err(ManifestError::RecencyBaselineMismatch { field }),
            "{name}"
        );
    }
}

#[test]
fn attestation_is_a_tagged_value() {
    let mut signed = manifest();
    signed.attestation = Attestation::Signed {
        signer: "release-owner".to_string(),
        signature_digest: "9a".repeat(32),
    };
    let value = signed.to_value();
    assert_eq!(value["attestation"]["kind"], "signed");
    assert_eq!(value["attestation"]["signer"], "release-owner");
    assert_eq!(parse_manifest(&value).unwrap(), signed);
    assert_ne!(signed.digest().unwrap(), manifest().digest().unwrap());
    signed.attestation = Attestation::Signed {
        signer: "release-owner".to_string(),
        signature_digest: "short".to_string(),
    };
    assert_eq!(
        parse_manifest(&signed.to_value()),
        Err(ManifestError::MalformedDigest {
            field: "attestation.signature_digest".to_string()
        })
    );
    signed.attestation = Attestation::Signed {
        signer: String::new(),
        signature_digest: "9a".repeat(32),
    };
    assert_eq!(
        parse_manifest(&signed.to_value()),
        Err(ManifestError::EmptyComponent {
            field: "attestation.signer".to_string()
        })
    );
}

#[test]
fn residue_classification_is_total_over_observation_fields() {
    let mut trace = SemanticTrace::new([observation_schema()]).unwrap();
    let mut extra = observation(0, "inc-a", None);
    extra["attempt_started_at"] = json!(1);
    assert_eq!(
        trace.record(OBSERVATION_TYPE, &extra),
        Err(ResidueError::UnclassifiedField {
            type_name: OBSERVATION_TYPE.to_string(),
            field: "attempt_started_at".to_string(),
        })
    );
    let mut missing = observation(0, "inc-a", None);
    missing.as_object_mut().unwrap().remove("hint_text");
    assert_eq!(
        trace.record(OBSERVATION_TYPE, &missing),
        Err(ResidueError::MissingField {
            type_name: OBSERVATION_TYPE.to_string(),
            field: "hint_text".to_string(),
        })
    );
    assert_eq!(
        trace.record("unregistered", &json!({})),
        Err(ResidueError::UnknownType {
            type_name: "unregistered".to_string()
        })
    );
    assert_eq!(
        trace.record(OBSERVATION_TYPE, &json!(1)),
        Err(ResidueError::NotAnObject {
            type_name: OBSERVATION_TYPE.to_string()
        })
    );
    assert!(matches!(
        SemanticTrace::new([observation_schema(), observation_schema()]),
        Err(ResidueError::DuplicateType { .. })
    ));
    let expected: Vec<ResidueEntry> = [
        ("database_incarnation_id", Rule::Relative),
        ("decided_at_ms", Rule::Drop),
        ("hold_expires_at", Rule::Presence),
        ("run_id", Rule::Drop),
    ]
    .into_iter()
    .map(|(field, rule)| ResidueEntry {
        type_name: OBSERVATION_TYPE.to_string(),
        field: field.to_string(),
        rule,
    })
    .collect();
    assert_eq!(trace.residue(), expected);
}

#[test]
fn clock_named_keep_fields_equal_the_pinned_allowlist() {
    let schemas = [observation_schema(), Manifest::field_schema()];
    let kept_clock_fields: BTreeSet<&str> = schemas
        .iter()
        .flat_map(|schema| schema.rules().iter())
        .filter(|(field, rule)| **rule == Rule::Keep && is_clock_named(field))
        .map(|(field, _)| field.as_str())
        .collect();
    assert_eq!(
        kept_clock_fields,
        CLOCK_FIELD_KEEP_ALLOWLIST
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        ObservationSchema::new("leaky", [("decided_at_ms", Rule::Keep)]),
        Err(ResidueError::ClockFieldKept {
            type_name: "leaky".to_string(),
            field: "decided_at_ms".to_string(),
        })
    );
    assert!(ObservationSchema::new("dropped", [("decided_at_ms", Rule::Drop)]).is_ok());
    for word in [
        "retry_deadline",
        "wall_clock",
        "created_at",
        "created_at_ns",
        "updated_at_us",
        "elapsed_ms",
        "start_time",
    ] {
        assert!(is_clock_named(word), "{word}");
    }
    assert!(!is_clock_named("occurrence_id"));
}

#[test]
fn host_environment_and_incarnation_fields_are_never_kept() {
    for field in [
        "hostname",
        "host_name",
        "host_hostname",
        "cwd",
        "pid",
        "writer_pid",
        "process_id",
        "parent_process_id",
        "source_process_id_value",
        "ppid",
        "project_root",
        "artifact_path",
        "boot_id",
        "uid",
        "owner_uid",
        "euid",
        "gid",
        "database_incarnation_id",
    ] {
        assert!(is_never_kept(field), "{field}");
        assert_eq!(
            ObservationSchema::new("leaky", [(field, Rule::Keep)]),
            Err(ResidueError::HostFieldKept {
                type_name: "leaky".to_string(),
                field: field.to_string(),
            })
        );
        assert!(ObservationSchema::new("dropped", [(field, Rule::Drop)]).is_ok());
    }
    assert!(!is_never_kept("occurrence_id"));
    assert!(!is_never_kept("rapid_response"));
    // The gates match snake_case spellings, so any other spelling is refused
    // before the gates run.
    for field in [
        "hostName",
        "writerPid",
        "database-incarnation",
        "",
        "Now_ms",
        "process__id",
        "process_id_",
        "_pid",
    ] {
        assert_eq!(
            ObservationSchema::new("spelled", [(field, Rule::Drop)]),
            Err(ResidueError::FieldNotSnakeCase {
                type_name: "spelled".to_string(),
                field: field.to_string(),
            }),
            "{field:?}"
        );
    }
}

#[test]
fn a_field_declared_twice_is_refused_at_schema_construction() {
    assert_eq!(
        ObservationSchema::new("twice", [("pid", Rule::Keep), ("pid", Rule::Drop)]),
        Err(ResidueError::DuplicateField {
            type_name: "twice".to_string(),
            field: "pid".to_string(),
        })
    );
}

#[test]
fn presence_and_relative_rules_hide_incarnation_values_but_not_their_structure() {
    let run = |incarnations: [&str; 3], holds: [Option<i64>; 3]| {
        let mut trace = SemanticTrace::new([observation_schema()]).unwrap();
        for (sequence, (incarnation, hold)) in incarnations.into_iter().zip(holds).enumerate() {
            trace
                .record(
                    OBSERVATION_TYPE,
                    &observation(sequence as u64, incarnation, hold),
                )
                .unwrap();
        }
        trace.digest().unwrap()
    };
    let base = run(["a", "a", "b"], [Some(1), Some(2), None]);
    assert_eq!(base, run(["x", "x", "y"], [Some(9), Some(8), None]));
    assert_ne!(
        base,
        run(["x", "y", "y"], [Some(1), Some(2), None]),
        "merged ids differ"
    );
    assert_ne!(
        base,
        run(["a", "a", "b"], [Some(1), None, None]),
        "presence differs"
    );
}

#[test]
fn a_refused_observation_leaves_relative_numbering_unchanged() {
    let schema =
        || ObservationSchema::new("pair", [("a", Rule::Relative), ("b", Rule::Relative)]).unwrap();
    let mut refused = SemanticTrace::new([schema()]).unwrap();
    assert!(matches!(
        refused.record("pair", &json!({"a": "x", "b": 0.5})),
        Err(ResidueError::NotCanonical(_))
    ));
    refused
        .record("pair", &json!({"a": "y", "b": "ok"}))
        .unwrap();
    let mut clean = SemanticTrace::new([schema()]).unwrap();
    clean.record("pair", &json!({"a": "y", "b": "ok"})).unwrap();
    assert_eq!(refused.digest().unwrap(), clean.digest().unwrap());
}

#[test]
fn a_kept_value_the_digest_cannot_encode_is_refused_at_record_time() {
    let schema = || {
        ObservationSchema::new("scored", [("score", Rule::Keep), ("id", Rule::Relative)]).unwrap()
    };
    let mut trace = SemanticTrace::new([schema()]).unwrap();
    assert!(matches!(
        trace.record("scored", &json!({"score": 0.5, "id": "x"})),
        Err(ResidueError::NotCanonical(_))
    ));
    trace
        .record("scored", &json!({"score": 1, "id": "y"}))
        .unwrap();
    let mut clean = SemanticTrace::new([schema()]).unwrap();
    clean
        .record("scored", &json!({"score": 1, "id": "y"}))
        .unwrap();
    assert_eq!(trace.digest().unwrap(), clean.digest().unwrap());
}

#[test]
fn dropped_fields_never_reach_the_trace_digest() {
    let mut a = SemanticTrace::new([observation_schema()]).unwrap();
    let mut b = SemanticTrace::new([observation_schema()]).unwrap();
    let mut late = observation(0, "inc", None);
    late["decided_at_ms"] = json!(1);
    late["run_id"] = json!("model_execution-9-9");
    a.record(OBSERVATION_TYPE, &observation(0, "inc", None))
        .unwrap();
    b.record(OBSERVATION_TYPE, &late).unwrap();
    assert_eq!(a.digest().unwrap(), b.digest().unwrap());
    let mut c = SemanticTrace::new([observation_schema()]).unwrap();
    let mut changed = observation(0, "inc", None);
    changed["hint_text"] = json!("other");
    c.record(OBSERVATION_TYPE, &changed).unwrap();
    assert_ne!(a.digest().unwrap(), c.digest().unwrap());
}

/// The reducer differential runs in `enumerate` mode; a manifest records that
/// mode and the pinned spec digest, and the mode enters the digest.
#[test]
fn an_enumerate_run_records_its_mode_and_the_pinned_spec_digest() {
    let mut enumerate = manifest();
    enumerate.execution_mode = eval_core::ExecutionMode::Enumerate;
    enumerate.run_identity.eligibility_spec_digest = eval_core::ELIGIBILITY_SPEC_DIGEST.to_string();
    enumerate.eval_run_id = eval_run_id(&enumerate.run_identity).unwrap();
    let parsed = parse_manifest(&enumerate.to_value()).unwrap();
    assert_eq!(parsed.execution_mode, eval_core::ExecutionMode::Enumerate);
    assert_eq!(
        parsed.run_identity.eligibility_spec_digest,
        eval_core::ELIGIBILITY_SPEC_DIGEST
    );
    assert_eq!(
        enumerate.to_value()["execution_mode"],
        serde_json::json!("enumerate")
    );
    for mode in [
        eval_core::ExecutionMode::Generate,
        eval_core::ExecutionMode::ReplayTape,
    ] {
        let mut other = enumerate.clone();
        other.execution_mode = mode;
        assert_ne!(other.digest().unwrap(), enumerate.digest().unwrap());
    }
}
