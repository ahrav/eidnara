mod claim_fixture;

use std::num::NonZeroUsize;

use claim_fixture::{DOMAIN, Fixture, decision, intent};
use kernel::applicability::EvalBudget;
use kernel::{KernelError, ScopeSpec, Sensitivity};

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
}
