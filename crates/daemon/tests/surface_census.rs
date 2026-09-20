mod support;

use daemon::edit_receipts::AccountingBinding;
use daemon::packing::AccountingProfile;
use serde_json::{Value, json};
use support::kernel_daemon::{KernelDaemon, envelope};

const OCCURRENCE: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn edit_context() -> Value {
    json!({
        "context_revision": "rev-1",
        "representation": "repr-1",
        "spans": [{"occurrence_id": OCCURRENCE, "buffer_len": 100, "span": [0, 10]}],
        "selection": [OCCURRENCE],
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_two_and_the_query_route_are_default_closed() {
    let daemon = KernelDaemon::start().await;
    let project = daemon.project().to_owned();

    let query = envelope(
        "retrieval.query",
        &project,
        json!({"query": "id:rule", "remaining_ms": 5_000, "destination": "local"}),
    );
    assert_eq!(daemon.terminal(query).await, "disabled");

    let mut prepare = edit_context();
    prepare["action"] = json!("append");
    prepare["edit_bytes"] = json!(10);
    assert_eq!(
        daemon
            .terminal(envelope("retrieval.prepare", &project, prepare))
            .await,
        "disabled"
    );
    let mut apply = edit_context();
    apply["preparation_id"] = json!("x-y");
    apply["accounting_profile"] =
        serde_json::to_value(AccountingBinding::of(&AccountingProfile::exact_tokenizer())).unwrap();
    assert_eq!(
        daemon
            .terminal(envelope("retrieval.apply", &project, apply))
            .await,
        "disabled"
    );
    let confirm = json!({
        "preparation_id": "x-y",
        "forwarded_identity": "f",
        "applied_identity": null,
        "outcome": "keep",
    });
    assert_eq!(
        daemon
            .terminal(envelope("retrieval.confirm", &project, confirm))
            .await,
        "disabled"
    );
    daemon.shutdown().await;
}
