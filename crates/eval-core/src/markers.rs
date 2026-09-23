use std::collections::BTreeSet;

/// One hard case's coverage witness: the test at `test` records the marker only
/// after asserting the case's preconditions, never the invariant itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub name: &'static str,
    pub test: &'static str,
}

/// Every marker the evaluator's suites may record, globally unique by name.
pub const MARKERS: [Marker; 59] = [
    Marker {
        name: "ing_four_seam_hold_correct_release_query",
        test: "crates/daemon/tests/eval_ingestion.rs::hold_embedding_commit_correction_release_query_makes_the_predecessor_obsolete",
    },
    Marker {
        name: "ing_adapter_round_trip_expected_equals_published_plus_refused",
        test: "crates/daemon/tests/eval_ingestion.rs::rendered_worlds_round_trip_through_the_opencode_adapter_with_exact_accounting",
    },
    Marker {
        name: "ing_generated_observations_never_lead",
        test: "crates/daemon/tests/eval_ingestion.rs::generated_observations_never_lead_and_a_boundary_fixture_refuses",
    },
    Marker {
        name: "ing_git_units_fixed_revision_time_from_projection",
        test: "crates/daemon/tests/eval_ingestion.rs::git_units_keep_revision_one_and_take_valid_time_from_the_projection",
    },
    Marker {
        name: "ing_observation_time_inert",
        test: "crates/daemon/tests/eval_ingestion.rs::observation_time_is_inert_for_identity_and_eligibility",
    },
    Marker {
        name: "ldg_injection_exact_page_bound",
        test: "crates/daemon/tests/eval_ledger.rs::exact_page_bound_loses_the_rule_at_the_exact_lane",
    },
    Marker {
        name: "ldg_injection_lexical_accepted_bound",
        test: "crates/daemon/tests/eval_ledger.rs::lexical_accepted_bound_loses_the_rule_at_the_lexical_lane",
    },
    Marker {
        name: "ldg_injection_dense_k_bound",
        test: "crates/daemon/tests/eval_ledger.rs::dense_k_bound_loses_the_rule_at_the_dense_lane",
    },
    Marker {
        name: "ldg_injection_eligibility_retracted",
        test: "crates/daemon/tests/eval_ledger.rs::a_retired_object_loses_the_rule_at_eligibility",
    },
    Marker {
        name: "ldg_injection_fusion_union_bound",
        test: "crates/daemon/tests/eval_ledger.rs::fused_union_bound_loses_the_rule_at_fusion",
    },
    Marker {
        name: "ldg_injection_selection_result_rows",
        test: "crates/daemon/tests/eval_ledger.rs::result_rows_bound_loses_the_rule_at_selection",
    },
    Marker {
        name: "ldg_injection_packing_skipped",
        test: "crates/daemon/tests/eval_ledger.rs::optional_budget_loses_the_rule_at_packing",
    },
    Marker {
        name: "sls_injection_candidate_window",
        test: "crates/daemon/tests/eval_surface_ledger.rs::an_old_segment_outside_the_window_is_lost_at_the_candidate_window",
    },
    Marker {
        name: "sls_injection_prompt_gate",
        test: "crates/daemon/tests/eval_surface_ledger.rs::a_short_prompt_is_lost_at_the_length_gate",
    },
    Marker {
        name: "sls_injection_score_threshold",
        test: "crates/daemon/tests/eval_surface_ledger.rs::a_raised_threshold_is_lost_at_the_threshold",
    },
    Marker {
        name: "sls_injection_result_cap",
        test: "crates/daemon/tests/eval_surface_ledger.rs::a_fourth_match_is_lost_at_the_cap",
    },
    Marker {
        name: "sls_injection_attachment",
        test: "crates/daemon/tests/eval_surface_ledger.rs::a_native_array_without_the_tail_is_lost_at_attachment",
    },
    Marker {
        name: "rid_capabilities_read_during_cassette_run",
        test: "crates/daemon/tests/eval_cassette.rs::replay_preserves_the_transcript_and_the_declarations",
    },
    Marker {
        name: "rid_rust_cassette_miss_constructed",
        test: "crates/daemon/tests/eval_cassette.rs::one_byte_in_each_covered_field_misses_and_the_dropped_fields_do_not",
    },
    Marker {
        name: "rid_cassette_namespace_refused",
        test: "crates/daemon/tests/eval_cassette.rs::a_regenerated_frame_or_another_namespace_refuses_before_any_request",
    },
    Marker {
        name: "rid_reviewer_cassette_miss_reached",
        test: "crates/daemon/tests/eval_cassette.rs::memory_reviewer_replays_through_the_keyed_peer",
    },
    Marker {
        name: "wm_pair_set_aged_history_spans_median",
        test: "crates/eval-core/tests/pairs.rs::a_pair_set_carries_a_falsification_pair_and_a_natural_fresh_control",
    },
    Marker {
        name: "wm_baseline_ran_on_falsification_pair",
        test: "crates/eval-core/tests/pairs.rs::the_recency_baseline_misses_every_falsifier_or_blocks_suite_b",
    },
    Marker {
        name: "wm_baseline_ran_on_positive_control_pair",
        test: "crates/eval-core/tests/pairs.rs::the_recency_baseline_delivers_a_positive_control_or_is_vacuous",
    },
    Marker {
        name: "mtr_injection_mediation_boundary_observed",
        test: "crates/eval-core/tests/injection.rs::obedience_is_the_observed_side_effect_and_echo_is_only_exposure",
    },
    Marker {
        name: "mtr_injection_model_output_observed",
        test: "crates/eval-core/tests/injection.rs::obedience_is_the_observed_side_effect_and_echo_is_only_exposure",
    },
    Marker {
        name: "mtr_second_session_read_memory",
        test: "crates/eval-core/tests/injection.rs::a_canary_written_into_memory_surfaces_in_a_later_session_that_read_it",
    },
    Marker {
        name: "flt_quiescence_receipt_all_zero",
        test: "crates/daemon/tests/eval_aging.rs::a_quiescent_copy_resumes_the_full_replay_in_one_incarnation",
    },
    Marker {
        name: "ing_aged_arm_restarted_between_sessions",
        test: "crates/daemon/tests/eval_aging.rs::a_quiescent_copy_resumes_the_full_replay_in_one_incarnation",
    },
    Marker {
        name: "ing_window_has_pre_snapshot_supersession",
        test: "crates/daemon/tests/eval_aging.rs::a_quiescent_copy_resumes_the_full_replay_in_one_incarnation",
    },
    Marker {
        name: "ing_window_has_pre_snapshot_retirement",
        test: "crates/daemon/tests/eval_aging.rs::a_quiescent_copy_resumes_the_full_replay_in_one_incarnation",
    },
    Marker {
        name: "flt_copy_attempted_mid_episode",
        test: "crates/daemon/tests/eval_aging.rs::a_copy_with_pending_work_is_refused_by_the_counter_it_left",
    },
    Marker {
        name: "flt_checkpoint_observed_busy",
        test: "crates/daemon/tests/eval_aging.rs::a_reader_holding_the_projection_leaves_the_checkpoint_busy",
    },
    Marker {
        name: "sls_memstore_copy_refused_live_handle",
        test: "crates/daemon/tests/eval_aging.rs::a_copy_beside_a_live_memory_store_handle_is_refused",
    },
    Marker {
        name: "flt_prefix_history_slipped",
        test: "crates/daemon/tests/eval_aging.rs::a_quiescent_copy_resumes_the_full_replay_in_one_incarnation",
    },
    Marker {
        name: "flt_foreign_incarnation_refused_at_reopen",
        test: "crates/daemon/tests/eval_aging.rs::a_foreign_incarnation_is_refused_at_reopen",
    },
    Marker {
        name: "wm_bulk_scaffold_presented_as_aged",
        test: "crates/eval-core/tests/manifest.rs::a_prefix_then_generate_run_is_replay_built_only",
    },
    Marker {
        name: "flt_premature_success_fixture_refused",
        test: "crates/eval-core/tests/fault.rs::a_premature_success_fixture_is_refused",
    },
    Marker {
        name: "flt_incomplete_coverage_named_not_pass",
        test: "crates/eval-core/tests/fault.rs::a_missing_receipt_is_incomplete_coverage_not_pass",
    },
    Marker {
        name: "flt_crash_model_label_refused",
        test: "crates/eval-core/tests/fault.rs::a_power_loss_label_and_a_host_kill_are_refused",
    },
    Marker {
        name: "flt_liveness_unmet_named_at_bound",
        test: "crates/eval-core/tests/fault.rs::liveness_is_unmet_at_the_bound_or_when_a_fault_healed",
    },
    Marker {
        name: "flt_lost_reply_unknown_until_readback",
        test: "crates/daemon/tests/eval_fault.rs::a_lost_reply_stays_unknown_until_readback_at_after_recovery",
    },
    Marker {
        name: "flt_every_declared_cut_receipted",
        test: "crates/daemon/tests/eval_fault.rs::the_fault_campaign_receipts_every_declared_cut",
    },
    Marker {
        name: "flt_kill_barrier_read_before_kill",
        test: "crates/daemon/tests/eval_fault.rs::a_test_binary_child_killed_at_a_named_cut_recovers",
    },
    Marker {
        name: "flt_r11_recorded_as_expected_refusal",
        test: "crates/daemon/tests/eval_fault.rs::deletion_bearing_catch_up_is_an_expected_refusal_and_a_permanent_stall",
    },
    Marker {
        name: "flt_r24_recorded_as_expected_refusal",
        test: "crates/daemon/tests/eval_fault.rs::receipt_quota_exhaustion_is_an_expected_refusal",
    },
    Marker {
        name: "flt_liveness_bounds_met_with_faults_armed",
        test: "crates/daemon/tests/eval_fault.rs::liveness_bounds_are_met_with_outside_core_faults_armed",
    },
    Marker {
        name: "flt_corruption_detected_at_quiescence",
        test: "crates/daemon/tests/eval_fault.rs::a_corrupted_quiescent_file_is_detected_before_any_store_opens",
    },
    Marker {
        name: "flt_external_lock_holder_released",
        test: "crates/daemon/tests/eval_fault.rs::an_external_lock_holder_blocks_then_releases",
    },
    Marker {
        name: "flt_artifact_fault_named_errno",
        test: "crates/daemon/tests/eval_fault.rs::artifact_faults_fail_with_their_named_errno_and_heal_by_reopen_or_consumption",
    },
    Marker {
        name: "sls_embedding_publication_held_then_released",
        test: "crates/daemon/tests/eval_fault.rs::a_held_publication_admits_once_and_publishes_on_release",
    },
    Marker {
        name: "flt_leak_ledger_sampled_before_reopen",
        test: "crates/daemon/tests/eval_growth.rs::a_never_restored_campaign_samples_every_quiescence_and_refuses_a_restore",
    },
    Marker {
        name: "flt_restore_under_never_restored_refused",
        test: "crates/eval-core/tests/growth.rs::a_restore_under_never_restored_is_refused_and_a_restoring_ledger_gives_no_leak_verdict",
    },
    Marker {
        name: "flt_headroom_accounted_from_store_constants",
        test: "crates/daemon/tests/eval_growth.rs::reviewer_headroom_is_accounted_from_the_stores_own_constants",
    },
    Marker {
        name: "xc_envelope_breach_stops_the_run",
        test: "crates/daemon/tests/eval_growth.rs::a_deliberate_envelope_breach_names_the_resource_and_publishes_nothing",
    },
    Marker {
        name: "xc_parallel_campaigns_isolated",
        test: "crates/daemon/tests/eval_growth.rs::two_concurrent_campaigns_on_one_checkout_match_their_serial_digests",
    },
    Marker {
        name: "xc_shared_fixture_refused",
        test: "crates/eval-core/tests/growth.rs::a_shared_root_namespace_or_port_is_refused",
    },
    Marker {
        name: "flt_swarm_mix_complete",
        test: "crates/daemon/tests/eval_growth.rs::the_swarm_mix_exercises_every_operation_kind",
    },
    Marker {
        name: "flt_incomplete_mix_not_success",
        test: "crates/eval-core/tests/growth.rs::a_mix_missing_an_operation_is_not_sustainability_success",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageError {
    Unregistered(String),
    Incomplete {
        missing: BTreeSet<&'static str>,
    },
    /// No registered marker's test path starts with the suite prefix.
    EmptySuite(String),
}

debug_display!(CoverageError);

/// The markers one run has witnessed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    fired: BTreeSet<&'static str>,
}

impl Coverage {
    pub fn record(&mut self, name: &str) -> Result<(), CoverageError> {
        let marker = MARKERS
            .iter()
            .find(|marker| marker.name == name)
            .ok_or_else(|| CoverageError::Unregistered(name.to_string()))?;
        self.fired.insert(marker.name);
        Ok(())
    }

    pub fn fired(&self) -> &BTreeSet<&'static str> {
        &self.fired
    }

    /// `Ok` only when every registered marker whose test path starts with
    /// `suite` fired; a missing entry is `Incomplete`, never a pass, and a
    /// prefix that selects no marker is `EmptySuite`. An empty `suite` names
    /// the whole registry.
    pub fn complete(&self, suite: &str) -> Result<(), CoverageError> {
        let owned: Vec<&'static str> = MARKERS
            .iter()
            .filter(|marker| marker.test.starts_with(suite))
            .map(|marker| marker.name)
            .collect();
        if owned.is_empty() {
            return Err(CoverageError::EmptySuite(suite.to_string()));
        }
        let missing: BTreeSet<&'static str> = owned
            .into_iter()
            .filter(|name| !self.fired.contains(name))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CoverageError::Incomplete { missing })
        }
    }
}
