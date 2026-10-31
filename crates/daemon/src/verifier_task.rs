//! The attestation verifier's per-data-home task: it runs [`Verifier::pass`] on the blocking pool
//! while the Kernel is ready, over the bindings' participating per-root view and each root's
//! effective `verifier.enabled`, and paces the passes.

use std::collections::HashMap;
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
        // The memory reviewer's Ready gate: the store only while the coordinator reports it ready.
        Arc::new(move || crate::memory_reviewer::worker::ready_kernel(&kernel)),
        Arc::new(move || bound_projects(&bindings)),
        enabled_reader(),
        cancel,
    )
}

/// The bound projects in bound-root order: the participating per-root view's roots in path order,
/// each project once, with every canonical root it binds once. The canonical root is the one the
/// binding's scope and source capture's repository identity derive from, so a spelling bound
/// through a symlink reads the scope's repository after the link is retargeted.
fn bound_projects(bindings: &Mutex<crate::RouteBindings>) -> Vec<Project> {
    // The snapshot frees the route mutex before the roster is built.
    let bound: Vec<crate::kernel_routes::ProjectBinding> = bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .latest_per_root()
        .into_values()
        .map(|(_, binding)| binding.kernel_project.clone())
        .collect();
    let mut projects: Vec<Project> = Vec::new();
    let mut positions: HashMap<String, usize> = HashMap::new();
    for project in bound {
        let root = Root {
            path: project.root().to_path_buf(),
            digest: project.digest().to_owned(),
        };
        let scope_id = project.scope_id();
        match positions.get(&scope_id) {
            Some(&position) => {
                let roots = &mut projects[position].roots;
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
            None => {
                positions.insert(scope_id.clone(), projects.len());
                projects.push(Project {
                    scope_id,
                    roots: vec![root],
                });
            }
        }
    }
    projects
}

/// The production flag reader: each project root's effective `verifier.enabled`.
fn enabled_reader() -> Arc<dyn Fn(&Path) -> bool + Send + Sync> {
    enabled_reader_with(|key| std::env::var(key).ok())
}

/// The user tier's `verifier.enabled` under the `XDG_CONFIG_HOME` and `HOME` that `env` names; the
/// key is user-tier only, so one read answers for every project root.
fn enabled_reader_with(
    env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> Arc<dyn Fn(&Path) -> bool + Send + Sync> {
    let config = Mutex::new(crate::config::ConfigCache::default());
    Arc::new(move |_root| {
        config
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .verifier_enabled(env("XDG_CONFIG_HOME").as_deref(), env("HOME").as_deref())
    })
}

/// The data home's verifier task: a pass every [`IDLE_INTERVAL`], or [`RETRY_INTERVAL`] after a
/// pass that progressed or failed, run only while `kernel` yields the store. Cancellation raises
/// the running pass's budget, which the pass polls between steps, and the task joins the pass
/// before it returns.
///
/// A pass that panics takes the process-local [`Verifier`] state with it, and the next pass starts
/// from a fresh one, as after a restart: every cursor rescans from the start, and the skip checks
/// and the write set's receipt replay keep the rescan's writes idempotent.
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use host_runtime::RouteHandle;

    use super::*;
    use crate::{RouteBindings, SessionBinding};

    fn binding(root: &Path) -> SessionBinding {
        SessionBinding {
            project_root: root.to_path_buf(),
            kernel_project: crate::kernel_routes::ProjectBinding::new(root),
            harness: "daemon-test".to_owned(),
            session: "session".to_owned(),
            model_key: None,
            config: crate::config::DaemonConfig::default(),
            history_budget_tokens: crate::memory_render::DEFAULT_HISTORY_BUDGET_TOKENS,
            credential_fingerprints: std::collections::BTreeMap::new(),
            context_capabilities: crate::context_capabilities::LatchedCapabilities::Unreadable(
                "no_declaration",
            ),
        }
    }

    /// Projects come in bound-root order, whatever the bind order, with every canonical root a
    /// project binds in that order, each once, and each project once.
    #[test]
    fn bound_projects_follow_bound_root_order() {
        let dir = tempfile::tempdir().unwrap();
        let [a, b, c] = ["a", "b", "c"].map(|name| dir.path().join(name));
        for root in [&a, &b, &c] {
            std::fs::create_dir(root).unwrap();
        }
        // `d` is a second spelling of `a`, so it binds `a`'s project and adds no root.
        let d = dir.path().join("d");
        std::os::unix::fs::symlink(&a, &d).unwrap();
        let mut bindings = RouteBindings::default();
        for (channel, root) in [&c, &d, &a, &b, &c].into_iter().enumerate() {
            let route = RouteHandle {
                channel: u16::try_from(channel).unwrap(),
                epoch: 1,
            };
            bindings.insert(route, binding(root));
        }
        let projects = bound_projects(&Mutex::new(bindings));
        let roots: Vec<Vec<PathBuf>> = projects
            .iter()
            .map(|project| project.roots.iter().map(|root| root.path.clone()).collect())
            .collect();
        let canonical = |root: &PathBuf| std::fs::canonicalize(root).unwrap();
        assert_eq!(
            roots,
            vec![
                vec![canonical(&a)],
                vec![canonical(&b)],
                vec![canonical(&c)]
            ]
        );
        drop(d);
        let scopes: Vec<String> = [&a, &b, &c]
            .map(|root| crate::kernel_routes::ProjectBinding::new(root).scope_id())
            .into();
        assert_eq!(
            projects
                .iter()
                .map(|project| project.scope_id.clone())
                .collect::<Vec<_>>(),
            scopes
        );
    }

    /// A project bound only through a symlink gets the canonical root the binding's scope was
    /// derived from, the root source capture derives the repository identity from, so a symlink
    /// retargeted after the bind leaves the verifier reading the scope's repository.
    #[test]
    fn a_project_bound_through_a_symlink_gets_its_canonical_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut bindings = RouteBindings::default();
        bindings.insert(
            RouteHandle {
                channel: 0,
                epoch: 1,
            },
            binding(&link),
        );
        let projects = bound_projects(&Mutex::new(bindings));
        let binding = crate::kernel_routes::ProjectBinding::new(&link);
        assert_eq!(
            projects,
            vec![Project {
                scope_id: binding.scope_id(),
                roots: vec![Root {
                    path: binding.root().to_path_buf(),
                    digest: binding.digest().to_owned(),
                }],
            }]
        );
        assert_eq!(
            projects[0].roots[0].path,
            std::fs::canonicalize(&real).unwrap()
        );
    }

    /// The production reader reads the flag from the user tier its environment names: false with
    /// no file, true under a user-tier `true`, and false for a project-tier `true` alone.
    #[test]
    fn the_flag_reader_follows_the_environments_user_tier() {
        let xdg = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let config_home = xdg.path().to_str().unwrap().to_owned();
        let reader =
            enabled_reader_with(move |key| (key == "XDG_CONFIG_HOME").then(|| config_home.clone()));
        assert!(!reader(project.path()));
        std::fs::create_dir_all(project.path().join(".eidnara")).unwrap();
        std::fs::write(
            project.path().join(".eidnara/eidnara.jsonc"),
            r#"{ "verifier": { "enabled": true } }"#,
        )
        .unwrap();
        assert!(!reader(project.path()));

        let enabled = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(enabled.path().join("eidnara")).unwrap();
        std::fs::write(
            enabled.path().join("eidnara/eidnara.jsonc"),
            r#"{ "verifier": { "enabled": true } }"#,
        )
        .unwrap();
        let config_home = enabled.path().to_str().unwrap().to_owned();
        let reader =
            enabled_reader_with(move |key| (key == "XDG_CONFIG_HOME").then(|| config_home.clone()));
        assert!(reader(project.path()));
    }
}
