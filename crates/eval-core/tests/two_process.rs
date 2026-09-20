//! Child-role two-process witnesses: the parent re-executes this test binary
//! with `--ignored --exact <role>` and compares the digests each child prints.

mod support;

use std::process::{Command, Stdio};

use context_core::canonical_json::protocol_digest;
use eval_core::{
    ChoiceKind, Mode, ObservationSchema, Rule, SemanticTrace, eval_run_id, generate_all,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use support::{
    OBSERVATION_TYPE, identity, manifest_for, observation, observation_schema, world_config,
};

const CODE_SHA_ENV: &str = "EIDNARA_EVAL_CHILD_CODE_SHA";
const WORLD_SEED_ENV: &str = "EIDNARA_EVAL_CHILD_WORLD_SEED";
const REPORT_PREFIX: &str = "EIDNARA_EVAL_DIGESTS ";
const LEAK_TYPE: &str = "planted_map_leak";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Digests {
    run_id: String,
    manifest: String,
    trace: String,
}

fn child(role: &str, code_sha: Option<&str>) -> Digests {
    serde_json::from_str(&child_report(role, code_sha.map(|sha| (CODE_SHA_ENV, sha)))).unwrap()
}

fn child_report(role: &str, env: Option<(&str, &str)>) -> String {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", role, "--nocapture"])
        .stdout(Stdio::piped());
    if let Some((key, value)) = env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "child {role} failed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report = stdout
        .lines()
        .find_map(|line| line.strip_prefix(REPORT_PREFIX))
        .unwrap_or_else(|| panic!("no digest report in {stdout}"));
    report.to_string()
}

fn child_identity() -> eval_core::RunIdentity {
    let mut identity = identity();
    if let Ok(sha) = std::env::var(CODE_SHA_ENV) {
        identity.build.code_sha = sha;
    }
    identity
}

fn child_trace() -> SemanticTrace {
    let mut trace = SemanticTrace::new([observation_schema()]).unwrap();
    for sequence in 0..4 {
        trace
            .record(
                OBSERVATION_TYPE,
                &observation(sequence, "inc-child", Some(1)),
            )
            .unwrap();
    }
    trace
}

fn digests_for(trace: &SemanticTrace) -> Digests {
    let identity = child_identity();
    Digests {
        run_id: eval_run_id(&identity).unwrap(),
        manifest: manifest_for(identity.clone(), trace).digest().unwrap(),
        trace: trace.digest().unwrap(),
    }
}

fn report(trace: &SemanticTrace) {
    println!(
        "{REPORT_PREFIX}{}",
        serde_json::to_string(&digests_for(trace)).unwrap()
    );
}

#[test]
#[ignore = "child role; the two_process tests launch it"]
fn child_role_reports_digests() {
    report(&child_trace());
}

/// Negative control: a `Keep` field whose array follows `HashMap` iteration order.
#[test]
#[ignore = "child role; the two_process tests launch it"]
fn child_role_reports_digests_with_planted_map_leak() {
    let leak = ObservationSchema::new(LEAK_TYPE, [("candidates", Rule::Keep)]).unwrap();
    let mut trace = SemanticTrace::new([observation_schema(), leak]).unwrap();
    trace
        .record(OBSERVATION_TYPE, &observation(0, "inc-child", None))
        .unwrap();
    #[allow(clippy::disallowed_types, reason = "the planted leak is the control")]
    let map: std::collections::HashMap<u32, String> =
        (0..64).map(|n| (n, format!("candidate-{n}"))).collect();
    let leaked: Vec<String> = map.into_values().collect();
    trace
        .record(LEAK_TYPE, &json!({"candidates": leaked}))
        .unwrap();
    report(&trace);
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WorldDigests {
    log: String,
    tape: String,
}

fn world_digests(seed: u64) -> WorldDigests {
    let world = generate_all(seed, &world_config(), Mode::Generate).unwrap();
    let kinds: std::collections::BTreeSet<ChoiceKind> =
        world.tape.entries.iter().map(|c| c.site.kind).collect();
    assert_eq!(kinds.len(), 7, "the compared world draws every choice kind");
    WorldDigests {
        log: world.log.digest(),
        tape: protocol_digest("test-tape/v1", &serde_json::to_value(&world.tape).unwrap()).unwrap(),
    }
}

#[test]
#[ignore = "child role; the two_process tests launch it"]
fn child_role_reports_world_digests() {
    let seed = std::env::var(WORLD_SEED_ENV)
        .map(|seed| seed.parse().unwrap())
        .unwrap_or(7);
    println!(
        "{REPORT_PREFIX}{}",
        serde_json::to_string(&world_digests(seed)).unwrap()
    );
}

#[test]
fn two_process_same_world_inputs_yield_equal_log_and_tape_digests() {
    let role = "child_role_reports_world_digests";
    let first: WorldDigests = serde_json::from_str(&child_report(role, None)).unwrap();
    let second: WorldDigests = serde_json::from_str(&child_report(role, None)).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, world_digests(7), "the parent agrees");
    let reseeded: WorldDigests =
        serde_json::from_str(&child_report(role, Some((WORLD_SEED_ENV, "8")))).unwrap();
    assert_ne!(first.log, reseeded.log);
    assert_ne!(first.tape, reseeded.tape);
}

#[test]
fn two_process_same_identity_yields_equal_manifest_and_trace_digests() {
    let first = child("child_role_reports_digests", None);
    let second = child("child_role_reports_digests", None);
    assert_eq!(first, second);
    assert_eq!(first, digests_for(&child_trace()), "the parent agrees");
}

#[test]
fn two_process_planted_map_order_leak_fails_the_equality_test() {
    let role = "child_role_reports_digests_with_planted_map_leak";
    let first = child(role, None);
    let second = child(role, None);
    assert_eq!(first.run_id, second.run_id);
    assert_eq!(
        first.manifest, second.manifest,
        "the manifest carries no leaked order"
    );
    assert_ne!(
        first.trace, second.trace,
        "the planted map order must be observed"
    );

    let ordered = |candidates: [&str; 2]| {
        let leak = ObservationSchema::new(LEAK_TYPE, [("candidates", Rule::Keep)]).unwrap();
        let mut trace = SemanticTrace::new([leak]).unwrap();
        trace
            .record(LEAK_TYPE, &json!({"candidates": candidates}))
            .unwrap();
        trace.digest().unwrap()
    };
    assert_ne!(
        ordered(["a", "b"]),
        ordered(["b", "a"]),
        "array order enters the trace digest"
    );
}

#[test]
fn two_process_changed_build_component_fails_the_identity_check() {
    let base = child("child_role_reports_digests", None);
    let rebuilt = child("child_role_reports_digests", Some(&"5e".repeat(20)));
    assert_ne!(base.run_id, rebuilt.run_id);
    assert_ne!(base.manifest, rebuilt.manifest);
    assert_eq!(
        base.trace, rebuilt.trace,
        "the trace does not depend on the build"
    );
}
