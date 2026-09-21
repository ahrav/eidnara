use std::collections::BTreeSet;

use eval_core::{
    CHAIN_STAGES, ChainStage, Completed, Evidence, Ledger, LedgerError,
    MAX_CANDIDATES_PER_STAGE_OBSERVATION, Observation, Presence, Required, Stage, StageKind,
    StageVerdict,
};

const RULE: &str = "rule";
const OTHER: &str = "other";
const STALE: &str = "stale";
const THROUGH: ChainStage = ChainStage::Packing;

fn ids(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn observe(stage: ChainStage, names: &[&str]) -> Observation<ChainStage> {
    Observation::new(stage, 0, Some(1), ids(names)).unwrap()
}

fn ledger(observations: impl IntoIterator<Item = Observation<ChainStage>>) -> Ledger<ChainStage> {
    let mut ledger = Ledger::default();
    for observation in observations {
        ledger.observe(observation);
    }
    ledger
}

fn required(entry: ChainStage) -> Vec<Required<ChainStage>> {
    vec![Required {
        occurrence: RULE.to_string(),
        entry,
    }]
}

fn judge(ledger: &Ledger<ChainStage>, entry: ChainStage) -> StageVerdict<ChainStage> {
    ledger.verdict(&required(entry), &BTreeSet::new(), THROUGH)
}

fn clean_run() -> Vec<Observation<ChainStage>> {
    CHAIN_STAGES
        .iter()
        .map(|stage| observe(*stage, &[RULE, OTHER]))
        .collect()
}

/// `clean_run` with `dropped` losing the rule.
fn run_dropping(dropped: &[ChainStage]) -> Vec<Observation<ChainStage>> {
    CHAIN_STAGES
        .iter()
        .map(|stage| {
            if dropped.contains(stage) {
                observe(*stage, &[OTHER])
            } else {
                observe(*stage, &[RULE, OTHER])
            }
        })
        .collect()
}

#[test]
fn the_chain_stages_are_pinned_in_production_order_with_their_kinds() {
    assert_eq!(<ChainStage as Stage>::ALL, &CHAIN_STAGES);
    assert_eq!(
        <ChainStage as Stage>::REACHABILITY,
        eval_core::Reachability::TestOnly
    );
    assert_eq!(
        <eval_core::Surface1Stage as Stage>::REACHABILITY,
        eval_core::Reachability::DefaultProduction
    );
    assert_eq!(
        <eval_core::Surface1Stage as Stage>::ALL,
        &eval_core::SURFACE1_STAGES
    );
    assert!(
        eval_core::SURFACE1_STAGES
            .iter()
            .all(|stage| stage.kind() == StageKind::Filter)
    );
    let kinds: Vec<(ChainStage, StageKind, usize)> = CHAIN_STAGES
        .iter()
        .map(|stage| (*stage, stage.kind(), stage.ordinal()))
        .collect();
    assert_eq!(
        kinds,
        [
            (ChainStage::Exact, StageKind::Source, 0),
            (ChainStage::Lexical, StageKind::Source, 1),
            (ChainStage::Dense, StageKind::Source, 2),
            (ChainStage::Eligibility, StageKind::Filter, 3),
            (ChainStage::Fusion, StageKind::Filter, 4),
            (ChainStage::Selection, StageKind::Filter, 5),
            (ChainStage::Packing, StageKind::Filter, 6),
        ]
    );
    let names: Vec<String> = CHAIN_STAGES
        .iter()
        .map(|stage| serde_json::to_string(stage).unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "\"exact\"",
            "\"lexical\"",
            "\"dense\"",
            "\"eligibility\"",
            "\"fusion\"",
            "\"selection\"",
            "\"packing\""
        ]
    );
}

#[test]
fn stage_presence_is_three_valued_and_an_unjoinable_stage_has_none() {
    let ledger = ledger([
        observe(ChainStage::Exact, &[RULE]),
        observe(ChainStage::Eligibility, &[OTHER]),
        Observation::unjoinable(ChainStage::Fusion, 0, Some(1)),
    ]);
    assert_eq!(
        ledger.presence(ChainStage::Exact, RULE),
        Some(Presence::Reached)
    );
    assert_eq!(
        ledger.presence(ChainStage::Eligibility, RULE),
        Some(Presence::ReachedEvidenceAbsent)
    );
    assert_eq!(
        ledger.presence(ChainStage::Selection, RULE),
        Some(Presence::NotReached)
    );
    assert_eq!(ledger.presence(ChainStage::Fusion, RULE), None);
}

#[test]
fn a_full_run_that_keeps_the_rule_through_the_terminal_stage_is_clean() {
    let ledger = ledger(clean_run());
    for entry in [ChainStage::Exact, ChainStage::Lexical, ChainStage::Dense] {
        assert_eq!(judge(&ledger, entry), StageVerdict::Clean);
    }
}

#[test]
fn the_first_loss_is_the_earliest_absent_stage_on_the_entry_path() {
    let cases: [(ChainStage, ChainStage, StageVerdict<ChainStage>); 9] = [
        (
            ChainStage::Exact,
            ChainStage::Exact,
            StageVerdict::FirstLoss(ChainStage::Exact),
        ),
        (ChainStage::Exact, ChainStage::Lexical, StageVerdict::Clean),
        (ChainStage::Exact, ChainStage::Dense, StageVerdict::Clean),
        (
            ChainStage::Exact,
            ChainStage::Eligibility,
            StageVerdict::FirstLoss(ChainStage::Eligibility),
        ),
        (
            ChainStage::Exact,
            ChainStage::Packing,
            StageVerdict::FirstLoss(ChainStage::Packing),
        ),
        (ChainStage::Dense, ChainStage::Exact, StageVerdict::Clean),
        (
            ChainStage::Dense,
            ChainStage::Dense,
            StageVerdict::FirstLoss(ChainStage::Dense),
        ),
        (
            ChainStage::Lexical,
            ChainStage::Fusion,
            StageVerdict::FirstLoss(ChainStage::Fusion),
        ),
        (
            ChainStage::Lexical,
            ChainStage::Selection,
            StageVerdict::FirstLoss(ChainStage::Selection),
        ),
    ];
    for (entry, dropped, expected) in cases {
        assert_eq!(
            judge(&ledger(run_dropping(&[dropped])), entry),
            expected,
            "entry {entry:?}, dropped at {dropped:?}"
        );
    }
}

#[test]
fn the_earliest_loss_wins_and_later_absences_are_not_a_second_verdict() {
    let observations = run_dropping(&[
        ChainStage::Fusion,
        ChainStage::Selection,
        ChainStage::Packing,
    ]);
    assert_eq!(
        judge(&ledger(observations), ChainStage::Exact),
        StageVerdict::FirstLoss(ChainStage::Fusion)
    );
}

#[test]
fn an_unreached_entry_is_indeterminate_not_a_loss_downstream() {
    let observations = CHAIN_STAGES
        .iter()
        .filter(|stage| **stage != ChainStage::Dense)
        .map(|stage| observe(*stage, &[OTHER]));
    assert_eq!(
        judge(&ledger(observations), ChainStage::Dense),
        StageVerdict::Indeterminate
    );
}

#[test]
fn a_chain_that_never_reaches_the_terminal_stage_is_indeterminate_not_clean() {
    let ledger = ledger(
        CHAIN_STAGES
            .iter()
            .take(4)
            .map(|stage| observe(*stage, &[RULE])),
    );
    assert_eq!(
        judge(&ledger, ChainStage::Exact),
        StageVerdict::Indeterminate
    );
    assert_eq!(
        ledger.verdict(
            &required(ChainStage::Exact),
            &BTreeSet::new(),
            ChainStage::Eligibility
        ),
        StageVerdict::Clean,
        "the same observations are clean through the stage they did reach"
    );
    let nothing_required: Vec<Required<ChainStage>> = Vec::new();
    assert_eq!(
        ledger.verdict(&nothing_required, &BTreeSet::new(), THROUGH),
        StageVerdict::Indeterminate,
        "an unreached terminal is not clean when nothing is required either"
    );
    assert_eq!(
        Ledger::<ChainStage>::default().verdict(&nothing_required, &BTreeSet::new(), THROUGH),
        StageVerdict::Indeterminate,
        "an empty ledger certifies nothing"
    );
    assert_eq!(
        ledger.verdict(&nothing_required, &ids(&[STALE]), THROUGH),
        StageVerdict::Indeterminate,
        "a stale occurrence cannot be cleared through a terminal that was not reached"
    );
    let mut opaque_terminal = Ledger::default();
    for observation in clean_run() {
        opaque_terminal.observe(match observation.stage() {
            ChainStage::Packing => Observation::unjoinable(ChainStage::Packing, 0, Some(1)),
            _ => observation,
        });
    }
    assert_eq!(
        opaque_terminal.verdict(&nothing_required, &BTreeSet::new(), THROUGH),
        StageVerdict::Indeterminate,
        "an unjoinable terminal certifies nothing"
    );
}

/// The shell may leave a filter unobserved when a refusal discards its output
/// and the next stage's empty output stands in; only an unjoinable stage
/// blocks attribution.
#[test]
fn an_unobserved_filter_between_observed_stages_is_transparent() {
    let observations = [
        observe(ChainStage::Exact, &[RULE]),
        observe(ChainStage::Eligibility, &[RULE]),
        observe(ChainStage::Selection, &[]),
    ];
    assert_eq!(
        ledger(observations).verdict(
            &required(ChainStage::Exact),
            &BTreeSet::new(),
            ChainStage::Selection
        ),
        StageVerdict::FirstLoss(ChainStage::Selection)
    );
}

#[test]
fn a_stage_that_ends_the_request_reports_an_empty_output() {
    let observations = [
        observe(ChainStage::Exact, &[RULE, OTHER]),
        observe(ChainStage::Eligibility, &[RULE, OTHER]),
        observe(ChainStage::Fusion, &[]),
    ];
    assert_eq!(
        judge(&ledger(observations), ChainStage::Exact),
        StageVerdict::FirstLoss(ChainStage::Fusion)
    );
}

#[test]
fn an_unjoinable_stage_is_never_evidence_absence() {
    let unjoinable = |observations: &mut Vec<Observation<ChainStage>>| {
        observations[3] = Observation::unjoinable(ChainStage::Eligibility, 0, Some(1));
    };
    let mut kept = clean_run();
    unjoinable(&mut kept);
    assert_eq!(
        judge(&ledger(kept), ChainStage::Exact),
        StageVerdict::Clean,
        "a rule present after the unjoinable stage was not lost there"
    );

    let mut lost_after = run_dropping(&[ChainStage::Selection, ChainStage::Packing]);
    unjoinable(&mut lost_after);
    assert_eq!(
        judge(&ledger(lost_after), ChainStage::Exact),
        StageVerdict::FirstLoss(ChainStage::Selection),
        "presence at fusion proves the rule passed the unjoinable stage"
    );

    let mut lost_behind = run_dropping(&[
        ChainStage::Fusion,
        ChainStage::Selection,
        ChainStage::Packing,
    ]);
    unjoinable(&mut lost_behind);
    assert_eq!(
        judge(&ledger(lost_behind), ChainStage::Exact),
        StageVerdict::Indeterminate,
        "a loss right behind an unjoinable stage cannot be placed"
    );

    let trailing = ledger([
        observe(ChainStage::Exact, &[RULE]),
        observe(ChainStage::Eligibility, &[RULE]),
        Observation::unjoinable(ChainStage::Fusion, 0, Some(1)),
    ]);
    assert_eq!(
        judge(&trailing, ChainStage::Exact),
        StageVerdict::Indeterminate,
        "an unjoinable last stage is neither a loss nor clean"
    );
}

#[test]
fn every_permutation_of_the_observations_folds_the_same() {
    let base = [
        observe(ChainStage::Exact, &[RULE, OTHER]),
        Observation::new(ChainStage::Eligibility, 0, Some(1), ids(&[RULE])).unwrap(),
        Observation::new(ChainStage::Eligibility, 1, Some(1), ids(&[RULE, OTHER])).unwrap(),
        observe(ChainStage::Fusion, &[RULE, OTHER]),
        observe(ChainStage::Selection, &[OTHER]),
        observe(ChainStage::Selection, &[RULE, OTHER]),
    ];
    let reference = ledger(base.clone());
    let expected = judge(&reference, ChainStage::Exact);
    assert_eq!(
        expected,
        StageVerdict::Indeterminate,
        "the two selection observations contradict"
    );
    assert_eq!(reference.presence(ChainStage::Selection, RULE), None);
    let mut indices: Vec<usize> = (0..base.len()).collect();
    let mut seen = 0;
    permutations(&mut indices, 0, &mut |order| {
        seen += 1;
        let permuted = ledger(order.iter().map(|index| base[*index].clone()));
        assert_eq!(
            permuted.presence(ChainStage::Selection, RULE),
            None,
            "{order:?}"
        );
        assert_eq!(judge(&permuted, ChainStage::Exact), expected, "{order:?}");
        for stage in [
            ChainStage::Exact,
            ChainStage::Eligibility,
            ChainStage::Fusion,
        ] {
            assert_eq!(
                permuted.presence(stage, RULE),
                reference.presence(stage, RULE),
                "{order:?}"
            );
        }
    });
    assert_eq!(seen, 720);

    let distinct = ledger(base[..5].iter().cloned());
    let expected = judge(&distinct, ChainStage::Exact);
    assert_eq!(
        expected,
        StageVerdict::FirstLoss(ChainStage::Selection),
        "a loss before an unreached terminal stage is still a loss"
    );
    let mut indices: Vec<usize> = (0..5).collect();
    permutations(&mut indices, 0, &mut |order| {
        let permuted = ledger(order.iter().map(|index| base[*index].clone()));
        assert_eq!(permuted, distinct, "{order:?}");
        assert_eq!(judge(&permuted, ChainStage::Exact), expected, "{order:?}");
    });
}

fn permutations(items: &mut Vec<usize>, start: usize, visit: &mut impl FnMut(&[usize])) {
    if start == items.len() {
        visit(items);
        return;
    }
    for index in start..items.len() {
        items.swap(start, index);
        permutations(items, start + 1, visit);
        items.swap(start, index);
    }
}

#[test]
fn the_highest_sequence_is_the_stage_output() {
    let ledger = ledger([
        Observation::new(ChainStage::Selection, 0, Some(1), ids(&[RULE, OTHER])).unwrap(),
        Observation::new(ChainStage::Selection, 1, Some(1), ids(&[OTHER])).unwrap(),
    ]);
    assert_eq!(
        ledger.presence(ChainStage::Selection, RULE),
        Some(Presence::ReachedEvidenceAbsent)
    );
    assert_eq!(ledger.observations().count(), 2);
}

#[test]
fn identical_duplicates_are_idempotent_and_contradictions_are_indeterminate() {
    let mut idempotent = ledger(clean_run());
    let before = idempotent.clone();
    idempotent.observe(observe(ChainStage::Fusion, &[RULE, OTHER]));
    idempotent.observe(observe(ChainStage::Fusion, &[RULE, OTHER]));
    assert_eq!(idempotent, before);
    assert_eq!(judge(&idempotent, ChainStage::Exact), StageVerdict::Clean);

    for order in [
        [&[RULE, OTHER][..], &[OTHER][..]],
        [&[OTHER], &[RULE, OTHER]],
    ] {
        let mut contradicted = ledger(run_dropping(&[ChainStage::Fusion]));
        contradicted.observe(observe(ChainStage::Fusion, order[0]));
        contradicted.observe(observe(ChainStage::Fusion, order[1]));
        assert_eq!(
            judge(&contradicted, ChainStage::Exact),
            StageVerdict::Indeterminate
        );
        assert_eq!(
            contradicted.presence(ChainStage::Fusion, RULE),
            None,
            "a contradicted stage reads as unjoinable in either order"
        );
    }
}

#[test]
fn a_fold_accepts_exactly_one_commit_read_incarnation() {
    let mut observations = clean_run();
    observations[5] =
        Observation::new(ChainStage::Selection, 0, Some(2), ids(&[RULE, OTHER])).unwrap();
    let two = ledger(observations);
    assert_eq!(two.incarnations(), BTreeSet::from([1, 2]));
    assert_eq!(judge(&two, ChainStage::Exact), StageVerdict::Indeterminate);

    let mut untagged = clean_run();
    untagged[0] = Observation::new(ChainStage::Exact, 0, None, ids(&[RULE, OTHER])).unwrap();
    untagged[6] = Observation::new(ChainStage::Packing, 0, None, ids(&[RULE, OTHER])).unwrap();
    let one = ledger(untagged);
    assert_eq!(one.incarnations(), BTreeSet::from([1]));
    assert_eq!(
        judge(&one, ChainStage::Exact),
        StageVerdict::Clean,
        "a return that carries no incarnation asserts nothing about it"
    );
}

#[test]
fn a_delivered_stale_occurrence_names_the_stage_it_entered() {
    let stale = ids(&[STALE]);
    let judge_stale =
        |ledger: &Ledger<ChainStage>| ledger.verdict(&required(ChainStage::Exact), &stale, THROUGH);
    let delivered = ledger(CHAIN_STAGES.iter().map(|stage| match stage {
        ChainStage::Exact | ChainStage::Lexical | ChainStage::Dense => observe(*stage, &[RULE]),
        _ => observe(*stage, &[RULE, STALE]),
    }));
    assert_eq!(
        judge_stale(&delivered),
        StageVerdict::StaleIngress(ChainStage::Eligibility)
    );

    let filtered = ledger(CHAIN_STAGES.iter().map(|stage| match stage {
        ChainStage::Exact => observe(*stage, &[RULE, STALE]),
        _ => observe(*stage, &[RULE]),
    }));
    assert_eq!(
        judge_stale(&filtered),
        StageVerdict::Clean,
        "stale evidence the chain removed before the terminal stage was not served"
    );

    let both = ledger(CHAIN_STAGES.iter().map(|stage| match stage {
        ChainStage::Selection | ChainStage::Packing => observe(*stage, &[STALE]),
        _ => observe(*stage, &[RULE, STALE]),
    }));
    assert_eq!(
        judge_stale(&both),
        StageVerdict::StaleIngress(ChainStage::Exact),
        "the earlier ordinal is named when both a loss and an ingress exist"
    );

    let tie = ledger(CHAIN_STAGES.iter().map(|stage| match stage {
        ChainStage::Selection | ChainStage::Packing => observe(*stage, &[STALE]),
        _ => observe(*stage, &[RULE]),
    }));
    assert_eq!(
        judge_stale(&tie),
        StageVerdict::FirstLoss(ChainStage::Selection),
        "at equal ordinals the loss is named"
    );

    let mut opaque_terminal = clean_run();
    opaque_terminal[3] = observe(ChainStage::Eligibility, &[RULE, OTHER, STALE]);
    opaque_terminal[6] = Observation::unjoinable(ChainStage::Packing, 0, Some(1));
    assert_eq!(
        judge_stale(&ledger(opaque_terminal)),
        StageVerdict::Indeterminate,
        "a stale occurrence seen before an unjoinable terminal stage cannot be cleared"
    );

    let mut behind_opaque = clean_run();
    behind_opaque[3] = Observation::unjoinable(ChainStage::Eligibility, 0, Some(1));
    behind_opaque[4] = observe(ChainStage::Fusion, &[RULE, OTHER, STALE]);
    behind_opaque[5] = observe(ChainStage::Selection, &[RULE, OTHER, STALE]);
    behind_opaque[6] = observe(ChainStage::Packing, &[RULE, OTHER, STALE]);
    assert_eq!(
        judge_stale(&ledger(behind_opaque)),
        StageVerdict::Indeterminate,
        "a first sighting right behind an unjoinable stage could have entered there"
    );

    let mut absent_after_opaque = clean_run();
    absent_after_opaque[0] = Observation::unjoinable(ChainStage::Exact, 0, Some(1));
    absent_after_opaque[4] = observe(ChainStage::Fusion, &[RULE, OTHER, STALE]);
    absent_after_opaque[5] = observe(ChainStage::Selection, &[RULE, OTHER, STALE]);
    absent_after_opaque[6] = observe(ChainStage::Packing, &[RULE, OTHER, STALE]);
    assert_eq!(
        judge_stale(&ledger(absent_after_opaque)),
        StageVerdict::StaleIngress(ChainStage::Fusion),
        "a filter's output without the occurrence closes the opacity before it"
    );

    let mut dropped_behind_opaque = clean_run();
    dropped_behind_opaque[3] = Observation::unjoinable(ChainStage::Eligibility, 0, Some(1));
    dropped_behind_opaque[4] = observe(ChainStage::Fusion, &[RULE, OTHER, STALE]);
    assert_eq!(
        judge_stale(&ledger(dropped_behind_opaque)),
        StageVerdict::Clean,
        "stale evidence removed before the terminal stage is clean wherever it entered"
    );

    let reintroduced = ledger(CHAIN_STAGES.iter().map(|stage| match stage {
        ChainStage::Exact | ChainStage::Fusion | ChainStage::Selection | ChainStage::Packing => {
            observe(*stage, &[RULE, STALE])
        }
        _ => observe(*stage, &[RULE]),
    }));
    assert_eq!(
        judge_stale(&reintroduced),
        StageVerdict::StaleIngress(ChainStage::Fusion),
        "a filter that removed the occurrence clears the earlier sighting; the delivered copy entered later"
    );

    let mut opaque_after_ingress = clean_run();
    opaque_after_ingress[0] = observe(ChainStage::Exact, &[RULE, OTHER, STALE]);
    opaque_after_ingress[3] = Observation::unjoinable(ChainStage::Eligibility, 0, Some(1));
    opaque_after_ingress[4] = observe(ChainStage::Fusion, &[RULE, OTHER, STALE]);
    opaque_after_ingress[5] = observe(ChainStage::Selection, &[RULE, OTHER, STALE]);
    opaque_after_ingress[6] = observe(ChainStage::Packing, &[RULE, OTHER, STALE]);
    assert_eq!(
        judge_stale(&ledger(opaque_after_ingress)),
        StageVerdict::StaleIngress(ChainStage::Exact),
        "an unjoinable stage after a known sighting does not hide where it entered"
    );
}

#[test]
fn a_stage_observation_over_the_kernel_batch_is_refused() {
    let within: BTreeSet<String> = (0..MAX_CANDIDATES_PER_STAGE_OBSERVATION)
        .map(|index| format!("occurrence-{index}"))
        .collect();
    let observation = Observation::new(ChainStage::Exact, 0, None, within.clone()).unwrap();
    assert_eq!(
        observation.evidence(),
        &Evidence::Candidates(within.clone())
    );
    assert_eq!(
        observation.at(ChainStage::Lexical).stage(),
        ChainStage::Lexical
    );
    let mut over = within;
    over.insert("occurrence-over".to_string());
    assert_eq!(
        Observation::new(ChainStage::Exact, 0, None, over).err(),
        Some(LedgerError::CandidatesOverBound {
            count: MAX_CANDIDATES_PER_STAGE_OBSERVATION + 1,
            bound: MAX_CANDIDATES_PER_STAGE_OBSERVATION,
        })
    );
}

#[test]
fn completed_folds_compare_only_within_one_persisted_store() {
    let clean = Completed {
        verdict: judge(&ledger(clean_run()), ChainStage::Exact),
        database_incarnation_id: "store-a".to_string(),
    };
    let lost = Completed {
        verdict: judge(
            &ledger(run_dropping(&[ChainStage::Packing])),
            ChainStage::Exact,
        ),
        database_incarnation_id: "store-a".to_string(),
    };
    let restarted: Completed<ChainStage> = Completed {
        verdict: StageVerdict::Clean,
        database_incarnation_id: "store-a".into(),
    };
    let other_store: Completed<ChainStage> = Completed {
        verdict: StageVerdict::Clean,
        database_incarnation_id: "store-b".into(),
    };
    assert_eq!(clean.verdict, StageVerdict::Clean);
    assert_eq!(lost.verdict, StageVerdict::FirstLoss(ChainStage::Packing));
    assert_eq!(clean.agrees_with(&restarted), Ok(true));
    assert_eq!(clean.agrees_with(&lost), Ok(false));
    assert_eq!(
        clean.agrees_with(&other_store),
        Err(LedgerError::CrossStore {
            left: "store-a".into(),
            right: "store-b".into()
        })
    );
    let json = serde_json::to_string(&lost).unwrap();
    assert_eq!(
        json,
        r#"{"verdict":{"first_loss":"packing"},"database_incarnation_id":"store-a"}"#
    );
    assert_eq!(
        serde_json::from_str::<Completed<ChainStage>>(&json).unwrap(),
        lost
    );
}
