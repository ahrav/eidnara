mod claim_fixture;

use std::num::NonZeroUsize;
use std::ops::ControlFlow;

use claim_fixture::{DOMAIN, Fixture, decision, intent};
use kernel::applicability::EvalBudget;
use kernel::{ClaimFactsError, KernelError, ScopeSpec, Sensitivity};

const SCOPE: &str = "project:a";

fn scope(scope_id: &str) -> ScopeSpec {
    ScopeSpec {
        scope_id: scope_id.to_string(),
        object_id: format!("{scope_id}-object"),
        domain_id: DOMAIN.to_string(),
        source_kind: "fixture".to_string(),
        source_id: scope_id.to_string(),
        source_revision: 1,
        sensitivity: Sensitivity::Normal,
        terms: Vec::new(),
    }
}

fn commit(fixture: &Fixture, key: &str, indices: &[i64], scope_id: &str, source_kind: &str) -> i64 {
    fixture
        .store
        .commit(intent(key), |envelope| {
            for index in indices {
                let mut spec = decision(*index, 1);
                spec.scope_id = Some(scope_id.to_string());
                spec.source_kind = source_kind.to_string();
                spec.source_id = format!("source-{index}");
                envelope.insert_decision(spec)?;
            }
            Ok(String::new())
        })
        .unwrap()
        .commit_seq
}

fn page(
    fixture: &Fixture,
    requested: i64,
    after: Option<(i64, &str)>,
    max_rows: usize,
) -> Vec<(i64, String)> {
    fixture
        .store
        .scope_decisions_after_within_budget(
            SCOPE,
            "verifier",
            requested,
            after,
            NonZeroUsize::new(max_rows).unwrap(),
            &EvalBudget::unbounded(),
        )
        .unwrap()
}

fn object(index: i64) -> String {
    format!("decision-object-{index}")
}

#[test]
fn the_page_walks_a_scopes_live_decisions_in_commit_order_after_the_cursor() {
    let fixture = Fixture::open();
    fixture
        .store
        .commit(intent("scopes"), |envelope| {
            envelope.insert_scope(scope(SCOPE))?;
            envelope.insert_scope(scope("project:b"))?;
            Ok(String::new())
        })
        .unwrap();
    // Object ids sort against commit order: decision 9 commits first.
    let first = commit(&fixture, "first", &[9], SCOPE, "assistant");
    let second = commit(&fixture, "second", &[2, 1], SCOPE, "assistant");
    commit(&fixture, "other-scope", &[3], "project:b", "assistant");
    commit(&fixture, "verifier", &[4], SCOPE, "verifier");
    let retired = commit(&fixture, "retired", &[5], SCOPE, "assistant");
    let before_retire = fixture.tip();
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_decision(&object(5))?;
            Ok(String::new())
        })
        .unwrap();
    let last = commit(&fixture, "last", &[6], SCOPE, "assistant");
    let tip = fixture.tip();

    let all = vec![
        (first, object(9)),
        (second, object(1)),
        (second, object(2)),
        (last, object(6)),
    ];
    assert_eq!(page(&fixture, tip, None, 32), all);
    assert_eq!(
        page(&fixture, before_retire, None, 32),
        vec![
            (first, object(9)),
            (second, object(1)),
            (second, object(2)),
            (retired, object(5)),
        ]
    );
    assert_eq!(page(&fixture, tip, None, 2), all[..2]);
    // A cursor whose object id sorts after every later decision's still pages past its sequence.
    assert_eq!(page(&fixture, tip, Some((first, &object(9))), 32), all[1..]);
    assert_eq!(
        page(&fixture, tip, Some((second, &object(1))), 32),
        all[2..]
    );
    assert_eq!(page(&fixture, tip, Some((second, &object(2))), 1), all[3..]);
    assert!(page(&fixture, tip, Some((last, &object(6))), 32).is_empty());

    let refused = |requested, budget: &EvalBudget| {
        fixture.store.scope_decisions_after_within_budget(
            SCOPE,
            "verifier",
            requested,
            None,
            NonZeroUsize::MIN,
            budget,
        )
    };
    assert_eq!(
        refused(tip + 1, &EvalBudget::unbounded()),
        Err(KernelError::FutureSnapshot)
    );
    assert_eq!(
        refused(-1, &EvalBudget::unbounded()),
        Err(KernelError::InvalidInput)
    );
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(refused(tip, &cancelled), Err(KernelError::Deadline));
    // The read takes its reader under the budget's progress handler, which interrupts the query
    // itself at the deadline, and not only the wait for a reader.
    let acquisitions = fixture.store.controlled_acquisitions_for_test();
    assert_eq!(page(&fixture, tip, None, 32), all);
    assert_eq!(
        fixture.store.controlled_acquisitions_for_test(),
        acquisitions + 1
    );
}

#[test]
fn the_registry_source_page_walks_a_scopes_live_decisions_of_one_source_kind_in_source_order() {
    let fixture = Fixture::open();
    fixture
        .store
        .commit(intent("scopes"), |envelope| {
            envelope.insert_scope(scope(SCOPE))?;
            envelope.insert_scope(scope("project:b"))?;
            Ok(String::new())
        })
        .unwrap();
    let insert = |key: &str, rows: &[(i64, &str, &str, &str)]| {
        fixture
            .store
            .commit(intent(key), |envelope| {
                for (index, scope_id, source_kind, source_id) in rows {
                    let mut spec = decision(*index, 1);
                    spec.scope_id = Some((*scope_id).to_string());
                    spec.source_kind = (*source_kind).to_string();
                    spec.source_id = (*source_id).to_string();
                    envelope.insert_decision(spec)?;
                }
                Ok(String::new())
            })
            .unwrap()
    };
    // Source order runs against commit order, and two decisions share one source id, inserted
    // against their object id order.
    insert(
        "first",
        &[
            (1, SCOPE, "verifier", "c"),
            (2, SCOPE, "assistant", "a"),
            (3, "project:b", "verifier", "b"),
        ],
    );
    insert(
        "second",
        &[(5, SCOPE, "verifier", "a"), (4, SCOPE, "verifier", "a")],
    );
    insert("retired", &[(6, SCOPE, "verifier", "b")]);
    let before_retire = fixture.tip();
    fixture
        .store
        .commit(intent("retire"), |envelope| {
            envelope.retire_decision(&object(6))?;
            Ok(String::new())
        })
        .unwrap();
    let retired = fixture.tip();
    insert("later", &[(7, SCOPE, "verifier", "0")]);
    let target = fixture.store.capture_commit_read_target().unwrap();
    let budget = EvalBudget::unbounded();
    // Each visit reads through the page's transaction before the statement steps on.
    let source_page = |through: i64, after: Option<(&str, &str)>, max_rows: usize, take: usize| {
        fixture
            .store
            .read_at(target, &budget, |snapshot| {
                let mut rows = Vec::new();
                let visited = snapshot.scope_source_decisions_after(
                    SCOPE,
                    "verifier",
                    through,
                    after,
                    NonZeroUsize::new(max_rows).unwrap(),
                    |source_id, object_id| {
                        assert!(snapshot.decision_standing(&object_id).unwrap().is_some());
                        rows.push((source_id, object_id));
                        if rows.len() == take {
                            ControlFlow::Break(())
                        } else {
                            ControlFlow::Continue(())
                        }
                    },
                )?;
                assert_eq!(visited, rows.len());
                Ok::<_, ClaimFactsError>(rows)
            })
            .unwrap()
    };
    let pair = |source: &str, index: i64| (source.to_string(), object(index));
    let all = vec![pair("a", 4), pair("a", 5), pair("c", 1)];
    assert_eq!(source_page(retired, None, 32, usize::MAX), all);
    assert_eq!(
        source_page(before_retire, None, 32, usize::MAX),
        vec![pair("a", 4), pair("a", 5), pair("b", 6), pair("c", 1)]
    );
    assert_eq!(source_page(retired, None, 2, usize::MAX), all[..2]);
    assert_eq!(source_page(retired, None, 32, 1), all[..1]);
    assert_eq!(
        source_page(retired, Some(("a", &object(4))), 32, usize::MAX),
        all[1..]
    );
    assert_eq!(
        source_page(retired, Some(("a", &object(5))), 1, usize::MAX),
        all[2..]
    );
    assert!(source_page(retired, Some(("c", &object(1))), 32, usize::MAX).is_empty());
    assert_eq!(
        source_page(target.through_commit, None, 32, usize::MAX)[0],
        pair("0", 7)
    );

    let refused = |through: i64, budget: &EvalBudget| {
        fixture.store.read_at(target, budget, |snapshot| {
            snapshot
                .scope_source_decisions_after(
                    SCOPE,
                    "verifier",
                    through,
                    None,
                    NonZeroUsize::MIN,
                    |_, _| ControlFlow::Continue(()),
                )
                .map_err(ClaimFactsError::from)
        })
    };
    for through in [-1, target.through_commit + 1] {
        assert_eq!(
            refused(through, &budget),
            Err(ClaimFactsError::Kernel(KernelError::InvalidInput))
        );
    }
    let cancelled = EvalBudget::unbounded();
    cancelled.cancel();
    assert_eq!(
        refused(retired, &cancelled),
        Err(ClaimFactsError::Kernel(KernelError::Deadline))
    );
}
