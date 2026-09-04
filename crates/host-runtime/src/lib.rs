//! Host runtime for the wire contract in `docs/host-wire-protocol.md`.

// `deny(unsafe_code)` permits Broca's scoped `allow` for its `pre_exec` hook.
// `PR_SET_PDEATHSIG` terminates harness children when the host dies.
#![deny(unsafe_code)]

pub mod auth;
pub mod client;
pub mod composite;
pub mod config;
pub mod connection_file;
pub mod generation;
pub mod handler;
pub mod harness_closure;
pub mod lifecycle;
#[doc(hidden)]
pub mod ring_transport;

mod connection;
mod control;
mod dispatch;
mod file_mode;
#[doc(hidden)]
pub mod frame_channel;
mod instance;
mod panic_boundary;
mod routing;
mod runtime;
#[doc(hidden)]
pub mod setup_socket;
mod store_fs;
// Ring setup and tests name raw envelope types, while the managed client API
// exposes only responses, stream items, and call errors.
#[doc(hidden)]
pub mod wire;

pub use auth::{
    AuthError, AuthStage, Authenticated, CLIENT_AUTH_DOMAIN, ClientAuth, ClientAuthenticated,
    ClientHello, DEFAULT_CLIENT_ROLE, MAX_AUTH_MESSAGE_LEN, NONCE_LEN, PROOF_LEN,
    SERVER_PROOF_DOMAIN, ServerProof, authenticate_client, authenticate_server, compute_proof,
};
pub use client::{
    CLIENT_CONTROL_QUEUE_FRAMES, CLIENT_DATA_QUEUE_FRAMES, CLIENT_FRAME_TIMEOUT,
    CLIENT_HANDSHAKE_TIMEOUT, CLIENT_MAX_LIVE_STREAMS, CLIENT_MAX_PENDING_REQUESTS,
    CLIENT_QUEUED_BYTES, CLIENT_REQUEST_TIMEOUT, CLIENT_RETAINED_RESPONSE_BYTES,
    CLIENT_ROUTE_OPEN_TIMEOUT, CLIENT_SHUTDOWN_TIMEOUT, CLIENT_STREAM_QUEUE_ITEMS, CallError,
    Client, ClientError, HostStatusSnapshot, RequestOptions, Response, ResponseStream, SendOutcome,
    StreamItem,
};
pub use composite::{
    CompositeComponent, PrimaryComponent, SecondaryComponent, ShutdownError, StaticComposite,
};
pub use config::{ConfigError, HostConfig, HostInit, HostLimits, HostTiming, LivenessPolicy};
pub use connection_file::{
    ConnectionFileError, ConnectionInfo, DAEMON_ID_LEN, KEY_LEN, MAX_CONNECTION_FILE_LEN,
    MIN_KEY_LEN, SCHEMA_VERSION, read_for_client as read_connection_file,
};
pub use handler::{
    BindOutcome, HealthReport, HealthStatus, HostHandler, InitError, ManifestSnapshot,
    OutputBuffer, RequestCtx, RequestOutcome, ResourceDeclaration, RouteClass, RouteHandle,
    RouteIdentity, RouteTarget, StreamClosed, TargetKind,
};
pub use instance::{
    CONNECTION_FILE_NAME, InstanceError, MANAGED_DIR_NAME, RUNTIME_DIR_NAME, data_dir_path,
    managed_dir_path, runtime_dir_path,
};
pub use lifecycle::{
    COORDINATION_DIR_NAME, LIFECYCLE_RECORD_NAME, LIFETIME_LOCK_NAME, LifecyclePhase,
    LifecycleProbe, LifecycleRecord, LifecycleState, LifecycleTransactionLock, NamespaceAnchor,
    PAYLOAD_MANIFEST_DIGEST_LEN, ProbeFreshness, PublicationSummary, TRANSACTION_LOCK_NAME,
    UNSUPPORTED_STATE_SCHEMA_REASON, coordination_dir_path, is_canonical_payload_digest,
    lifecycle_dir_path, probe_lifecycle,
};
pub use runtime::{HostError, run, run_with_publish_hook};
/// The version-2 body cap. Published so a consumer preparing an output can
/// gate on the same value frame admission enforces, rather than restating it.
pub use wire::MAX_FRAME_BODY_LEN;
/// `EIDNARA_LAUNCH_NONCE_ENV` and `EIDNARA_MODULE_ID_ENV` let module-side code use the names injected at spawn.
pub use wire::{EIDNARA_LAUNCH_NONCE_ENV, EIDNARA_MODULE_ID_ENV};

pub use tokio_util::sync::CancellationToken;
