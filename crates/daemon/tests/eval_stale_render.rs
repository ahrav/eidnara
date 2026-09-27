//! The stale-preference export reads a served segment's tier by matching its
//! body against the stored tiers as the daemon's renderer serves them: this
//! renders one segment at every tier through `render_history_segment_at_tier`
//! and checks `export_capture` names that tier, with markup and a
//! heading-shaped line in every tier's text.

#![cfg(feature = "test-support")]

use std::collections::BTreeMap;

use daemon::decay_render::{DecayRenderHistorySegment, render_history_segment_at_tier};
use eval_core::{
    M1_PLACEHOLDER, STALE_CAPTURE_SCHEMA, SegmentTiers, StaleCapture, export_capture, fact_world,
};
use serde_json::json;

#[test]
fn the_export_reads_the_tier_the_daemon_rendered() {
    let world = fact_world(0x5EED_B000_0000_0002, 1);
    let pair = &world.pairs[0];
    let tier_text = |tier: u8| {
        format!(
            "Tier {tier}: <b> & {} set to {}.\n## not a heading",
            pair.subject, pair.stale_value
        )
    };
    let stored = DecayRenderHistorySegment {
        start_message: 1,
        end_message: 2,
        title: "Ports & <limits>".to_string(),
        content: tier_text(1),
        p1: Some(tier_text(1)),
        p2: Some(tier_text(2)),
        p3: Some(tier_text(3)),
        p4: Some(tier_text(4)),
        importance: Some(50),
        legacy: Some(0),
        ..DecayRenderHistorySegment::default()
    };
    let row = SegmentTiers {
        start_message: 1,
        end_message: 2,
        p1: stored.p1.clone(),
        p2: stored.p2.clone(),
        p3: stored.p3.clone(),
        p4: stored.p4.clone(),
    };
    for tier in 1..=4u8 {
        let history = format!(
            "<session-history>\n{}\n</session-history>",
            render_history_segment_at_tier(&stored, tier)
        );
        let request = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": history}, {"type": "text", "text": M1_PLACEHOLDER}]},
        ]});
        let capture = StaleCapture {
            schema: STALE_CAPTURE_SCHEMA.to_string(),
            harness: "opencode".to_string(),
            summarizer: "fixture/scripted".to_string(),
            requests: BTreeMap::from([(pair.task.clone(), request)]),
            world: world.clone(),
            segments: vec![row.clone()],
        };
        let export = export_capture(&capture).unwrap();
        assert_eq!(export.pairs[0].stale_tier, tier, "{history}");
    }
}

/// The production precedence sentence is the one M0 measured as arm (b).
#[test]
fn the_served_precedence_sentence_is_the_measured_one() {
    assert_eq!(
        daemon::decay_render::PRECEDENCE_SENTENCE,
        eval_core::PRECEDENCE_SENTENCE
    );
}
