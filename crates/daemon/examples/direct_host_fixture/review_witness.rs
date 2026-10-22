//! The `review-witness` control: a fixture-class and a production-class MemoryReviewer run over the running host's own Kernel and Memory Store, so the installed review command reads their outcomes through the real host. The caller has raised the root's memories authority to MODULE over the wire. The production `MEMORY_CLASSES`, the production selector, the selection slot's enqueue, and a production `Worker` pass run here. The provider is a scripted local TLS peer, and the worker's activation record lives in a private home, so the host's own worker keeps its closed gate and sends nothing.
//!
//! The fixture class is a `canonical_claims` and a `promoted_memory` form of one decision over evidence whose repository provenance this control supplies: a witness of the path, not of production eligibility. The production class is the three forms the production claim materializer publishes of another decision, each `Sensitive`; the selector admits none of them, and the same inputs enqueued anyway abstain `owner_sensitive` before any request.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use daemon::memory_reviewer::coordinator::InvestigationPermits;
use daemon::memory_reviewer::model_request::{Endpoint, Providers};
use daemon::memory_reviewer::selection::MEMORY_CLASSES;
use daemon::memory_reviewer::settlement::read_selected_proposal;
use daemon::memory_reviewer::worker::{MemoryReviewerHost, ProjectRoute, RootScope, Worker};
use host_runtime::model_execution::supervisor::Supervisor;
use kernel::KernelStore;
use memory_store::MemoryStore;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::support::memory_reviewer_forms as forms;
use crate::support::memory_reviewer_gate::{
    Deployment, NoPublicModel, activation_record, write_activation_record,
};
use crate::support::memory_reviewer_publish::{kernel_incarnation, now_ms};
use crate::support::tls_peer::Peer;

const MODEL: &str = "claude-fixture";
const CREDENTIAL: &str = "ANTHROPIC_API_KEY";
const CREDENTIAL_IDENTITY: &str = "fixture-credential-identity";

/// The route-owned project scope of `root`, inserted with its domain when no route has committed for the project yet, beside the memory domain the decisions live in. Returns the project digest and the scope id.
fn ensure_route_scope(kernel: &KernelStore, root: &Path) -> Result<(String, String), &'static str> {
    use daemon::kernel_route_fixtures::{MEMORY_DOMAIN, ensure_domain, route_project_scope};
    let spec = route_project_scope(root);
    let scope_id = spec.scope_id.clone();
    let digest = spec.terms[0]
        .exact_value
        .clone()
        .ok_or("scope_unavailable")?;
    kernel
        .commit(
            forms::intent(&format!("review-witness-scope-{digest}")),
            |envelope| {
                ensure_domain(envelope, MEMORY_DOMAIN)?;
                if envelope.scope_terms(&spec.scope_id)?.is_none() {
                    ensure_domain(envelope, &spec.domain_id)?;
                    envelope.insert_scope(spec)?;
                }
                Ok(String::new())
            },
        )
        .map_err(|_| "scope_unavailable")?;
    Ok((digest, scope_id))
}

fn worker(
    home: &Path,
    kernel: &Arc<KernelStore>,
    store: &Arc<MemoryStore>,
    peer: &Peer,
    route: ProjectRoute,
) -> Arc<Worker> {
    let host = Arc::new(MemoryReviewerHost {
        supervisor: Arc::new(Supervisor::new(Arc::new(NoPublicModel))),
        credentials: BTreeMap::from([(
            CREDENTIAL.to_string(),
            zeroize::Zeroizing::new("sk-fixture-reviewer".to_string()),
        )]),
        credential_identities: OnceLock::from(BTreeMap::from([(
            CREDENTIAL.to_string(),
            CREDENTIAL_IDENTITY.to_string(),
        )])),
        worker_instance: "review-witness".to_string(),
    });
    let endpoint = Endpoint::for_test("localhost", peer.port, peer.roots.clone())
        .expect("the local peer is a test endpoint");
    let kernel = Arc::clone(kernel);
    Arc::new(Worker {
        host,
        home: home.to_path_buf(),
        store: Arc::clone(store),
        kernel: Arc::new(move || Some(Arc::clone(&kernel))),
        projects: Arc::new(move || vec![route.clone()]),
        status: Arc::default(),
        permits: Arc::new(InvestigationPermits::default()),
        providers: Providers::at(endpoint.clone(), endpoint, "us-east-1"),
        rejection: Default::default(),
    })
}

/// Runs both classes for the MODULE root `project_root`. Answers the originating decision's exact target tokens, the fixture evidence, each fixture job's selected read in the wire's shape, each production job's identity, and the provider requests, connections, and production attempts the pass made.
pub async fn run(core: &daemon::HandlerCore, project_root: &str) -> Result<Value, &'static str> {
    let kernel = core.kernel_store_for_test().ok_or("kernel_unavailable")?;
    let store = core.memory_store_for_test().ok_or("store_unavailable")?;
    let authority = store
        .module_authority_for_route(project_root, "memories")
        .map_err(|_| "store_unavailable")?
        .ok_or("authority_not_module")?;
    let project = authority.project.as_str();
    let root = PathBuf::from(project_root);
    let (digest, scope_id) = ensure_route_scope(&kernel, &root)?;
    // The host's claim runner would publish its own forms of the fixture decision; this control runs the materializer itself.
    core.pause_claim_sources_for_test();
    let now = now_ms();

    let (_, published) = forms::materialize_decision(&kernel, &scope_id, now);
    if published != 3 {
        return Err("materializer_published_other_forms");
    }
    let production = forms::frozen_inputs(&forms::live_descriptors(&kernel, MEMORY_CLASSES));
    let (decision, evidence) = forms::publish_fixture_forms(&kernel, &scope_id);
    let page = forms::select(&kernel, &store, project, &digest, MEMORY_CLASSES);
    if page.references.len() != 2 {
        return Err("selector_admitted_other_forms");
    }
    let (known_as_of, states) = kernel
        .object_states(std::slice::from_ref(&decision))
        .map_err(|_| "kernel_unavailable")?;
    let state = states[0].as_ref().ok_or("kernel_unavailable")?;
    let target = json!({
        "object_id": decision,
        "source_revision": state.object.source_revision,
        "known_as_of": known_as_of,
        "commit_token": state.latest_change_commit_seq,
    });
    forms::enqueue_selection(&store, project, &page, now);
    forms::enqueue_selection(&store, project, &production, now + 1);

    let home = tempfile::tempdir().map_err(|_| "home_unavailable")?;
    let mut peer = Peer::start().await;
    write_activation_record(
        home.path(),
        &activation_record(&Deployment {
            model: MODEL,
            kernel_incarnation: &kernel_incarnation(&kernel),
            memstore_incarnation: &store
                .memory_reviewer_store_incarnation()
                .map_err(|_| "store_unavailable")?,
            provider: &peer.sender().provider_identity(),
            credential: CREDENTIAL,
            credential_fingerprint: CREDENTIAL_IDENTITY,
        }),
    );
    let retain = forms::stepping_as(MODEL, forms::RETAIN_STEP);
    let served = peer.serve_script(vec![retain.clone(), retain]);
    let route = ProjectRoute {
        project: project.to_string(),
        authority_generation: authority.generation,
        roots: vec![RootScope {
            project_root: root,
            project_digest: digest,
            scope_id,
        }],
    };
    let ran = worker(home.path(), &kernel, &store, &peer, route)
        .pass(&CancellationToken::new())
        .await;
    if ran != page.references.len() + production.references.len() {
        return Err("worker_ran_other_jobs");
    }
    let requests = served.await.map_err(|_| "peer_failed")?.len();

    let mut fixture = Vec::new();
    for inputs in &page.references {
        let identity = inputs
            .causal_identity()
            .map_err(|_| "identity_unavailable")?;
        let selected = read_selected_proposal(&kernel, &store, project, &identity, now_ms())
            .map_err(|_| "proposal_unreadable")?;
        fixture.push(json!({
            "kind": "proposal",
            "causal_identity": identity,
            "reference": selected.reference,
            "proposal": selected.proposal,
            "review_expires_at": selected.review_expires_at,
        }));
    }
    let mut production_identities = Vec::new();
    let mut production_attempts = 0;
    for inputs in &production.references {
        let identity = inputs
            .causal_identity()
            .map_err(|_| "identity_unavailable")?;
        production_attempts += store
            .list_memory_reviewer_attempts(project, &identity)
            .map_err(|_| "store_unavailable")?
            .len();
        production_identities.push(identity);
    }
    Ok(json!({
        "target": target,
        "evidence": evidence,
        "fixture": fixture,
        "production": production_identities,
        "provider_requests": requests,
        "provider_connections": peer.connection_count(),
        "production_attempts": production_attempts,
    }))
}
