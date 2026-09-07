import hostRelease from "../../../../../release/host-release.json";

/**
 * The literal unions of `release/host-release.json`, written out once so a
 * value can be typed as a member. `typeof` over the JSON module widens every
 * array to `string[]`, so these tuples are the only place a literal type
 * lives; the test beside this module asserts each tuple equals its JSON array
 * as a set, and the JSON stays the owner of the values themselves.
 */

/** `cli.commands` */
export const DAEMON_COMMANDS = ["start", "stop", "restart", "status", "doctor"] as const;

/** `cli.states` */
export const DAEMON_STATES = [
    "unavailable",
    "stopped",
    "starting",
    "running",
    "stopping",
    "wedged",
] as const;

/** `cli.check_ids` */
export const CHECK_IDS = [
    "artifact.bootstrap",
    "artifact.current_generation",
    "artifact.input_qualification",
    "artifact.native_payload",
    "compatibility.control",
    "compatibility.daemon",
    "compatibility.epochs",
    "compatibility.modules",
    "compatibility.proof",
    "credentials.broca",
    "filesystem.capacity.bootstrap",
    "filesystem.capacity.generation",
    "filesystem.permissions",
    "filesystem.support",
    "install.layout",
    "lifecycle.evidence",
    "lifecycle.fences",
    "lifecycle.publication",
    "platform.support",
    "readiness.kernel",
    "readiness.storage",
    "readiness.synapse",
    "readiness.transport",
] as const;

/** `cli.check_statuses` */
export const CHECK_STATUSES = ["pass", "fail", "warn", "skip"] as const;

/** `cli.remediations` */
export const REMEDIATIONS = [
    "align_versions",
    "free_storage",
    "inspect_daemon_process",
    "inspect_kernel_projector",
    "inspect_storage",
    "inspect_synapse",
    "install_native_payload",
    "reinstall_eidnara",
    "report_bug",
    "restart_with_supported_harness",
    "run_daemon_restart",
    "run_daemon_start",
    "set_data_directory",
    "use_supported_install_layout",
    "use_supported_platform",
    "wait_and_retry",
] as const;

/** `cli.reasons.failing_by_precedence[].id`, in precedence order */
export const FAILING_REASONS = [
    "internal_error",
    "no_data_dir",
    "unsupported_filesystem",
    "unsupported_platform",
    "unsupported_install_layout",
    "unsupported_state_schema",
    "native_payload_invalid",
    "native_payload_missing",
    "insufficient_storage",
    "native_probe_unavailable",
    "wedged",
    "publication_invalid",
    "publication_stale",
    "publication_missing",
    "authentication_failed",
    "unsupported_proof_version",
    "incompatible_control",
    "incompatible_daemon",
    "incompatible_module",
    "incompatible_epochs",
    "shutdown_timeout",
    "startup_timeout",
    "lifecycle_busy",
    "storage_unavailable",
    "kernel_unavailable",
    "storage_starting",
    "kernel_starting",
    "synapse_degraded",
    "synapse_starting",
    "harness_unavailable",
    "stopping",
    "starting",
    "not_running",
] as const;

/** `cli.reasons.non_failing` */
export const NON_FAILING_REASONS = [
    "already_running",
    "already_stopped",
    "healthy",
    "kernel_capacity_warn",
    "kernel_lagging",
    "no_required_consumer",
    "started",
    "stopped",
    "synapse_unsupported",
] as const;

/** `cli.readiness_states.transport` */
export const TRANSPORT_READINESS_STATES = ["ready", "starting", "unavailable"] as const;

/** `cli.readiness_states.storage` */
export const STORAGE_READINESS_STATES = ["ready", "starting", "unavailable"] as const;

/** `cli.readiness_states.synapse` */
export const SYNAPSE_READINESS_STATES = ["ready", "starting", "degraded", "unsupported"] as const;

/** `cli.readiness_states.kernel` */
export const KERNEL_READINESS_STATES = ["ready", "starting", "unavailable"] as const;

/** `harness_unavailable.reasons_by_precedence[].id`, in precedence order */
export const HARNESS_UNAVAILABLE_REASONS = [
    "descriptor_absent",
    "descriptor_invalid",
    "closure_incomplete",
    "argument_variant_invalid",
    "provider_unsupported",
    "auth_mechanism_unsupported",
    "credential_missing",
    "credential_value_too_large",
    "credential_snapshot_mismatch",
] as const;

/** `install_layouts` */
export const INSTALL_LAYOUTS = [
    "bun_physical_link",
    "compiled_bun_external",
    "npm_hoisted",
    "npm_nested",
] as const;

/** `epochs` keys */
export const EPOCH_NAMES = [
    "compartment_render",
    "memory_render",
    "profile_claude_code_anthropic",
    "state_sync",
    "tagger",
] as const;

/** `versions.modules` keys */
export const MODULE_KEYS = ["broca", "context", "synapse"] as const;

export type DaemonCommand = (typeof DAEMON_COMMANDS)[number];
export type DaemonState = (typeof DAEMON_STATES)[number];
export type CheckId = (typeof CHECK_IDS)[number];
export type CheckStatus = (typeof CHECK_STATUSES)[number];
export type Remediation = (typeof REMEDIATIONS)[number];
export type FailingReason = (typeof FAILING_REASONS)[number];
export type NonFailingReason = (typeof NON_FAILING_REASONS)[number];
export type TransportReadinessState = (typeof TRANSPORT_READINESS_STATES)[number];
export type StorageReadinessState = (typeof STORAGE_READINESS_STATES)[number];
export type SynapseReadinessState = (typeof SYNAPSE_READINESS_STATES)[number];
export type KernelReadinessState = (typeof KERNEL_READINESS_STATES)[number];
export type HarnessUnavailableReason = (typeof HARNESS_UNAVAILABLE_REASONS)[number];
export type InstallLayout = (typeof INSTALL_LAYOUTS)[number];
export type EpochName = (typeof EPOCH_NAMES)[number];
export type ModuleKey = (typeof MODULE_KEYS)[number];

/** Every tuple paired with the JSON array it mirrors, for the equality test. */
export const VOCABULARY_SOURCES: ReadonlyArray<{
    name: string;
    tuple: readonly string[];
    json: readonly string[];
}> = [
    { name: "cli.commands", tuple: DAEMON_COMMANDS, json: hostRelease.cli.commands },
    { name: "cli.states", tuple: DAEMON_STATES, json: hostRelease.cli.states },
    { name: "cli.check_ids", tuple: CHECK_IDS, json: hostRelease.cli.check_ids },
    { name: "cli.check_statuses", tuple: CHECK_STATUSES, json: hostRelease.cli.check_statuses },
    { name: "cli.remediations", tuple: REMEDIATIONS, json: hostRelease.cli.remediations },
    {
        name: "cli.reasons.failing_by_precedence",
        tuple: FAILING_REASONS,
        json: hostRelease.cli.reasons.failing_by_precedence.map((entry) => entry.id),
    },
    {
        name: "cli.reasons.non_failing",
        tuple: NON_FAILING_REASONS,
        json: hostRelease.cli.reasons.non_failing,
    },
    {
        name: "cli.readiness_states.transport",
        tuple: TRANSPORT_READINESS_STATES,
        json: hostRelease.cli.readiness_states.transport,
    },
    {
        name: "cli.readiness_states.storage",
        tuple: STORAGE_READINESS_STATES,
        json: hostRelease.cli.readiness_states.storage,
    },
    {
        name: "cli.readiness_states.synapse",
        tuple: SYNAPSE_READINESS_STATES,
        json: hostRelease.cli.readiness_states.synapse,
    },
    {
        name: "cli.readiness_states.kernel",
        tuple: KERNEL_READINESS_STATES,
        json: hostRelease.cli.readiness_states.kernel,
    },
    {
        name: "harness_unavailable.reasons_by_precedence",
        tuple: HARNESS_UNAVAILABLE_REASONS,
        json: hostRelease.harness_unavailable.reasons_by_precedence.map((entry) => entry.id),
    },
    { name: "install_layouts", tuple: INSTALL_LAYOUTS, json: hostRelease.install_layouts },
    { name: "epochs", tuple: EPOCH_NAMES, json: Object.keys(hostRelease.epochs) },
    {
        name: "versions.modules",
        tuple: MODULE_KEYS,
        json: Object.keys(hostRelease.versions.modules),
    },
];
