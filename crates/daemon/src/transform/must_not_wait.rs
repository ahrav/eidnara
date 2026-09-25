//! The must-not-wait contract: every event that must rebuild or bust does so with a published summary still unrendered and a live summarizer run, and only the ordinary Execute-band arm waits for that run.

use memory_store::MemoryStore;

use super::tests::{
    canonical_read, cc_req, comp, item, pctx, req, run, seed_astro_divergence, spine, store,
    system_item, with_usage,
};
use super::*;

const SESSION: &str = "hold";

/// A session whose first fold rendered segments 1 and 2 and whose segment 3 is published but unrendered, as a below-threshold pass leaves it.
fn pending(dir: &std::path::Path) -> (MemoryStore, Vec<IngressMessage>) {
    let s = store(dir);
    let mut messages = vec![item("anchor", 1, "alpha"), item("fold-target", 2, "beta")];
    run(&s, &req(SESSION, "cfg0", messages.clone()), &spine());
    s.replace_history_segments(
        SESSION,
        &[
            comp(1, 1, 1, "anchor", "first coverage"),
            comp(2, 2, 2, "fold-target", "second coverage"),
        ],
    )
    .unwrap();
    messages.push(item("tail", 3, "tail"));
    let first = run(&s, &req(SESSION, "cfg0", messages.clone()), &spine());
    assert_eq!(
        (first.action.as_str(), first.materialize_reason.as_deref()),
        ("HARD", Some("coverage_fold")),
        "a session's first publication folds at once"
    );
    s.append_history_segments(SESSION, &[comp(3, 3, 3, "tail", "third coverage")])
        .unwrap();
    messages.push(item("next", 4, "next"));
    assert_eq!(rendered(&s), 2);
    (s, messages)
}

fn rendered(s: &MemoryStore) -> i64 {
    s.load(SESSION).unwrap().meta.rendered_history_segment_seq()
}

/// Usage below the proactive percentage at the default execute threshold of 65.
fn quiet(messages: &[IngressMessage], cfg: &str) -> TransformRequest {
    with_usage(req(SESSION, cfg, messages.to_vec()), 60_000, 200_000)
}

fn live_run<'a>(dir: &'a str, now_ms: i64) -> ProducerContext<'a> {
    let mut ctx = pctx("git:proj", dir, now_ms);
    ctx.history_summarizer_active = true;
    ctx
}

fn served(response: &TransformResponse) -> (&str, Option<&str>) {
    (
        response.action.as_str(),
        response.materialize_reason.as_deref(),
    )
}

#[test]
fn a_pending_summary_waits_below_the_threshold_and_ten_repeats_serve_the_same_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (s, messages) = pending(dir.path());
    let ctx = live_run("/nonexistent-docs", 0);
    let first = transform(&s, &quiet(&messages, "cfg0"), &ctx).unwrap();
    assert_eq!(served(&first), ("SOFT+", None));
    for _ in 0..10 {
        let repeat = transform(&s, &quiet(&messages, "cfg0"), &ctx).unwrap();
        assert_eq!(served(&repeat), ("SOFT+", None));
        assert_eq!(
            serde_json::to_vec(&repeat.messages).unwrap(),
            serde_json::to_vec(&first.messages).unwrap()
        );
    }
    assert_eq!(rendered(&s), 2, "no cause renders segment 3");
    assert!(s.load(SESSION).unwrap().core.pending_changes.is_empty());
}

#[test]
fn only_the_ordinary_execute_arm_waits_for_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let (s, messages) = pending(dir.path());
    let execute = with_usage(req(SESSION, "cfg0", messages.clone()), 140_000, 200_000);
    let held = transform(&s, &execute, &live_run("/nonexistent-docs", 0)).unwrap();
    assert_eq!(
        served(&held),
        ("SOFT+", None),
        "the live run defers Execute"
    );
    assert_eq!(rendered(&s), 2);
    let released = run(&s, &execute, &spine());
    assert_eq!(served(&released), ("SOFT", Some("coverage_fold")));
    assert_eq!(rendered(&s), 3);
}

/// Each hard member and each forced member busts with a live run and a pending summary; the forced ones reach the classifier through the bust gate, so a hold inserted before that gate fails them.
#[test]
fn every_must_not_wait_member_rebuilds_or_busts_through_a_live_run() {
    type Setup = fn(&MemoryStore, &[IngressMessage]) -> (TransformRequest, i64);
    let cases: [(&str, Setup, (&str, &str)); 6] = [
        (
            "render-config epoch",
            |_, messages| (quiet(messages, "cfg1"), 0),
            ("HARD", "epoch_change"),
        ),
        (
            "project-memory epoch",
            |s, messages| {
                let loaded = s.load(SESSION).unwrap();
                let mut meta = loaded.meta.clone();
                meta.project_memory_epoch_pending = true;
                s.commit(SESSION, loaded.row_version, &loaded.core, &meta)
                    .unwrap();
                (quiet(messages, "cfg0"), 0)
            },
            ("HARD", "project_memory_epoch"),
        ),
        (
            "idle TTL with an in-process anchor",
            |_, messages| (quiet(messages, "cfg0"), 1),
            ("HARD", "ttl_expiry"),
        ),
        (
            "Force85",
            |_, messages| {
                (
                    with_usage(req(SESSION, "cfg0", messages.to_vec()), 172_000, 200_000),
                    0,
                )
            },
            ("SOFT", "coverage_fold"),
        ),
        (
            "Emergency95",
            |_, messages| {
                (
                    with_usage(req(SESSION, "cfg0", messages.to_vec()), 192_000, 200_000),
                    0,
                )
            },
            ("SOFT", "coverage_fold"),
        ),
        (
            "explicit flush",
            |s, messages| {
                s.arm_soft_refresh(SESSION).unwrap();
                (quiet(messages, "cfg0"), 0)
            },
            ("SOFT", "explicit_flush"),
        ),
    ];
    for (member, setup, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (s, messages) = pending(dir.path());
        let (request, anchor) = setup(&s, &messages);
        let mut ctx = live_run("/nonexistent-docs", 1_000_000_000);
        if anchor != 0 {
            ctx.observed_last_response_at_ms = Some(1_000_000_000 - 3_600_000);
        } else {
            ctx.now_ms = 0;
        }
        let response = transform(&s, &request, &ctx).unwrap();
        assert_eq!(
            served(&response),
            (expected.0, Some(expected.1)),
            "{member}"
        );
        assert_eq!(rendered(&s), 3, "{member} renders the pending summary");
    }
}

#[test]
fn the_drain_latch_busts_through_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let (s, messages) = pending(dir.path());
    let loaded = s.load(SESSION).unwrap();
    let mut meta = loaded.meta.clone();
    meta.emergency_drain_active = true;
    meta.emergency_drain_entered_at_ms = 1;
    s.commit(SESSION, loaded.row_version, &loaded.core, &meta)
        .unwrap();
    let request = with_usage(req(SESSION, "cfg0", messages), 124_000, 200_000);
    let response = transform(&s, &request, &live_run("/nonexistent-docs", 0)).unwrap();
    assert_ne!(response.action, "SOFT+", "{response:?}");
    assert_eq!(rendered(&s), 3);
}

#[test]
fn an_expired_cache_ttl_activates_the_pending_summary_with_or_without_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let (s, messages) = pending(dir.path());
    let mut anchored = pctx("git:proj", "/nonexistent-docs", 1_000_000_000);
    anchored.observed_last_response_at_ms = Some(1_000_000_000 - 3_600_000);
    let hard = transform(&s, &quiet(&messages, "cfg0"), &anchored).unwrap();
    assert_eq!(served(&hard), ("HARD", Some("ttl_expiry")));
    assert_eq!(rendered(&s), 3);

    let dir = tempfile::tempdir().unwrap();
    let (s, messages) = pending(dir.path());
    // Without an in-process anchor, as after a restart, the expired TTL enters the Execute band.
    let unanchored = pctx("git:proj", "/nonexistent-docs", 1_000_000_000);
    let soft = transform(&s, &quiet(&messages, "cfg0"), &unanchored).unwrap();
    assert_eq!(served(&soft), ("SOFT", Some("coverage_fold")));
    assert_eq!(rendered(&s), 3);
}

#[test]
fn boundary_absence_steps_once_then_reconciles_through_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let (s, _) = pending(dir.path());
    let reverted = vec![item("anchor", 1, "alpha"), item("new-turn", 2, "rewritten")];
    let ctx = live_run("/nonexistent-docs", 0);
    let observing = transform(&s, &quiet(&reverted, "cfg0"), &ctx).unwrap();
    assert_eq!(observing.action, "SOFT+");
    assert!(observing.reconcile_pending);
    let rebuilt = transform(&s, &quiet(&reverted, "cfg0"), &ctx).unwrap();
    assert_eq!(served(&rebuilt), ("HARD", Some("reconcile")));
}

#[test]
fn a_boundary_divergence_recut_rebuilds_through_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let request = seed_astro_divergence(&s, "astro-hold", 2_402);
    let response = transform(&s, &request, &live_run("/nonexistent-docs", 0)).unwrap();
    assert_eq!(
        served(&response),
        ("HARD", Some("boundary_divergence_recut"))
    );
}

#[test]
fn a_covered_system_message_absorbs_through_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    s.replace_history_segments("absorb", &[comp(1, 1, 1, "m1", "SUMMARY")])
        .unwrap();
    let mut items = vec![item("m1", 1, "covered"), item("t2", 2, "tail")];
    assert_eq!(
        run(&s, &cc_req("absorb", "cfg0", items.clone()), &spine()).action,
        "HARD"
    );
    items.push(system_item("sys3", 3, "late identity"));
    items.push(item("t4", 4, "after"));
    s.append_history_segments("absorb", &[comp(2, 2, 3, "sys3", "SUMMARY TWO")])
        .unwrap();
    let response = transform(
        &s,
        &cc_req("absorb", "cfg0", items),
        &live_run("/nonexistent-docs", 0),
    )
    .unwrap();
    assert_eq!(served(&response), ("HARD", Some("coverage_fold")));
}

#[test]
fn explicit_flush_on_the_additive_only_path_reports_m1_delta() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    let mut ctx = live_run("/nonexistent-docs", 0);
    ctx.compaction_enabled = false;
    let messages = vec![item("a", 1, "x")];
    transform(&s, &quiet(&messages, "cfg0"), &ctx).unwrap();
    s.arm_soft_refresh(SESSION).unwrap();
    let flushed = transform(&s, &quiet(&messages, "cfg0"), &ctx).unwrap();
    assert_eq!(served(&flushed), ("SOFT", Some("m1_delta")));
}

#[test]
fn identity_drift_on_a_covered_message_refuses_the_pass_without_replaying_frozen_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (s, mut messages) = pending(dir.path());
    let before = s.load(SESSION).unwrap();
    messages[1] = item("fold-target", 2, "beta, rewritten");
    let refused = transform(
        &s,
        &quiet(&messages, "cfg0"),
        &live_run("/nonexistent-docs", 0),
    );
    assert!(
        matches!(refused, Err(TransformError::IdentityDrift(ref mid)) if mid == "fold-target"),
        "{refused:?}"
    );
    let after = s.load(SESSION).unwrap();
    assert_eq!(after.row_version, before.row_version);
    assert_eq!(rendered(&s), 2);
}

#[test]
fn a_project_memory_revision_is_a_hard_member_too() {
    let dir = tempfile::tempdir().unwrap();
    let (s, messages) = pending(dir.path());
    let mut ctx = live_run("/nonexistent-docs", 0);
    ctx.project_memory = canonical_read(9, &[("mem_rule", "PROJECT_RULES", "A new rule.")]);
    let response = transform(&s, &quiet(&messages, "cfg0"), &ctx).unwrap();
    assert_eq!(served(&response), ("HARD", Some("project_memory_epoch")));
    assert_eq!(rendered(&s), 3);
}

/// The expressions each pass-planning path computed inline before `activation_gates` owned them.
fn ordinary_path_before(bits: [bool; 12], pass: scheduler::PassDecision) -> (bool, bool) {
    let [
        active,
        initialized,
        flush,
        render,
        first,
        recut,
        ttl,
        absorb,
        external,
        epoch,
        _,
        reconcile,
    ] = bits;
    let emergency = matches!(
        pass,
        scheduler::PassDecision::Force85 | scheduler::PassDecision::Emergency95
    ) || bits[10];
    let hard = first || recut || ttl || absorb || external || epoch;
    let veto = active
        && pass == scheduler::PassDecision::Execute
        && !hard
        && !emergency
        && !flush
        && !render
        && !reconcile
        && initialized;
    (hard, veto)
}

fn additive_path_before(bits: [bool; 12], pass: scheduler::PassDecision) -> (bool, bool) {
    let [
        active,
        initialized,
        flush,
        render,
        _,
        _,
        ttl,
        _,
        external,
        epoch,
        _,
        _,
    ] = bits;
    let hard = ttl || external || epoch;
    let veto = active
        && pass == scheduler::PassDecision::Execute
        && !hard
        && !flush
        && !render
        && initialized;
    (hard, veto)
}

#[test]
fn the_shared_gate_matches_both_paths_before_extraction_for_every_input() {
    use scheduler::PassDecision::*;
    for pass in [Defer, Execute, Force85, Emergency95] {
        for mask in 0u32..1 << 12 {
            let bits: [bool; 12] = std::array::from_fn(|index| mask & (1 << index) != 0);
            let [
                active,
                initialized,
                flush,
                render,
                first,
                recut,
                ttl,
                absorb,
                external,
                epoch,
                latch,
                reconcile,
            ] = bits;
            let ordinary = activation_gates(&ActivationGateInputs {
                pass,
                history_summarizer_active: active,
                initialized,
                soft_refresh_pending: flush,
                render_config_changed: render,
                first_fold_due: first,
                boundary_divergence_recut: recut,
                idle_ttl_fired: ttl,
                system_absorb_hard_due: absorb,
                external_revision_changed: external,
                project_memory_epoch_hard_due: epoch,
                emergency_arm_engaged: matches!(pass, Force85 | Emergency95) || latch,
                reconcile_hard_due: reconcile,
            });
            assert_eq!(
                (
                    ordinary.hard_fold_requested,
                    ordinary.ordinary_history_summarizer_veto
                ),
                ordinary_path_before(bits, pass),
                "ordinary {pass:?} {mask:#b}"
            );
            let additive = activation_gates(&ActivationGateInputs {
                pass,
                history_summarizer_active: active,
                initialized,
                soft_refresh_pending: flush,
                render_config_changed: render,
                first_fold_due: false,
                boundary_divergence_recut: false,
                idle_ttl_fired: ttl,
                system_absorb_hard_due: false,
                external_revision_changed: external,
                project_memory_epoch_hard_due: epoch,
                emergency_arm_engaged: false,
                reconcile_hard_due: false,
            });
            assert_eq!(
                (
                    additive.hard_fold_requested,
                    additive.ordinary_history_summarizer_veto
                ),
                additive_path_before(bits, pass),
                "additive {pass:?} {mask:#b}"
            );
        }
    }
}
