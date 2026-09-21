use std::collections::BTreeSet;

/// One hard case's coverage witness: the test at `test` records the marker only
/// after asserting the case's preconditions, never the invariant itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub name: &'static str,
    pub test: &'static str,
}

/// Every marker the evaluator's suites may record, globally unique by name.
pub const MARKERS: [Marker; 21] = [
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
