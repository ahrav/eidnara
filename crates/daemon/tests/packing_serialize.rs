mod support;

use std::collections::BTreeSet;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use daemon::dispatch::{MAX_WIRE_BODY_BYTES, guard_calls};
use daemon::packing::{
    AccountingBound, AccountingBounds, AccountingExceeded, AccountingProfile, Charged,
    ClaudeTokens, OptionalAdmission, OptionalRequest, PackingFailure, PackingLimitRefusal,
    PackingLimits, PackingTrace, Preparation, PreparationRefusal, RequiredInputs,
    SerializationBound, SerializationBounds, cost_cache, finalize, prepare_optional,
    prepare_required,
};
use daemon::projection_gates::{
    ManifestRefusal, PACKING_APPROVAL_ID, PACKING_LIMITS, REQUIRED_LIMITS, RuntimeManifest,
};
use kernel::applicability::EvalBudget;
use retrieval::packing::{OptionalBounds, RequiredBounds};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use support::packing::{Fixture, ToolSpan, accounting_bounds, bounds, byte_profile, tool_span};

const REQUIRED: ToolSpan = tool_span("req", "1", "required bytes stay\n");
const GROUPS: [ToolSpan; 3] = [
    tool_span("opt-a", "1", "first optional group\n"),
    tool_span("opt-b", "1", "second optional group, a bit longer\n"),
    tool_span("opt-c", "1", "third\n"),
];

fn wide() -> OptionalBounds {
    OptionalBounds {
        max_fused_candidates: NonZeroUsize::new(16).unwrap(),
        max_parents: NonZeroUsize::new(16).unwrap(),
        max_spans_per_parent: NonZeroUsize::new(16).unwrap(),
        max_payload_loads: NonZeroUsize::new(16).unwrap(),
        max_payload_bytes: NonZeroU64::new(1 << 20).unwrap(),
        max_item_bytes: NonZeroU64::new(1 << 19).unwrap(),
    }
}

fn fixture() -> Fixture {
    Fixture::new(&[REQUIRED, GROUPS[0], GROUPS[1], GROUPS[2]])
}

fn admit(fixture: &Fixture, spans: &[ToolSpan]) -> OptionalAdmission {
    admit_under(fixture, spans, &byte_profile(), &accounting_bounds()).unwrap()
}

fn admit_under(
    fixture: &Fixture,
    spans: &[ToolSpan],
    profile: &AccountingProfile,
    accounting: &AccountingBounds,
) -> Result<OptionalAdmission, PreparationRefusal> {
    let mut trace = PackingTrace::default();
    let budget = EvalBudget::unbounded();
    let inputs = RequiredInputs {
        kernel: &fixture.kernel,
        project: &fixture.project,
        destination: kernel::ArtifactDestination::Local,
        budget: &budget,
        profile,
    };
    let required = prepare_required(
        &fixture.store,
        inputs,
        &[REQUIRED.request()],
        &bounds(1 << 20),
        accounting,
        &mut trace,
    )
    .unwrap();
    let requests: Vec<OptionalRequest> = spans
        .iter()
        .map(|span| OptionalRequest {
            occurrence: span.id(),
            revision: 1,
        })
        .collect();
    prepare_optional(
        &fixture.store,
        inputs,
        &required,
        &requests,
        &wide(),
        accounting,
        &mut trace,
    )
}

fn serialization(max_serialized_bytes: usize, max_adjustment_passes: usize) -> SerializationBounds {
    SerializationBounds {
        max_serialized_bytes,
        max_adjustment_passes,
    }
}

fn prepare(
    fixture: &Fixture,
    serialized_bytes: usize,
    pass_cap: usize,
) -> Result<Preparation, PackingFailure> {
    finalize(
        admit(fixture, &GROUPS),
        &serialization(serialized_bytes, pass_cap),
        &EvalBudget::unbounded(),
    )
}

fn full_len(fixture: &Fixture) -> usize {
    prepare(fixture, 1 << 20, 8).unwrap().body().len()
}

fn text(preparation: &Preparation) -> &str {
    std::str::from_utf8(preparation.body()).unwrap()
}

#[test]
fn the_body_is_measured_once_reserved_exactly_and_written_through_the_guard() {
    let fixture = fixture();
    for profile in [byte_profile(), AccountingProfile::exact_tokenizer()] {
        let admission = admit_under(&fixture, &GROUPS, &profile, &accounting_bounds()).unwrap();
        let rendered = admission.ledger().rendered_bytes();
        let expected = admission.ledger().text().to_owned();
        guard_calls::reset();
        let preparation = finalize(
            admission,
            &serialization(rendered, 8),
            &EvalBudget::unbounded(),
        )
        .unwrap();
        assert_eq!(guard_calls::counts(), (1, 1), "{}", profile.identity());
        assert_eq!(preparation.body().len(), rendered);
        assert_eq!(preparation.body(), expected.as_bytes());
        assert_eq!(preparation.passes(), 0);
        assert!(preparation.removed().is_empty());
        assert_eq!(preparation.admitted().len(), 3);
        assert_eq!(
            *preparation.identity().as_bytes(),
            <[u8; 32]>::from(Sha256::digest(preparation.body()))
        );
        assert!(text(&preparation).contains("required bytes stay"));
        for group in GROUPS {
            assert!(text(&preparation).contains(group.payload.trim_end()));
        }
    }
}

#[test]
fn a_wrapper_overflow_removes_the_last_admitted_group_and_reclaims_its_wrappers() {
    let fixture = fixture();
    let full = prepare(&fixture, 1 << 20, 8).unwrap();
    let full_len = full.body().len();
    let last = full.admitted().last().unwrap().clone();

    guard_calls::reset();
    let repaired = prepare(&fixture, full_len - 1, 8).unwrap();
    assert_eq!(guard_calls::counts(), (2, 1));
    assert_eq!(repaired.passes(), 1);
    assert_eq!(repaired.removed().len(), 1);
    assert_eq!(repaired.removed()[0].group.first_fused, 2);
    assert_eq!(repaired.removed()[0].index, 2);
    assert_eq!(repaired.admitted().len(), 2);
    assert!(repaired.body().len() < full_len);
    assert!(text(&repaired).contains("required bytes stay"));
    assert!(!text(&repaired).contains("third"));
    let twin = admit(&fixture, &GROUPS[..2]);
    assert_eq!(repaired.body(), twin.ledger().text().as_bytes());
    assert_eq!(repaired.ledger(), twin.ledger());
    assert_eq!(
        repaired.ledger().total_with_headroom(),
        ClaudeTokens::new(full.ledger().total_with_headroom().get() - last.cost.get())
    );
    assert_eq!(prepare(&fixture, full_len - 1, 1).unwrap().passes(), 1);
    let wrappers = repaired
        .ledger()
        .entries()
        .iter()
        .filter(|entry| matches!(entry.item, Charged::GroupOpen(_) | Charged::GroupClose(_)))
        .count();
    assert_eq!(wrappers, 4);
    assert_eq!(
        repaired.ledger().total_with_headroom(),
        ClaudeTokens::new(repaired.body().len() as u64)
    );
}

#[test]
fn cap_exhaustion_emits_nothing_and_never_removes_required_items() {
    let fixture = fixture();
    let full_len = full_len(&fixture);
    assert_eq!(
        prepare(&fixture, full_len - 1, 0).unwrap_err(),
        PackingFailure::AdjustmentCapExhausted {
            passes: 0,
            bound: SerializationBound::SerializedBytes,
        }
    );

    let required_only = admit(&fixture, &[]);
    let required_len = required_only.ledger().rendered_bytes();
    let stripped = prepare(&fixture, required_len, 16).unwrap();
    assert_eq!(stripped.passes(), 3);
    assert!(stripped.admitted().is_empty());
    assert_eq!(stripped.removed().len(), 3);
    assert_eq!(
        stripped
            .removed()
            .iter()
            .map(|group| group.index)
            .collect::<Vec<_>>(),
        [2, 1, 0]
    );
    assert_eq!(stripped.body(), required_only.ledger().text().as_bytes());
    assert!(text(&stripped).contains("required bytes stay"));

    guard_calls::reset();
    assert_eq!(
        prepare(&fixture, required_len - 1, 16).unwrap_err(),
        PackingFailure::AdjustmentCapExhausted {
            passes: 3,
            bound: SerializationBound::SerializedBytes,
        }
    );
    assert_eq!(guard_calls::counts(), (4, 0));

    assert!(matches!(
        prepare(&fixture, required_len - 1, 2),
        Err(PackingFailure::AdjustmentCapExhausted { passes: 2, .. })
    ));
}

#[test]
fn an_exhausted_budget_refuses_before_any_measurement() {
    let fixture = fixture();
    let admission = admit(&fixture, &GROUPS);
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    guard_calls::reset();
    assert_eq!(
        finalize(admission, &serialization(1 << 20, 8), &cancelled).unwrap_err(),
        PackingFailure::Deadline
    );
    assert_eq!(guard_calls::counts(), (0, 0));
}

#[test]
fn an_accounting_overflow_is_refused_by_the_optional_phase_before_any_measurement() {
    let fixture = fixture();
    let admission = admit(&fixture, &GROUPS);
    let bytes = admission.ledger().rendered_bytes();
    let tokens = admission.ledger().total_with_headroom().get();
    let at_value = AccountingBounds {
        max_rendered_bytes: bytes,
        max_estimated_tokens: ClaudeTokens::new(tokens),
    };
    for (accounting, exceeded) in [
        (
            AccountingBounds {
                max_rendered_bytes: bytes - 1,
                ..at_value
            },
            AccountingExceeded {
                bound: AccountingBound::RenderedBytes,
                value: bytes as u64,
                limit: (bytes - 1) as u64,
            },
        ),
        (
            AccountingBounds {
                max_estimated_tokens: ClaudeTokens::new(tokens - 1),
                ..at_value
            },
            AccountingExceeded {
                bound: AccountingBound::EstimatedTokens,
                value: tokens,
                limit: tokens - 1,
            },
        ),
    ] {
        guard_calls::reset();
        assert_eq!(
            admit_under(&fixture, &GROUPS, &byte_profile(), &accounting).unwrap_err(),
            PreparationRefusal::Accounting(exceeded)
        );
        assert_eq!(guard_calls::counts(), (0, 0), "{exceeded:?}");
    }

    let admission = admit_under(&fixture, &GROUPS, &byte_profile(), &at_value).unwrap();
    let saturated = finalize(
        admission,
        &serialization(bytes, 0),
        &EvalBudget::unbounded(),
    )
    .unwrap();
    assert_eq!(saturated.passes(), 0);
    assert_eq!(saturated.body().len(), bytes);
}

fn identity_hex(fixture: &Fixture, serialized_bytes: usize) -> String {
    prepare(fixture, serialized_bytes, 8)
        .unwrap()
        .identity()
        .to_string()
}

fn refusal_at_token_edge(fixture: &Fixture, max_estimated_tokens: u64) -> PreparationRefusal {
    let tight = AccountingBounds {
        max_rendered_bytes: 1 << 20,
        max_estimated_tokens: ClaudeTokens::new(max_estimated_tokens),
    };
    admit_under(fixture, &GROUPS, &byte_profile(), &tight).unwrap_err()
}

#[test]
fn identical_inputs_give_byte_identical_output_across_cache_states_threads_and_processes() {
    let fixture = fixture();
    let full_len = full_len(&fixture);
    let reference = identity_hex(&fixture, full_len);

    let admission = admit(&fixture, &GROUPS);
    let edge = admission.ledger().total_with_headroom().get() - 1;
    let edge_reference = refusal_at_token_edge(&fixture, edge);
    assert!(matches!(
        edge_reference,
        PreparationRefusal::Accounting(AccountingExceeded {
            bound: AccountingBound::EstimatedTokens,
            ..
        })
    ));

    cost_cache::clear();
    assert_eq!(identity_hex(&fixture, full_len), reference);
    assert_eq!(refusal_at_token_edge(&fixture, edge), edge_reference);
    assert_eq!(identity_hex(&fixture, full_len), reference);
    assert_eq!(refusal_at_token_edge(&fixture, edge), edge_reference);
    cost_cache::rotate();
    assert_eq!(identity_hex(&fixture, full_len), reference);
    assert_eq!(refusal_at_token_edge(&fixture, edge), edge_reference);

    let identities: BTreeSet<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| identity_hex(&fixture, full_len)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    assert_eq!(identities, BTreeSet::from([reference.clone()]));

    let full = prepare(&fixture, full_len, 8).unwrap();
    let adjusted = prepare(&fixture, full_len - 1, 8).unwrap();
    assert_ne!(adjusted.identity(), full.identity());
    let member_digest = |preparation: &Preparation| {
        let mut hasher = Sha256::new();
        for member in preparation
            .admitted()
            .iter()
            .chain(preparation.removed())
            .flat_map(|group| group.group.members())
        {
            hasher.update(member.to_string());
        }
        <[u8; 32]>::from(hasher.finalize())
    };
    assert_eq!(member_digest(&adjusted), member_digest(&full));

    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_process_prints_the_preparation_identity",
            "--nocapture",
            "--include-ignored",
        ])
        .env("EIDNARA_PACKING_CHILD_LEN", full_len.to_string())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let stdout = String::from_utf8(child.stdout).unwrap();
    let printed = stdout
        .lines()
        .find_map(|line| line.strip_prefix("identity="))
        .unwrap_or_else(|| panic!("no identity line in {stdout}"));
    assert_eq!(printed, reference);
}

#[test]
#[ignore = "run by the fresh-process check"]
fn child_process_prints_the_preparation_identity() {
    let Ok(len) = std::env::var("EIDNARA_PACKING_CHILD_LEN") else {
        return;
    };
    println!(
        "identity={}",
        identity_hex(&fixture(), len.parse().unwrap())
    );
}

const PROTOCOL: &str = "limits.v1";

fn manifest(limits: Map<String, Value>, hooks: Map<String, Value>) -> Value {
    json!({
        "protocol_version": PROTOCOL,
        "invalidation_identity": {
            "schema_version": retrieval::SCHEMA_VERSION,
            "tokenizer_fingerprint": support::projection_gate::FINGERPRINT,
            "analysis_identity": retrieval::lexical::AnalysisIdentity::current().as_str(),
            "embedding_model": support::projection_gate::MODEL,
            "projection_policy_version": support::projection_gate::POLICY,
            "identity_contract_version": support::projection_gate::CONTRACT,
            "limit_manifest_protocol_version": PROTOCOL,
            "vector_dimension": 8,
            "generation_epoch": 1,
        },
        "limits": limits,
        "hooks": hooks,
    })
}

fn packing_values() -> [(&'static str, u64); 11] {
    [
        ("packing_fused_candidates", 16),
        ("packing_payload_loads", 12),
        ("packing_payload_bytes", 4096),
        ("packing_item_bytes", 1024),
        ("packing_parents", 8),
        ("packing_spans_per_parent", 4),
        ("packing_rendered_bytes", 8192),
        ("packing_estimated_tokens", 2048),
        ("packing_serialized_bytes", 8000),
        ("packing_adjustment_passes", 3),
        ("packing_deadline_ms", 250),
    ]
}

fn required_limits() -> Map<String, Value> {
    REQUIRED_LIMITS
        .into_iter()
        .map(|name| (name.to_owned(), json!(1_000_000)))
        .collect()
}

fn limits_with_packing() -> Map<String, Value> {
    let mut limits = required_limits();
    for (name, value) in packing_values() {
        limits.insert(name.to_owned(), json!(value));
    }
    limits
}

fn approval(enabled: bool) -> Map<String, Value> {
    let mut hooks = Map::new();
    hooks.insert(
        PACKING_APPROVAL_ID.to_owned(),
        json!({ "enabled": enabled }),
    );
    hooks
}

fn parse_limits(limits: Map<String, Value>) -> PackingLimits {
    PackingLimits::from_manifest(
        &RuntimeManifest::parse(&manifest(limits, approval(true))).unwrap(),
    )
    .unwrap()
}

#[test]
fn packing_limits_join_the_manifest_as_one_approved_group() {
    assert_eq!(packing_values().map(|(name, _)| name), PACKING_LIMITS);
    let limits = parse_limits(limits_with_packing());
    assert_eq!(limits.fused_candidates.get(), 16);
    assert_eq!(limits.payload_loads.get(), 12);
    assert_eq!(limits.payload_bytes.get(), 4096);
    assert_eq!(limits.item_bytes.get(), 1024);
    assert_eq!(limits.parents.get(), 8);
    assert_eq!(limits.spans_per_parent.get(), 4);
    assert_eq!(limits.rendered_bytes.get(), 8192);
    assert_eq!(limits.estimated_tokens, ClaudeTokens::new(2048));
    assert_eq!(limits.serialized_bytes.get(), 8000);
    assert_eq!(limits.adjustment_passes, 3);
    assert_eq!(limits.deadline, Duration::from_millis(250));
    assert_eq!(
        limits.optional_bounds(),
        OptionalBounds {
            max_fused_candidates: NonZeroUsize::new(16).unwrap(),
            max_parents: NonZeroUsize::new(8).unwrap(),
            max_spans_per_parent: NonZeroUsize::new(4).unwrap(),
            max_payload_loads: NonZeroUsize::new(12).unwrap(),
            max_payload_bytes: NonZeroU64::new(4096).unwrap(),
            max_item_bytes: NonZeroU64::new(1024).unwrap(),
        }
    );
    assert_eq!(
        limits.required_bounds(ClaudeTokens::new(7)),
        RequiredBounds {
            max_payload_loads: NonZeroUsize::new(12).unwrap(),
            max_payload_bytes: NonZeroU64::new(4096).unwrap(),
            max_item_bytes: NonZeroU64::new(1024).unwrap(),
            token_limit: ClaudeTokens::new(7),
        }
    );
    assert_eq!(
        limits.accounting_bounds(),
        AccountingBounds {
            max_rendered_bytes: 8192,
            max_estimated_tokens: ClaudeTokens::new(2048),
        }
    );
    assert_eq!(
        limits.serialization_bounds(),
        SerializationBounds {
            max_serialized_bytes: 8000,
            max_adjustment_passes: 3,
        }
    );

    let without = RuntimeManifest::parse(&manifest(required_limits(), Map::new())).unwrap();
    assert_eq!(without.packing, None);
    assert_eq!(
        PackingLimits::from_manifest(&without),
        Err(PackingLimitRefusal::Absent)
    );
}

#[test]
fn an_unapproved_partial_unknown_mismatched_or_zero_packing_group_is_refused() {
    assert_eq!(
        RuntimeManifest::parse(&manifest(limits_with_packing(), Map::new())),
        Err(ManifestRefusal::PackingUnapproved)
    );
    assert_eq!(
        RuntimeManifest::parse(&manifest(limits_with_packing(), approval(false))),
        Err(ManifestRefusal::PackingUnapproved)
    );
    let mut partial = limits_with_packing();
    partial.remove("packing_serialized_bytes");
    assert_eq!(
        RuntimeManifest::parse(&manifest(partial, approval(true))),
        Err(ManifestRefusal::MissingLimit(
            "packing_serialized_bytes".to_owned()
        ))
    );
    let mut unknown = limits_with_packing();
    unknown.insert("packing_unknown".to_owned(), json!(1));
    assert_eq!(
        RuntimeManifest::parse(&manifest(unknown, approval(true))),
        Err(ManifestRefusal::UnknownLimit("packing_unknown".to_owned()))
    );
    let mut non_numeric = limits_with_packing();
    non_numeric.insert("packing_parents".to_owned(), json!("eight"));
    assert_eq!(
        RuntimeManifest::parse(&manifest(non_numeric, approval(true))),
        Err(ManifestRefusal::NonNumericLimit(
            "packing_parents".to_owned()
        ))
    );
    let mut mismatched = manifest(limits_with_packing(), approval(true));
    mismatched["invalidation_identity"]["limit_manifest_protocol_version"] = json!("other");
    assert!(matches!(
        RuntimeManifest::parse(&mismatched),
        Err(ManifestRefusal::ProtocolMismatch { .. })
    ));
    for name in PACKING_LIMITS {
        let mut zero = limits_with_packing();
        zero.insert(name.to_owned(), json!(0));
        let parsed = RuntimeManifest::parse(&manifest(zero, approval(true))).unwrap();
        let expected = if name == "packing_adjustment_passes" {
            None
        } else {
            Some(PackingLimitRefusal::Zero(name))
        };
        assert_eq!(
            PackingLimits::from_manifest(&parsed).err(),
            expected,
            "{name}"
        );
    }
    for name in ["packing_rendered_bytes", "packing_serialized_bytes"] {
        let mut past_cap = limits_with_packing();
        past_cap.insert(name.to_owned(), json!(MAX_WIRE_BODY_BYTES + 1));
        let parsed = RuntimeManifest::parse(&manifest(past_cap, approval(true))).unwrap();
        assert_eq!(
            PackingLimits::from_manifest(&parsed),
            Err(PackingLimitRefusal::OutOfRange(name))
        );
        let mut at_cap = limits_with_packing();
        at_cap.insert(name.to_owned(), json!(MAX_WIRE_BODY_BYTES));
        let parsed = RuntimeManifest::parse(&manifest(at_cap, approval(true))).unwrap();
        assert!(PackingLimits::from_manifest(&parsed).is_ok(), "{name}");
    }
}

/// One `PackingLimits` feeds both phases: `accounting_bounds` to admission and
/// `serialization_bounds` to `finalize`.
fn prepare_under(
    fixture: &Fixture,
    limits: &PackingLimits,
) -> Result<Result<Preparation, PackingFailure>, PreparationRefusal> {
    let admission = admit_under(
        fixture,
        &GROUPS,
        &byte_profile(),
        &limits.accounting_bounds(),
    )?;
    Ok(finalize(
        admission,
        &limits.serialization_bounds(),
        &EvalBudget::unbounded(),
    ))
}

#[test]
fn each_serialization_bound_saturates_at_its_value_and_refuses_at_value_plus_one() {
    let fixture = fixture();
    let admission = admit(&fixture, &GROUPS);
    let bytes = admission.ledger().rendered_bytes() as u64;
    let tokens = admission.ledger().total_with_headroom().get();
    let mut at_value = limits_with_packing();
    at_value.insert("packing_rendered_bytes".to_owned(), json!(bytes));
    at_value.insert("packing_estimated_tokens".to_owned(), json!(tokens));
    at_value.insert("packing_serialized_bytes".to_owned(), json!(bytes));
    at_value.insert("packing_adjustment_passes".to_owned(), json!(0));
    let saturated = prepare_under(&fixture, &parse_limits(at_value.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(saturated.body().len() as u64, bytes);
    assert_eq!(saturated.passes(), 0);

    let tightened = |name: &str| {
        let mut tightened = at_value.clone();
        let current = tightened[name].as_u64().unwrap();
        tightened.insert(name.to_owned(), json!(current - 1));
        parse_limits(tightened)
    };
    for (name, exceeded) in [
        (
            "packing_rendered_bytes",
            AccountingExceeded {
                bound: AccountingBound::RenderedBytes,
                value: bytes,
                limit: bytes - 1,
            },
        ),
        (
            "packing_estimated_tokens",
            AccountingExceeded {
                bound: AccountingBound::EstimatedTokens,
                value: tokens,
                limit: tokens - 1,
            },
        ),
    ] {
        guard_calls::reset();
        assert_eq!(
            prepare_under(&fixture, &tightened(name)).unwrap_err(),
            PreparationRefusal::Accounting(exceeded),
            "{name}"
        );
        assert_eq!(guard_calls::counts(), (0, 0), "{name}");
    }
    guard_calls::reset();
    assert_eq!(
        prepare_under(&fixture, &tightened("packing_serialized_bytes"))
            .unwrap()
            .unwrap_err(),
        PackingFailure::AdjustmentCapExhausted {
            passes: 0,
            bound: SerializationBound::SerializedBytes,
        }
    );
    assert_eq!(guard_calls::counts(), (1, 0));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            paths.extend(walk(&path));
        } else {
            paths.push(path);
        }
    }
    paths
}

#[test]
fn the_packing_path_reaches_no_legacy_clamp_or_selection_module() {
    let packing_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/packing");
    let mut packing_sources = String::new();
    for path in walk(&packing_dir) {
        assert_ne!(path.file_name().unwrap(), "selection.rs");
        let source = std::fs::read_to_string(&path).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap();
        packing_sources.push_str(production);
    }
    for forbidden in [
        "trim_memories_to_budget",
        "trim_user_profile_to_budget",
        "render_memory_line",
        "history_budget_tokens",
    ] {
        assert!(!packing_sources.contains(forbidden), "{forbidden}");
    }
}
