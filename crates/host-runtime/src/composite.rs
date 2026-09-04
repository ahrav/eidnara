//!
//! The host `RouteRegistry` exclusively owns route lifecycle and channel reuse.
//! The host `RouteRegistry` exclusively owns route reservation, liveness, closing, and finalization.
//! The route map records only ownership of host-validated handles.
//! A route-map entry is inserted before the child's `bind` call.
//! A route-map entry remains until the child's `route_gone` callback returns.
//!
//! The direct profile's primary is `context/tool_provider`.
//! The direct profile's secondary is `synapse/management_surface`.
//! The direct profile's tertiary is `broca/management_surface`.
//! The direct profile publishes its primary, secondary, and tertiary entries in that order.
//! Generic component types let tests substitute deterministic children.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use crate::config::HostInit;
use crate::handler::{
    BindOutcome, HealthReport, HealthStatus, HostHandler, InitError, ManifestSnapshot, RequestCtx,
    RequestOutcome, ResourceDeclaration, RouteHandle, RouteIdentity, RouteTarget,
};

/// The composite reports a `ShutdownError` message's byte length, not its contents.
/// Diagnostics report only the `ShutdownError` message's byte length under protocol V24.
/// Reporting only the byte length prevents component detail from reaching host logs.
/// A component may include detailed diagnostics in `ShutdownError` because host logs report only its byte length.
#[derive(Debug)]
pub struct ShutdownError(pub String);

impl std::fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "component shutdown failed: {}", self.0)
    }
}

impl std::error::Error for ShutdownError {}

/// `CompositeComponent` has no shared `initialize` method because each role has a different initialization input.
pub trait CompositeComponent: Send + Sync + 'static {
    fn manifest(&self) -> ManifestSnapshot;

    fn install_connection_key(&self, _key: [u8; 32]) {}

    /// `resources` declares immutable resources before initialization.
    /// The default returns no resource reservation.
    /// No resource reservation preserves general single-pool admission for existing components.
    fn resources(&self) -> ResourceDeclaration {
        ResourceDeclaration::default()
    }

    fn bind(
        &self,
        route: RouteHandle,
        identity: RouteIdentity,
    ) -> impl Future<Output = BindOutcome> + Send;

    fn handle(&self, ctx: RequestCtx) -> impl Future<Output = RequestOutcome> + Send;

    fn route_gone(&self, route: RouteHandle) -> impl Future<Output = ()> + Send;

    fn health(&self) -> impl Future<Output = HealthReport> + Send;

    /// The composite drains all remaining children before returning a failure.
    /// The composite returns one deterministic redacted shutdown failure after draining all children.
    fn shutdown(&self) -> impl Future<Output = Result<(), ShutdownError>> + Send;
}

pub trait PrimaryComponent: CompositeComponent {
    fn initialize(&self, init: HostInit) -> impl Future<Output = Result<(), InitError>> + Send;

    /// `activate` defaults to `Ok(())` for components without deferred activation.
    fn activate(&self) -> impl Future<Output = Result<(), InitError>> + Send {
        async { Ok(()) }
    }
}

/// A missing or invalid artifact resolves to `Ok(())` with the component disabled.
/// An artifact-disabled component rejects `bind` with `artifact_invalid`.
/// An artifact-disabled component remains published in the catalog.
/// `Err` is reserved for host-fatal invariant failures.
pub trait SecondaryComponent: CompositeComponent {
    fn initialize(&self) -> impl Future<Output = Result<(), InitError>> + Send;

    fn activate(&self) -> impl Future<Output = Result<(), InitError>> + Send {
        async { Ok(()) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Child {
    Primary,
    Secondary,
    Tertiary,
}

pub struct StaticComposite<P, S, B> {
    primary: P,
    secondary: S,
    tertiary: B,
    primary_id: Box<str>,
    secondary_id: Box<str>,
    tertiary_id: Box<str>,
    routes: Mutex<HashMap<RouteHandle, Child>>,
}

impl<P: PrimaryComponent, S: SecondaryComponent, B: SecondaryComponent> StaticComposite<P, S, B> {
    /// Duplicate module IDs are rejected to keep bind dispatch unambiguous.
    pub fn new(primary: P, secondary: S, tertiary: B) -> Result<Self, InitError> {
        let primary_id = primary.manifest().module_id.into_boxed_str();
        let secondary_id = secondary.manifest().module_id.into_boxed_str();
        let tertiary_id = tertiary.manifest().module_id.into_boxed_str();
        if primary_id == secondary_id || primary_id == tertiary_id || secondary_id == tertiary_id {
            return Err(InitError(
                "composite components share one module ID".to_owned(),
            ));
        }
        Ok(Self {
            primary,
            secondary,
            tertiary,
            primary_id,
            secondary_id,
            tertiary_id,
            routes: Mutex::new(HashMap::new()),
        })
    }

    fn child_of_route(&self, route: RouteHandle) -> Option<Child> {
        self.routes
            .lock()
            .expect("composite route map")
            .get(&route)
            .copied()
    }
}

fn severity(status: HealthStatus) -> u8 {
    match status {
        HealthStatus::Ok => 0,
        HealthStatus::Degraded => 1,
        HealthStatus::Failing => 2,
    }
}

/// `ChildPanic` marks a caught child panic. The payload never leaves `catch_child_panic`: it is
/// destroyed inside `catch_unwind` by `discard_payload`, so a payload whose own `Drop` panics
/// cannot unwind into the composite.
#[derive(Debug, PartialEq, Eq)]
struct ChildPanic;

/// `discard_payload` drops a caught panic payload inside `catch_unwind`. A payload whose `Drop`
/// panics is leaked, which bounds the regress at one level.
fn discard_payload(payload: Box<dyn std::any::Any + Send + 'static>) -> ChildPanic {
    if let Err(second) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        drop(payload);
    })) {
        std::mem::forget(second);
    }
    ChildPanic
}

/// `catch_child_panic` takes a thunk rather than a future so that a panic in the synchronous
/// setup of a `fn -> impl Future` callback is caught as well as a panic while polling or
/// dropping the future. Every panic during callback setup, polling, or future drop returns
/// `Err(ChildPanic)`.
async fn catch_child_panic<F: Future>(
    callback: impl FnOnce() -> F,
) -> Result<F::Output, ChildPanic> {
    let future = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Box::pin(callback())))
        .map_err(discard_payload)?;
    let mut guard = ChildFuture(Some(future));
    let output = std::future::poll_fn(|cx| {
        let future = guard
            .0
            .as_mut()
            .expect("child future is present until completion");
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(poll) => poll.map(Ok),
            Err(payload) => std::task::Poll::Ready(Err(discard_payload(payload))),
        }
    })
    .await;
    // Dropping the future runs child `Drop` impls, which are child code too.
    let future = guard.0.take();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(future)))
        .map_err(discard_payload)?;
    output
}

/// `ChildFuture` owns a child future between polls so that dropping it while pending, as a
/// cancelled `catch_child_panic` does, still runs the child's `Drop` inside `catch_unwind`.
/// A panic there is swallowed: cancellation has no result to attach it to.
struct ChildFuture<F>(Option<std::pin::Pin<Box<F>>>);

impl<F> Drop for ChildFuture<F> {
    fn drop(&mut self) {
        if let Some(future) = self.0.take()
            && let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(future)))
        {
            discard_payload(payload);
        }
    }
}

/// The composite records only the byte length of each returned shutdown error.
fn shutdown_failure_note(
    id: &str,
    outcome: Result<Result<(), ShutdownError>, ChildPanic>,
) -> Option<String> {
    match outcome {
        Ok(Ok(())) => None,
        Ok(Err(err)) => Some(format!(
            "{id} shutdown failed ({} bytes of detail redacted)",
            err.0.len()
        )),
        Err(ChildPanic) => Some(format!("{id} shutdown panicked")),
    }
}

impl<P: PrimaryComponent, S: SecondaryComponent, B: SecondaryComponent> HostHandler
    for StaticComposite<P, S, B>
{
    fn install_connection_key(&self, key: [u8; 32]) {
        self.primary.install_connection_key(key);
        self.secondary.install_connection_key(key);
        self.tertiary.install_connection_key(key);
    }

    fn manifests(&self) -> Vec<ManifestSnapshot> {
        vec![
            self.primary.manifest(),
            self.secondary.manifest(),
            self.tertiary.manifest(),
        ]
    }

    fn resource_declarations(&self) -> Vec<ResourceDeclaration> {
        // Resource declarations use the same deterministic order as `manifests`.
        vec![
            self.primary.resources(),
            self.secondary.resources(),
            self.tertiary.resources(),
        ]
    }

    async fn initialize(&self, init: HostInit) -> Result<(), InitError> {
        // Independent children initialize concurrently; primary, then secondary, then tertiary errors win when initializers fail in the same poll.
        tokio::try_join!(
            biased;
            self.primary.initialize(init),
            self.secondary.initialize(),
            self.tertiary.initialize()
        )?;
        Ok(())
    }

    async fn activate(&self) -> Result<(), InitError> {
        // Children activate concurrently; when activations fail in the same poll, primary, then secondary, then tertiary errors take precedence.
        tokio::try_join!(
            biased;
            self.primary.activate(),
            self.secondary.activate(),
            self.tertiary.activate()
        )?;
        Ok(())
    }

    async fn bind(
        &self,
        route: RouteHandle,
        target: RouteTarget,
        identity: RouteIdentity,
    ) -> BindOutcome {
        let child = if target.module_id == self.primary_id.as_ref() {
            Child::Primary
        } else if target.module_id == self.secondary_id.as_ref() {
            Child::Secondary
        } else if target.module_id == self.tertiary_id.as_ref() {
            Child::Tertiary
        } else {
            return BindOutcome::Reject {
                code: crate::control::CODE_TARGET_UNAVAILABLE.to_owned(),
                message: "target module is not part of this composition".to_owned(),
            };
        };
        // The map records the target child before `bind` so `route_gone` can dispatch if the route closes while `bind` is pending.
        // The entry remains after a rejected bind because the host invokes `route_gone` for
        // rejected handles (`docs/host-wire-protocol.md`), and that call must reach the rejecting child.
        self.routes
            .lock()
            .expect("composite route map")
            .insert(route, child);
        match child {
            Child::Primary => self.primary.bind(route, identity).await,
            Child::Secondary => self.secondary.bind(route, identity).await,
            Child::Tertiary => self.tertiary.bind(route, identity).await,
        }
    }

    async fn handle(&self, ctx: RequestCtx) -> RequestOutcome {
        match self.child_of_route(ctx.route) {
            Some(Child::Primary) => self.primary.handle(ctx).await,
            Some(Child::Secondary) => self.secondary.handle(ctx).await,
            Some(Child::Tertiary) => self.tertiary.handle(ctx).await,
            None => RequestOutcome::error(
                crate::control::CODE_INTERNAL_ERROR,
                "route is not mapped to a component",
            ),
        }
    }

    async fn route_gone(&self, route: RouteHandle) {
        let child = self.child_of_route(route);
        match child {
            Some(Child::Primary) => self.primary.route_gone(route).await,
            Some(Child::Secondary) => self.secondary.route_gone(route).await,
            Some(Child::Tertiary) => self.tertiary.route_gone(route).await,
            None => return,
        }
        // `route_gone` removes a route only after the dispatched child callback returns.
        self.routes
            .lock()
            .expect("composite route map")
            .remove(&route);
    }

    async fn health(&self) -> HealthReport {
        let panicked = |id: &str| HealthReport {
            status: HealthStatus::Failing,
            detail: Some(format!("{id} health check panicked")),
            metrics: None,
        };
        let primary = catch_child_panic(|| self.primary.health())
            .await
            .unwrap_or_else(|ChildPanic| panicked(&self.primary_id));
        let secondary = catch_child_panic(|| self.secondary.health())
            .await
            .unwrap_or_else(|ChildPanic| panicked(&self.secondary_id));
        let tertiary = catch_child_panic(|| self.tertiary.health())
            .await
            .unwrap_or_else(|ChildPanic| panicked(&self.tertiary_id));
        // `Ok < Degraded < Failing`; equal severities use catalog order: primary, secondary, then tertiary.
        // child.
        let component_status = |report: &HealthReport| match report.status {
            HealthStatus::Ok => "ok",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Failing => "failing",
        };
        let mut components = serde_json::Map::new();
        for (id, report) in [
            (self.primary_id.as_ref(), &primary),
            (self.secondary_id.as_ref(), &secondary),
            (self.tertiary_id.as_ref(), &tertiary),
        ] {
            components.insert(
                id.to_owned(),
                serde_json::json!({
                    "status": component_status(report),
                    "metrics": report.metrics.clone(),
                }),
            );
        }
        let metrics = serde_json::json!({"components": components});
        let mut winner = primary;
        for candidate in [secondary, tertiary] {
            if severity(candidate.status) > severity(winner.status) {
                winner = candidate;
            }
        }
        winner.metrics = Some(metrics);
        winner
    }

    async fn shutdown(&self) {
        // An earlier failure does not skip later children's drains.
        // cleanly returned.
        let mut failures: Vec<String> = Vec::new();
        let outcomes = [
            shutdown_failure_note(
                &self.tertiary_id,
                catch_child_panic(|| self.tertiary.shutdown()).await,
            ),
            shutdown_failure_note(
                &self.secondary_id,
                catch_child_panic(|| self.secondary.shutdown()).await,
            ),
            shutdown_failure_note(
                &self.primary_id,
                catch_child_panic(|| self.primary.shutdown()).await,
            ),
        ];
        failures.extend(outcomes.into_iter().flatten());
        if !failures.is_empty() {
            panic!("{}", failures.join("; "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// `Fake` is a deterministic child whose `bind`, `health`, and `shutdown` behavior is fixed at
    /// construction. `panic_before_future` panics in the synchronous part of a callback, before
    /// any future exists, which models a component written as `fn -> impl Future`.
    struct Fake {
        id: &'static str,
        reject_bind: bool,
        panic_in_health: AtomicBool,
        panic_before_future: bool,
        shutdowns: AtomicUsize,
    }

    impl Fake {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                reject_bind: false,
                panic_in_health: AtomicBool::new(false),
                panic_before_future: false,
                shutdowns: AtomicUsize::new(0),
            }
        }

        fn panicking_before_any_future(id: &'static str) -> Self {
            Self {
                panic_before_future: true,
                ..Self::new(id)
            }
        }

        fn rejecting(id: &'static str) -> Self {
            Self {
                reject_bind: true,
                ..Self::new(id)
            }
        }

        fn panicking_health(id: &'static str) -> Self {
            Self {
                panic_in_health: AtomicBool::new(true),
                ..Self::new(id)
            }
        }
    }

    impl CompositeComponent for Fake {
        fn manifest(&self) -> ManifestSnapshot {
            ManifestSnapshot {
                module_id: self.id.to_owned(),
                module_version: "0".to_owned(),
                provides: Vec::new(),
                control_ops: Vec::new(),
            }
        }

        async fn bind(&self, _route: RouteHandle, _identity: RouteIdentity) -> BindOutcome {
            if self.reject_bind {
                BindOutcome::Reject {
                    code: "rejected".to_owned(),
                    message: "fake rejects every bind".to_owned(),
                }
            } else {
                BindOutcome::Accept
            }
        }

        async fn handle(&self, _ctx: RequestCtx) -> RequestOutcome {
            RequestOutcome::error("unused", "fake never handles requests")
        }

        async fn route_gone(&self, _route: RouteHandle) {}

        fn health(&self) -> impl Future<Output = HealthReport> + Send {
            if self.panic_before_future {
                panic!("{} health panicked before returning a future", self.id);
            }
            let panic_while_polled = self.panic_in_health.load(Ordering::Relaxed);
            let id = self.id;
            async move {
                if panic_while_polled {
                    panic!("{id} health panicked");
                }
                HealthReport {
                    status: HealthStatus::Ok,
                    detail: None,
                    metrics: None,
                }
            }
        }

        fn shutdown(&self) -> impl Future<Output = Result<(), ShutdownError>> + Send {
            self.shutdowns.fetch_add(1, Ordering::Relaxed);
            if self.panic_before_future {
                panic!("{} shutdown panicked before returning a future", self.id);
            }
            async { Ok(()) }
        }
    }

    impl PrimaryComponent for Fake {
        async fn initialize(&self, _init: HostInit) -> Result<(), InitError> {
            Ok(())
        }
    }

    impl SecondaryComponent for Fake {
        async fn initialize(&self) -> Result<(), InitError> {
            Ok(())
        }
    }

    fn identity() -> RouteIdentity {
        RouteIdentity {
            project_root: std::path::PathBuf::from("/"),
            harness: "test".to_owned(),
            session: "s".to_owned(),
            consumer_module_id: None,
            consumer_launch_nonce: None,
            consumer_capabilities: Vec::new(),
            admission_facts: None,
            credential_fingerprints: std::collections::BTreeMap::new(),
        }
    }

    fn target(module_id: &str) -> RouteTarget {
        RouteTarget {
            module_id: module_id.to_owned(),
            kind: crate::handler::TargetKind::ManagementSurface,
        }
    }

    #[tokio::test]
    async fn a_rejected_bind_keeps_its_route_until_the_host_sends_route_gone() {
        let composite = StaticComposite::new(
            Fake::new("primary"),
            Fake::rejecting("secondary"),
            Fake::new("tertiary"),
        )
        .expect("distinct ids");
        let rejected = RouteHandle {
            channel: 1,
            epoch: 1,
        };

        let outcome = composite
            .bind(rejected, target("secondary"), identity())
            .await;
        assert!(matches!(outcome, BindOutcome::Reject { .. }));
        // The host still owes the rejecting child one `route_gone`, which needs the mapping.
        assert_eq!(composite.child_of_route(rejected), Some(Child::Secondary));
        composite.route_gone(rejected).await;
        assert_eq!(composite.child_of_route(rejected), None);

        let unknown = RouteHandle {
            channel: 2,
            epoch: 1,
        };
        let outcome = composite.bind(unknown, target("nobody"), identity()).await;
        assert!(matches!(outcome, BindOutcome::Reject { .. }));
        assert_eq!(composite.child_of_route(unknown), None);
    }

    #[tokio::test]
    async fn a_primary_health_panic_is_reported_like_any_other_child_panic() {
        let composite = StaticComposite::new(
            Fake::panicking_health("primary"),
            Fake::new("secondary"),
            Fake::new("tertiary"),
        )
        .expect("distinct ids");

        let report = composite.health().await;
        assert_eq!(report.status, HealthStatus::Failing);
        assert_eq!(
            report.detail.as_deref(),
            Some("primary health check panicked")
        );
        let components = &report.metrics.expect("aggregate metrics")["components"];
        assert_eq!(components["primary"]["status"], "failing");
        assert_eq!(components["secondary"]["status"], "ok");
        assert_eq!(components["tertiary"]["status"], "ok");
    }

    #[tokio::test]
    async fn a_panic_before_the_health_future_exists_is_still_a_failing_report() {
        let composite = StaticComposite::new(
            Fake::panicking_before_any_future("primary"),
            Fake::new("secondary"),
            Fake::new("tertiary"),
        )
        .expect("distinct ids");

        let report = composite.health().await;
        assert_eq!(report.status, HealthStatus::Failing);
        assert_eq!(
            report.detail.as_deref(),
            Some("primary health check panicked")
        );
    }

    #[tokio::test]
    async fn a_panic_before_the_shutdown_future_exists_does_not_skip_later_drains() {
        let primary = Fake::new("primary");
        let secondary = Fake::panicking_before_any_future("secondary");
        let tertiary = Fake::new("tertiary");
        let composite = StaticComposite::new(primary, secondary, tertiary).expect("distinct ids");

        // The composite's own failure-list panic is caught here so its message can be inspected.
        let mut shutdown = Box::pin(composite.shutdown());
        let payload = std::future::poll_fn(|cx| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                shutdown.as_mut().poll(cx)
            })) {
                Ok(poll) => poll.map(Ok),
                Err(payload) => std::task::Poll::Ready(Err(payload)),
            }
        })
        .await
        .expect_err("one child panic surfaces after all drains");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .expect("composite panics with a formatted failure list");
        assert_eq!(message, "secondary shutdown panicked");
        // Shutdown drains tertiary, secondary, then primary; the primary drain must still run.
        assert_eq!(composite.primary.shutdowns.load(Ordering::Relaxed), 1);
        assert_eq!(composite.tertiary.shutdowns.load(Ordering::Relaxed), 1);
    }

    /// `PanicsOnDrop` models child state whose destructor is itself faulty.
    struct PanicsOnDrop;

    impl Drop for PanicsOnDrop {
        fn drop(&mut self) {
            panic!("child drop panicked");
        }
    }

    struct ReadyButPanicsOnDrop(PanicsOnDrop);

    impl Future for ReadyButPanicsOnDrop {
        type Output = ();

        fn poll(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<()> {
            std::task::Poll::Ready(())
        }
    }

    #[tokio::test]
    async fn a_drop_panic_after_completion_is_reported_as_a_child_panic() {
        let outcome = catch_child_panic(|| ReadyButPanicsOnDrop(PanicsOnDrop)).await;
        assert_eq!(outcome, Err(ChildPanic));
    }

    #[tokio::test]
    async fn a_payload_that_panics_on_drop_stays_inside_the_boundary() {
        let outcome = catch_child_panic(|| async {
            std::panic::panic_any(PanicsOnDrop);
        })
        .await;
        assert_eq!(outcome, Err(ChildPanic));

        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            catch_child_panic(|| async {
                struct PanicsWithHostilePayloadOnDrop;
                impl Drop for PanicsWithHostilePayloadOnDrop {
                    fn drop(&mut self) {
                        std::panic::panic_any(PanicsOnDrop);
                    }
                }
                let _state = PanicsWithHostilePayloadOnDrop;
                std::future::pending::<()>().await;
            }),
        )
        .await;
        assert!(cancelled.is_err(), "only the timeout can complete");
    }

    #[tokio::test]
    async fn a_drop_panic_during_cancellation_does_not_escape() {
        let child = || async {
            let _state = PanicsOnDrop;
            std::future::pending::<()>().await;
        };
        // Cancelling the boundary while the child is pending drops the child future from the
        // timeout's teardown, not from `catch_child_panic`'s own code.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            catch_child_panic(child),
        )
        .await;
        assert!(
            outcome.is_err(),
            "the child never completes; only the timeout can"
        );
    }
}
