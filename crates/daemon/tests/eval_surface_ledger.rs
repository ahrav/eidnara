//! The stage ledger over surface 1 through the direct-host fixture: the
//! thirteen auto-search stages observed from the host's own returns, five
//! injected faults, and the failure-class assignment of each.

#![cfg(all(unix, feature = "test-support"))]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use daemon::harness_sources::{Harness, SessionIdentity, opencode_units};
use daemon::transform::{UserHintOutcome, UserHintPass, UserHintSkip};
use eval_core::{
    Cell, Coverage, CoverageError, Delivery, DurableState, FailureClass, MARKERS, Mode, Outcome,
    Presence, RenderConfig, Required, SURFACE1_STAGES, SessionSpec, Slice, StageVerdict,
    Surface1Stage, WorldConfig, classify, generate_all, render,
};
use memory_store::StoredHistorySegment;
use support::direct_host::FixtureProcess;
use support::eval_surface::{
    EPOCH_MS, Knobs, Pass, SurfaceLedger, World, block_on, expected_id, identities, mid, observe,
    pass, seed_store, segment,
};

const SUITE: &str = "crates/daemon/tests/eval_surface_ledger.rs::";
const SEED: u64 = 0x5155_5FAC;
const PROJECT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HINT_OPEN: &str = "<eidnara-search-hint>";
const WINDOW: usize = 100;
/// Long enough for the length gate and lexically distinct from every other segment.
const PROMPT: &str = "quasar nebula survey cadence decision";
const PHRASE: &str = "quasar nebula survey cadence decision: weekly";
const FILLER: &str = "lunch and office plants chatter";

fn world(messages: u32) -> World {
    let config = WorldConfig {
        sessions: vec![SessionSpec {
            messages,
            tool_span_every: 0,
            correction_every: 0,
            invalidation_every: 0,
        }],
        repositories: Vec::new(),
        epoch_ms: EPOCH_MS,
        tick_ms: 1_000,
        max_events_per_log: 512,
        planted: Vec::new(),
    };
    let generated = generate_all(SEED, &config, Mode::Generate).unwrap();
    let rendering = render(
        &generated.log,
        &RenderConfig {
            project_id: PROJECT.to_string(),
            repository_id: "repo-0".to_string(),
            object_format: "sha1".to_string(),
        },
    )
    .unwrap();
    let session = rendering.messages[0].session_id.clone();
    assert!(rendering.messages.iter().all(|m| m.session_id == session));
    World {
        session,
        messages: rendering.messages,
    }
}

/// Segment `sequence` covers message `sequence - 1`; `phrase_of` picks each
/// segment's summary.
fn segments(world: &World, phrase_of: impl Fn(i64) -> &'static str) -> Vec<StoredHistorySegment> {
    world
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let sequence = index as i64 + 1;
            segment(sequence, message, phrase_of(sequence))
        })
        .collect()
}

/// The rendered hint names every selected segment by title and counts them,
/// so the render stage is read from the hint itself, not copied from the cap.
fn assert_rendered(outcome: &UserHintOutcome) {
    let count = outcome.trace.selected.len();
    let header = format!("Your memory may contain {count} related fragment");
    assert!(outcome.hint_text.contains(&header), "{}", outcome.hint_text);
    for sequence in &outcome.trace.selected {
        let title = format!("- C{sequence} ");
        assert!(outcome.hint_text.contains(&title), "{}", outcome.hint_text);
    }
}

fn verdict(ledger: &SurfaceLedger, occurrence: &str) -> StageVerdict<Surface1Stage> {
    ledger.verdict(
        &[Required {
            occurrence: occurrence.to_string(),
            entry: Surface1Stage::TailEligibility,
        }],
        &BTreeSet::new(),
        Surface1Stage::Attachment,
    )
}

/// The failure class of a failing cassette-slice task whose store held the knowledge.
fn class(verdict: StageVerdict<Surface1Stage>) -> Option<FailureClass> {
    classify(Cell {
        delivery: Delivery::of(verdict),
        durable_state: DurableState::Held,
        slice: Slice::Cassette,
        outcome: Outcome::Fail,
    })
}

/// Every attached segment ends on a message whose native identity the request
/// carried; the production adapter reads that native message and the evaluator
/// encoder gives it exactly the id the renderer expected.
fn prove_identities(world: &World, run: &Pass, segments: &[StoredHistorySegment]) {
    let Some(UserHintPass::Decided(outcome)) = run.outcome.as_ref() else {
        panic!("the tail was eligible");
    };
    assert!(!outcome.trace.selected.is_empty());
    let session = SessionIdentity {
        project_id: PROJECT.to_string(),
        harness: Harness::OpenCode,
        session_id: world.session.clone(),
    };
    for sequence in &outcome.trace.selected {
        let segment = segments.iter().find(|s| s.sequence == *sequence).unwrap();
        let (end_mid, _) = daemon::wire::split_block_id(&segment.end_message_id)
            .expect("a seeded segment ends on a block id");
        let native = run
            .native
            .iter()
            .find(|message| message["info"]["id"].as_str() == Some(end_mid))
            .expect("the attached segment ends on a native message the request carried");
        let units = opencode_units(&session, native).unwrap();
        let identity: Vec<(&str, &str)> = units[0]
            .identity
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let encoded = eval_core::encode(&eval_core::Occurrence {
            class: units[0].class.code(),
            identity: &identity,
            revision: &units[0].revision,
            representation: units[0].representation.as_str(),
            span: None,
        })
        .unwrap();
        let rendered = world.messages.iter().find(|m| mid(m) == end_mid).unwrap();
        assert_eq!(encoded.occurrence_id, expected_id(rendered), "{end_mid}");
    }
}

struct Run {
    world: World,
    segments: Vec<StoredHistorySegment>,
    identities: BTreeMap<i64, String>,
    pass: Pass,
    ledger: SurfaceLedger,
}

fn run(messages: u32, phrase_of: impl Fn(i64) -> &'static str, prompt: &str, knobs: &Knobs) -> Run {
    block_on(async {
        let world = world(messages);
        let segments = segments(&world, phrase_of);
        let identities = identities(&world, &segments);
        let root = tempfile::tempdir().unwrap();
        seed_store(root.path(), &world.session, &segments);
        let fixture = FixtureProcess::start_at(root.path().to_path_buf());
        let pass = pass(&fixture, &world, prompt, knobs).await;
        let _ = fixture.shutdown();
        let mut ledger = SurfaceLedger::default();
        observe(&mut ledger, pass.outcome.as_ref(), &identities);
        Run {
            world,
            segments,
            identities,
            pass,
            ledger,
        }
    })
}

fn only_second(sequence: i64) -> &'static str {
    if sequence == 2 { PHRASE } else { FILLER }
}

fn outcome(run: &Run) -> &UserHintOutcome {
    match run.pass.outcome.as_ref() {
        Some(UserHintPass::Decided(outcome)) => outcome,
        other => panic!("the tail was eligible: {other:?}"),
    }
}

#[test]
fn a_matching_segment_is_delivered_through_every_stage() {
    let run = run(6, only_second, PROMPT, &Knobs::default());
    let outcome = outcome(&run);
    assert_eq!(outcome.trace.window.len(), 6);
    assert_eq!(outcome.trace.matched, [2]);
    assert_eq!(outcome.trace.selected, [2]);
    assert!(outcome.hint_text.contains(HINT_OPEN));
    assert!(outcome.applied && outcome.attached && !outcome.deferred);
    assert_rendered(outcome);
    let rule = &run.identities[&2];
    for stage in SURFACE1_STAGES {
        assert_eq!(
            run.ledger.presence(stage, rule),
            Some(Presence::Reached),
            "{stage:?}"
        );
    }
    let other = &run.identities[&3];
    assert_eq!(
        run.ledger.presence(Surface1Stage::MatchFilter, other),
        Some(Presence::ReachedEvidenceAbsent)
    );
    assert_eq!(verdict(&run.ledger, rule), StageVerdict::Clean);
    prove_identities(&run.world, &run.pass, &run.segments);
    assert_eq!(
        class(StageVerdict::Clean),
        Some(FailureClass::Indeterminate)
    );
}

fn an_old_segment_outside_the_window_is_lost_at_the_candidate_window(coverage: &mut Coverage) {
    let run = run(
        WINDOW as u32 + 1,
        |sequence| if sequence == 1 { PHRASE } else { FILLER },
        PROMPT,
        &Knobs::default(),
    );
    let outcome = outcome(&run);
    assert_eq!(outcome.trace.window.len(), WINDOW);
    assert!(!outcome.trace.window.contains(&1));
    assert!(outcome.trace.window.contains(&2));
    assert!(outcome.trace.matched.is_empty());
    let rule = &run.identities[&1];
    assert_eq!(
        run.ledger.presence(Surface1Stage::TokenGate, rule),
        Some(Presence::Reached)
    );
    coverage.record("sls_injection_candidate_window").unwrap();
    let verdict = verdict(&run.ledger, rule);
    assert_eq!(
        verdict,
        StageVerdict::FirstLoss(Surface1Stage::CandidateWindow)
    );
    assert_eq!(class(verdict), Some(FailureClass::Interference));
}

fn a_short_prompt_is_lost_at_the_length_gate(coverage: &mut Coverage) {
    let run = run(6, only_second, "quasar cadence", &Knobs::default());
    let outcome = outcome(&run);
    assert!(outcome.trace.suppression);
    assert!(!outcome.trace.length);
    assert!(outcome.hint_text.is_empty());
    let rule = &run.identities[&2];
    assert_eq!(
        run.ledger.presence(Surface1Stage::CandidateWindow, rule),
        Some(Presence::NotReached)
    );
    coverage.record("sls_injection_prompt_gate").unwrap();
    let verdict = verdict(&run.ledger, rule);
    assert_eq!(verdict, StageVerdict::FirstLoss(Surface1Stage::LengthGate));
    assert_eq!(class(verdict), Some(FailureClass::Interference));
}

fn a_raised_threshold_is_lost_at_the_threshold(coverage: &mut Coverage) {
    let run = run(
        6,
        only_second,
        PROMPT,
        &Knobs {
            threshold: 1.5,
            ..Knobs::default()
        },
    );
    let outcome = outcome(&run);
    assert_eq!(
        outcome.trace.matched,
        [2],
        "the segment is in the window and matches"
    );
    assert!(!outcome.trace.threshold);
    assert!(outcome.trace.selected.is_empty());
    let rule = &run.identities[&2];
    assert_eq!(
        run.ledger.presence(Surface1Stage::MatchFilter, rule),
        Some(Presence::Reached)
    );
    coverage.record("sls_injection_score_threshold").unwrap();
    let verdict = verdict(&run.ledger, rule);
    assert_eq!(verdict, StageVerdict::FirstLoss(Surface1Stage::Threshold));
    assert_eq!(class(verdict), Some(FailureClass::Interference));
}

/// Four of twelve segments match, so the shared tokens stay rare enough for
/// the match filter; equal scores and recencies leave the sequence order to
/// decide which three the cap keeps.
fn a_fourth_match_is_lost_at_the_cap(coverage: &mut Coverage) {
    let run = run(
        12,
        |sequence| {
            if (1..=4).contains(&sequence) {
                PHRASE
            } else {
                FILLER
            }
        },
        PROMPT,
        &Knobs::default(),
    );
    let outcome = outcome(&run);
    assert_eq!(outcome.trace.matched, [1, 2, 3, 4]);
    assert_eq!(outcome.trace.selected, [1, 2, 3]);
    assert!(outcome.attached);
    assert_rendered(outcome);
    assert!(!outcome.hint_text.contains("- C4 "));
    let rule = &run.identities[&4];
    assert_eq!(
        run.ledger.presence(Surface1Stage::Threshold, rule),
        Some(Presence::Reached)
    );
    coverage.record("sls_injection_result_cap").unwrap();
    let capped = verdict(&run.ledger, rule);
    assert_eq!(capped, StageVerdict::FirstLoss(Surface1Stage::Cap));
    assert_eq!(class(capped), Some(FailureClass::Interference));
    assert_eq!(
        verdict(&run.ledger, &run.identities[&1]),
        StageVerdict::Clean
    );
}

fn a_native_array_without_the_tail_is_lost_at_attachment(coverage: &mut Coverage) {
    let run = run(
        6,
        only_second,
        PROMPT,
        &Knobs {
            native_tail: false,
            ..Knobs::default()
        },
    );
    let outcome = outcome(&run);
    assert_eq!(outcome.trace.selected, [2]);
    assert!(outcome.applied, "the served block carries the hint");
    assert!(!outcome.attached, "no native message carries it");
    let rule = &run.identities[&2];
    assert_eq!(
        run.ledger.presence(Surface1Stage::OverlayApply, rule),
        Some(Presence::Reached)
    );
    assert_eq!(
        run.ledger.presence(Surface1Stage::Attachment, rule),
        Some(Presence::ReachedEvidenceAbsent),
        "rendered but undelivered"
    );
    coverage.record("sls_injection_attachment").unwrap();
    let verdict = verdict(&run.ledger, rule);
    assert_eq!(verdict, StageVerdict::FirstLoss(Surface1Stage::Attachment));
    assert_eq!(class(verdict), Some(FailureClass::Interference));
}

/// The outcome never reaches the wire: a response serializes to the same bytes
/// whatever `user_hint` holds.
#[test]
fn the_user_hint_pass_leaves_the_wire_response_bytes_unchanged() {
    let mut response = daemon::transform::TransformResponse::need_full_sync(None);
    let bare = serde_json::to_vec(&response).unwrap();
    response.user_hint = Some(UserHintPass::Skipped {
        reason: UserHintSkip::AlreadyDecided,
    });
    assert_eq!(serde_json::to_vec(&response).unwrap(), bare);
    response.user_hint = Some(UserHintPass::Decided(UserHintOutcome {
        block_id: "tail-1#0".to_string(),
        hint_text: HINT_OPEN.to_string(),
        trace: daemon::transform::UserHintTrace::default(),
        deferred: false,
        applied: true,
        attached: true,
    }));
    assert_eq!(serde_json::to_vec(&response).unwrap(), bare);
    assert_eq!(
        <Surface1Stage as eval_core::Stage>::REACHABILITY,
        eval_core::Reachability::DefaultProduction
    );
}

/// A second pass over the same tail decides nothing because the first pass's
/// decision is frozen; the shell must not read that as the tail being
/// ineligible.
#[test]
fn a_repeated_pass_reports_the_frozen_decision_as_unjoinable() {
    block_on(async {
        let world = world(6);
        let segments = segments(&world, only_second);
        let identities = identities(&world, &segments);
        let root = tempfile::tempdir().unwrap();
        seed_store(root.path(), &world.session, &segments);
        let fixture = FixtureProcess::start_at(root.path().to_path_buf());
        let first = pass(&fixture, &world, PROMPT, &Knobs::default()).await;
        let second = pass(&fixture, &world, PROMPT, &Knobs::default()).await;
        let _ = fixture.shutdown();
        assert!(matches!(first.outcome, Some(UserHintPass::Decided(_))));
        assert_eq!(
            second.outcome,
            Some(UserHintPass::Skipped {
                reason: UserHintSkip::AlreadyDecided
            })
        );
        let rule = &identities[&2];
        let mut ledger = SurfaceLedger::default();
        observe(&mut ledger, second.outcome.as_ref(), &identities);
        assert_eq!(ledger.presence(Surface1Stage::TailEligibility, rule), None);
        assert_eq!(verdict(&ledger, rule), StageVerdict::Indeterminate);
    });
}

/// A shell that reports the adapter's view of survivors instead of the host's
/// names the wrong verdict: only the host's own attachment check counts.
#[test]
fn a_false_survivor_from_the_adapter_fails_the_self_test() {
    let run = run(
        6,
        only_second,
        PROMPT,
        &Knobs {
            native_tail: false,
            ..Knobs::default()
        },
    );
    let rule = &run.identities[&2];
    assert_eq!(
        verdict(&run.ledger, rule),
        StageVerdict::FirstLoss(Surface1Stage::Attachment)
    );
    let mut forged = outcome(&run).clone();
    forged.attached = run.pass.response["operations"].is_array();
    assert!(
        forged.attached,
        "the recipe exists, but it does not carry the hint natively"
    );
    let forged = UserHintPass::Decided(forged);
    let mut adapter_view = SurfaceLedger::default();
    observe(&mut adapter_view, Some(&forged), &run.identities);
    assert_eq!(
        verdict(&adapter_view, rule),
        StageVerdict::Clean,
        "an adapter-supplied survivor would hide the attachment loss"
    );
    let mut mis_mapped = run.identities.clone();
    mis_mapped.insert(2, run.identities[&3].clone());
    mis_mapped.insert(3, run.identities[&2].clone());
    let mut wrong = SurfaceLedger::default();
    observe(&mut wrong, run.pass.outcome.as_ref(), &mis_mapped);
    assert_eq!(
        verdict(&wrong, rule),
        StageVerdict::FirstLoss(Surface1Stage::MatchFilter),
        "a swapped identity moves the loss to the wrong stage"
    );
}

type Scenario = fn(&mut Coverage);

fn scenarios() -> [(&'static str, Scenario); 5] {
    [
        (
            "an_old_segment_outside_the_window_is_lost_at_the_candidate_window",
            an_old_segment_outside_the_window_is_lost_at_the_candidate_window,
        ),
        (
            "a_short_prompt_is_lost_at_the_length_gate",
            a_short_prompt_is_lost_at_the_length_gate,
        ),
        (
            "a_raised_threshold_is_lost_at_the_threshold",
            a_raised_threshold_is_lost_at_the_threshold,
        ),
        (
            "a_fourth_match_is_lost_at_the_cap",
            a_fourth_match_is_lost_at_the_cap,
        ),
        (
            "a_native_array_without_the_tail_is_lost_at_attachment",
            a_native_array_without_the_tail_is_lost_at_attachment,
        ),
    ]
}

fn run_scenario(name: &str) {
    let (_, scenario) = scenarios().into_iter().find(|(n, _)| *n == name).unwrap();
    let mut coverage = Coverage::default();
    scenario(&mut coverage);
    let marker = MARKERS
        .iter()
        .find(|m| m.test == format!("{SUITE}{name}"))
        .unwrap();
    assert!(
        coverage.fired().contains(marker.name),
        "{name} records its marker"
    );
}

#[test]
fn candidate_window_injection() {
    run_scenario("an_old_segment_outside_the_window_is_lost_at_the_candidate_window");
}

#[test]
fn prompt_gate_injection() {
    run_scenario("a_short_prompt_is_lost_at_the_length_gate");
}

#[test]
fn score_injection() {
    run_scenario("a_raised_threshold_is_lost_at_the_threshold");
}

#[test]
fn render_cap_injection() {
    run_scenario("a_fourth_match_is_lost_at_the_cap");
}

#[test]
fn attach_injection() {
    run_scenario("a_native_array_without_the_tail_is_lost_at_attachment");
}

#[test]
fn surface_markers_each_name_a_scenario_here() {
    let scenario_names: BTreeSet<&str> = scenarios().iter().map(|(n, _)| *n).collect();
    let owned: Vec<&str> = MARKERS
        .iter()
        .filter(|m| m.name.starts_with("sls_"))
        .map(|m| {
            m.test
                .strip_prefix(SUITE)
                .unwrap_or_else(|| panic!("{}", m.test))
        })
        .collect();
    assert_eq!(owned.len(), scenario_names.len());
    for test in owned {
        assert!(scenario_names.contains(test), "{test}");
    }
    let mut coverage = Coverage::default();
    coverage.record("sls_injection_prompt_gate").unwrap();
    assert!(matches!(
        coverage.complete(SUITE),
        Err(CoverageError::Incomplete { .. })
    ));
}

/// The completeness proof: one run of every scenario fires every marker this
/// suite owns.
#[test]
fn every_surface_marker_fires_across_the_scenarios() {
    let mut coverage = Coverage::default();
    for (_, scenario) in scenarios() {
        scenario(&mut coverage);
    }
    coverage.complete(SUITE).unwrap();
}
