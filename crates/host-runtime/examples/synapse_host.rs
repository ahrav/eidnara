//! Smoke host that publishes a Synapse lane and waits for a signal.
//!
//! Usage:
//!
//! `synapse_host <data-dir> <bundle-dir|-> [<ort-library> <ort-library-sha256>]`
//!
//! Pass `-` as `<bundle-dir>` to run without a bundle (degraded-lane smoke); the ORT arguments are then unused and may be omitted.

use std::time::Duration;

use host_runtime::synapse::{SynapseComponent, SynapseConfig, SynapseLimits};
use host_runtime::{
    BindOutcome, CancellationToken, CompositeComponent, HealthReport, HostConfig, HostInit,
    HostLimits, InitError, ManifestSnapshot, PrimaryComponent, RequestCtx, RequestOutcome,
    RouteHandle, RouteIdentity, SecondaryComponent, ShutdownError, StaticComposite,
};

struct EchoPrimary;

impl CompositeComponent for EchoPrimary {
    fn manifest(&self) -> ManifestSnapshot {
        ManifestSnapshot {
            module_id: "context".to_owned(),
            module_version: env!("CARGO_PKG_VERSION").to_owned(),
            provides: vec![serde_json::json!({
                "role": "tool_provider",
                "tools": [{
                    "name": "echo",
                    "execution_mode": "pure",
                    "schema": {"type": "object"}
                }]
            })],
            control_ops: Vec::new(),
        }
    }

    async fn bind(&self, _route: RouteHandle, _identity: RouteIdentity) -> BindOutcome {
        BindOutcome::Accept
    }

    async fn handle(&self, ctx: RequestCtx) -> RequestOutcome {
        let Ok(mut body) = ctx.reserve_output(ctx.body.len()).await else {
            return RequestOutcome::error("internal_error", "output reservation unavailable");
        };
        body.extend_from_slice(&ctx.body)
            .expect("reservation matches request length");
        RequestOutcome::Response {
            body,
            binary: ctx.binary,
        }
    }

    async fn route_gone(&self, _route: RouteHandle) {}

    async fn health(&self) -> HealthReport {
        HealthReport::ok()
    }

    async fn shutdown(&self) -> Result<(), ShutdownError> {
        Ok(())
    }
}

impl PrimaryComponent for EchoPrimary {
    async fn initialize(&self, _init: HostInit) -> Result<(), InitError> {
        Ok(())
    }
}

struct PlaceholderBroca;

impl CompositeComponent for PlaceholderBroca {
    fn manifest(&self) -> ManifestSnapshot {
        ManifestSnapshot {
            module_id: "broca".to_owned(),
            module_version: env!("CARGO_PKG_VERSION").to_owned(),
            provides: vec![serde_json::json!({"role": "management_surface"})],
            control_ops: Vec::new(),
        }
    }

    async fn bind(&self, _route: RouteHandle, _identity: RouteIdentity) -> BindOutcome {
        BindOutcome::Reject {
            code: "artifact_invalid".to_owned(),
            message: "broca is unavailable in this smoke host".to_owned(),
        }
    }

    async fn handle(&self, _ctx: RequestCtx) -> RequestOutcome {
        RequestOutcome::error("internal_error", "unreachable: broca binds are rejected")
    }

    async fn route_gone(&self, _route: RouteHandle) {}

    async fn health(&self) -> HealthReport {
        HealthReport::ok()
    }

    async fn shutdown(&self) -> Result<(), ShutdownError> {
        Ok(())
    }
}

impl SecondaryComponent for PlaceholderBroca {
    async fn initialize(&self) -> Result<(), InitError> {
        Ok(())
    }
}

const STARTUP_BUDGET: Duration = Duration::from_secs(10);

/// `(device, inode, mtime)` of the connection file, or `None` while it is absent.
/// The mtime guards against an unlinked inode number being reused by the new publication.
fn publication_identity(
    path: &std::path::Path,
) -> Option<(u64, u64, Option<std::time::SystemTime>)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino(), metadata.modified().ok()))
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let usage =
        "usage: synapse_host <data-dir> <bundle-dir|-> [<ort-library> <ort-library-sha256>]";
    let data_dir = std::path::PathBuf::from(args.next().expect(usage));
    let bundle_dir = args.next().expect(usage);

    // The ORT arguments only matter when a bundle is loaded; the degraded-lane smoke runs without them.
    let synapse_config = (bundle_dir != "-").then(|| SynapseConfig {
        bundle_dir: std::path::PathBuf::from(bundle_dir),
        bundle_manifest_sha256: None,
        ort_library: std::path::PathBuf::from(args.next().expect(usage)),
        ort_library_sha256: args.next().expect(usage),
        limits: SynapseLimits::default(),
    });
    let synapse = SynapseComponent::new(synapse_config);
    let composite = StaticComposite::new(EchoPrimary, synapse, PlaceholderBroca)
        .expect("composite module IDs are distinct");

    // The lane declares its retained-result cap as retained resident bytes, so the host budget must cover it above the floor plus one inbound body.
    let config = HostConfig {
        data_dir: Some(data_dir.clone()),
        daemon_ver: "eidnara-host/synapse-smoke".to_owned(),
        limits: HostLimits {
            max_resident_bytes: host_runtime::config::MIN_RESIDENT_BYTES
                + u64::from(host_runtime::MAX_FRAME_BODY_LEN)
                + SynapseLimits::default().max_retained_result_bytes,
            ..Default::default()
        },
        ..Default::default()
    };
    let publication = data_dir
        .join("eidnara")
        .join("run")
        .join(host_runtime::CONNECTION_FILE_NAME);
    // A predecessor's connection file can still be present in a reused data directory. The host publishes by rename, so a new publication is a new inode; readiness waits for a file whose identity differs from the stale one rather than for any file to exist.
    let stale_publication = publication_identity(&publication);
    let shutdown = CancellationToken::new();
    let host = tokio::spawn(host_runtime::run(composite, config, shutdown.clone()));

    // A host that stays alive without publishing must fail the smoke run instead of hanging it.
    let startup_deadline = tokio::time::Instant::now() + STARTUP_BUDGET;
    loop {
        if host.is_finished() {
            let result = host.await;
            eprintln!("host exited before publishing: {result:?}");
            std::process::exit(1);
        }
        if tokio::time::Instant::now() >= startup_deadline {
            eprintln!("host did not publish within {STARTUP_BUDGET:?}");
            std::process::exit(1);
        }
        if let Some(current) = publication_identity(&publication)
            && Some(current) != stale_publication
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    println!("READY {}", publication.display());

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }
    shutdown.cancel();
    match host.await {
        Ok(Ok(())) => println!("SHUTDOWN graceful"),
        Ok(Err(error)) => {
            eprintln!("SHUTDOWN failed: {error}");
            std::process::exit(1);
        }
        Err(join) => {
            eprintln!("SHUTDOWN join failed: {join}");
            std::process::exit(1);
        }
    }
}
