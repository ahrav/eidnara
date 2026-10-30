//! The attestation verifier's per-data-home task: it runs [`Verifier::pass`] on the blocking pool
//! while the Kernel is ready, over the bindings' participating per-root view and each root's
//! effective `verifier.enabled`, and paces the passes.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use kernel::KernelStore;
use kernel::applicability::EvalBudget;
use tokio_util::sync::CancellationToken;

use crate::attestation_verifier::bounded_pass::{
    Counters, IDLE_INTERVAL, PASS_DEADLINE, Project, RETRY_INTERVAL, Root, Verifier,
};

/// The data home's production task: [`run`] over the Kernel while its coordinator reports it
/// ready, the bindings' participating per-root view, and each root's effective flag.
pub(crate) fn task(
    counters: Arc<Counters>,
    kernel: Arc<crate::kernel_routes::KernelOpenCoordinator>,
    bindings: Arc<Mutex<crate::RouteBindings>>,
    cancel: CancellationToken,
) -> impl Future<Output = ()> + Send + 'static {
    run(
        counters,
        Arc::new(move || {
            (kernel.state() == crate::kernel_routes::KernelState::Ready)
                .then(|| kernel.kernel_store().ok())
                .flatten()
        }),
        Arc::new(move || bound_projects(&bindings)),
        enabled_reader(),
        cancel,
    )
}

/// The bound projects in bound-root order: the participating per-root view's roots in path order,
/// each project once, with every root it binds.
fn bound_projects(bindings: &Mutex<crate::RouteBindings>) -> Vec<Project> {
    let bindings = bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut projects: Vec<Project> = Vec::new();
    for (root, (_, binding)) in bindings.latest_per_root() {
        let project = &binding.kernel_project;
        let root = Root {
            path: root.to_path_buf(),
            digest: project.digest().to_owned(),
        };
        let scope_id = project.scope_id();
        match projects.iter_mut().find(|known| known.scope_id == scope_id) {
            Some(known) => known.roots.push(root),
            None => projects.push(Project {
                scope_id,
                roots: vec![root],
            }),
        }
    }
    projects
}

/// The production flag reader: each project root's effective `verifier.enabled`.
fn enabled_reader() -> Arc<dyn Fn(&Path) -> bool + Send + Sync> {
    let config = Mutex::new(crate::config::ConfigCache::default());
    Arc::new(move |root| {
        config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .verifier_enabled(root)
    })
}

/// The data home's verifier task: a pass every [`IDLE_INTERVAL`], or [`RETRY_INTERVAL`] after a
/// pass that progressed or failed, run only while `kernel` yields the store. Cancellation raises
/// the running pass's budget, which the pass polls between steps, and the task joins the pass
/// before it returns.
pub async fn run(
    counters: Arc<Counters>,
    kernel: Arc<dyn Fn() -> Option<Arc<KernelStore>> + Send + Sync>,
    projects: Arc<dyn Fn() -> Vec<Project> + Send + Sync>,
    enabled: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
    cancel: CancellationToken,
) {
    let mut verifier = Verifier::default();
    loop {
        let mut interval = RETRY_INTERVAL;
        if let Some(store) = kernel() {
            let counters = Arc::clone(&counters);
            let projects = Arc::clone(&projects);
            let enabled = Arc::clone(&enabled);
            let clock = EvalBudget::unbounded().bounded_by(Instant::now() + PASS_DEADLINE);
            let interrupt = clock.clone();
            let mut carried = std::mem::take(&mut verifier);
            let mut worker = tokio::task::spawn_blocking(move || {
                let report = carried.pass(&store, &projects(), &*enabled, &counters, &clock);
                (carried, report)
            });
            tokio::select! {
                _ = cancel.cancelled() => {
                    interrupt.cancel();
                    let _ = (&mut worker).await;
                    return;
                }
                joined = &mut worker => match joined {
                    Ok((carried, report)) => {
                        verifier = carried;
                        if !report.progressed && !report.failed {
                            interval = IDLE_INTERVAL;
                        }
                    }
                    Err(error) => eprintln!("daemon: verifier pass failed: {error}"),
                },
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
    }
}
